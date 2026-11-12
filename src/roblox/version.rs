//! Works out which Roblox build is running: `VersionCandidate`s from the exe path, versions.ini, State.json, the
//! client-version API, the logs and the offsets cache.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::PCWSTR;

use crate::util::ini::Ini;
use crate::util::text::{from_wide, wide};

/// ^version-[0-9a-f]{16}$
pub(crate) fn is_version_hash(s: &str) -> bool {
    s.len() == 24 && s.starts_with("version-") && s.bytes().skip(8).all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid(s: Option<String>) -> Option<String> {
    s.filter(|s| is_version_hash(s))
}

pub(crate) fn hash_from_path(exe_path: &str) -> Option<String> {
    Path::new(exe_path)
        .iter()
        .filter_map(|p| p.to_str())
        .find(|n| is_version_hash(n))
        .map(str::to_owned)
}

/// "0, 745, 0, 7451137" -> "0.745.0.7451137".
pub(crate) fn normalize_file_version(s: &str) -> Option<String> {
    let o: String = s
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| {
            if c == ',' {
                '.'
            } else if c.is_ascii() {
                c
            } else {
                '?'
            }
        })
        .collect();
    let (mut parts, mut digit) = (0, false);
    for c in o.chars() {
        if c.is_ascii_digit() {
            digit = true;
        } else if c == '.' && digit {
            parts += 1;
            digit = false;
        } else {
            return None;
        }
    }
    (digit && parts == 3).then_some(o)
}

