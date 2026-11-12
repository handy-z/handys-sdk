use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_ROUNDED_RECT};
use windows::Win32::Graphics::DirectWrite::{DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteTextFormat};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, CreateFontIndirectW, CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, FW_SEMIBOLD,
    FillRect, GetDC, GetMonitorInfoW, GetTextExtentPoint32W, HFONT, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_TOPMOST,
    NONCLIENTMETRICSW, RegisterClassExW, SPI_GETNONCLIENTMETRICS, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_DPICHANGED,
    WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::d2d::{Canvas, Vector2, dwrite_factory, text_format};
use super::theme::rgb;

const CLASS: PCWSTR = w!("handy-status-overlay");

struct State {
    hwnd: Cell<HWND>,
    font: Cell<HFONT>, // GDI, for when Direct2D can't start
    format: RefCell<Option<IDWriteTextFormat>>,
    canvas: RefCell<Canvas>, // premultiplied: a layered window, its edges blend into what's under it
    dpi: Cell<u32>,
    text: RefCell<String>,
    on: Cell<bool>,
}

/// Click-through, never-focused pill (e.g. "Macro on") in the primary work area's bottom-left corner: green dot when
/// on, red when off, white semibold system message font on translucent dark, at the primary monitor's DPI.
/// Use from one UI thread (the one with the message loop).
pub struct StatusOverlay {
    s: Box<State>, // the window's GWLP_USERDATA points here, so it never moves
}

impl Default for StatusOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusOverlay {
    pub fn new() -> Self {
        Self {
            s: Box::new(State {
                hwnd: Cell::new(HWND::default()),
                font: Cell::new(HFONT::default()),
                format: RefCell::new(None),
                canvas: RefCell::new(Canvas::new(D2D1_ALPHA_MODE_PREMULTIPLIED)),
                dpi: Cell::new(96),
                text: RefCell::new(String::new()),
                on: Cell::new(false),
            }),
        }
    }

    /// Shows it (no activation).
    pub fn create(&self) {
        let s = &*self.s;
        if !s.hwnd.get().is_invalid() {
            return;
        }
        let inst = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: inst.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        unsafe { RegisterClassExW(&wc) }; // fails harmlessly when already registered
        // Created at (0, 0), so GetDpiForWindow gives the primary monitor's DPI.
        let Ok(h) = (unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(inst.into()),
                Some(std::ptr::from_ref(s).cast()),
            )
        }) else {
            return;
        };
        s.hwnd.set(h);
        s.apply_font(unsafe { GetDpiForWindow(h) });
        s.render();
        let _ = unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
    }

    /// Redraws only when text or on changed.
    pub fn set(&self, text: &str, on: bool) {
        let s = &*self.s;
        if *s.text.borrow() == text && on == s.on.get() {
            return;
        }
        *s.text.borrow_mut() = text.to_owned();
        s.on.set(on);
        s.render();
    }

    pub(crate) fn destroy(&self) {
        let s = &*self.s;
        let h = s.hwnd.replace(HWND::default());
        if !h.is_invalid() {
            let _ = unsafe { DestroyWindow(h) };
        }
        let f = s.font.replace(HFONT::default());
        if !f.is_invalid() {
            let _ = unsafe { DeleteObject(f.into()) };
        }
    }
}

