//! A game's main window, built from a row table: its own title bar, a status header, page tabs, then cards.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::marker::PhantomPinned;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Once;

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_RANGE, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_TRIMMING_GRANULARITY_NONE,
    DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, CreateSolidBrush, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_NOCLIP,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FW_BOLD, FW_SEMIBOLD,
    FillRect, GetDC, GetTextExtentPoint32W, GetTextMetricsW, HBRUSH, HDC, HFONT, InvalidateRect, IsRectEmpty, LOGFONTW,
    PAINTSTRUCT, PtInRect, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC,
    ScreenToClient, SelectObject, SetBkColor, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::Graphics::Imaging::IWICBitmapSource;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_LEFT, SS_NOPREFIX, SS_OWNERDRAW};
use windows::Win32::System::WindowsProgramming::MulDiv;
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CDDS_PREPAINT, CDIS_FOCUS, CDIS_HOT, CDRF_DODEFAULT, CDRF_SKIPDEFAULT, CheckDlgButton,
    DRAWITEMSTRUCT, EM_SETLIMITTEXT, HTREEITEM, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
    IsDlgButtonChecked, LoadIconWithScaleDown, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, NMTVITEMCHANGE, SetWindowTheme,
    TOOLTIPS_CLASSW, TREE_VIEW_ITEM_STATE_FLAGS, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETMAXTIPWIDTH,
    TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW, TVE_EXPAND, TVI_FIRST, TVI_ROOT, TVIF_STATE,
    TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVIS_STATEIMAGEMASK, TVITEMW, TVM_DELETEITEM, TVM_EXPAND,
    TVM_INSERTITEMW, TVM_SETBKCOLOR, TVM_SETEXTENDEDSTYLE, TVM_SETITEMW, TVM_SETTEXTCOLOR, TVN_ITEMCHANGINGW,
    TVS_CHECKBOXES, TVS_EX_DOUBLEBUFFER, TVS_FULLROWSELECT, TVS_HASBUTTONS, TVS_LINESATROOT, TVS_SHOWSELALWAYS,
    WC_TREEVIEWW, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_CONTROL, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BN_CLICKED, BS_CHECKBOX, BS_PUSHBUTTON, CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DI_NORMAL,
    DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW, DrawIconEx, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS,
    ES_AUTOHSCROLL, ES_NUMBER, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GWL_EXSTYLE, GWL_STYLE,
    GWLP_USERDATA, GetClientRect, GetDlgItem, GetDlgItemTextW, GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowTextW, HICON, HMENU, HTCAPTION, HTCLIENT, ICON_BIG, ICON_SMALL, IDC_ARROW, IDI_APPLICATION, IsChild,
    IsDialogMessageW, IsIconic, IsWindowVisible, KillTimer, LoadCursorW, LoadIconW, MSG, MoveWindow, NCCALCSIZE_PARAMS,
    NONCLIENTMETRICSW, PostQuitMessage, RegisterClassExW, SC_CLOSE, SC_MINIMIZE, SHOW_WINDOW_CMD, SM_CXICON,
    SPI_GETNONCLIENTMETRICS, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageW, SetDlgItemInt,
    SetDlgItemTextW, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    UISF_HIDEFOCUS, WA_INACTIVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_DRAWITEM,
    WM_DWMCOLORIZATIONCOLORCHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_NOTIFY, WM_PAINT, WM_QUERYUISTATE,
    WM_SETFONT, WM_SETICON, WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CHILD,
    WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, IDC_HAND, SetCursor, WM_MOUSEWHEEL, WM_SETCURSOR, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

use super::Tone;
use super::d2d::{Align, Canvas, Vector2, dwrite_factory, icon_image, load_image, pixels_image, rect_f, text_format};
use super::theme::{Theme, blend, gradient_at, readable_on, readable_on_outlined, rgb, system_theme};
use crate::util::text::{from_wide, parse_bounded, wide};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Page,
    Section,
    Value,
    Check,
    Number,
    Text,
    Note,
    Tree,
    Choice,
    Tile,
}

/// One dashboard row. Page: starts a page; with two or more, tabs under the header switch them (Ctrl+Tab too).
/// Section: title over a card; the rows after it sit on the card. Value: muted label + text, `lines` tall; with
/// `picture` (DIP wide), the text leaves that much room at the right for an image (set_picture). Rich value: a Value
/// whose text is colored, bold or italic spans (set_spans), for text styled like the game's own.
/// Check: toggle switch the game owns (set_check); text after a '\t' is a muted hint by the switch, where each
/// `[...]` is drawn as a keycap (e.g. "[F1]", "hold [Mouse 4]"; "F1" stays plain text).
/// Number: whole number in lo..=hi, with - and + buttons (and the mouse wheel) stepping by 1.
/// Text: one-line text field the value column wide, reported as typed (on_text).
/// Choice: segmented control of `options` the game owns (set_choice, on_choice); Left/Right step it.
/// Note: full-width warning-colored text under the card (empty = blank).
/// Tree: card-wide scrolling tree `lines` items tall with a checkbox per item; the game owns the checks (set_tree,
/// on_tree_check). Items keep their expanded state across set_tree calls by key.
/// Tile: a stat in its own box; consecutive tiles on a card sit two per line across it (a lone last one takes one
/// column). The label is small and muted at the top; set_text / set_spans give the value: its first line big, the text
/// after the first '\n' a small muted line under it (colored spans keep their colors). Text too long is cut with "…".
/// A picture tile (`picture`, DIP tall) puts an image (set_picture) under the label, fitted and centered across the
/// tile, then the text in the body face, one line per '\n', `lines` of them, each cut with "…". Tiles on one line of
/// the grid take the taller one's height so they sit level.
/// Labels wider than Spec.label_width widen the label column (up to LABEL_MAX_DIP) instead of being cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub id: i32,
    pub text: String,
    pub lines: i32,
    pub lo: i32,
    pub hi: i32,
    pub picture: i32,
    pub rich: bool,
    pub options: Vec<String>,
}

impl Row {
    fn new(kind: Kind, id: i32, text: &str) -> Self {
        Self {
            kind,
            id,
            text: text.to_owned(),
            lines: 1,
            lo: 0,
            hi: 0,
            picture: 0,
            rich: false,
            options: Vec::new(),
        }
    }
    /// How many text lines tall (Value, Tree, picture Tile).
    pub fn lines(mut self, n: i32) -> Self {
        self.lines = n;
        self
    }
    /// A Value's picture slot, DIP wide; a Tile's picture area, DIP tall.
    pub fn picture(mut self, dip: i32) -> Self {
        self.picture = dip;
        self
    }
}

pub fn page(title: &str) -> Row {
    Row::new(Kind::Page, 0, title)
}
pub fn section(title: &str) -> Row {
    Row::new(Kind::Section, 0, title)
}
pub fn value(id: i32, label: &str) -> Row {
    Row::new(Kind::Value, id, label)
}
pub fn rich_value(id: i32, label: &str) -> Row {
    Row {
        rich: true,
        ..value(id, label)
    }
}
pub fn check(id: i32, text: &str) -> Row {
    Row::new(Kind::Check, id, text)
}
pub fn number(id: i32, label: &str, lo: i32, hi: i32) -> Row {
    Row {
        lo,
        hi,
        ..Row::new(Kind::Number, id, label)
    }
}
pub fn text(id: i32, label: &str) -> Row {
    Row::new(Kind::Text, id, label)
}
pub fn note(id: i32) -> Row {
    Row::new(Kind::Note, id, "")
}
pub fn tree(id: i32, lines: i32) -> Row {
    Row::new(Kind::Tree, id, "").lines(lines)
}
pub fn choice(id: i32, label: &str, options: &[&str]) -> Row {
    Row {
        options: options.iter().map(|&o| o.to_owned()).collect(),
        ..Row::new(Kind::Choice, id, label)
    }
}
pub fn tile(id: i32, label: &str) -> Row {
    Row {
        rich: true, // set_text becomes one plain span, so both setters draw the same way
        ..Row::new(Kind::Tile, id, label)
    }
}

/// A tile's spans split at the first '\n' into its big line and its small one. Later '\n's become spaces: the small
/// line is one line too.
fn tile_lines(spans: &[Span]) -> (Vec<Span>, Vec<Span>) {
    let (mut big, mut small) = (Vec::new(), Vec::new());
    let mut split = false;
    let with = |s: &Span, text: &str| Span {
        text: text.replace('\n', " "),
        ..s.clone()
    };
    for s in spans {
        if split {
            small.push(with(s, &s.text));
        } else if let Some((a, b)) = s.text.split_once('\n') {
            split = true;
            big.push(with(s, a));
            small.push(with(s, b));
        } else {
            big.push(s.clone());
        }
    }
    (big, small)
}

/// Spans split at every '\n' into lines, each span keeping its style on both sides of a split.
fn span_lines(spans: &[Span]) -> Vec<Vec<Span>> {
    let mut lines = vec![Vec::new()];
    for s in spans {
        for (k, part) in s.text.split('\n').enumerate() {
            if k > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty() {
                let line = lines.last_mut().expect("starts with one line");
                line.push(Span {
                    text: part.to_owned(),
                    ..s.clone()
                });
            }
        }
    }
    lines
}

/// Tile `k` of a run across a card `width` wide: its column's left offset and width (two columns `gap` apart).
fn tile_column(k: usize, width: i32, gap: i32) -> (i32, i32) {
    let w = (width - gap) / 2;
    if k.is_multiple_of(2) {
        (0, w)
    } else {
        (w + gap, width - w - gap)
    }
}

/// A Check hint split into muted text and `[key]` keycaps, in order and trimmed: "hold [Mouse 4]" gives
/// ("hold", false), ("Mouse 4", true). A '[' with no ']' after it stays text.
fn hint_parts(hint: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut rest = hint;
    while let Some((before, after)) = rest.split_once('[')
        && let Some((key, tail)) = after.split_once(']')
    {
        out.push((before.trim(), false));
        out.push((key.trim(), true));
        rest = tail;
    }
    out.push((rest.trim(), false));
    out.retain(|p| !p.0.is_empty());
    out
}

/// The segment of `n` equal ones across `width` pixels that x falls in (clamped to the ends).
fn segment_at(width: i32, n: usize, x: i32) -> usize {
    (x.max(0) as usize * n / width.max(1) as usize).min(n.saturating_sub(1))
}

/// A run of rich value text. No colors: the theme's text color; several: a left-to-right gradient across the span.
/// '\n' starts a new line. A color too close to the card is lightened or darkened until it reads (readable_on).
/// stroke_opacity > 0: outlined in `stroke` at that opacity over the card, like a Roblox UIStroke.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub colors: Vec<COLORREF>,
    pub bold: bool,
    pub italic: bool,
    pub stroke: COLORREF,
    pub stroke_opacity: f32,
}

impl Span {
    /// Plain text in the theme's color.
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Default::default()
        }
    }
}

/// One tree item, given in pre-order (every parent before its children).
#[derive(Clone, Debug, PartialEq)]
pub struct TreeItem {
    /// Unique among the items; an item with the same key in the next set_tree is the same item.
    pub key: String,
    /// Index of the parent item in the same vector; None = top level.
    pub parent: Option<usize>,
    pub text: String,
    pub checked: bool,
    /// false: no box (e.g. a root that can't be picked).
    pub checkbox: bool,
    /// Expanded when it first appears or turns true; otherwise the user's expanding stays.
    pub open: bool,
}

impl Default for TreeItem {
    fn default() -> Self {
        Self {
            key: String::new(),
            parent: None,
            text: String::new(),
            checked: false,
            checkbox: true,
            open: false,
        }
    }
}

/// The exe's icon resource the dashboard shows in its title bar, Alt+Tab and the taskbar: `1 ICON "app.ico"` in
/// res/app.rc (the first icon is also what Explorer shows). Without one, the stock application icon.
pub(crate) const APP_ICON_ID: u16 = 1;

pub struct Spec {
    pub title: String,
    /// Drawn as a small pill right after the title (e.g. the app's version); empty: none. `Dashboard::set_badge`
    /// changes it.
    pub badge: String,
    pub window_class: String,
    pub rows: Vec<Row>,
    /// Note id for Number hints.
    pub hint_row: Option<i32>,
    /// DIP
    pub label_width: i32,
    pub value_width: i32,
    pub tick_ms: u32,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            title: String::new(),
            badge: String::new(),
            window_class: String::new(),
            rows: Vec::new(),
            hint_row: None,
            label_width: 110,
            value_width: 350,
            tick_ms: 50,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Text,
    Muted,
    Warn,
}
#[derive(Clone, Copy)]
struct Look {
    // how a child static paints
    role: Role,
    on_card: bool,
}

#[derive(Default)]
struct TreeState {
    items: Vec<TreeItem>,               // last set
    handle: HashMap<String, HTREEITEM>, // by key
    index: HashMap<isize, usize>,       // HTREEITEM -> into items
}

#[derive(Default)]
struct Fonts {
    faces: [HFONT; 4], // body text: regular, bold, italic, bold italic (index: bold | italic << 1)
    semibold: HFONT,
    big: HFONT,
    text: [Option<IDWriteTextFormat>; 4], // the same faces for DirectWrite
    semibold_text: Option<IDWriteTextFormat>,
    big_text: Option<IDWriteTextFormat>,
}

type Slot<F> = RefCell<Option<Rc<F>>>;

// The title bar is the top CAPTION_DIP of the client area (WM_NCCALCSIZE drops the system one); dragging it moves
// the window (WM_NCHITTEST says HTCAPTION), and its two buttons are drawn and clicked here.
const CAPTION_DIP: i32 = 32;
const CAPTION_BUTTON_DIP: i32 = 46;
const MINIMIZE: i32 = 0;
const CLOSE: i32 = 1;
const BADGE: i32 = 2; // the badge is clicked like a caption button
const BADGE_PAD_DIP: i32 = 7;
const BADGE_DOT_DIP: i32 = 6;
const STATUS: i32 = 3; // the status header's tooltip tool
const LABEL_MAX_DIP: i32 = 220; // past this a label is cut again, so one long label can't make the window huge
const STEP_DIP: i32 = 28; // a Number row's - and + buttons

/// A game's main window, built from a row table: its own title bar (icon, title, minimize and close), a status
/// header, page tabs, then cards. Follows the system light/dark mode and accent color live, sizes itself to the
/// shown page at the window's DPI (re-layout on WM_DPICHANGED), and Tab moves focus (IsDialogMessage). All methods
/// and callbacks run on the thread that called create()/run(). Callbacks get the dashboard to call its setters, and
/// may borrow anything that outlives it ('a).
pub struct Dashboard<'a> {
    spec: Spec,
    on_tick: Slot<dyn Fn(&Dashboard<'a>) + 'a>,
    on_check: Slot<dyn Fn(&Dashboard<'a>, i32, bool) + 'a>,
    on_number: Slot<dyn Fn(&Dashboard<'a>, i32, i32) + 'a>,
    on_text: Slot<dyn Fn(&Dashboard<'a>, i32, &str) + 'a>,
    on_tree_check: Slot<dyn Fn(&Dashboard<'a>, i32, usize, bool) + 'a>,
    on_choice: Slot<dyn Fn(&Dashboard<'a>, i32, usize) + 'a>,
    on_badge_click: Slot<dyn Fn(&Dashboard<'a>) + 'a>,
    on_close: Slot<dyn Fn(&Dashboard<'a>) -> bool + 'a>,
    on_end_session: Slot<dyn Fn() + 'a>,

    hwnd: Cell<HWND>,
    header: Cell<HWND>,
    inst: Cell<HINSTANCE>,
    icon: Cell<HICON>,     // title bar (and ICON_SMALL)
    big_icon: Cell<HICON>, // Alt+Tab and taskbar (ICON_BIG)
    active: Cell<bool>,    // the title dims while another window is in front
    cap_hot: Cell<i32>,    // caption button under the mouse / pressed; -1 = none
    cap_down: Cell<i32>,
    badge: RefCell<(String, Tone, String)>, // text, dot, hover tip
    tooltip: Cell<HWND>,
    tip_rect: Cell<RECT>, // the badge rect the tooltip's tool has
    // Drawing: Direct2D and DirectWrite on the canvas; the Win32 controls (labels, fields, trees) draw themselves with
    // the HFONTs. GDI is the fallback when Direct2D can't start.
    canvas: RefCell<Canvas>,
    icon_image: RefCell<Option<IWICBitmapSource>>, // icon, for the canvas
    fonts: RefCell<Fonts>,
    line_h: Cell<i32>, // GDI line metrics, so drawn text lines up with the controls'
    ascent: Cell<i32>,
    big_line_h: Cell<i32>,
    family: RefCell<String>,                 // empty: the system's
    spans: RefCell<HashMap<i32, Vec<Span>>>, // last spans per rich value id
    brushes: Cell<[HBRUSH; 3]>,              // bg, card, field
    theme: Cell<Theme>,
    dpi: Cell<u32>,
    owns_loop: Cell<bool>,
    tone: Cell<Tone>,
    status_title: RefCell<String>,
    status_detail: RefCell<String>,
    status_tip: RefCell<String>,
    header_rect: Cell<RECT>,         // client rect, for the status tooltip
    width: Cell<i32>,                // client width
    page_bottoms: RefCell<Vec<i32>>, // per page: where its last row ends
    page: Cell<i32>,
    pages: Cell<i32>,
    page_of: RefCell<Vec<i32>>,       // per row
    ctls: RefCell<Vec<(HWND, HWND)>>, // per row: label (may be null), control
    boxes: RefCell<Vec<RECT>>,        // per row: Number/Text field box (empty for others)
    slots: RefCell<Vec<RECT>>,        // per row: Value picture slot (empty for others)
    pictures: RefCell<HashMap<i32, (PathBuf, Option<u32>, Option<IWICBitmapSource>)>>, // file, tint, image per id
    cards: RefCell<Vec<(i32, RECT)>>, // page, rect
    looks: RefCell<HashMap<isize, Look>>, // by HWND
    shown: RefCell<HashMap<i32, String>>, // last text per control id
    numbers: RefCell<HashMap<i32, i32>>, // last value set/applied per Number id
    choices: RefCell<HashMap<i32, usize>>, // last set_choice per Choice id
    steppers: RefCell<Vec<(HWND, usize, i32)>>, // Number - and + buttons: button, row, step
    setting_tree: Cell<bool>,         // lets set_tree's own check changes through TVN_ITEMCHANGING
    trees: RefCell<HashMap<i32, TreeState>>,
    _pin: PhantomPinned, // the window keeps a pointer to it
}

/// The dot colour for `tone` (status header and badge).
fn tone_color(theme: &Theme, tone: Tone) -> COLORREF {
    match tone {
        Tone::Good => theme.good,
        Tone::Busy => theme.busy,
        Tone::Bad => theme.bad,
        Tone::Neutral => theme.text2,
    }
}

fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}
fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}
fn point_of(lp: LPARAM) -> POINT {
    POINT {
        x: (lp.0 & 0xFFFF) as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    }
}
fn is_empty(r: &RECT) -> bool {
    r.right == r.left
}
/// An image's drawn size fitted into `s` (keeping its aspect, never enlarged); None for no size.
fn fitted(img: &IWICBitmapSource, s: &RECT) -> Option<(f32, f32)> {
    let (mut iw, mut ih) = (0u32, 0u32);
    if unsafe { img.GetSize(&mut iw, &mut ih) }.is_err() || iw == 0 || ih == 0 {
        return None;
    }
    let (sw, sh) = ((s.right - s.left) as f32, (s.bottom - s.top) as f32);
    let k = (sw / iw as f32).min(sh / ih as f32).min(1.0);
    Some((iw as f32 * k, ih as f32 * k))
}
// Tree checkboxes: state image 1 = off, 2 = on.
fn checked_state(state: u32) -> bool {
    ((state & TVIS_STATEIMAGEMASK.0) >> 12) == 2
}
fn register_common_controls() {
    static DONE: Once = Once::new();
    DONE.call_once(|| {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TREEVIEW_CLASSES,
        };
        let _ = unsafe { InitCommonControlsEx(&icc) };
    });
}
fn window_text(h: HWND) -> String {
    let n = unsafe { GetWindowTextLengthW(h) }.max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..got])
}
fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> LRESULT {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))) }
}
fn set_item_state(tv: HWND, h: HTREEITEM, state: u32, mask: u32) {
    let t = TVITEMW {
        mask: TVIF_STATE,
        hItem: h,
        state: TREE_VIEW_ITEM_STATE_FLAGS(state),
        stateMask: TREE_VIEW_ITEM_STATE_FLAGS(mask),
        ..Default::default()
    };
    send(tv, TVM_SETITEMW, 0, &raw const t as isize);
}

