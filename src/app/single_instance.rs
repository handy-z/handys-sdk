use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::PCWSTR;

use crate::util::text::wide;

/// One running copy per mutex name.
pub struct SingleInstance {
    name: Vec<u16>,
    class: Vec<u16>,
    mutex: Option<HANDLE>,
}

impl SingleInstance {
    pub fn new(mutex_name: &str, window_class: &str) -> Self {
        Self {
            name: wide(mutex_name),
            class: wide(window_class),
            mutex: None,
        }
    }

    /// true = this copy owns the mutex (repeat calls are no-ops). false = another copy does: its window
    /// (FindWindowW(window_class)) is restored and brought to front, and no handle stays open here.
    pub fn acquire(&mut self) -> bool {
        if self.mutex.is_some() {
            return true;
        }
        unsafe {
            if let Ok(m) = CreateMutexW(None, false, PCWSTR(self.name.as_ptr())) {
                if GetLastError() != ERROR_ALREADY_EXISTS {
                    self.mutex = Some(m);
                    return true;
                }
                let _ = CloseHandle(m);
            }
            if let Ok(w) = FindWindowW(PCWSTR(self.class.as_ptr()), PCWSTR::null()) {
                let _ = ShowWindow(w, if IsIconic(w).as_bool() { SW_RESTORE } else { SW_SHOW });
                let _ = SetForegroundWindow(w);
            }
        }
        false
    }

    pub fn release(&mut self) {
        if let Some(m) = self.mutex.take() {
            unsafe {
                let _ = CloseHandle(m);
            }
        }
    }

    pub fn is_running(mutex_name: &str) -> bool {
        let name = wide(mutex_name);
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) } {
            Ok(m) => {
                unsafe {
                    let _ = CloseHandle(m);
                }
                true
            }
            Err(_) => false,
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_single_instance_second_acquire_fails() {
        let name = format!("Local\\handy-sdk-rs-test-{}", std::process::id());
        let (mut a, mut b) = (
            SingleInstance::new(&name, "no-such-class"),
            SingleInstance::new(&name, "no-such-class"),
        );
        assert!(a.acquire());
        assert!(a.acquire()); // second call on the same object is a no-op
        assert!(SingleInstance::is_running(&name));
        assert!(!b.acquire());
        a.release();
        assert!(!SingleInstance::is_running(&name)); // the failed b.acquire must not keep the mutex alive
    }
}
