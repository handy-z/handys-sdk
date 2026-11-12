use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW};

use crate::util::text::from_wide;

/// Physical key or button state right now (high bit of GetAsyncKeyState).
pub fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyKind {
    #[default]
    Keyboard,
    Mouse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub kind: KeyKind,
    pub scan: u16,
    pub extended: bool,
    /// 1=L 2=R 3=M 4=X1 5=X2
    pub mouse_button: i32,
}

const fn kb(scan: u16, extended: bool) -> Key {
    Key {
        kind: KeyKind::Keyboard,
        scan,
        extended,
        mouse_button: 0,
    }
}
const fn ms(b: i32) -> Key {
    Key {
        kind: KeyKind::Mouse,
        scan: 0,
        extended: false,
        mouse_button: b,
    }
}

pub const SPACE: Key = kb(0x39, false);
pub const SHIFT: Key = kb(0x2A, false); // LSHIFT
pub const ENTER: Key = kb(0x1C, false);
pub const ESC: Key = kb(0x01, false);
pub const R: Key = kb(0x13, false);
pub const SLASH: Key = kb(0x35, false);
pub const BACKSLASH: Key = kb(0x2B, false);
pub const MOUSE_LEFT: Key = ms(1);

/// Case-insensitive; None if the label is not recognised.
pub fn key_from_label(label: &str) -> Option<Key> {
    let s = label.to_ascii_uppercase();
    let b = s.as_bytes();
    if let [c @ b'A'..=b'Z'] = b {
        const LETTERS: [u16; 26] = [
            0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13,
            0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
        ];
        return Some(kb(LETTERS[usize::from(c - b'A')], false));
    }
    const DIGITS: [&str; 10] = [
        "ZERO", "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE",
    ];
    if let Some(i) = DIGITS.iter().position(|d| *d == s) {
        return Some(digit_key(i as i32));
    }
    if (2..=3).contains(&b.len()) && b[0] == b'F' && b[1].is_ascii_digit() {
        let n: u16 = s[1..].parse().ok()?; // "F1X" is not F1
        return match n {
            1..=10 => Some(kb(0x3A + n, false)),
            11 => Some(kb(0x57, false)),
            12 => Some(kb(0x58, false)),
            _ => None,
        };
    }
    Some(match s.as_str() {
        "M1" => ms(1),
        "M2" => ms(2),
        "M3" => ms(3),
        "LCTRL" => kb(0x1D, false),
        "RCTRL" => kb(0x1D, true),
        "LSHIFT" => kb(0x2A, false),
        "RSHIFT" => kb(0x36, false),
        "LALT" => kb(0x38, false),
        "RALT" => kb(0x38, true),
        "SPACE" => kb(0x39, false),
        "TAB" => kb(0x0F, false),
        "ENTER" | "RETURN" => kb(0x1C, false),
        _ => return None,
    })
}

/// `label` if known, else `fallback`, with true for "used the fallback". The fallback must be valid.
pub fn resolve_key(label: &str, fallback: &str) -> (Key, bool) {
    match key_from_label(label) {
        Some(k) => (k, false),
        None => (key_from_label(fallback).unwrap_or_default(), true),
    }
}

/// Key name as Windows shows it (layout dependent), else "VK 0x%02X".
pub fn vk_name(vk: u32) -> String {
    let sc = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as i32;
    let mut w = [0u16; 64];
    if sc != 0 && unsafe { GetKeyNameTextW(sc << 16, &mut w) } > 0 {
        return from_wide(&w);
    }
    format!("VK 0x{vk:02X}")
}

/// d in 0..=9; the top-row digit key.
pub fn digit_key(d: i32) -> Key {
    kb(if d == 0 { 0x0B } else { (0x01 + d) as u16 }, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F1;

    fn scan(label: &str) -> u16 {
        key_from_label(label).unwrap().scan
    }

    #[test]
    fn key_known_labels() {
        let e = key_from_label("E").unwrap();
        assert!(e.kind == KeyKind::Keyboard && e.scan == 0x12);
        let l = key_from_label("lctrl").unwrap();
        assert!(l.scan == 0x1D && !l.extended);
        let r = key_from_label("RCTRL").unwrap();
        assert!(r.scan == 0x1D && r.extended);
        let m = key_from_label("M1").unwrap();
        assert!(m.kind == KeyKind::Mouse && m.mouse_button == 1);
        assert_eq!(scan("One"), 0x02);
        assert_eq!(scan("Q"), 0x10);
        assert_eq!(scan("Zero"), 0x0B);
        assert_eq!(scan("F11"), 0x57);
        assert_eq!(scan("F10"), 0x44);
        assert_eq!(scan("Return"), 0x1C);
        assert!(key_from_label("NUMPADSTAR").is_none());
        assert!(key_from_label("F1X").is_none());
        assert!(key_from_label("F13").is_none());
    }

    #[test]
    fn key_unknown_falls_back() {
        let (k, fb) = resolve_key("NUMPADSTAR", "E");
        assert!(k.scan == 0x12 && fb);
        let (k, fb) = resolve_key("", "LCTRL");
        assert!(k.scan == 0x1D && fb);
        let (k, fb) = resolve_key("Q", "E");
        assert!(k.scan == 0x10 && !fb);
    }

    #[test]
    fn key_digit_key() {
        assert_eq!(digit_key(0).scan, 0x0B);
        assert_eq!(digit_key(1).scan, 0x02);
        assert_eq!(digit_key(9).scan, 0x0A);
    }

    #[test]
    fn key_vk_name() {
        assert_eq!(vk_name(u32::from(VK_F1.0)), "F1");
        assert!(vk_name(0xFF).starts_with("VK 0x"));
    }
}
