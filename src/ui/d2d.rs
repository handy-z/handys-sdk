//! How the dashboard and pill draw: Direct2D on its CPU rasterizer into memory bitmaps copied to GDI device contexts,
//! DirectWrite text, WIC images.
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, GENERIC_READ, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap,
    ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFactory5,
    IDWriteFontCollection, IDWriteFontCollection1, IDWriteInlineObject, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AddFontResourceExW, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, FR_PRIVATE, HBITMAP, HDC, HGDIOBJ, IntersectRect, LOGFONTW, SRCCOPY,
    SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory, WICBitmapCacheOnLoad,
    WICBitmapDitherTypeNone, WICBitmapLockWrite, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, w};
use windows_numerics::Matrix3x2;
pub use windows_numerics::Vector2;

// One per thread, which is the same as one per process for the one UI thread that uses them (the factories
// are single-threaded and COM objects stay on their thread). Never torn down: at process exit the main thread's
// thread-locals are dropped after the COM DLLs have gone, and releasing into them then crashes the exit.
thread_local! {
    static D2D: ManuallyDrop<OnceCell<Option<ID2D1Factory>>> = const { ManuallyDrop::new(OnceCell::new()) };
    static DWRITE: ManuallyDrop<OnceCell<Option<IDWriteFactory>>> = const { ManuallyDrop::new(OnceCell::new()) };
    static WIC: ManuallyDrop<OnceCell<Option<IWICImagingFactory>>> = const { ManuallyDrop::new(OnceCell::new()) };
    // Fonts added with add_font_file, and the DirectWrite collection made from them (rebuilt on each add).
    static FONT_FILES: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static FONTS: ManuallyDrop<RefCell<Option<IDWriteFontCollection1>>> =
        const { ManuallyDrop::new(RefCell::new(None)) };
}

/// Direct2D factory, made on first use; None when Windows can't make one. UI thread.
pub(crate) fn d2d_factory() -> Option<ID2D1Factory> {
    D2D.with(|f| {
        f.get_or_init(|| unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.ok())
            .clone()
    })
}

pub fn dwrite_factory() -> Option<IDWriteFactory> {
    DWRITE.with(|f| {
        f.get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.ok())
            .clone()
    })
}

pub(crate) fn wic_factory() -> Option<IWICImagingFactory> {
    WIC.with(|f| {
        f.get_or_init(|| unsafe {
            // WIC is COM: make sure this (the UI) thread has it. Already set up, in either model, is fine; never torn down.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
        })
        .clone()
    })
}

pub(crate) fn d2d_color(c: COLORREF, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (c.0 & 0xFF) as f32 / 255.0,
        g: ((c.0 >> 8) & 0xFF) as f32 / 255.0,
        b: ((c.0 >> 16) & 0xFF) as f32 / 255.0,
        a: alpha,
    }
}

fn rebuild_fonts() {
    let Some(f) = dwrite_factory().and_then(|f| f.cast::<IDWriteFactory5>().ok()) else {
        return; // Windows 10 1703+
    };
    let Ok(builder) = (unsafe { f.CreateFontSetBuilder() }) else {
        return;
    };
    FONT_FILES.with_borrow(|files| {
        for path in files {
            if let Ok(file) = unsafe { f.CreateFontFileReference(&HSTRING::from(path.as_path()), None) } {
                let _ = unsafe { builder.AddFontFile(&file) };
            }
        }
    });
    if let Ok(set) = unsafe { builder.CreateFontSet() }
        && let Ok(fonts) = unsafe { f.CreateFontCollectionFromFontSet(&set) }
    {
        FONTS.with(|g| *g.borrow_mut() = Some(fonts));
    }
}

/// A font file for this process only, for both GDI (the Win32 controls) and DirectWrite (everything drawn here), e.g.
/// a game's own font. Each face (Regular, Bold, Italic...) is its own file. False when neither could load it.
pub fn add_font_file(path: &Path) -> bool {
    let gdi = unsafe { AddFontResourceExW(&HSTRING::from(path), FR_PRIVATE, None) } > 0;
    if !path.exists() {
        return gdi;
    }
    if !FONT_FILES.with_borrow(|v| v.iter().any(|p| p == path)) {
        FONT_FILES.with_borrow_mut(|v| v.push(path.to_path_buf()));
        rebuild_fonts();
    }
    true
}

