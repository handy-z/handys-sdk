//! `OffsetStore`: the offsets disk cache, filled from offsets.imtheo.lol with ETag revalidation and 404 backoff.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::offsets::{OffsetKeys, Offsets, parse_offsets, validate_offsets};

/// status 0 = network failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResult {
    pub status: i32,
    pub body: String,
    pub etag: String,
}

/// `(url, if_none_match)`.
pub type HttpGet = Arc<dyn Fn(&str, &str) -> HttpResult + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffsetStatus {
    Ready,
    WaitingForOffsets,
    Gated,
    Invalid,
}

const MAX_BODY: usize = 1 << 20;

/// 30, 60, 120, 240, 300 s cap.
pub(crate) fn backoff(attempt: i32) -> Duration {
    let attempt = attempt.max(0);
    Duration::from_secs(if attempt >= 4 { 300 } else { 30 << attempt })
}

/// The top-level `clientVersionUpload` of the client-version API.
pub(crate) fn parse_client_version(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v.get("clientVersionUpload")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq)]
pub struct OffsetResult {
    pub status: OffsetStatus,
    pub offsets: Option<Offsets>,
    pub message: String,
    pub retry_in: Duration,
}

fn result(status: OffsetStatus, offsets: Option<Offsets>, message: impl Into<String>, retry_in: u64) -> OffsetResult {
    OffsetResult {
        status,
        offsets,
        message: message.into(),
        retry_in: Duration::from_secs(retry_in),
    }
}

fn hash_of(v: &str) -> &str {
    v.strip_prefix("version-").unwrap_or(v)
}