impl Drop for StatusOverlay {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl State {
    fn apply_font(&self, dpi: u32) {
        self.dpi.set(dpi);
        let old = self.font.replace(HFONT::default());
        if !old.is_invalid() {
            let _ = unsafe { DeleteObject(old.into()) };
        }
        let mut ncm = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let _ = unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                ncm.cbSize,
                Some((&raw mut ncm).cast()),
                0,
                dpi,
            )
        };
        ncm.lfMessageFont.lfWeight = FW_SEMIBOLD.0 as i32;
        self.font.set(unsafe { CreateFontIndirectW(&ncm.lfMessageFont) });
        *self.format.borrow_mut() = text_format(&ncm.lfMessageFont);
    }

    // Draws into a premultiplied 32-bit bitmap and hands it to UpdateLayeredWindow, so the rounded edges blend smoothly.
    fn render(&self) {
        let hwnd = self.hwnd.get();
        if hwnd.is_invalid() {
            return;
        }
        let dpi = self.dpi.get() as i32;
        let px = |dip: i32| dip * dpi / 96;
        let text = self.text.borrow().clone();
        let wide: Vec<u16> = text.encode_utf16().collect();
        let format = self.format.borrow().clone();
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let mut tm = DWRITE_TEXT_METRICS::default();
            let layout = format.as_ref().zip(dwrite_factory()).and_then(|(f, dw)| {
                let l = dw.CreateTextLayout(&wide, f, 10000.0, 10000.0).ok()?;
                let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                l.GetMetrics(&mut tm).ok()?;
                Some(l)
            });
            if layout.is_none() {
                // no DirectWrite: GDI measures
                let f = SelectObject(mem, self.font.get().into());
                let mut s = SIZE::default();
                let _ = GetTextExtentPoint32W(mem, &wide, &mut s);
                SelectObject(mem, f);
                tm.widthIncludingTrailingWhitespace = s.cx as f32;
                tm.height = s.cy as f32;
            }
            let (pad_x, pad_y, dot, gap) = (px(12), px(6), px(8), px(8));
            let w = pad_x + dot + gap + tm.widthIncludingTrailingWhitespace.ceil() as i32 + pad_x;
            let h = tm.height.ceil() as i32 + 2 * pad_y;
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return;
            };
            let old = SelectObject(mem, bmp.into());
            let all = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let light = if self.on.get() {
                rgb(108, 203, 95)
            } else {
                rgb(255, 99, 99)
            };
            let mut canvas = self.canvas.borrow_mut();
            if let Some(layout) = layout.as_ref().filter(|_| canvas.begin(mem, &all, None)) {
                let rt = canvas.rt().clone();
                rt.Clear(Some(&D2D1_COLOR_F::default()));
                let r = h as f32 / 2.0 - 0.5; // the 1-pixel edge sits on the pixels inside the bitmap's edge
                let pill = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 0.5,
                        top: 0.5,
                        right: w as f32 - 0.5,
                        bottom: h as f32 - 0.5,
                    },
                    radiusX: r,
                    radiusY: r,
                };
                if let Some(b) = canvas.brush(rgb(28, 28, 28), 220.0 / 255.0) {
                    rt.FillRoundedRectangle(&pill, &b);
                }
                if let Some(b) = canvas.brush(rgb(255, 255, 255), 50.0 / 255.0) {
                    rt.DrawRoundedRectangle(&pill, &b, 1.0, None);
                }
                canvas.ellipse(
                    Vector2::new(pad_x as f32 + dot as f32 / 2.0, h as f32 / 2.0),
                    dot as f32 / 2.0,
                    light,
                );
                if let Some(b) = canvas.brush(rgb(255, 255, 255), 1.0) {
                    rt.DrawTextLayout(
                        Vector2::new((pad_x + dot + gap) as f32, pad_y as f32),
                        layout,
                        &b,
                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
                canvas.end();
            } else {
                // no Direct2D: a square, opaque pill drawn with GDI
                let bg = CreateSolidBrush(rgb(28, 28, 28));
                let dot_brush = CreateSolidBrush(light);
                FillRect(mem, &all, bg);
                let d = RECT {
                    left: pad_x,
                    top: (h - dot) / 2,
                    right: pad_x + dot,
                    bottom: (h + dot) / 2,
                };
                FillRect(mem, &d, dot_brush);
                let _ = DeleteObject(bg.into());
                let _ = DeleteObject(dot_brush.into());
                let f = SelectObject(mem, self.font.get().into());
                SetBkMode(mem, TRANSPARENT);
                SetTextColor(mem, rgb(255, 255, 255));
                let _ = TextOutW(mem, pad_x + dot + gap, pad_y, &wide);
                SelectObject(mem, f);
                // GDI leaves alpha at 0: make it opaque
                let px = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize);
                for a in px.iter_mut().skip(3).step_by(4) {
                    *a = 255;
                }
            }
            drop(canvas); // UpdateLayeredWindow can move us to another monitor, and WM_DPICHANGED renders again
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(
                MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
                &mut mi,
            );
            let pos = POINT {
                x: mi.rcWork.left + px(12),
                y: mi.rcWork.bottom - h - px(12),
            }; // above the taskbar
            let src = POINT::default();
            let size = SIZE { cx: w, cy: h };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&pos),
                Some(&size),
                Some(mem),
                Some(&src),
                Default::default(),
                Some(&blend),
                ULW_ALPHA,
            );
            // above a fullscreen game
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        // SAFETY: set from the StatusOverlay's boxed State, which outlives its window (destroy() runs on drop).
        let o = (GetWindowLongPtrW(h, GWLP_USERDATA) as *const State).as_ref();
        if let Some(o) = o
            && !o.hwnd.get().is_invalid()
        {
            match msg {
                WM_DPICHANGED => {
                    o.apply_font(u32::from((wp.0 >> 16) as u16));
                    o.render();
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
