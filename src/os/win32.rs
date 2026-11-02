//! Thin, safe wrappers over the Win32 calls the app makes. This is the only
//! module allowed to use `unsafe` (the crate root denies it elsewhere): each
//! block is one documented API call on values this module creates or checks,
//! and each wrapper exposes a safe signature to the rest of the app.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    COLORREF, CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, GlobalFree,
    HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWCP_ROUNDSMALL,
    DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, Arc, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt,
    CLIP_DEFAULT_PRECIS, ClientToScreen, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
    CreateFontIndirectW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_QUALITY,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, ETO_CLIPPED, ETO_OPTIONS, EndPaint,
    EnumFontFamiliesExW, ExtTextOutW, FW_BOLD, FW_NORMAL, GGO_NATIVE, GGO_UNHINTED, GLYPHMETRICS,
    GetDC, GetGlyphOutlineW, GetMonitorInfoW, GetTextExtentPoint32W, GetTextMetricsW, HBITMAP,
    HBRUSH, HDC, HFONT, HGDIOBJ, HMONITOR, InvalidateRect, LOGFONTW, MAT2,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, OUT_TT_PRECIS, PAINTSTRUCT, PS_SOLID,
    Pie, ReleaseDC, SRCCOPY, ScreenToClient, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    TEXTMETRICW, TEXTMETRICW as TM, TRANSPARENT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetFinalPathNameByHandleW, OPEN_EXISTING,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, IPersistFile, STGM_READ,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{
    FindResourceW, GetModuleFileNameW, GetModuleHandleW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadResource, LockResource, SetDefaultDllDirectories, SizeofResource,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock, HeapEnableTerminationOnCorruption,
    HeapSetInformation,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_BINARY,
    RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, ProcessImageLoadPolicy, SetEvent, SetProcessMitigationPolicy,
};
use windows::Win32::System::Time::{
    DYNAMIC_TIME_ZONE_INFORMATION, EnumDynamicTimeZoneInformation, GetDynamicTimeZoneInformation,
    SystemTimeToTzSpecificLocalTimeEx,
};
use windows::Win32::UI::Controls::{EM_SETMARGINS, EM_SETSEL};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor,
    GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, FOLDERID_Desktop, FOLDERID_Downloads, FOLDERID_Public, FOLDERID_SkyDrive,
    FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, SHGetKnownFolderPath, SetWindowSubclass,
    Shell_NotifyIconW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, w};

/// NUL-terminated UTF-16.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pcw(v: &[u16]) -> PCWSTR {
    PCWSTR(v.as_ptr())
}

pub fn is_null(h: HWND) -> bool {
    h.0.is_null()
}

// ---------------------------------------------------------------------------
// Process set-up
// ---------------------------------------------------------------------------

/// First thing at start-up: DLLs load from System32 only, the heap
/// terminates the process on corruption, executable images may only come
/// from local, non-low-integrity files, and the process is Per-Monitor V2
/// DPI aware (the manifest says so too; this covers a missing manifest).
pub fn harden_process() {
    // SAFETY: plain flag arguments; failures are ignored (older Windows).
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        let _ = HeapSetInformation(None, HeapEnableTerminationOnCorruption, None, 0);
        let mut policy = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
        // NoRemoteImages | NoLowMandatoryLabelImages | PreferSystem32Images
        policy.Anonymous.Flags = 0b111;
        let _ = SetProcessMitigationPolicy(
            ProcessImageLoadPolicy,
            (&raw const policy).cast(),
            size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
        );
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// COM on this thread (the Startup shortcut uses IShellLinkW).
pub fn init_com() {
    // SAFETY: called once on the UI thread before any COM use.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
}

pub fn module_instance() -> HINSTANCE {
    // SAFETY: NULL asks for the exe's own module handle.
    unsafe {
        GetModuleHandleW(PCWSTR::null())
            .map(Into::into)
            .unwrap_or_default()
    }
}

/// Full path of the running exe, as Windows loaded it.
pub fn exe_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    // SAFETY: the buffer and its length are passed together.
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// The bytes of one resource of this exe (numeric type and id), or None.
/// Resources are mapped with the image and live as long as the process.
pub fn resource(kind: u16, id: u16) -> Option<&'static [u8]> {
    let as_id = |v: u16| PCWSTR(std::ptr::without_provenance::<u16>(usize::from(v)));
    // SAFETY: lookups in our own module; LockResource returns a pointer into
    // the mapped image, valid for SizeofResource bytes for the process's life.
    unsafe {
        let module = Some(module_instance().into());
        let info = FindResourceW(module, as_id(id), as_id(kind));
        if info.is_invalid() {
            return None;
        }
        let size = SizeofResource(module, info) as usize;
        let data = LoadResource(module, info).ok()?;
        let ptr = LockResource(data);
        (!ptr.is_null() && size > 0).then(|| std::slice::from_raw_parts(ptr.cast::<u8>(), size))
    }
}

// ---------------------------------------------------------------------------
// Windows and messages
// ---------------------------------------------------------------------------

pub type Handler = fn(HWND, u32, usize, isize) -> Option<isize>;

static HANDLER: std::sync::OnceLock<Handler> = std::sync::OnceLock::new();

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(h) = HANDLER.get()
        && let Some(r) = h(hwnd, msg, wparam.0, lparam.0)
    {
        return LRESULT(r);
    }
    // SAFETY: forwarding the message unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub const CLASS_NAME: PCWSTR = w!("WeekNumberRs");

/// Register the one window class every app window uses; `handler` gets
/// every message first (None = default processing).
pub fn register_class(handler: Handler) -> bool {
    let _ = HANDLER.set(handler);
    // SAFETY: a fully initialised class structure with static strings.
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: module_instance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    }
}

pub struct WindowSpec<'a> {
    pub ex_style: u32,
    pub style: u32,
    pub title: &'a str,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub const WS_POPUP_: u32 = WS_POPUP.0;
pub const WS_DIALOG: u32 = WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0;
/// A dialog the user can resize and maximise (the Legal notices window).
pub const WS_RESIZABLE: u32 = WS_DIALOG | WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0;
pub const WS_EX_TOPMOST_: u32 = WS_EX_TOPMOST.0;
pub const WS_EX_TOOLWINDOW_: u32 = WS_EX_TOOLWINDOW.0;
pub const WS_EX_NOACTIVATE_: u32 = WS_EX_NOACTIVATE.0;

pub fn create_window(spec: &WindowSpec<'_>) -> Option<HWND> {
    let title = wide(spec.title);
    // SAFETY: registered class, owned strings outliving the call.
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(spec.ex_style),
            CLASS_NAME,
            pcw(&title),
            WINDOW_STYLE(spec.style),
            spec.x,
            spec.y,
            spec.w,
            spec.h,
            None,
            None,
            Some(module_instance()),
            None,
        )
    };
    h.ok()
}

/// A hidden top-level window for the tray: top-level (not message-only) so it
/// receives the TaskbarCreated broadcast.
pub fn create_hidden_window() -> Option<HWND> {
    create_window(&WindowSpec {
        ex_style: WS_EX_TOOLWINDOW.0,
        style: WS_POPUP.0,
        title: "WeekNumber",
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    })
}

