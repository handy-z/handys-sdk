use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, EnumWindows, GA_ROOT, GW_OWNER, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, WINEVENT_OUTOFCONTEXT, WindowFromPoint,
};
use windows::core::BOOL;

use crate::roblox::events::RobloxFocusChanged;
use crate::util::event_bus::EventBus;

/// Pid owning the root ancestor of w; 0 if w is null or has no owner.
pub(crate) fn process_of_window(w: HWND) -> u32 {
    if w.is_invalid() {
        return 0;
    }
    let root = unsafe { GetAncestor(w, GA_ROOT) };
    let mut pid = 0;
    if root.is_invalid() || unsafe { GetWindowThreadProcessId(root, Some(&mut pid)) } == 0 {
        return 0;
    }
    pid
}

pub fn foreground_is(pid: u32) -> bool {
    pid != 0 && process_of_window(unsafe { GetForegroundWindow() }) == pid
}

pub fn cursor_over(pid: u32) -> bool {
    let mut pt = POINT::default();
    pid != 0 && unsafe { GetCursorPos(&mut pt) }.is_ok() && process_of_window(unsafe { WindowFromPoint(pt) }) == pid
}

/// Foreground window and the window under the cursor both belong to pid (false for pid 0).
pub fn roblox_ready(pid: u32) -> bool {
    foreground_is(pid) && cursor_over(pid)
}

/// pid's first visible, unowned top-level window.
pub fn process_window(pid: u32) -> Option<HWND> {
    if pid == 0 {
        return None;
    }
    unsafe extern "system" fn each(w: HWND, p: LPARAM) -> BOOL {
        // SAFETY: p is the (pid, found) pair below, alive for the whole EnumWindows call.
        let f = unsafe { &mut *(p.0 as *mut (u32, Option<HWND>)) };
        let mut wp = 0;
        unsafe { GetWindowThreadProcessId(w, Some(&mut wp)) };
        if wp != f.0 || !unsafe { IsWindowVisible(w) }.as_bool() || unsafe { GetWindow(w, GW_OWNER) }.is_ok() {
            return true.into();
        }
        f.1 = Some(w);
        false.into()
    }
    let mut find: (u32, Option<HWND>) = (pid, None);
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&raw mut find as isize));
    }
    find.1
}

/// Brings process_window(pid) to the foreground.
pub fn activate_process_window(pid: u32) -> bool {
    process_window(pid).is_some_and(|w| unsafe { SetForegroundWindow(w) }.as_bool())
}

struct Inner {
    out: Arc<AtomicBool>,
    bus: Option<EventBus>,
    pid: AtomicU32,
}

impl Inner {
    fn refresh(&self) {
        self.publish(foreground_is(self.pid.load(Ordering::Relaxed)));
    }
    fn publish(&self, focused: bool) {
        if self.out.swap(focused, Ordering::Relaxed) != focused
            && let Some(bus) = &self.bus
        {
            bus.emit(RobloxFocusChanged { focused });
        }
    }
}

// The WinEvent callback is a plain function, so it reaches the started watcher through here.
static WATCHER: AtomicPtr<Inner> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    // SAFETY: set only while the started watcher (which owns the Arc) is alive; stop() clears it first, and the
    // out-of-context callback runs on the thread that called start()/stop().
    if let Some(w) = unsafe { WATCHER.load(Ordering::Acquire).as_ref() } {
        w.refresh();
    }
}

/// Keeps `out` = (pid != 0 && foreground_is(pid)) via a WinEvent foreground hook; when `bus` is set, emits
/// RobloxFocusChanged each time that value changes (not on every refresh; stop(), also run on drop, may emit).
/// One instance at a time (the callback uses a static pointer); start()/stop() on the UI thread.
pub struct ForegroundWatcher {
    inner: Arc<Inner>,
    hook: Option<HWINEVENTHOOK>,
}

impl ForegroundWatcher {
    pub fn new(out: Arc<AtomicBool>, bus: Option<EventBus>) -> Self {
        Self {
            inner: Arc::new(Inner {
                out,
                bus,
                pid: AtomicU32::new(0),
            }),
            hook: None,
        }
    }

    pub fn start(&mut self) {
        if self.hook.is_some() {
            return;
        }
        WATCHER.store(Arc::as_ptr(&self.inner).cast_mut(), Ordering::Release);
        let h = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        self.hook = (!h.is_invalid()).then_some(h);
        self.refresh();
    }

    /// Unhooks, out = false.
    pub fn stop(&mut self) {
        if let Some(h) = self.hook.take() {
            let _ = unsafe { UnhookWinEvent(h) };
        }
        let me = Arc::as_ptr(&self.inner).cast_mut();
        let _ = WATCHER.compare_exchange(me, std::ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
        self.inner.publish(false);
    }

    pub fn set_pid(&self, pid: u32) {
        self.inner.pid.store(pid, Ordering::Relaxed);
    }

    pub fn refresh(&self) {
        self.inner.refresh();
    }
}

impl Drop for ForegroundWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_process_of_window_null() {
        assert_eq!(process_of_window(HWND::default()), 0);
        assert!(!roblox_ready(0));
    }

    #[test]
    fn events_foreground_watcher_emits_only_on_change() {
        let bus = EventBus::new();
        let inbox = bus.inbox::<RobloxFocusChanged>(64);
        let out = Arc::new(AtomicBool::new(false));
        let fw = ForegroundWatcher::new(out.clone(), Some(bus.clone()));
        fw.set_pid(0xFFFF_FFF0); // not the foreground
        fw.refresh();
        fw.refresh();
        assert!(!out.load(Ordering::Relaxed));
        assert!(inbox.drain().is_empty());
    }
}
