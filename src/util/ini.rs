use std::path::{Path, PathBuf};

use windows::Win32::System::WindowsProgramming::{
    GetPrivateProfileIntW, GetPrivateProfileStringW, WritePrivateProfileStringW,
};
use windows::core::PCWSTR;

use super::text::{from_wide, wide, wide_path};

/// One section of a Win32 profile (.ini) file. Bools are stored as 0/1.
/// A missing file, section or key gives the default. Not thread-safe across writers of the same file.
pub struct Ini {
    path: PathBuf,
    file: Vec<u16>,
    section: Vec<u16>,
}

impl Ini {
    /// The "macro" section, where every game keeps its settings.
    pub fn new(file: impl AsRef<Path>) -> Self {
        Self::with_section(file, "macro")
    }

    pub fn with_section(file: impl AsRef<Path>, section: &str) -> Self {
        let path = file.as_ref().to_path_buf();
        Self {
            file: wide_path(&path),
            path,
            section: wide(section),
        }
    }

    pub fn get_int(&self, key: &str, def: i32) -> i32 {
        let key = wide(key);
        unsafe { GetPrivateProfileIntW(self.sec(), PCWSTR(key.as_ptr()), def, self.fil()) as i32 }
    }

    /// Result clamped to [lo, hi].
    pub fn get_int_clamped(&self, key: &str, def: i32, lo: i32, hi: i32) -> i32 {
        self.get_int(key, def).clamp(lo, hi)
    }

    pub fn get_bool(&self, key: &str, def: bool) -> bool {
        self.get_int(key, i32::from(def)) != 0
    }

    /// Up to 4095 UTF-16 units.
    pub fn get_str(&self, key: &str, def: &str) -> String {
        let (key, def) = (wide(key), wide(def));
        let mut buf = vec![0u16; 4096];
        unsafe {
            GetPrivateProfileStringW(
                self.sec(),
                PCWSTR(key.as_ptr()),
                PCWSTR(def.as_ptr()),
                Some(&mut buf),
                self.fil(),
            );
        }
        from_wide(&buf)
    }

    pub fn put_int(&self, key: &str, v: i32) {
        self.put_str(key, &v.to_string());
    }

    pub fn put_bool(&self, key: &str, v: bool) {
        self.put_int(key, i32::from(v));
    }

    pub fn put_str(&self, key: &str, v: &str) {
        // A new file gets a UTF-16 BOM so the profile API stores Unicode instead of the ANSI code page.
        if !self.path.exists() {
            let _ = std::fs::write(&self.path, [0xFF, 0xFE]);
        }
        let (key, v) = (wide(key), wide(v));
        unsafe {
            let _ = WritePrivateProfileStringW(self.sec(), PCWSTR(key.as_ptr()), PCWSTR(v.as_ptr()), self.fil());
        }
    }

    fn sec(&self) -> PCWSTR {
        PCWSTR(self.section.as_ptr())
    }
    fn fil(&self) -> PCWSTR {
        PCWSTR(self.file.as_ptr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(n: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("handy_sdk_rs_ini_{n}.ini"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn ini_missing_file_gives_defaults() {
        let ini = Ini::new(fresh("missing"));
        assert_eq!(ini.get_int("a", 5), 5);
        assert_eq!(ini.get_str("s", "x"), "x");
        assert!(ini.get_bool("b", true));
        assert!(!ini.get_bool("b", false));
    }

    #[test]
    fn ini_roundtrip_int_bool_utf8_string() {
        let f = fresh("roundtrip");
        let w = Ini::new(&f);
        w.put_int("n", -7);
        w.put_bool("on", true);
        w.put_bool("off", false);
        w.put_str("s", "Shiny,Sparkling \u{2248}");
        let r = Ini::new(&f);
        assert_eq!(r.get_int("n", 0), -7);
        assert!(r.get_bool("on", false));
        assert!(!r.get_bool("off", true));
        assert_eq!(r.get_str("s", ""), "Shiny,Sparkling \u{2248}");
    }

    #[test]
    fn ini_clamps_on_read() {
        let ini = Ini::new(fresh("clamp"));
        ini.put_int("latencyMs", 999);
        ini.put_int("neg", -5);
        assert_eq!(ini.get_int_clamped("latencyMs", 12, 0, 100), 100);
        assert_eq!(ini.get_int_clamped("neg", 12, 0, 1000), 0);
        assert_eq!(ini.get_int_clamped("absent", 12, 0, 100), 12);
    }

    #[test]
    fn ini_sections_are_separate() {
        let f = fresh("sections");
        Ini::with_section(&f, "version-abc").put_int("iconColor", 1);
        assert_eq!(Ini::with_section(&f, "version-abc").get_int("iconColor", 0), 1);
        assert_eq!(Ini::new(&f).get_int("iconColor", 0), 0);
    }
}
