//! The macOS backend (Apple silicon): AppKit windows driven through the
//! Objective-C runtime, a menu bar status item with the native menu, Core
//! Text, and the calendar on Liquid Glass (NSGlassEffectView on macOS 26,
//! the popover material before it).
//!
//! The app's windows paint into back buffers as everywhere else; each
//! window's view shows the finished buffer as its layer's contents. Native
//! events become the Win32 messages the windows handle. Coordinates the app
//! sees are pixels, top-left based, at the backing scale of the screen a
//! window is on (96 DPI x the scale factor).
#![allow(unsafe_code)]

mod objc;
mod text;

pub use text::Font;

use super::unix::{self, HWND, Kind, core, wm};
use objc::{
    CGPoint, CGRect, CGSize, ClassBuilder, Id, NIL, Sel, class, send0, send1, send2, send3, send4,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub use super::unix::{
    Brush, Cursor, Dc, EN_CHANGE_, EN_KILLFOCUS_, EN_SETFOCUS_, FontMetrics, Instance, MonitorGeom,
    Quality, Rect, SM_CXEDGE_, SM_CXICON_, SM_CXSMICON_, Surface, TrayOp, TzInfo, VK_CONTROL_,
    VK_ESCAPE_, VK_RETURN_, VK_SHIFT_, VK_TAB_, WM_APP_KEY, WM_TRAY, WS_DIALOG, WS_EX_NOACTIVATE_,
    WS_EX_TOOLWINDOW_, WS_EX_TOPMOST_, WS_POPUP_, WS_RESIZABLE, WallTime, WindowSpec,
    app_icon_image, command_parts, current_time_zone, enum_time_zones, is_null, local_now,
    rect_from_lparam, utc_now, utc_to_zone,
};

const IVAR: &std::ffi::CStr = c"wnHwnd";

/// WM_TRAY lparam: the status menu is about to open.
const TRAY_MENU_REFRESH: isize = 0x7F00;

struct MacWin {
    window: Id,
    view: Id,
    delegate: Id,
    kind: Kind,
    glass: bool,
    cursor: Cursor,
}

struct Mac {
    windows: HashMap<HWND, MacWin>,
    edits: HashMap<HWND, (HWND, u16, Id)>,
    target: Id,
    status_item: Id,
    status_menu: Id,
    tray_hwnd: Option<HWND>,
    menu_items: Vec<(u32, String, Option<bool>)>,
    pointer_in: Option<HWND>,
    scroll: f64,
    chosen: Option<u32>,
}

thread_local! {
    static MAC: RefCell<Option<Mac>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Mac) -> R) -> Option<R> {
    MAC.with(|m| m.try_borrow_mut().ok()?.as_mut().map(f))
}

static HANDOVER: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Start-up and classes
// ---------------------------------------------------------------------------

pub fn harden_process() {}
pub fn init_com() {}

fn hwnd_of(obj: Id) -> HWND {
    HWND(objc::get_ivar(obj, IVAR))
}

/// Define the AppKit subclasses once: the view that turns events into
/// messages, a panel that can take the keyboard, the window delegate and
/// the target for menus, the status item and text fields.
fn define_classes() {
    // SAFETY: every method below is an extern "C" fn whose prototype
    // matches the type encoding given with it.
    unsafe {
        if let Some(mut v) = ClassBuilder::new(c"WNView", c"NSView") {
            v.ivar_usize(IVAR);
            v.method(c"isFlipped", view_yes as *const c_void, c"B@:");
            v.method(c"acceptsFirstResponder", view_yes as *const c_void, c"B@:");
            v.method(c"acceptsFirstMouse:", view_yes1 as *const c_void, c"B@:@");
            v.method(c"wantsUpdateLayer", view_yes as *const c_void, c"B@:");
            v.method(c"updateLayer", view_update_layer as *const c_void, c"v@:");
            v.method(c"mouseDown:", view_mouse_down as *const c_void, c"v@:@");
            v.method(c"mouseUp:", view_mouse_up as *const c_void, c"v@:@");
            v.method(
                c"rightMouseDown:",
                view_right_down as *const c_void,
                c"v@:@",
            );
            v.method(c"rightMouseUp:", view_right_up as *const c_void, c"v@:@");
            v.method(c"mouseMoved:", view_mouse_moved as *const c_void, c"v@:@");
            v.method(c"mouseDragged:", view_mouse_moved as *const c_void, c"v@:@");
            v.method(c"mouseExited:", view_mouse_exited as *const c_void, c"v@:@");
            v.method(c"scrollWheel:", view_scroll as *const c_void, c"v@:@");
            v.method(c"keyDown:", view_key_down as *const c_void, c"v@:@");
            v.method(
                c"updateTrackingAreas",
                view_update_tracking as *const c_void,
                c"v@:",
            );
            v.register();
        }
        if let Some(mut p) = ClassBuilder::new(c"WNPanel", c"NSPanel") {
            p.method(c"canBecomeKeyWindow", view_yes as *const c_void, c"B@:");
            p.method(c"canBecomeMainWindow", view_no as *const c_void, c"B@:");
            p.register();
        }
        if let Some(mut d) = ClassBuilder::new(c"WNDelegate", c"NSObject") {
            d.ivar_usize(IVAR);
            d.protocol(c"NSWindowDelegate");
            d.method(
                c"windowDidBecomeKey:",
                win_became_key as *const c_void,
                c"v@:@",
            );
            d.method(
                c"windowDidResignKey:",
                win_resigned_key as *const c_void,
                c"v@:@",
            );
            d.method(
                c"windowShouldClose:",
                win_should_close as *const c_void,
                c"B@:@",
            );
            d.method(
                c"windowDidResize:",
                win_did_resize as *const c_void,
                c"v@:@",
            );
            d.method(
                c"windowDidChangeBackingProperties:",
                win_backing as *const c_void,
                c"v@:@",
            );
            d.register();
        }
        if let Some(mut t) = ClassBuilder::new(c"WNTarget", c"NSObject") {
            t.protocol(c"NSMenuDelegate");
            t.protocol(c"NSTextFieldDelegate");
            t.method(c"trayClick:", tray_click as *const c_void, c"v@:@");
            t.method(c"menuItem:", menu_item as *const c_void, c"v@:@");
            t.method(c"menuPick:", menu_pick as *const c_void, c"v@:@");
            t.method(
                c"menuNeedsUpdate:",
                menu_needs_update as *const c_void,
                c"v@:@",
            );
            t.method(
                c"controlTextDidChange:",
                text_changed as *const c_void,
                c"v@:@",
            );
            t.method(
                c"controlTextDidBeginEditing:",
                text_began as *const c_void,
                c"v@:@",
            );
            t.method(
                c"controlTextDidEndEditing:",
                text_ended as *const c_void,
                c"v@:@",
            );
            t.method(
                c"control:textView:doCommandBySelector:",
                text_command as *const c_void,
                c"B@:@@:",
            );
            t.method(c"wake:", wake_noop as *const c_void, c"v@:@");
            t.register();
        }
    }
}

pub fn register_class(handler: core::Handler) -> bool {
    core::register_class(handler);
    core::set_filter(filter);
    if with(|_| ()).is_some() {
        return true;
    }
    define_classes();
    // SAFETY: the shared application, created on the main thread; policy
    // Accessory (1): no Dock icon, as a menu bar app.
    unsafe {
        let app: Id = send0(class(c"NSApplication"), c"sharedApplication");
        let _: bool = send1(app, c"setActivationPolicy:", 1isize);
        let _: () = send0(app, c"finishLaunching");
    }
    let target = objc::new(c"WNTarget");
    MAC.with(|m| {
        *m.borrow_mut() = Some(Mac {
            windows: HashMap::new(),
            edits: HashMap::new(),
            target,
            status_item: NIL,
            status_menu: NIL,
            tray_hwnd: None,
            menu_items: Vec::new(),
            pointer_in: None,
            scroll: 0.0,
            chosen: None,
        })
    });
    true
}

/// ~/Library/Application Support/WeekNumber.
pub fn settings_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/WeekNumber")
}

impl Instance {
    pub fn claim() -> Option<Instance> {
        // $TMPDIR is a per-user, private folder on macOS.
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("weeknumber"));
        let dir = base.and_then(|b| unix::instance::private_dir(&b));
        Instance::claim_in(dir, &unix::instance::tag(), || {
            message_box_error(
                "WeekNumber is already running and did not respond in time. Quit it, then open WeekNumber again.",
            );
        })
    }
}

// ---------------------------------------------------------------------------
// Screens and coordinates
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Screen {
    frame: CGRect,
    visible: CGRect,
    scale: f64,
}

fn screens() -> Vec<Screen> {
    // SAFETY: NSScreen class methods and properties; the array is
    // autoreleased and read at once.
    unsafe {
        let list: Id = send0(class(c"NSScreen"), c"screens");
        let n: usize = send0(list, c"count");
        (0..n)
            .map(|i| {
                let s: Id = send1(list, c"objectAtIndex:", i);
                Screen {
                    frame: send0(s, c"frame"),
                    visible: send0(s, c"visibleFrame"),
                    scale: send0::<f64>(s, c"backingScaleFactor").clamp(1.0, 4.0),
                }
            })
            .collect()
    }
}

/// The primary screen's height in points (Cocoa's y axis points up from
/// its bottom edge).
fn primary_height(list: &[Screen]) -> f64 {
    list.first().map_or(900.0, |s| s.frame.size.height)
}

/// A Cocoa rectangle (points) -> top-left pixels at `scale`.
fn rect_to_px(r: CGRect, h: f64, scale: f64) -> Rect {
    let left = r.origin.x * scale;
    let top = (h - r.origin.y - r.size.height) * scale;
    Rect {
        left: left.round() as i32,
        top: top.round() as i32,
        right: (left + r.size.width * scale).round() as i32,
        bottom: (top + r.size.height * scale).round() as i32,
    }
}