impl<'a> Dashboard<'a> {
    pub fn new(spec: Spec) -> Pin<Box<Self>> {
        let badge = RefCell::new((spec.badge.clone(), Tone::Neutral, String::new()));
        Box::pin(Self {
            spec,
            on_tick: RefCell::new(None),
            on_check: RefCell::new(None),
            on_number: RefCell::new(None),
            on_text: RefCell::new(None),
            on_tree_check: RefCell::new(None),
            on_choice: RefCell::new(None),
            on_badge_click: RefCell::new(None),
            on_close: RefCell::new(None),
            on_end_session: RefCell::new(None),
            hwnd: Cell::new(HWND::default()),
            header: Cell::new(HWND::default()),
            inst: Cell::new(HINSTANCE::default()),
            icon: Cell::new(HICON::default()),
            big_icon: Cell::new(HICON::default()),
            active: Cell::new(true),
            cap_hot: Cell::new(-1),
            cap_down: Cell::new(-1),
            badge,
            tooltip: Cell::new(HWND::default()),
            tip_rect: Cell::new(RECT::default()),
            canvas: RefCell::new(Canvas::default()),
            icon_image: RefCell::new(None),
            fonts: RefCell::new(Fonts::default()),
            line_h: Cell::new(0),
            ascent: Cell::new(0),
            big_line_h: Cell::new(0),
            family: RefCell::new(String::new()),
            spans: RefCell::new(HashMap::new()),
            brushes: Cell::new([HBRUSH::default(); 3]),
            theme: Cell::new(Theme::default()),
            dpi: Cell::new(96),
            owns_loop: Cell::new(false),
            tone: Cell::new(Tone::Neutral),
            status_title: RefCell::new(String::new()),
            status_detail: RefCell::new(String::new()),
            status_tip: RefCell::new(String::new()),
            header_rect: Cell::new(RECT::default()),
            width: Cell::new(0),
            page_bottoms: RefCell::new(Vec::new()),
            page: Cell::new(0),
            pages: Cell::new(1),
            page_of: RefCell::new(Vec::new()),
            ctls: RefCell::new(Vec::new()),
            boxes: RefCell::new(Vec::new()),
            slots: RefCell::new(Vec::new()),
            pictures: RefCell::new(HashMap::new()),
            cards: RefCell::new(Vec::new()),
            looks: RefCell::new(HashMap::new()),
            shown: RefCell::new(HashMap::new()),
            numbers: RefCell::new(HashMap::new()),
            choices: RefCell::new(HashMap::new()),
            steppers: RefCell::new(Vec::new()),
            setting_tree: Cell::new(false),
            trees: RefCell::new(HashMap::new()),
            _pin: PhantomPinned,
        })
    }

    /// Every tick_ms, once right after create, and after on_check.
    pub fn on_tick(&self, f: impl Fn(&Dashboard<'a>) + 'a) {
        *self.on_tick.borrow_mut() = Some(Rc::new(f));
    }
    /// Check clicked: `checked` is the opposite of the box's state. The box does not change; the game calls set_check.
    pub fn on_check(&self, f: impl Fn(&Dashboard<'a>, i32, bool) + 'a) {
        *self.on_check.borrow_mut() = Some(Rc::new(f));
    }
    /// Only a valid value that differs from the last one set (set_number) or applied. Invalid text shows
    /// "Enter a whole number from lo to hi (not saved)." in hint_row; leaving the field restores the last value.
    pub fn on_number(&self, f: impl Fn(&Dashboard<'a>, i32, i32) + 'a) {
        *self.on_number.borrow_mut() = Some(Rc::new(f));
    }
    /// Every change to a Text field.
    pub fn on_text(&self, f: impl Fn(&Dashboard<'a>, i32, &str) + 'a) {
        *self.on_text.borrow_mut() = Some(Rc::new(f));
    }
    /// Tree item's box clicked or Space pressed on it: `checked` is the state asked for, index is into the last
    /// set_tree. The box does not change; the game calls set_tree.
    pub fn on_tree_check(&self, f: impl Fn(&Dashboard<'a>, i32, usize, bool) + 'a) {
        *self.on_tree_check.borrow_mut() = Some(Rc::new(f));
    }
    /// A Choice segment clicked or Left/Right pressed on it: `index` is the option asked for. The control does not
    /// change; the game calls set_choice.
    pub fn on_choice(&self, f: impl Fn(&Dashboard<'a>, i32, usize) + 'a) {
        *self.on_choice.borrow_mut() = Some(Rc::new(f));
    }
    /// false = stay open (the game calls close() later); unset = close.
    pub fn on_close(&self, f: impl Fn(&Dashboard<'a>) -> bool + 'a) {
        *self.on_close.borrow_mut() = Some(Rc::new(f));
    }
    /// The badge clicked (pressed and released on it); the badge shows a hand cursor once this is set.
    pub fn on_badge_click(&self, f: impl Fn(&Dashboard<'a>) + 'a) {
        *self.on_badge_click.borrow_mut() = Some(Rc::new(f));
    }
    /// Logoff/shutdown: may end the process without WM_DESTROY.
    pub fn on_end_session(&self, f: impl Fn() + 'a) {
        *self.on_end_session.borrow_mut() = Some(Rc::new(f));
    }

    /// Runs on_tick now (e.g. to filter at once on a key in a Text field, not on the next tick).
    pub fn tick(&self) {
        let f = self.on_tick.borrow().clone();
        if let Some(f) = f {
            f(self);
        }
    }

    fn px(&self, dip: i32) -> i32 {
        unsafe { MulDiv(dip, self.dpi.get() as i32, 96) }
    }

    /// Create + show + message loop; returns the WM_QUIT code (1 = no window).
    pub fn run(&self, show: SHOW_WINDOW_CMD) -> i32 {
        self.owns_loop.set(true);
        if !self.create() {
            return 1;
        }
        let _ = unsafe { ShowWindow(self.hwnd.get(), show) };
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            self.dispatch(&msg);
        }
        msg.wParam.0 as i32
    }

    /// Hidden window + tick timer, no loop (tests); WM_DESTROY posts no quit.
    pub fn create(&self) -> bool {
        let inst: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap_or_default().into();
        self.inst.set(inst);
        let class = HSTRING::from(self.spec.window_class.as_str());
        let mut icon = unsafe { LoadIconW(Some(inst), PCWSTR(APP_ICON_ID as usize as *const u16)) };
        if icon.is_err() {
            icon = unsafe { LoadIconW(None, IDI_APPLICATION) };
        }
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: inst,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            hIcon: icon.unwrap_or_default(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
            return false;
        }
        let made = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                &class,
                &HSTRING::from(self.spec.title.as_str()),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                400,
                None,
                None,
                Some(inst),
                Some(std::ptr::from_ref(self).cast()),
            )
        };
        if made.is_err() {
            return false;
        }
        self.tick(); // the first paint already shows data
        !self.hwnd.get().is_invalid()
    }

    /// One message-loop step (run() uses it); Enter/Esc are dropped.
    pub(crate) fn dispatch(&self, msg: &MSG) {
        // IsDialogMessage turns Enter/Esc into IDOK/IDCANCEL carrying control 1/2's hwnd, which would fire a game row
        // with id 1 or 2 (99n's macro checkbox is id 2). No row uses them, so they are dropped before it.
        let key = msg.wParam.0 as u16;
        if matches!(msg.message, WM_KEYDOWN | WM_KEYUP | WM_CHAR) && (key == VK_RETURN.0 || key == VK_ESCAPE.0) {
            return;
        }
        let h = self.hwnd.get();
        let pages = self.pages.get();
        if msg.message == WM_KEYDOWN
            && key == VK_TAB.0
            && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0
            && pages > 1
            && !h.is_invalid()
            && (msg.hwnd == h || unsafe { IsChild(h, msg.hwnd) }.as_bool())
        {
            let step = if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
                pages - 1
            } else {
                1
            };
            self.show_page((self.page.get() + step) % pages);
            return;
        }
        if msg.message == WM_KEYDOWN && (key == VK_LEFT.0 || key == VK_RIGHT.0) {
            // before IsDialogMessage, which would move focus to the next control instead
            let row = self.ctls.borrow().iter().position(|c| c.1 == msg.hwnd);
            if let Some(i) = row.filter(|&i| self.spec.rows[i].kind == Kind::Choice) {
                let n = self.spec.rows[i].options.len();
                let cur = self.choices.borrow().get(&self.spec.rows[i].id).copied().unwrap_or(0);
                let k = if key == VK_LEFT.0 {
                    cur.saturating_sub(1)
                } else {
                    (cur + 1).min(n.saturating_sub(1))
                };
                self.pick(i, k);
                return;
            }
        }
        if !h.is_invalid() && unsafe { IsDialogMessageW(h, msg) }.as_bool() {
            return; // Tab / Shift+Tab focus order
        }
        unsafe {
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }

    /// Sends only text that changed (no flicker). On a rich value: one plain span.
    pub fn set_text(&self, id: i32, text: &str) {
        if self.spec.rows.iter().any(|r| r.id == id && r.rich) {
            return self.set_spans(id, &[Span::plain(text)]);
        }
        {
            let mut shown = self.shown.borrow_mut();
            let last = shown.entry(id).or_default();
            if last == text {
                return;
            }
            text.clone_into(last);
        }
        let _ = unsafe { SetDlgItemTextW(self.hwnd.get(), id, &HSTRING::from(text)) };
    }

    pub fn set_check(&self, id: i32, checked: bool) {
        let h = self.hwnd.get();
        if (unsafe { IsDlgButtonChecked(h, id) } == BST_CHECKED.0) != checked {
            let _ = unsafe { CheckDlgButton(h, id, if checked { BST_CHECKED } else { BST_UNCHECKED }) };
        }
    }

    /// Shows option `index` of a Choice selected (out of range: none). Does not call on_choice.
    pub fn set_choice(&self, id: i32, index: usize) {
        if self.choices.borrow_mut().insert(id, index) == Some(index) {
            return;
        }
        if let Ok(c) = unsafe { GetDlgItem(Some(self.hwnd.get()), id) } {
            let _ = unsafe { InvalidateRect(Some(c), None, false) };
        }
    }

    /// Does not call on_number.
    pub fn set_number(&self, id: i32, value: i32) {
        self.numbers.borrow_mut().insert(id, value); // first, so the EN_CHANGE this sends sees no change
        let _ = unsafe { SetDlgItemInt(self.hwnd.get(), id, value as u32, true) };
    }

    /// A Value row's or picture tile's picture: an image file (PNG, JPEG, BMP...) fitted into its slot, recolored to
    /// `tint` (0xRRGGBB) when given; empty clears it. Reads the file only when the path or tint changes, and keeps no
    /// lock on it. False when nothing is shown (empty, missing, unreadable).
    pub fn set_picture(&self, id: i32, file: &Path, tint: Option<u32>) -> bool {
        {
            let mut pictures = self.pictures.borrow_mut();
            let (shown, shown_tint, image) = pictures.entry(id).or_default();
            if shown == file && *shown_tint == tint {
                return image.is_some();
            }
        }
        let image = load_image(file, tint); // decoded into memory: the file isn't kept open
        self.show_picture(id, file.to_path_buf(), tint, image)
    }

    /// A Value row's picture slot (a picture tile's picture area) size in physical pixels (for rendering into it), None
    /// for no slot / not laid out yet.
    pub fn picture_size(&self, id: i32) -> Option<(u32, u32)> {
        let i = self
            .spec
            .rows
            .iter()
            .position(|r| r.id == id && matches!(r.kind, Kind::Value | Kind::Tile))?;
        let s = *self.slots.borrow().get(i)?;
        (!is_empty(&s)).then(|| ((s.right - s.left) as u32, (s.bottom - s.top) as u32))
    }

    /// Whether a Value row's or picture tile's picture can be seen now: the window shown, not minimized, and the row on
    /// the current page. Cheap enough to call every tick before rendering into it.
    pub fn picture_visible(&self, id: i32) -> bool {
        let h = self.hwnd.get();
        let on_page = self
            .spec
            .rows
            .iter()
            .position(|r| r.id == id && matches!(r.kind, Kind::Value | Kind::Tile))
            .is_some_and(|i| self.page_of.borrow().get(i) == Some(&self.page.get()));
        on_page && unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() }
    }

    /// Shows premultiplied BGRA pixels (w x h, as raster::render makes them) as that row's picture, replacing any image;
    /// repaints the slot. A later set_picture always replaces it again (even with the same file/tint as before the pixels).
    pub fn set_picture_pixels(&self, id: i32, w: u32, h: u32, bgra: &[u8]) {
        // "<pixels>" can't be a Windows file name, so no set_picture path is ever taken for "unchanged"
        self.show_picture(id, PathBuf::from("<pixels>"), None, pixels_image(w, h, bgra));
    }

    fn show_picture(&self, id: i32, file: PathBuf, tint: Option<u32>, image: Option<IWICBitmapSource>) -> bool {
        let shown = image.is_some();
        let old = self.pictures.borrow_mut().insert(id, (file, tint, image));
        if let (Some((_, _, Some(old))), Ok(mut canvas)) = (old, self.canvas.try_borrow_mut()) {
            canvas.forget(&old); // pixel pictures change every tick: don't let their bitmaps pile up
        }
        let slots = self.slots.borrow().clone();
        for (r, s) in self.spec.rows.iter().zip(&slots) {
            if r.id == id && r.kind == Kind::Value {
                // no erase: WM_ERASEBKGND does nothing and the paint fills the slot's background
                let _ = unsafe { InvalidateRect(Some(self.hwnd.get()), Some(s), false) };
            } else if r.id == id
                && r.kind == Kind::Tile
                && let Ok(c) = unsafe { GetDlgItem(Some(self.hwnd.get()), id) }
            {
                let _ = unsafe { InvalidateRect(Some(c), None, false) }; // a tile draws its own picture
            }
        }
        shown
    }

    /// A rich value's text; repaints only on change. set_text on a rich value sets one plain span.
    pub fn set_spans(&self, id: i32, spans: &[Span]) {
        {
            let mut all = self.spans.borrow_mut();
            let shown = all.entry(id).or_default();
            if shown == spans {
                return;
            }
            *shown = spans.to_vec();
        }
        let mut plain: String = spans.iter().map(|s| s.text.as_str()).collect();
        if let Some(r) = self.spec.rows.iter().find(|r| r.id == id && r.kind == Kind::Tile) {
            plain = format!("{}: {plain}", r.text); // a tile has no label control of its own to read
        }
        let h = self.hwnd.get();
        // what a screen reader reads; the spans are drawn (draw_spans, draw_tile)
        let _ = unsafe { SetDlgItemTextW(h, id, &HSTRING::from(plain)) };
        if let Ok(c) = unsafe { GetDlgItem(Some(h), id) } {
            let _ = unsafe { InvalidateRect(Some(c), None, false) };
        }
    }

    /// Font family for all the window's text, e.g. a game's own font loaded with add_font_file (DirectWrite can't see
    /// one loaded with AddFontResourceExW alone); empty = the system's (Segoe UI). Re-lays out the window, since line
    /// heights follow the font.
    pub fn set_font_family(&self, family: &str) {
        if *self.family.borrow() == family {
            return;
        }
        family.clone_into(&mut self.family.borrow_mut());
        if self.header.get().is_invalid() {
            return; // not built yet: create() makes the fonts
        }
        self.apply_fonts();
        self.layout(None); // line heights follow the font
        let _ = unsafe {
            RedrawWindow(
                Some(self.hwnd.get()),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            )
        };
    }

    /// Replaces a Tree's items, matched by key: new keys are inserted in place, gone keys removed with their subtrees,
    /// and only text and checks that changed are touched (scroll, selection and expanded items stay put).
    pub fn set_tree(&self, id: i32, items: &[TreeItem]) {
        let Ok(tv) = (unsafe { GetDlgItem(Some(self.hwnd.get()), id) }) else {
            return;
        };
        // taken out while the tree view runs (its notifications reach this window)
        let mut st = self.trees.borrow_mut().remove(&id).unwrap_or_default();
        let parent_key = |v: &[TreeItem], it: &TreeItem| it.parent.map(|p| v[p].key.clone()).unwrap_or_default();
        let wanted: HashMap<&str, String> = items
            .iter()
            .map(|it| (it.key.as_str(), parent_key(items, it)))
            .collect();
        let image = |it: &TreeItem| (if it.checkbox { if it.checked { 2 } else { 1 } } else { 0 }) << 12;
        self.setting_tree.set(true);
        // Gone (or moved) items go first, children before parents, so no handle outlives the parent that took it down.
        for old in st.items.iter().rev() {
            if wanted
                .get(old.key.as_str())
                .is_some_and(|w| *w == parent_key(&st.items, old))
            {
                continue;
            }
            if let Some(h) = st.handle.remove(&old.key) {
                send(tv, TVM_DELETEITEM, 0, h.0);
            }
        }
        let mut handles = Vec::with_capacity(items.len());
        let mut last_child: HashMap<Option<usize>, HTREEITEM> = HashMap::new(); // by parent: where the next new sibling goes
        let mut to_open = Vec::new();
        for it in items {
            let mut text = wide(&it.text);
            let h = if let Some(&h) = st.handle.get(&it.key) {
                let old = &st.items[st.index[&h.0]];
                if old.text != it.text {
                    let t = TVITEMW {
                        mask: TVIF_TEXT,
                        hItem: h,
                        pszText: windows::core::PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    send(tv, TVM_SETITEMW, 0, &raw const t as isize);
                }
                if image(old) != image(it) {
                    set_item_state(tv, h, image(it), TVIS_STATEIMAGEMASK.0);
                }
                if it.open && !old.open {
                    to_open.push(h);
                }
                h
            } else {
                let ins = TVINSERTSTRUCTW {
                    hParent: it.parent.map_or(TVI_ROOT, |p| handles[p]),
                    hInsertAfter: last_child.get(&it.parent).copied().unwrap_or(TVI_FIRST),
                    Anonymous: TVINSERTSTRUCTW_0 {
                        item: TVITEMW {
                            mask: TVIF_TEXT,
                            pszText: windows::core::PWSTR(text.as_mut_ptr()),
                            ..Default::default()
                        },
                    },
                };
                let h = HTREEITEM(send(tv, TVM_INSERTITEMW, 0, &raw const ins as isize).0);
                set_item_state(tv, h, image(it), TVIS_STATEIMAGEMASK.0); // set at insert, TVS_CHECKBOXES resets it
                st.handle.insert(it.key.clone(), h);
                if it.open {
                    to_open.push(h);
                }
                h
            };
            handles.push(h);
            last_child.insert(it.parent, h);
        }
        for h in to_open {
            send(tv, TVM_EXPAND, TVE_EXPAND.0 as usize, h.0); // after its children exist
        }
        self.setting_tree.set(false);
        st.items = items.to_vec();
        st.index = handles.iter().enumerate().map(|(i, h)| (h.0, i)).collect();
        self.trees.borrow_mut().insert(id, st);
    }

    /// The title bar badge: `text` in the pill after a dot colored by `tone`, `tip` as its hover tooltip (empty text:
    /// no badge). Repaints only on change.
    pub fn set_badge(&self, text: &str, tone: Tone, tip: &str) {
        {
            let mut b = self.badge.borrow_mut();
            if b.0 == text && b.1 == tone && b.2 == tip {
                return;
            }
            *b = (text.into(), tone, tip.into());
        }
        self.invalidate_caption();
        self.sync_tip(true);
    }

    /// Points the tooltip's tool at the badge's rect (and, with `text`, sets its tip) when they changed.
    fn sync_tip(&self, text: bool) {
        let tip = self.tooltip.get();
        if tip.is_invalid() {
            return;
        }
        let rect = self.badge_pill().unwrap_or_default();
        let moved = rect != self.tip_rect.get();
        if !moved && !text {
            return;
        }
        self.tip_rect.set(rect);
        let mut s = wide(&self.badge.borrow().2);
        let ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd.get(),
            uId: BADGE as usize,
            rect,
            lpszText: PWSTR(s.as_mut_ptr()),
            ..Default::default()
        };
        if moved {
            send(tip, TTM_NEWTOOLRECTW, 0, &raw const ti as isize);
        }
        if text {
            send(tip, TTM_UPDATETIPTEXTW, 0, &raw const ti as isize);
        }
    }

    fn create_tooltip(&self) {
        let h = self.hwnd.get();
        let Ok(tip) = (unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(h),
                None,
                Some(self.inst.get()),
                None,
            )
        }) else {
            return;
        };
        // the badge's rect comes with the first paint (sync_tip), the header's from layout
        let tools = [
            (BADGE, self.badge.borrow().2.clone(), RECT::default()),
            (STATUS, self.status_tip.borrow().clone(), self.header_rect.get()),
        ];
        for (id, text, rect) in tools {
            let mut s = wide(&text);
            let ti = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_SUBCLASS,
                hwnd: h,
                uId: id as usize,
                rect,
                lpszText: PWSTR(s.as_mut_ptr()),
                ..Default::default()
            };
            send(tip, TTM_ADDTOOLW, 0, &raw const ti as isize);
        }
        send(tip, TTM_SETMAXTIPWIDTH, 0, self.px(480) as isize); // long tips wrap, and '\n' breaks a line
        self.tooltip.set(tip);
    }

    /// The status header's hover tooltip, for details too technical for the header itself; empty: none.
    pub fn set_status_tip(&self, tip: &str) {
        if *self.status_tip.borrow() == tip {
            return;
        }
        tip.clone_into(&mut self.status_tip.borrow_mut());
        self.sync_status_tip();
    }

    /// Points the status tool at the header's rect with the current tip.
    fn sync_status_tip(&self) {
        let tip = self.tooltip.get();
        if tip.is_invalid() {
            return;
        }
        let mut s = wide(&self.status_tip.borrow());
        let ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd.get(),
            uId: STATUS as usize,
            rect: self.header_rect.get(),
            lpszText: PWSTR(s.as_mut_ptr()),
            ..Default::default()
        };
        send(tip, TTM_NEWTOOLRECTW, 0, &raw const ti as isize);
        send(tip, TTM_UPDATETIPTEXTW, 0, &raw const ti as isize);
    }