pub fn destroy_window(h: HWND) {
    // SAFETY: destroying a window this thread created; errors ignored.
    unsafe {
        let _ = DestroyWindow(h);
    }
}

pub fn post_message(h: HWND, msg: u32, wparam: usize, lparam: isize) {
    // SAFETY: posting to a window of this thread.
    unsafe {
        let _ = PostMessageW(Some(h), msg, WPARAM(wparam), LPARAM(lparam));
    }
}

pub fn post_quit() {
    // SAFETY: ends this thread's message loop.
    unsafe { PostQuitMessage(0) }
}

pub fn register_window_message(name: &str) -> u32 {
    let n = wide(name);
    // SAFETY: NUL-terminated string.
    unsafe { RegisterWindowMessageW(pcw(&n)) }
}

/// Let `msg` through UIPI from lower-integrity senders (TaskbarCreated).
pub fn allow_message(h: HWND, msg: u32) {
    // SAFETY: plain arguments.
    unsafe {
        let _ = ChangeWindowMessageFilterEx(h, msg, MSGFLT_ALLOW, None);
    }
}

/// The message loop. Blocks until a message arrives or a newer launch asks
/// this copy to hand over through the instance's quit event
/// (then calls `on_event`); no timer, no polling.
pub fn run_message_loop(instance: &Instance, mut on_event: impl FnMut()) -> i32 {
    let mut msg = MSG::default();
    let event = instance.quit.as_ref();
    loop {
        let handles: Vec<HANDLE> = event.map(|e| e.0).into_iter().collect();
        // SAFETY: valid handle slice (possibly empty); waits for input.
        let r = unsafe {
            MsgWaitForMultipleObjectsEx(
                if handles.is_empty() {
                    None
                } else {
                    Some(&handles)
                },
                u32::MAX,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if !handles.is_empty() && r == WAIT_OBJECT_0 {
            on_event();
        }
        // SAFETY: standard PeekMessage/Translate/Dispatch on this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return msg.wParam.0 as i32;
                }
                if !dialog_key(&msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

/// Keys pressed in a child edit control that its top-level window handles
/// (Tab, Return, Escape): forwarded as WM_APP_KEY to the top level.
fn dialog_key(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let key = msg.wParam.0 as u16;
    if ![VK_TAB_, VK_RETURN_, VK_ESCAPE_].contains(&key) {
        return false;
    }
    // SAFETY: GetAncestor on the message's window handle.
    let top = unsafe { GetAncestor(msg.hwnd, GA_ROOT) };
    if top == msg.hwnd || is_null(top) {
        return false;
    }
    post_message(top, WM_APP_KEY, usize::from(key), 0);
    true
}

pub const WM_APP_KEY: u32 = WM_APP + 0x20;
pub const VK_TAB_: u16 = 0x09;
pub const VK_RETURN_: u16 = 0x0D;
pub const VK_ESCAPE_: u16 = 0x1B;

// ---------------------------------------------------------------------------
// Geometry, DPI and chrome
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Rect {
        Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorGeom {
    pub monitor: Rect,
    pub work: Rect,
    pub dpi: u32,
}

/// Where the calendar hangs from when not at the pointer (macOS only).
pub fn tray_anchor() -> Option<(i32, i32)> {
    None
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: writes one POINT.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn monitor_geom(hmon: HMONITOR) -> MonitorGeom {
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: valid monitor handle from MonitorFrom*; out-params are locals.
    unsafe {
        let _ = GetMonitorInfoW(hmon, &mut mi);
        if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() {
            dx = system_dpi();
        }
    }
    MonitorGeom {
        monitor: mi.rcMonitor.into(),
        work: mi.rcWork.into(),
        dpi: dx.clamp(48, 960),
    }
}

/// The monitor under the cursor (not the primary one).
pub fn cursor_monitor() -> MonitorGeom {
    let (x, y) = cursor_pos();
    // SAFETY: MonitorFromPoint with DEFAULTTONEAREST always returns a monitor.
    monitor_geom(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) })
}

pub fn system_dpi() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetDpiForSystem() }.clamp(48, 960)
}

pub fn window_dpi(h: HWND) -> u32 {
    // SAFETY: plain handle.
    let d = unsafe { GetDpiForWindow(h) };
    if d == 0 { system_dpi() } else { d }
}

pub fn metric_for_dpi(index: i32, dpi: u32) -> i32 {
    // SAFETY: plain arguments.
    unsafe { GetSystemMetricsForDpi(SYSTEM_METRICS_INDEX(index), dpi) }
}

pub fn system_metric(index: i32) -> i32 {
    // SAFETY: plain argument.
    unsafe { GetSystemMetrics(SYSTEM_METRICS_INDEX(index)) }
}

pub const SM_CXSMICON_: i32 = SM_CXSMICON.0;
pub const SM_CXICON_: i32 = SM_CXICON.0;
pub const SM_CXEDGE_: i32 = SM_CXEDGE.0;

/// Outer window size for a client area of `w` x `h` at `dpi`.
pub fn outer_size(style: u32, ex_style: u32, w: i32, h: i32, dpi: u32) -> (i32, i32, i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    // SAFETY: one RECT in/out.
    unsafe {
        let _ = AdjustWindowRectExForDpi(
            &mut r,
            WINDOW_STYLE(style),
            false,
            WINDOW_EX_STYLE(ex_style),
            dpi,
        );
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// Thin SetWindowPos wrapper; the flags mirror the Win32 call one to one.
#[allow(clippy::too_many_arguments)]
pub fn set_window_pos(
    h: HWND,
    x: i32,
    y: i32,
    w: i32,
    hgt: i32,
    topmost: Option<bool>,
    no_size: bool,
    no_move: bool,
) {
    let mut flags = SWP_NOACTIVATE;
    if no_size {
        flags |= SWP_NOSIZE;
    }
    if no_move {
        flags |= SWP_NOMOVE;
    }
    let after = match topmost {
        Some(true) => Some(HWND_TOPMOST),
        Some(false) => Some(HWND_NOTOPMOST),
        None => {
            flags |= SWP_NOZORDER;
            None
        }
    };
    // SAFETY: window of this thread.
    unsafe {
        let _ = SetWindowPos(h, after, x, y, w, hgt, flags);
    }
}

pub fn client_to_screen(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    // SAFETY: one POINT in/out.
    unsafe {
        let _ = ClientToScreen(h, &mut p);
    }
    (p.x, p.y)
}

pub fn screen_to_client(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    // SAFETY: one POINT in/out.
    unsafe {
        let _ = ScreenToClient(h, &mut p);
    }
    (p.x, p.y)
}

pub fn show_window(h: HWND, activate: bool) {
    // SAFETY: window of this thread.
    unsafe {
        let _ = ShowWindow(h, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
    }
}

pub fn restore_window(h: HWND) {
    // SAFETY: as above.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
    }
}

pub fn set_foreground(h: HWND) -> bool {
    // SAFETY: as above.
    unsafe { SetForegroundWindow(h).as_bool() }
}

pub fn set_focus(h: HWND) {
    // SAFETY: window of this thread.
    unsafe {
        let _ = SetFocus(Some(h));
    }
}

pub fn invalidate(h: HWND) {
    // SAFETY: whole client area.
    unsafe {
        let _ = InvalidateRect(Some(h), None, false);
    }
}

pub fn set_timer(h: HWND, id: usize, ms: u32) {
    // SAFETY: timer owned by our window; no callback.
    unsafe {
        SetTimer(Some(h), id, ms.max(10), None);
    }
}

pub fn kill_timer(h: HWND, id: usize) {
    // SAFETY: as above.
    unsafe {
        let _ = KillTimer(Some(h), id);
    }
}

pub fn capture(h: HWND) {
    // SAFETY: window of this thread.
    unsafe {
        SetCapture(h);
    }
}

pub fn release_capture() {
    // SAFETY: no arguments.
    unsafe {
        let _ = ReleaseCapture();
    }
}

/// Ask for WM_MOUSELEAVE once the pointer leaves `h`.
pub fn track_leave(h: HWND) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: h,
        dwHoverTime: 0,
    };
    // SAFETY: one struct.
    unsafe {
        let _ = TrackMouseEvent(&mut t);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Hand,
    Cross,
}

pub fn set_cursor(c: Cursor) {
    let id = match c {
        Cursor::Arrow => IDC_ARROW,
        Cursor::Hand => IDC_HAND,
        Cursor::Cross => IDC_CROSS,
    };
    // SAFETY: shared system cursors, never freed.
    unsafe {
        SetCursor(LoadCursorW(None, id).ok());
    }
}

fn dwm_int(h: HWND, attr: DWMWINDOWATTRIBUTE, value: i32) -> bool {
    // SAFETY: a 4-byte value and its size.
    unsafe { DwmSetWindowAttribute(h, attr, (&raw const value).cast(), 4).is_ok() }
}

/// Windows 11 rounded corners (no-op before Windows 11).
pub fn round_corners(h: HWND, small: bool) {
    dwm_int(
        h,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        if small {
            DWMWCP_ROUNDSMALL.0
        } else {
            DWMWCP_ROUND.0
        },
    );
}

/// Dark caption; attribute 19 on builds 1809-1903.
pub fn dark_title_bar(h: HWND, dark: bool) {
    if !dwm_int(h, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark)) {
        dwm_int(h, DWMWINDOWATTRIBUTE(19), i32::from(dark));
    }
}

pub fn set_title(h: HWND, title: &str) {
    let t = wide(title);
    // SAFETY: NUL-terminated string.
    unsafe {
        let _ = SetWindowTextW(h, pcw(&t));
    }
}

pub fn is_window(h: HWND) -> bool {
    // SAFETY: any handle value is accepted.
    unsafe { IsWindow(Some(h)).as_bool() }
}

pub fn message_box_error(text: &str) {
    let t = wide(text);
    // SAFETY: NUL-terminated strings, no owner.
    unsafe {
        MessageBoxW(
            None,
            pcw(&t),
            w!("WeekNumber"),
            MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Whether a key is held down now (Ctrl, Shift, ...).
pub fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

pub const VK_SHIFT_: u16 = 0x10;
pub const VK_CONTROL_: u16 = 0x11;

/// The longest gap between the clicks of a double click (ms).
pub fn double_click_ms() -> u64 {
    // SAFETY: no arguments.
    u64::from(unsafe { GetDoubleClickTime() })
}

/// WM_GETMINMAXINFO: set the smallest outer size the user can drag to.
pub fn set_min_track_size(lparam: isize, w: i32, h: i32) {
    // SAFETY: Windows passes a valid, writable MINMAXINFO in lparam for
    // WM_GETMINMAXINFO, which is the only message this is called for.
    unsafe {
        if let Some(info) = (lparam as *mut MINMAXINFO).as_mut() {
            info.ptMinTrackSize = POINT { x: w, y: h };
        }
    }
}

/// The client area's size.
pub fn client_size(h: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    // SAFETY: a RECT out-parameter.
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

pub fn beep() {
    // SAFETY: plain argument.
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK);
    }
}

pub fn tick_ms() -> u64 {
    // SAFETY: no arguments.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// An owned HICON.
pub struct Icon(pub HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: we own the icon.
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

/// The largest frame of the exe's own icon (the same app.ico frames the
/// Python version reads), decoded.
pub fn app_icon_image() -> Option<crate::gfx::Rgba> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let group = resource(RT_GROUP_ICON, 1)?;
    let id = crate::png::largest_icon_id(group)?;
    crate::png::decode(resource(RT_ICON, id)?)
}

/// The exe's icon resource at exactly `size` px (LoadImage picks the frame).
pub fn app_icon(size: i32) -> Option<Icon> {
    // SAFETY: resource id 1 of our own module; the icon is owned by Icon.
    unsafe {
        LoadImageW(
            Some(module_instance()),
            // MAKEINTRESOURCEW(1): the RT_GROUP_ICON id build.rs writes.
            PCWSTR(std::ptr::without_provenance::<u16>(1)),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
        .ok()
        .map(|h| Icon(HICON(h.0)))
    }
}

/// An icon from top-down straight-alpha RGBA pixels (alpha mask included).
pub fn icon_from_rgba(w: usize, h: usize, rgba: &[[u8; 4]]) -> Option<Icon> {
    let mut bits: *mut c_void = std::ptr::null_mut();
    let header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: a w*h 32-bit DIB whose bits pointer we fill with exactly w*h
    // pixels; both bitmaps are deleted after CreateIconIndirect copied them.
    unsafe {
        let color = CreateDIBSection(None, &header, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() || rgba.len() != w * h {
            let _ = DeleteObject(color.into());
            return None;
        }
        let dst = std::slice::from_raw_parts_mut(bits.cast::<[u8; 4]>(), w * h);
        for (d, s) in dst.iter_mut().zip(rgba) {
            *d = [s[2], s[1], s[0], s[3]];
        }
        let mask = CreateBitmap(w as i32, h as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.map(Icon)
    }
}

/// The shell shows the tray icon at exactly one size.
/// The font size for the week number in a `size` px tray icon: the whole
/// icon (the digits then fill it).
pub fn tray_text_height(size: usize) -> usize {
    size
}

pub fn tray_icon_sizes(size: usize) -> Vec<usize> {
    vec![size]
}

/// An icon from its first rendering (one size on Windows).
pub fn icon_from_frames(frames: Vec<crate::gfx::Rgba>) -> Option<Icon> {
    let f = frames.into_iter().next()?;
    icon_from_rgba(f.w, f.h, &f.px)
}

/// WM_SETICON small and big; the window does not take ownership.
pub fn set_window_icons(h: HWND, small: &Icon, big: &Icon) {
    // SAFETY: icons outlive the window (the caller keeps them).
    unsafe {
        SendMessageW(
            h,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0.0 as isize)),
        );
        SendMessageW(
            h,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big.0.0 as isize)),
        );
    }
}

// ---------------------------------------------------------------------------
// Notification-area icon and menus
// ---------------------------------------------------------------------------

pub const WM_TRAY: u32 = WM_APP + 1;

fn notify_data(h: HWND, icon: Option<&Icon>, tip: &str, flags: u32) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: h,
        uID: 1,
        uFlags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS(flags),
        uCallbackMessage: WM_TRAY,
        hIcon: icon.map(|i| i.0).unwrap_or_default(),
        ..Default::default()
    };
    for (slot, unit) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
        *slot = unit;
    }
    data
}

/// Add, modify or remove our notification-area icon.
pub fn tray_icon(h: HWND, op: TrayOp, icon: Option<&Icon>, tip: &str) -> bool {
    let (msg, flags) = match op {
        TrayOp::Add => (NIM_ADD, NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0),
        TrayOp::Modify => (NIM_MODIFY, NIF_ICON.0 | NIF_TIP.0),
        TrayOp::Delete => (NIM_DELETE, 0),
    };
    let data = notify_data(h, icon, tip, flags);
    // SAFETY: a fully initialised NOTIFYICONDATAW.
    unsafe { Shell_NotifyIconW(msg, &data).as_bool() }
}

#[derive(Clone, Copy)]
pub enum TrayOp {
    Add,
    Modify,
    Delete,
}

// ---------------------------------------------------------------------------
// Drawing: DIB back buffer, solid fills, GDI text
// ---------------------------------------------------------------------------

/// A borrowed device context.
#[derive(Clone, Copy)]
pub struct Dc(HDC);

impl Dc {
    /// A Tk canvas oval with fill and 1 px outline on the box (x1, y1)-(x2, y2)
    /// as tkWinDraw.c draws it: XFillArc becomes GDI Pie with a pen and a
    /// brush in the fill colour, XDrawArc becomes Arc in the outline colour,
    /// both on the box widened by one pixel, start and end at 3 o'clock.
    pub fn tk_oval(self, x1: i32, y1: i32, x2: i32, y2: i32, fill: u32, outline: u32) {
        let (w, h) = (f64::from(x2 - x1), f64::from(y2 - y1));
        let xs = (f64::from(x1) + w / 2.0 + w / 2.0 + 0.5) as i32;
        let ys = (f64::from(y1) + h / 2.0 + 0.5) as i32;
        // SAFETY: temporary pens and brush, deselected and deleted after use.
        unsafe {
            let pen = CreatePen(PS_SOLID, 0, COLORREF(fill));
            let brush = CreateSolidBrush(COLORREF(fill));
            let old_pen = SelectObject(self.0, pen.into());
            let old_brush = SelectObject(self.0, brush.into());
            let _ = Pie(self.0, x1, y1, x2 + 1, y2 + 1, xs, ys, xs, ys);
            SelectObject(self.0, old_brush);
            let _ = DeleteObject(brush.into());
            let line = CreatePen(PS_SOLID, 1, COLORREF(outline));
            SelectObject(self.0, line.into());
            let _ = DeleteObject(pen.into());
            SetBkMode(self.0, TRANSPARENT);
            let _ = Arc(self.0, x1, y1, x2 + 1, y2 + 1, xs, ys, xs, ys);
            SelectObject(self.0, old_pen);
            let _ = DeleteObject(line.into());
        }
    }

    /// Text at (x, y) = top-left of the line box, transparent background.
    pub fn text(self, font: &Font, x: i32, y: i32, s: &str, color: u32) {
        let t: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: font owned by `font`, selected only for this call.
        unsafe {
            let old = SelectObject(self.0, font.0.into());
            SetBkMode(self.0, TRANSPARENT);
            SetTextColor(self.0, COLORREF(color));
            let _ = ExtTextOutW(
                self.0,
                x,
                y,
                ETO_OPTIONS(0),
                None,
                PCWSTR(t.as_ptr()),
                t.len() as u32,
                None,
            );
            SelectObject(self.0, old);
        }
    }
}

impl Dc {
    /// Text as text() draws it, clipped to `clip`.
    pub fn text_clipped(self, font: &Font, x: i32, y: i32, s: &str, color: u32, clip: Rect) {
        let t: Vec<u16> = s.encode_utf16().collect();
        let rc = RECT {
            left: clip.left,
            top: clip.top,
            right: clip.right,
            bottom: clip.bottom,
        };
        // SAFETY: font owned by `font`, selected only for this call.
        unsafe {
            let old = SelectObject(self.0, font.0.into());
            SetBkMode(self.0, TRANSPARENT);
            SetTextColor(self.0, COLORREF(color));
            let _ = ExtTextOutW(
                self.0,
                x,
                y,
                ETO_CLIPPED,
                Some(&rc),
                PCWSTR(t.as_ptr()),
                t.len() as u32,
                None,
            );
            SelectObject(self.0, old);
        }
    }
}

fn measure_on(dc: HDC, font: &Font, s: &str) -> i32 {
    let t: Vec<u16> = s.encode_utf16().collect();
    let mut size = SIZE::default();
    // SAFETY: font selected only for this call.
    unsafe {
        let old = SelectObject(dc, font.0.into());
        let _ = GetTextExtentPoint32W(dc, &t, &mut size);
        SelectObject(dc, old);
    }
    size.cx
}

/// The TrueType outline of one character in `font`, unhinted or grid-fitted
/// by GDI, in pixels (x right, y up from the pen origin on the baseline).
/// None for a character the font has no outline for.
pub fn glyph_outline(font: &Font, ch: char, hinted: bool) -> Option<Vec<crate::gfx::Contour>> {
    use crate::gfx::{Contour, PathSeg};
    const GDI_ERROR: u32 = u32::MAX;
    const TT_PRIM_LINE: u16 = 1;
    const TT_PRIM_QSPLINE: u16 = 2;
    let one = windows::Win32::Graphics::Gdi::FIXED { fract: 0, value: 1 };
    let zero = windows::Win32::Graphics::Gdi::FIXED { fract: 0, value: 0 };
    let identity = MAT2 {
        eM11: one,
        eM12: zero,
        eM21: zero,
        eM22: one,
    };
    let format = windows::Win32::Graphics::Gdi::GET_GLYPH_OUTLINE_FORMAT(
        GGO_NATIVE.0 | if hinted { 0 } else { GGO_UNHINTED.0 },
    );
    let mut gm = GLYPHMETRICS::default();
    let mut buf: Vec<u8> = Vec::new();
    // SAFETY: screen DC borrowed and released; the font is selected only
    // for these calls; the second call writes at most `len` bytes into a
    // buffer of exactly that size.
    unsafe {
        let dc = GetDC(None);
        let old = SelectObject(dc, font.0.into());
        let len = GetGlyphOutlineW(dc, ch as u32, format, &mut gm, 0, None, &identity);
        if len != GDI_ERROR && len > 0 && len <= 1 << 20 {
            buf.resize(len as usize, 0);
            let got = GetGlyphOutlineW(
                dc,
                ch as u32,
                format,
                &mut gm,
                len,
                Some(buf.as_mut_ptr().cast()),
                &identity,
            );
            if got == GDI_ERROR {
                buf.clear();
            }
        }
        SelectObject(dc, old);
        ReleaseDC(None, dc);
    }
    if buf.is_empty() {
        return None;
    }
    // TTPOLYGONHEADER { cb: u32, dwType: u32, pfxStart: POINTFX } followed
    // by TTPOLYCURVE { wType: u16, cpfx: u16, apfx: [POINTFX; cpfx] }
    // records; POINTFX is two 16.16 FIXED values.
    let u16_at = |b: &[u8], i: usize| b.get(i..i + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
    let u32_at = |b: &[u8], i: usize| {
        b.get(i..i + 4)
            .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    };
    let fixed = |b: &[u8], i: usize| {
        b.get(i..i + 4)
            .map(|v| f64::from(i32::from_le_bytes([v[0], v[1], v[2], v[3]])) / 65536.0)
    };
    let point = |b: &[u8], i: usize| Some((fixed(b, i)?, fixed(b, i + 4)?));
    let mut contours = Vec::new();
    let mut at = 0usize;
    while at + 16 <= buf.len() {
        let cb = u32_at(&buf, at)? as usize;
        if cb < 16 || at + cb > buf.len() {
            return None;
        }
        let start = point(&buf, at + 8)?;
        let mut segs = Vec::new();
        let mut i = at + 16;
        while i + 4 <= at + cb {
            let kind = u16_at(&buf, i)?;
            let n = usize::from(u16_at(&buf, i + 2)?);
            let pts: Vec<(f64, f64)> = (0..n)
                .map(|k| point(&buf, i + 4 + 8 * k))
                .collect::<Option<_>>()?;
            match kind {
                TT_PRIM_LINE => segs.extend(pts.iter().map(|&(x, y)| PathSeg::Line(x, y))),
                TT_PRIM_QSPLINE if n >= 2 => {
                    // A B-spline: implied on-curve points between controls.
                    for k in 0..n - 1 {
                        let c = pts[k];
                        let end = if k + 2 < n {
                            (
                                (pts[k].0 + pts[k + 1].0) / 2.0,
                                (pts[k].1 + pts[k + 1].1) / 2.0,
                            )
                        } else {
                            pts[n - 1]
                        };
                        segs.push(PathSeg::Quad(c.0, c.1, end.0, end.1));
                    }
                }
                _ => return None,
            }
            i += 4 + 8 * n;
        }
        contours.push(Contour { start, segs });
        at += cb;
    }
    Some(contours)
}

/// A 32-bit top-down DIB with its own memory DC: every window paints its
/// whole client area here, then copies it to the screen in one BitBlt.
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub w: usize,
    pub h: usize,
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Option<Surface> {
        let (w, h) = (w.max(1), h.max(1));
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: DIB and DC owned by the Surface and released in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let Ok(bitmap) =
                CreateDIBSection(Some(dc), &header, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(dc);
                return None;
            };
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap.into());
            Some(Surface {
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                w,
                h,
            })
        }
    }

    pub fn dc(&self) -> Dc {
        Dc(self.dc)
    }

    /// The pixels, 0x00RRGGBB, row-major, top-down.
    pub fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: CreateDIBSection allocated exactly w*h 32-bit pixels that
        // live as long as the bitmap, which lives as long as self; GDI is
        // not drawing on it while this borrow exists (single thread).
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.w * self.h) }
    }

    /// GDI may still be batching drawing calls; flush before touching pixels.
    pub fn flush(&self) {
        // SAFETY: no arguments.
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::GdiFlush();
        }
    }

    pub fn fill_rect(&mut self, r: Rect, rgb: u32) {
        self.flush();
        let (w, h) = (self.w as i32, self.h as i32);
        let (x0, x1) = (r.left.clamp(0, w), r.right.clamp(0, w));
        let (y0, y1) = (r.top.clamp(0, h), r.bottom.clamp(0, h));
        let wu = self.w;
        let px = self.pixels();
        for y in y0..y1 {
            px[y as usize * wu + x0 as usize..y as usize * wu + x1 as usize].fill(rgb);
        }
    }

    /// Copy an opaque image (alpha ignored) with its top-left at (x, y).
    pub fn blit(&mut self, img: &crate::gfx::Rgba, x: i32, y: i32) {
        self.flush();
        let (sw, sh) = (self.w as i32, self.h as i32);
        let wu = self.w;
        let px = self.pixels();
        for iy in 0..img.h as i32 {
            let ty = y + iy;
            if ty < 0 || ty >= sh {
                continue;
            }
            for ix in 0..img.w as i32 {
                let tx = x + ix;
                if tx < 0 || tx >= sw {
                    continue;
                }
                let p = img.px[iy as usize * img.w + ix as usize];
                px[ty as usize * wu + tx as usize] =
                    (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
            }
        }
    }

    /// Draw an image with alpha over what is already there, with Tk's
    /// photo-image blend (truncating integer arithmetic).
    pub fn blend(&mut self, img: &crate::gfx::Rgba, x: i32, y: i32) {
        self.flush();
        let (sw, sh) = (self.w as i32, self.h as i32);
        let wu = self.w;
        let px = self.pixels();
        for iy in 0..img.h as i32 {
            let ty = y + iy;
            if ty < 0 || ty >= sh {
                continue;
            }
            for ix in 0..img.w as i32 {
                let tx = x + ix;
                if tx < 0 || tx >= sw {
                    continue;
                }
                let p = img.px[iy as usize * img.w + ix as usize];
                let a = u32::from(p[3]);
                if a == 0 {
                    continue;
                }
                let d = &mut px[ty as usize * wu + tx as usize];
                let un = 255 - a;
                let mix = |bg: u32, c: u8| (bg * un + u32::from(c) * a) / 255;
                let r = mix((*d >> 16) & 0xFF, p[0]);
                let g = mix((*d >> 8) & 0xFF, p[1]);
                let b = mix(*d & 0xFF, p[2]);
                *d = (r << 16) | (g << 8) | b;
            }
        }
    }

    /// Copy to the window's DC (at 0, 0).
    pub fn present(&self, target: Dc) {
        // SAFETY: both DCs valid for the call.
        unsafe {
            let _ = BitBlt(
                target.0,
                0,
                0,
                self.w as i32,
                self.h as i32,
                Some(self.dc),
                0,
                0,
                SRCCOPY,
            );
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: we own the DC and the bitmap.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// BeginPaint/EndPaint around `f`, which gets the window DC.
pub fn paint(h: HWND, f: impl FnOnce(Dc)) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: paired BeginPaint/EndPaint on a window of this thread.
    unsafe {
        let dc = BeginPaint(h, &mut ps);
        if !dc.is_invalid() {
            f(Dc(dc));
        }
        let _ = EndPaint(h, &ps);
    }
}

/// An owned GDI font.
pub struct Font(HFONT, pub FontMetrics);

#[derive(Clone, Copy, Debug, Default)]
pub struct FontMetrics {
    pub ascent: i32,
    pub descent: i32,
}

impl FontMetrics {
    pub fn linespace(self) -> i32 {
        self.ascent + self.descent
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// As Tk: the system setting (ClearType or not).
    Default,
    /// Grayscale antialiasing, for rendering coverage masks.
    Antialiased,
}

impl Font {
    /// `px` pixels em height (Tk's negative font size), bold or normal, as
    /// Tk's CreateFontIndirect call builds it.
    pub fn new(family: &str, px: i32, bold: bool, quality: Quality) -> Option<Font> {
        let mut lf = LOGFONTW {
            lfHeight: -px.max(1),
            lfWeight: if bold {
                FW_BOLD.0 as i32
            } else {
                FW_NORMAL.0 as i32
            },
            lfCharSet: DEFAULT_CHARSET,
            lfOutPrecision: OUT_TT_PRECIS,
            lfClipPrecision: CLIP_DEFAULT_PRECIS,
            lfQuality: if quality == Quality::Default {
                DEFAULT_QUALITY
            } else {
                ANTIALIASED_QUALITY
            },
            ..Default::default()
        };
        for (slot, unit) in lf.lfFaceName.iter_mut().zip(family.encode_utf16().take(31)) {
            *slot = unit;
        }
        // SAFETY: a filled LOGFONTW; the font is owned by Font.
        let hf = unsafe { CreateFontIndirectW(&lf) };
        if hf.is_invalid() {
            return None;
        }
        let mut tm = TEXTMETRICW::default();
        // SAFETY: screen DC borrowed and released; font selected temporarily.
        unsafe {
            let dc = GetDC(None);
            let old = SelectObject(dc, hf.into());
            let _ = GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            ReleaseDC(None, dc);
        }
        Some(Font(
            hf,
            FontMetrics {
                ascent: tm.tmAscent,
                descent: tm.tmDescent,
            },
        ))
    }

    pub fn metrics(&self) -> FontMetrics {
        self.1
    }

    /// Text width in pixels (Tk_TextWidth: GetTextExtentPoint32W).
    pub fn measure(&self, s: &str) -> i32 {
        // SAFETY: screen DC borrowed and released.
        unsafe {
            let dc = GetDC(None);
            let w = measure_on(dc, self, s);
            ReleaseDC(None, dc);
            w
        }
    }

    pub fn handle(&self) -> isize {
        self.0.0 as isize
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: we own the font.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

/// Tk's TkTextFont/TkDefaultFont on Windows: the system message font.
pub fn tk_text_font() -> Option<Font> {
    let dpi = system_dpi();
    let mut ncm = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: one NONCLIENTMETRICSW with its size.
    unsafe {
        windows::Win32::UI::HiDpi::SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some((&raw mut ncm).cast()),
            0,
            dpi,
        )
        .ok()?;
    }
    let lf = ncm.lfMessageFont;
    let n = lf
        .lfFaceName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(lf.lfFaceName.len());
    let face = String::from_utf16_lossy(&lf.lfFaceName[..n]);
    let px = if lf.lfHeight < 0 {
        -lf.lfHeight
    } else {
        lf.lfHeight.max(1)
    };
    Font::new(&face, px, lf.lfWeight >= 700, Quality::Default)
}

unsafe extern "system" fn font_enum_proc(
    lf: *const LOGFONTW,
    _tm: *const TM,
    _kind: u32,
    lparam: LPARAM,
) -> i32 {
    // SAFETY: GDI passes a valid LOGFONTW; lparam is the &mut Vec<String>
    // given to EnumFontFamiliesExW below, alive for the whole enumeration.
    unsafe {
        let names = &mut *(lparam.0 as *mut Vec<String>);
        let face = &(*lf).lfFaceName;
        let len = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        names.push(String::from_utf16_lossy(&face[..len]));
    }
    1
}

/// Every installed font family name (Tk's `font families`).
pub fn font_families() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let lf = LOGFONTW {
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };
    // SAFETY: screen DC borrowed and released; the callback writes only to
    // `names`, which outlives the call.
    unsafe {
        let dc = GetDC(None);
        EnumFontFamiliesExW(
            dc,
            &lf,
            Some(font_enum_proc),
            LPARAM(&raw mut names as isize),
            0,
        );
        ReleaseDC(None, dc);
    }
    names
}

/// A solid brush, for WM_CTLCOLOREDIT. Owned.
pub struct Brush(HBRUSH);

impl Brush {
    pub fn new(c: u32) -> Brush {
        // SAFETY: plain colour.
        Brush(unsafe { CreateSolidBrush(COLORREF(c)) })
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        // SAFETY: we own the brush.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

/// WM_CTLCOLOREDIT: set the edit control's text and background colours on
/// its DC (wparam) and return the brush for the rest of it.
pub fn ctl_color(wparam: usize, fg: u32, bg: u32, brush: &Brush) -> isize {
    let dc = HDC(wparam as *mut c_void);
    // SAFETY: the DC passed with WM_CTLCOLOREDIT, valid for the message.
    unsafe {
        SetTextColor(dc, COLORREF(fg));
        SetBkColor(dc, COLORREF(bg));
    }
    brush.0.0 as isize
}

// ---------------------------------------------------------------------------
// Edit control (the colour dialog's hex field)
// ---------------------------------------------------------------------------

pub const EN_CHANGE_: u16 = EN_CHANGE as u16;
pub const EN_KILLFOCUS_: u16 = EN_KILLFOCUS as u16;
pub const EN_SETFOCUS_: u16 = EN_SETFOCUS as u16;

pub fn create_edit(parent: HWND, id: u16, font: &Font, text: &str) -> Option<HWND> {
    let t = wide(text);
    // SAFETY: child of our window; the system EDIT class.
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            pcw(&t),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WINDOW_STYLE(ES_LEFT as u32 | ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(usize::from(id) as *mut c_void)),
            Some(module_instance()),
            None,
        )
        .ok()?
    };
    // SAFETY: WM_SETFONT with a font the dialog keeps alive while the edit exists.
    unsafe {
        SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.handle() as usize)),
            Some(LPARAM(0)),
        );
        SendMessageW(
            h,
            EM_SETMARGINS,
            Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)),
            Some(LPARAM(0)),
        );
        let _ = SetWindowSubclass(h, Some(edit_subclass), 1, 0);
    }
    Some(h)
}