/// The screen whose pixel rectangle holds (x, y), and its pixel rect.
fn screen_at_px(x: i32, y: i32) -> (Screen, f64) {
    let list = screens();
    let h = primary_height(&list);
    let found = list.iter().find(|s| {
        let r = rect_to_px(s.frame, h, s.scale);
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    });
    (
        found.or(list.first()).copied().unwrap_or(Screen {
            frame: CGRect::new(0.0, 0.0, 1440.0, 900.0),
            visible: CGRect::new(0.0, 0.0, 1440.0, 875.0),
            scale: 2.0,
        }),
        h,
    )
}

/// Top-left pixel rectangle -> Cocoa frame in points on its screen.
fn px_to_rect(x: i32, y: i32, w: i32, hgt: i32) -> (CGRect, f64) {
    let (s, h) = screen_at_px(x, y);
    let sc = s.scale;
    (
        CGRect::new(
            f64::from(x) / sc,
            h - f64::from(y) / sc - f64::from(hgt) / sc,
            f64::from(w) / sc,
            f64::from(hgt) / sc,
        ),
        sc,
    )
}

fn mouse_location_pt() -> CGPoint {
    // SAFETY: a class method returning the pointer position in points.
    unsafe { send0(class(c"NSEvent"), c"mouseLocation") }
}

/// Where the calendar hangs from: the bottom centre of the menu bar item,
/// as native menu bar pop-overs do, on whichever display's menu bar it
/// was clicked. Where the system does not say where the item is (macOS 26
/// hosts it elsewhere): the pointer when it is in a menu bar right of the
/// app menus (a click on the item), else the right end of the menu bar of the display under the
/// pointer, where menu bar items sit.
pub fn tray_anchor() -> Option<(i32, i32)> {
    let list = screens();
    let item = with(|m| m.status_item).filter(|i| !i.is_null());
    // SAFETY: the status item's button and its window, read on the main
    // thread.
    let frame: Option<CGRect> = item.and_then(|item| unsafe {
        let button: Id = send0(item, c"button");
        if button.is_null() {
            return None;
        }
        let win: Id = send0(button, c"window");
        if win.is_null() {
            return None;
        }
        Some(send0(win, c"frame"))
    });
    menu_bar_anchor(&list, frame, mouse_location_pt())
}

/// The screen holding a point (Cocoa points); the right and top edges
/// count, as the menu bar's top row and a pointer pressed to the edge
/// report them.
fn screen_of(list: &[Screen], p: CGPoint) -> Option<&Screen> {
    list.iter().find(|s| {
        let f = s.frame;
        p.x >= f.origin.x
            && p.x <= f.origin.x + f.size.width
            && p.y >= f.origin.y
            && p.y <= f.origin.y + f.size.height
    })
}

/// A screen's top edge and the height of its menu bar (points).
fn menu_bar(s: &Screen) -> (f64, f64) {
    let top = s.frame.origin.y + s.frame.size.height;
    let bar = top - (s.visible.origin.y + s.visible.size.height);
    (top, if bar > 0.0 { bar } else { 24.0 })
}

/// tray_anchor's choice, in pixels, from the screens, the status item's
/// window frame (if any) and the pointer (Cocoa points).
fn menu_bar_anchor(list: &[Screen], item: Option<CGRect>, pointer: CGPoint) -> Option<(i32, i32)> {
    let h = primary_height(list);
    let to_px = |s: &Screen, x: f64, y: f64| {
        (
            (x * s.scale).round() as i32,
            ((h - y) * s.scale).round() as i32,
        )
    };
    // A real item sits in a menu bar, right of the app menus (an item the
    // system could not fit reports itself at the far left).
    if let Some(f) = item.filter(|f| f.size.width > 0.0) {
        let centre = CGPoint {
            x: f.origin.x + f.size.width / 2.0,
            y: f.origin.y + f.size.height / 2.0,
        };
        if let Some(s) = screen_of(list, centre) {
            let (top, bar) = menu_bar(s);
            if (f.origin.y + f.size.height - top).abs() <= bar
                && f.origin.x - s.frame.origin.x > s.frame.size.width * 0.3
            {
                return Some(to_px(s, centre.x, f.origin.y));
            }
        }
    }
    // The pointer, where a click on the item leaves it: in a menu bar,
    // right of the app menus (not a pointer resting in the top left
    // corner while the calendar opens some other way).
    let s = screen_of(list, pointer).or(list.first())?;
    let (top, bar) = menu_bar(s);
    if pointer.y >= top - bar
        && pointer.y <= top
        && pointer.x - s.frame.origin.x > s.frame.size.width * 0.3
    {
        return Some(to_px(s, pointer.x, top - bar));
    }
    Some(to_px(
        s,
        s.frame.origin.x + s.frame.size.width - 120.0,
        top - bar,
    ))
}

pub fn cursor_pos() -> (i32, i32) {
    let p = mouse_location_pt();
    let list = screens();
    let h = primary_height(&list);
    let s = list
        .iter()
        .find(|s| {
            let f = s.frame;
            p.x >= f.origin.x
                && p.x <= f.origin.x + f.size.width
                && p.y >= f.origin.y
                && p.y <= f.origin.y + f.size.height
        })
        .or(list.first())
        .map_or(2.0, |s| s.scale);
    ((p.x * s).round() as i32, ((h - p.y) * s).round() as i32)
}

/// The screen under the pointer, with its work area (below the menu bar,
/// beside the Dock).
pub fn cursor_monitor() -> MonitorGeom {
    let p = mouse_location_pt();
    let list = screens();
    let h = primary_height(&list);
    let s = list
        .iter()
        .find(|s| {
            let f = s.frame;
            p.x >= f.origin.x
                && p.x <= f.origin.x + f.size.width
                && p.y >= f.origin.y
                && p.y <= f.origin.y + f.size.height
        })
        .or(list.first())
        .copied()
        .unwrap_or(Screen {
            frame: CGRect::new(0.0, 0.0, 1440.0, 900.0),
            visible: CGRect::new(0.0, 0.0, 1440.0, 875.0),
            scale: 2.0,
        });
    MonitorGeom {
        monitor: rect_to_px(s.frame, h, s.scale),
        work: rect_to_px(s.visible, h, s.scale),
        dpi: (96.0 * s.scale).round() as u32,
    }
}

pub fn system_dpi() -> u32 {
    let list = screens();
    (96.0 * list.first().map_or(2.0, |s| s.scale)).round() as u32
}

fn window_scale(win: Id) -> f64 {
    if win.is_null() {
        return 2.0;
    }
    // SAFETY: an NSWindow property.
    unsafe { send0::<f64>(win, c"backingScaleFactor").clamp(1.0, 4.0) }
}

pub fn window_dpi(h: HWND) -> u32 {
    let win = with(|m| m.windows.get(&h).map(|w| w.window))
        .flatten()
        .unwrap_or(NIL);
    (96.0 * window_scale(win)).round() as u32
}

pub fn metric_for_dpi(index: i32, dpi: u32) -> i32 {
    let scale = |v: f64| (v * f64::from(dpi) / 96.0).round() as i32;
    match index {
        SM_CXSMICON_ => scale(16.0),
        SM_CXICON_ => scale(32.0),
        _ => system_metric(index),
    }
}

