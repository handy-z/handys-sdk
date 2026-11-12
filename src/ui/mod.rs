pub mod d2d;
pub mod dashboard;
pub mod gpu_overlay;
pub mod overlay;
pub mod theme;

/// Header dot colour: grey closed, amber on the way, green ready, red stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Busy,
    Good,
    Bad,
}

/// Opens `url` in the default browser; false when the shell refused.
pub fn open_url(url: &str) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    // ShellExecute's result is an HINSTANCE that is a status code: above 32 is success.
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL) };
    r.0 as usize > 32
}