/// Swallow the beep a single-line edit makes for Return/Escape (the
/// top-level window handles them through WM_APP_KEY).
unsafe extern "system" fn edit_subclass(
    h: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_CHAR && matches!(wp.0 as u16, 0x0D | 0x1B | 0x09) {
        return LRESULT(0);
    }
    if msg == WM_GETDLGCODE {
        // SAFETY: default processing of the edit control.
        let r = unsafe { DefSubclassProc(h, msg, wp, lp) };
        return LRESULT(r.0 | (DLGC_WANTALLKEYS as isize));
    }
    // SAFETY: as above.
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

pub fn set_font(h: HWND, font: isize) {
    // SAFETY: WM_SETFONT with a font the owner keeps alive.
    unsafe {
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font as usize)), Some(LPARAM(1)));
    }
}

pub fn get_text(h: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length are passed together.
    let n = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

pub fn select_all(h: HWND) {
    // SAFETY: EM_SETSEL on our edit control.
    unsafe {
        SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

pub fn move_child(h: HWND, x: i32, y: i32, w: i32, hgt: i32) {
    // SAFETY: child window of ours.
    unsafe {
        let _ = MoveWindow(h, x, y, w, hgt, true);
    }
}

/// The RECT an lparam points to (WM_DPICHANGED's suggested rectangle).
pub fn rect_from_lparam(lp: isize) -> Option<Rect> {
    // SAFETY: for WM_DPICHANGED, lparam points to a RECT valid during the
    // message; it is copied out immediately.
    unsafe { (lp as *const RECT).as_ref().map(|r| (*r).into()) }
}

/// The child window of a WM_COMMAND (lparam) and its notification code.
pub fn command_parts(wp: usize, lp: isize) -> (HWND, u16, u16) {
    (HWND(lp as *mut c_void), (wp >> 16) as u16, wp as u16)
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

pub fn set_clipboard_text(owner: HWND, text: &str) -> bool {
    let units: Vec<u16> = wide(text);
    let bytes = units.len() * 2;
    // SAFETY: standard clipboard sequence; the global memory is handed to the
    // clipboard on success and freed by us on failure.
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard().is_ok()
            && let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes)
        {
            let p = GlobalLock(mem);
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), p.cast::<u16>(), units.len());
                let _ = GlobalUnlock(mem);
                ok = SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(mem.0))).is_ok();
            }
            if !ok {
                let _ = GlobalFree(Some(HGLOBAL(mem.0)));
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

fn wall(st: SYSTEMTIME) -> WallTime {
    WallTime {
        year: i32::from(st.wYear),
        month: u32::from(st.wMonth),
        day: u32::from(st.wDay),
        hour: u32::from(st.wHour),
        minute: u32::from(st.wMinute),
        second: u32::from(st.wSecond),
        millis: u32::from(st.wMilliseconds),
    }
}

pub fn local_now() -> WallTime {
    // SAFETY: no arguments.
    wall(unsafe { GetLocalTime() })
}

pub fn utc_now() -> WallTime {
    // SAFETY: no arguments.
    wall(unsafe { GetSystemTime() })
}

/// One Windows time zone.
#[derive(Clone)]
pub struct TzInfo(DYNAMIC_TIME_ZONE_INFORMATION);

impl TzInfo {
    pub fn key(&self) -> String {
        let k = &self.0.TimeZoneKeyName;
        let n = k.iter().position(|&c| c == 0).unwrap_or(k.len());
        String::from_utf16_lossy(&k[..n])
    }
    /// Standard-time offset in minutes.
    pub fn standard_offset(&self) -> i32 {
        -(self.0.Bias + self.0.StandardBias)
    }
}

/// Every zone EnumDynamicTimeZoneInformation lists (capped at 1000).
pub fn enum_time_zones() -> Vec<TzInfo> {
    let mut out = Vec::new();
    for index in 0..1000 {
        let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: one struct out.
        let r = unsafe { EnumDynamicTimeZoneInformation(index, &mut info) };
        if r != 0 {
            break;
        }
        if info.TimeZoneKeyName[0] != 0 {
            out.push(TzInfo(info));
        }
    }
    out
}

pub fn current_time_zone() -> Option<TzInfo> {
    let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
    // SAFETY: one struct out.
    let r = unsafe { GetDynamicTimeZoneInformation(&mut info) };
    (r != u32::MAX && info.TimeZoneKeyName[0] != 0).then_some(TzInfo(info))
}

/// UTC -> wall clock in `zone`, with Windows' dynamic DST rules.
pub fn utc_to_zone(zone: &TzInfo, utc: WallTime) -> Option<WallTime> {
    let u = SYSTEMTIME {
        wYear: utc.year as u16,
        wMonth: utc.month as u16,
        wDayOfWeek: 0,
        wDay: utc.day as u16,
        wHour: utc.hour as u16,
        wMinute: utc.minute as u16,
        wSecond: utc.second as u16,
        wMilliseconds: 0,
    };
    let mut local = SYSTEMTIME::default();
    // SAFETY: two SYSTEMTIMEs and one zone struct.
    unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(&zone.0), &u, &mut local).ok()? };
    Some(wall(local))
}

