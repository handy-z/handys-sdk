use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::HSTRING;

/// RGB(r, g, b) as GDI packs it (0x00BBGGRR).
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn r(c: COLORREF) -> u8 {
    c.0 as u8
}
fn g(c: COLORREF) -> u8 {
    (c.0 >> 8) as u8
}
fn b(c: COLORREF) -> u8 {
    (c.0 >> 16) as u8
}

/// Windows 11 colors for the dashboard and overlay. `dark` follows Settings > Personalization > Colors > app mode;
/// accent is the user's accent color (lightened in dark mode, as Windows does).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    pub bg: COLORREF,
    pub card: COLORREF,
    pub stroke: COLORREF,
    pub text: COLORREF,
    pub text2: COLORREF,
    pub field: COLORREF,
    pub field_line: COLORREF,
    pub accent: COLORREF,
    pub on_accent: COLORREF,
    pub good: COLORREF,
    pub busy: COLORREF,
    pub bad: COLORREF,
}

fn read_dword(key: &str, value: &str) -> Option<u32> {
    let mut out = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut out).cast()),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(out)
}

fn mix(a: COLORREF, bb: COLORREF, pct_b: i32) -> COLORREF {
    let ch = |x: u8, y: u8| ((x as i32 * (100 - pct_b) + y as i32 * pct_b) / 100) as u8;
    rgb(ch(r(a), r(bb)), ch(g(a), g(bb)), ch(b(a), b(bb)))
}

fn luminance(c: COLORREF) -> f32 {
    // WCAG relative luminance
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r(c)) + 0.7152 * lin(g(c)) + 0.0722 * lin(b(c))
}

fn contrast(a: COLORREF, bb: COLORREF) -> f32 {
    let (la, lb) = (luminance(a), luminance(bb));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// c moved toward white (on a dark bg) or black (on a light one) until it has 3:1 contrast with bg (WCAG's floor for
/// large text), so a game's very dark or pale text colors still read on the card. Already readable: unchanged.
pub(crate) fn readable_on(c: COLORREF, bg: COLORREF) -> COLORREF {
    let away = if luminance(bg) < 0.5 {
        rgb(255, 255, 255)
    } else {
        rgb(0, 0, 0)
    };
    (0..100)
        .step_by(5)
        .map(|pct| mix(c, away, pct))
        .find(|&m| contrast(m, bg) >= 3.0)
        .unwrap_or(away)
}

/// The same for outlined text: an outline that itself reads on bg (3:1) carries the text, so c stays as it is.
pub(crate) fn readable_on_outlined(c: COLORREF, bg: COLORREF, outline: COLORREF) -> COLORREF {
    if contrast(outline, bg) >= 3.0 {
        c
    } else {
        readable_on(c, bg)
    }
}

/// top laid over bg at opacity 0..1: what a semi-transparent outline looks like on a flat card.
pub(crate) fn blend(top: COLORREF, bg: COLORREF, opacity: f32) -> COLORREF {
    mix(bg, top, (opacity.clamp(0.0, 1.0) * 100.0).round() as i32)
}

/// The color t (0..1) of the way along evenly spaced gradient stops; 0 (black) for none.
pub(crate) fn gradient_at(stops: &[COLORREF], t: f32) -> COLORREF {
    if stops.is_empty() {
        return COLORREF(0);
    }
    let pos = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (pos as usize).min(stops.len() - 1);
    let j = (i + 1).min(stops.len() - 1);
    mix(stops[i], stops[j], ((pos - i as f32) * 100.0).round() as i32)
}

pub(crate) fn system_theme() -> Theme {
    let light = read_dword(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "AppsUseLightTheme",
    );
    let dark = light == Some(0);
    // AccentColor is 0xAABBGGRR, so its low 24 bits are already a COLORREF.
    let a = read_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")
        .map_or(rgb(0, 103, 192), |v| COLORREF(v & 0xFF_FFFF));
    if dark {
        return Theme {
            dark: true,
            bg: rgb(32, 32, 32),
            card: rgb(43, 43, 43),
            stroke: rgb(29, 29, 29),
            text: rgb(255, 255, 255),
            text2: rgb(207, 207, 207),
            field: rgb(55, 55, 55),
            field_line: rgb(140, 140, 140),
            accent: mix(a, rgb(255, 255, 255), 45),
            on_accent: rgb(0, 0, 0),
            good: rgb(108, 203, 95),
            busy: rgb(252, 225, 0),
            bad: rgb(255, 153, 164),
        };
    }
    Theme {
        dark: false,
        bg: rgb(243, 243, 243),
        card: rgb(251, 251, 251),
        stroke: rgb(229, 229, 229),
        text: rgb(27, 27, 27),
        text2: rgb(96, 96, 96),
        field: rgb(255, 255, 255),
        field_line: rgb(134, 134, 134),
        accent: mix(a, rgb(0, 0, 0), 10),
        on_accent: rgb(255, 255, 255),
        good: rgb(15, 123, 15),
        busy: rgb(157, 93, 0),
        bad: rgb(196, 43, 28),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_theme_readable_and_gradient() {
        let (dark, light) = (rgb(43, 43, 43), rgb(251, 251, 251));
        assert_eq!(readable_on(rgb(255, 162, 0), dark), rgb(255, 162, 0)); // Carrot already reads on the dark card
        assert_ne!(readable_on(rgb(58, 61, 62), dark), rgb(58, 61, 62)); // Darkened is lightened...
        assert!(r(readable_on(rgb(58, 61, 62), dark)) >= 110); // ...to 3:1 contrast (gray ~115 on #2B2B2B)
        assert!(r(readable_on(rgb(255, 240, 188), light)) < 160); // Shiny's pale yellow darkened on a light card
        // Fisch's coins: #362D03 at 70% over the light card reads, so the pale fill stays; over the dark card it doesn't
        let (coins, hud_stroke) = (rgb(255, 253, 228), rgb(54, 45, 3));
        assert_eq!(readable_on_outlined(coins, light, blend(hud_stroke, light, 0.7)), coins);
        // Chaotic: black, white outline
        assert_eq!(
            readable_on_outlined(rgb(20, 19, 22), dark, rgb(255, 255, 255)),
            rgb(20, 19, 22)
        );
        assert_eq!(
            readable_on_outlined(rgb(58, 61, 62), dark, blend(COLORREF(0), dark, 0.64)),
            readable_on(rgb(58, 61, 62), dark)
        );
        assert!(
            blend(rgb(200, 0, 100), light, 0.0) == light && blend(rgb(200, 0, 100), light, 1.0) == rgb(200, 0, 100)
        );
        let stops = [rgb(0, 0, 0), rgb(200, 100, 0), rgb(0, 0, 200)];
        assert!(gradient_at(&stops, 0.0) == rgb(0, 0, 0) && gradient_at(&stops, 1.0) == rgb(0, 0, 200));
        assert!(gradient_at(&stops, 0.5) == rgb(200, 100, 0) && gradient_at(&stops, 0.25) == rgb(100, 50, 0));
        assert!(gradient_at(&[], 0.5) == COLORREF(0) && gradient_at(&[rgb(1, 2, 3)], 0.7) == rgb(1, 2, 3));
    }
}
