//! The `Updater`: the background state machine (RobloxClosed -> Validating -> WaitingForOffsets / OffsetsGated /
//! Invalid -> Preparing -> Ready) that attaches to Roblox and keeps a `Session` with offsets that fit it.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::roblox::instance::{child_by_class, data_model, instance_class};
use crate::roblox::offsets::{OffsetKeys, Offsets, SDK_KEYS};
use crate::roblox::process::RobloxProcess;
use crate::roblox::updater::{HttpGet, OffsetStatus, OffsetStore, parse_client_version};
use crate::roblox::version::{
    VersionCandidate, VersionInputs, hash_from_api, hash_from_logs, learned_version, player_guid_from_state,
    remember_version, state_file_candidates, version_candidates,
};
use crate::ui::Tone;

/// Preparing = offsets resolved, UpdaterHooks::prepare is running (e.g. a game-specific setup step).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppStatus {
    RobloxClosed,
    Validating,
    WaitingForOffsets,
    OffsetsGated,
    Invalid,
    Preparing,
    Ready,
}

/// Dashboard label.
pub fn status_name(s: AppStatus) -> &'static str {
    status_name_with(s, "Preparing")
}

/// `preparing` names what the game's prepare hook does (e.g. "Calibrating").
pub fn status_name_with(s: AppStatus, preparing: &'static str) -> &'static str {
    match s {
        AppStatus::RobloxClosed => "Waiting for Roblox",
        AppStatus::Validating => "Checking Roblox version",
        AppStatus::WaitingForOffsets => "Waiting for offsets",
        AppStatus::OffsetsGated => "Offsets access denied",
        AppStatus::Invalid => "Offsets don't fit this Roblox",
        AppStatus::Preparing => preparing,
        AppStatus::Ready => "Ready",
    }
}

/// Header dot: grey closed, amber on the way, green ready, red stuck.
pub fn status_tone(s: AppStatus) -> Tone {
    match s {
        AppStatus::RobloxClosed => Tone::Neutral,
        AppStatus::Ready => Tone::Good,
        AppStatus::OffsetsGated | AppStatus::Invalid => Tone::Bad,
        _ => Tone::Busy,
    }
}

/// None = pass, else the name of the failing check.
pub type SmokeFailure = Option<String>;
pub type SmokeTest = Box<dyn Fn(&RobloxProcess, &Offsets) -> SmokeFailure + Send + Sync>;

/// Checks: DataModel class, Workspace, Camera, Players.
pub fn production_smoke_test(p: &RobloxProcess, o: &Offsets) -> SmokeFailure {
    if !o.has_all(&SDK_KEYS) {
        return Some("missing offset key".into());
    }
    let dm = data_model(p, o);
    if dm == 0 || instance_class(p, o, dm).as_deref() != Some("DataModel") {
        return Some("DataModel class".into());
    }
    let ws = child_by_class(p, o, dm, "Workspace");
    if ws == 0 {
        return Some("Workspace".into());
    }
    if child_by_class(p, o, ws, "Camera") == 0 {
        return Some("Camera".to_owned());
    }
    if child_by_class(p, o, dm, "Players") == 0 {
        return Some("Players".into());
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolveResult {
    pub status: OffsetStatus,
    pub hash: String,
    pub message: String,
    pub offsets: Option<Offsets>,
    pub retry_in: Duration,
}

impl Default for ResolveResult {
    fn default() -> Self {
        Self {
            status: OffsetStatus::Invalid,
            hash: String::new(),
            message: String::new(),
            offsets: None,
            retry_in: Duration::from_secs(30),
        }
    }
}

/// First candidate that loads and passes `smoke` wins; its hash is remembered for `file_version` in `versions_ini`.
/// `more` (slow sources: network, log scan) is called once, only after every candidate so far failed; repeats are
/// skipped.
pub(crate) fn resolve_offsets(
    first: &[VersionCandidate],
    store: &mut OffsetStore,
    smoke: &dyn Fn(&Offsets) -> SmokeFailure,
    versions_ini: &Path,
    file_version: &str,
    stop: Option<&dyn Fn() -> bool>,
    more: Option<&dyn Fn() -> Vec<VersionCandidate>>,
) -> ResolveResult {
    let mut out = ResolveResult::default();
    let mut cands = first.to_vec();
    let (mut waiting, mut gated, mut extended) = (false, false, false);
    let mut retry = Duration::ZERO;
    let stopped = || stop.is_some_and(|s| s());
    let mut i = 0;
    loop {
        if i == cands.len()
            && !extended
            && let Some(more) = more
            && !stopped()
        {
            extended = true;
            for c in more() {
                if !cands.iter().any(|d| d.hash == c.hash) {
                    cands.push(c);
                }
            }
        }
        if i == cands.len() {
            break;
        }
        let c = cands[i].clone();
        i += 1;
        if stopped() {
            out.message += "stopped";
            return out;
        }
        let r = store.load(&c.hash);
        if r.status == OffsetStatus::Ready
            && let Some(offsets) = r.offsets
        {
            match smoke(&offsets) {
                None => {
                    // Weak sources are never learned: a probe or an old log could pin the wrong hash to this version.
                    if c.source != "cache-probe" && c.source != "logs" {
                        remember_version(versions_ini, file_version, &c.hash);
                    }
                    out.status = OffsetStatus::Ready;
                    out.message = format!("{} ({})", c.hash, c.source);
                    out.hash = c.hash;
                    out.offsets = Some(offsets);
                    return out;
                }
                Some(fail) => {
                    out.message += &format!("{} ({}): smoke test failed: {fail}; ", c.hash, c.source);
                    continue;
                }
            }
        }
        waiting |= r.status == OffsetStatus::WaitingForOffsets;
        gated |= r.status == OffsetStatus::Gated;
        if r.status != OffsetStatus::Invalid && (retry.is_zero() || r.retry_in < retry) {
            retry = r.retry_in;
        }
        out.message += &format!("{} ({}): {}; ", c.hash, c.source, r.message);
    }
    if cands.is_empty() {
        out.message = "no version candidates".into();
    }
    out.status = if gated {
        OffsetStatus::Gated
    } else if waiting {
        OffsetStatus::WaitingForOffsets
    } else {
        OffsetStatus::Invalid
    };
    if !retry.is_zero() {
        out.retry_in = retry;
    }
    out
}

#[derive(Default)]
pub struct Session {
    pub proc: RobloxProcess,
    pub offsets: Offsets,
    /// Offsets a game found at runtime (e.g. a color or layout value); carried over when revalidated offsets replace
    /// these.
    pub calibrated: HashMap<String, i64>,
}

#[derive(Default)]
pub struct UpdaterHooks {
    /// After offsets resolve and before Ready (status Preparing); may fill `session.calibrated`.
    /// Returning false abandons this Roblox process (it is expected to have exited or been stopped). The third
    /// argument waits up to the duration; it returns false once Roblox exits or stop is requested.
    pub prepare: Option<Box<dyn Fn(&mut Session, &str, &dyn Fn(Duration) -> bool) -> bool + Send + Sync>>,
    /// About every 100 ms while Ready, on the updater thread.
    pub tick: Option<Box<dyn Fn(&Arc<Session>) + Send + Sync>>,
}

pub type ProcessFinder = Box<dyn Fn() -> Option<RobloxProcess> + Send + Sync>;

struct State {
    status: AppStatus,
    detail: String,
    retry_at: Option<Instant>,
}

pub struct Updater {
    get: HttpGet,
    smoke: SmokeTest,
    dir: PathBuf,
    required: OffsetKeys,
    hooks: UpdaterHooks,
    finder: ProcessFinder,
    state: Mutex<State>,
    session: Mutex<Option<Arc<Session>>>,
}

const CLIENT_VERSION_URL: &str = "https://clientsettings.roblox.com/v2/client-version/WindowsPlayer";

#[derive(PartialEq)]
enum WaitResult {
    Timeout,
    Exited,
    Stopped,
}

fn wait_for(stop: &AtomicBool, p: &RobloxProcess, d: Duration) -> WaitResult {
    if stop.load(Relaxed) {
        return WaitResult::Stopped;
    }
    if p.wait_exit(d.as_millis().try_into().unwrap_or(u32::MAX)) {
        return WaitResult::Exited;
    }
    if stop.load(Relaxed) {
        WaitResult::Stopped
    } else {
        WaitResult::Timeout
    }
}

fn read_file(f: &Path) -> String {
    std::fs::read(f).map(String::from_utf8_lossy_owned).unwrap_or_default()
}

/// The API's and the logs' hash for one attached process, once found: neither changes while it runs, so a retry
/// during game load need not ask again.
type SlowHints = RefCell<[Option<String>; 2]>;

/// Found once, then from `seen`.
fn remembered(seen: &SlowHints, i: usize, find: impl FnOnce() -> Option<String>) -> Option<String> {
    if seen.borrow()[i].is_none() {
        let h = find();
        seen.borrow_mut()[i] = h;
    }
    seen.borrow()[i].clone()
}

/// Without `slow`: only the exe path, versions.ini and State.json, no network, log scan or cache probe.
fn candidates_for(
    p: &RobloxProcess,
    get: &HttpGet,
    cache_dir: &Path,
    versions_ini: &Path,
    slow: Option<&SlowHints>,
) -> Vec<VersionCandidate> {
    let exe_dir = Path::new(p.exe_path()).parent().unwrap_or(Path::new(""));
    let mut inp = VersionInputs {
        exe_path: p.exe_path().to_owned(),
        file_version: p.file_version().to_owned(),
        learned: Some(Box::new(|| learned_version(versions_ini, p.file_version()))),
        state: Some(Box::new(|| {
            state_file_candidates(exe_dir)
                .iter()
                .find_map(|f| player_guid_from_state(&read_file(f)))
        })),
        ..Default::default()
    };
    let Some(seen) = slow else {
        return version_candidates(&inp);
    };
    inp.api = Some(Box::new(|| {
        remembered(seen, 0, || {
            let r = get(CLIENT_VERSION_URL, "");
            if r.status == 200 {
                hash_from_api(&r.body, p.file_version())
            } else {
                None
            }
        })
    }));
    inp.logs = Some(Box::new(|| {
        remembered(seen, 1, || {
            let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
            hash_from_logs(&PathBuf::from(local).join("Roblox").join("logs"), p.file_version())
        })
    }));
    let mut cached: Vec<(SystemTime, String)> = std::fs::read_dir(cache_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let t = p.metadata().and_then(|m| m.modified()).ok()?;
            Some((t, format!("version-{}", p.file_stem()?.to_string_lossy())))
        })
        .collect();
    cached.sort_by_key(|a| std::cmp::Reverse(a.0));
    inp.cached_hashes = cached.into_iter().map(|c| c.1).collect();
    version_candidates(&inp)
}

impl Updater {
    /// `data_dir` holds versions.ini and offsets/imtheo; `required_keys` must include SDK_KEYS.
    pub fn new(
        get: HttpGet,
        smoke: impl Fn(&RobloxProcess, &Offsets) -> SmokeFailure + Send + Sync + 'static,
        data_dir: impl Into<PathBuf>,
        required_keys: &[&'static str],
    ) -> Self {
        Self {
            get,
            smoke: Box::new(smoke),
            dir: data_dir.into(),
            required: required_keys.to_vec(),
            hooks: UpdaterHooks::default(),
            finder: Box::new(RobloxProcess::find),
            state: Mutex::new(State {
                status: AppStatus::RobloxClosed,
                detail: String::new(),
                retry_at: None,
            }),
            session: Mutex::new(None),
        }
    }

    pub fn with_hooks(mut self, hooks: UpdaterHooks) -> Self {
        self.hooks = hooks;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_finder(mut self, finder: impl Fn() -> Option<RobloxProcess> + Send + Sync + 'static) -> Self {
        self.finder = Box::new(finder);
        self
    }

    pub fn status(&self) -> AppStatus {
        self.state.lock().unwrap().status
    }

    pub fn detail(&self) -> String {
        let st = self.state.lock().unwrap();
        match st.retry_at {
            None => st.detail.clone(),
            Some(at) => format!(
                "{}retry in {}s",
                st.detail,
                at.saturating_duration_since(Instant::now()).as_secs()
            ),
        }
    }

    pub fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().unwrap().clone()
    }

    fn set(&self, status: AppStatus, detail: impl Into<String>, retry_at: Option<Instant>) {
        *self.state.lock().unwrap() = State {
            status,
            detail: detail.into(),
            retry_at,
        };
    }

    fn store_session(&self, s: Option<Arc<Session>>) {
        *self.session.lock().unwrap() = s;
    }

    pub fn run(&self, stop: &AtomicBool) {
        let mut store = OffsetStore::new(
            self.dir.join("offsets").join("imtheo"),
            self.get.clone(),
            &self.required,
        );
        while !stop.load(Relaxed) {
            let Some(p) = (self.finder)() else {
                self.set(AppStatus::RobloxClosed, "start Roblox and join a game", None);
                std::thread::sleep(Duration::from_millis(250));
                continue;
            };
            self.attach(&p, &mut store, stop);
            self.store_session(None);
            if !stop.load(Relaxed) {
                self.set(AppStatus::RobloxClosed, "Roblox exited", None);
            }
        }
    }

    /// Returns when the process exits or stop is requested.
    fn attach(&self, proc: &RobloxProcess, store: &mut OffsetStore, stop: &AtomicBool) {
        let versions_ini = self.dir.join("versions.ini");
        let cache_dir = self.dir.join("offsets").join("imtheo");
        store.reset_backoff(); // 404s from the last session's prefetch must not lengthen this wait
        let attached_at = Instant::now();
        let smoke = |o: &Offsets| (self.smoke)(proc, o);
        let stopped = || stop.load(Relaxed);
        let seen = SlowHints::default();
        let more = || candidates_for(proc, &self.get, &cache_dir, &versions_ini, Some(&seen));
        let res = loop {
            self.set(AppStatus::Validating, proc.file_version(), None);
            let cands = candidates_for(proc, &self.get, &cache_dir, &versions_ini, None);
            if stop.load(Relaxed) {
                return;
            }
            let res = resolve_offsets(
                &cands,
                store,
                &smoke,
                &versions_ini,
                proc.file_version(),
                Some(&stopped),
                Some(&more),
            );
            if res.status == OffsetStatus::Ready {
                break res;
            }
            if stop.load(Relaxed) {
                return;
            }
            let s = match res.status {
                OffsetStatus::Gated => AppStatus::OffsetsGated,
                OffsetStatus::Invalid => AppStatus::Invalid,
                _ => AppStatus::WaitingForOffsets,
            };
            let mut until = Instant::now() + res.retry_in;
            // Smoke checks fail until Roblox has loaded the game; recheck soon after launch instead of in 30 s.
            if res.status == OffsetStatus::Invalid && attached_at.elapsed() < Duration::from_secs(120) {
                until = Instant::now() + Duration::from_secs(2);
            }
            self.set(s, res.message, Some(until));
            while Instant::now() < until {
                if wait_for(stop, proc, Duration::from_millis(250)) != WaitResult::Timeout {
                    return;
                }
            }
        };

        let Some(offsets) = res.offsets else { return }; // never: Ready always carries offsets
        let version = format!("{}, Dumped At: {}", res.message, offsets.dumped_at);
        let mut sess = Session {
            proc: proc.clone(),
            offsets,
            calibrated: HashMap::new(),
        };
        if let Some(prepare) = &self.hooks.prepare {
            self.set(AppStatus::Preparing, version.clone(), None);
            let wait = |d: Duration| wait_for(stop, proc, d) == WaitResult::Timeout;
            if !prepare(&mut sess, &res.hash, &wait) {
                return;
            }
        }
        let mut sess = Arc::new(sess);
        self.store_session(Some(sess.clone()));
        self.set(AppStatus::Ready, version, None);

        let (mut last_reval, mut last_prefetch) = (Instant::now(), Instant::now());
        loop {
            if wait_for(stop, proc, Duration::from_millis(100)) != WaitResult::Timeout {
                return;
            }
            if let Some(tick) = &self.hooks.tick {
                tick(&sess);
            }
            let now = Instant::now();
            if now - last_reval >= Duration::from_secs(30 * 60) {
                last_reval = now;
                let r = store.revalidate(&res.hash);
                if r.status == OffsetStatus::Ready
                    && let Some(o) = r.offsets
                    && o.values != sess.offsets.values
                    && (self.smoke)(proc, &o).is_none()
                {
                    sess = Arc::new(Session {
                        proc: proc.clone(),
                        offsets: o,
                        calibrated: sess.calibrated.clone(),
                    });
                    self.store_session(Some(sess.clone()));
                }
            }
            if now - last_prefetch >= Duration::from_secs(10 * 60) {
                last_prefetch = now;
                let r = (self.get)(CLIENT_VERSION_URL, "");
                if r.status == 200
                    && let Some(v) = parse_client_version(&r.body)
                {
                    store.load(&v);
                }
                store.prune(5);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roblox::http::win_http_get;
    use crate::roblox::instance::{children, instance_name};
    use crate::roblox::memory::{ClassNameCache, Memory};
    use crate::roblox::offsets::tests::sdk_json;
    use crate::roblox::updater::HttpResult;
    use crate::util::thread::StopThread;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicI32;

    const A: &str = "version-aaaaaaaaaaaaaaaa";
    const B: &str = "version-bbbbbbbbbbbbbbbb";

    fn cand(hash: &str, source: &'static str) -> VersionCandidate {
        VersionCandidate {
            hash: hash.into(),
            source,
        }
    }

    fn pass(_: &Offsets) -> SmokeFailure {
        None
    }

    struct Fix {
        dir: PathBuf,
    }

    impl Fix {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("handy-sdk-rs-updater-thread-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn store(&self, served: &[&'static str]) -> OffsetStore {
            let served = served.to_vec();
            let get: HttpGet = Arc::new(
                move |url: &str, _: &str| match served.iter().find(|v| url.contains(**v)) {
                    Some(v) => HttpResult {
                        status: 200,
                        body: sdk_json(v, None, 8),
                        etag: "e".into(),
                    },
                    None => HttpResult {
                        status: 404,
                        ..Default::default()
                    },
                },
            );
            OffsetStore::new(self.dir.join("cache"), get, &SDK_KEYS)
        }
        fn ini(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn version_wrong_hint_rejected_by_smoke_test() {
        let f = Fix::new("wronghint");
        let mut s = f.store(&[A, B]);
        let smoke = |o: &Offsets| {
            if o.roblox_version == B {
                None
            } else {
                Some("wrong".to_owned())
            }
        };
        let r = resolve_offsets(
            &[cand(A, "path"), cand(B, "state")],
            &mut s,
            &smoke,
            &f.ini("versions.ini"),
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.status, OffsetStatus::Ready);
        assert_eq!(r.hash, B);
        assert_eq!(r.offsets.map(|o| o.roblox_version).as_deref(), Some(B));
        assert_eq!(learned_version(&f.ini("versions.ini"), "0.1.2.3").as_deref(), Some(B));
    }

    #[test]
    fn updater_resolve_does_not_remember_probe_or_logs_hits() {
        let f = Fix::new("noremember");
        let mut s = f.store(&[A, B]);
        for (h, src) in [(A, "cache-probe"), (B, "logs")] {
            let r = resolve_offsets(
                &[cand(h, src)],
                &mut s,
                &pass,
                &f.ini("versions.ini"),
                "0.1.2.3",
                None,
                None,
            );
            assert_eq!(r.status, OffsetStatus::Ready);
            assert_eq!(r.hash, h);
        }
        assert!(learned_version(&f.ini("versions.ini"), "0.1.2.3").is_none());
    }

    #[test]
    fn updater_resolve_first_pass_wins_and_stops() {
        let f = Fix::new("firstpass");
        let mut s = f.store(&[A, B]);
        let smokes = Cell::new(0);
        let smoke = |_: &Offsets| {
            smokes.set(smokes.get() + 1);
            None
        };
        let r = resolve_offsets(
            &[cand(A, "path"), cand(B, "state")],
            &mut s,
            &smoke,
            &f.ini("versions.ini"),
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.hash, A);
        assert_eq!(smokes.get(), 1);
    }

    #[test]
    fn updater_resolve_asks_slow_sources_only_after_the_rest_fail() {
        let f = Fix::new("slow");
        let mut s = f.store(&[A, B]);
        let (asked, smokes) = (Cell::new(0), Cell::new(0));
        let more = || {
            asked.set(asked.get() + 1);
            vec![cand(A, "api"), cand(B, "logs")]
        };
        let ini = f.ini("v.ini");
        let r = resolve_offsets(&[cand(A, "path")], &mut s, &pass, &ini, "0.1.2.3", None, Some(&more));
        assert_eq!(r.hash, A);
        assert_eq!(asked.get(), 0);
        let smoke = |o: &Offsets| {
            smokes.set(smokes.get() + 1);
            if o.roblox_version == B {
                None
            } else {
                Some("wrong".to_owned())
            }
        };
        let r = resolve_offsets(&[cand(A, "path")], &mut s, &smoke, &ini, "0.1.2.3", None, Some(&more));
        assert_eq!(r.hash, B);
        assert_eq!(asked.get(), 1);
        assert_eq!(smokes.get(), 2); // A from `more` is not tried twice
        let r = resolve_offsets(&[], &mut s, &pass, &ini, "0.1.2.3", None, Some(&more));
        assert_eq!(r.hash, A);
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn updater_resolve_none_pass_waits_and_lists_candidates() {
        let f = Fix::new("nonepass");
        let mut s = f.store(&[A]);
        let r = resolve_offsets(
            &[cand(A, "path"), cand(B, "state")],
            &mut s,
            &|_| Some("nope".to_owned()),
            &f.ini("versions.ini"),
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.status, OffsetStatus::WaitingForOffsets); // B is unpublished (404)
        assert!(r.offsets.is_none());
        assert!(r.message.contains(A) && r.message.contains(B));
        assert!(!r.retry_in.is_zero());
        assert!(learned_version(&f.ini("versions.ini"), "0.1.2.3").is_none());
    }

    #[test]
    fn updater_resolve_smoke_failure_only_is_invalid() {
        let f = Fix::new("smokeonly");
        let mut s = f.store(&[A]);
        let ini = f.ini("v.ini");
        let r = resolve_offsets(
            &[cand(A, "path")],
            &mut s,
            &|_| Some("nope".to_owned()),
            &ini,
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.status, OffsetStatus::Invalid);
        assert_eq!(
            resolve_offsets(&[], &mut s, &pass, &ini, "x", None, None).status,
            OffsetStatus::Invalid
        );
    }

    #[test]
    fn updater_resolve_stops_between_candidates() {
        let f = Fix::new("stops");
        let calls = Arc::new(AtomicI32::new(0));
        let c = calls.clone();
        let get: HttpGet = Arc::new(move |_: &str, _: &str| {
            c.fetch_add(1, Relaxed);
            HttpResult::default()
        });
        let mut s = OffsetStore::new(f.dir.join("cache"), get, &SDK_KEYS);
        let stop = || calls.load(Relaxed) >= 1;
        let r = resolve_offsets(
            &[cand(A, "path"), cand(B, "state")],
            &mut s,
            &pass,
            &f.ini("v.ini"),
            "0.1.2.3",
            Some(&stop),
            None,
        );
        assert_eq!(calls.load(Relaxed), 1);
        assert_ne!(r.status, OffsetStatus::Ready);
    }

    #[test]
    fn updater_run_without_roblox_stays_closed_in_injected_dir() {
        let f = Fix::new("run");
        let hook_calls = Arc::new(AtomicI32::new(0));
        let (h1, h2) = (hook_calls.clone(), hook_calls.clone());
        let hooks = UpdaterHooks {
            prepare: Some(Box::new(move |_, _, _| {
                h1.fetch_add(1, Relaxed);
                true
            })),
            tick: Some(Box::new(move |_| {
                h2.fetch_add(1, Relaxed);
            })),
        };
        let u = Arc::new(
            Updater::new(
                Arc::new(|_: &str, _: &str| HttpResult::default()),
                |_, _| None,
                &f.dir,
                &SDK_KEYS,
            )
            .with_hooks(hooks)
            .with_finder(|| None),
        );
        {
            let u = u.clone();
            let _t = StopThread::spawn(move |st| u.run(st));
            std::thread::sleep(Duration::from_millis(400));
        }
        assert_eq!(u.status(), AppStatus::RobloxClosed);
        assert!(u.session().is_none());
        assert_eq!(hook_calls.load(Relaxed), 0); // hooks only run once a process has offsets
    }

    #[test]
    fn updater_resolve_message_names_failing_smoke_check() {
        let f = Fix::new("smokemsg");
        let mut s = f.store(&[A]);
        let r = resolve_offsets(
            &[cand(A, "path")],
            &mut s,
            &|_| Some("Camera".to_owned()),
            &f.ini("v.ini"),
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.status, OffsetStatus::Invalid);
        assert!(r.message.contains("Camera"));
        assert!(r.message.contains(A));
    }

    #[test]
    fn updater_resolve_message_names_missing_offset_key() {
        let f = Fix::new("keymsg");
        let key = SDK_KEYS[0];
        let field = format!("\"{}\":8", key.split_once('.').unwrap().1);
        let b = sdk_json(A, None, 8).replacen(&field, "\"Zzz\":8", 1);
        let get: HttpGet = Arc::new(move |_: &str, _: &str| HttpResult {
            status: 200,
            body: b.clone(),
            etag: "e".into(),
        });
        let mut s = OffsetStore::new(f.dir.join("cache"), get, &SDK_KEYS);
        let r = resolve_offsets(
            &[cand(A, "path")],
            &mut s,
            &pass,
            &f.ini("v.ini"),
            "0.1.2.3",
            None,
            None,
        );
        assert_eq!(r.status, OffsetStatus::Invalid);
        assert!(r.message.contains(key), "{}", r.message);
    }

    #[test]
    fn updater_slow_hints_found_once_missing_asked_again() {
        let seen = SlowHints::default();
        let calls = std::cell::Cell::new(0);
        let ask = |v: Option<&str>| {
            calls.set(calls.get() + 1);
            v.map(str::to_owned)
        };
        assert_eq!(remembered(&seen, 0, || ask(None)), None); // offline: asked again next time
        assert_eq!(
            remembered(&seen, 0, || ask(Some("version-a"))).as_deref(),
            Some("version-a")
        );
        assert_eq!(
            remembered(&seen, 0, || ask(Some("version-b"))).as_deref(),
            Some("version-a")
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn status_name_labels_preparing_per_game() {
        assert_eq!(status_name(AppStatus::Ready), "Ready");
        assert_eq!(status_name(AppStatus::Preparing), "Preparing");
        assert_eq!(
            status_name_with(AppStatus::Preparing, "Calibrating cooldown"),
            "Calibrating cooldown"
        );
    }

    #[test]
    fn status_tone_colors_the_header() {
        assert_eq!(status_tone(AppStatus::RobloxClosed), Tone::Neutral);
        assert_eq!(status_tone(AppStatus::Validating), Tone::Busy);
        assert_eq!(status_tone(AppStatus::Preparing), Tone::Busy);
        assert_eq!(status_tone(AppStatus::Invalid), Tone::Bad);
        assert_eq!(status_tone(AppStatus::Ready), Tone::Good);
    }

    // Forwards to the process and counts read() calls.
    struct CountingMemory<'a> {
        p: &'a RobloxProcess,
        reads: Cell<usize>,
    }

    impl Memory for CountingMemory<'_> {
        fn read(&self, addr: usize, out: &mut [u8]) -> bool {
            self.reads.set(self.reads.get() + 1);
            self.p.read(addr, out)
        }
        fn class_names(&self) -> Option<&ClassNameCache> {
            self.p.class_names()
        }
    }

    fn walk(m: &dyn Memory, o: &Offsets, root: usize, limit: usize) -> usize {
        let (mut seen, mut q) = (0, VecDeque::from([root]));
        while seen < limit
            && let Some(i) = q.pop_front()
        {
            let _ = instance_name(m, o, i);
            let _ = instance_class(m, o, i);
            seen += 1;
            q.extend(children(m, o, i));
        }
        seen
    }

    // Live benchmark: needs a running Roblox client + network. Walks the running game's Workspace reading every
    // instance's name and class, and prints reads and time per walk.
    #[test]
    #[ignore = "needs a running Roblox and the network"]
    fn live_read_bench() {
        if RobloxProcess::find().is_none() {
            println!("  SKIP: Roblox not running");
            return;
        }
        let dir = std::env::temp_dir().join("handy_sdk_rs_live_bench");
        let u = Arc::new(Updater::new(
            Arc::new(win_http_get),
            production_smoke_test,
            dir,
            &SDK_KEYS,
        ));
        let s = {
            let u2 = u.clone();
            let _t = StopThread::spawn(move |st| u2.run(st));
            for _ in 0..300 {
                if u.status() == AppStatus::Ready {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            u.session()
        };
        let s = s.expect("a session");
        let ws = child_by_class(&s.proc, &s.offsets, data_model(&s.proc, &s.offsets), "Workspace");
        assert_ne!(ws, 0);
        let m = CountingMemory {
            p: &s.proc,
            reads: Cell::new(0),
        };
        for run in 1..=3 {
            // run 1 fills the class-name cache
            m.reads.set(0);
            let t0 = Instant::now();
            let n = walk(&m, &s.offsets, ws, 50000);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let reads = m.reads.get();
            println!(
                "  walk {run}: {n} instances, {reads} reads ({:.2} each), {ms:.1} ms ({:.2} us each)",
                reads as f64 / n as f64,
                1000.0 * ms / n as f64
            );
        }
    }
}