// ---------------------------------------------------------------------------
// Single instance and hand-over
// ---------------------------------------------------------------------------

/// An owned kernel handle (mutex or event).
pub struct NamedMutex(HANDLE);
pub struct NamedEvent(HANDLE);

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl Drop for NamedEvent {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub enum MutexResult {
    Acquired(NamedMutex),
    AlreadyExists,
    Failed,
}

pub fn create_mutex(name: &str) -> MutexResult {
    let n = wide(name);
    // SAFETY: NUL-terminated name, default security.
    unsafe {
        match CreateMutexW(None, false, pcw(&n)) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(h);
                    MutexResult::AlreadyExists
                } else {
                    MutexResult::Acquired(NamedMutex(h))
                }
            }
            Err(_) => MutexResult::Failed,
        }
    }
}

/// Open or create the named auto-reset event.
pub fn create_event(name: &str) -> Option<NamedEvent> {
    let n = wide(name);
    // SAFETY: as above.
    unsafe {
        CreateEventW(None, false, false, pcw(&n))
            .ok()
            .map(NamedEvent)
    }
}

impl NamedEvent {
    pub fn set(&self) -> bool {
        // SAFETY: we own the handle.
        unsafe { SetEvent(self.0).is_ok() }
    }
}

// ---------------------------------------------------------------------------
// Settings folder and single instance with hand-over
// ---------------------------------------------------------------------------

