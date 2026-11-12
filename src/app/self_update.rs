use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};

use crate::app::crash_guard::local_time;
use crate::roblox::http::win_http_get_capped;
use crate::ui::Tone;
use crate::util::paths;

const API_HEADERS: &str = "Accept: application/vnd.github+json";
// Per WinHTTP step, so a slow but moving download is fine; the check runs in the background, so it never delays startup.
const TIMEOUT_MS: i32 = 10000;
const API_CAP: usize = 1024 * 1024;
const SHA_CAP: usize = 4096;
const EXE_CAP: usize = 32 * 1024 * 1024;

/// What `SelfUpdate::start` found.
pub enum Startup {
    /// A staged update was swapped in and started: exit at once.
    Restarting,
    Running(Arc<SelfUpdate>),
}

/// Where the update stands; `SelfUpdate::badge` turns it into the dashboard badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// No check: a development build or another copy already running.
    Disabled,
    Checking,
    Latest,
    /// This copy was just installed by an update.
    Updated,
    Downloading {
        percent: u8,
    },
    /// Verified and staged as `<exe>.new`: installs on `finish` after `request_restart`, or at the next start.
    Ready {
        version: String,
    },
    /// The release check got no HTTP response at all.
    Offline,
    Failed {
        reason: String,
    },
}

/// The dashboard badge: `text` in the pill, a dot colored by `tone`, `tip` on hover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
    pub tip: String,
}

/// Updates the app from its GitHub releases (https://api.github.com/repos/<owner>/<repo>/releases/latest): a tag
/// `v<semver>` newer than the running version with the assets `<exe_name>` and `<exe_name>.sha256` (first 64 hex chars,
/// any case) is downloaded in the background, SHA-256-verified and staged next to the exe; it is swapped in on
/// `finish` after `request_restart`, or at the next `start`. Installs, stages and failures go to `update.log`.
pub struct SelfUpdate {
    owner: String,
    repo: String,
    current: String,
    exe: Option<PathBuf>,
    state: Mutex<UpdateState>,
    restart: AtomicBool,
}

impl SelfUpdate {
    /// Call first in main, before the single-instance mutex (a new copy started from here must not find it held).
    /// `enabled` is false for development builds or when another copy is running: then nothing is checked or installed
    /// and the state is Disabled. Cleans up an update's leftovers; if enabled and an update is staged, swaps it in,
    /// starts it with the same arguments and gives Restarting (the caller exits at once). Otherwise gives Running and,
    /// if enabled, starts one detached background thread that checks for, downloads, verifies and stages a newer
    /// release.
    pub fn start(owner: &str, repo: &str, current: &str, enabled: bool) -> Startup {
        let exe = std::env::current_exe().ok();
        let mut just_updated = None; // Some: check in the background
        if let Some(exe) = &exe {
            let Some(updated) = prepare(exe, current, enabled, start_exe) else {
                return Startup::Restarting;
            };
            just_updated = enabled.then_some(updated);
        } else if enabled {
            log("?", current, "?", "current_exe failed");
        }
        let state = match just_updated {
            Some(_) => UpdateState::Checking,
            None if enabled => UpdateState::Failed {
                reason: "current_exe failed".into(),
            },
            None => UpdateState::Disabled,
        };
        let me = Arc::new(Self::new(owner, repo, current, exe, state));
        if let Some(just_updated) = just_updated {
            let bg = me.clone();
            // detached: it never blocks shutdown, and a later start deletes the .part it may leave
            let spawned = std::thread::Builder::new().name("self-update".into()).spawn(move || {
                let Some(exe) = bg.exe.clone() else { return };
                let url = format!("https://api.github.com/repos/{}/{}/releases/latest", bg.owner, bg.repo);
                check(&exe, &bg.current, &url, just_updated, &real_fetch, &|s| bg.set(s));
            });
            if let Err(e) = spawned {
                me.set(UpdateState::Failed {
                    reason: format!("thread: {e}"),
                });
            }
        }
        Startup::Running(me)
    }

