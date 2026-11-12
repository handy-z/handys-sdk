use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain2,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{Sleep, WaitForSingleObjectEx};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, HWND_TOPMOST, IsIconic, LWA_ALPHA,
    MSG, PM_REMOVE, PeekMessageW, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{HSTRING, Interface, PCWSTR, w};

unsafe extern "system" fn def_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

struct Device {
    swap: IDXGISwapChain2,
    // kept alive: they show the swap chain in the window
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    dc: ID2D1DeviceContext,
    waitable: HANDLE,
    w: i32,
    h: i32,
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            self.dc.SetTarget(None);
            if !self.waitable.is_invalid() {
                let _ = CloseHandle(self.waitable);
            }
        }
    }
}

/// Click-through, never-focused, top-most window over a target window's client area, drawn on the GPU: Direct2D into a
/// DirectComposition swap chain (premultiplied alpha, flip model, at most one frame queued), so a frame reaches the
/// screen at the next composition with no CPU copy. The game draws each frame in show()'s paint callback. Use from one
/// thread: the one that called create(), which must also call pump() for the window's messages.
pub struct GpuOverlay {
    class: HSTRING,
    hwnd: Cell<HWND>,
    placed: Cell<RECT>, // where the window is, in screen pixels
    shown: Cell<bool>,
    device: RefCell<Option<Device>>,
    brushes: RefCell<Vec<(D2D1_COLOR_F, ID2D1SolidColorBrush)>>, // a few colors
}

impl GpuOverlay {
    pub fn new(window_class: &str) -> Self {
        Self {
            class: HSTRING::from(window_class),
            hwnd: Cell::new(HWND::default()),
            placed: Cell::new(RECT::default()),
            shown: Cell::new(false),
            device: RefCell::new(None),
            brushes: RefCell::new(Vec::new()),
        }
    }