/// %LOCALAPPDATA%\WeekNumber (the user's AppData\Local folder).
pub fn settings_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("AppData\\Local"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("WeekNumber")
}

/// Eviction hand-over budget for a launch: wait up to 5 s for the running
/// copy to quit, checking every 100 ms.
const HANDOVER_TIMEOUT_MS: u64 = 5000;
const HANDOVER_POLL_MS: u64 = 100;

/// This copy's claim on being the only one running: the single-instance
/// mutex, and the event a newer launch sets to ask it to quit.
pub struct Instance {
    _mutex: Option<NamedMutex>,
    quit: Option<NamedEvent>,
}

impl Instance {
    /// Take the single-instance mutex, asking a running copy to quit
    /// first. None when the running copy did not hand over in time.
    pub fn claim() -> Option<Instance> {
        let tag = object_tag();
        let name = format!("Local\\WeekNumber_{tag}_SingleInstance");
        let event_name = format!("Local\\WeekNumber_{tag}_QuitEvent");
        let mutex = match create_mutex(&name) {
            MutexResult::Acquired(m) => Some(m),
            // Fail open: a broken guard must not brick the app.
            MutexResult::Failed => None,
            MutexResult::AlreadyExists => {
                if let Some(event) = create_event(&event_name) {
                    event.set();
                }
                let start = tick_ms();
                let mut got = None;
                while got.is_none() && tick_ms() - start < HANDOVER_TIMEOUT_MS {
                    std::thread::sleep(std::time::Duration::from_millis(HANDOVER_POLL_MS));
                    if let MutexResult::Acquired(m) = create_mutex(&name) {
                        got = Some(m);
                    }
                }
                if got.is_none() {
                    message_box_error(
                        "WeekNumber is already running and did not respond in time. Quit it \
                         from Task Manager, then relaunch.",
                    );
                    return None;
                }
                got
            }
        };
        Some(Instance {
            _mutex: mutex,
            quit: create_event(&event_name),
        })
    }
}