    /// Header: colored dot + title, detail on up to two muted lines below (empty: no room kept for it). Repaints only
    /// on change. Technical details belong in set_status_tip.
    pub fn set_status(&self, tone: Tone, title: &str, detail: &str) {
        if tone == self.tone.get() && *self.status_title.borrow() == title && *self.status_detail.borrow() == detail {
            return;
        }
        let gap_changed = self.status_detail.borrow().is_empty() != detail.is_empty();
        self.tone.set(tone);
        title.clone_into(&mut self.status_title.borrow_mut());
        detail.clone_into(&mut self.status_detail.borrow_mut());
        let header = self.header.get();
        if header.is_invalid() {
            return;
        }
        if gap_changed {
            self.layout(None); // the header only keeps room for detail lines while there is detail
        }
        let spoken = if detail.is_empty() {
            title.to_owned()
        } else {
            format!("{title} - {detail}")
        };
        let _ = unsafe { SetWindowTextW(header, &HSTRING::from(spoken)) }; // screen readers
        let _ = unsafe { InvalidateRect(Some(header), None, false) };
    }

    /// 0-based, in Page row order.
    pub(crate) fn show_page(&self, p: i32) {
        if p < 0 || p >= self.pages.get() {
            return;
        }
        self.page.set(p);
        let mut tab = None;
        let ctls = self.ctls.borrow().clone();
        let page_of = self.page_of.borrow().clone();
        for (i, &(label, ctl)) in ctls.iter().enumerate() {
            unsafe {
                if self.spec.rows[i].kind == Kind::Page {
                    let _ = ShowWindow(ctl, if self.pages.get() > 1 { SW_SHOWNA } else { SW_HIDE });
                    let _ = InvalidateRect(Some(ctl), None, false); // selected look
                    if page_of[i] == p {
                        tab = Some(ctl);
                    }
                    continue;
                }
                let show = if page_of[i] == p { SW_SHOWNA } else { SW_HIDE };
                if !label.is_invalid() {
                    let _ = ShowWindow(label, show);
                }
                if !ctl.is_invalid() {
                    let _ = ShowWindow(ctl, show);
                }
            }
        }
        for &(b, row, _) in self.steppers.borrow().iter() {
            let _ = unsafe { ShowWindow(b, if page_of[row] == p { SW_SHOWNA } else { SW_HIDE }) };
        }
        // A focused control that just hid would leave the keyboard nowhere.
        let h = self.hwnd.get();
        let f = unsafe { GetFocus() };
        if let Some(tab) = tab
            && !f.is_invalid()
            && unsafe { IsChild(h, f) }.as_bool()
            && !unsafe { IsWindowVisible(f) }.as_bool()
        {
            let _ = unsafe { SetFocus(Some(tab)) };
        }
        self.fit(None);
        let _ = unsafe { InvalidateRect(Some(h), None, true) }; // cards
    }

    pub fn page(&self) -> i32 {
        self.page.get()
    }

    pub fn set_title(&self, title: &str) {
        let _ = unsafe { SetWindowTextW(self.hwnd.get(), &HSTRING::from(title)) };
        self.invalidate_caption();
    }

    /// Taskbar button until the window comes to the front.
    pub fn flash(&self) {
        let f = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd: self.hwnd.get(),
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        let _ = unsafe { FlashWindowEx(&f) };
    }

    /// DestroyWindow, no on_close.
    pub fn close(&self) {
        let h = self.hwnd.get();
        if !h.is_invalid() {
            let _ = unsafe { DestroyWindow(h) };
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    fn load_icons(&self) {
        for i in [&self.icon, &self.big_icon] {
            let old = i.replace(HICON::default());
            if !old.is_invalid() {
                let _ = unsafe { DestroyIcon(old) };
            }
        }
        // Each at the size Windows draws it at this DPI, picked from the icon's own sizes rather than stretched.
        let load = |size: i32| {
            unsafe {
                LoadIconWithScaleDown(
                    Some(self.inst.get()),
                    PCWSTR(APP_ICON_ID as usize as *const u16),
                    size,
                    size,
                )
            }
            .unwrap_or_default()
        };
        self.icon.set(load(self.px(16)));
        self.big_icon
            .set(load(unsafe { GetSystemMetricsForDpi(SM_CXICON, self.dpi.get()) }));
        let h = self.hwnd.get();
        if !self.icon.get().is_invalid() {
            send(h, WM_SETICON, ICON_SMALL as usize, self.icon.get().0 as isize);
        }
        if !self.big_icon.get().is_invalid() {
            send(h, WM_SETICON, ICON_BIG as usize, self.big_icon.get().0 as isize);
        }
        *self.icon_image.borrow_mut() = icon_image(self.icon.get());
        self.invalidate_caption();
    }

    fn client(&self) -> RECT {
        let mut rc = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd.get(), &mut rc) };
        rc
    }

    // client rect
    fn caption_button(&self, b: i32) -> RECT {
        let rc = self.client();
        let w = self.px(CAPTION_BUTTON_DIP);
        let right = rc.right - if b == CLOSE { 0 } else { w };
        RECT {
            left: right - w,
            top: 0,
            right,
            bottom: self.px(CAPTION_DIP),
        }
    }

    // MINIMIZE, CLOSE, BADGE or -1 = none
    fn caption_button_at(&self, p: POINT) -> i32 {
        if let Some(pill) = self.badge_pill()
            && unsafe { PtInRect(&pill, p) }.as_bool()
        {
            return BADGE;
        }
        [MINIMIZE, CLOSE]
            .into_iter()
            .find(|&b| unsafe { PtInRect(&self.caption_button(b), p) }.as_bool())
            .unwrap_or(-1)
    }

    /// Where the title starts in the caption.
    fn title_left(&self) -> i32 {
        self.px(12) + self.px(16) + self.px(10)
    }

    /// The badge's pill in the client area, if one is shown.
    fn badge_pill(&self) -> Option<RECT> {
        let title = window_text(self.hwnd.get());
        self.badge_rects(self.title_left(), self.px(CAPTION_DIP), &title)
            .map(|(pill, _)| pill)
    }

    fn invalidate_caption(&self) {
        let h = self.hwnd.get();
        if h.is_invalid() {
            return;
        }
        let mut rc = self.client();
        rc.bottom = self.px(CAPTION_DIP);
        let _ = unsafe { InvalidateRect(Some(h), Some(&rc), true) };
    }

    fn text_format(&self, i: usize) -> Option<IDWriteTextFormat> {
        self.fonts.borrow().text[i].clone()
    }

    // Windows 11's title bar: icon and title on the left, minimize and close 46 DIP wide on the right. Close goes red
    // under the mouse; the title and glyphs dim while another window is active.
    fn draw_caption(&self, canvas: &mut Canvas) {
        let (cap, icon, x) = (self.px(CAPTION_DIP), self.px(16), self.px(12));
        let theme = self.theme.get();
        if let Some(img) = self.icon_image.borrow().as_ref() {
            let top = ((cap - icon) / 2) as f32;
            canvas.image(
                img,
                &D2D_RECT_F {
                    left: x as f32,
                    top,
                    right: (x + icon) as f32,
                    bottom: top + icon as f32,
                },
            );
        }
        let title = window_text(self.hwnd.get());
        let ink = if self.active.get() { theme.text } else { theme.text2 };
        let r = D2D_RECT_F {
            left: (x + icon + self.px(10)) as f32,
            top: 0.0,
            right: (self.caption_button(MINIMIZE).left - self.px(8)) as f32,
            bottom: cap as f32,
        };
        canvas.text(&title, self.text_format(0).as_ref(), &r, ink, Align::Left, false);
        if let Some((pill, text)) = self.badge_rects(r.left as i32, cap, &title) {
            let (badge, tone, _) = self.badge.borrow().clone();
            let hot = self.cap_hot.get() == BADGE && self.on_badge_click.borrow().is_some();
            let fill = blend(theme.text, theme.bg, if hot { 0.16 } else { 0.1 });
            canvas.round_rect(&rect_f(&pill), (pill.bottom - pill.top) as f32 / 2.0, fill, None);
            let dot = self.px(BADGE_DOT_DIP) as f32;
            canvas.ellipse(
                Vector2::new(
                    (pill.left + self.px(BADGE_PAD_DIP)) as f32 + dot / 2.0,
                    (pill.top + pill.bottom) as f32 / 2.0,
                ),
                dot / 2.0,
                tone_color(&theme, tone),
            );
            canvas.text(
                &badge,
                self.text_format(0).as_ref(),
                &rect_f(&text),
                theme.text2,
                Align::Left,
                false,
            );
        }
        for b in [MINIMIZE, CLOSE] {
            let r = self.caption_button(b);
            let (hot_b, down_b) = (self.cap_hot.get(), self.cap_down.get());
            let hot = hot_b == b && (down_b < 0 || down_b == b);
            let down = hot && down_b == b;
            let red = rgb(196, 43, 28);
            if hot {
                let c = if b == CLOSE {
                    if down { blend(red, theme.bg, 0.9) } else { red }
                } else {
                    blend(theme.text, theme.bg, if down { 0.05 } else { 0.08 })
                };
                canvas.fill(&rect_f(&r), c, 1.0);
            }
            let glyph = if b == CLOSE && hot { rgb(255, 255, 255) } else { ink };
            let cx = ((r.left + r.right) / 2) as f32;
            let cy = ((r.top + r.bottom) / 2) as f32;
            let arm = self.px(5) as f32;
            let line = self.px(1).max(1) as f32;
            if b == MINIMIZE {
                // whole pixels, so the bar stays sharp
                canvas.fill(
                    &D2D_RECT_F {
                        left: cx - arm,
                        top: cy,
                        right: cx + arm,
                        bottom: cy + line,
                    },
                    glyph,
                    1.0,
                );
            } else {
                canvas.line(
                    Vector2::new(cx - arm, cy - arm),
                    Vector2::new(cx + arm, cy + arm),
                    glyph,
                    line,
                );
                canvas.line(
                    Vector2::new(cx - arm, cy + arm),
                    Vector2::new(cx + arm, cy - arm),
                    glyph,
                    line,
                );
            }
        }
    }

    /// The badge's pill and its text, after `title` drawn from `left` in a caption `cap` pixels tall; None when there
    /// is no badge or it would not fit before the minimize button.
    fn badge_rects(&self, left: i32, cap: i32, title: &str) -> Option<(RECT, RECT)> {
        let badge = self.badge.borrow().0.clone();
        if badge.is_empty() {
            return None;
        }
        let (title_w, badge_w) = (self.text_width(title), self.text_width(&badge));
        let (pad, h) = (self.px(BADGE_PAD_DIP), self.px(18));
        let dot = self.px(BADGE_DOT_DIP) + self.px(5); // the tone dot and its gap before the text
        let pill = RECT {
            left: left + title_w + self.px(8),
            top: (cap - h) / 2,
            right: left + title_w + self.px(8) + dot + badge_w + 2 * pad,
            bottom: (cap - h) / 2 + h,
        };
        (pill.right <= self.caption_button(MINIMIZE).left - self.px(8)).then(|| {
            let text = RECT {
                left: pill.left + pad + dot,
                right: pill.right,
                ..pill
            };
            (pill, text)
        })
    }