pub fn system_metric(index: i32) -> i32 {
    match index {
        // The status item image: 18 pt at the main screen's scale.
        SM_CXSMICON_ => (18.0 * f64::from(system_dpi()) / 96.0).round() as i32,
        SM_CXICON_ => metric_for_dpi(SM_CXICON_, system_dpi()),
        SM_CXEDGE_ => 2,
        36 | 37 => 4,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// The style mask for a dialog: titled, closable, miniaturizable (and
/// resizable).
fn dialog_mask(resizable: bool) -> usize {
    1 | 2 | 4 | if resizable { 8 } else { 0 }
}

/// Frame size around a content size, in pixels: the title bar's height.
pub fn outer_size(style: u32, _ex_style: u32, w: i32, h: i32, dpi: u32) -> (i32, i32, i32, i32) {
    if style & WS_POPUP_ != 0 {
        return (0, 0, w, h);
    }
    let resizable = style & WS_RESIZABLE == WS_RESIZABLE;
    let scale = f64::from(dpi) / 96.0;
    let content = CGRect::new(0.0, 0.0, f64::from(w) / scale, f64::from(h) / scale);
    // SAFETY: a class method computing a frame from a content rect.
    let frame: CGRect = unsafe {
        send2(
            class(c"NSWindow"),
            c"frameRectForContentRect:styleMask:",
            content,
            dialog_mask(resizable),
        )
    };
    let title = ((frame.size.height - content.size.height) * scale).round() as i32;
    (0, -title, w, h + title)
}

pub fn create_window(spec: &WindowSpec<'_>) -> Option<HWND> {
    let kind = Kind::of(spec);
    let h = core::new_hwnd();
    let (frame, scale) = px_to_rect(spec.x, spec.y, spec.w.max(1), spec.h.max(1));
    let calendar = kind == Kind::Popup && spec.title == crate::app::APP_NAME;
    // SAFETY: AppKit objects created and configured on the main thread;
    // each +1 reference is kept in MacWin and released in destroy_window.
    let (window, view, delegate, glass) = unsafe {
        let window: Id = match kind {
            Kind::Popup | Kind::Tip => {
                let p: Id = send0(class(c"WNPanel"), c"alloc");
                // Borderless (0) | non-activating panel (1 << 7).
                send4(
                    p,
                    c"initWithContentRect:styleMask:backing:defer:",
                    frame,
                    1usize << 7,
                    2usize,
                    false,
                )
            }
            Kind::Dialog { resizable } => {
                let content: CGRect = send2(
                    class(c"NSWindow"),
                    c"contentRectForFrameRect:styleMask:",
                    frame,
                    dialog_mask(resizable),
                );
                let w: Id = send0(class(c"NSWindow"), c"alloc");
                send4(
                    w,
                    c"initWithContentRect:styleMask:backing:defer:",
                    content,
                    dialog_mask(resizable),
                    2usize,
                    false,
                )
            }
        };
        if window.is_null() {
            return None;
        }
        let _: () = send1(window, c"setReleasedWhenClosed:", false);
        let _: () = send1(window, c"setAcceptsMouseMovedEvents:", true);
        let _: () = send1(window, c"setTitle:", objc::nsstring(spec.title));
        let delegate = objc::new(c"WNDelegate");
        objc::set_ivar(delegate, IVAR, h.0);
        let _: () = send1(window, c"setDelegate:", delegate);
        let content_rect: CGRect = send0(send0::<Id>(window, c"contentView"), c"frame");
        let view: Id = send0(class(c"WNView"), c"alloc");
        let view: Id = send1(view, c"initWithFrame:", content_rect);
        objc::set_ivar(view, IVAR, h.0);
        let _: () = send1(view, c"setWantsLayer:", true);
        // NSViewLayerContentsRedrawNever: the contents are ours.
        let _: () = send1(view, c"setLayerContentsRedrawPolicy:", 0isize);
        let mut glass = false;
        match kind {
            Kind::Popup | Kind::Tip => {
                let clear: Id = send0(class(c"NSColor"), c"clearColor");
                let _: () = send1(window, c"setBackgroundColor:", clear);
                let _: () = send1(window, c"setOpaque:", false);
                let _: () = send1(window, c"setHasShadow:", true);
                let _: () = send1(window, c"setHidesOnDeactivate:", false);
                // NSPopUpMenuWindowLevel (101); tooltips just above.
                let level: isize = if kind == Kind::Tip { 102 } else { 101 };
                let _: () = send1(window, c"setLevel:", level);
                if kind == Kind::Tip {
                    let _: () = send1(window, c"setIgnoresMouseEvents:", true);
                }
                // Visible on every Space and over full-screen apps.
                let _: () = send1(
                    window,
                    c"setCollectionBehavior:",
                    (1usize << 0) | (1usize << 8),
                );
                if calendar {
                    glass = true;
                    let container = glass_container(content_rect, scale);
                    let _: () = send1(container_content(container), c"addSubview:", view);
                    let _: () = send1(view, c"setAutoresizingMask:", 18usize);
                    let _: () = send1(window, c"setContentView:", container);
                    objc::release(container);
                } else {
                    let layer: Id = send0(view, c"layer");
                    let radius = if kind == Kind::Tip { 4.0 } else { 6.0 };
                    let _: () = send1(layer, c"setCornerRadius:", radius);
                    let _: () = send1(layer, c"setMasksToBounds:", true);
                    let _: () = send1(window, c"setContentView:", view);
                }
            }
            Kind::Dialog { .. } => {
                let _: () = send1(window, c"setContentView:", view);
            }
        }
        let layer: Id = send0(view, c"layer");
        let _: () = send1(layer, c"setContentsScale:", scale);
        (window, view, delegate, glass)
    };
    with(|m| {
        m.windows.insert(
            h,
            MacWin {
                window,
                view,
                delegate,
                kind,
                glass,
                cursor: Cursor::Arrow,
            },
        )
    });
    if kind == (Kind::Dialog { resizable: true }) {
        core::send(h, wm::WM_GETMINMAXINFO, 0, h.0 as isize);
    }
    Some(h)
}

/// The calendar's backdrop: Liquid Glass where the system has it
/// (NSGlassEffectView, macOS 26), else the popover material.
///
/// # Safety
/// Main thread only; returns a +1 view.
unsafe fn glass_container(frame: CGRect, scale: f64) -> Id {
    let radius = 16.0;
    let _ = scale;
    // SAFETY: class lookups at runtime; methods of the classes found.
    unsafe {
        let glass = class(c"NSGlassEffectView");
        if !glass.is_null() {
            let g: Id = send0(glass, c"alloc");
            let g: Id = send1(g, c"initWithFrame:", frame);
            let _: () = send1(g, c"setCornerRadius:", radius);
            let inner: Id = send0(class(c"NSView"), c"alloc");
            let inner: Id = send1(
                inner,
                c"initWithFrame:",
                CGRect::new(0.0, 0.0, frame.size.width, frame.size.height),
            );
            let _: () = send1(inner, c"setAutoresizingMask:", 18usize);
            let _: () = send1(g, c"setContentView:", inner);
            objc::release(inner);
            return g;
        }
        let v: Id = send0(class(c"NSVisualEffectView"), c"alloc");
        let v: Id = send1(v, c"initWithFrame:", frame);
        // NSVisualEffectMaterialPopover (6), behind window (0), active (1).
        let _: () = send1(v, c"setMaterial:", 6isize);
        let _: () = send1(v, c"setBlendingMode:", 0isize);
        let _: () = send1(v, c"setState:", 1isize);
        let _: () = send1(v, c"setWantsLayer:", true);
        let layer: Id = send0(v, c"layer");
        let _: () = send1(layer, c"setCornerRadius:", radius);
        let _: () = send1(layer, c"setMasksToBounds:", true);
        v
    }
}

/// Where the calendar's own view goes inside the backdrop.
///
/// # Safety
/// `container` is a view from glass_container.
unsafe fn container_content(container: Id) -> Id {
    // SAFETY: NSGlassEffectView has contentView; NSVisualEffectView is
    // itself the container.
    unsafe {
        let responds: bool = send1(container, c"respondsToSelector:", objc::sel(c"contentView"));
        if responds {
            let inner: Id = send0(container, c"contentView");
            if !inner.is_null() {
                return inner;
            }
        }
        container
    }
}

/// The glass's corner radius, in pixels of the window. Liquid Glass takes
/// it directly; the popover material is clipped by a stretchable mask
/// image, so the window's shadow follows the rounded corners too (a layer
/// corner radius would leave it square).
pub fn set_glass_radius(h: HWND, radius_px: f64) {
    let Some((win, _)) = window_of(h) else { return };
    // SAFETY: AppKit calls on the main thread; the mask image is released
    // once the view holds it.
    unsafe {
        let scale: f64 = send0(win, c"backingScaleFactor");
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let r = (radius_px / scale).max(1.0);
        let container: Id = send0(win, c"contentView");
        if container.is_null() {
            return;
        }
        let glass: bool = send1(
            container,
            c"respondsToSelector:",
            objc::sel(c"setCornerRadius:"),
        );
        if glass {
            let _: () = send1(container, c"setCornerRadius:", r);
        } else if let Some(mask) = rounded_mask(r, scale) {
            let layer: Id = send0(container, c"layer");
            if !layer.is_null() {
                let _: () = send1(layer, c"setCornerRadius:", 0.0f64);
                let _: () = send1(layer, c"setMasksToBounds:", false);
            }
            let _: () = send1(container, c"setMaskImage:", mask);
            objc::release(mask);
        }
        let _: () = send0(win, c"invalidateShadow");
    }
}

/// A (2r + 1)-point rounded square whose cap insets keep the corners when
/// it stretches over the view: the documented way to round a visual effect
/// view. +1 NSImage.
///
/// # Safety
/// Main thread only.
unsafe fn rounded_mask(r: f64, scale: f64) -> Option<Id> {
    let side_pt = 2.0 * r.ceil() + 1.0;
    let n = (side_pt * scale).round() as usize;
    let rp = r * scale;
    let mut bytes = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            // Coverage of the rounded square, 4x4 samples per pixel.
            let mut inside = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f64 + (f64::from(sx) + 0.5) / 4.0;
                    let fy = y as f64 + (f64::from(sy) + 0.5) / 4.0;
                    let cx = fx.clamp(rp, n as f64 - rp);
                    let cy = fy.clamp(rp, n as f64 - rp);
                    if (fx - cx).powi(2) + (fy - cy).powi(2) <= rp * rp {
                        inside += 1;
                    }
                }
            }
            let a = (inside * 255 / 16) as u8;
            bytes.extend_from_slice(&[0, 0, 0, a]); // premultiplied black
        }
    }
    // SAFETY: the bytes are copied into a CFData the image keeps.
    unsafe {
        let data = objc::CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
        let provider = objc::CGDataProviderCreateWithCFData(data);
        let cs = objc::CGColorSpaceCreateDeviceRGB();
        // Byte order 32 little with premultiplied-first alpha: BGRA.
        let image = objc::CGImageCreate(
            n,
            n,
            8,
            32,
            n * 4,
            cs,
            (2 << 12) | 2,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        objc::CGColorSpaceRelease(cs);
        objc::CGDataProviderRelease(provider);
        objc::cf_release(data);
        if image.is_null() {
            return None;
        }
        let ns: Id = send0(class(c"NSImage"), c"alloc");
        let size = objc::CGSize {
            width: side_pt,
            height: side_pt,
        };
        let ns: Id = send2(ns, c"initWithCGImage:size:", image as Id, size);
        objc::CGImageRelease(image);
        if ns.is_null() {
            return None;
        }
        let insets = objc::NSEdgeInsets {
            top: r,
            left: r,
            bottom: r,
            right: r,
        };
        let _: () = send1(ns, c"setCapInsets:", insets);
        Some(ns)
    }
}

/// Whether `h` shows its content on glass (the calendar on macOS).
pub fn is_glass(h: HWND) -> bool {
    with(|m| m.windows.get(&h).is_some_and(|w| w.glass)).unwrap_or(false)
}