/// Per-user, per-install names for the mutex and the hand-over event: the
/// same SHA-256 tag the Python version derives (domain, user, home, exe).
fn object_tag() -> String {
    let exe = exe_path()
        .and_then(|p| final_path(&p))
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let seed = [
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default(),
        std::env::var("USERPROFILE").unwrap_or_default(),
        exe,
    ]
    .join("|");
    crate::sha256::hex_prefix(seed.as_bytes(), 16)
}

// ---------------------------------------------------------------------------
// Registry: HKCU Run key and Task Manager's StartupApproved state
// ---------------------------------------------------------------------------

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Where Task Manager keeps its enabled/disabled mark for an autostart entry.
#[derive(Clone, Copy)]
pub enum Approved {
    /// Entries under HKCU\...\Run.
    Run,
    /// Shortcuts in the Startup folder.
    StartupFolder,
}

impl Approved {
    fn key(self) -> PCWSTR {
        match self {
            Approved::Run => {
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run")
            }
            Approved::StartupFolder => w!(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder"
            ),
        }
    }
}

/// A REG_SZ value under HKCU\...\Run; None when absent, of any other type
/// (a tampered REG_EXPAND_SZ counts as foreign) or longer than 1023 chars.
pub fn run_get(name: &str) -> Option<String> {
    let n = wide(name);
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: buffer and its byte size passed together; REG_SZ only.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            pcw(&n),
            RRF_RT_REG_SZ | RRF_NOEXPAND,
            Some(&mut kind),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if r.is_err() || kind != REG_SZ {
        return None;
    }
    let units = (bytes as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..units]);
    Some(s.trim_end_matches('\0').to_string())
}

