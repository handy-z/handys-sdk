use std::path::{Path, PathBuf};
use std::sync::Mutex;

static APP_NAME: Mutex<String> = Mutex::new(String::new());
static OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Names the %LOCALAPPDATA% folder, e.g. "handy's tool". Call once at startup, before `app_data_dir()`.
pub fn set_app_name(name: &str) {
    *APP_NAME.lock().unwrap() = name.to_owned();
}

/// Whether `app_data_dir()` can answer without panicking.
pub(crate) fn has_app_name() -> bool {
    OVERRIDE.lock().is_ok_and(|o| o.is_some()) || APP_NAME.lock().is_ok_and(|n| !n.is_empty())
}

/// Tests only: `None` restores the default.
pub fn set_app_data_dir_for_tests(p: Option<&Path>) {
    *OVERRIDE.lock().unwrap() = p.map(Path::to_path_buf);
}

/// %LOCALAPPDATA%\<app name>, created if missing. Panics when no app name (or test dir) is set.
pub fn app_data_dir() -> PathBuf {
    let over = OVERRIDE.lock().unwrap().clone(); // its own statement: the lock is released before a panic below
    let dir = over.unwrap_or_else(|| {
        let name = APP_NAME.lock().unwrap().clone();
        assert!(!name.is_empty(), "app_data_dir: set_app_name was not called");
        std::env::var_os("LOCALAPPDATA")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(name)
    });
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_require_app_name_or_override() {
        assert!(std::panic::catch_unwind(app_data_dir).is_err());
        let dir = std::env::temp_dir().join("handy-sdk-rs-paths-test");
        set_app_data_dir_for_tests(Some(&dir));
        assert!(app_data_dir() == dir && dir.exists());
        set_app_data_dir_for_tests(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
