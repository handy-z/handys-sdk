use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// A thread with a stop flag, stopped and joined on drop.
pub struct StopThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl StopThread {
    /// `f` should return soon after the flag turns true.
    pub fn spawn(f: impl FnOnce(&AtomicBool) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        Self {
            stop,
            handle: Some(std::thread::spawn(move || f(&flag))),
        }
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for StopThread {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