    fn new(owner: &str, repo: &str, current: &str, exe: Option<PathBuf>, state: UpdateState) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            current: current.into(),
            exe,
            state: Mutex::new(state),
            restart: AtomicBool::new(false),
        }
    }

    fn set(&self, s: UpdateState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Any thread.
    pub fn state(&self) -> UpdateState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn badge(&self) -> Badge {
        badge(&self.current, &self.state())
    }

    /// The release notes of the newest version known (a staged one, else the running one).
    pub fn release_url(&self) -> String {
        let ver = match self.state() {
            UpdateState::Ready { version } => version,
            _ => self.current.clone(),
        };
        let base = format!("https://github.com/{}/{}/releases", self.owner, self.repo);
        if ver.is_empty() {
            base
        } else {
            format!("{base}/tag/v{ver}")
        }
    }

    /// Asks `finish` to install the staged update; only when Ready. True when accepted (the app should then close).
    pub fn request_restart(&self) -> bool {
        let ready = matches!(self.state(), UpdateState::Ready { .. });
        if ready {
            self.restart.store(true, Ordering::Relaxed);
        }
        ready
    }

    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::Relaxed)
    }

    /// Call at the very end of main, after the app has shut down (keys released, threads joined, single-instance mutex
    /// released): if a restart was requested and an update is staged, swaps it in and starts it.
    pub fn finish(&self) {
        let Some(exe) = &self.exe else { return };
        if self.restart_requested() && with_ext(exe, ".new").exists() {
            let app = exe.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
            let msg = swap_and_start(exe, start_exe).map_or_else(|e| e, |()| "installed on restart".into());
            log(app, &self.current, "staged", &msg);
        }
    }
}

/// Text, tone and tip of the badge for `state` with `current` running (`SelfUpdate::badge` for the live state).
pub fn badge(current: &str, state: &UpdateState) -> Badge {
    let (tail, tone, tip) = match state {
        UpdateState::Disabled => (
            String::new(),
            Tone::Neutral,
            "Development build: no update check".into(),
        ),
        UpdateState::Checking => (
            "checking\u{2026}".into(),
            Tone::Neutral,
            "Checking GitHub for a newer version".into(),
        ),
        UpdateState::Latest => (
            "latest".into(),
            Tone::Good,
            "Up to date \u{b7} click for release notes".into(),
        ),
        UpdateState::Updated => (
            "updated".into(),
            Tone::Good,
            format!("Just updated to v{current} \u{b7} click for release notes"),
        ),
        UpdateState::Downloading { percent } => (
            format!("downloading {percent}%"),
            Tone::Busy,
            "Downloading the new version".into(),
        ),
        UpdateState::Ready { version } => (
            format!("v{version} ready"),
            Tone::Busy,
            format!("Click to restart into v{version} (or it installs at next start)"),
        ),
        UpdateState::Offline => ("offline".into(), Tone::Neutral, "Could not reach GitHub".into()),
        UpdateState::Failed { reason } => ("update failed".into(), Tone::Bad, reason.replace('\n', " ")),
    };
    let text = if tail.is_empty() {
        format!("v{current}")
    } else {
        format!("v{current} \u{b7} {tail}")
    };
    Badge { text, tone, tip }
}

/// url, extra headers, body cap, per-step timeout, progress (bytes so far, Content-Length) -> (status, body).
type Fetch<'f> = dyn Fn(&str, &str, usize, i32, &mut dyn FnMut(usize, Option<usize>)) -> (i32, Vec<u8>) + 'f;

fn real_fetch(
    url: &str,
    headers: &str,
    cap: usize,
    timeout_ms: i32,
    progress: &mut dyn FnMut(usize, Option<usize>),
) -> (i32, Vec<u8>) {
    win_http_get_capped(url, headers, cap, timeout_ms, progress)
}