    /// Width in pixels of `s` in the body font (the caption uses it too).
    fn text_width(&self, s: &str) -> i32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        unsafe {
            let dc = GetDC(Some(self.hwnd.get()));
            let old = SelectObject(dc, self.fonts.borrow().faces[0].into());
            let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
            SelectObject(dc, old);
            ReleaseDC(Some(self.hwnd.get()), dc);
        }
        size.cx
    }

    fn draw_caption_gdi(&self, dc: HDC) {
        let (cap, icon, x) = (self.px(CAPTION_DIP), self.px(16), self.px(12));
        let theme = self.theme.get();
        unsafe {
            if !self.icon.get().is_invalid() {
                let _ = DrawIconEx(dc, x, (cap - icon) / 2, self.icon.get(), icon, icon, 0, None, DI_NORMAL);
            }
            let mut title: Vec<u16> = window_text(self.hwnd.get()).encode_utf16().collect();
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, if self.active.get() { theme.text } else { theme.text2 });
            let old = SelectObject(dc, self.fonts.borrow().faces[0].into());
            let mut t = RECT {
                left: x + icon + self.px(10),
                top: 0,
                right: self.caption_button(MINIMIZE).left - self.px(8),
                bottom: cap,
            };
            DrawTextW(
                dc,
                &mut title,
                &mut t,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            if let Some((_, mut b)) = self.badge_rects(t.left, cap, &window_text(self.hwnd.get())) {
                let mut badge: Vec<u16> = self.badge.borrow().0.encode_utf16().collect();
                SetTextColor(dc, theme.text2);
                DrawTextW(dc, &mut badge, &mut b, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            }
            for b in [MINIMIZE, CLOSE] {
                let mut r = self.caption_button(b);
                let mut glyph: Vec<u16> = (if b == CLOSE { "\u{2715}" } else { "\u{2013}" })
                    .encode_utf16()
                    .collect();
                DrawTextW(dc, &mut glyph, &mut r, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
            }
            SelectObject(dc, old);
        }
    }

    fn hint(&self, text: &str) {
        if let Some(row) = self.spec.hint_row {
            self.set_text(row, text);
        }
    }

    fn on_edit(&self, r: &Row, kill_focus: bool) {
        let mut buf = [0u16; 16];
        unsafe { GetDlgItemTextW(self.hwnd.get(), r.id, &mut buf) };
        let last = self.numbers.borrow().get(&r.id).copied();
        if let Some(v) = parse_bounded(&from_wide(&buf), r.lo, r.hi) {
            self.hint("");
            if last == Some(v) {
                return;
            }
            self.numbers.borrow_mut().insert(r.id, v);
            let f = self.on_number.borrow().clone();
            if let Some(f) = f {
                f(self, r.id, v);
            }
        } else if kill_focus {
            // leaving an invalid field restores the last valid value
            self.hint("");
            unsafe {
                let _ = match last {
                    Some(v) => SetDlgItemInt(self.hwnd.get(), r.id, v as u32, true),
                    None => SetDlgItemTextW(self.hwnd.get(), r.id, w!("")),
                };
            }
        } else {
            self.hint(&format!("Enter a whole number from {} to {} (not saved).", r.lo, r.hi));
        }
    }

    // A step button or the wheel: the field's value (else the last one, else lo) moved by `step` within lo..=hi, put
    // in the field as if typed, so EN_CHANGE reports it through on_edit like typing does.
    fn step(&self, i: usize, step: i32) {
        let r = &self.spec.rows[i];
        let field = self.ctls.borrow()[i].1;
        let typed = parse_bounded(&window_text(field), r.lo, r.hi);
        let base = typed
            .or_else(|| self.numbers.borrow().get(&r.id).copied())
            .unwrap_or(r.lo);
        let v = base.saturating_add(step).clamp(r.lo, r.hi);
        if typed != Some(v) {
            let _ = unsafe { SetDlgItemInt(self.hwnd.get(), r.id, v as u32, true) };
        }
    }

    // Asks the game for option k of Choice row i (when it isn't the shown one), then shows the answer at once.
    fn pick(&self, i: usize, k: usize) {
        let r = &self.spec.rows[i];
        if k >= r.options.len() || self.choices.borrow().get(&r.id) == Some(&k) {
            return;
        }
        let f = self.on_choice.borrow().clone();
        if let Some(f) = f {
            f(self, r.id, k);
            if !self.hwnd.get().is_invalid() {
                self.tick(); // on_choice may have closed the window
            }
        }
    }

    fn apply_theme(&self) {
        let theme = system_theme();
        self.theme.set(theme);
        let old = self.brushes.replace(unsafe {
            [
                CreateSolidBrush(theme.bg),
                CreateSolidBrush(theme.card),
                CreateSolidBrush(theme.field),
            ]
        });
        for b in old {
            if !b.is_invalid() {
                let _ = unsafe { DeleteObject(b.into()) };
            }
        }
        let h = self.hwnd.get();
        let dark = BOOL::from(theme.dark);
        unsafe {
            let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const dark).cast(), 4);
            // one surface from title bar to bottom edge (Windows 11; ignored before)
            let caption = theme.bg;
            let _ = DwmSetWindowAttribute(h, DWMWA_CAPTION_COLOR, (&raw const caption).cast(), 4);
        }
        self.theme_trees();
        let _ = unsafe {
            RedrawWindow(
                Some(h),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME,
            )
        };
    }

    // Tree views draw themselves: card background, theme text, and the dark scrollbars, expand arrows and checkboxes
    // in dark mode.
    fn theme_trees(&self) {
        let theme = self.theme.get();
        let ctls = self.ctls.borrow().clone();
        for (r, &(_, c)) in self.spec.rows.iter().zip(&ctls) {
            if r.kind != Kind::Tree {
                continue;
            }
            let name = if theme.dark {
                w!("DarkMode_Explorer")
            } else {
                w!("Explorer")
            };
            let _ = unsafe { SetWindowTheme(c, name, PCWSTR::null()) };
            send(c, TVM_SETBKCOLOR, 0, theme.card.0 as isize);
            send(c, TVM_SETTEXTCOLOR, 0, theme.text.0 as isize);
        }
    }

    fn apply_fonts(&self) {
        let h = self.hwnd.get();
        let mut f = Fonts::default();
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
                self.dpi.get(),
            )
        };
        let mut lf: LOGFONTW = ncm.lfMessageFont;
        let family: Vec<u16> = self.family.borrow().encode_utf16().collect();
        if !family.is_empty() && family.len() < lf.lfFaceName.len() {
            lf.lfFaceName = [0; 32];
            lf.lfFaceName[..family.len()].copy_from_slice(&family);
        }
        for i in 0..4 {
            let mut face = lf;
            if i & 1 != 0 {
                face.lfWeight = FW_BOLD.0 as i32;
            }
            face.lfItalic = u8::from(i & 2 != 0);
            f.faces[i] = unsafe { CreateFontIndirectW(&face) };
            f.text[i] = text_format(&face);
        }
        lf.lfWeight = FW_SEMIBOLD.0 as i32;
        f.semibold = unsafe { CreateFontIndirectW(&lf) };
        f.semibold_text = text_format(&lf);
        lf.lfHeight = lf.lfHeight * 3 / 2;
        f.big = unsafe { CreateFontIndirectW(&lf) };
        f.big_text = text_format(&lf);
        unsafe {
            let dc = GetDC(Some(h));
            let mut tm = TEXTMETRICW::default();
            let old = SelectObject(dc, f.big.into());
            let _ = GetTextMetricsW(dc, &mut tm);
            self.big_line_h.set(tm.tmHeight);
            SelectObject(dc, f.faces[0].into());
            let _ = GetTextMetricsW(dc, &mut tm);
            self.line_h.set(tm.tmHeight);
            self.ascent.set(tm.tmAscent);
            SelectObject(dc, old);
            ReleaseDC(Some(h), dc);
        }
        let (font, semibold) = (f.faces[0], f.semibold);
        let old = self.fonts.replace(f);
        let ctls = self.ctls.borrow().clone();
        for (r, &(label, ctl)) in self.spec.rows.iter().zip(&ctls) {
            if !label.is_invalid() {
                let lf = if r.kind == Kind::Section { semibold } else { font };
                send(label, WM_SETFONT, lf.0 as usize, 0);
            }
            if !ctl.is_invalid() {
                send(ctl, WM_SETFONT, font.0 as usize, 0);
            }
        }
        for o in old.faces.into_iter().chain([old.semibold, old.big]) {
            if !o.is_invalid() {
                let _ = unsafe { DeleteObject(o.into()) };
            }
        }
    }

    // Lays out every page from the same top in DIPs scaled to the current DPI; the window fits the shown page (fit).
    // Header (dot, title, two detail lines while there is detail), tabs when there are 2+ pages, then section titles
    // over cards.
    fn layout(&self, suggested: Option<&RECT>) {
        let px = |d| self.px(d);
        let (m, pad, pad_v, row_h) = (px(16), px(14), px(6), px(34));
        let widest = self
            .spec
            .rows
            .iter()
            .filter(|r| matches!(r.kind, Kind::Value | Kind::Number | Kind::Text | Kind::Choice))
            .map(|r| self.text_width(&r.text))
            .max()
            .unwrap_or(0);
        // a long label moves the value column right rather than being cut (labels sit px(8) short of the column)
        let label_w = (widest + px(8)).min(px(LABEL_MAX_DIP)).max(px(self.spec.label_width));
        let value_w = px(self.spec.value_width);
        let card_w = label_w + value_w + 2 * pad;
        let x = m + pad;
        let h = self.hwnd.get();
        let (font, semibold, big) = {
            let f = self.fonts.borrow();
            (f.faces[0], f.semibold, f.big)
        };
        let ctls = self.ctls.borrow().clone();
        let page_of = self.page_of.borrow().clone();
        let steppers = self.steppers.borrow().clone();
        let n = ctls.len();
        let (mut boxes, mut slots, mut cards) = (vec![RECT::default(); n], vec![RECT::default(); n], Vec::new());
        let mut bottoms;
        unsafe {
            let dc = GetDC(Some(h));
            let mut tm = TEXTMETRICW::default();
            let old = SelectObject(dc, big.into());
            let _ = GetTextMetricsW(dc, &mut tm);
            let big_h = tm.tmHeight;
            SelectObject(dc, font.into());
            let _ = GetTextMetricsW(dc, &mut tm);
            let line_h = tm.tmHeight;
            let text_dy = (row_h - line_h) / 2;

            let detail_h = if self.status_detail.borrow().is_empty() {
                0
            } else {
                px(2) + 2 * line_h
            };
            let header_h = big_h + detail_h;
            let header_top = px(CAPTION_DIP) + m; // under the title bar
            let _ = MoveWindow(self.header.get(), m, header_top, card_w, header_h, false);
            self.header_rect.set(RECT {
                left: m,
                top: header_top,
                right: m + card_w,
                bottom: header_top + header_h,
            });
            let mut top = header_top + header_h + px(12);
            if self.pages.get() > 1 {
                SelectObject(dc, semibold.into());
                let mut tx = m;
                for (r, &(_, ctl)) in self.spec.rows.iter().zip(&ctls) {
                    if r.kind == Kind::Page {
                        let t16: Vec<u16> = r.text.encode_utf16().collect();
                        let mut t = windows::Win32::Foundation::SIZE::default();
                        let _ = GetTextExtentPoint32W(dc, &t16, &mut t);
                        let _ = MoveWindow(ctl, tx, top, t.cx + px(28), px(36), false);
                        tx += t.cx + px(28);
                    }
                }
                top += px(36) + px(12);
            }

            bottoms = vec![top; self.pages.get() as usize];
            let mut y = top;
            let mut card_top = -1;
            let mut page = 0;
            let close_card = |y: &mut i32, card_top: &mut i32, page: i32, cards: &mut Vec<(i32, RECT)>| {
                if *card_top < 0 {
                    return;
                }
                *y += pad_v;
                cards.push((
                    page,
                    RECT {
                        left: m,
                        top: *card_top,
                        right: m + card_w,
                        bottom: *y,
                    },
                ));
                *card_top = -1;
            };
            let open_card = |y: &mut i32, card_top: &mut i32| {
                if *card_top >= 0 {
                    return;
                }
                *card_top = *y;
                *y += pad_v;
            };
            let mut tile_k = 0usize; // this row's place in the run of tiles it is in
            let mut tile_h = 0; // the current line of tiles' height
            for (i, r) in self.spec.rows.iter().enumerate() {
                let (label, ctl) = ctls[i];
                let rh = row_h + (r.lines - 1) * line_h; // extra lines at text spacing, not full rows
                if r.kind != Kind::Tile {
                    tile_k = 0;
                }
                match r.kind {
                    Kind::Tile => {
                        open_card(&mut y, &mut card_top);
                        if tile_k.is_multiple_of(2) {
                            // a new line of tiles, as tall as its taller tile so the two sit level
                            let next = self.spec.rows.get(i + 1).filter(|n| n.kind == Kind::Tile);
                            tile_h = next.map_or(self.tile_height(r), |n| self.tile_height(r).max(self.tile_height(n)));
                            y += px(4) + tile_h + px(4);
                        }
                        let (dx, tw) = tile_column(tile_k, label_w + value_w, px(8));
                        let t = RECT {
                            left: x + dx,
                            top: y - px(4) - tile_h,
                            right: x + dx + tw,
                            bottom: y - px(4),
                        };
                        let _ = MoveWindow(ctl, t.left, t.top, tw, tile_h, false);
                        if r.picture != 0 {
                            slots[i] = self.tile_picture(&t, r.picture);
                        }
                        tile_k += 1;
                    }
                    Kind::Page => {
                        close_card(&mut y, &mut card_top, page, &mut cards);
                        bottoms[page as usize] = bottoms[page as usize].max(y);
                        page = page_of[i];
                        y = top;
                    }
                    Kind::Section => {
                        close_card(&mut y, &mut card_top, page, &mut cards);
                        if y > top {
                            y += px(14);
                        }
                        let _ = MoveWindow(label, m + px(2), y, card_w, line_h, false);
                        y += line_h + px(6);
                    }
                    Kind::Value => {
                        open_card(&mut y, &mut card_top);
                        let _ = MoveWindow(label, x, y + text_dy, label_w - px(8), line_h, false);
                        let pic = if r.picture != 0 { px(r.picture) } else { 0 };
                        let tw = value_w - if pic != 0 { pic + px(8) } else { 0 };
                        let _ = MoveWindow(ctl, x + label_w, y + text_dy, tw, rh - text_dy, false);
                        if pic != 0 {
                            slots[i] = RECT {
                                left: x + label_w + value_w - pic,
                                top: y + px(4),
                                right: x + label_w + value_w,
                                bottom: y + rh - px(4),
                            };
                        }
                        y += rh;
                    }
                    Kind::Number | Kind::Text => {
                        open_card(&mut y, &mut card_top);
                        let _ = MoveWindow(label, x, y + text_dy, label_w - px(8), line_h, false);
                        // a Number's - and + sit inside its box, right of the field
                        let steps = if r.kind == Kind::Number { 2 * px(STEP_DIP) } else { 0 };
                        let bw = if r.kind == Kind::Text { value_w } else { px(84) + steps };
                        let b = RECT {
                            left: x + label_w,
                            top: y + px(3),
                            right: x + label_w + bw,
                            bottom: y + row_h - px(3),
                        };
                        boxes[i] = b;
                        let _ = MoveWindow(
                            ctl,
                            b.left + px(10),
                            b.top + (b.bottom - b.top - line_h) / 2,
                            b.right - b.left - px(20) - steps,
                            line_h,
                            false,
                        );
                        for &(btn, _, step) in steppers.iter().filter(|s| s.1 == i) {
                            let right = b.right - px(2) - if step < 0 { px(STEP_DIP) } else { 0 };
                            let (t, bh) = (b.top + px(3), b.bottom - b.top - px(6)); // clear of the bottom line
                            let _ = MoveWindow(btn, right - px(STEP_DIP), t, px(STEP_DIP), bh, false);
                        }
                        y += rh;
                    }
                    Kind::Choice => {
                        open_card(&mut y, &mut card_top);
                        let _ = MoveWindow(label, x, y + text_dy, label_w - px(8), line_h, false);
                        // equal segments, each as wide as the widest option needs
                        let seg = r.options.iter().map(|o| self.text_width(o)).max().unwrap_or(0) + px(24);
                        let cw = (seg * r.options.len().max(1) as i32).min(value_w);
                        let _ = MoveWindow(ctl, x + label_w, y + px(3), cw, row_h - px(6), false);
                        y += rh;
                    }
                    Kind::Check => {
                        open_card(&mut y, &mut card_top);
                        let _ = MoveWindow(ctl, x, y, label_w + value_w, row_h, false);
                        y += rh;
                    }
                    Kind::Note => {
                        close_card(&mut y, &mut card_top, page, &mut cards);
                        let _ = MoveWindow(ctl, m + px(2), y + px(4), card_w, line_h, false);
                        y += line_h + px(8);
                    }
                    Kind::Tree => {
                        open_card(&mut y, &mut card_top);
                        let tree_h = r.lines * (line_h + px(8)) + px(4);
                        let _ = MoveWindow(ctl, x, y + px(2), label_w + value_w, tree_h, false);
                        y += tree_h + px(6);
                    }
                }
            }
            close_card(&mut y, &mut card_top, page, &mut cards);
            bottoms[page as usize] = bottoms[page as usize].max(y);
            SelectObject(dc, old);
            ReleaseDC(Some(h), dc);
        }
        *self.boxes.borrow_mut() = boxes;
        *self.slots.borrow_mut() = slots;
        *self.cards.borrow_mut() = cards;
        *self.page_bottoms.borrow_mut() = bottoms;
        self.width.set(m + card_w + m);
        self.sync_status_tip();
        self.fit(suggested);
        let _ = unsafe { InvalidateRect(Some(h), None, true) };
    }

    // The one place the window is sized: as wide as the cards, as tall as the shown page. The top-left stays put
    // unless a DPI change suggests a new place.
    fn fit(&self, suggested: Option<&RECT>) {
        let h = self.hwnd.get();
        let bottom = self
            .page_bottoms
            .borrow()
            .get(self.page.get() as usize)
            .copied()
            .unwrap_or(0);
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: self.width.get(),
            bottom: bottom + self.px(16),
        };
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongPtrW(h, GWL_STYLE) as u32);
            let ex = WINDOW_EX_STYLE(GetWindowLongPtrW(h, GWL_EXSTYLE) as u32);
            let _ = AdjustWindowRectExForDpi(&mut rc, style, false, ex, self.dpi.get());
            rc.top = 0; // no system caption or top frame (WM_NCCALCSIZE): the client area starts at the window's top edge
            let flags = SWP_NOZORDER
                | SWP_NOACTIVATE
                | if suggested.is_some() {
                    Default::default()
                } else {
                    SWP_NOMOVE
                };
            let (sx, sy) = suggested.map_or((0, 0), |s| (s.left, s.top));
            let _ = SetWindowPos(h, None, sx, sy, rc.right - rc.left, rc.bottom - rc.top, flags);
        }
    }

    fn build(&self, inst: HINSTANCE) {
        let h = self.hwnd.get();
        let child = |cls: PCWSTR, text: &str, style: u32, id: i32| unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls,
                &HSTRING::from(text),
                WS_CHILD | WINDOW_STYLE(style),
                0,
                0,
                0,
                0,
                Some(h),
                Some(HMENU(id as isize as *mut _)),
                Some(inst),
                None,
            )
            .unwrap_or_default()
        };
        let (stat, button, edit) = (w!("STATIC"), w!("BUTTON"), w!("EDIT"));
        let label_style = SS_LEFT.0 | SS_NOPREFIX.0 | SS_ENDELLIPSIS.0;
        self.header.set(child(stat, "", WS_VISIBLE.0 | SS_OWNERDRAW.0, 0));
        let mut page = 0;
        let mut seen_page = false;
        let (mut page_of, mut ctls, mut looks, mut steppers) = (Vec::new(), Vec::new(), HashMap::new(), Vec::new());
        let null = HWND::default();
        for r in &self.spec.rows {
            if r.kind == Kind::Page && std::mem::replace(&mut seen_page, true) {
                page += 1;
            }
            page_of.push(page);
            let (mut label, mut ctl) = (null, null);
            match r.kind {
                Kind::Page => ctl = child(button, &r.text, BS_PUSHBUTTON as u32 | WS_TABSTOP.0, 0), // custom drawn
                Kind::Section => {
                    label = child(stat, &r.text, SS_LEFT.0 | SS_NOPREFIX.0, 0);
                    looks.insert(
                        label.0 as isize,
                        Look {
                            role: Role::Text,
                            on_card: false,
                        },
                    );
                }
                Kind::Value => {
                    label = child(stat, &r.text, label_style, 0);
                    let style = if r.rich {
                        SS_OWNERDRAW.0
                    } else {
                        SS_LEFT.0 | SS_NOPREFIX.0
                    };
                    ctl = child(stat, "", style, r.id);
                    looks.insert(
                        label.0 as isize,
                        Look {
                            role: Role::Muted,
                            on_card: true,
                        },
                    );
                    looks.insert(
                        ctl.0 as isize,
                        Look {
                            role: Role::Text,
                            on_card: true,
                        },
                    );
                }
                // the label is drawn in the box; until a value comes it is also the accessible name
                Kind::Tile => ctl = child(stat, &r.text, SS_OWNERDRAW.0, r.id),
                Kind::Check => {
                    // the game owns the state; drawn as a switch
                    ctl = child(button, &r.text, BS_CHECKBOX as u32 | WS_TABSTOP.0, r.id);
                    self.shown.borrow_mut().insert(r.id, r.text.clone());
                }
                Kind::Number => {
                    label = child(stat, &r.text, label_style, 0);
                    ctl = child(edit, "", WS_TABSTOP.0 | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32, r.id);
                    looks.insert(
                        label.0 as isize,
                        Look {
                            role: Role::Muted,
                            on_card: true,
                        },
                    );
                    send(ctl, EM_SETLIMITTEXT, 6, 0);
                    // - and +, custom drawn; no tab stop, since the keyboard types in the field itself
                    for (glyph, step) in [("\u{2212}", -1), ("+", 1)] {
                        steppers.push((child(button, glyph, BS_PUSHBUTTON as u32, 0), ctls.len(), step));
                    }
                }
                Kind::Choice => {
                    label = child(stat, &r.text, label_style, 0);
                    // a real button for focus and screen readers (it reads the label); drawn as segments
                    ctl = child(button, &r.text, BS_PUSHBUTTON as u32 | WS_TABSTOP.0, r.id);
                    looks.insert(
                        label.0 as isize,
                        Look {
                            role: Role::Muted,
                            on_card: true,
                        },
                    );
                }
                Kind::Text => {
                    label = child(stat, &r.text, label_style, 0);
                    ctl = child(edit, "", WS_TABSTOP.0 | ES_AUTOHSCROLL as u32, r.id);
                    looks.insert(
                        label.0 as isize,
                        Look {
                            role: Role::Muted,
                            on_card: true,
                        },
                    );
                }
                Kind::Note => {
                    ctl = child(stat, "", SS_LEFT.0 | SS_NOPREFIX.0, r.id);
                    looks.insert(
                        ctl.0 as isize,
                        Look {
                            role: Role::Warn,
                            on_card: false,
                        },
                    );
                }
                Kind::Tree => {
                    register_common_controls();
                    let style = TVS_HASBUTTONS
                        | TVS_LINESATROOT
                        | TVS_SHOWSELALWAYS
                        | TVS_FULLROWSELECT
                        | WS_TABSTOP.0
                        | WS_VSCROLL.0;
                    ctl = child(WC_TREEVIEWW, "", style, r.id);
                    unsafe {
                        // TVS_CHECKBOXES only works when added after creation, before any item
                        let s = GetWindowLongPtrW(ctl, GWL_STYLE);
                        SetWindowLongPtrW(ctl, GWL_STYLE, s | TVS_CHECKBOXES as isize);
                    }
                    send(
                        ctl,
                        TVM_SETEXTENDEDSTYLE,
                        TVS_EX_DOUBLEBUFFER as usize,
                        TVS_EX_DOUBLEBUFFER as isize,
                    );
                }
            }
            ctls.push((label, ctl));
        }
        self.pages.set(page + 1);
        let n = ctls.len();
        *self.page_of.borrow_mut() = page_of;
        *self.ctls.borrow_mut() = ctls;
        *self.looks.borrow_mut() = looks;
        *self.steppers.borrow_mut() = steppers;
        *self.boxes.borrow_mut() = vec![RECT::default(); n];
        *self.slots.borrow_mut() = vec![RECT::default(); n];
        self.theme_trees();
        self.apply_fonts();
        self.layout(None);
        self.show_page(0);
    }

    fn paint(&self, dc: HDC, area: &RECT) {
        let client = self.client();
        if unsafe { IsRectEmpty(area) }.as_bool() {
            return;
        }
        let theme = self.theme.get();
        let page = self.page.get();
        let [bg_brush, card_brush, field_brush] = self.brushes.get();
        let cards = self.cards.borrow().clone();
        let boxes = self.boxes.borrow().clone();
        let page_of = self.page_of.borrow().clone();
        let mut canvas = self.canvas.try_borrow_mut().ok();
        let began = canvas.as_mut().is_some_and(|c| c.begin(dc, &client, Some(area)));
        let Some(canvas) = canvas.as_mut().filter(|_| began) else {
            // no Direct2D: GDI, square cards
            unsafe {
                FillRect(dc, area, bg_brush);
                self.draw_caption_gdi(dc);
                for (p, rc) in &cards {
                    if *p == page {
                        FillRect(dc, rc, card_brush);
                    }
                }
                for (i, b) in boxes.iter().enumerate() {
                    if !is_empty(b) && page_of[i] == page {
                        FillRect(dc, b, field_brush);
                    }
                }
            }
            return;
        };
        canvas.fill(&rect_f(area), theme.bg, 1.0); // the canvas replaces everything in area
        self.draw_caption(canvas);
        for (p, rc) in &cards {
            if *p == page {
                canvas.round_rect(&rect_f(rc), self.px(8) as f32, theme.card, Some(theme.stroke));
            }
        }
        let ctls = self.ctls.borrow().clone();
        let focus = unsafe { GetFocus() };
        for (i, b) in boxes.iter().enumerate() {
            if is_empty(b) || page_of[i] != page {
                continue;
            }
            let focused = focus == ctls[i].1;
            canvas.round_rect(&rect_f(b), self.px(4) as f32, theme.field, Some(theme.stroke));
            // Windows 11 text box: a thin bottom line at rest, a thicker accent one while editing.
            let line_h = if focused { self.px(2) } else { self.px(1).max(1) } as f32;
            let r = D2D_RECT_F {
                left: (b.left + self.px(2)) as f32,
                top: b.bottom as f32 - line_h,
                right: (b.right - self.px(2)) as f32,
                bottom: b.bottom as f32,
            };
            canvas.fill(&r, if focused { theme.accent } else { theme.field_line }, 1.0);
        }
        let slots = self.slots.borrow().clone();
        let pictures = self.pictures.borrow();
        for (i, s) in slots.iter().enumerate() {
            // a tile draws its own picture: the parent's paint is clipped out under it
            if is_empty(s) || page_of[i] != page || self.spec.rows[i].kind == Kind::Tile {
                continue;
            }
            let Some((_, _, Some(img))) = pictures.get(&self.spec.rows[i].id) else {
                continue;
            };
            let Some((w, h)) = fitted(img, s) else {
                continue;
            };
            // right-aligned and centered vertically
            let top = s.top as f32 + ((s.bottom - s.top) as f32 - h) / 2.0;
            canvas.image(
                img,
                &D2D_RECT_F {
                    left: s.right as f32 - w,
                    top,
                    right: s.right as f32,
                    bottom: top + h,
                },
            );
        }
        canvas.end();
    }

    fn draw_header(&self, di: &DRAWITEMSTRUCT) {
        let rc = di.rcItem;
        let theme = self.theme.get();
        let dot = tone_color(&theme, self.tone.get());
        let d = self.px(10);
        let left = rc.left + d + self.px(12);
        let big_line_h = self.big_line_h.get();
        let title = self.status_title.borrow().clone();
        let detail = self.status_detail.borrow().clone();
        let mut canvas = self.canvas.try_borrow_mut().ok();
        let began = canvas.as_mut().is_some_and(|c| c.begin(di.hDC, &rc, None));
        let Some(canvas) = canvas.as_mut().filter(|_| began) else {
            // no Direct2D: GDI text, no dot
            let dc = di.hDC;
            let f = self.fonts.borrow();
            unsafe {
                FillRect(dc, &rc, self.brushes.get()[0]);
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, theme.text);
                let old = SelectObject(dc, f.big.into());
                let mut t = RECT {
                    left,
                    top: rc.top,
                    right: rc.right,
                    bottom: rc.top + big_line_h,
                };
                let mut s: Vec<u16> = title.encode_utf16().collect();
                DrawTextW(dc, &mut s, &mut t, DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                SelectObject(dc, f.faces[0].into());
                SetTextColor(dc, theme.text2);
                let mut t = RECT {
                    left,
                    top: t.bottom + self.px(2),
                    right: rc.right,
                    bottom: rc.bottom,
                };
                let mut s: Vec<u16> = detail.encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut s,
                    &mut t,
                    DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
                SelectObject(dc, old);
            }
            return;
        };
        let title_bottom = (rc.top + big_line_h) as f32;
        canvas.fill(&rect_f(&rc), theme.bg, 1.0);
        let center = Vector2::new(
            (rc.left + self.px(2)) as f32 + d as f32 / 2.0,
            rc.top as f32 + big_line_h as f32 / 2.0,
        );
        canvas.ellipse(center, d as f32 / 2.0, dot);
        let (big, body) = {
            let f = self.fonts.borrow();
            (f.big_text.clone(), f.text[0].clone())
        };
        let r = D2D_RECT_F {
            left: left as f32,
            top: rc.top as f32,
            right: rc.right as f32,
            bottom: title_bottom,
        };
        canvas.text(&title, big.as_ref(), &r, theme.text, Align::Left, false);
        let r = D2D_RECT_F {
            left: left as f32,
            top: title_bottom + self.px(2) as f32,
            right: rc.right as f32,
            bottom: rc.bottom as f32,
        };
        canvas.text(&detail, body.as_ref(), &r, theme.text2, Align::Left, true);
        canvas.end();
    }

    fn draw_spans(&self, di: &DRAWITEMSTRUCT, spans: &[Span]) {
        let rc = di.rcItem;
        let theme = self.theme.get();
        let body = self.text_format(0);
        let mut canvas = self.canvas.try_borrow_mut().ok();
        let began = body.is_some() && canvas.as_mut().is_some_and(|c| c.begin(di.hDC, &rc, None));
        let (Some(body), Some(canvas)) = (body.filter(|_| began), canvas.as_mut().filter(|_| began)) else {
            let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
            return self.draw_plain_gdi(di, &plain);
        };
        canvas.fill(&rect_f(&rc), theme.card, 1.0);
        self.span_layout(canvas, spans, &body, &rc, theme.card, false);
        canvas.end();
    }

    // No Direct2D: the plain text in the theme's color on the card.
    fn draw_plain_gdi(&self, di: &DRAWITEMSTRUCT, text: &str) {
        let dc = di.hDC;
        unsafe {
            FillRect(dc, &di.rcItem, self.brushes.get()[1]);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, self.theme.get().text);
            let old = SelectObject(dc, self.fonts.borrow().faces[0].into());
            let mut t = di.rcItem;
            let mut s: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(dc, &mut s, &mut t, DT_NOPREFIX | DT_NOCLIP);
            SelectObject(dc, old);
        }
    }

    // A tile's height: its label, big line and small line (a picture tile: label, picture and `lines` body lines),
    // inside the box's padding.
    fn tile_height(&self, r: &Row) -> i32 {
        let line_h = self.line_h.get();
        if r.picture != 0 {
            self.px(24) + (1 + r.lines) * line_h + self.px(r.picture)
        } else {
            self.px(16) + 2 * line_h + self.big_line_h.get()
        }
    }

    // A picture tile's picture area in a tile at `rc`: under the label, the box's width inside its padding.
    fn tile_picture(&self, rc: &RECT, dip: i32) -> RECT {
        let top = rc.top + self.px(12) + self.line_h.get();
        RECT {
            left: rc.left + self.px(12),
            top,
            right: rc.right - self.px(12),
            bottom: top + self.px(dip),
        }
    }

    // A rounded box a shade off the card, like a field: the label small and muted, the value's first line in the
    // status header's big face, its second line small and muted where a span has no color of its own. A picture tile:
    // the picture under the label, then each line in the body face.
    fn draw_tile(&self, di: &DRAWITEMSTRUCT, r: &Row, spans: &[Span]) {
        let (rc, label) = (di.rcItem, r.text.as_str());
        let theme = self.theme.get();
        let (big, body) = {
            let f = self.fonts.borrow();
            (f.big_text.clone(), f.text[0].clone())
        };
        let mut canvas = self.canvas.try_borrow_mut().ok();
        let began = big.is_some() && body.is_some() && canvas.as_mut().is_some_and(|c| c.begin(di.hDC, &rc, None));
        let (Some(big), Some(body), Some(canvas)) = (
            big.filter(|_| began),
            body.filter(|_| began),
            canvas.as_mut().filter(|_| began),
        ) else {
            let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
            return self.draw_plain_gdi(di, &format!("{label}\n{plain}"));
        };
        canvas.fill(&rect_f(&rc), theme.card, 1.0); // the corners outside the box
        canvas.round_rect(&rect_f(&rc), self.px(6) as f32, theme.field, Some(theme.stroke));
        let (line_h, big_h) = (self.line_h.get(), self.big_line_h.get());
        let (left, right) = (rc.left + self.px(12), rc.right - self.px(12));
        let at = |top: i32, h: i32| RECT {
            left,
            top,
            right,
            bottom: top + h,
        };
        let top = rc.top + self.px(8);
        canvas.text(
            label,
            Some(&body),
            &rect_f(&at(top, line_h)),
            theme.text2,
            Align::Left,
            false,
        );
        if r.picture != 0 {
            let p = self.tile_picture(&rc, r.picture);
            if let Some((_, _, Some(img))) = self.pictures.borrow().get(&r.id)
                && let Some((w, h)) = fitted(img, &p)
            {
                let (cx, cy) = ((p.left + p.right) as f32 / 2.0, (p.top + p.bottom) as f32 / 2.0);
                let to = D2D_RECT_F {
                    left: cx - w / 2.0,
                    top: cy - h / 2.0,
                    right: cx + w / 2.0,
                    bottom: cy + h / 2.0,
                };
                canvas.image(img, &to);
            }
            for (k, line) in span_lines(spans).iter().take(r.lines.max(0) as usize).enumerate() {
                let t = p.bottom + self.px(4) + k as i32 * line_h;
                self.span_layout(canvas, line, &body, &at(t, line_h), theme.field, true);
            }
            canvas.end();
            return;
        }
        let (first, mut second) = tile_lines(spans);
        for s in second.iter_mut().filter(|s| s.colors.is_empty()) {
            s.colors = vec![theme.text2];
        }
        self.span_layout(canvas, &first, &big, &at(top + line_h, big_h), theme.field, true);
        self.span_layout(
            canvas,
            &second,
            &body,
            &at(top + line_h + big_h, line_h),
            theme.field,
            true,
        );
        canvas.end();
    }

    // Spans left to right on `bg`, a new line at each '\n', cut at r's right edge (one_line: one line, cut with "…"):
    // one DirectWrite layout, each span its own face, each glyph its own color (a gradient span's change along it). An
    // outlined span's outline is the text drawn a DIP off in all 8 directions under the fill, as the wiki draws the
    // game's outlined text (text-shadow). Color glyphs (emoji) keep their own colors.
    fn span_layout(
        &self,
        canvas: &mut Canvas,
        spans: &[Span],
        format: &IDWriteTextFormat,
        rc: &RECT,
        bg: COLORREF,
        one_line: bool,
    ) {
        let theme = self.theme.get();
        let all: Vec<u16> = spans.iter().flat_map(|s| s.text.encode_utf16()).collect();
        let (w, h) = ((rc.right - rc.left) as f32, (rc.bottom - rc.top) as f32);
        let Some(layout) = dwrite_factory().and_then(|dw| unsafe { dw.CreateTextLayout(&all, format, w, h) }.ok())
        else {
            return;
        };
        let sign = if one_line { canvas.ellipsis(format) } else { None };
        unsafe {
            // the format is shared with single-line text: set everything this needs
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let trim = DWRITE_TRIMMING {
                granularity: if one_line {
                    DWRITE_TRIMMING_GRANULARITY_CHARACTER
                } else {
                    DWRITE_TRIMMING_GRANULARITY_NONE
                },
                delimiter: 0,
                delimiterCount: 0,
            };
            let _ = layout.SetTrimming(&trim, sign.as_ref());
            if !one_line {
                // lines as far apart as the controls' (GDI) lines: the rows were laid out for them
                let _ = layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    self.line_h.get() as f32,
                    self.ascent.get() as f32,
                );
            }
        }
        let len = |s: &Span| s.text.encode_utf16().count() as u32;
        let mut pos = 0u32;
        let mut outlined = false;
        for s in spans {
            let range = DWRITE_TEXT_RANGE {
                startPosition: pos,
                length: len(s),
            };
            let o = s.stroke_opacity > 0.0;
            outlined |= o;
            let edge = if o { blend(s.stroke, bg, s.stroke_opacity) } else { bg };
            unsafe {
                let _ = layout.SetFontWeight(
                    if s.bold {
                        DWRITE_FONT_WEIGHT_BOLD
                    } else {
                        DWRITE_FONT_WEIGHT_NORMAL
                    },
                    range,
                );
                let _ = layout.SetFontStyle(
                    if s.italic {
                        DWRITE_FONT_STYLE_ITALIC
                    } else {
                        DWRITE_FONT_STYLE_NORMAL
                    },
                    range,
                );
                if let Some(b) = canvas.brush(edge, if o { 1.0 } else { 0.0 }) {
                    let _ = layout.SetDrawingEffect(&b, range);
                }
            }
            pos += range.length;
        }
        let rt = canvas.rt().clone();
        let (x, y, off) = (rc.left as f32, rc.top as f32, self.px(1) as f32);
        if outlined && let Some(clear) = canvas.brush(bg, 0.0) {
            // outlines first; the other spans are drawn see-through in this pass
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if dx != 0 || dy != 0 {
                        let at = Vector2::new(x + dx as f32 * off, y + dy as f32 * off);
                        unsafe { rt.DrawTextLayout(at, &layout, &clear, D2D1_DRAW_TEXT_OPTIONS_CLIP) };
                    }
                }
            }
        }
        pos = 0;
        for s in spans {
            let edge = blend(s.stroke, bg, s.stroke_opacity);
            let mut ink = |c: COLORREF| {
                let c = if s.stroke_opacity > 0.0 {
                    readable_on_outlined(c, bg, edge)
                } else {
                    readable_on(c, bg)
                };
                canvas.brush(c, 1.0)
            };
            let units: Vec<u16> = s.text.encode_utf16().collect();
            if s.colors.len() < 2 {
                if let Some(b) = ink(s.colors.first().copied().unwrap_or(theme.text)) {
                    let range = DWRITE_TEXT_RANGE {
                        startPosition: pos,
                        length: units.len() as u32,
                    };
                    let _ = unsafe { layout.SetDrawingEffect(&b, range) };
                }
            } else {
                let nl = u16::from(b'\n');
                let glyphs = units.iter().filter(|&&c| c != nl).count();
                let mut done = 0usize; // the gradient position
                for (k, &c) in units.iter().enumerate() {
                    if c == nl {
                        continue;
                    }
                    let t = if glyphs > 1 {
                        done as f32 / (glyphs - 1) as f32
                    } else {
                        0.0
                    };
                    done += 1;
                    if let Some(b) = ink(gradient_at(&s.colors, t)) {
                        let range = DWRITE_TEXT_RANGE {
                            startPosition: pos + k as u32,
                            length: 1,
                        };
                        let _ = unsafe { layout.SetDrawingEffect(&b, range) };
                    }
                }
            }
            pos += units.len() as u32;
        }
        if let Some(b) = canvas.brush(theme.text, 1.0) {
            unsafe {
                rt.DrawTextLayout(
                    Vector2::new(x, y),
                    &layout,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                )
            };
        }
    }

    // Page tabs, toggle switches, choices and number steps stay real buttons (focus, Space, screen readers); only
    // their look is ours. A Number row's buttons are its steps (its own control is an edit, which draws itself).
    fn custom_draw(&self, cd: &NMCUSTOMDRAW) -> LRESULT {
        let ctls = self.ctls.borrow().clone();
        let stepper = self
            .steppers
            .borrow()
            .iter()
            .find(|s| s.0 == cd.hdr.hwndFrom)
            .map(|s| s.1);
        let Some(i) = stepper.or_else(|| ctls.iter().position(|c| c.1 == cd.hdr.hwndFrom)) else {
            return LRESULT(CDRF_DODEFAULT as isize);
        };
        let r = &self.spec.rows[i];
        if r.kind == Kind::Tree || cd.dwDrawStage != CDDS_PREPAINT {
            return LRESULT(CDRF_DODEFAULT as isize); // the tree is themed by theme_trees
        }
        let btn = cd.hdr.hwndFrom;
        let rc = cd.rc;
        let Ok(mut canvas) = self.canvas.try_borrow_mut() else {
            return LRESULT(CDRF_DODEFAULT as isize);
        };
        if !canvas.begin(cd.hdc, &rc, None) {
            return LRESULT(CDRF_DODEFAULT as isize); // no Direct2D: Windows draws the plain button or checkbox
        }
        let theme = self.theme.get();
        let px = |d| self.px(d) as f32;
        let hot = (cd.uItemState.0 & CDIS_HOT.0) != 0;
        let focus =
            (cd.uItemState.0 & CDIS_FOCUS.0) != 0 && (send(btn, WM_QUERYUISTATE, 0, 0).0 as u32 & UISF_HIDEFOCUS) == 0;
        let full = window_text(btn);
        let (text, hint) = full.split_once('\t').unwrap_or((&full, ""));
        let (left, top, right, bottom) = (rc.left as f32, rc.top as f32, rc.right as f32, rc.bottom as f32);
        let rf = |l, t, r, b| D2D_RECT_F {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let (body, semibold) = {
            let f = self.fonts.borrow();
            (f.text[0].clone(), f.semibold_text.clone())
        };

        if r.kind == Kind::Number {
            // a NumberBox spin button: flat on the field, a subtle fill under the mouse
            canvas.fill(&rect_f(&rc), theme.field, 1.0);
            if hot {
                canvas.round_rect(&rect_f(&rc), px(3), blend(theme.text, theme.field, 0.08), None);
            }
            canvas.text(&full, body.as_ref(), &rect_f(&rc), theme.text2, Align::Center, false);
        } else if r.kind == Kind::Choice {
            // Windows 11 segmented control: one rounded field, the selected segment filled with the accent
            canvas.fill(&rect_f(&rc), theme.card, 1.0);
            let fill = if hot {
                blend(theme.text, theme.field, 0.05)
            } else {
                theme.field
            };
            let edge = if focus { theme.text } else { theme.stroke };
            canvas.round_rect(&rect_f(&rc), px(4), fill, Some(edge));
            let sel = self.choices.borrow().get(&r.id).copied();
            let seg = (right - left) / r.options.len().max(1) as f32;
            for (k, o) in r.options.iter().enumerate() {
                let s = rf(left + seg * k as f32, top, left + seg * (k + 1) as f32, bottom);
                let on = sel == Some(k);
                if on {
                    let inner = rf(s.left + px(2), s.top + px(2), s.right - px(2), s.bottom - px(2));
                    canvas.round_rect(&inner, px(3), theme.accent, None);
                }
                let (format, ink) = if on {
                    (semibold.as_ref(), theme.on_accent)
                } else {
                    (body.as_ref(), theme.text)
                };
                canvas.text(o, format, &s, ink, Align::Center, false);
            }
        } else if r.kind == Kind::Page {
            let selected = self.page_of.borrow()[i] == self.page.get();
            let cx = (left + right) / 2.0;
            canvas.fill(&rect_f(&rc), theme.bg, 1.0);
            if hot && !selected {
                canvas.round_rect(
                    &rf(left + px(2), top + px(2), right - px(2), bottom - px(2)),
                    px(5),
                    theme.card,
                    None,
                );
            }
            if focus {
                let fill = if hot && !selected { theme.card } else { theme.bg };
                canvas.round_rect(
                    &rf(left + 1.0, top + 1.0, right - 1.0, bottom - 1.0),
                    px(5),
                    fill,
                    Some(theme.text),
                );
            }
            if selected {
                canvas.round_rect(
                    &rf(cx - px(8), bottom - px(6), cx + px(8), bottom - px(3)),
                    1.5 * px(1),
                    theme.accent,
                    None,
                );
            }
            let format = if selected { semibold } else { body };
            let ink = if selected || hot { theme.text } else { theme.text2 };
            canvas.text(
                text,
                format.as_ref(),
                &rf(left, top, right, bottom - px(4)),
                ink,
                Align::Center,
                false,
            );
        } else {
            // Check: label left, hint and switch right
            let on = send(btn, BM_GETCHECK, 0, 0).0 as u32 == BST_CHECKED.0;
            canvas.fill(&rect_f(&rc), theme.card, 1.0);
            let (sw, sh) = (px(40), px(20));
            let (sx, sy) = (right - sw - px(4), (top + bottom - sh) / 2.0);
            let s = rf(sx, sy, sx + sw, sy + sh);
            if focus {
                let inset = px(3);
                let ring = rf(s.left - inset, s.top - inset, s.right + inset, s.bottom + inset);
                canvas.round_rect(&ring, sh / 2.0 + inset, theme.card, Some(theme.text));
            }
            if on {
                canvas.round_rect(&s, sh / 2.0, theme.accent, None);
            } else {
                canvas.round_rect(&s, sh / 2.0, theme.card, Some(theme.text2));
            }
            let knob = if on {
                px(if hot { 14 } else { 12 })
            } else {
                px(if hot { 12 } else { 10 })
            };
            let kx = if on { s.right - sh / 2.0 } else { s.left + sh / 2.0 };
            canvas.ellipse(
                Vector2::new(kx, s.top + sh / 2.0),
                knob / 2.0,
                if on { theme.on_accent } else { theme.text2 },
            );
            // the hint right to left from the switch: muted words, `[key]` parts as keycaps
            let end = sx - px(12);
            let mut hx = end;
            for (part, key) in hint_parts(hint).into_iter().rev() {
                let w = self.text_width(part) as f32;
                if key {
                    let (cw, ch) = (w + px(12), px(22));
                    let cap = rf(hx - cw, (top + bottom - ch) / 2.0, hx, (top + bottom + ch) / 2.0);
                    canvas.round_rect(&cap, px(4), theme.field, Some(theme.stroke));
                    canvas.text(part, body.as_ref(), &cap, theme.text, Align::Center, false);
                    hx -= cw + px(5);
                } else {
                    canvas.text(
                        part,
                        body.as_ref(),
                        &rf(left, top, hx, bottom),
                        theme.text2,
                        Align::Right,
                        false,
                    );
                    hx -= w + px(5);
                }
            }
            let label_right = if hx < end { hx - px(3) } else { end };
            canvas.text(
                text,
                body.as_ref(),
                &rf(left, top, label_right, bottom),
                theme.text,
                Align::Left,
                false,
            );
        }
        canvas.end();
        LRESULT(CDRF_SKIPDEFAULT as isize)
    }

    fn handle(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let h = self.hwnd.get();
        let def = || unsafe { DefWindowProcW(h, msg, wp, lp) };
        match msg {
            WM_CREATE => {
                self.dpi.set(unsafe { GetDpiForWindow(h) });
                self.apply_theme();
                // SAFETY: WM_CREATE's lParam is the CREATESTRUCTW.
                let inst = unsafe { (*(lp.0 as *const CREATESTRUCTW)).hInstance };
                self.build(inst);
                self.load_icons();
                self.create_tooltip();
                unsafe { SetTimer(Some(h), 1, self.spec.tick_ms, None) };
                LRESULT(0)
            }
            WM_NCCALCSIZE if wp.0 != 0 => {
                // keep the side and bottom frame (and DWM's shadow and corners), drop the system title bar
                // SAFETY: with wParam set, lParam is the NCCALCSIZE_PARAMS.
                let p = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
                let top = p.rgrc[0].top;
                def();
                p.rgrc[0].top = top;
                LRESULT(0)
            }
            WM_NCHITTEST => {
                // the title bar drags the window (and snaps, and right-click opens the system menu)
                let hit = def();
                let mut p = point_of(lp);
                let _ = unsafe { ScreenToClient(h, &mut p) };
                if hit.0 == HTCLIENT as isize && p.y < self.px(CAPTION_DIP) && self.caption_button_at(p) < 0 {
                    LRESULT(HTCAPTION as isize)
                } else {
                    hit
                }
            }
            WM_ACTIVATE => {
                self.active.set(loword(wp.0) != WA_INACTIVE);
                self.invalidate_caption();
                def()
            }
            WM_MOUSEMOVE => {
                let b = self.caption_button_at(point_of(lp));
                if b != self.cap_hot.get() {
                    self.cap_hot.set(b);
                    self.invalidate_caption();
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    if b >= 0 {
                        let _ = unsafe { TrackMouseEvent(&mut t) };
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // over a Number box (the field and its buttons pass the wheel up): step it
                let mut p = point_of(lp);
                let _ = unsafe { ScreenToClient(h, &mut p) };
                let page_of = self.page_of.borrow().clone();
                let at = self.boxes.borrow().iter().enumerate().position(|(i, b)| {
                    self.spec.rows[i].kind == Kind::Number
                        && page_of[i] == self.page.get()
                        && unsafe { PtInRect(b, p) }.as_bool()
                });
                match at {
                    Some(i) => {
                        self.step(i, if hiword(wp.0) as i16 > 0 { 1 } else { -1 });
                        LRESULT(0)
                    }
                    None => def(),
                }
            }
            WM_MOUSELEAVE => {
                self.cap_hot.set(-1);
                self.invalidate_caption();
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let b = self.caption_button_at(point_of(lp));
                self.cap_down.set(b);
                if b >= 0 {
                    unsafe { SetCapture(h) };
                    self.invalidate_caption();
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let b = self.cap_down.replace(-1);
                if b >= 0 {
                    // a click: pressed and released on the same button
                    let _ = unsafe { ReleaseCapture() };
                    self.invalidate_caption();
                    if self.caption_button_at(point_of(lp)) == b {
                        if b == BADGE {
                            let f = self.on_badge_click.borrow().clone();
                            if let Some(f) = f {
                                f(self);
                            }
                        } else {
                            let cmd = if b == CLOSE { SC_CLOSE } else { SC_MINIMIZE };
                            send(h, WM_SYSCOMMAND, cmd as usize, 0);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                if self.cap_down.get() >= 0 {
                    self.cap_down.set(-1);
                    self.invalidate_caption();
                }
                def()
            }
            WM_TIMER => {
                self.tick();
                LRESULT(0)
            }
            WM_COMMAND => {
                self.command(loword(wp.0) as i32, hiword(wp.0), HWND(lp.0 as *mut _));
                LRESULT(0)
            }
            WM_NOTIFY => {
                // SAFETY: WM_NOTIFY's lParam is an NMHDR at the head of the notification's own struct.
                let n = unsafe { &*(lp.0 as *const NMHDR) };
                if n.code == NM_CUSTOMDRAW {
                    return self.custom_draw(unsafe { &*(lp.0 as *const NMCUSTOMDRAW) });
                }
                if n.code == TVN_ITEMCHANGINGW && !self.setting_tree.get() {
                    // A click or Space on a checkbox: ask the game and keep the box as it is (the game calls set_tree).
                    let c = unsafe { &*(lp.0 as *const NMTVITEMCHANGE) };
                    let check_change = (c.uChanged & TVIF_STATE.0) != 0
                        && ((c.uStateNew ^ c.uStateOld) & TVIS_STATEIMAGEMASK.0) != 0
                        && (c.uStateOld & TVIS_STATEIMAGEMASK.0) != 0; // no old image = the item is being set up
                    if !check_change {
                        return def();
                    }
                    let id = n.idFrom as i32;
                    let index = self
                        .trees
                        .borrow()
                        .get(&id)
                        .and_then(|t| t.index.get(&c.hItem.0).copied());
                    let f = self.on_tree_check.borrow().clone();
                    if let (Some(i), Some(f)) = (index, f) {
                        f(self, id, i, checked_state(c.uStateNew));
                    }
                    return LRESULT(1); // blocks the change
                }
                def()
            }
            WM_DRAWITEM => {
                // SAFETY: WM_DRAWITEM's lParam is the DRAWITEMSTRUCT.
                let di = unsafe { &*(lp.0 as *const DRAWITEMSTRUCT) };
                if di.hwndItem == self.header.get() {
                    self.draw_header(di);
                    return LRESULT(1);
                }
                let ctls = self.ctls.borrow().clone();
                for (r, c) in self.spec.rows.iter().zip(&ctls) {
                    if r.rich && di.hwndItem == c.1 {
                        let spans = self.spans.borrow().get(&r.id).cloned().unwrap_or_default();
                        if r.kind == Kind::Tile {
                            self.draw_tile(di, r, &spans);
                        } else {
                            self.draw_spans(di, &spans);
                        }
                        return LRESULT(1);
                    }
                }
                def()
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as *mut _);
                let theme = self.theme.get();
                let [bg, card, field] = self.brushes.get();
                unsafe {
                    if msg == WM_CTLCOLOREDIT {
                        SetTextColor(dc, theme.text);
                        SetBkColor(dc, theme.field);
                        return LRESULT(field.0 as isize);
                    }
                    let look = self.looks.borrow().get(&lp.0).copied().unwrap_or(Look {
                        role: Role::Text,
                        on_card: false,
                    });
                    SetTextColor(
                        dc,
                        match look.role {
                            Role::Muted => theme.text2,
                            Role::Warn => theme.busy,
                            Role::Text => theme.text,
                        },
                    );
                    SetBkColor(dc, if look.on_card { theme.card } else { theme.bg });
                    LRESULT(if look.on_card { card.0 } else { bg.0 } as isize)
                }
            }
            // paint fills all it repaints (its GDI fallback too): erasing first would only paint it twice
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR if loword(lp.0 as usize) == HTCLIENT && self.on_badge_click.borrow().is_some() => {
                let mut p = POINT::default();
                let _ = unsafe { GetCursorPos(&mut p) };
                let _ = unsafe { ScreenToClient(h, &mut p) };
                if self.caption_button_at(p) == BADGE {
                    unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
                    LRESULT(1)
                } else {
                    def()
                }
            }
            WM_PAINT => {
                self.sync_tip(false); // the badge moves with the title, its text and the DPI
                let mut ps = PAINTSTRUCT::default();
                let dc = unsafe { BeginPaint(h, &mut ps) };
                self.paint(dc, &ps.rcPaint);
                let _ = unsafe { EndPaint(h, &ps) };
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                // light/dark or accent
                if lp.0 != 0
                    && unsafe { PCWSTR(lp.0 as *const u16).to_string() }.is_ok_and(|s| s == "ImmersiveColorSet")
                {
                    self.apply_theme();
                }
                def()
            }
            WM_DWMCOLORIZATIONCOLORCHANGED => {
                self.apply_theme();
                def()
            }
            WM_DPICHANGED => {
                self.dpi.set(hiword(wp.0));
                self.apply_fonts();
                self.load_icons();
                // SAFETY: WM_DPICHANGED's lParam is the suggested window RECT.
                let suggested = unsafe { *(lp.0 as *const RECT) };
                self.layout(Some(&suggested));
                LRESULT(0)
            }
            WM_CLOSE => {
                let f = self.on_close.borrow().clone();
                if f.is_none_or(|f| f(self)) {
                    let _ = unsafe { DestroyWindow(h) };
                }
                LRESULT(0)
            }
            WM_ENDSESSION => {
                let f = self.on_end_session.borrow().clone();
                if wp.0 != 0
                    && let Some(f) = f
                {
                    f();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = unsafe { KillTimer(Some(h), 1) };
                if self.owns_loop.get() {
                    unsafe { PostQuitMessage(0) };
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                unsafe { SetWindowLongPtrW(h, GWLP_USERDATA, 0) };
                self.hwnd.set(HWND::default());
                self.header.set(HWND::default());
                def()
            }
            _ => def(),
        }
    }

    fn command(&self, id: i32, code: u32, from: HWND) {
        let stepper = self.steppers.borrow().iter().find(|s| s.0 == from).copied();
        if let Some((_, i, step)) = stepper
            && code == BN_CLICKED
        {
            return self.step(i, step);
        }
        let ctls = self.ctls.borrow().clone();
        for (i, r) in self.spec.rows.iter().enumerate() {
            // Only the row's own control: IsDialogMessage sends Enter/Esc as IDOK/IDCANCEL (ids 1/2) with no hwnd.
            if r.id != id || from != ctls[i].1 {
                continue;
            }
            let h = self.hwnd.get();
            let invalidate_box = || {
                let b = self.boxes.borrow()[i];
                let _ = unsafe { InvalidateRect(Some(h), Some(&b), true) }; // accent line
            };
            match r.kind {
                Kind::Page if code == BN_CLICKED => self.show_page(self.page_of.borrow()[i]),
                Kind::Check if code == BN_CLICKED => {
                    let f = self.on_check.borrow().clone();
                    if let Some(f) = f {
                        f(self, id, unsafe { IsDlgButtonChecked(h, id) } != BST_CHECKED.0);
                        if !self.hwnd.get().is_invalid() {
                            self.tick(); // show the new state now; on_check may have closed the window
                        }
                    }
                }
                Kind::Choice if code == BN_CLICKED => {
                    // the segment under the mouse; Space (mouse elsewhere) moves to the next option
                    let n = r.options.len();
                    let (mut p, mut rc) = (POINT::default(), RECT::default());
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                        let _ = ScreenToClient(from, &mut p);
                        let _ = GetClientRect(from, &mut rc);
                    }
                    let k = if unsafe { PtInRect(&rc, p) }.as_bool() {
                        segment_at(rc.right, n, p.x)
                    } else {
                        self.choices.borrow().get(&id).map_or(0, |&k| (k + 1) % n.max(1))
                    };
                    self.pick(i, k);
                }
                Kind::Number => {
                    if code == EN_CHANGE || code == EN_KILLFOCUS {
                        self.on_edit(r, code == EN_KILLFOCUS);
                    }
                    if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                        invalidate_box();
                    }
                }
                Kind::Text => {
                    let f = self.on_text.borrow().clone();
                    if code == EN_CHANGE
                        && let Some(f) = f
                    {
                        f(self, id, &window_text(ctls[i].1));
                    }
                    if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                        invalidate_box();
                    }
                }
                _ => {}
            }
            break;
        }
    }
}

impl Drop for Dashboard<'_> {
    fn drop(&mut self) {
        let h = self.hwnd.get();
        if !h.is_invalid() {
            unsafe {
                // the window stops reaching this first: nothing runs on a dashboard being dropped
                SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                let _ = DestroyWindow(h);
            }
        }
        let f = self.fonts.get_mut();
        for o in f.faces.into_iter().chain([f.semibold, f.big]) {
            if !o.is_invalid() {
                let _ = unsafe { DeleteObject(o.into()) };
            }
        }
        for b in self.brushes.get() {
            if !b.is_invalid() {
                let _ = unsafe { DeleteObject(b.into()) };
            }
        }
        for i in [self.icon.get(), self.big_icon.get()] {
            if !i.is_invalid() {
                let _ = unsafe { DestroyIcon(i) };
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let d = (*(lp.0 as *const CREATESTRUCTW)).lpCreateParams as *const Dashboard<'static>;
            (*d).hwnd.set(h);
            SetWindowLongPtrW(h, GWLP_USERDATA, d as isize);
        }
        // SAFETY: set from the pinned Dashboard, which clears it before it goes (Drop, WM_NCDESTROY).
        match (GetWindowLongPtrW(h, GWLP_USERDATA) as *const Dashboard<'static>).as_ref() {
            Some(d) => d.handle(msg, wp, lp),
            None => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dwm::DwmFlush;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        GetObjectW, RDW_UPDATENOW, UpdateWindow,
    };
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    use windows::Win32::UI::Controls::{TTM_GETTEXTW, TTM_GETTOOLCOUNT};
    use windows::Win32::UI::Controls::{
        TVE_COLLAPSE, TVGN_CHILD, TVGN_NEXT, TVGN_ROOT, TVIS_EXPANDED, TVM_GETCOUNT, TVM_GETITEMSTATE, TVM_GETITEMW,
        TVM_GETNEXTITEM,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowRect, IsWindow, PM_NOREMOVE, PW_RENDERFULLCONTENT, PeekMessageW, SW_SHOWMINNOACTIVE,
        SW_SHOWNOACTIVATE, WM_GETFONT, WM_QUIT,
    };

    fn spec(rows: Vec<Row>) -> Spec {
        Spec {
            title: "t".into(),
            window_class: "handy-sdk-test-dashboard".into(),
            rows,
            ..Default::default()
        }
    }
    fn item(h: HWND, id: i32) -> HWND {
        unsafe { GetDlgItem(Some(h), id) }.unwrap_or_default()
    }
    fn text_of(h: HWND, id: i32) -> String {
        let mut buf = [0u16; 256];
        unsafe { GetDlgItemTextW(h, id, &mut buf) };
        from_wide(&buf)
    }
    fn command(h: HWND, id: i32, code: u32) {
        send(
            h,
            WM_COMMAND,
            (code as usize) << 16 | id as usize,
            item(h, id).0 as isize,
        );
    }
    fn make_lparam(x: i32, y: i32) -> isize {
        ((y as u16 as u32) << 16 | x as u16 as u32) as isize
    }

    #[test]
    fn dashboard_set_text_and_check() {
        let checks = RefCell::new(Vec::new());
        let d = Dashboard::new(spec(vec![value(1, "Status"), check(2, "On")]));
        d.on_check(|_, id, on| checks.borrow_mut().push((id, on)));
        assert!(d.create());
        assert!(!unsafe { IsWindowVisible(d.hwnd()) }.as_bool());
        d.set_text(1, "Ready");
        assert_eq!(text_of(d.hwnd(), 1), "Ready");
        d.set_text(1, "caf\u{e9}");
        assert_eq!(text_of(d.hwnd(), 1), "caf\u{e9}");
        assert_eq!(text_of(d.hwnd(), 2), "On");
        d.set_check(2, true);
        assert_eq!(unsafe { IsDlgButtonChecked(d.hwnd(), 2) }, BST_CHECKED.0);
        // the box asks the game for the opposite state and leaves itself alone
        command(d.hwnd(), 2, BN_CLICKED);
        assert_eq!(*checks.borrow(), [(2, false)]);
        assert_eq!(unsafe { IsDlgButtonChecked(d.hwnd(), 2) }, BST_CHECKED.0);
    }

    #[test]
    fn dashboard_draws_its_own_title_bar() {
        let closes = Cell::new(0);
        let d = Dashboard::new(spec(vec![value(1, "Status")]));
        d.on_close(|_| {
            closes.set(closes.get() + 1);
            false
        });
        assert!(d.create());
        let h = d.hwnd();
        let (mut win, mut client, mut origin) = (RECT::default(), RECT::default(), POINT::default());
        unsafe {
            GetWindowRect(h, &mut win).unwrap();
            GetClientRect(h, &mut client).unwrap();
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin);
        }
        assert_eq!(origin.y, win.top); // no system title bar: the client area starts at the window's top edge
        let dpi = unsafe { GetDpiForWindow(h) } as i32;
        let dip = |v: i32| unsafe { MulDiv(v, dpi, 96) };
        let hit = |x: i32, y: i32| {
            let mut p = POINT { x, y };
            let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut p) };
            send(h, WM_NCHITTEST, 0, make_lparam(p.x, p.y)).0
        };
        assert_eq!(hit(dip(80), dip(16)), HTCAPTION as isize); // the bar drags the window
        assert_eq!(hit(client.right - dip(20), dip(16)), HTCLIENT as isize); // close button: ours to click
        assert_eq!(hit(client.right - dip(70), dip(16)), HTCLIENT as isize); // minimize button
        assert_eq!(hit(dip(80), dip(40)), HTCLIENT as isize); // under the bar
        // press and release on close: the window's normal close path (on_close decides)
        let close = make_lparam(client.right - dip(20), dip(16));
        send(h, WM_LBUTTONDOWN, 1, close);
        send(h, WM_LBUTTONUP, 0, close);
        assert_eq!(closes.get(), 1);
        // pressed on close but released elsewhere: nothing
        send(h, WM_LBUTTONDOWN, 1, close);
        send(h, WM_LBUTTONUP, 0, make_lparam(dip(80), dip(16)));
        assert_eq!(closes.get(), 1);
    }

    #[test]
    fn dashboard_badge_sits_after_the_title() {
        let mut s = spec(vec![value(1, "Status")]);
        s.badge = "v1.2.3".into();
        let d = Dashboard::new(s);
        assert!(d.create());
        let cap = d.px(CAPTION_DIP);
        let (pill, text) = d.badge_rects(d.px(38), cap, "t").expect("fits");
        assert!(pill.left > d.px(38) + d.text_width("t")); // a gap after the title
        assert!(pill.top > 0 && pill.bottom < cap); // inside the caption, centered
        assert_eq!(pill.bottom - pill.top, d.px(18));
        assert!(text.left > pill.left && text.right == pill.right);
        assert!(pill.right < d.caption_button(MINIMIZE).left); // clear of the buttons
        assert!(d.badge_rects(d.px(38), cap, &"x".repeat(500)).is_none()); // no room after a huge title: none
        let plain = Dashboard::new(spec(vec![value(1, "Status")]));
        assert!(plain.create());
        assert!(plain.badge_rects(plain.px(38), cap, "t").is_none()); // no badge set

        // set_badge: the pill grows with the text, and an empty text hides it
        d.set_badge("v1.2.3 \u{b7} downloading 42%", Tone::Busy, "Downloading");
        let (wider, _) = d.badge_rects(d.px(38), cap, "t").expect("fits");
        assert!(wider.right > pill.right && wider.left == pill.left);
        d.set_badge("", Tone::Neutral, "");
        assert!(d.badge_pill().is_none());
        plain.set_badge("v1", Tone::Good, "tip");
        assert!(plain.badge_pill().is_some());
    }

    #[test]
    fn dashboard_badge_is_clickable() {
        let clicks = Cell::new(0);
        let mut s = spec(vec![value(1, "Status")]);
        s.badge = "v1.2.3 \u{b7} up-to-date".into();
        let d = Dashboard::new(s);
        d.on_badge_click(|_| clicks.set(clicks.get() + 1));
        assert!(d.create());
        let h = d.hwnd();
        let pill = d.badge_pill().expect("shown");
        let hit = |x: i32, y: i32| {
            let mut p = POINT { x, y };
            let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut p) };
            send(h, WM_NCHITTEST, 0, make_lparam(p.x, p.y)).0
        };
        let (cx, cy) = ((pill.left + pill.right) / 2, (pill.top + pill.bottom) / 2);
        assert_eq!(hit(cx, cy), HTCLIENT as isize); // ours to click, not a drag area
        assert_eq!(hit(pill.right + d.px(10), cy), HTCAPTION as isize); // the rest of the bar still drags
        assert_eq!(hit(pill.left - d.px(3), cy), HTCAPTION as isize);
        let on = make_lparam(cx, cy);
        send(h, WM_LBUTTONDOWN, 1, on);
        send(h, WM_LBUTTONUP, 0, on);
        assert_eq!(clicks.get(), 1);
        // pressed on the badge, released elsewhere: nothing
        send(h, WM_LBUTTONDOWN, 1, on);
        send(h, WM_LBUTTONUP, 0, make_lparam(pill.right + d.px(10), cy));
        assert_eq!(clicks.get(), 1);
        // the tooltip has two tools: the badge and the status header
        assert_eq!(send(d.tooltip.get(), TTM_GETTOOLCOUNT, 0, 0).0, 2);
        d.set_badge("v1.2.3 \u{b7} v1.2.4 ready", Tone::Busy, "Click to restart");
        assert_eq!(d.tip_rect.get(), d.badge_pill().unwrap());
    }

    #[test]
    fn dashboard_enter_esc_do_not_toggle_check() {
        let checks = Cell::new(0);
        let d = Dashboard::new(spec(vec![check(1, "A"), check(2, "On")]));
        d.on_check(|_, _, _| checks.set(checks.get() + 1));
        assert!(d.create());
        // a command whose hwnd isn't the row's own control is ignored
        send(d.hwnd(), WM_COMMAND, 1 | (BN_CLICKED as usize) << 16, 0); // IDOK
        send(d.hwnd(), WM_COMMAND, 2 | (BN_CLICKED as usize) << 16, 0); // IDCANCEL
        assert_eq!(checks.get(), 0);
        // the real path: IsDialogMessage turns Enter/Esc into IDOK/IDCANCEL carrying control 1/2's own hwnd
        for key in [VK_ESCAPE, VK_RETURN] {
            for m in [WM_KEYDOWN, WM_CHAR, WM_KEYUP] {
                let msg = MSG {
                    hwnd: item(d.hwnd(), 2),
                    message: m,
                    wParam: WPARAM(key.0 as usize),
                    ..Default::default()
                };
                d.dispatch(&msg);
            }
        }
        assert_eq!(checks.get(), 0);
        command(d.hwnd(), 2, BN_CLICKED); // a real click still reaches the game
        assert_eq!(checks.get(), 1);
    }

    #[test]
    fn dashboard_number_rejects_and_restores() {
        let numbers = RefCell::new(Vec::new());
        let d = Dashboard::new(Spec {
            hint_row: Some(99),
            ..spec(vec![section("Timings"), number(10, "Latency", 0, 100), note(99)])
        });
        d.on_number(|_, id, v| numbers.borrow_mut().push((id, v)));
        assert!(d.create());
        let h = d.hwnd();
        let set = |s: &str| unsafe { SetDlgItemTextW(h, 10, &HSTRING::from(s)) }.unwrap();
        d.set_number(10, 12);
        assert_eq!(text_of(h, 10), "12");
        assert!(numbers.borrow().is_empty()); // set_number does not echo back
        set("500");
        command(h, 10, EN_CHANGE);
        assert!(numbers.borrow().is_empty());
        assert!(text_of(h, 99).starts_with("Enter a whole number from 0 to 100"));
        assert_eq!(text_of(h, 99), "Enter a whole number from 0 to 100 (not saved).");
        command(h, 10, EN_KILLFOCUS);
        assert_eq!(text_of(h, 10), "12");
        assert!(text_of(h, 99).is_empty());
        assert!(numbers.borrow().is_empty()); // restoring the last value is not a change
        set("42");
        command(h, 10, EN_CHANGE);
        assert_eq!(*numbers.borrow(), [(10, 42)]);
        set("12"); // differs from the last applied value (42), so it fires
        assert_eq!(*numbers.borrow(), [(10, 42), (10, 12)]);
    }

    #[test]
    fn dashboard_number_without_hint_row() {
        let d = Dashboard::new(spec(vec![number(10, "Latency", 0, 100)]));
        assert!(d.create());
        unsafe { SetDlgItemTextW(d.hwnd(), 10, w!("500")) }.unwrap(); // no hint row: nothing to show, nothing to crash
        command(d.hwnd(), 10, EN_KILLFOCUS);
        assert!(text_of(d.hwnd(), 10).is_empty()); // no valid value yet: leaving clears the field
    }

    #[test]
    fn dashboard_close_can_be_deferred() {
        let allow = Cell::new(false);
        let asked = Cell::new(0);
        let d = Dashboard::new(spec(vec![value(1, "Status")]));
        d.on_close(|_| {
            asked.set(asked.get() + 1);
            allow.get()
        });
        assert!(d.create());
        let h = d.hwnd();
        send(h, WM_CLOSE, 0, 0);
        assert!(asked.get() == 1 && unsafe { IsWindow(Some(h)) }.as_bool());
        d.set_title("closing");
        assert_eq!(window_text(h), "closing");
        d.close();
        assert!(!unsafe { IsWindow(Some(h)) }.as_bool() && d.hwnd().is_invalid());
        let mut m = MSG::default();
        // create() does not own the loop
        assert!(!unsafe { PeekMessageW(&mut m, None, WM_QUIT, WM_QUIT, PM_NOREMOVE) }.as_bool());
    }

    #[test]
    fn dashboard_pages_show_one_at_a_time() {
        let d = Dashboard::new(spec(vec![
            page("Overview"),
            value(1, "Status"),
            page("Settings"),
            section("Timings"),
            number(10, "Latency", 0, 100),
        ]));
        assert!(d.create());
        let h = d.hwnd();
        let visible = |id| unsafe { IsWindowVisible(item(h, id)) }.as_bool();
        let _ = unsafe { ShowWindow(h, SW_SHOWNA) }; // child visibility only counts under a visible parent
        assert_eq!(d.page(), 0);
        assert!(visible(1) && !visible(10));
        d.show_page(1);
        assert!(!visible(1) && visible(10));
        for shift in [false, true] {
            // Ctrl+Tab cycles forward, Ctrl+Shift+Tab back; both wrap
            let mut keys = [0u8; 256];
            unsafe { GetKeyboardState(&mut keys) }.unwrap();
            keys[VK_CONTROL.0 as usize] = 0x80;
            keys[VK_SHIFT.0 as usize] = if shift { 0x80 } else { 0 };
            unsafe { SetKeyboardState(&keys) }.unwrap();
            let msg = MSG {
                hwnd: item(h, 10),
                message: WM_KEYDOWN,
                wParam: WPARAM(VK_TAB.0 as usize),
                ..Default::default()
            };
            d.dispatch(&msg);
            keys[VK_CONTROL.0 as usize] = 0;
            keys[VK_SHIFT.0 as usize] = 0;
            unsafe { SetKeyboardState(&keys) }.unwrap();
            assert_eq!(d.page(), if shift { 1 } else { 0 });
        }
        d.show_page(7); // out of range: ignored
        assert_eq!(d.page(), 1);
    }

    #[test]
    fn dashboard_single_page_has_no_tabs() {
        let d = Dashboard::new(spec(vec![page("Only"), value(1, "Status")]));
        assert!(d.create());
        let _ = unsafe { ShowWindow(d.hwnd(), SW_SHOWNA) };
        let tab = unsafe { FindWindowExW(Some(d.hwnd()), None, w!("BUTTON"), w!("Only")) }.unwrap_or_default();
        assert!(!tab.is_invalid() && !unsafe { IsWindowVisible(tab) }.as_bool());
        assert!(unsafe { IsWindowVisible(item(d.hwnd(), 1)) }.as_bool());
    }

    #[test]
    fn dashboard_status_header_text() {
        let d = Dashboard::new(spec(vec![value(1, "Status")]));
        assert!(d.create());
        d.set_status(Tone::Good, "Ready", "version-abc");
        let header = unsafe { FindWindowExW(Some(d.hwnd()), None, w!("STATIC"), PCWSTR::null()) }.unwrap(); // first child
        assert_eq!(window_text(header), "Ready - version-abc"); // what a screen reader reads
    }

    #[test]
    fn dashboard_value_picture() {
        let d = Dashboard::new(spec(vec![value(1, "Plain"), value(2, "Fish").lines(2).picture(96)]));
        assert!(d.create());
        let (mut plain, mut fish) = (RECT::default(), RECT::default());
        unsafe {
            GetWindowRect(item(d.hwnd(), 1), &mut plain).unwrap();
            GetWindowRect(item(d.hwnd(), 2), &mut fish).unwrap();
        }
        assert!(fish.right - fish.left < plain.right - plain.left); // the text leaves the slot free
        // a 1x1 24-bit BMP: file header, info header, one pixel padded to 4 bytes
        let bmp = std::env::temp_dir().join(format!("handy-sdk-test-picture-{}.bmp", std::process::id()));
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&(14u32 + 40 + 4).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(14u32 + 40).to_le_bytes());
        for v in [40u32, 1, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0; 24]);
        b.extend_from_slice(b"\x20\x40\x60\x00");
        std::fs::write(&bmp, &b).unwrap();
        assert!(d.set_picture(2, &bmp, None));
        assert!(d.set_picture(2, &bmp, None)); // same file: kept
        assert!(d.set_picture(2, &bmp, Some(0x0c0fd4))); // recolored: read again
        assert!(std::fs::remove_file(&bmp).is_ok()); // no lock left on it
        assert!(!d.set_picture(2, Path::new(""), None)); // cleared
        assert!(!d.set_picture(2, &bmp, None)); // gone now
        let (w, h) = d.picture_size(2).unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(d.picture_size(1), None); // no slot
        std::fs::write(&bmp, &b).unwrap(); // back, but the map still says (bmp, None): only the pixels can unstick it
        d.set_picture_pixels(2, w, h, &vec![0xff; (w * h * 4) as usize]);
        assert!(d.pictures.borrow()[&2].0 == Path::new("<pixels>"));
        assert!(d.set_picture(2, &bmp, None)); // same file as before the pixels: read again, not "unchanged"
        assert!(d.pictures.borrow()[&2].0 == bmp);
        let _ = std::fs::remove_file(&bmp);
    }

    #[test]
    fn dashboard_picture_visible() {
        let d = Dashboard::new(spec(vec![
            page("Main"),
            value(1, "Fish").picture(96),
            page("Settings"),
            value(2, "Other").picture(96),
        ]));
        assert!(d.create());
        assert!(!d.picture_visible(1)); // the window isn't shown yet
        let _ = unsafe { ShowWindow(d.hwnd(), SW_SHOWNA) };
        assert!(d.picture_visible(1) && !d.picture_visible(2));
        d.show_page(1);
        assert!(!d.picture_visible(1) && d.picture_visible(2));
        assert!(!d.picture_visible(3)); // no such row
        let _ = unsafe { ShowWindow(d.hwnd(), SW_SHOWMINNOACTIVE) };
        assert!(!d.picture_visible(2));
    }

    #[test]
    fn dashboard_rich_value_spans() {
        let d = Dashboard::new(spec(vec![rich_value(1, "Fish").lines(2)]));
        d.set_font_family("Arial"); // before create: kept for the fonts create makes
        assert!(d.create());
        d.set_spans(
            1,
            &[
                Span {
                    text: "Carrot ".into(),
                    colors: vec![rgb(255, 162, 0)],
                    bold: true,
                    ..Default::default()
                },
                Span {
                    text: "Mackerel".into(),
                    colors: vec![rgb(142, 187, 191), rgb(0, 0, 255)],
                    ..Default::default()
                },
                Span {
                    text: "\n3.9 kg".into(),
                    bold: true,
                    italic: true,
                    stroke: rgb(0, 0, 0),
                    stroke_opacity: 0.42,
                    ..Default::default()
                }, // outlined
            ],
        );
        assert_eq!(text_of(d.hwnd(), 1), "Carrot Mackerel\n3.9 kg"); // the plain text, for screen readers
        d.set_text(1, "plain"); // one span in the theme's color
        assert_eq!(text_of(d.hwnd(), 1), "plain");
        let face = || {
            // the font the window's controls were given
            let font = send(item(d.hwnd(), 1), WM_GETFONT, 0, 0).0;
            let mut lf = LOGFONTW::default();
            unsafe {
                GetObjectW(
                    HFONT(font as *mut _).into(),
                    size_of::<LOGFONTW>() as i32,
                    Some((&raw mut lf).cast()),
                )
            };
            from_wide(&lf.lfFaceName)
        };
        assert_eq!(face(), "Arial");
        d.set_font_family(""); // back to the system's, re-laid out live
        assert_ne!(face(), "Arial");
        let _ = unsafe { UpdateWindow(d.hwnd()) };
    }

    fn ti(key: &str, parent: Option<usize>, text: &str) -> TreeItem {
        TreeItem {
            key: key.into(),
            parent,
            text: text.into(),
            ..Default::default()
        }
    }
    fn next(tv: HWND, how: u32, h: HTREEITEM) -> HTREEITEM {
        HTREEITEM(send(tv, TVM_GETNEXTITEM, how as usize, h.0).0)
    }
    fn tree_text(tv: HWND, h: HTREEITEM) -> String {
        let mut buf = [0u16; 64];
        let t = TVITEMW {
            mask: TVIF_TEXT,
            hItem: h,
            pszText: windows::core::PWSTR(buf.as_mut_ptr()),
            cchTextMax: 64,
            ..Default::default()
        };
        send(tv, TVM_GETITEMW, 0, &raw const t as isize);
        from_wide(&buf)
    }
    fn state(tv: HWND, h: HTREEITEM, mask: u32) -> u32 {
        send(tv, TVM_GETITEMSTATE, h.0 as usize, mask as isize).0 as u32
    }
    fn image(tv: HWND, h: HTREEITEM) -> u32 {
        (state(tv, h, TVIS_STATEIMAGEMASK.0) >> 12) & 0xF
    }
    // TreeView_SetCheckState: what a click or Space on the box does
    fn user_check(tv: HWND, h: HTREEITEM, on: bool) {
        set_item_state(tv, h, if on { 2 << 12 } else { 1 << 12 }, TVIS_STATEIMAGEMASK.0);
    }

    #[test]
    fn dashboard_tree_set_update_and_user_check() {
        let asked = RefCell::new(Vec::new());
        let d = Dashboard::new(spec(vec![tree(6, 6)]));
        d.on_tree_check(|_, id, i, on| {
            assert_eq!(id, 6);
            asked.borrow_mut().push((i, on));
        });
        assert!(d.create());
        let tv = item(d.hwnd(), 6);
        // Workspace (no box) > Map > {Coin, Gem}
        d.set_tree(
            6,
            &[
                TreeItem {
                    checkbox: false,
                    open: true,
                    ..ti("W", None, "Workspace")
                },
                ti("W/Map", Some(0), "Map"),
                TreeItem {
                    checked: true,
                    ..ti("W/Map/Coin", Some(1), "Coin")
                },
                ti("W/Map/Gem", Some(1), "Gem"),
            ],
        );
        assert_eq!(send(tv, TVM_GETCOUNT, 0, 0).0, 4);
        let root = next(tv, TVGN_ROOT, HTREEITEM(0));
        let map = next(tv, TVGN_CHILD, root);
        let coin = next(tv, TVGN_CHILD, map);
        let gem = next(tv, TVGN_NEXT, coin);
        assert_eq!(
            [
                tree_text(tv, root),
                tree_text(tv, map),
                tree_text(tv, coin),
                tree_text(tv, gem)
            ],
            ["Workspace", "Map", "Coin", "Gem"]
        );
        assert!(image(tv, root) == 0 && image(tv, map) == 1 && image(tv, coin) == 2);
        assert_ne!(state(tv, root, TVIS_EXPANDED.0) & TVIS_EXPANDED.0, 0); // open
        assert!(asked.borrow().is_empty());

        // a click or Space: the game is asked, the box stays
        user_check(tv, gem, true);
        assert_eq!(*asked.borrow(), [(3, true)]);
        assert_eq!(image(tv, gem), 1);

        // Coin goes, Key arrives between Gem's neighbours, Map's text changes; kept items keep their handles
        d.set_tree(
            6,
            &[
                TreeItem {
                    checkbox: false,
                    open: true,
                    ..ti("W", None, "Workspace")
                },
                ti("W/Map", Some(0), "Map x2"),
                TreeItem {
                    checked: true,
                    ..ti("W/Map/Gem", Some(1), "Gem")
                },
                ti("W/Map/Key", Some(1), "Key"),
            ],
        );
        assert_eq!(send(tv, TVM_GETCOUNT, 0, 0).0, 4);
        assert!(next(tv, TVGN_CHILD, map) == gem && tree_text(tv, map) == "Map x2" && image(tv, gem) == 2);
        assert_eq!(tree_text(tv, next(tv, TVGN_NEXT, gem)), "Key");
        user_check(tv, next(tv, TVGN_NEXT, gem), true);
        assert_eq!(*asked.borrow(), [(3, true), (3, true)]); // the index in the latest set_tree
    }

    #[test]
    fn dashboard_text_reports_each_change() {
        let got = RefCell::new(Vec::new());
        let d = Dashboard::new(spec(vec![text(7, "Find")]));
        d.on_text(|_, id, s| {
            assert_eq!(id, 7);
            got.borrow_mut().push(s.to_owned());
        });
        assert!(d.create());
        unsafe {
            SetDlgItemTextW(d.hwnd(), 7, w!("coin")).unwrap(); // sends EN_CHANGE, as typing does
            SetDlgItemTextW(d.hwnd(), 7, w!("")).unwrap();
        }
        assert_eq!(*got.borrow(), ["coin", ""]);
    }

    #[test]
    fn dashboard_tree_opens_an_item_whose_open_turns_true() {
        let d = Dashboard::new(spec(vec![tree(6, 6)]));
        assert!(d.create());
        let tv = item(d.hwnd(), 6);
        let root = || next(tv, TVGN_ROOT, HTREEITEM(0));
        let open = || state(tv, root(), TVIS_EXPANDED.0) & TVIS_EXPANDED.0 != 0;
        d.set_tree(6, &[ti("Map", None, "Map"), ti("Map/Coin", Some(0), "Coin")]);
        assert!(!open());
        let opened = [
            TreeItem {
                open: true,
                ..ti("Map", None, "Map")
            },
            ti("Map/Coin", Some(0), "Coin"),
        ];
        d.set_tree(6, &opened);
        assert!(open());
        // the user closes it: a still-true open leaves it closed
        send(tv, TVM_EXPAND, TVE_COLLAPSE.0 as usize, root().0);
        d.set_tree(6, &opened);
        assert!(!open());
    }

    #[test]
    fn dashboard_draws_with_direct2d() {
        // Shown for a moment and read back: the card and an "on" switch's track come out in the theme's exact colors.
        let d = Dashboard::new(spec(vec![section("Game"), check(1, "On\tF1"), value(2, "Status")]));
        assert!(d.create());
        d.set_check(1, true);
        let h = d.hwnd();
        let mut rc = RECT::default();
        let (mut card, mut accent) = (0, 0);
        unsafe {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            let _ = RedrawWindow(
                Some(h),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
            let _ = DwmFlush();
            GetWindowRect(h, &mut rc).unwrap();
            let (w, ht) = (rc.right - rc.left, rc.bottom - rc.top);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -ht,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(mem, bmp.into());
            assert!(PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool());
            let t = system_theme();
            let px = std::slice::from_raw_parts(bits.cast::<u8>(), (w * ht * 4) as usize);
            for p in px.as_chunks::<4>().0 {
                let c = rgb(p[2], p[1], p[0]); // BGRA
                card += i32::from(c == t.card);
                accent += i32::from(c == t.accent);
            }
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
        assert!(card > 1000, "{card}"); // the card behind the rows
        assert!(accent > 50, "{accent}"); // the switch's track
    }

    #[test]
    fn dashboard_status_tip_and_header_gap() {
        let d = Dashboard::new(spec(vec![value(1, "Status")]));
        assert!(d.create());
        let header_h = || {
            let mut r = RECT::default();
            unsafe { GetWindowRect(d.header.get(), &mut r) }.unwrap();
            r.bottom - r.top
        };
        d.set_status(Tone::Good, "Ready", "Attached");
        let with_detail = header_h();
        d.set_status(Tone::Good, "Ready", "");
        assert!(header_h() < with_detail); // no blank detail lines
        d.set_status(Tone::Good, "Ready", "Attached");
        assert_eq!(header_h(), with_detail);
        d.set_status_tip("version-abc (C:\\x), Dumped At 1");
        assert_eq!(d.header_rect.get().bottom - d.header_rect.get().top, with_detail);
        let mut buf = [0u16; 128];
        let mut ti = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: d.hwnd(),
            uId: STATUS as usize,
            lpszText: PWSTR(buf.as_mut_ptr()),
            ..Default::default()
        };
        send(d.tooltip.get(), TTM_GETTEXTW, buf.len(), &raw mut ti as isize);
        assert_eq!(from_wide(&buf), "version-abc (C:\\x), Dumped At 1");
    }

    #[test]
    fn dashboard_fits_the_shown_page() {
        let d = Dashboard::new(spec(vec![
            page("Short"),
            value(1, "Status"),
            page("Tall"),
            value(2, "A"),
            value(3, "B"),
            value(4, "C"),
            value(5, "D"),
        ]));
        assert!(d.create());
        let rect = || {
            let mut r = RECT::default();
            unsafe { GetWindowRect(d.hwnd(), &mut r) }.unwrap();
            r
        };
        let short = rect();
        d.show_page(1);
        let tall = rect();
        assert!(tall.bottom - tall.top > short.bottom - short.top + d.px(100));
        assert_eq!((tall.left, tall.top, tall.right), (short.left, short.top, short.right)); // top-left stays
        d.show_page(0);
        assert_eq!(rect(), short);
    }

    #[test]
    fn dashboard_long_label_widens_the_column() {
        let label = "Max studs (0 = any)";
        let d = Dashboard::new(Spec {
            label_width: 40,
            ..spec(vec![number(1, label, 0, 100), value(2, "Short")])
        });
        assert!(d.create());
        let label_w = |id_row: usize| {
            let mut r = RECT::default();
            unsafe { GetWindowRect(d.ctls.borrow()[id_row].0, &mut r) }.unwrap();
            r.right - r.left
        };
        assert!(label_w(0) >= d.text_width(label)); // not cut
        assert_eq!(label_w(1), label_w(0)); // one column for all
        let huge = Dashboard::new(spec(vec![value(1, &"x".repeat(300))]));
        assert!(huge.create());
        let mut r = RECT::default();
        unsafe { GetWindowRect(huge.ctls.borrow()[0].0, &mut r) }.unwrap();
        assert_eq!(r.right - r.left, huge.px(LABEL_MAX_DIP) - huge.px(8)); // capped
    }

    #[test]
    fn hint_parts_split_keycaps() {
        assert_eq!(hint_parts("F1"), [("F1", false)]);
        assert_eq!(hint_parts("[F1]"), [("F1", true)]);
        assert_eq!(hint_parts("hold [Mouse 4]"), [("hold", false), ("Mouse 4", true)]);
        assert_eq!(
            hint_parts("press [Ctrl] + [F2] now"),
            [
                ("press", false),
                ("Ctrl", true),
                ("+", false),
                ("F2", true),
                ("now", false)
            ]
        );
        assert_eq!(hint_parts("a [b"), [("a [b", false)]); // unclosed: text
        assert!(hint_parts("").is_empty());
    }

    #[test]
    fn segment_at_splits_evenly() {
        assert_eq!(segment_at(300, 3, 0), 0);
        assert_eq!(segment_at(300, 3, 99), 0);
        assert_eq!(segment_at(300, 3, 100), 1);
        assert_eq!(segment_at(300, 3, 299), 2);
        assert_eq!(segment_at(300, 3, 400), 2); // clamped
        assert_eq!(segment_at(300, 3, -5), 0);
        assert_eq!(segment_at(300, 0, 10), 0); // no options: no panic
    }

    #[test]
    fn tile_lines_split_at_the_first_newline() {
        let gold = vec![rgb(255, 200, 0)];
        let s = |text: &str, colors: &[COLORREF]| Span {
            text: text.into(),
            colors: colors.to_vec(),
            ..Default::default()
        };
        let texts = |v: &[Span]| v.iter().map(|s| s.text.clone()).collect::<Vec<_>>();
        let (big, small) = tile_lines(&[Span::plain("12 caught\n294/h\nstreak 12")]);
        assert_eq!(
            (texts(&big), texts(&small)),
            (vec!["12 caught".into()], vec!["294/h streak 12".into()])
        );
        let (big, small) = tile_lines(&[s("≈ ", &[]), s("59 C$\n1/h", &gold), s(" more", &[])]);
        assert_eq!(texts(&big), ["≈ ", "59 C$"]);
        assert_eq!(texts(&small), ["1/h", " more"]);
        assert_eq!((big[1].colors.clone(), small[0].colors.clone()), (gold.clone(), gold)); // colors kept both sides
        let (big, small) = tile_lines(&[Span::plain("0:01:00")]);
        assert_eq!((texts(&big), small.len()), (vec!["0:01:00".into()], 0)); // no second line
    }

    #[test]
    fn span_lines_split_at_every_newline() {
        let gold = vec![rgb(255, 200, 0)];
        let gold_span = Span {
            text: "1/27\n≈ 9 C$".into(),
            colors: gold.clone(),
            ..Default::default()
        };
        let lines = span_lines(&[Span::plain("Carp\n182 kg · "), gold_span, Span::plain("\n")]);
        let texts: Vec<Vec<&str>> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(texts, [vec!["Carp"], vec!["182 kg · ", "1/27"], vec!["≈ 9 C$"], vec![]]);
        assert!(lines[1][1].colors == gold && lines[2][0].colors == gold); // the style on both sides
        assert_eq!(span_lines(&[]).len(), 1);
    }

    #[test]
    fn dashboard_picture_tiles_sit_level() {
        let d = Dashboard::new(spec(vec![
            section("Fish"),
            tile(1, "Last catch").picture(90).lines(3),
            tile(2, "Rarest").picture(60).lines(2),
            tile(3, "Plain"),
        ]));
        assert!(d.create());
        let rect = |id| {
            let mut r = RECT::default();
            unsafe { GetWindowRect(item(d.hwnd(), id), &mut r) }.unwrap();
            r
        };
        let (a, b, c) = (rect(1), rect(2), rect(3));
        let tall = d.px(24) + 4 * d.line_h.get() + d.px(90); // label, picture, 3 lines, padding
        assert_eq!((a.top, a.bottom - a.top), (b.top, tall));
        assert_eq!(b.bottom - b.top, tall); // the shorter one takes the taller's height
        let plain_h = d.px(16) + 2 * d.line_h.get() + d.big_line_h.get();
        assert_eq!(c.bottom - c.top, plain_h); // the next line: a plain tile, as before
        // the picture area sits inside its tile, under the label
        let to_client = |r: RECT| {
            let mut p = [
                POINT { x: r.left, y: r.top },
                POINT {
                    x: r.right,
                    y: r.bottom,
                },
            ];
            for q in &mut p {
                let _ = unsafe { ScreenToClient(d.hwnd(), q) };
            }
            RECT {
                left: p[0].x,
                top: p[0].y,
                right: p[1].x,
                bottom: p[1].y,
            }
        };
        let (a, s) = (to_client(a), d.slots.borrow()[1]);
        assert!(s.left > a.left && s.right < a.right && s.top > a.top + d.line_h.get() && s.bottom < a.bottom);
        assert_eq!(s.bottom - s.top, d.px(90));
        let (w, h) = d.picture_size(1).unwrap();
        assert_eq!((w as i32, h as i32), (s.right - s.left, s.bottom - s.top));
        assert_eq!(d.picture_size(3), None); // a plain tile has no picture
        let _ = unsafe { ShowWindow(d.hwnd(), SW_SHOWNA) };
        assert!(d.picture_visible(1));
        d.set_picture_pixels(1, w, h, &vec![0xff; (w * h * 4) as usize]);
        d.set_spans(
            1,
            &[Span::plain("Big Carp · Rare\n182.1 kg · 1/27\n≈ 1,106 C$ · +195 XP")],
        );
        let _ = unsafe { UpdateWindow(d.hwnd()) }; // paints the picture and the lines without failing
    }

    #[test]
    fn tile_column_splits_the_card_in_two() {
        assert_eq!(tile_column(0, 101, 9), (0, 46));
        assert_eq!(tile_column(1, 101, 9), (55, 46)); // ends at the card's edge
        assert_eq!(tile_column(2, 101, 9), (0, 46)); // the next line, one column even when alone
    }

    #[test]
    fn dashboard_tiles_sit_two_per_line() {
        let d = Dashboard::new(spec(vec![
            section("S"),
            tile(1, "Catches"),
            tile(2, "Earned"),
            tile(3, "XP"),
            section("T"),
            tile(4, "A"),
            value(5, "V"),
            tile(6, "B"),
        ]));
        assert!(d.create());
        let rect = |id| {
            let mut r = RECT::default();
            unsafe { GetWindowRect(item(d.hwnd(), id), &mut r) }.unwrap();
            r
        };
        let (a, b, c) = (rect(1), rect(2), rect(3));
        let tile_h = d.px(16) + 2 * d.line_h.get() + d.big_line_h.get();
        assert_eq!((a.top, a.bottom - a.top), (b.top, tile_h)); // one line
        assert_eq!(b.left, a.right + d.px(8));
        assert_eq!((c.left, c.right, c.top), (a.left, a.right, a.bottom + d.px(8))); // the lone last: one column
        let card = d.cards.borrow()[0].1;
        assert_eq!(b.right - a.left, card.right - card.left - 2 * d.px(14)); // the card's width
        assert_eq!(card.bottom - card.top, 2 * d.px(6) + 2 * (tile_h + d.px(8)));
        assert_eq!(rect(6).left, rect(4).left); // a run ends at any other row
        d.set_text(1, "12 caught\n294/h");
        assert_eq!(text_of(d.hwnd(), 1), "Catches: 12 caught\n294/h"); // label and value, for screen readers
        assert_eq!(text_of(d.hwnd(), 2), "Earned"); // no value yet: the label
        let _ = unsafe { UpdateWindow(d.hwnd()) };
    }

    #[test]
    fn dashboard_choice_keys_report_and_set_choice_shows() {
        let picks = RefCell::new(Vec::new());
        let d = Dashboard::new(spec(vec![choice(7, "Mode", &["Off", "Aim", "Lock"])]));
        d.on_choice(|d, id, k| {
            picks.borrow_mut().push((id, k));
            d.set_choice(id, k); // the game takes it
        });
        assert!(d.create());
        let key = |vk: u16| {
            let msg = MSG {
                hwnd: item(d.hwnd(), 7),
                message: WM_KEYDOWN,
                wParam: WPARAM(vk as usize),
                ..Default::default()
            };
            d.dispatch(&msg);
        };
        d.set_choice(7, 0);
        assert!(picks.borrow().is_empty()); // set_choice does not echo back
        key(VK_RIGHT.0);
        key(VK_RIGHT.0);
        key(VK_RIGHT.0); // already the last: nothing
        key(VK_LEFT.0);
        assert_eq!(*picks.borrow(), [(7, 1), (7, 2), (7, 1)]);
        assert_eq!(d.choices.borrow()[&7], 1);
    }

    #[test]
    fn dashboard_number_steps_with_buttons_and_wheel() {
        let numbers = RefCell::new(Vec::new());
        let d = Dashboard::new(spec(vec![number(10, "Latency", 0, 100)]));
        d.on_number(|_, id, v| numbers.borrow_mut().push((id, v)));
        assert!(d.create());
        let h = d.hwnd();
        let (minus, plus) = {
            let s = d.steppers.borrow();
            (s[0].0, s[1].0)
        };
        let click = |b: HWND| send(h, WM_COMMAND, (BN_CLICKED as usize) << 16, b.0 as isize);
        d.set_number(10, 99);
        click(plus);
        click(plus); // at hi: stays
        assert_eq!(text_of(h, 10), "100");
        click(minus);
        assert_eq!(*numbers.borrow(), [(10, 100), (10, 99)]);
        // the wheel over the box steps too; elsewhere it does nothing
        let b = d.boxes.borrow()[0];
        let wheel = |x: i32, y: i32, delta: i16| {
            let mut p = POINT { x, y };
            let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut p) };
            send(h, WM_MOUSEWHEEL, (delta as u16 as usize) << 16, make_lparam(p.x, p.y));
        };
        wheel(b.left + 2, (b.top + b.bottom) / 2, -120);
        assert_eq!(text_of(h, 10), "98");
        wheel(b.left + 2, b.bottom + d.px(40), 120);
        assert_eq!(*numbers.borrow(), [(10, 100), (10, 99), (10, 98)]);
    }
}