/// Write `command` as a REG_SZ value under HKCU\...\Run.
pub fn run_set(name: &str, command: &str) -> bool {
    let n = wide(name);
    let c = wide(command);
    // SAFETY: NUL-terminated name; the value is the NUL-terminated UTF-16
    // string with its exact byte length.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            pcw(&n),
            REG_SZ.0,
            Some(c.as_ptr().cast()),
            (c.len() * 2) as u32,
        )
        .is_ok()
    }
}

/// Delete a value under HKCU\...\Run; true when it is gone afterwards.
pub fn run_delete(name: &str) -> bool {
    let n = wide(name);
    // SAFETY: NUL-terminated names.
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, pcw(&n)) };
    r.is_ok() || r == ERROR_FILE_NOT_FOUND
}

/// Task Manager's state for an entry (REG_BINARY), if any.
pub fn approved_get(key: Approved, name: &str) -> Option<Vec<u8>> {
    let n = wide(name);
    let mut buf = [0u8; 64];
    let mut bytes = buf.len() as u32;
    // SAFETY: buffer and its size passed together; REG_BINARY only.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.key(),
            pcw(&n),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    r.is_ok()
        .then(|| buf[..(bytes as usize).min(buf.len())].to_vec())
}

pub fn approved_set(key: Approved, name: &str, data: &[u8]) -> bool {
    let n = wide(name);
    // SAFETY: NUL-terminated name and a byte slice with its length.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.key(),
            pcw(&n),
            REG_BINARY.0,
            Some(data.as_ptr().cast()),
            data.len() as u32,
        )
        .is_ok()
    }
}