/// "a.b.c.d"
pub fn file_version_of(exe_path: &str) -> Option<String> {
    let path = wide(exe_path);
    let path = PCWSTR(path.as_ptr());
    let size = unsafe { GetFileVersionInfoSizeW(path, None) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(path, None, size, buf.as_mut_ptr().cast()) }.ok()?;
    let query = |sub: &str| -> Option<(*const u8, u32)> {
        let sub = wide(sub);
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        let ok = unsafe { VerQueryValueW(buf.as_ptr().cast(), PCWSTR(sub.as_ptr()), &mut p, &mut len) }.as_bool();
        (ok && !p.is_null()).then_some((p.cast_const().cast(), len))
    };
    // VS_FIXEDFILEINFO's 4th part is 16 bits (0.745.0.45569); the string FileVersion has the full build.
    let mut langs = Vec::new(); // low word = lang, high word = codepage
    if let Some((p, len)) = query("\\VarFileInfo\\Translation") {
        for i in 0..len as usize / 4 {
            // SAFETY: the block holds `len` bytes of (lang, codepage) u16 pairs.
            let (lang, cp) = unsafe {
                (
                    p.add(4 * i).cast::<u16>().read_unaligned(),
                    p.add(4 * i + 2).cast::<u16>().read_unaligned(),
                )
            };
            langs.push(u32::from(lang) | (u32::from(cp) << 16));
        }
    }
    langs.extend([0x04B0_0409, 0x04E4_0409]);
    for l in langs {
        let sub = format!("\\StringFileInfo\\{:04X}{:04X}\\FileVersion", l & 0xFFFF, l >> 16);
        if let Some((p, n)) = query(&sub) {
            // SAFETY: a string value of `n` UTF-16 units (with its NUL) inside the block.
            let s = unsafe { std::slice::from_raw_parts(p.cast::<u16>(), n as usize) };
            if let Some(v) = normalize_file_version(&from_wide(s)) {
                return Some(v);
            }
        }
    }
    let (p, len) = query("\\")?;
    if (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: checked to hold a whole VS_FIXEDFILEINFO.
    let fi = unsafe { p.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
    let (ms, ls) = (fi.dwFileVersionMS, fi.dwFileVersionLS);
    Some(format!("{}.{}.{}.{}", ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF))
}

pub fn state_file_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut d = exe_dir;
    for _ in 0..4 {
        if d.as_os_str().is_empty() {
            break;
        }
        for p in [d.join("State.json"), d.join("Config").join("State.json")] {
            if p.is_file() {
                out.push(p);
            }
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    out
}

fn json(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

pub fn player_guid_from_state(state_json: &str) -> Option<String> {
    let v = json(state_json)?;
    valid(v.get("Player")?.get("VersionGuid")?.as_str().map(str::to_owned))
}

pub fn hash_from_api(api_json: &str, file_version: &str) -> Option<String> {
    let v = json(api_json)?;
    if file_version.is_empty() || v.get("version")?.as_str()? != file_version {
        return None;
    }
    valid(v.get("clientVersionUpload")?.as_str().map(str::to_owned))
}

pub(crate) fn hash_from_logs(logs_dir: &Path, file_version: &str) -> Option<String> {
    if file_version.is_empty() {
        return None;
    }
    let prefix = format!("{file_version}_");
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(logs_dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with(&prefix) && n.contains("_Player_") && !n.contains("CrashHandler") && n.ends_with(".log")
        })
        .map(|e| {
            (
                e.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH),
                e.path(),
            )
        })
        .collect();
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, f) in files {
        let Ok(t) = std::fs::read(&f) else { continue };
        for p in (0..t.len()).filter(|p| t[*p..].starts_with(b"version-")) {
            if let Some(h) = t
                .get(p..p + 24)
                .and_then(|h| std::str::from_utf8(h).ok())
                .filter(|h| is_version_hash(h))
            {
                return Some(h.to_owned());
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionCandidate {
    pub hash: String,
    pub source: &'static str,
}

type Hint<'a> = Option<Box<dyn Fn() -> Option<String> + 'a>>;

#[derive(Default)]
pub struct VersionInputs<'a> {
    pub exe_path: String,
    pub file_version: String,
    pub learned: Hint<'a>,
    pub state: Hint<'a>,
    pub api: Hint<'a>,
    pub logs: Hint<'a>,
    /// Newest first.
    pub cached_hashes: Vec<String>,
}

pub fn version_candidates(inp: &VersionInputs) -> Vec<VersionCandidate> {
    fn add(out: &mut Vec<VersionCandidate>, h: Option<String>, source: &'static str) {
        if let Some(hash) = valid(h)
            && !out.iter().any(|c| c.hash == hash)
        {
            out.push(VersionCandidate { hash, source });
        }
    }
    let mut out = Vec::new();
    add(&mut out, hash_from_path(&inp.exe_path), "path");
    for (hint, source) in [
        (&inp.learned, "learned"),
        (&inp.state, "state"),
        (&inp.api, "api"),
        (&inp.logs, "logs"),
    ] {
        if let Some(f) = hint {
            add(&mut out, f(), source);
        }
    }
    // Probing every cached file only when nothing names the version: a stale cache must not shadow a new one.
    if out.is_empty() {
        for h in &inp.cached_hashes {
            add(&mut out, Some(h.clone()), "cache-probe");
        }
    }
    out
}

pub(crate) fn learned_version(ini: &Path, file_version: &str) -> Option<String> {
    valid(Some(Ini::with_section(ini, "map").get_str(file_version, "")))
}

pub(crate) fn remember_version(ini: &Path, file_version: &str, hash: &str) {
    if is_version_hash(hash) && !file_version.is_empty() {
        Ini::with_section(ini, "map").put_str(file_version, hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmp(n: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("handy_sdk_rs_ver_{n}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn hint<'a>(h: Option<&'a str>) -> Hint<'a> {
        Some(Box::new(move || h.map(str::to_owned)))
    }

    #[test]
    fn version_from_versioned_path() {
        let h =
            hash_from_path("C:/Users/a/AppData/Local/Roblox/Versions/version-5a21b87bbc2641cf/RobloxPlayerBeta.exe");
        assert_eq!(h.as_deref(), Some("version-5a21b87bbc2641cf"));
    }

    #[test]
    fn version_static_path_has_no_hash() {
        assert!(
            hash_from_path("C:/Users/a/AppData/Local/Voidstrap/RblxVersions/WindowsPlayer/RobloxPlayerBeta.exe")
                .is_none()
        );
    }

    #[test]
    fn version_state_player_block_only() {
        let h = player_guid_from_state(
            r#"{"A":{"VersionGuid":"version-0000000000000000"},"Player":{"x":{"y":1},"VersionGuid":"version-9ac8595712b447a7"},"Studio":{"VersionGuid":"version-1111111111111111"}}"#,
        );
        assert_eq!(h.as_deref(), Some("version-9ac8595712b447a7"));
        assert!(
            player_guid_from_state(
                r#"{"Player":{"VersionGuid":""},"Studio":{"VersionGuid":"version-1111111111111111"}}"#
            )
            .is_none()
        );
        assert!(player_guid_from_state(r#"{"Player":{"VersionGuid":"../evil"}}"#).is_none());
        assert!(player_guid_from_state(r#"{"Studio":{"VersionGuid":"version-1111111111111111"}}"#).is_none());
    }

    #[test]
    fn version_state_files_found_by_walking_up() {
        let r = tmp("walk");
        put(&r.join("Config").join("State.json"), "{}");
        std::fs::create_dir_all(r.join("RblxVersions").join("WindowsPlayer")).unwrap();
        let c = state_file_candidates(&r.join("RblxVersions").join("WindowsPlayer"));
        assert_eq!(c, [r.join("Config").join("State.json")]);
        let r2 = tmp("walk2");
        put(&r2.join("State.json"), "{}");
        std::fs::create_dir_all(r2.join("Versions").join("WindowsPlayer")).unwrap();
        assert!(!state_file_candidates(&r2.join("Versions").join("WindowsPlayer")).is_empty());
    }

    #[test]
    fn version_api_requires_matching_file_version() {
        let j = r#"{"version":"0.745.0.7451137","clientVersionUpload":"version-5a21b87bbc2641cf"}"#;
        assert_eq!(
            hash_from_api(j, "0.745.0.7451137").as_deref(),
            Some("version-5a21b87bbc2641cf")
        );
        assert!(hash_from_api(j, "0.744.0.7441080").is_none());
        assert!(hash_from_api(r#"{"version":"1","clientVersionUpload":"bad"}"#, "1").is_none());
    }

    #[test]
    fn version_logs_pick_newest_matching() {
        let d = tmp("logs");
        let old = d.join("0.744.0.7441080_20261026T005014Z_Player_00001_last.log");
        put(&old, "x C:/version-1111111111111111/ y");
        put(
            &d.join("0.743.0.1_20261028T005014Z_Player_00002_last.log"),
            "x version-2222222222222222",
        );
        put(
            &d.join("0.744.0.7441080_20261028T005014Z_CrashHandler_00003_last.log"),
            "version-3333333333333333",
        );
        put(
            &d.join("0.744.0.7441080_20261027T005014Z_Player_07797_last.log"),
            "a/version-9ac8595712b447a7/b",
        );
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(48 * 3600))
            .unwrap();
        drop(f);
        assert_eq!(
            hash_from_logs(&d, "0.744.0.7441080").as_deref(),
            Some("version-9ac8595712b447a7")
        );
    }

    #[test]
    fn version_static_path_uses_hints() {
        let inp = VersionInputs {
            exe_path: "C:/x/RblxVersions/WindowsPlayer/RobloxPlayerBeta.exe".into(),
            file_version: "0.744.0.7441080".into(),
            learned: hint(None),
            state: hint(Some("version-9ac8595712b447a7")),
            api: hint(None),
            ..Default::default()
        };
        let c = version_candidates(&inp);
        assert_eq!(
            c[0],
            VersionCandidate {
                hash: "version-9ac8595712b447a7".into(),
                source: "state"
            }
        );
    }

    #[test]
    fn version_candidates_dedupe_and_probe_only_without_hints() {
        let mut inp = VersionInputs {
            exe_path: "C:/x/WindowsPlayer/RobloxPlayerBeta.exe".into(),
            state: hint(Some("version-5a21b87bbc2641cf")),
            api: hint(Some("version-5a21b87bbc2641cf")),
            cached_hashes: vec!["version-5a21b87bbc2641cf".into(), "version-9ac8595712b447a7".into()],
            ..Default::default()
        };
        let c = version_candidates(&inp); // a hint exists: no cache probing
        assert_eq!(
            c,
            [VersionCandidate {
                hash: "version-5a21b87bbc2641cf".into(),
                source: "state"
            }]
        );
        inp.state = hint(Some("garbage"));
        inp.api = hint(Some("garbage"));
        let c = version_candidates(&inp); // no valid hint: probe the cache, newest first
        assert_eq!(
            c,
            [
                VersionCandidate {
                    hash: "version-5a21b87bbc2641cf".into(),
                    source: "cache-probe"
                },
                VersionCandidate {
                    hash: "version-9ac8595712b447a7".into(),
                    source: "cache-probe"
                },
            ]
        );
    }

    #[test]
    fn version_remember_roundtrip() {
        let ini = tmp("ini").join("versions.ini");
        assert!(learned_version(&ini, "0.744.0.1").is_none());
        remember_version(&ini, "0.744.0.1", "version-9ac8595712b447a7");
        remember_version(&ini, "0.744.0.2", "nonsense");
        assert_eq!(
            learned_version(&ini, "0.744.0.1").as_deref(),
            Some("version-9ac8595712b447a7")
        );
        assert!(learned_version(&ini, "0.744.0.2").is_none());
    }

    #[test]
    fn version_normalize_file_version() {
        assert_eq!(
            normalize_file_version("0, 745, 0, 7451137").as_deref(),
            Some("0.745.0.7451137")
        );
        assert_eq!(
            normalize_file_version("0.744.0.7441080").as_deref(),
            Some("0.744.0.7441080")
        );
        for bad in ["garbage", "1.2.3", "1.2.3.", ""] {
            assert!(normalize_file_version(bad).is_none(), "{bad}");
        }
    }
}
