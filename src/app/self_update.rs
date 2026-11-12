use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};

use crate::app::crash_guard::local_time;
use crate::roblox::http::win_http_get_capped;
use crate::util::paths;

const API_HEADERS: &str = "Accept: application/vnd.github+json";
// GitHub slow or offline must not hold up startup: each WinHTTP step of the release check gets 3 s.
const API_TIMEOUT_MS: i32 = 3000;
const API_CAP: usize = 1024 * 1024;
// Once an update is known the downloads may take longer (this is per step, so a slow but moving download is fine).
const DOWNLOAD_TIMEOUT_MS: i32 = 10000;
const SHA_CAP: usize = 4096;
const EXE_CAP: usize = 32 * 1024 * 1024;

/// Checks https://api.github.com/repos/<owner>/<repo>/releases/latest. When its tag (v<semver>) is newer than
/// `current` (the app's CARGO_PKG_VERSION) and it has the assets `<exe_name>` and `<exe_name>.sha256`, downloads the
/// exe, verifies its SHA-256 against the .sha256 file (first 64 hex chars, case-insensitive), swaps it in place of the
/// running exe and starts it with the same arguments. Returns true when the new version was started: the caller must
/// exit at once without starting anything (call it before taking a single-instance mutex, or the new copy finds it
/// held). Any failure leaves everything as it was, appends one line to `app_data_dir()/update.log`, and returns false.
pub fn update_on_startup(owner: &str, repo: &str, current: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        log("?", current, "?", "current_exe failed");
        return false;
    };
    cleanup(&exe);
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let app = exe.file_stem().and_then(|n| n.to_str()).unwrap_or_default();

    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let release = match get(&url, API_HEADERS, API_CAP, API_TIMEOUT_MS, "release check")
        .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| format!("release JSON: {e}")))
    {
        Ok(r) => r,
        Err(e) => {
            log(app, current, "?", &e);
            return false;
        }
    };
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, current) {
        return false;
    }
    let latest = &tag[1..];
    match install(&release, name, &exe) {
        Ok(()) => {
            log(app, current, latest, "ok");
            true
        }
        Err(e) => {
            log(app, current, latest, &e);
            false
        }
    }
}

fn install(release: &Value, name: &str, exe: &Path) -> Result<(), String> {
    let (exe_url, sha_url) = pick_assets(release, name)?;
    let sha_file = get(&sha_url, "", SHA_CAP, DOWNLOAD_TIMEOUT_MS, ".sha256")?;
    let want = parse_sha256(&String::from_utf8_lossy(&sha_file)).ok_or(".sha256: no SHA-256 at its start")?;
    let bytes = get(&exe_url, "", EXE_CAP, DOWNLOAD_TIMEOUT_MS, name)?;
    if sha256(&bytes)? != want {
        return Err("checksum mismatch".into());
    }
    swap_and_start(exe, &bytes, start)
}

/// Status 200 and at most `cap` bytes, or why not.
fn get(url: &str, headers: &str, cap: usize, timeout_ms: i32, what: &str) -> Result<Vec<u8>, String> {
    let (status, body) = win_http_get_capped(url, headers, cap, timeout_ms);
    if status != 200 {
        return Err(format!("{what}: HTTP status {status}"));
    }
    if body.len() > cap {
        return Err(format!("{what}: larger than {cap} bytes"));
    }
    Ok(body)
}

/// Starts `exe` with this process's arguments, detached so nothing waits on it.
fn start(exe: &Path) -> Result<(), String> {
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

/// Deletes `<exe>.old` and `<exe>.new`. The old process may still be exiting (a running exe cannot be deleted), so
/// this retries for up to 2 s in all, then gives up silently.
fn cleanup(exe: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    for p in [with_ext(exe, ".old"), with_ext(exe, ".new")] {
        while p.exists() && fs::remove_file(&p).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Puts `bytes` in place of `exe` (Windows allows renaming a running exe) and calls `start` on it. On any failure the
/// original exe is back at `exe`.
fn swap_and_start(exe: &Path, bytes: &[u8], start: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let (new, old) = (with_ext(exe, ".new"), with_ext(exe, ".old"));
    if let Err(e) = fs::write(&new, bytes) {
        let _ = fs::remove_file(&new);
        return Err(format!("write .new: {e}"));
    }
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
        let mut started = None;
        swap_and_start(&exe, b"new", |p| {
            started = Some(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(started.as_deref(), Some(exe.as_path()));
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(with_ext(&exe, ".old")).unwrap(), b"old");
        assert!(!with_ext(&exe, ".new").exists());

        cleanup(&exe);
        assert!(!with_ext(&exe, ".old").exists());
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_swap_rolls_back() {
        let exe = temp_exe("swap-rollback");
        let err = swap_and_start(&exe, b"new", |_| Err("start: nope".into())).unwrap_err();
        assert_eq!(err, "start: nope");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!with_ext(&exe, ".old").exists() && !with_ext(&exe, ".new").exists());

        fs::create_dir(with_ext(&exe, ".new")).unwrap(); // the write fails
        assert!(swap_and_start(&exe, b"new", |_| panic!("must not start")).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!with_ext(&exe, ".old").exists());
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }

    #[test]
    fn self_update_cleanup() {
        let exe = temp_exe("cleanup");
        fs::write(with_ext(&exe, ".old"), b"x").unwrap();
        fs::write(with_ext(&exe, ".new"), b"y").unwrap();
        let t = Instant::now();
        cleanup(&exe);
        assert!(!with_ext(&exe, ".old").exists() && !with_ext(&exe, ".new").exists());
        assert!(exe.exists());
        cleanup(&exe); // nothing to delete: returns at once
        assert!(t.elapsed() < Duration::from_secs(1));
        let _ = fs::remove_dir_all(exe.parent().unwrap());
    }
}