/// The background check: reports Checking, then Latest/Updated, Offline, Failed, or Downloading.. then Ready once
/// the verified exe is staged as `<exe>.new`. Holds no lock itself; `report` only swaps the state.
fn check(exe: &Path, current: &str, api_url: &str, just_updated: bool, fetch: &Fetch, report: &dyn Fn(UpdateState)) {
    report(UpdateState::Checking);
    let app = exe.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
    let (status, body) = fetch(api_url, API_HEADERS, API_CAP, TIMEOUT_MS, &mut |_, _| {});
    if status == 0 {
        report(UpdateState::Offline);
        return;
    }
    let release = match ok_body(status, body, API_CAP, "release check")
        .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| format!("release JSON: {e}")))
    {
        Ok(r) => r,
        Err(reason) => {
            log(app, current, "?", &reason);
            report(UpdateState::Failed { reason });
            return;
        }
    };
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, current) {
        report(if just_updated {
            UpdateState::Updated
        } else {
            UpdateState::Latest
        });
        return;
    }
    let latest = &tag[1..];
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    match download(&release, name, fetch, report).and_then(|bytes| stage(exe, &bytes)) {
        Ok(()) => {
            log(app, current, latest, "staged");
            report(UpdateState::Ready { version: latest.into() });
        }
        Err(reason) => {
            log(app, current, latest, &reason);
            report(UpdateState::Failed { reason });
        }
    }
}

/// The release's exe for `name`, SHA-256-verified against its .sha256 asset.
fn download(release: &Value, name: &str, fetch: &Fetch, report: &dyn Fn(UpdateState)) -> Result<Vec<u8>, String> {
    let (exe_url, sha_url) = pick_assets(release, name)?;
    let (status, body) = fetch(&sha_url, "", SHA_CAP, TIMEOUT_MS, &mut |_, _| {});
    let sha_file = ok_body(status, body, SHA_CAP, ".sha256")?;
    let want = parse_sha256(&String::from_utf8_lossy(&sha_file)).ok_or(".sha256: no SHA-256 at its start")?;
    report(UpdateState::Downloading { percent: 0 });
    let mut shown = 0u8;
    let (status, body) = fetch(&exe_url, "", EXE_CAP, TIMEOUT_MS, &mut |got, total| {
        if let Some(total) = total.filter(|&t| t > 0) {
            let percent = (got.saturating_mul(100) / total).min(100) as u8;
            if percent != shown {
                shown = percent;
                report(UpdateState::Downloading { percent });
            }
        }
    });
    let bytes = ok_body(status, body, EXE_CAP, name)?;
    if sha256(&bytes)? != want {
        return Err("checksum mismatch".into());
    }
    Ok(bytes)
}

/// Status 200 and at most `cap` bytes, or why not.
fn ok_body(status: i32, body: Vec<u8>, cap: usize, what: &str) -> Result<Vec<u8>, String> {
    if status != 200 {
        return Err(format!("{what}: HTTP status {status}"));
    }
    if body.len() > cap {
        return Err(format!("{what}: larger than {cap} bytes"));
    }
    Ok(body)
}

/// Writes verified `bytes` as `<exe>.part`, then renames it to `<exe>.new`, so a cut-off write is never staged.
fn stage(exe: &Path, bytes: &[u8]) -> Result<(), String> {
    let (part, new) = (with_ext(exe, ".part"), with_ext(exe, ".new"));
    let res = fs::write(&part, bytes)
        .map_err(|e| format!("write .part: {e}"))
        .and_then(|()| fs::rename(&part, &new).map_err(|e| format!("rename .part: {e}")));
    if res.is_err() {
        let _ = fs::remove_file(&part);
    }
    res
}

/// Cleans up leftovers, then (if enabled) installs a staged update: None when it was swapped in and started (exit at
/// once), else whether this copy was just installed by an update. A staged update that fails to install is logged
/// and deleted.
fn prepare(exe: &Path, current: &str, enabled: bool, start: impl FnOnce(&Path) -> Result<(), String>) -> Option<bool> {
    let just_updated = cleanup(exe, enabled);
    if enabled && with_ext(exe, ".new").exists() {
        let app = exe.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
        match swap_and_start(exe, start) {
            Ok(()) => {
                log(app, current, "staged", "installed at start");
                return None;
            }
            Err(e) => {
                log(app, current, "staged", &e);
                let _ = fs::remove_file(with_ext(exe, ".new"));
            }
        }
    }
    Some(just_updated)
}

