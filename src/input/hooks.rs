use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_RWIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_INJECTED,
    LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_USER, WM_XBUTTONDOWN,
    WM_XBUTTONUP, XBUTTON1, XBUTTON2,
};

use super::key::key_down;
use super::send::INJECTED_TAG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub msg: u32,
    pub vk: u32,
    pub scan: u32,
    pub flags: u32,
    pub time: u32,
    pub qpc: i64,
    pub down: bool,
    /// LLKHF_INJECTED
    pub injected: bool,
    /// dwExtraInfo == INJECTED_TAG
    pub ours: bool,
    /// Alt flag, Ctrl, LWin or RWin
    pub modifier_down: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub msg: u32,
    /// 1 left, 2 right, 3 middle, 4 X1, 5 X2; 0 = move/wheel/other
    pub button: i32,
    pub down: bool,
    pub mouse_data: u32,
    pub time: u32,
    pub qpc: i64,
    /// LLMHF_INJECTED
    pub injected: bool,
    pub ours: bool,
}

pub(crate) fn key_event_from(msg: u32, k: &KBDLLHOOKSTRUCT, modifier_down: bool, qpc: i64) -> KeyEvent {
    KeyEvent {
        msg,
        vk: k.vkCode,
        scan: k.scanCode,
        flags: k.flags.0,
        time: k.time,
        qpc,
        down: msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN,
        injected: (k.flags & LLKHF_INJECTED).0 != 0,
        ours: k.dwExtraInfo == INJECTED_TAG,
        modifier_down,
    }
}

pub(crate) fn mouse_event_from(msg: u32, m: &MSLLHOOKSTRUCT, qpc: i64) -> MouseEvent {
    let (button, down) = match msg {
        WM_LBUTTONDOWN => (1, true),
        WM_LBUTTONUP => (1, false),
        WM_RBUTTONDOWN => (2, true),
        WM_RBUTTONUP => (2, false),
        WM_MBUTTONDOWN => (3, true),
        WM_MBUTTONUP => (3, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let x = (m.mouseData >> 16) as u16;
            (
                if x == XBUTTON1 {
                    4
                } else if x == XBUTTON2 {
                    5
                } else {
                    0
                },
                msg == WM_XBUTTONDOWN,
            )
        }
        _ => (0, false),
    };
    MouseEvent {
        msg,
        button,
        down,
        mouse_data: m.mouseData,
        time: m.time,
        qpc,
        injected: m.flags & LLMHF_INJECTED != 0,
        ours: m.dwExtraInfo == INJECTED_TAG,
    }
}

/// A handler returns true to swallow the event. Handlers run inside the hook callback: they MUST be O(1) and
/// non-blocking (read atomics only).
#[derive(Default)]
pub struct Handlers {
    pub key: Option<Box<dyn Fn(&KeyEvent) -> bool + Send + Sync>>,
    pub mouse: Option<Box<dyn Fn(&MouseEvent) -> bool + Send + Sync>>,
}

struct Inner {
    h: Handlers,
    thread_id: AtomicU32,
    installed: AtomicBool, // every requested hook installed; valid once thread_id is set
}

// The hook procs are plain functions, so they reach the started instance through here.
static HOOKS: AtomicPtr<Inner> = AtomicPtr::new(std::ptr::null_mut());

fn qpc_now() -> i64 {
    let mut now = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
    }
    now
}

fn handlers() -> Option<&'static Handlers> {
    // SAFETY: set only while an Inner is alive (its LowLevelHooks holds the Arc and clears this before dropping it),
    // and the procs only run on the hook thread, which stop() joins first.
    unsafe { HOOKS.load(Ordering::Acquire).as_ref() }.map(|i| &i.h)
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(f) = handlers().and_then(|h| h.mouse.as_ref())
    {
        // SAFETY: for HC_ACTION, lParam points to an MSLLHOOKSTRUCT.
        let m = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        if f(&mouse_event_from(wp.0 as u32, m, qpc_now())) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(f) = handlers().and_then(|h| h.key.as_ref())
    {
        // SAFETY: for HC_ACTION, lParam points to a KBDLLHOOKSTRUCT.
        let ks = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let modifier =
            (ks.flags & LLKHF_ALTDOWN).0 != 0 || key_down(VK_CONTROL.0) || key_down(VK_LWIN.0) || key_down(VK_RWIN.0);
        if f(&key_event_from(wp.0 as u32, ks, modifier, qpc_now())) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn run(inner: &Inner) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let m = inner
            .h
            .mouse
            .as_ref()
            .and_then(|_| SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0).ok());
        let k = inner
            .h
            .key
            .as_ref()
            .and_then(|_| SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), module, 0).ok());
        let failed = (inner.h.mouse.is_some() && m.is_none()) || (inner.h.key.is_some() && k.is_none());
        inner.installed.store(!failed, Ordering::Release);
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE); // create the message queue
        inner.thread_id.store(GetCurrentThreadId(), Ordering::Release);
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        for h in [m, k].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

/// Low-level hooks on their own TIME_CRITICAL thread. Only hooks whose handler is set are installed.
/// `start()` returns once the thread's queue exists (or after ~2 s). Only ONE instance may be started at a time (the
/// hook procs reach it through a static pointer).
pub struct LowLevelHooks {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl LowLevelHooks {
    pub fn new(h: Handlers) -> Self {
        Self {
            inner: Arc::new(Inner {
                h,
                thread_id: AtomicU32::new(0),
                installed: AtomicBool::new(false),
            }),
            thread: None,
        }
    }

