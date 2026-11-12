use std::path::Path;
use std::sync::Mutex;

/// What a game's settings struct implements so `SharedSettings::save` can write it.
pub trait SaveSettings {
    fn save(&self, ini: &Path);
}

/// A game's settings shared between its engine, dashboard and updater threads.
#[derive(Default)]
pub struct SharedSettings<S> {
    s: Mutex<S>,
    file: Mutex<()>,
}

impl<S: Clone + SaveSettings> SharedSettings<S> {
    pub fn new(s: S) -> Self {
        Self {
            s: Mutex::new(s),
            file: Mutex::new(()),
        }
    }
    pub fn get(&self) -> S {
        self.s.lock().unwrap().clone()
    }
    pub fn update(&self, f: impl FnOnce(&mut S)) {
        f(&mut self.s.lock().unwrap());
    }
    /// Writes a copy under a file-only lock, so readers (the engines poll every 1 ms) never wait on the disk.
    /// The copy is taken after the file lock, so the last save to finish always writes the newest settings.
    pub fn save(&self, ini: &Path) {
        let _file = self.file.lock().unwrap();
        let s = self.get();
        s.save(ini);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::{Arc, Barrier};

    #[derive(Clone)]
    struct Slow(Arc<(Barrier, AtomicBool)>);
    impl SaveSettings for Slow {
        fn save(&self, _: &Path) {
            self.0.0.wait(); // the reader below runs while this save is in progress
            self.0.0.wait();
            self.0.1.store(true, SeqCst);
        }
    }

    #[test]
    fn shared_settings_get_does_not_wait_for_save() {
        let sync = Arc::new((Barrier::new(2), AtomicBool::new(false)));
        let store = Arc::new(SharedSettings::new(Slow(sync.clone())));
        let st = store.clone();
        let saver = std::thread::spawn(move || st.save(Path::new("unused.ini")));
        sync.0.wait(); // save has started
        let _ = store.get(); // would deadlock if save held the settings lock
        assert!(!sync.1.load(SeqCst));
        sync.0.wait();
        saver.join().unwrap();
        assert!(sync.1.load(SeqCst));
    }
}
