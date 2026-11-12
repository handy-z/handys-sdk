use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

use super::key::{Key, KeyKind};
use crate::util::timing::precise_wait;

/// Stamped on every injected event (dwExtraInfo) so our own LL hooks can ignore them.
pub(crate) const INJECTED_TAG: usize = 0x4856_4231;

/// Keyboard: scan code (+extended, +keyup), wVk 0. Mouse: button flags only, never MOUSEEVENTF_MOVE.
pub(crate) fn make_input(key: &Key, down: bool) -> INPUT {
    if key.kind == KeyKind::Keyboard {
        let mut flags = KEYEVENTF_SCANCODE;
        if key.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        let ki = KEYBDINPUT {
            wScan: key.scan,
            dwFlags: flags,
            dwExtraInfo: INJECTED_TAG,
            ..Default::default()
        };
        return INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        };
    }
    let mut mi = MOUSEINPUT {
        dwExtraInfo: INJECTED_TAG,
        ..Default::default()
    };
    mi.dwFlags = match (key.mouse_button, down) {
        (1, true) => MOUSEEVENTF_LEFTDOWN,
        (1, false) => MOUSEEVENTF_LEFTUP,
        (2, true) => MOUSEEVENTF_RIGHTDOWN,
        (2, false) => MOUSEEVENTF_RIGHTUP,
        (3, true) => MOUSEEVENTF_MIDDLEDOWN,
        (3, false) => MOUSEEVENTF_MIDDLEUP,
        (b, d) => {
            mi.mouseData = u32::from(if b == 5 { XBUTTON2 } else { XBUTTON1 });
            if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }
        }
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    }
}

fn send(key: &Key, down: bool) {
    let input = make_input(key, down);
    // A dropped up event leaves the key held in the game, so ups get two retries; downs are sent once.
    for _ in 0..if down { 1 } else { 3 } {
        if unsafe { SendInput(&[input], size_of::<INPUT>() as i32) } != 0 {
            break;
        }
    }
}

pub fn press(key: &Key) {
    send(key, true);
}

pub fn release(key: &Key) {
    send(key, false);
}

/// press, precise_wait(hold), release.
pub fn tap(key: &Key, hold: Duration) {
    press(key);
    precise_wait(hold);
    release(key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::key::{MOUSE_LEFT, SPACE};
    use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MOVE;

    #[test]
    fn send_make_input_keyboard() {
        let d = make_input(
            &Key {
                kind: KeyKind::Keyboard,
                scan: 0x1D,
                extended: true,
                mouse_button: 0,
            },
            true,
        );
        let ki = unsafe { d.Anonymous.ki };
        assert!(d.r#type == INPUT_KEYBOARD && ki.wVk.0 == 0 && ki.wScan == 0x1D);
        assert_eq!(ki.dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY);
        assert_eq!(ki.dwExtraInfo, INJECTED_TAG);
    }

    #[test]
    fn send_make_input_up_flags() {
        assert_eq!(
            unsafe { make_input(&SPACE, false).Anonymous.ki }.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP
        );
        assert_eq!(
            unsafe { make_input(&MOUSE_LEFT, false).Anonymous.mi }.dwFlags,
            MOUSEEVENTF_LEFTUP
        );
    }

    #[test]
    fn send_make_input_mouse_never_moves() {
        let mouse = |b| Key {
            kind: KeyKind::Mouse,
            scan: 0,
            extended: false,
            mouse_button: b,
        };
        for b in 1..=5 {
            let i = make_input(&mouse(b), true);
            let mi = unsafe { i.Anonymous.mi };
            assert!(
                i.r#type == INPUT_MOUSE && (mi.dwFlags & MOUSEEVENTF_MOVE).0 == 0 && mi.dwExtraInfo == INJECTED_TAG
            );
        }
        assert_eq!(
            unsafe { make_input(&mouse(5), true).Anonymous.mi }.mouseData,
            u32::from(XBUTTON2)
        );
    }
}