pub fn destroy_window(h: HWND) {
    if is_window(h) {
        core::send(h, wm::WM_DESTROY, 0, 0);
    }
    let taken = with(|m| {
        if m.pointer_in == Some(h) {
            m.pointer_in = None;
        }
        let edits: Vec<HWND> = m
            .edits
            .iter()
            .filter(|(_, e)| e.0 == h)
            .map(|(k, _)| *k)
            .collect();
        let fields: Vec<Id> = edits
            .iter()
            .filter_map(|e| m.edits.remove(e))
            .map(|e| e.2)
            .collect();
        (m.windows.remove(&h), fields)
    });
    if let Some((win, fields)) = taken {
        for f in fields {
            objc::release(f);
        }
        if let Some(w) = win {
            // SAFETY: detach the delegate before closing so no callbacks
            // arrive for a window the app has forgotten; then release
            // what create_window retained.
            unsafe {
                let _: () = send1(w.window, c"setDelegate:", NIL);
                let _: () = send1(w.window, c"orderOut:", NIL);
                let _: () = send0(w.window, c"close");
            }
            objc::release(w.view);
            objc::release(w.delegate);
            objc::release(w.window);
        }
    }
    core::forget(h);
}

pub fn is_window(h: HWND) -> bool {
    with(|m| m.windows.contains_key(&h)).unwrap_or(false)
}

/// The tray's message target: a handle with no window.
pub fn create_hidden_window() -> Option<HWND> {
    let h = core::new_hwnd();
    with(|m| m.tray_hwnd = Some(h))?;
    Some(h)
}

pub fn register_window_message(_name: &str) -> u32 {
    0
}

pub fn allow_message(_h: HWND, _msg: u32) {}

fn window_of(h: HWND) -> Option<(Id, Kind)> {
    with(|m| m.windows.get(&h).map(|w| (w.window, w.kind))).flatten()
}

pub fn show_window(h: HWND, activate: bool) {
    let Some((win, kind)) = window_of(h) else {
        return;
    };
    // SAFETY: ordering a window of this app on the main thread.
    unsafe {
        if activate && kind != Kind::Tip {
            if matches!(kind, Kind::Dialog { .. }) {
                activate_app();
            }
            let _: () = send1(win, c"makeKeyAndOrderFront:", NIL);
        } else {
            let _: () = send0(win, c"orderFrontRegardless");
        }
    }
}

fn activate_app() {
    // SAFETY: activates this app (a menu bar app has to, for its dialogs
    // to take the keyboard).
    unsafe {
        let app: Id = send0(class(c"NSApplication"), c"sharedApplication");
        let _: () = send1(app, c"activateIgnoringOtherApps:", true);
    }
}

pub fn restore_window(h: HWND) {
    let Some((win, _)) = window_of(h) else { return };
    // SAFETY: as above.
    unsafe {
        let mini: bool = send0(win, c"isMiniaturized");
        if mini {
            let _: () = send1(win, c"deminiaturize:", NIL);
        }
    }
}

pub fn set_foreground(h: HWND) -> bool {
    let Some((win, kind)) = window_of(h) else {
        return false;
    };
    if matches!(kind, Kind::Dialog { .. }) {
        activate_app();
    }
    // SAFETY: as above; becoming key reports WM_ACTIVATE via the delegate.
    unsafe {
        let _: () = send1(win, c"makeKeyAndOrderFront:", NIL);
    }
    true
}

pub fn set_focus(h: HWND) {
    if let Some((parent, field)) = with(|m| m.edits.get(&h).map(|e| (e.0, e.2))).flatten() {
        if let Some((win, _)) = window_of(parent) {
            // SAFETY: making a text field of this window first responder.
            unsafe {
                let _: bool = send1(win, c"makeFirstResponder:", field);
            }
        }
        return;
    }
    let Some(view) = with(|m| m.windows.get(&h).map(|w| (w.window, w.view))).flatten() else {
        return;
    };
    // SAFETY: the window's own view takes the keyboard.
    unsafe {
        let _: bool = send1(view.0, c"makeFirstResponder:", view.1);
    }
}

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
    if let Some((parent, field)) = with(|m| m.edits.get(&h).map(|e| (e.0, e.2))).flatten() {
        let scale = window_scale(window_of(parent).map_or(NIL, |p| p.0));
        // SAFETY: a subview frame in its flipped parent's points.
        unsafe {
            let r = CGRect::new(
                f64::from(x) / scale,
                f64::from(y) / scale,
                f64::from(w) / scale,
                f64::from(hgt) / scale,
            );
            let _: () = send1(field, c"setFrame:", r);
        }
        return;
    }
    let Some((win, kind)) = window_of(h) else {
        return;
    };
    let _ = topmost;
    // SAFETY: moving/resizing our window; frame in points on its screen.
    unsafe {
        let cur: CGRect = send0(win, c"frame");
        let list = screens();
        let ph = primary_height(&list);
        let scale = window_scale(win);
        let cur_px = rect_to_px(cur, ph, scale);
        let (nx, ny) = if no_move {
            (cur_px.left, cur_px.top)
        } else {
            (x, y)
        };
        let (nw, nh) = if no_size {
            (cur_px.right - cur_px.left, cur_px.bottom - cur_px.top)
        } else {
            (w, hgt)
        };
        let (frame, _) = px_to_rect(nx, ny, nw.max(1), nh.max(1));
        let _ = kind;
        let _: () = send2(win, c"setFrame:display:", frame, true);
    }
}

pub fn client_size(h: HWND) -> (i32, i32) {
    let Some((win, view)) = with(|m| m.windows.get(&h).map(|w| (w.window, w.view))).flatten()
    else {
        return (0, 0);
    };
    let scale = window_scale(win);
    // SAFETY: a view's bounds.
    let b: CGRect = unsafe { send0(view, c"bounds") };
    (
        (b.size.width * scale).round() as i32,
        (b.size.height * scale).round() as i32,
    )
}

fn client_origin(h: HWND) -> (i32, i32) {
    let Some((win, view)) = with(|m| m.windows.get(&h).map(|w| (w.window, w.view))).flatten()
    else {
        return (0, 0);
    };
    // SAFETY: converting the view's bounds to window, then screen points.
    unsafe {
        let b: CGRect = send0(view, c"bounds");
        let in_win: CGRect = send2(view, c"convertRect:toView:", b, NIL);
        let on_screen: CGRect = send1(win, c"convertRectToScreen:", in_win);
        let list = screens();
        let r = rect_to_px(on_screen, primary_height(&list), window_scale(win));
        (r.left, r.top)
    }
}

pub fn client_to_screen(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let (ox, oy) = client_origin(h);
    (ox + x, oy + y)
}

pub fn screen_to_client(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let (ox, oy) = client_origin(h);
    (x - ox, y - oy)
}

pub fn invalidate(h: HWND) {
    core::invalidate(h);
}

pub fn set_timer(h: HWND, id: usize, ms: u32) {
    core::set_timer(h, id, ms);
}

pub fn kill_timer(h: HWND, id: usize) {
    core::kill_timer(h, id);
}

/// A view keeps the mouse between press and release by itself.
pub fn capture(_h: HWND) {}
pub fn release_capture() {}

pub fn track_leave(h: HWND) {
    core::track_leave(h);
}

pub fn set_cursor(c: Cursor) {
    let changed = with(|m| {
        let h = m.pointer_in?;
        let w = m.windows.get_mut(&h)?;
        let changed = w.cursor != c;
        w.cursor = c;
        Some(changed)
    })
    .flatten();
    if changed.is_some() {
        let name: &std::ffi::CStr = match c {
            Cursor::Arrow => c"arrowCursor",
            Cursor::Hand => c"pointingHandCursor",
            Cursor::Cross => c"crosshairCursor",
        };
        // SAFETY: shared system cursors.
        unsafe {
            let cur: Id = send0(class(c"NSCursor"), name);
            let _: () = send0(cur, c"set");
        }
    }
}

/// Pop-ups already have rounded corners here (layer or glass).
pub fn round_corners(_h: HWND, _small: bool) {}

/// The light or dark appearance for a window (its title bar, and the
/// calendar's glass tint).
pub fn dark_title_bar(h: HWND, dark: bool) {
    let Some((win, _)) = window_of(h) else { return };
    let name = if dark {
        "NSAppearanceNameDarkAqua"
    } else {
        "NSAppearanceNameAqua"
    };
    // SAFETY: a named system appearance set on our window.
    unsafe {
        let ap: Id = send1(
            class(c"NSAppearance"),
            c"appearanceNamed:",
            objc::nsstring(name),
        );
        let _: () = send1(win, c"setAppearance:", ap);
    }
}

pub fn set_title(h: HWND, title: &str) {
    if let Some(field) = with(|m| m.edits.get(&h).map(|e| e.2)).flatten() {
        // SAFETY: a text field's string; EN_CHANGE follows, as for EDIT.
        unsafe {
            let _: () = send1(field, c"setStringValue:", objc::nsstring(title));
        }
        notify_edit(h, EN_CHANGE_);
        return;
    }
    if let Some((win, _)) = window_of(h) {
        // SAFETY: a window title.
        unsafe {
            let _: () = send1(win, c"setTitle:", objc::nsstring(title));
        }
    }
}

pub fn set_min_track_size(lparam: isize, w: i32, h: i32) {
    let hwnd = HWND(lparam as usize);
    let Some((win, _)) = window_of(hwnd) else {
        return;
    };
    let scale = window_scale(win);
    // SAFETY: the window's smallest frame size in points.
    unsafe {
        let _: () = send1(
            win,
            c"setMinSize:",
            CGSize {
                width: f64::from(w) / scale,
                height: f64::from(h) / scale,
            },
        );
    }
}

pub fn key_down(vk: u16) -> bool {
    core::key_down(vk)
}