    /// True when every requested hook installed. False also when the thread did not come up within ~2 s.
    pub fn start(&mut self) -> bool {
        if self.thread.is_some() {
            return self.inner.installed.load(Ordering::Acquire);
        }
        HOOKS.store(Arc::as_ptr(&self.inner).cast_mut(), Ordering::Release);
        self.inner.thread_id.store(0, Ordering::Release);
        let inner = self.inner.clone();
        self.thread = Some(std::thread::spawn(move || run(&inner)));
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.inner.thread_id.load(Ordering::Acquire) == 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::yield_now(); // queue exists once id is set
        }
        self.inner.installed.load(Ordering::Acquire)
    }

    pub fn stop(&mut self) {
        let Some(t) = self.thread.take() else { return };
        // start() may have given up waiting: the quit message needs the id, or join would wait forever
        while self.inner.thread_id.load(Ordering::Acquire) == 0 && !t.is_finished() {
            std::thread::yield_now();
        }
        unsafe {
            let _ = PostThreadMessageW(
                self.inner.thread_id.load(Ordering::Acquire),
                WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        let _ = t.join();
        let me = Arc::as_ptr(&self.inner).cast_mut();
        let _ = HOOKS.compare_exchange(me, std::ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
    }
}

impl Drop for LowLevelHooks {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Fresh-press edge for a key fed from hook events (auto-repeat is not fresh). A "repeat" more than 1100 ms after the
/// last down/repeat (longer than the 1 s max auto-repeat delay) means the key-up was missed, so it counts as fresh.
/// Wrap-safe on the ms tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct HotkeyEdge {
    pub down: bool,
    pub last_down_ms: u32,
}

impl HotkeyEdge {
    pub fn fresh(&mut self, is_down: bool, time_ms: u32) -> bool {
        let f = is_down && (!self.down || time_ms.wrapping_sub(self.last_down_ms) > 1100);
        if is_down {
            self.last_down_ms = time_ms;
        }
        self.down = is_down;
        f
    }
}

/// Edge detector for polled keys. Starts as "was down" so a key held at startup is no press.
#[derive(Clone, Copy, Debug)]
pub struct KeyPoller {
    was: bool,
}

impl Default for KeyPoller {
    fn default() -> Self {
        Self { was: true }
    }
}

impl KeyPoller {
    pub fn step(&mut self, down: bool) -> bool {
        !std::mem::replace(&mut self.was, down) && down
    }
    pub fn pressed(&mut self, vk: u32) -> bool {
        self.step(key_down(vk as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F1;
    use windows::Win32::UI::WindowsAndMessaging::{WM_MOUSEMOVE, WM_SYSKEYUP};

    #[test]
    fn hooks_hotkey_edge_ignores_repeat() {
        let mut e = HotkeyEdge::default();
        assert!(e.fresh(true, 1000));
        assert!(!e.fresh(true, 1500)); // auto-repeat
        assert!(!e.fresh(false, 1600));
        assert!(e.fresh(true, 1700));
    }

    #[test]
    fn hooks_hotkey_edge_recovers_missed_keyup() {
        let mut e = HotkeyEdge::default();
        assert!(e.fresh(true, 1000));
        // held: 1000 ms initial delay then 33 ms repeats are never fresh, however long the hold
        assert!(!e.fresh(true, 2000));
        for t in (2033..5000).step_by(33) {
            assert!(!e.fresh(true, t));
        }
        // last repeat was at 4970; key-up missed. Exactly 1100 ms later is still a repeat, beyond that is fresh.
        assert!(!e.fresh(true, 6070));
        assert!(e.fresh(true, 7195));
        // tick wrap-around
        let mut w = HotkeyEdge::default();
        assert!(w.fresh(true, 0xFFFF_FF00));
        assert!(!w.fresh(true, 0x100)); // 512 ms later
        assert!(w.fresh(true, 0x700)); // 1536 ms
    }

    #[test]
    fn hooks_event_fields_from_ll_structs() {
        let k = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_F1.0),
            scanCode: 0x3B,
            flags: LLKHF_INJECTED,
            time: 1234,
            dwExtraInfo: INJECTED_TAG,
        };
        let e = key_event_from(WM_KEYDOWN, &k, false, 99);
        assert!(e.down && e.vk == u32::from(VK_F1.0) && e.scan == 0x3B && e.injected && e.ours);
        assert!(e.time == 1234 && e.qpc == 99);
        let up = key_event_from(WM_SYSKEYUP, &k, true, 0);
        assert!(!up.down && up.modifier_down);
        let m = MSLLHOOKSTRUCT {
            mouseData: u32::from(XBUTTON2) << 16,
            flags: 0,
            ..Default::default()
        };
        let me = mouse_event_from(WM_XBUTTONDOWN, &m, 0);
        assert!(me.button == 5 && me.down && !me.injected && !me.ours);
        let lu = mouse_event_from(WM_LBUTTONUP, &m, 0);
        assert!(lu.button == 1 && !lu.down);
        assert_eq!(mouse_event_from(WM_MOUSEMOVE, &m, 0).button, 0);
    }

    #[test]
    fn hooks_key_poller_held_at_start_is_not_a_press() {
        let mut p = KeyPoller::default();
        assert!(!p.step(true));
        assert!(!p.step(true));
        assert!(!p.step(false));
        assert!(p.step(true));
        assert!(!p.step(true));
    }

    #[test]
    fn hooks_mouse_unknown_xbutton_keeps_down() {
        let m = MSLLHOOKSTRUCT {
            mouseData: 0x0003 << 16,
            ..Default::default()
        };
        let e = mouse_event_from(WM_XBUTTONDOWN, &m, 0);
        assert!(e.button == 0 && e.down);
    }
}