/// The hash becomes a file name, so refuse anything path-like.
fn safe_name(h: &str) -> bool {
    !h.is_empty() && h.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Missing or unreadable reads as empty; invalid UTF-8 is replaced, so a garbage file just fails to parse.
fn slurp(p: &Path) -> String {
    std::fs::read(p).map(String::from_utf8_lossy_owned).unwrap_or_default()
}

pub struct OffsetStore {
    cache_dir: PathBuf,
    get: HttpGet,
    required: OffsetKeys,
    not_found: HashMap<String, i32>,
}

impl OffsetStore {
    pub fn new(cache_dir: impl Into<PathBuf>, get: HttpGet, required: &[&'static str]) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            get,
            required: required.to_vec(),
            not_found: HashMap::new(),
        }
    }

    fn file(&self, version: &str, ext: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.{ext}", hash_of(version)))
    }

    fn read_cache(&self, version: &str) -> Option<Offsets> {
        let p = self.file(version, "json");
        if !p.exists() {
            return None;
        }
        let o = parse_offsets(&slurp(&p)).ok()?;
        validate_offsets(&o, version, &self.required).ok().map(|()| o)
    }

    /// Cache first, else GET.
    pub fn load(&mut self, version: &str) -> OffsetResult {
        if !safe_name(hash_of(version)) {
            return result(OffsetStatus::Invalid, None, "bad version string", 0);
        }
        if let Some(o) = self.read_cache(version) {
            return result(OffsetStatus::Ready, Some(o), "cache", 0);
        }
        self.fetch(version, "")
    }

    /// Conditional GET.
    pub(crate) fn revalidate(&mut self, version: &str) -> OffsetResult {
        if !safe_name(hash_of(version)) {
            return result(OffsetStatus::Invalid, None, "bad version string", 0);
        }
        let etag = if self.read_cache(version).is_some() {
            slurp(&self.file(version, "etag"))
        } else {
            String::new()
        };
        self.fetch(version, &etag)
    }

    pub(crate) fn prune(&self, keep: usize) {
        let Ok(dir) = std::fs::read_dir(&self.cache_dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, PathBuf)> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| {
                (
                    p.metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH),
                    p,
                )
            })
            .collect();
        if files.len() <= keep {
            return;
        }
        files.sort_by_key(|a| std::cmp::Reverse(a.0));
        for (_, p) in &files[keep..] {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_file(p.with_extension("etag"));
        }
    }

    pub(crate) fn reset_backoff(&mut self) {
        self.not_found.clear();
    }

    fn fetch(&mut self, version: &str, etag: &str) -> OffsetResult {
        let r = (self.get)(&format!("https://offsets.imtheo.lol/{version}/offsets.json"), etag);
        if r.status == 200 {
            if r.body.len() > MAX_BODY {
                return result(OffsetStatus::Invalid, None, "offsets body too large", 0);
            }
            let o = match parse_offsets(&r.body).and_then(|o| validate_offsets(&o, version, &self.required).map(|()| o))
            {
                Ok(o) => o,
                Err(e) => return result(OffsetStatus::Invalid, None, e, 0),
            };
            let _ = std::fs::create_dir_all(&self.cache_dir);
            let _ = std::fs::write(self.file(version, "json"), &r.body);
            let _ = std::fs::write(self.file(version, "etag"), &r.etag);
            self.not_found.remove(version);
            return result(OffsetStatus::Ready, Some(o), "downloaded", 0);
        }
        // Any non-200: a still-valid cached copy beats waiting (304, offline, server hiccup).
        if let Some(o) = self.read_cache(version) {
            return result(OffsetStatus::Ready, Some(o), format!("cache (status {})", r.status), 0);
        }
        match r.status {
            404 => {
                let n = self.not_found.entry(version.to_owned()).or_insert(0);
                let wait = backoff(*n);
                *n += 1;
                OffsetResult {
                    status: OffsetStatus::WaitingForOffsets,
                    offsets: None,
                    message: "offsets not published yet".into(),
                    retry_in: wait,
                }
            }
            403 => result(OffsetStatus::Gated, None, "offsets access gated (403)", 600),
            0 => result(OffsetStatus::WaitingForOffsets, None, "network failure", 30),
            s => result(OffsetStatus::WaitingForOffsets, None, format!("http {s}"), 30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::offsets::SDK_KEYS;
    use crate::roblox::offsets::tests::sdk_json;
    use std::sync::Mutex;

    const VER: &str = "version-9ac8595712b447a7";

    fn body(ver: &str) -> String {
        sdk_json(ver, None, 8)
    }

    struct Served {
        status: i32,
        calls: i32,
        body: String,
        etag: String,
        last_if_none: String,
    }

    struct Fix {
        dir: PathBuf,
        st: Arc<Mutex<Served>>,
    }

    impl Fix {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("handy-sdk-rs-updater-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            let st = Served {
                status: 200,
                calls: 0,
                body: body(VER),
                etag: "W/\"x\"".into(),
                last_if_none: String::new(),
            };
            Self {
                dir,
                st: Arc::new(Mutex::new(st)),
            }
        }
        fn store(&self) -> OffsetStore {
            let st = self.st.clone();
            let get: HttpGet = Arc::new(move |_: &str, inm: &str| {
                let mut s = st.lock().unwrap();
                s.calls += 1;
                s.last_if_none = inm.to_owned();
                let ok = s.status == 200;
                HttpResult {
                    status: s.status,
                    body: if ok { s.body.clone() } else { String::new() },
                    etag: if ok { s.etag.clone() } else { String::new() },
                }
            });
            OffsetStore::new(&self.dir, get, &SDK_KEYS)
        }
        fn with(&self, f: impl FnOnce(&mut Served)) {
            f(&mut self.st.lock().unwrap());
        }
        fn calls(&self) -> i32 {
            self.st.lock().unwrap().calls
        }
        fn last_if_none(&self) -> String {
            self.st.lock().unwrap().last_if_none.clone()
        }
        fn cached(&self, ext: &str) -> PathBuf {
            self.dir.join(format!("9ac8595712b447a7.{ext}"))
        }
    }

    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn read_all(p: &Path) -> String {
        String::from_utf8(std::fs::read(p).unwrap()).unwrap()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn updater_200_caches_and_ready() {
        let f = Fix::new("200");
        let mut s = f.store();
        assert_eq!(s.load(VER).status, OffsetStatus::Ready);
        assert!(f.cached("json").exists());
        assert!(f.cached("etag").exists());
        s.load(VER);
        assert_eq!(f.calls(), 1); // second load is a cache hit
    }

    #[test]
    fn updater_304_uses_cache() {
        let f = Fix::new("304");
        let mut s = f.store();
        let first = s.load(VER);
        f.with(|s| s.status = 304);
        let r = s.revalidate(VER);
        assert_eq!(r.status, OffsetStatus::Ready);
        assert_eq!(f.last_if_none(), "W/\"x\"");
        assert!(r.offsets.is_some() && r.offsets.unwrap().values == first.offsets.unwrap().values);
    }

    #[test]
    fn updater_404_waits_with_backoff() {
        let f = Fix::new("404");
        f.with(|s| s.status = 404);
        let mut s = f.store();
        let r = s.load(VER);
        assert_eq!(r.status, OffsetStatus::WaitingForOffsets);
        assert_eq!(r.retry_in, secs(30));
        assert_eq!(s.load(VER).retry_in, secs(60));
        for (i, want) in [30, 60, 120, 240, 300, 300].into_iter().enumerate() {
            assert_eq!(backoff(i as i32), secs(want));
        }
    }

    #[test]
    fn updater_403_gated() {
        let f = Fix::new("403");
        f.with(|s| s.status = 403);
        let r = f.store().load(VER);
        assert_eq!(r.status, OffsetStatus::Gated);
        assert_eq!(r.retry_in, secs(600));
    }

    #[test]
    fn updater_rejects_version_mismatch() {
        let f = Fix::new("mismatch");
        f.with(|s| s.body = body("version-1111111111111111"));
        assert_eq!(f.store().load(VER).status, OffsetStatus::Invalid);
        assert!(!f.cached("json").exists());
    }

    #[test]
    fn updater_rejects_oversize() {
        let f = Fix::new("oversize");
        f.with(|s| s.body = body(VER) + &" ".repeat(1 << 20));
        assert_eq!(f.store().load(VER).status, OffsetStatus::Invalid);
    }

    #[test]
    fn updater_offline_uses_cache() {
        let f = Fix::new("offline");
        let mut s = f.store();
        assert_eq!(s.load(VER).status, OffsetStatus::Ready);
        f.with(|s| s.status = 0);
        assert_eq!(s.revalidate(VER).status, OffsetStatus::Ready);
        std::fs::remove_file(f.cached("json")).unwrap();
        assert_eq!(s.load(VER).status, OffsetStatus::WaitingForOffsets);
    }

    #[test]
    fn updater_prune_keeps_newest_5() {
        let f = Fix::new("prune");
        let s = f.store();
        std::fs::create_dir_all(&f.dir).unwrap();
        let now = SystemTime::now();
        for i in 0..7u64 {
            let p = f.dir.join(format!("h{i}.json"));
            std::fs::write(&p, "{}").unwrap();
            std::fs::write(f.dir.join(format!("h{i}.etag")), "e").unwrap();
            let file = std::fs::File::options().write(true).open(&p).unwrap();
            file.set_modified(now - Duration::from_secs((10 - i) * 3600)).unwrap(); // h6 newest
        }
        s.prune(5);
        assert!(!f.dir.join("h0.json").exists() && !f.dir.join("h0.etag").exists());
        assert!(!f.dir.join("h1.json").exists());
        for i in 2..7 {
            assert!(f.dir.join(format!("h{i}.json")).exists());
        }
    }

    #[test]
    fn client_version_parse() {
        let v =
            parse_client_version(r#"{"version":"0.745.0.7451137","clientVersionUpload":"version-5a21b87bbc2641cf"}"#);
        assert_eq!(v.as_deref(), Some("version-5a21b87bbc2641cf"));
        assert!(parse_client_version("{}").is_none());
    }

    #[test]
    fn updater_rejects_path_like_version() {
        let f = Fix::new("pathlike");
        let mut s = f.store();
        for v in ["version-../x", "version-a/b", "version-a\\b", "version-..", ""] {
            assert_eq!(s.load(v).status, OffsetStatus::Invalid);
            assert_eq!(s.revalidate(v).status, OffsetStatus::Invalid);
        }
        assert_eq!(f.calls(), 0);
    }

    #[test]
    fn updater_bad_cache_is_refetched() {
        for (variant, junk) in [
            b"\x01garbage\xff{{".to_vec(),
            body("version-1111111111111111").into_bytes(),
        ]
        .into_iter()
        .enumerate()
        {
            let f = Fix::new(&format!("badcache{variant}"));
            let mut s = f.store();
            std::fs::create_dir_all(&f.dir).unwrap();
            std::fs::write(f.cached("json"), junk).unwrap();
            assert_eq!(s.load(VER).status, OffsetStatus::Ready);
            assert_eq!(f.calls(), 1);
            assert_eq!(read_all(&f.cached("json")), body(VER));
        }
    }

    #[test]
    fn updater_revalidate_200_replaces_cache() {
        let f = Fix::new("reval200");
        let mut s = f.store();
        s.load(VER);
        f.with(|s| {
            s.body = body(VER) + " "; // still valid, different bytes
            s.etag = "W/\"y\"".into();
        });
        assert_eq!(s.revalidate(VER).status, OffsetStatus::Ready);
        assert_eq!(f.last_if_none(), "W/\"x\"");
        assert_eq!(read_all(&f.cached("json")), body(VER) + " ");
        assert_eq!(read_all(&f.cached("etag")), "W/\"y\"");
    }

    #[test]
    fn updater_404_backoff_resets_on_attach() {
        // prefetch 404s must not lengthen the next attach's wait
        let f = Fix::new("404reset");
        f.with(|s| s.status = 404);
        let mut s = f.store();
        assert_eq!(s.load(VER).retry_in, secs(30));
        assert_eq!(s.load(VER).retry_in, secs(60));
        s.reset_backoff();
        assert_eq!(s.load(VER).retry_in, secs(30));
    }

    // Form: 404, 404, 200 (counter reset), cache removed, 404 again -> back to 30s.
    #[test]
    fn updater_404_counter_resets_after_success() {
        let f = Fix::new("404success");
        f.with(|s| s.status = 404);
        let mut s = f.store();
        assert_eq!(s.load(VER).retry_in, secs(30));
        assert_eq!(s.load(VER).retry_in, secs(60));
        f.with(|s| s.status = 200);
        assert_eq!(s.load(VER).status, OffsetStatus::Ready);
        std::fs::remove_file(f.cached("json")).unwrap();
        f.with(|s| s.status = 404);
        assert_eq!(s.load(VER).retry_in, secs(30));
    }
}