pub fn approved_delete(key: Approved, name: &str) {
    let n = wide(name);
    // SAFETY: NUL-terminated names.
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.key(), pcw(&n));
    }
}

// ---------------------------------------------------------------------------
// Files, folders and the Startup shortcut
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum KnownFolder {
    Startup,
    Desktop,
    Downloads,
    Public,
    OneDrive,
}

pub fn known_folder(f: KnownFolder) -> Option<PathBuf> {
    let id = match f {
        KnownFolder::Startup => &FOLDERID_Startup,
        KnownFolder::Desktop => &FOLDERID_Desktop,
        KnownFolder::Downloads => &FOLDERID_Downloads,
        KnownFolder::Public => &FOLDERID_Public,
        KnownFolder::OneDrive => &FOLDERID_SkyDrive,
    };
    // SAFETY: the returned string is copied, then freed with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s.map(PathBuf::from)
    }
}

pub fn temp_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    // SAFETY: buffer and length passed together.
    let n = unsafe { windows::Win32::Storage::FileSystem::GetTempPathW(Some(&mut buf)) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

pub fn windows_dir() -> PathBuf {
    let mut buf = vec![0u16; 32768];
    // SAFETY: buffer and length passed together.
    let n =
        unsafe { windows::Win32::System::SystemInformation::GetWindowsDirectoryW(Some(&mut buf)) }
            as usize;
    if n > 0 && n < buf.len() {
        PathBuf::from(String::from_utf16_lossy(&buf[..n]))
    } else {
        PathBuf::from("C:\\Windows")
    }
}

pub const DRIVE_REMOVABLE_: u32 = 2;
pub const DRIVE_REMOTE_: u32 = 4;
pub const DRIVE_CDROM_: u32 = 5;

pub fn drive_type(root: &Path) -> u32 {
    let r = wide(&root.to_string_lossy());
    // SAFETY: NUL-terminated path.
    unsafe { GetDriveTypeW(pcw(&r)) }
}

/// The canonical path of an existing file or folder (links resolved,
/// normalised, long form), or None if it does not exist.
pub fn final_path(p: &Path) -> Option<PathBuf> {
    let name = wide(&p.to_string_lossy());
    // SAFETY: opened for attribute reading only and closed before return.
    unsafe {
        let h = CreateFileW(
            pcw(&name),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .ok()?;
        let mut buf = vec![0u16; 32768];
        let n = GetFinalPathNameByHandleW(h, &mut buf, FILE_NAME_NORMALIZED) as usize;
        let _ = CloseHandle(h);
        if n == 0 || n >= buf.len() {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..n]);
        // \\?\C:\x -> C:\x ; \\?\UNC\server\share -> \\server\share
        let s = if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{rest}")
        } else {
            s.strip_prefix("\\\\?\\").map(str::to_string).unwrap_or(s)
        };
        Some(PathBuf::from(s))
    }
}

/// The target path stored in a shortcut, without resolving or searching
/// (no network access, no UI).
pub fn read_shortcut(lnk: &Path) -> Option<PathBuf> {
    let l = wide(&lnk.to_string_lossy());
    // SAFETY: as above; the path buffer is sized for the call.
    unsafe {
        let link =
            CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file = link.cast::<IPersistFile>().ok()?;
        file.Load(pcw(&l), STGM_READ).ok()?;
        let mut buf = vec![0u16; 32768];
        // SLGP_RAWPATH: the stored path, environment variables unexpanded.
        link.GetPath(&mut buf, std::ptr::null_mut(), 0x4).ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}