/// DirectWrite text format from a GDI font, in pixels (the canvases draw at 96 DPI: 1 unit = 1 pixel). A family added
/// with add_font_file comes from those files, any other from the system's fonts.
pub(crate) fn text_format(font: &LOGFONTW) -> Option<IDWriteTextFormat> {
    let dw = dwrite_factory()?;
    let face = PCWSTR(font.lfFaceName.as_ptr());
    let fonts: Option<IDWriteFontCollection> = FONTS.with(|g| {
        let g = g.borrow();
        let g = g.as_ref()?;
        let (mut index, mut exists) = (0u32, BOOL(0));
        unsafe { g.FindFamilyName(face, &mut index, &mut exists) }.ok()?;
        exists.as_bool().then(|| g.clone().into())
    });
    // A negative lfHeight is the em size (what DirectWrite wants); a positive one is the cell height, about 1.25 ems.
    let size = if font.lfHeight < 0 {
        -font.lfHeight as f32
    } else {
        (font.lfHeight as f32 * 0.8).max(1.0)
    };
    let weight = DWRITE_FONT_WEIGHT(if font.lfWeight != 0 { font.lfWeight } else { 400 });
    let style = if font.lfItalic != 0 {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    unsafe {
        dw.CreateTextFormat(
            face,
            fonts.as_ref(),
            weight,
            style,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!(""),
        )
    }
    .ok()
}

/// Recolors premultiplied BGRA pixels to `tint` (0xRRGGBB) at each pixel's own brightness (its strongest channel), so
/// shape and shading stay: how a game shows a recolored item. Premultiplied stays valid: no channel exceeds alpha.
pub(crate) fn tint_pixels(px: &mut [u8], tint: u32) {
    let [_, r, g, b] = tint.to_be_bytes();
    for p in px.as_chunks_mut::<4>().0 {
        let v = u32::from(p[0].max(p[1]).max(p[2]));
        p[0] = (u32::from(b) * v / 255) as u8;
        p[1] = (u32::from(g) * v / 255) as u8;
        p[2] = (u32::from(r) * v / 255) as u8;
    }
}

/// An image file decoded into memory (premultiplied BGRA), with no lock left on the file, recolored to `tint`
/// (0xRRGGBB, tint_pixels) when given. None when unreadable.
pub(crate) fn load_image(file: &Path, tint: Option<u32>) -> Option<IWICBitmapSource> {
    let wic = wic_factory()?;
    if file.as_os_str().is_empty() {
        return None;
    }
    unsafe {
        let decoder = wic
            .CreateDecoderFromFilename(&HSTRING::from(file), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter = wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        // decoded now: file closed
        let bitmap = wic.CreateBitmapFromSource(&converter, WICBitmapCacheOnLoad).ok()?;
        if let Some(tint) = tint {
            let lock = bitmap.Lock(std::ptr::null(), WICBitmapLockWrite.0 as u32).ok()?;
            let (mut size, mut data) = (0u32, std::ptr::null_mut());
            lock.GetDataPointer(&mut size, &mut data).ok()?;
            if !data.is_null() {
                tint_pixels(std::slice::from_raw_parts_mut(data, size as usize), tint);
            }
        }
        Some(bitmap.into())
    }
}

/// Premultiplied BGRA pixels (w x h, 4*w bytes per row) copied into a WIC bitmap. None for no pixels or a short buffer.
pub(crate) fn pixels_image(w: u32, h: u32, bgra: &[u8]) -> Option<IWICBitmapSource> {
    let len = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if len == 0 || bgra.len() < len {
        return None;
    }
    let wic = wic_factory()?;
    unsafe { wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppPBGRA, w * 4, &bgra[..len]) }
        .ok()
        .map(Into::into)
}

pub(crate) fn icon_image(icon: HICON) -> Option<IWICBitmapSource> {
    let wic = wic_factory()?;
    if icon.is_invalid() {
        return None;
    }
    unsafe {
        let bgra = wic.CreateBitmapFromHICON(icon).ok()?;
        let converter = wic.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &bgra,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        wic.CreateBitmapFromSource(&converter, WICBitmapCacheOnLoad)
            .ok()
            .map(Into::into)
    }
}

pub(crate) fn rect_f(r: &RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

// One render target per size, each bound once to its own memory bitmap. Full dashboard repaints measured 6.5 ms this
// way, 8.7 ms with the old GDI+, 10.4 ms binding one target to each control's DC in turn, 15.8 ms on a GPU target.
// Brushes and bitmaps belong to one target.
struct Target {
    rt: Option<ID2D1DCRenderTarget>,
    mem: HDC, // the target's own bitmap, bound once; end() copies it to the DC being painted
    bmp: HBITMAP,
    old: HGDIOBJ,
    brushes: HashMap<(u32, u32), ID2D1SolidColorBrush>,
    // per source; the source is held so its address can't be reused
    bitmaps: HashMap<usize, (IWICBitmapSource, ID2D1Bitmap)>,
}

impl Drop for Target {
    fn drop(&mut self) {
        self.brushes.clear();
        self.bitmaps.clear();
        self.rt = None;
        unsafe {
            if !self.mem.is_invalid() {
                SelectObject(self.mem, self.old);
                let _ = DeleteDC(self.mem);
            }
            if !self.bmp.is_invalid() {
                let _ = DeleteObject(self.bmp.into());
            }
        }
    }
}

/// Direct2D drawing into GDI device contexts (a window's WM_PAINT, an owner-drawn or custom-drawn control): drawn on
/// Direct2D's CPU rasterizer into a memory bitmap, then copied to the DC. Coordinates are the DC's own, in pixels.
/// Text is DirectWrite. If Direct2D can't start, begin() is false and the caller draws with GDI.
pub struct Canvas {
    alpha: D2D1_ALPHA_MODE,
    targets: HashMap<(i32, i32), Target>, // by width, height
    on: Option<(i32, i32)>,               // the one drawing, between begin() and end()
    dest: HDC,                            // where it goes, and where in it
    area: RECT,
    blit: RECT, // the part of area drawn and copied
    // the "â€¦" for cut text, per format (the format is held so its address can't be reused)
    ellipses: HashMap<usize, (IDWriteTextFormat, IDWriteInlineObject)>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new(D2D1_ALPHA_MODE_IGNORE)
    }
}

impl Canvas {
    /// IGNORE for opaque windows (ClearType text); PREMULTIPLIED for a layered window's 32-bit bitmap.
    pub fn new(alpha: D2D1_ALPHA_MODE) -> Self {
        Self {
            alpha,
            targets: HashMap::new(),
            on: None,
            dest: HDC::default(),
            area: RECT::default(),
            blit: RECT::default(),
            ellipses: HashMap::new(),
        }
    }

    fn make(&self, w: i32, h: i32) -> Option<Target> {
        let factory = d2d_factory()?;
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
        let mut t = Target {
            rt: None,
            mem: unsafe { CreateCompatibleDC(None) },
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            brushes: HashMap::new(),
            bitmaps: HashMap::new(),
        };
        if t.mem.is_invalid() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        t.bmp = unsafe { CreateDIBSection(Some(t.mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        t.old = unsafe { SelectObject(t.mem, t.bmp.into()) };
        // Direct2D's own CPU rasterizer: drawing for GDI DCs, a GPU target copies every control back from the GPU,
        // which measured slower (see Target). Same pixels either way.
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: self.alpha,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let all = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let rt = unsafe { factory.CreateDCRenderTarget(&props) }.ok()?;
        unsafe { rt.BindDC(t.mem, &all) }.ok()?;
        // ClearType needs an opaque background to blend into
        unsafe {
            rt.SetTextAntialiasMode(if self.alpha == D2D1_ALPHA_MODE_IGNORE {
                D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE
            } else {
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE
            })
        };
        t.rt = Some(rt);
        Some(t)
    }

    /// Draw in area until end(). With clip, only the part of area inside it is drawn and copied: a window repainting
    /// part of itself passes its whole client area (one size, one bitmap) and the part to repaint.
    pub(crate) fn begin(&mut self, dc: HDC, area: &RECT, clip: Option<&RECT>) -> bool {
        let (w, h) = (area.right - area.left, area.bottom - area.top);
        if self.on.is_some() || w <= 0 || h <= 0 {
            return false;
        }
        self.blit = *area;
        if let Some(c) = clip
            && !unsafe { IntersectRect(&mut self.blit, area, c) }.as_bool()
        {
            return false; // nothing of area to repaint
        }
        let key = (w, h);
        if self.targets.len() > 16 {
            self.targets.clear(); // a DPI change or a resize: start over now and then
        }
        if !self.targets.contains_key(&key) {
            let Some(t) = self.make(w, h) else {
                return false;
            };
            self.targets.insert(key, t);
        }
        self.on = Some(key);
        self.dest = dc;
        self.area = *area;
        let rt = self.rt().clone();
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::translation(-area.left as f32, -area.top as f32));
            rt.PushAxisAlignedClip(&rect_f(&self.blit), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        true
    }

    /// A lost target is made again on the next begin().
    pub fn end(&mut self) {
        let Some(key) = self.on.take() else {
            return;
        };
        let t = &self.targets[&key];
        let rt = t.rt.as_ref().expect("made in begin");
        unsafe { rt.PopAxisAlignedClip() };
        if let Err(e) = unsafe { rt.EndDraw(None, None) }
            && e.code() == D2DERR_RECREATE_TARGET
        {
            self.targets.remove(&key); // made again on the next begin()
            return;
        }
        let b = self.blit;
        let _ = unsafe {
            BitBlt(
                self.dest,
                b.left,
                b.top,
                b.right - b.left,
                b.bottom - b.top,
                Some(t.mem),
                b.left - self.area.left,
                b.top - self.area.top,
                SRCCOPY,
            )
        };
    }

    fn target(&mut self) -> &mut Target {
        let key = self.on.expect("between begin() and end()");
        self.targets.get_mut(&key).expect("made in begin")
    }

    /// The render target being drawn, between begin() and end().
    pub(crate) fn rt(&mut self) -> &ID2D1RenderTarget {
        self.target().rt.as_ref().expect("made in begin")
    }

    /// None only when Direct2D can't make one (out of memory); the drawing that wanted it is skipped.
    pub fn brush(&mut self, c: COLORREF, alpha: f32) -> Option<ID2D1SolidColorBrush> {
        let t = self.target();
        if let Some(b) = t.brushes.get(&(c.0, alpha.to_bits())) {
            return Some(b.clone());
        }
        let rt = t.rt.as_ref().expect("made in begin");
        let b = unsafe { rt.CreateSolidColorBrush(&d2d_color(c, alpha), None) }.ok()?;
        if t.brushes.len() > 512 {
            t.brushes.clear(); // colors that vary per frame would otherwise pile up while the window lives
        }
        t.brushes.insert((c.0, alpha.to_bits()), b.clone());
        Some(b)
    }

    pub fn fill(&mut self, r: &D2D_RECT_F, c: COLORREF, alpha: f32) {
        if let Some(b) = self.brush(c, alpha) {
            unsafe { self.rt().FillRectangle(r, &b) };
        }
    }

    /// Filled, and outlined 1 pixel inside its edge (crisp, as Windows 11 draws card edges) when stroke is given.
    pub(crate) fn round_rect(&mut self, r: &D2D_RECT_F, radius: f32, fill: COLORREF, stroke: Option<COLORREF>) {
        let rad = radius.min((r.right - r.left) / 2.0).min((r.bottom - r.top) / 2.0);
        if let Some(b) = self.brush(fill, 1.0) {
            unsafe {
                self.rt().FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: *r,
                        radiusX: rad,
                        radiusY: rad,
                    },
                    &b,
                )
            };
        }
        if let Some(s) = stroke
            && let Some(b) = self.brush(s, 1.0)
        {
            // centered on the pixel row inside the edge, so the 1-pixel line isn't smeared over two
            let inner = D2D_RECT_F {
                left: r.left + 0.5,
                top: r.top + 0.5,
                right: r.right - 0.5,
                bottom: r.bottom - 0.5,
            };
            let rr = (rad - 0.5).max(0.0);
            unsafe {
                self.rt().DrawRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: inner,
                        radiusX: rr,
                        radiusY: rr,
                    },
                    &b,
                    1.0,
                    None,
                )
            };
        }
    }

    pub(crate) fn ellipse(&mut self, center: Vector2, radius: f32, c: COLORREF) {
        if let Some(b) = self.brush(c, 1.0) {
            unsafe {
                self.rt().FillEllipse(
                    &D2D1_ELLIPSE {
                        point: center,
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &b,
                )
            };
        }
    }

    pub fn line(&mut self, a: Vector2, b: Vector2, c: COLORREF, width: f32) {
        if let Some(br) = self.brush(c, 1.0) {
            unsafe { self.rt().DrawLine(a, b, &br, width, None) };
        }
    }

    /// One line, vertically centered in r, cut with an ellipsis when too long; wrap: word-wrapped from r's top instead.
    pub fn text(
        &mut self,
        s: &str,
        format: Option<&IDWriteTextFormat>,
        r: &D2D_RECT_F,
        c: COLORREF,
        align: Align,
        wrap: bool,
    ) {
        let Some(format) = format else {
            return;
        };
        if s.is_empty() {
            return;
        }
        unsafe {
            let _ = format.SetTextAlignment(match align {
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            });
            let _ = format.SetParagraphAlignment(if wrap {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            });
            let _ = format.SetWordWrapping(if wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });
        }
        // one "â€¦" per format, made on its first use
        let key = format.as_raw() as usize;
        if !self.ellipses.contains_key(&key)
            && let Some(sign) = dwrite_factory().and_then(|f| unsafe { f.CreateEllipsisTrimmingSign(format) }.ok())
        {
            self.ellipses.insert(key, (format.clone(), sign));
        }
        let trim = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        let _ = unsafe { format.SetTrimming(&trim, self.ellipses.get(&key).map(|e| &e.1)) };
        let wide: Vec<u16> = s.encode_utf16().collect();
        if let Some(b) = self.brush(c, 1.0) {
            unsafe {
                self.rt().DrawText(
                    &wide,
                    format,
                    r,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                )
            };
        }
    }

    /// Drops the bitmaps made from `source`, for a picture replaced every frame that would otherwise pile up.
    pub fn forget(&mut self, source: &IWICBitmapSource) {
        let key = source.as_raw() as usize;
        for t in self.targets.values_mut() {
            t.bitmaps.remove(&key);
        }
    }

    /// Stretched to r, linear filtering.
    pub fn image(&mut self, source: &IWICBitmapSource, r: &D2D_RECT_F) {
        let key = source.as_raw() as usize;
        let t = self.target();
        if !t.bitmaps.contains_key(&key) {
            if t.bitmaps.len() > 64 {
                t.bitmaps.clear(); // pictures change (a new fish each catch): drop the old ones now and then
            }
            let rt = t.rt.as_ref().expect("made in begin");
            let Ok(bitmap) = (unsafe { rt.CreateBitmapFromWicBitmap(source, None) }) else {
                return;
            };
            t.bitmaps.insert(key, (source.clone(), bitmap));
        }
        let bitmap = t.bitmaps[&key].1.clone();
        unsafe {
            self.rt()
                .DrawBitmap(&bitmap, Some(r), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_keeps_shading_and_stays_premultiplied() {
        // BGRA: an opaque orange, a half-transparent white (premultiplied: 128s), a clear pixel
        let mut px = [0x20, 0x80, 0xff, 0xff, 0x80, 0x80, 0x80, 0x80, 0, 0, 0, 0];
        tint_pixels(&mut px, 0x0c0fd4);
        assert_eq!(px[..4], [0xd4, 0x0f, 0x0c, 0xff]); // the orange's brightest channel was full: the tint itself
        assert_eq!(px[4..8], [0x6a, 0x07, 0x06, 0x80]); // half as bright, never over its alpha
        assert_eq!(px[8..], [0, 0, 0, 0]);
    }
    use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_WEIGHT_SEMI_BOLD};
    use windows::Win32::Graphics::Gdi::FW_SEMIBOLD;

    fn family_of(f: &IDWriteTextFormat) -> String {
        let mut name = [0u16; 64];
        let _ = unsafe { f.GetFontFamilyName(&mut name) };
        crate::util::text::from_wide(&name)
    }

    fn face(name: &str) -> [u16; 32] {
        let mut out = [0u16; 32];
        for (o, c) in out.iter_mut().zip(name.encode_utf16()) {
            *o = c;
        }
        out
    }

    #[test]
    fn d2d_text_format_from_gdi_font() {
        let lf = LOGFONTW {
            lfHeight: -12,
            lfWeight: FW_SEMIBOLD.0 as i32,
            lfItalic: 1,
            lfFaceName: face("Segoe UI"),
            ..Default::default()
        };
        let f = text_format(&lf).expect("a format");
        assert_eq!(family_of(&f), "Segoe UI");
        assert_eq!(unsafe { f.GetFontSize() }, 12.0); // em size in pixels: the canvases draw at 96 DPI
        assert_eq!(unsafe { f.GetFontWeight() }, DWRITE_FONT_WEIGHT_SEMI_BOLD);
        assert_eq!(unsafe { f.GetFontStyle() }, DWRITE_FONT_STYLE_ITALIC);
    }

    #[test]
    fn d2d_added_font_file_reaches_directwrite() {
        // A game's own font is added at runtime; DirectWrite must take that family from the added file.
        let file = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
            .join("Fonts")
            .join("consola.ttf");
        if !file.exists() {
            return; // no Consolas on this machine
        }
        assert!(add_font_file(&file));
        let lf = LOGFONTW {
            lfHeight: -14,
            lfFaceName: face("Consolas"),
            ..Default::default()
        };
        let f = text_format(&lf).expect("a format");
        let fonts = unsafe { f.GetFontCollection() }.ok();
        let mut system = None;
        unsafe { dwrite_factory().unwrap().GetSystemFontCollection(&mut system, false) }.unwrap();
        assert!(fonts.is_some() && fonts != system); // the added file's collection, not the system's
        assert!(!add_font_file(Path::new(r"C:\no\such\font.ttf")));
    }

    #[test]
    fn d2d_image_decodes_a_file_without_holding_it() {
        // a 2x1 32-bit BMP, written by hand
        let file = std::env::temp_dir().join(format!("handy-d2d-test-{}.bmp", std::process::id()));
        let mut bmp = Vec::new();
        bmp.extend_from_slice(&0x4D42u16.to_le_bytes());
        bmp.extend_from_slice(&(14u32 + 40 + 8).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&(14u32 + 40).to_le_bytes());
        for v in [40u32, 2, 1] {
            bmp.extend_from_slice(&v.to_le_bytes());
        }
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]); // BI_RGB and the rest
        for px in [0xFFFF_0000u32, 0xFF00_FF00] {
            bmp.extend_from_slice(&px.to_le_bytes());
        }
        std::fs::write(&file, &bmp).unwrap();
        let image = load_image(&file, None).expect("decoded");
        let (mut w, mut h) = (0, 0);
        unsafe { image.GetSize(&mut w, &mut h) }.unwrap();
        assert!(w == 2 && h == 1);
        assert!(std::fs::remove_file(&file).is_ok()); // nothing left open on it
        assert!(load_image(&file, None).is_none());
    }
}