/// Starts `exe` with this process's arguments, detached so nothing waits on it.
fn start_exe(exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::DETACHED_PROCESS;
    Command::new(exe)
        .args(std::env::args_os().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS.0)
        .spawn()
        .map(drop)
        .map_err(|e| format!("start: {e}"))
}

fn with_ext(exe: &Path, ext: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(ext);
    s.into()
}

/// Deletes the previous version's `<exe>.old` and (when `enabled`; a disabled copy may run beside one that is
/// downloading) an unfinished `<exe>.part`; true when `.old` was there, i.e. this copy was just installed. The old
/// process may still be exiting (a running exe cannot be deleted), so this retries for up to 2 s, then gives up.
fn cleanup(exe: &Path, enabled: bool) -> bool {
    let old = with_ext(exe, ".old");
    let just_updated = old.exists();
    let deadline = Instant::now() + Duration::from_secs(2);
    let part = enabled.then(|| with_ext(exe, ".part"));
    for p in std::iter::once(old).chain(part) {
        while p.exists() && fs::remove_file(&p).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    just_updated
}

/// Puts the staged `<exe>.new` in place of `exe` (Windows allows renaming a running exe) and calls `start` on it. On
/// any failure the original exe is back at `exe` and `.new` is gone.
fn swap_and_start(exe: &Path, start: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let (new, old) = (with_ext(exe, ".new"), with_ext(exe, ".old"));
    if let Err(e) = fs::rename(exe, &old) {
        let _ = fs::remove_file(&new);
        return Err(format!("rename to .old: {e}"));
    }
    if let Err(e) = fs::rename(&new, exe) {
        let _ = fs::rename(&old, exe);
        let _ = fs::remove_file(&new);
        return Err(format!("rename .new: {e}"));
    }
    if let Err(e) = start(exe) {
        // One replacing rename, so there is never a moment without an exe at `exe`.
        let _ = fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// `MAJOR.MINOR.PATCH`, digits only; anything else (a pre-release suffix too) is None.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

/// Whether `tag` is `v<version>` with a version above `current`.
fn is_newer(tag: &str, current: &str) -> bool {
    match (tag.strip_prefix('v').and_then(parse_version), parse_version(current)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

/// The `browser_download_url`s of the assets `<exe_name>` and `<exe_name>.sha256`.
fn pick_assets(release: &Value, exe_name: &str) -> Result<(String, String), String> {
    let url = |name: &str| {
        release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"] == name)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("release has no asset {name}"))
    };
    Ok((url(exe_name)?, url(&format!("{exe_name}.sha256"))?))
}

/// The SHA-256 the file starts with (64 hex chars, any case; `sha256sum` and `Get-FileHash` output both work).
fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let hex = text.get(..64)?;
    if text[64..].starts_with(|c: char| c.is_ascii_hexdigit()) {
        return None; // longer than a SHA-256
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        let pair = &hex[i * 2..i * 2 + 2];
        if !pair.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        *b = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    let st = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if st.is_ok() {
        Ok(out)
    } else {
        Err(format!("BCryptHash: {:#x}", st.0))
    }
}

/// One line in update.log. Every error is ignored.
fn log(app: &str, current: &str, latest: &str, msg: &str) {
    use std::io::Write;
    if !paths::has_app_name() {
        return; // app_data_dir would panic
    }
    let line = format!(
        "{} {app} {current} -> {latest}: {}\n",
        local_time(),
        msg.replace('\n', " ")
    );
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::app_data_dir().join("update.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn temp_exe(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("handy-sdk-self-update-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("handys-x.exe");
        fs::write(&exe, b"old").unwrap();
        exe
    }

    fn release(tag: &str) -> Vec<u8> {
        serde_json::json!({
            "tag_name": tag,
            "assets": [
                {"name": "handys-x.exe", "browser_download_url": "https://h/handys-x.exe"},
                {"name": "handys-x.exe.sha256", "browser_download_url": "https://h/handys-x.exe.sha256"}
            ]
        })
        .to_string()
        .into_bytes()
    }

    /// Runs `check` for 0.1.0 against fake responses (api, .sha256 file, exe) and gives the states it reported.
    fn run(exe: &Path, just_updated: bool, api: (i32, Vec<u8>), sha: &str, bin: (i32, &[u8])) -> Vec<UpdateState> {
        let seen = RefCell::new(Vec::new());
        let fetch = |url: &str, _: &str, _: usize, _: i32, progress: &mut dyn FnMut(usize, Option<usize>)| {
            if url == "https://api" {
                api.clone()
            } else if url.ends_with(".sha256") {
                (200, sha.as_bytes().to_vec())
            } else {
                progress(1, Some(bin.1.len()));
                progress(1, Some(bin.1.len())); // the same percent again: not reported twice
                progress(bin.1.len(), Some(bin.1.len()));
                (bin.0, bin.1.to_vec())
            }
        };
        check(exe, "0.1.0", "https://api", just_updated, &fetch, &|s| {
            seen.borrow_mut().push(s)
        });
        seen.into_inner()
    }

    #[test]
    fn self_update_check_stages_a_newer_release() {
        let exe = temp_exe("check-newer");
        let states = run(&exe, false, (200, release("v0.2.0")), ABC_SHA256, (200, b"abc"));
        use UpdateState::*;
        assert_eq!(
            states,
            [
                Checking,
                Downloading { percent: 0 },
                Downloading { percent: 33 },
                Downloading { percent: 100 },
                Ready {
                    version: "0.2.0".into()
                }
            ]
        );
        assert_eq!(fs::read(with_ext(&exe, ".new")).unwrap(), b"abc");
        assert!(!with_ext(&exe, ".part").exists());
        assert_eq!(fs::read(&exe).unwrap(), b"old"); // nothing installed yet
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_check_outcomes() {
        let exe = temp_exe("check-outcomes");
        let last = |just_updated, api, sha: &str, bin| run(&exe, just_updated, api, sha, bin).pop().unwrap();
        let failed = |s: UpdateState, want: &str| matches!(s, UpdateState::Failed { reason } if reason.contains(want));
        let v010 = || (200, release("v0.1.0"));
        assert_eq!(
            run(&exe, false, v010(), "", (200, b"")),
            [UpdateState::Checking, UpdateState::Latest]
        );
        assert_eq!(last(true, v010(), "", (200, b"")), UpdateState::Updated);
        assert_eq!(last(false, (0, vec![]), "", (200, b"")), UpdateState::Offline);
        assert!(failed(last(false, (404, vec![]), "", (200, b"")), "HTTP status 404"));
        assert!(failed(
            last(false, (200, b"<html>".to_vec()), "", (200, b"")),
            "release JSON"
        ));
        assert!(failed(
            last(false, (200, release("v0.2.0")), "nope", (200, b"abc")),
            ".sha256"
        ));
        // a cut-off download is a failure, not offline (the release check got through)
        assert!(failed(
            last(false, (200, release("v0.2.0")), ABC_SHA256, (0, b"ab")),
            "HTTP status 0"
        ));
        assert!(failed(
            last(false, (200, release("v0.2.0")), ABC_SHA256, (200, b"abd")),
            "checksum mismatch"
        ));
        assert!(!with_ext(&exe, ".new").exists() && !with_ext(&exe, ".part").exists()); // nothing staged
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_stage_via_part() {
        let exe = temp_exe("stage");
        fs::write(with_ext(&exe, ".new"), b"older stage").unwrap();
        stage(&exe, b"abc").unwrap();
        assert_eq!(fs::read(with_ext(&exe, ".new")).unwrap(), b"abc");
        assert!(!with_ext(&exe, ".part").exists());
        fs::remove_file(with_ext(&exe, ".new")).unwrap();
        fs::create_dir(with_ext(&exe, ".part")).unwrap(); // the write fails
        assert!(stage(&exe, b"abc").unwrap_err().contains("write .part"));
        assert!(!with_ext(&exe, ".new").exists());
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_prepare_applies_staged() {
        let exe = temp_exe("prepare");
        // disabled: a staged update stays for the copy that is allowed to install it
        fs::write(with_ext(&exe, ".new"), b"new").unwrap();
        assert_eq!(prepare(&exe, "0.1.0", false, |_| panic!("must not start")), Some(false));
        assert!(with_ext(&exe, ".new").exists());
        // enabled: swapped in and started
        let started = RefCell::new(None);
        assert_eq!(
            prepare(&exe, "0.1.0", true, |p| {
                *started.borrow_mut() = Some(p.to_path_buf());
                Ok(())
            }),
            None
        );
        assert_eq!(started.borrow().as_deref(), Some(exe.as_path()));
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        // the new copy's own start: the leftover says it was just updated
        assert_eq!(prepare(&exe, "0.2.0", true, |_| panic!("nothing staged")), Some(true));
        assert!(!with_ext(&exe, ".old").exists());
        // a staged update that will not start is rolled back and deleted
        fs::write(with_ext(&exe, ".new"), b"newer").unwrap();
        assert_eq!(prepare(&exe, "0.2.0", true, |_| Err("start: nope".into())), Some(false));
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert!(!with_ext(&exe, ".new").exists() && !with_ext(&exe, ".old").exists());
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_badges() {
        use UpdateState::*;
        let b = |s: UpdateState| {
            let b = badge("0.1.1", &s);
            (b.text, b.tone, b.tip)
        };
        let t = |s: &str| s.to_string();
        assert_eq!(
            b(Disabled),
            (t("v0.1.1"), Tone::Neutral, t("Development build: no update check"))
        );
        assert_eq!(b(Checking).0, "v0.1.1 \u{b7} checking\u{2026}");
        assert_eq!(b(Checking).1, Tone::Neutral);
        assert_eq!(
            b(Latest),
            (
                t("v0.1.1 \u{b7} latest"),
                Tone::Good,
                t("Up to date \u{b7} click for release notes")
            )
        );
        assert_eq!(
            b(Updated),
            (
                t("v0.1.1 \u{b7} updated"),
                Tone::Good,
                t("Just updated to v0.1.1 \u{b7} click for release notes")
            )
        );
        assert_eq!(b(Downloading { percent: 42 }).0, "v0.1.1 \u{b7} downloading 42%");
        assert_eq!(b(Downloading { percent: 42 }).1, Tone::Busy);
        assert_eq!(
            b(Ready { version: t("0.1.2") }),
            (
                t("v0.1.1 \u{b7} v0.1.2 ready"),
                Tone::Busy,
                t("Click to restart into v0.1.2 (or it installs at next start)")
            )
        );
        assert_eq!(
            b(Offline),
            (t("v0.1.1 \u{b7} offline"), Tone::Neutral, t("Could not reach GitHub"))
        );
        assert_eq!(
            b(Failed {
                reason: t("checksum\nmismatch")
            }),
            (t("v0.1.1 \u{b7} update failed"), Tone::Bad, t("checksum mismatch"))
        );
    }

    #[test]
    fn self_update_restart_and_release_url() {
        let up = SelfUpdate::new("o", "r", "0.1.1", None, UpdateState::Latest);
        assert_eq!(up.release_url(), "https://github.com/o/r/releases/tag/v0.1.1");
        assert!(!up.request_restart() && !up.restart_requested()); // nothing staged
        up.set(UpdateState::Ready {
            version: "0.1.2".into(),
        });
        assert_eq!(up.release_url(), "https://github.com/o/r/releases/tag/v0.1.2");
        assert!(up.request_restart() && up.restart_requested());
        up.finish(); // no exe: nothing to do
        assert_eq!(
            SelfUpdate::new("o", "r", "", None, UpdateState::Disabled).release_url(),
            "https://github.com/o/r/releases"
        );
    }

    #[test]
    fn self_update_versions() {
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert!(is_newer("v1.10.0", "1.9.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v1.0.0", "1.0.1"));
        assert!(!is_newer("1.2.0", "1.0.0")); // no v
        assert!(!is_newer("v2.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("v2.0", "1.0.0"));
        assert!(!is_newer("v2.0.0.0", "1.0.0"));
        assert!(!is_newer("v+2.0.0", "1.0.0"));
        assert!(!is_newer("v2..0", "1.0.0"));
        assert!(!is_newer("v2.0.0", "bad"));
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn self_update_pick_assets() {
        let json = r#"{
            "url": "https://api.github.com/repos/o/r/releases/1", "tag_name": "v0.2.0", "name": "v0.2.0",
            "draft": false, "prerelease": false,
            "assets": [
                {"name": "handys-vbl.exe.sha256", "size": 81, "content_type": "text/plain",
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.2.0/handys-vbl.exe.sha256"},
                {"name": "handys-esp.exe", "size": 9,
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.2.0/handys-esp.exe"},
                {"name": "handys-vbl.exe", "size": 4096, "content_type": "application/x-msdownload",
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.2.0/handys-vbl.exe"}
            ]
        }"#;
        let v: Value = serde_json::from_str(json).unwrap();
        let (exe, sha) = pick_assets(&v, "handys-vbl.exe").unwrap();
        assert_eq!(exe, "https://github.com/o/r/releases/download/v0.2.0/handys-vbl.exe");
        assert_eq!(
            sha,
            "https://github.com/o/r/releases/download/v0.2.0/handys-vbl.exe.sha256"
        );
        assert!(
            pick_assets(&v, "handys-esp.exe")
                .unwrap_err()
                .contains("handys-esp.exe.sha256")
        );
        assert!(pick_assets(&v, "handys-bms.exe").is_err());
        assert!(pick_assets(&serde_json::json!({"message": "Not Found"}), "handys-vbl.exe").is_err());
    }

    #[test]
    fn self_update_checksum() {
        let abc = sha256(b"abc").unwrap();
        assert_eq!(parse_sha256(ABC_SHA256), Some(abc));
        assert_eq!(
            parse_sha256(&format!("{}  handys-x.exe\n", ABC_SHA256.to_uppercase())),
            Some(abc)
        );
        assert_eq!(parse_sha256(&format!("\u{feff}\n{ABC_SHA256}\r\n")), Some(abc));
        assert_ne!(sha256(b"abd").unwrap(), abc);
        assert_eq!(parse_sha256(&ABC_SHA256[..63]), None);
        assert_eq!(parse_sha256(&format!("{ABC_SHA256}0")), None);
        assert_eq!(parse_sha256(&format!("g{}", &ABC_SHA256[1..])), None);
        assert_eq!(parse_sha256(""), None);
    }

    #[test]
    fn self_update_swap_starts_new_exe() {
        let exe = temp_exe("swap-ok");
        fs::write(with_ext(&exe, ".new"), b"new").unwrap();
        let mut started = None;
        swap_and_start(&exe, |p| {
            started = Some(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(started.as_deref(), Some(exe.as_path()));
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(with_ext(&exe, ".old")).unwrap(), b"old");
        assert!(!with_ext(&exe, ".new").exists());

        assert!(cleanup(&exe, true));
        assert!(!with_ext(&exe, ".old").exists());
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_swap_rolls_back() {
        let exe = temp_exe("swap-rollback");
        fs::write(with_ext(&exe, ".new"), b"new").unwrap();
        let err = swap_and_start(&exe, |_| Err("start: nope".into())).unwrap_err();
        assert_eq!(err, "start: nope");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!with_ext(&exe, ".old").exists() && !with_ext(&exe, ".new").exists());

        // nothing staged: the rename fails, the exe is put back
        assert!(swap_and_start(&exe, |_| panic!("must not start")).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!with_ext(&exe, ".old").exists());
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_cleanup() {
        let exe = temp_exe("cleanup");
        for ext in [".old", ".part", ".new"] {
            fs::write(with_ext(&exe, ext), b"x").unwrap();
        }
        assert!(cleanup(&exe, false)); // the previous version's leftover: just updated
        assert!(!with_ext(&exe, ".old").exists());
        assert!(with_ext(&exe, ".part").exists()); // disabled: maybe another copy's download
        let t = Instant::now();
        assert!(!cleanup(&exe, true));
        assert!(!with_ext(&exe, ".part").exists());
        assert!(with_ext(&exe, ".new").exists() && exe.exists()); // a staged update is prepare's to install
        assert!(!cleanup(&exe, true)); // nothing to delete: returns at once
        assert!(t.elapsed() < Duration::from_secs(1));
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }
}