pub fn double_click_ms() -> u64 {
    // SAFETY: a class property in seconds.
    let s: f64 = unsafe { send0(class(c"NSEvent"), c"doubleClickInterval") };
    (s * 1000.0).clamp(100.0, 2000.0) as u64
}

pub fn beep() {
    // SAFETY: no arguments.
    unsafe { objc::NSBeep() }
}

pub fn tick_ms() -> u64 {
    core::tick_ms()
}

pub fn post_quit() {
    core::post_quit();
}

pub fn message_box_error(text: &str) {
    activate_app();
    // SAFETY: a modal alert owned here and released after it closes.
    unsafe {
        let alert = objc::new(c"NSAlert");
        let _: () = send1(alert, c"setMessageText:", objc::nsstring("WeekNumber"));
        let _: () = send1(alert, c"setInformativeText:", objc::nsstring(text));
        // NSAlertStyleWarning (0).
        let _: () = send1(alert, c"setAlertStyle:", 0usize);
        let _: isize = send0(alert, c"runModal");
        objc::release(alert);
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// The window has been painted outside WM_PAINT: no repaint is owed.
pub fn validate(h: HWND) {
    core::validate(h);
}

pub fn paint(h: HWND, f: impl FnOnce(Dc<'_>)) {
    f(Dc::Window(h));
    core::validate(h);
}

/// Show pixels (0xAARRGGBB, premultiplied when `alpha`) in a view's layer.
fn set_layer_image(h: HWND, px: &[u32], w: usize, hgt: usize, alpha: bool) {
    let Some(view) = with(|m| m.windows.get(&h).map(|w| w.view)).flatten() else {
        return;
    };
    if px.len() < w * hgt || w == 0 || hgt == 0 {
        return;
    }
    // SAFETY: the pixel bytes are copied into a CFData owned by the image,
    // which the layer retains; every created object is released here.
    unsafe {
        let layer: Id = send0(view, c"layer");
        if layer.is_null() {
            return;
        }
        // Core Graphics images are bottom-up for a layer whose contents are
        // not flipped; our buffers are top-down.
        let flipped: bool = send0(layer, c"contentsAreFlipped");
        let mut bytes: Vec<u8> = Vec::with_capacity(w * hgt * 4);
        for row in 0..hgt {
            let r = if flipped { row } else { hgt - 1 - row };
            for p in &px[r * w..(r + 1) * w] {
                bytes.extend_from_slice(&p.to_le_bytes());
            }
        }
        let data = objc::CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
        let provider = objc::CGDataProviderCreateWithCFData(data);
        let cs = objc::CGColorSpaceCreateDeviceRGB();
        // Byte order 32 little (2 << 12) with premultiplied-first (2) or
        // none-skip-first (6) alpha: BGRA in memory.
        let info = (2 << 12) | if alpha { 2 } else { 6 };
        let image = objc::CGImageCreate(
            w,
            hgt,
            8,
            32,
            w * 4,
            cs,
            info,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        if !image.is_null() {
            let _: () = send1(layer, c"setContents:", image as Id);
            objc::CGImageRelease(image);
        }
        objc::CGColorSpaceRelease(cs);
        objc::CGDataProviderRelease(provider);
        objc::cf_release(data);
    }
}

pub fn present_pixels(h: HWND, px: &[u32], w: usize, hgt: usize) {
    let opaque: Vec<u32> = px.iter().map(|p| 0xFF00_0000 | p).collect();
    set_layer_image(h, &opaque, w, hgt, false);
}

/// The calendar on glass: the same frame drawn over black and over white
/// gives each pixel's coverage (white - black) and its colour (the black
/// rendering is the premultiplied colour); the glass shows through
/// wherever the card was.
pub fn present_matte(h: HWND, over_black: &mut Surface, over_white: &mut Surface) {
    let (w, hgt) = (over_black.w, over_black.h);
    if (over_white.w, over_white.h) != (w, hgt) {
        return;
    }
    let white: Vec<u32> = over_white.pixels().to_vec();
    let out: Vec<u32> = over_black
        .pixels()
        .iter()
        .zip(&white)
        .map(|(&b, &wpx)| matte(b, wpx))
        .collect();
    set_layer_image(h, &out, w, hgt, true);
}

/// One pixel of the difference matte, premultiplied 0xAARRGGBB.
pub fn matte(black: u32, white: u32) -> u32 {
    let ch = |v: u32, s: u32| (v >> s) & 0xFF;
    // Coverage from the channel that changed least (antialiased text
    // differs equally in all three).
    let diff = [16, 8, 0]
        .iter()
        .map(|&s| ch(white, s).saturating_sub(ch(black, s)))
        .max()
        .unwrap_or(0);
    let a = 255 - diff.min(255);
    let c = |s: u32| ch(black, s).min(a);
    (a << 24) | (c(16) << 16) | (c(8) << 8) | c(0)
}

// ---------------------------------------------------------------------------
// Fonts and icons
// ---------------------------------------------------------------------------

pub fn font_families() -> Vec<String> {
    text::families()
}

/// The text font for drop-down lists: the system font at 13 pt.
pub fn tk_text_font() -> Option<Font> {
    let px = (13.0 * f64::from(system_dpi()) / 96.0).round() as i32;
    Font::new("", px, false, Quality::Default)
}

pub fn glyph_outline(font: &Font, ch: char, _hinted: bool) -> Option<Vec<crate::gfx::Contour>> {
    font.outline_px(ch)
}

/// An icon: renderings of one picture at several pixel sizes.
#[derive(Clone)]
pub struct Icon {
    frames: Vec<crate::gfx::Rgba>,
}

pub fn icon_from_frames(frames: Vec<crate::gfx::Rgba>) -> Option<Icon> {
    (!frames.is_empty()).then_some(Icon { frames })
}

/// The status item image at 1x and 2x (18 pt).
/// The font size for the week number in a `size` px tray icon: the whole
/// icon (the digits then fill it).
pub fn tray_text_height(size: usize) -> usize {
    size
}

pub fn tray_icon_sizes(_size: usize) -> Vec<usize> {
    vec![18, 36]
}

pub fn app_icon(size: i32) -> Option<Icon> {
    let side = size.clamp(8, 256) as usize;
    let img = crate::png::ico_image(unix::APP_ICO, side as u32)?;
    let img = if img.w == side {
        img
    } else {
        img.resize(side, side, crate::gfx::Filter::Lanczos)
    };
    icon_from_frames(vec![img])
}

/// Windows on macOS show no icon in their title bar.
pub fn set_window_icons(_h: HWND, _small: &Icon, _big: &Icon) {}

/// An NSImage of `points` size from the icon's frames (+1). A picture in
/// pure white becomes a template image, which the menu bar tints to match
/// light and dark menu bars.
fn ns_image(icon: &Icon, points: f64) -> Id {
    // Pure white, judged on the solid pixels (resampling leaves faint,
    // slightly grey edges): the default colour, shown as a template.
    let template = icon.frames.iter().all(|f| {
        f.px.iter()
            .filter(|p| p[3] >= 96)
            .all(|p| p[0].min(p[1]).min(p[2]) >= 235)
    });
    // SAFETY: an image built from CGImages of our own pixels; each created
    // object is released once the image holds it.
    unsafe {
        let img: Id = send0(class(c"NSImage"), c"alloc");
        let img: Id = send1(
            img,
            c"initWithSize:",
            CGSize {
                width: points,
                height: points,
            },
        );
        for f in &icon.frames {
            let px: Vec<u32> =
                f.px.iter()
                    .map(|p| {
                        let a = u32::from(p[3]);
                        let pm = |c: u8| (u32::from(c) * a + 127) / 255;
                        (a << 24) | (pm(p[0]) << 16) | (pm(p[1]) << 8) | pm(p[2])
                    })
                    .collect();
            let bytes: Vec<u8> = px.iter().flat_map(|p| p.to_le_bytes()).collect();
            let data = objc::CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
            let provider = objc::CGDataProviderCreateWithCFData(data);
            let cs = objc::CGColorSpaceCreateDeviceRGB();
            let cg = objc::CGImageCreate(
                f.w,
                f.h,
                8,
                32,
                f.w * 4,
                cs,
                (2 << 12) | 2,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            if !cg.is_null() {
                let rep: Id = send0(class(c"NSBitmapImageRep"), c"alloc");
                let rep: Id = send1(rep, c"initWithCGImage:", cg as Id);
                let _: () = send1(
                    rep,
                    c"setSize:",
                    CGSize {
                        width: points,
                        height: points,
                    },
                );
                let _: () = send1(img, c"addRepresentation:", rep);
                objc::release(rep);
                objc::CGImageRelease(cg);
            }
            objc::CGColorSpaceRelease(cs);
            objc::CGDataProviderRelease(provider);
            objc::cf_release(data);
        }
        let _: () = send1(img, c"setTemplate:", template);
        img
    }
}

// ---------------------------------------------------------------------------
// Status item and menus
// ---------------------------------------------------------------------------

pub fn tray_icon(h: HWND, op: TrayOp, icon: Option<&Icon>, tip: &str) -> bool {
    let (item, target) = match with(|m| (m.status_item, m.target)) {
        Some(v) => v,
        None => return false,
    };
    // SAFETY: the system status bar's item for this app, created once and
    // kept until Delete; images and strings are released after use.
    unsafe {
        match op {
            TrayOp::Delete => {
                if !item.is_null() {
                    let bar: Id = send0(class(c"NSStatusBar"), c"systemStatusBar");
                    let _: () = send1(bar, c"removeStatusItem:", item);
                    objc::release(item);
                    with(|m| m.status_item = NIL);
                }
                true
            }
            TrayOp::Add | TrayOp::Modify => {
                let item = if item.is_null() {
                    let bar: Id = send0(class(c"NSStatusBar"), c"systemStatusBar");
                    // NSVariableStatusItemLength (-1).
                    let it: Id = send1(bar, c"statusItemWithLength:", -1.0f64);
                    let it = objc::retain(it);
                    let button: Id = send0(it, c"button");
                    let _: () = send1(button, c"setTarget:", target);
                    let _: () = send1(button, c"setAction:", objc::sel(c"trayClick:"));
                    // Left and right mouse up.
                    let _: isize = send1(button, c"sendActionOn:", (1usize << 2) | (1usize << 4));
                    with(|m| {
                        m.status_item = it;
                        m.tray_hwnd.get_or_insert(h);
                    });
                    it
                } else {
                    item
                };
                let button: Id = send0(item, c"button");
                if let Some(i) = icon {
                    let img = ns_image(i, 18.0);
                    let _: () = send1(button, c"setImage:", img);
                    objc::release(img);
                }
                let _: () = send1(button, c"setToolTip:", objc::nsstring(tip));
                true
            }
        }
    }
}

/// The status menu's items: (command, label, check state); 0 = separator.
pub fn set_tray_menu(items: Vec<(u32, String, Option<bool>)>) {
    let Some((menu, target)) = with(|m| {
        m.menu_items = items.clone();
        if m.status_menu.is_null() {
            let menu = objc::new(c"NSMenu");
            // SAFETY: an NSMenu we own; the target is its delegate.
            unsafe {
                let _: () = send1(menu, c"setDelegate:", m.target);
                let _: () = send1(menu, c"setAutoenablesItems:", false);
            }
            m.status_menu = menu;
        }
        (m.status_menu, m.target)
    }) else {
        return;
    };
    fill_menu(menu, target, &items, c"menuItem:");
}

/// Replace a menu's items.
fn fill_menu(menu: Id, target: Id, items: &[(u32, String, Option<bool>)], action: &std::ffi::CStr) {
    // SAFETY: menu items created, configured and handed to the menu (which
    // retains them); our references are released.
    unsafe {
        let _: () = send0(menu, c"removeAllItems");
        for (id, label, checked) in items {
            if *id == 0 {
                let sep: Id = send0(class(c"NSMenuItem"), c"separatorItem");
                let _: () = send1(menu, c"addItem:", sep);
                continue;
            }
            let item: Id = send0(class(c"NSMenuItem"), c"alloc");
            let item: Id = send3(
                item,
                c"initWithTitle:action:keyEquivalent:",
                objc::nsstring(label),
                objc::sel(action),
                objc::nsstring(""),
            );
            let _: () = send1(item, c"setTarget:", target);
            let _: () = send1(item, c"setTag:", *id as isize);
            let _: () = send1(item, c"setEnabled:", true);
            if let Some(c) = checked {
                let _: () = send1(item, c"setState:", isize::from(*c));
            }
            let _: () = send1(menu, c"addItem:", item);
            objc::release(item);
        }
    }
}

/// The calendar's copy menu: native, at the pointer; the chosen command.
pub fn popup_menu(owner: HWND, items: &[(u32, String)]) -> Option<u32> {
    let view = with(|m| m.windows.get(&owner).map(|w| w.view)).flatten()?;
    let target = with(|m| {
        m.chosen = None;
        m.target
    })?;
    let menu = objc::new(c"NSMenu");
    let list: Vec<(u32, String, Option<bool>)> =
        items.iter().map(|(i, l)| (*i, l.clone(), None)).collect();
    fill_menu(menu, target, &list, c"menuPick:");
    // SAFETY: the menu tracks modally from the pointer in the view's
    // coordinates; released afterwards.
    unsafe {
        let _: () = send1(menu, c"setAutoenablesItems:", false);
        let win: Id = send0(view, c"window");
        let p: CGPoint = send0(win, c"mouseLocationOutsideOfEventStream");
        let in_view: CGPoint = send2(view, c"convertPoint:fromView:", p, NIL);
        let _: bool = send3(
            menu,
            c"popUpMenuPositioningItem:atLocation:inView:",
            NIL,
            in_view,
            view,
        );
    }
    objc::release(menu);
    with(|m| m.chosen.take()).flatten()
}

// ---------------------------------------------------------------------------
// Edit field (NSTextField)
// ---------------------------------------------------------------------------

pub fn create_edit(parent: HWND, id: u16, font: &Font, text: &str) -> Option<HWND> {
    let (view, win, target) =
        with(|m| m.windows.get(&parent).map(|w| (w.view, w.window, m.target))).flatten()?;
    let h = core::new_hwnd();
    let scale = window_scale(win);
    // SAFETY: a text field added to our view; we keep a +1 reference.
    let field = unsafe {
        let f: Id = send0(class(c"NSTextField"), c"alloc");
        let f: Id = send1(f, c"initWithFrame:", CGRect::new(0.0, 0.0, 10.0, 10.0));
        let _: () = send1(f, c"setStringValue:", objc::nsstring(text));
        let _: () = send1(f, c"setBezeled:", false);
        let _: () = send1(f, c"setBordered:", false);
        let _: () = send1(f, c"setDrawsBackground:", true);
        // NSFocusRingTypeNone (1): the dialog draws the focused border.
        let _: () = send1(f, c"setFocusRingType:", 1usize);
        let _: () = send1(f, c"setTag:", h.0 as isize);
        let _: () = send1(f, c"setDelegate:", target);
        let size = f64::from(font.metrics().linespace()) / scale * 0.82;
        let nsfont: Id = send1(class(c"NSFont"), c"systemFontOfSize:", size.max(8.0));
        let _: () = send1(f, c"setFont:", nsfont);
        let cell: Id = send0(f, c"cell");
        let _: () = send1(cell, c"setUsesSingleLineMode:", true);
        let _: () = send1(cell, c"setScrollable:", true);
        let _: () = send1(view, c"addSubview:", f);
        f
    };
    with(|m| m.edits.insert(h, (parent, id, field)));
    core::invalidate(h);
    Some(h)
}

pub fn set_font(h: HWND, _font: isize) {
    core::invalidate(h);
}

pub fn get_text(h: HWND) -> String {
    let Some(field) = with(|m| m.edits.get(&h).map(|e| e.2)).flatten() else {
        return String::new();
    };
    // SAFETY: a text field's string value.
    objc::string_of(unsafe { send0(field, c"stringValue") })
}

pub fn select_all(h: HWND) {
    let Some(field) = with(|m| m.edits.get(&h).map(|e| e.2)).flatten() else {
        return;
    };
    // SAFETY: selects the field's text (and focuses it).
    unsafe {
        let _: () = send1(field, c"selectText:", NIL);
    }
}

pub fn move_child(h: HWND, x: i32, y: i32, w: i32, hgt: i32) {
    set_window_pos(h, x, y, w, hgt, None, false, false);
}

/// WM_CTLCOLOREDIT's answer: the field's text and background colours.
pub fn ctl_color(wparam: usize, fg: u32, bg: u32, _brush: &Brush) -> isize {
    let Some(field) = with(|m| m.edits.get(&HWND(wparam)).map(|e| e.2)).flatten() else {
        return 0;
    };
    let color = |c: u32| -> Id {
        let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
        // SAFETY: an autoreleased sRGB colour.
        unsafe {
            send4(
                class(c"NSColor"),
                c"colorWithSRGBRed:green:blue:alpha:",
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
                1.0f64,
            )
        }
    };
    // SAFETY: colours set on our text field.
    unsafe {
        let _: () = send1(field, c"setTextColor:", color(fg));
        let _: () = send1(field, c"setBackgroundColor:", color(bg));
    }
    0
}

fn notify_edit(h: HWND, code: u16) {
    if let Some((parent, id)) = with(|m| m.edits.get(&h).map(|e| (e.0, e.1))).flatten() {
        core::send(
            parent,
            wm::WM_COMMAND,
            (usize::from(code) << 16) | usize::from(id),
            h.0 as isize,
        );
    }
}

fn edit_of_notification(n: Id) -> HWND {
    // SAFETY: a notification whose object is one of our text fields.
    unsafe {
        let obj: Id = send0(n, c"object");
        HWND(send0::<isize>(obj, c"tag") as usize)
    }
}

/// Messages for the backend's own pieces: edit fields repaint by asking
/// their parent for colours.
fn filter(h: HWND, msg: u32, _wp: usize, _lp: isize) -> Option<isize> {
    let parent = with(|m| m.edits.get(&h).map(|e| e.0)).flatten()?;
    if msg == wm::WM_PAINT {
        core::send(parent, wm::WM_CTLCOLOREDIT, h.0, h.0 as isize);
    }
    Some(0)
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

pub fn set_clipboard_text(_owner: HWND, text: &str) -> bool {
    // SAFETY: the general pasteboard, cleared and given one string.
    unsafe {
        let pb: Id = send0(class(c"NSPasteboard"), c"generalPasteboard");
        let _: isize = send0(pb, c"clearContents");
        send2::<bool, Id, Id>(
            pb,
            c"setString:forType:",
            objc::nsstring(text),
            objc::nsstring("public.utf8-plain-text"),
        )
    }
}

// ---------------------------------------------------------------------------
// Event callbacks (extern "C", called by AppKit on the main thread)
// ---------------------------------------------------------------------------

extern "C" fn view_yes(_this: Id, _sel: Sel) -> bool {
    true
}

extern "C" fn view_no(_this: Id, _sel: Sel) -> bool {
    false
}

extern "C" fn view_yes1(_this: Id, _sel: Sel, _arg: Id) -> bool {
    true
}

extern "C" fn wake_noop(_this: Id, _sel: Sel, _arg: Id) {}

extern "C" fn view_update_layer(this: Id, _sel: Sel) {
    core::invalidate(hwnd_of(this));
}

/// The event's position in the view, in client pixels.
fn event_xy(view: Id, event: Id) -> (i32, i32) {
    // SAFETY: an NSEvent's window location converted into our view.
    unsafe {
        let p: CGPoint = send0(event, c"locationInWindow");
        let v: CGPoint = send2(view, c"convertPoint:fromView:", p, NIL);
        let win: Id = send0(view, c"window");
        let s = window_scale(win);
        ((v.x * s).floor() as i32, (v.y * s).floor() as i32)
    }
}

fn pack_xy(x: i32, y: i32) -> isize {
    ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize
}

fn update_modifiers(event: Id) {
    // SAFETY: an NSEvent's modifier flags.
    let flags: usize = unsafe { send0(event, c"modifierFlags") };
    let shift = flags & (1 << 17) != 0;
    // Command works as Ctrl does on Windows (copy, select all).
    let ctrl = flags & ((1 << 18) | (1 << 20)) != 0;
    core::set_modifiers(shift, ctrl);
}

fn mouse(this: Id, event: Id, msg: u32) {
    update_modifiers(event);
    let h = hwnd_of(this);
    let (x, y) = event_xy(this, event);
    core::send(h, msg, 0, pack_xy(x, y));
    core::pump();
}

extern "C" fn view_mouse_down(this: Id, _sel: Sel, event: Id) {
    mouse(this, event, wm::WM_LBUTTONDOWN);
}

extern "C" fn view_mouse_up(this: Id, _sel: Sel, event: Id) {
    mouse(this, event, wm::WM_LBUTTONUP);
}

extern "C" fn view_right_down(this: Id, _sel: Sel, event: Id) {
    mouse(this, event, wm::WM_RBUTTONDOWN);
}

extern "C" fn view_right_up(this: Id, _sel: Sel, event: Id) {
    mouse(this, event, wm::WM_RBUTTONUP);
}

extern "C" fn view_mouse_moved(this: Id, _sel: Sel, event: Id) {
    update_modifiers(event);
    let h = hwnd_of(this);
    with(|m| m.pointer_in = Some(h));
    let (x, y) = event_xy(this, event);
    core::send(
        h,
        wm::WM_SETCURSOR,
        h.0,
        (wm::HTCLIENT | (wm::WM_MOUSEMOVE << 16)) as isize,
    );
    core::send(h, wm::WM_MOUSEMOVE, 0, pack_xy(x, y));
    core::pump();
}

extern "C" fn view_mouse_exited(this: Id, _sel: Sel, _event: Id) {
    let h = hwnd_of(this);
    with(|m| {
        if m.pointer_in == Some(h) {
            m.pointer_in = None;
        }
    });
    core::mouse_left(h);
    core::pump();
}

extern "C" fn view_scroll(this: Id, _sel: Sel, event: Id) {
    let h = hwnd_of(this);
    // SAFETY: NSEvent scroll properties.
    let (dy, precise): (f64, bool) = unsafe {
        (
            send0(event, c"scrollingDeltaY"),
            send0(event, c"hasPreciseScrollingDeltas"),
        )
    };
    // A trackpad sends many small deltas: one notch per 24 points.
    let notches = with(|m| {
        if precise {
            m.scroll += dy;
            let n = (m.scroll / 24.0).trunc();
            m.scroll -= n * 24.0;
            n as i32
        } else {
            m.scroll = 0.0;
            if dy > 0.0 {
                1
            } else if dy < 0.0 {
                -1
            } else {
                0
            }
        }
    })
    .unwrap_or(0);
    for _ in 0..notches.unsigned_abs().min(4) {
        let delta: i16 = if notches > 0 { 120 } else { -120 };
        core::send(h, wm::WM_MOUSEWHEEL, (delta as u16 as usize) << 16, 0);
    }
    core::pump();
}

/// macOS key code -> Win32 virtual key.
pub fn virtual_key(code: u16, chars: &str) -> u16 {
    match code {
        53 => 0x1B,
        36 | 76 => 0x0D,
        48 => 0x09,
        51 => 0x08,
        117 => 0x2E,
        123 => 0x25,
        124 => 0x27,
        125 => 0x28,
        126 => 0x26,
        115 => 0x24,
        119 => 0x23,
        116 => 0x21,
        121 => 0x22,
        49 => 0x20,
        _ => match chars.chars().next() {
            Some(c) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u16,
            _ => 0,
        },
    }
}

extern "C" fn view_key_down(this: Id, _sel: Sel, event: Id) {
    update_modifiers(event);
    let h = hwnd_of(this);
    // SAFETY: NSEvent key properties.
    let (code, plain, typed): (u16, String, String) = unsafe {
        (
            send0(event, c"keyCode"),
            objc::string_of(send0(event, c"charactersIgnoringModifiers")),
            objc::string_of(send0(event, c"characters")),
        )
    };
    let vk = virtual_key(code, &plain);
    if vk != 0 {
        core::send(h, wm::WM_KEYDOWN, usize::from(vk), 0);
    }
    if !core::key_down(VK_CONTROL_) {
        for c in typed
            .chars()
            .filter(|c| !c.is_control() && (*c as u32) < 0xF700)
        {
            core::send(h, wm::WM_CHAR, c as usize, 0);
        }
    }
    core::pump();
}

extern "C" fn view_update_tracking(this: Id, _sel: Sel) {
    // SAFETY: replace the view's tracking area with one covering its
    // visible rect: moved, entered/exited, always active.
    unsafe {
        let areas: Id = send0(this, c"trackingAreas");
        let n: usize = send0(areas, c"count");
        for i in (0..n).rev() {
            let a: Id = send1(areas, c"objectAtIndex:", i);
            let _: () = send1(this, c"removeTrackingArea:", a);
        }
        let bounds: CGRect = send0(this, c"bounds");
        // MouseEnteredAndExited 1 | MouseMoved 2 | ActiveAlways 0x80 | InVisibleRect 0x200
        let opts: usize = 1 | 2 | 0x80 | 0x200;
        let area: Id = send0(class(c"NSTrackingArea"), c"alloc");
        let area: Id = send4(
            area,
            c"initWithRect:options:owner:userInfo:",
            bounds,
            opts,
            this,
            NIL,
        );
        let _: () = send1(this, c"addTrackingArea:", area);
        objc::release(area);
    }
}

extern "C" fn win_became_key(this: Id, _sel: Sel, _n: Id) {
    core::send(hwnd_of(this), wm::WM_ACTIVATE, wm::WA_ACTIVE as usize, 0);
    core::pump();
}

extern "C" fn win_resigned_key(this: Id, _sel: Sel, _n: Id) {
    core::send(hwnd_of(this), wm::WM_ACTIVATE, wm::WA_INACTIVE as usize, 0);
    core::pump();
}

extern "C" fn win_should_close(this: Id, _sel: Sel, _w: Id) -> bool {
    core::send(hwnd_of(this), wm::WM_CLOSE, 0, 0);
    core::pump();
    false
}

extern "C" fn win_did_resize(this: Id, _sel: Sel, _n: Id) {
    let h = hwnd_of(this);
    let (w, hgt) = client_size(h);
    core::send(h, wm::WM_SIZE, 0, pack_xy(w, hgt));
    core::invalidate(h);
    core::pump();
}

extern "C" fn win_backing(this: Id, _sel: Sel, _n: Id) {
    let h = hwnd_of(this);
    let Some(win) = window_of(h).map(|w| w.0) else {
        return;
    };
    let scale = window_scale(win);
    if let Some(view) = with(|m| m.windows.get(&h).map(|w| w.view)).flatten() {
        // SAFETY: the view's layer follows the new scale.
        unsafe {
            let layer: Id = send0(view, c"layer");
            let _: () = send1(layer, c"setContentsScale:", scale);
        }
    }
    // SAFETY: the window's current frame.
    let frame: CGRect = unsafe { send0(win, c"frame") };
    let list = screens();
    unix::set_suggested_rect(rect_to_px(frame, primary_height(&list), scale));
    let dpi = (96.0 * scale).round() as usize;
    core::send(h, wm::WM_DPICHANGED, dpi | (dpi << 16), 0);
    core::pump();
}

/// Show the menu bar item's menu once: attach, click, detach (so a left
/// click keeps opening the calendar). Returns when the menu closes.
fn open_status_menu(item: Id, menu: Id) {
    if item.is_null() || menu.is_null() {
        return;
    }
    // SAFETY: our status item and menu, on the main thread.
    unsafe {
        let _: () = send1(item, c"setMenu:", menu);
        let button: Id = send0(item, c"button");
        let _: () = send1(button, c"performClick:", NIL);
        let _: () = send1(item, c"setMenu:", NIL);
    }
}

/// The menu bar item's menu, as a right-click shows it.
pub fn show_status_menu() {
    let Some((tray, item, menu)) =
        with(|m| Some((m.tray_hwnd?, m.status_item, m.status_menu))).flatten()
    else {
        return;
    };
    core::send(tray, WM_TRAY, 0, TRAY_MENU_REFRESH);
    open_status_menu(item, menu);
}

extern "C" fn tray_click(_this: Id, _sel: Sel, _sender: Id) {
    let Some((tray, item, menu)) =
        with(|m| Some((m.tray_hwnd?, m.status_item, m.status_menu))).flatten()
    else {
        return;
    };
    // SAFETY: the event that triggered the action.
    let (kind, flags): (usize, usize) = unsafe {
        let app: Id = send0(class(c"NSApplication"), c"sharedApplication");
        let e: Id = send0(app, c"currentEvent");
        if e.is_null() {
            (0, 0)
        } else {
            (send0(e, c"type"), send0(e, c"modifierFlags"))
        }
    };
    // NSEventTypeRightMouseUp (4), or Control-click.
    if kind == 4 || flags & (1 << 18) != 0 {
        core::send(tray, WM_TRAY, 0, TRAY_MENU_REFRESH);
        open_status_menu(item, menu);
    } else {
        core::send(tray, WM_TRAY, 0, wm::WM_LBUTTONUP as isize);
    }
    core::pump();
}

extern "C" fn menu_item(_this: Id, _sel: Sel, sender: Id) {
    // SAFETY: the menu item's tag (our command id).
    let id: isize = unsafe { send0(sender, c"tag") };
    if let Some(tray) = with(|m| m.tray_hwnd).flatten() {
        core::post(tray, WM_TRAY, id as usize, wm::WM_COMMAND as isize);
    }
}

extern "C" fn menu_pick(_this: Id, _sel: Sel, sender: Id) {
    // SAFETY: as above.
    let id: isize = unsafe { send0(sender, c"tag") };
    with(|m| m.chosen = Some(id as u32));
}

extern "C" fn menu_needs_update(_this: Id, _sel: Sel, _menu: Id) {
    if let Some(tray) = with(|m| m.tray_hwnd).flatten() {
        core::send(tray, WM_TRAY, 0, TRAY_MENU_REFRESH);
    }
}

extern "C" fn text_changed(_this: Id, _sel: Sel, n: Id) {
    notify_edit(edit_of_notification(n), EN_CHANGE_);
    core::pump();
}

extern "C" fn text_began(_this: Id, _sel: Sel, n: Id) {
    notify_edit(edit_of_notification(n), EN_SETFOCUS_);
    core::pump();
}

extern "C" fn text_ended(_this: Id, _sel: Sel, n: Id) {
    notify_edit(edit_of_notification(n), EN_KILLFOCUS_);
    core::pump();
}

/// Return, Escape and Tab in the field go to the dialog (WM_APP_KEY).
extern "C" fn text_command(_this: Id, _sel: Sel, control: Id, _view: Id, command: Sel) -> bool {
    let key = if command == objc::sel(c"insertNewline:") {
        0x0D
    } else if command == objc::sel(c"cancelOperation:") {
        0x1B
    } else if command == objc::sel(c"insertTab:") || command == objc::sel(c"insertBacktab:") {
        0x09
    } else {
        return false;
    };
    // SAFETY: our text field's tag.
    let h = HWND(unsafe { send0::<isize>(control, c"tag") } as usize);
    if let Some(parent) = with(|m| m.edits.get(&h).map(|e| e.0)).flatten() {
        core::post(parent, WM_APP_KEY, key, 0);
    }
    true
}

// ---------------------------------------------------------------------------
// The message loop
// ---------------------------------------------------------------------------

extern "C" fn handover_on_main(_ctx: *mut c_void) {
    HANDOVER.store(true, Ordering::SeqCst);
    wake_loop();
}

/// Post an application-defined event so the loop's wait returns.
fn wake_loop() {
    // SAFETY: an autoreleased NSEventTypeApplicationDefined (15) event
    // posted to our own queue.
    unsafe {
        let e: Id = objc::send9::<Id, usize, CGPoint, usize, f64, isize, Id, i16, isize, isize>(
            class(c"NSEvent"),
            c"otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            15,
            CGPoint::default(),
            0,
            0.0,
            0,
            NIL,
            0,
            0,
            0,
        );
        if !e.is_null() {
            let app: Id = send0(class(c"NSApplication"), c"sharedApplication");
            let _: () = send2(app, c"postEvent:atStart:", e, false);
        }
    }
}

pub fn run_message_loop(instance: &Instance, mut on_event: impl FnMut()) -> i32 {
    use std::os::fd::AsRawFd;
    if let Some(fd) = instance.listener().map(|l| l.as_raw_fd()) {
        // A newer launch connects to the socket: wake the main thread.
        std::thread::spawn(move || {
            loop {
                if unix::sys::wait_readable(&[fd], -1).first() == Some(&true) {
                    objc::on_main_thread(handover_on_main);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        });
    }
    loop {
        let _pool = objc::Pool::new();
        core::pump();
        if let Some(code) = core::quit_requested() {
            return code;
        }
        if HANDOVER.swap(false, Ordering::SeqCst) && instance.quit_requested() {
            on_event();
            continue;
        }
        let wait = core::next_timeout().map(|d| d.as_secs_f64());
        // SAFETY: NSApplication's event queue on the main thread; the
        // event is dispatched with sendEvent:.
        unsafe {
            let app: Id = send0(class(c"NSApplication"), c"sharedApplication");
            let until: Id = match wait {
                Some(s) => send1(class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", s),
                None => send0(class(c"NSDate"), c"distantFuture"),
            };
            let mode = objc::nsstring("kCFRunLoopDefaultMode");
            let e: Id = send4(
                app,
                c"nextEventMatchingMask:untilDate:inMode:dequeue:",
                usize::MAX,
                until,
                mode,
                true,
            );
            if !e.is_null() {
                let _: () = send1(app, c"sendEvent:", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Login item (SMAppService, macOS 13+)
// ---------------------------------------------------------------------------

fn main_app_service() -> Option<Id> {
    let cls = class(c"SMAppService");
    if cls.is_null() {
        return None;
    }
    // SAFETY: the shared service object for this app bundle.
    let s: Id = unsafe { send0(cls, c"mainAppService") };
    (!s.is_null()).then_some(s)
}

/// Whether the app is registered as a login item (None: unavailable).
pub fn login_item_status() -> Option<bool> {
    let s = main_app_service()?;
    // SAFETY: SMAppServiceStatus: 1 = enabled.
    let status: isize = unsafe { send0(s, c"status") };
    Some(status == 1)
}

/// Register or unregister the login item; Err carries the system's reason.
pub fn login_item_set(enable: bool) -> Result<(), String> {
    let s = main_app_service().ok_or_else(|| "login items need macOS 13 or later".to_string())?;
    let mut error: Id = NIL;
    let sel: &std::ffi::CStr = if enable {
        c"registerAndReturnError:"
    } else {
        c"unregisterAndReturnError:"
    };
    // SAFETY: the method writes an autoreleased NSError (or nil) through
    // the pointer, which lives across the call.
    let ok: bool = unsafe { send1(s, sel, &raw mut error) };
    if ok || (!enable && login_item_status() == Some(false)) {
        return Ok(());
    }
    if error.is_null() {
        return Err(String::new());
    }
    // SAFETY: the error's localized description.
    Err(objc::string_of(unsafe {
        send0(error, c"localizedDescription")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_hangs_under_the_clicked_item() {
        // A 1512x982 Retina laptop (37 pt menu bar), and a 2560x1440
        // display to its right (scale 1, 25 pt menu bar), tops aligned.
        let laptop = Screen {
            frame: CGRect::new(0.0, 0.0, 1512.0, 982.0),
            visible: CGRect::new(0.0, 0.0, 1512.0, 945.0),
            scale: 2.0,
        };
        let wide = Screen {
            frame: CGRect::new(1512.0, -458.0, 2560.0, 1440.0),
            visible: CGRect::new(1512.0, -458.0, 2560.0, 1415.0),
            scale: 1.0,
        };
        let list = [laptop, wide];
        let away = CGPoint { x: 700.0, y: 400.0 };
        // The item on the laptop: under its centre, at the menu bar's foot.
        let item = CGRect::new(1300.0, 945.0, 30.0, 37.0);
        assert_eq!(menu_bar_anchor(&list, Some(item), away), Some((2630, 74)));
        // The item on the second display's menu bar: there, not at the
        // left edge of that display.
        let item = CGRect::new(3900.0, 957.0, 30.0, 25.0);
        assert_eq!(menu_bar_anchor(&list, Some(item), away), Some((3915, 25)));
        // No usable frame (macOS 26, or an item squeezed to the far left):
        // the pointer, when it is in that display's menu bar.
        let squeezed = CGRect::new(0.0, 945.0, 30.0, 37.0);
        let click = CGPoint {
            x: 3800.0,
            y: 970.0,
        };
        assert_eq!(
            menu_bar_anchor(&list, Some(squeezed), click),
            Some((3800, 25))
        );
        assert_eq!(menu_bar_anchor(&list, None, click), Some((3800, 25)));
        // A pointer resting in the top left corner is no click on the item.
        let corner = CGPoint { x: 2.0, y: 980.0 };
        assert_eq!(menu_bar_anchor(&list, None, corner), Some((2784, 74)));
        // Neither: the right end of the menu bar under the pointer.
        let below = CGPoint {
            x: 3000.0,
            y: 100.0,
        };
        assert_eq!(menu_bar_anchor(&list, None, below), Some((3952, 25)));
        assert_eq!(menu_bar_anchor(&list, None, away), Some((2784, 74)));
    }

    #[test]
    fn matte_recovers_alpha_and_colour() {
        // Card: black over black, white over white -> transparent.
        assert_eq!(matte(0x000000, 0xFFFFFF), 0x0000_0000);
        // Opaque pixel: the same both times.
        assert_eq!(matte(0x336699, 0x336699), 0xFF33_6699);
        // Half-covered white text: 128 over black, 255 over white.
        assert_eq!(matte(0x808080, 0xFFFFFF) >> 24, 0x80);
        assert!(matches!(matte(0x808080, 0xFFFFFF) & 0xFF, 0x7F | 0x80));
    }

    #[test]
    fn keys() {
        assert_eq!(virtual_key(53, ""), 0x1B);
        assert_eq!(virtual_key(0, "a"), 0x41);
        assert_eq!(virtual_key(123, ""), 0x25);
    }

    #[test]
    fn rects() {
        let r = rect_to_px(CGRect::new(10.0, 800.0, 100.0, 50.0), 900.0, 2.0);
        assert_eq!(
            r,
            Rect {
                left: 20,
                top: 100,
                right: 220,
                bottom: 200
            }
        );
    }
}