    /// Hidden until show(); the GPU objects come with the first show().
    pub fn create(&self) {
        if !self.hwnd.get().is_invalid() {
            return;
        }
        let inst = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(def_proc),
            hInstance: inst.into(),
            lpszClassName: PCWSTR(self.class.as_ptr()),
            ..Default::default()
        };
        unsafe { RegisterClassExW(&wc) }; // fails harmlessly when already registered
        // Layered + transparent: clicks go to the window below. No redirection bitmap: the content is the swap chain only.
        if let Ok(h) = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                &self.class,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(inst.into()),
                None,
            )
        } {
            self.hwnd.set(h);
            let _ = unsafe { SetLayeredWindowAttributes(h, Default::default(), 255, LWA_ALPHA) };
        }
    }

    /// Dispatches this thread's window messages.
    pub fn pump(&self) {
        let mut m = MSG::default();
        while unsafe { PeekMessageW(&mut m, Some(self.hwnd.get()), 0, 0, PM_REMOVE) }.as_bool() {
            unsafe { DispatchMessageW(&m) };
        }
    }

    /// Blocks until the display can take another frame (up to one refresh), so what's drawn next can be read as late
    /// as possible. Sleeps one 60 Hz frame while there is no swap chain to wait on.
    pub fn wait_for_frame(&self) {
        let waitable = self
            .device
            .borrow()
            .as_ref()
            .map(|d| d.waitable)
            .filter(|h| !h.is_invalid());
        match waitable {
            Some(h) => unsafe {
                WaitForSingleObjectEx(h, 100, true);
            },
            None => unsafe { Sleep(16) },
        }
    }

    pub fn hide(&self) {
        if !self.hwnd.get().is_invalid() && self.shown.get() {
            let _ = unsafe { ShowWindow(self.hwnd.get(), SW_HIDE) };
        }
        self.shown.set(false);
    }

    /// A solid brush on the current device, cached per color and dropped with the device. Only valid inside paint.
    pub fn brush(&self, c: D2D1_COLOR_F) -> Option<ID2D1SolidColorBrush> {
        if let Some((_, b)) = self.brushes.borrow().iter().find(|(color, _)| *color == c) {
            return Some(b.clone());
        }
        let dc = self.device.borrow().as_ref()?.dc.clone();
        let b = unsafe { dc.CreateSolidColorBrush(&c, None) }.ok()?;
        self.brushes.borrow_mut().push((c, b.clone()));
        Some(b)
    }

    /// After a lost device: everything is made again on the next show().
    fn drop_device(&self) {
        self.brushes.borrow_mut().clear();
        *self.device.borrow_mut() = None;
    }

    /// Direct2D draws into the swap chain's back buffer.
    fn bind_target(swap: &IDXGISwapChain2, dc: &ID2D1DeviceContext) -> bool {
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let Ok(surface) = (unsafe { swap.GetBuffer::<IDXGISurface>(0) }) else {
            return false;
        };
        let Ok(bitmap) = (unsafe { dc.CreateBitmapFromDxgiSurface(&surface, Some(&props)) }) else {
            return false;
        };
        unsafe { dc.SetTarget(&bitmap) }; // with a flip-model swap chain, buffer 0 is always the one to draw into
        true
    }

    /// Makes the GPU objects for a w x h client area; true when they exist.
    fn device(&self, w: i32, h: i32) -> bool {
        let mut slot = self.device.borrow_mut();
        if let Some(d) = slot.as_mut() {
            if d.w == w && d.h == h {
                return true;
            }
            // only the size changed
            unsafe { d.dc.SetTarget(None) };
            let ok = unsafe {
                d.swap
                    .ResizeBuffers(0, w as u32, h as u32, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS)
            }
            .is_ok()
                && Self::bind_target(&d.swap, &d.dc);
            if !ok {
                drop(slot);
                self.drop_device();
                return false;
            }
            d.w = w;
            d.h = h;
            return true;
        }
        let Some(d) = self.make_device(w, h) else {
            return false;
        };
        *slot = Some(d);
        true
    }

    fn make_device(&self, w: i32, h: i32) -> Option<Device> {
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT; // Direct2D needs it
        let mut d3d: Option<ID3D11Device> = None;
        let mut made = |kind| unsafe {
            D3D11CreateDevice(
                None,
                kind,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )
        };
        if made(D3D_DRIVER_TYPE_HARDWARE).is_err() && made(D3D_DRIVER_TYPE_WARP).is_err() {
            return None;
        }
        let d3d = d3d?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w as u32,
            Height: h as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: SWAP_FLAGS.0 as u32,
            ..Default::default()
        };
        unsafe {
            let dxgi: IDXGIDevice = d3d.cast().ok()?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
            let swap: IDXGISwapChain2 = factory
                .CreateSwapChainForComposition(&d3d, &desc, None)
                .ok()?
                .cast()
                .ok()?;
            swap.SetMaximumFrameLatency(1).ok()?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).ok()?;
            let target = dcomp.CreateTargetForHwnd(self.hwnd.get(), true).ok()?;
            let visual = dcomp.CreateVisual().ok()?;
            visual.SetContent(&swap).ok()?;
            target.SetRoot(&visual).ok()?;
            dcomp.Commit().ok()?;
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dc = d2d
                .CreateDevice(&dxgi)
                .ok()?
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .ok()?;
            if !Self::bind_target(&swap, &dc) {
                return None;
            }
            let waitable = swap.GetFrameLatencyWaitableObject();
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE); // ClearType needs an opaque background
            Some(Device {
                swap,
                _dcomp: dcomp,
                _target: target,
                _visual: visual,
                dc,
                waitable,
                w,
                h,
            })
        }
    }

    /// Covers target's client area and draws one frame with paint (the device context, already cleared to
    /// transparent, and the client area's size in pixels). Hides when the target is null or minimised, no Direct3D 11
    /// device (hardware or WARP) can be made, or the frame can't be presented (made again next frame).
    pub fn show(&self, target: HWND, paint: impl FnOnce(&ID2D1DeviceContext, D2D_SIZE_F)) {
        let mut rc = RECT::default();
        let mut origin = POINT::default();
        if self.hwnd.get().is_invalid()
            || target.is_invalid()
            || unsafe { IsIconic(target) }.as_bool()
            || unsafe { GetClientRect(target, &mut rc) }.is_err()
            || !unsafe { ClientToScreen(target, &mut origin) }.as_bool()
            || rc.right <= 0
            || rc.bottom <= 0
            || !self.device(rc.right, rc.bottom)
        {
            return self.hide();
        }
        let (dc, swap) = {
            let d = self.device.borrow();
            let d = d.as_ref().expect("made by device()");
            (d.dc.clone(), d.swap.clone())
        };
        let shown = unsafe {
            dc.BeginDraw();
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            paint(
                &dc,
                D2D_SIZE_F {
                    width: rc.right as f32,
                    height: rc.bottom as f32,
                },
            );
            dc.EndDraw(None, None).is_ok() && swap.Present(1, DXGI_PRESENT(0)).is_ok()
        };
        if !shown {
            // a lost device (driver update, GPU reset): made again on the next frame
            self.drop_device();
            return self.hide();
        }
        // after the frame is presented, so the window never shows the previous one at a new place
        let place = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + rc.right,
            bottom: origin.y + rc.bottom,
        };
        if !self.shown.get() || place != self.placed.get() {
            // top-most, above a fullscreen game
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd.get(),
                    Some(HWND_TOPMOST),
                    origin.x,
                    origin.y,
                    rc.right,
                    rc.bottom,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            self.placed.set(place);
            self.shown.set(true);
        }
    }
}

impl Drop for GpuOverlay {
    fn drop(&mut self) {
        self.drop_device();
        if !self.hwnd.get().is_invalid() {
            let _ = unsafe { DestroyWindow(self.hwnd.get()) };
        }
    }
}
