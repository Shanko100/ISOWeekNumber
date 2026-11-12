//! The Linux backend: X11 (or XWayland) windows, the tray icon over D-Bus
//! (StatusNotifierItem) or the XEmbed system tray, TrueType text and the
//! IANA time-zone database -- all spoken directly over sockets and files,
//! so one statically linked executable runs on any distribution.
//!
//! Native events become the Win32 messages the app's windows handle. Pop-ups
//! (flyout, menus, drop-down lists) grab the pointer and keyboard while they
//! are active, as menus do on X11, so a click elsewhere dismisses them.

mod dbus;
mod edit;
pub mod font;
mod frame;
mod msgbox;
mod portal;
mod sni;
mod x11;

pub use font::Font;
pub use portal::request_autostart;

use super::unix::{self, HWND, Kind, core, wm};
pub use super::unix::{MonitorGeom, Rect, is_null};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use x11::{Conn, Event};

pub use super::unix::Instance;

/// A window the backend created.
struct Win {
    xid: u32,
    kind: WinKind,
    /// Client area: position on the screen (pop-ups; dialogs as last
    /// known) and size.
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    depth: u8,
    mapped: bool,
    radius: i32,
    /// Transparent margin around a pop-up's client area that holds its
    /// shadow (compositing only); the X window is this much larger on
    /// every side.
    inset: i32,
    /// The shadow for the client size it was made for.
    shadow: Option<(i32, i32, Vec<u8>)>,
    /// The client size the Shape extension last cut corners for.
    shaped: (i32, i32),
    cursor: unix::Cursor,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WinKind {
    Top(Kind),
    Edit(HWND),
    MessageBox,
}

struct Atoms {
    protocols: u32,
    delete: u32,
    net_wm_name: u32,
    utf8: u32,
    net_wm_icon: u32,
    net_wm_pid: u32,
    wm_type: u32,
    type_dropdown: u32,
    type_popup: u32,
    type_tooltip: u32,
    type_dialog: u32,
    type_normal: u32,
    net_active: u32,
    clipboard: u32,
    targets: u32,
    paste: u32,
    gtk_variant: u32,
    workarea: u32,
    gtk_workareas: u32,
    xembed_info: u32,
    tray_opcode: u32,
    tray_selection: u32,
    manager: u32,
}

/// How the tray icon is shown.
enum TrayHost {
    None,
    Sni(Box<sni::Sni>),
    /// The XEmbed icon window and its size.
    Xembed {
        xid: u32,
        size: i32,
    },
}

struct Lx {
    x: Conn,
    a: Atoms,
    windows: HashMap<HWND, Win>,
    by_xid: HashMap<u32, HWND>,
    gc24: u32,
    gc32: Option<u32>,
    argb_colormap: Option<u32>,
    cursors: [u32; 3],
    active: Option<HWND>,
    grab: Option<HWND>,
    pointer_in: Option<HWND>,
    time: u32,
    utility: u32,
    clipboard: Option<String>,
    paste_into: Option<HWND>,
    dpi: u32,
    tray_hwnd: Option<HWND>,
    tray: TrayHost,
    tray_icon: Option<Icon>,
    tray_tip: String,
    tray_menu: Vec<sni::MenuItem>,
    /// The size the X tray last gave the icon (kept across re-docking).
    xembed_size: i32,
    /// GC and colormap for an ARGB tray icon where no compositing manager
    /// announced itself (made when first needed): the tray composites its
    /// icons itself (GNOME Shell's tray under Wayland).
    tray_argb: Option<(u32, u32)>,
    /// Where the panel said its icon was clicked (StatusNotifierItem
    /// Activate), and when: the flyout opens there.
    anchor: Option<(i32, i32, std::time::Instant)>,
}

thread_local! {
    static LX: RefCell<Option<Lx>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Lx) -> R) -> Option<R> {
    LX.with(|l| l.try_borrow_mut().ok()?.as_mut().map(f))
}

fn hwnd_of(xid: u32) -> Option<HWND> {
    with(|l| l.by_xid.get(&xid).copied()).flatten()
}

/// lparam for a mouse message: client x and y, 16 bits each.
fn pack_xy(x: i32, y: i32) -> isize {
    ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as isize
}

// ---------------------------------------------------------------------------
// Start-up
// ---------------------------------------------------------------------------

pub fn harden_process() {}
pub fn init_com() {}

/// Connect to the display and register the message handler.
pub fn register_class(handler: core::Handler) -> bool {
    core::register_class(handler);
    core::set_filter(filter);
    if with(|_| ()).is_some() {
        return true;
    }
    let Some(mut x) = Conn::connect() else {
        eprintln!("WeekNumber: cannot open the X display (DISPLAY is not set or not reachable).");
        return false;
    };
    let mut atom = |n: &str| x.atom(n);
    let screen_sel = "_NET_SYSTEM_TRAY_S0";
    let a = Atoms {
        protocols: atom("WM_PROTOCOLS"),
        delete: atom("WM_DELETE_WINDOW"),
        net_wm_name: atom("_NET_WM_NAME"),
        utf8: atom("UTF8_STRING"),
        net_wm_icon: atom("_NET_WM_ICON"),
        net_wm_pid: atom("_NET_WM_PID"),
        wm_type: atom("_NET_WM_WINDOW_TYPE"),
        type_dropdown: atom("_NET_WM_WINDOW_TYPE_DROPDOWN_MENU"),
        type_popup: atom("_NET_WM_WINDOW_TYPE_POPUP_MENU"),
        type_tooltip: atom("_NET_WM_WINDOW_TYPE_TOOLTIP"),
        type_dialog: atom("_NET_WM_WINDOW_TYPE_DIALOG"),
        type_normal: atom("_NET_WM_WINDOW_TYPE_NORMAL"),
        net_active: atom("_NET_ACTIVE_WINDOW"),
        clipboard: atom("CLIPBOARD"),
        targets: atom("TARGETS"),
        paste: atom("WEEKNUMBER_PASTE"),
        gtk_variant: atom("_GTK_THEME_VARIANT"),
        workarea: atom("_NET_WORKAREA"),
        gtk_workareas: atom("_GTK_WORKAREAS_D0"),
        xembed_info: atom("_XEMBED_INFO"),
        tray_opcode: atom("_NET_SYSTEM_TRAY_OPCODE"),
        tray_selection: atom(screen_sel),
        manager: atom("MANAGER"),
    };
    let cm = x.atom("_NET_WM_CM_S0");
    let compositing = x.get_selection_owner(cm) != 0;
    let root = x.setup.root;
    let gc24 = x.create_gc(root);
    let (gc32, argb_colormap) = match (compositing, x.setup.argb) {
        (true, Some(v)) => {
            let (gc, cmap) = argb_resources(&mut x, v.id);
            (Some(gc), Some(cmap))
        }
        _ => (None, None),
    };
    // XC_left_ptr, XC_hand2, XC_crosshair.
    let cursors = [x.font_cursor(68), x.font_cursor(60), x.font_cursor(34)];
    // A hidden window owns the clipboard and receives pastes.
    let utility = x.gen_id();
    x.create_window(
        utility,
        root,
        -10,
        -10,
        1,
        1,
        0,
        0,
        x11::CW_OVERRIDE_REDIRECT | x11::CW_EVENT_MASK,
        &[1, x11::EV_PROPERTY],
    );
    // Root: work area and tray manager changes.
    x.change_attributes(
        root,
        x11::CW_EVENT_MASK,
        &[x11::EV_PROPERTY | x11::EV_STRUCTURE],
    );
    let dpi = read_dpi(&mut x);
    x.flush();
    let lx = Lx {
        x,
        a,
        windows: HashMap::new(),
        by_xid: HashMap::new(),
        gc24,
        gc32,
        argb_colormap,
        cursors,
        active: None,
        grab: None,
        pointer_in: None,
        time: 0,
        utility,
        clipboard: None,
        paste_into: None,
        dpi,
        tray_hwnd: None,
        tray: TrayHost::None,
        tray_icon: None,
        tray_tip: String::new(),
        tray_menu: Vec::new(),
        xembed_size: (22.0 * f64::from(dpi) / 96.0).round() as i32,
        anchor: None,
        tray_argb: None,
    };
    LX.with(|l| *l.borrow_mut() = Some(lx));
    true
}

/// The desktop's font DPI: Xft.dpi from the resource database, else
/// GDK_SCALE, else 96.
fn read_dpi(x: &mut Conn) -> u32 {
    let rm = x.atom("RESOURCE_MANAGER");
    let root = x.setup.root;
    let from_db = x
        .get_property(root, rm, false, 1 << 16)
        .and_then(|(_, _, d)| {
            String::from_utf8_lossy(&d).lines().find_map(|l| {
                let v = l.strip_prefix("Xft.dpi:")?;
                v.trim().parse::<f64>().ok()
            })
        });
    let gdk = std::env::var("GDK_SCALE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|s| s * 96.0);
    from_db
        .or(gdk)
        .map_or(96, |d| d.round() as u32)
        .clamp(72, 480)
}

/// $XDG_CONFIG_HOME/weeknumber, else ~/.config/weeknumber.
pub fn settings_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("weeknumber")
}

impl Instance {
    pub fn claim() -> Option<Instance> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("weeknumber"))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/weeknumber"))
            });
        let dir = base.and_then(|b| unix::instance::private_dir(&b));
        Instance::claim_in(dir, &unix::instance::tag(), || {
            message_box_error(
                "WeekNumber is already running and did not respond in time. Quit it, \
                 then start WeekNumber again.",
            );
        })
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

pub use super::unix::WindowSpec;

pub fn create_window(spec: &WindowSpec<'_>) -> Option<HWND> {
    let kind = Kind::of(spec);
    let h = core::new_hwnd();
    with(|l| l.create(h, WinKind::Top(kind), spec, 0))??;
    Some(h)
}

impl Lx {
    fn create(&mut self, h: HWND, kind: WinKind, spec: &WindowSpec<'_>, parent: u32) -> Option<()> {
        let xid = self.x.gen_id();
        let root = self.x.setup.root;
        let popup = matches!(kind, WinKind::Top(Kind::Popup | Kind::Tip));
        let use_argb = popup && self.argb_colormap.is_some();
        let events = x11::EV_EXPOSURE
            | x11::EV_KEY_PRESS
            | x11::EV_BUTTON_PRESS
            | x11::EV_BUTTON_RELEASE
            | x11::EV_POINTER_MOTION
            | x11::EV_ENTER
            | x11::EV_LEAVE
            | x11::EV_STRUCTURE
            | x11::EV_FOCUS;
        let (depth, visual) = if use_argb {
            let v = self.x.setup.argb?;
            (v.depth, v.id)
        } else {
            (0, 0)
        };
        let mut mask = x11::CW_BACK_PIXEL | x11::CW_BORDER_PIXEL | x11::CW_EVENT_MASK;
        let mut values = vec![0, 0];
        if popup {
            mask |= x11::CW_OVERRIDE_REDIRECT;
            values.push(1);
        }
        values.push(events);
        if use_argb {
            mask |= x11::CW_COLORMAP;
            values.push(self.argb_colormap?);
        }
        let (w, hgt) = (spec.w.max(1), spec.h.max(1));
        self.x.create_window(
            xid,
            if parent != 0 { parent } else { root },
            spec.x,
            spec.y,
            w,
            hgt,
            depth,
            visual,
            mask,
            &values,
        );
        let pid = std::process::id();
        self.x
            .change_property32(xid, self.a.net_wm_pid, 6 /* CARDINAL */, &[pid]);
        if parent == 0 {
            self.set_title_props(xid, spec.title);
            self.x.change_property(
                xid,
                67, /* WM_CLASS */
                31, /* STRING */
                8,
                b"weeknumber\0WeekNumber\0",
            );
            self.x
                .change_property32(xid, self.a.protocols, 4 /* ATOM */, &[self.a.delete]);
            let wtype = match kind {
                WinKind::Top(Kind::Popup) if spec.title.is_empty() => self.a.type_dropdown,
                WinKind::Top(Kind::Popup) => self.a.type_popup,
                WinKind::Top(Kind::Tip) => self.a.type_tooltip,
                WinKind::MessageBox => self.a.type_dialog,
                _ => self.a.type_normal,
            };
            self.x.change_property32(xid, self.a.wm_type, 4, &[wtype]);
            if let WinKind::Top(Kind::Dialog { resizable }) = kind {
                self.size_hints(
                    xid,
                    spec.x,
                    spec.y,
                    w,
                    hgt,
                    (!resizable).then_some((w, hgt)),
                );
            } else if kind == WinKind::MessageBox {
                self.size_hints(xid, spec.x, spec.y, w, hgt, Some((w, hgt)));
            }
            if let Some(icon) = app_icon(32).zip(app_icon(16)) {
                self.icon_props(xid, &[&icon.0, &icon.1]);
            }
        }
        self.windows.insert(
            h,
            Win {
                xid,
                kind,
                x: spec.x,
                y: spec.y,
                w,
                h: hgt,
                depth: if use_argb {
                    32
                } else {
                    self.x.setup.root_depth
                },
                mapped: false,
                radius: 0,
                inset: 0,
                shadow: None,
                shaped: (0, 0),
                cursor: unix::Cursor::Arrow,
            },
        );
        self.by_xid.insert(xid, h);
        Some(())
    }

    fn set_title_props(&mut self, xid: u32, title: &str) {
        self.x
            .change_property(xid, self.a.net_wm_name, self.a.utf8, 8, title.as_bytes());
        let latin: Vec<u8> = title
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        self.x.change_property(xid, 39 /* WM_NAME */, 31, 8, &latin);
    }

    /// WM_NORMAL_HINTS: our position, and a fixed size unless resizable.
    fn size_hints(&mut self, xid: u32, x: i32, y: i32, w: i32, h: i32, fixed: Option<(i32, i32)>) {
        // flags: USPosition 1 | PPosition 4 | PMinSize 16 | PMaxSize 32 | PWinGravity 512
        let mut hints = [0u32; 18];
        hints[0] = 1 | 4 | 512;
        hints[1] = x as u32;
        hints[2] = y as u32;
        hints[3] = w as u32;
        hints[4] = h as u32;
        if let Some((fw, fh)) = fixed {
            hints[0] |= 16 | 32;
            hints[5] = fw as u32;
            hints[6] = fh as u32;
            hints[7] = fw as u32;
            hints[8] = fh as u32;
        }
        hints[17] = 1; // NorthWestGravity
        let wm_size_hints = 41;
        self.x
            .change_property32(xid, 40 /* WM_NORMAL_HINTS */, wm_size_hints, &hints);
    }

    fn icon_props(&mut self, xid: u32, icons: &[&Icon]) {
        let mut data = Vec::new();
        for icon in icons {
            for (side, px) in &icon.frames {
                data.push(*side as u32);
                data.push(*side as u32);
                data.extend(px.iter().map(|p| {
                    (u32::from(p[3]) << 24)
                        | (u32::from(p[0]) << 16)
                        | (u32::from(p[1]) << 8)
                        | u32::from(p[2])
                }));
            }
        }
        self.x.change_property32(xid, self.a.net_wm_icon, 6, &data);
    }

    fn deactivate(&mut self) -> Option<HWND> {
        let old = self.active.take()?;
        if self.grab == Some(old) {
            self.grab = None;
            self.x.ungrab_pointer(self.time);
            self.x.ungrab_keyboard(self.time);
        }
        Some(old)
    }

    fn grab_popup(&mut self, h: HWND) {
        let Some(xid) = self.windows.get(&h).map(|w| w.xid) else {
            return;
        };
        self.x.set_input_focus(xid, 0);
        for _ in 0..20 {
            if self.x.grab_pointer(xid, 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        self.x.grab_keyboard(xid, 0);
        self.grab = Some(h);
    }
}

/// A window that has gone stops receiving messages.
pub fn destroy_window(h: HWND) {
    let is_window = with(|l| l.windows.contains_key(&h)).unwrap_or(false);
    if is_window {
        core::send(h, wm::WM_DESTROY, 0, 0);
    }
    with(|l| {
        if l.active == Some(h) {
            l.deactivate();
        }
        if l.pointer_in == Some(h) {
            l.pointer_in = None;
        }
        if let Some(w) = l.windows.remove(&h) {
            l.by_xid.remove(&w.xid);
            l.x.destroy_window(w.xid);
            l.x.flush();
        }
        if l.paste_into == Some(h) {
            l.paste_into = None;
        }
    });
    edit::forget(h);
    core::forget(h);
    // The last window closed: hand the font memory back.
    let any_open = with(|l| {
        l.windows.values().any(|w| {
            matches!(w.kind, WinKind::Top(_) | WinKind::MessageBox) && w.xid != tray_xid(l)
        })
    })
    .unwrap_or(true);
    if !any_open {
        font::release_caches();
    }
}

fn tray_xid(l: &Lx) -> u32 {
    match l.tray {
        TrayHost::Xembed { xid, .. } => xid,
        _ => 0,
    }
}

pub fn is_window(h: HWND) -> bool {
    with(|l| l.windows.contains_key(&h)).unwrap_or(false)
}

/// A window with no screen presence: the tray icon's message target.
pub fn create_hidden_window() -> Option<HWND> {
    let h = core::new_hwnd();
    with(|l| l.tray_hwnd = Some(h))?;
    Some(h)
}

pub fn register_window_message(_name: &str) -> u32 {
    0
}

pub fn allow_message(_h: HWND, _msg: u32) {}

pub fn show_window(h: HWND, activate: bool) {
    let popup = with(|l| {
        let w = l.windows.get_mut(&h)?;
        w.mapped = true;
        let xid = w.xid;
        let kind = w.kind;
        if let WinKind::Top(Kind::Popup | Kind::Tip) = kind {
            l.x.configure(xid, x11::CFG_STACK, &[0]); // Above
        }
        l.x.map_window(xid);
        l.x.flush();
        Some(matches!(kind, WinKind::Top(Kind::Popup)))
    })
    .flatten();
    if activate && popup.is_some() {
        set_foreground(h);
    }
}

pub fn restore_window(h: HWND) {
    show_window(h, false);
}

/// Make `h` the active window: pop-ups take the pointer and keyboard;
/// dialogs ask the window manager. The previous one is told it lost the
/// activation first.
pub fn set_foreground(h: HWND) -> bool {
    let (old, kind) = match with(|l| {
        let kind = l.windows.get(&h)?.kind;
        if l.active == Some(h) {
            return None;
        }
        Some((l.deactivate(), kind))
    })
    .flatten()
    {
        Some(v) => v,
        None => return is_window(h),
    };
    if let Some(old) = old {
        core::send(old, wm::WM_ACTIVATE, wm::WA_INACTIVE as usize, 0);
    }
    with(|l| {
        l.active = Some(h);
        match kind {
            WinKind::Top(Kind::Popup) => l.grab_popup(h),
            WinKind::Top(Kind::Dialog { .. }) | WinKind::MessageBox => {
                if let Some(xid) = l.windows.get(&h).map(|w| w.xid) {
                    let e = Conn::client_message(xid, l.a.net_active, [1, l.time, 0, 0, 0]);
                    let root = l.x.setup.root;
                    l.x.send_event(root, 0x18_0000, &e);
                }
            }
            _ => {}
        }
        l.x.flush();
    });
    if kind != WinKind::Top(Kind::Tip) {
        core::send(h, wm::WM_ACTIVATE, wm::WA_ACTIVE as usize, 0);
    }
    true
}

pub fn set_focus(h: HWND) {
    if edit::is_edit(h) {
        edit::focus(h);
        return;
    }
    edit::blur_all_in(h);
    with(|l| {
        if let Some(w) = l.windows.get(&h)
            && w.mapped
        {
            let xid = w.xid;
            l.x.set_input_focus(xid, l.time);
            l.x.flush();
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn set_window_pos(
    h: HWND,
    x: i32,
    y: i32,
    w: i32,
    hgt: i32,
    _topmost: Option<bool>,
    no_size: bool,
    no_move: bool,
) {
    with(|l| {
        let Some(win) = l.windows.get_mut(&h) else {
            return;
        };
        let mut mask = 0;
        let mut values = Vec::new();
        let i = win.inset;
        if !no_move {
            mask |= x11::CFG_X | x11::CFG_Y;
            values.extend([x - i, y - i]);
            win.x = x;
            win.y = y;
        }
        if !no_size {
            mask |= x11::CFG_W | x11::CFG_H;
            values.extend([w.max(1) + 2 * i, hgt.max(1) + 2 * i]);
            win.w = w.max(1);
            win.h = hgt.max(1);
        }
        if mask != 0 {
            let xid = win.xid;
            let kind = win.kind;
            l.x.configure(xid, mask, &values);
            if !no_size && let WinKind::Top(Kind::Dialog { resizable: false }) = kind {
                l.size_hints(xid, x, y, w, hgt, Some((w, hgt)));
            }
            l.x.flush();
        }
    });
}

/// Client area size equals the window size here: the window manager adds
/// its frame outside it.
pub fn outer_size(_style: u32, _ex_style: u32, w: i32, h: i32, _dpi: u32) -> (i32, i32, i32, i32) {
    (0, 0, w, h)
}

pub fn client_size(h: HWND) -> (i32, i32) {
    with(|l| l.windows.get(&h).map(|w| (w.w, w.h)))
        .flatten()
        .unwrap_or((0, 0))
}

fn origin(h: HWND) -> (i32, i32) {
    with(|l| {
        let w = l.windows.get(&h)?;
        let (xid, kind, x, y) = (w.xid, w.kind, w.x, w.y);
        if let WinKind::Top(Kind::Popup | Kind::Tip) = kind {
            return Some((x, y));
        }
        let root = l.x.setup.root;
        l.x.translate(xid, root, 0, 0)
    })
    .flatten()
    .unwrap_or((0, 0))
}

pub fn client_to_screen(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let (ox, oy) = origin(h);
    (ox + x, oy + y)
}

pub fn screen_to_client(h: HWND, x: i32, y: i32) -> (i32, i32) {
    let (ox, oy) = origin(h);
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

/// The pointer stays with the window that took the button press (X11's
/// implicit grab), so capture needs no request.
pub fn capture(_h: HWND) {}
pub fn release_capture() {}

pub fn track_leave(h: HWND) {
    core::track_leave(h);
}

pub use super::unix::Cursor;

pub fn set_cursor(c: Cursor) {
    with(|l| {
        let Some(h) = l.pointer_in else { return };
        let cursors = l.cursors;
        if let Some(w) = l.windows.get_mut(&h)
            && w.cursor != c
        {
            w.cursor = c;
            let id = match c {
                Cursor::Arrow => cursors[0],
                Cursor::Hand => cursors[1],
                Cursor::Cross => cursors[2],
            };
            let xid = w.xid;
            l.x.change_attributes(xid, x11::CW_CURSOR, &[id]);
        }
    });
}

/// Rounded corners for pop-ups, as on Windows 11: with a compositor the
/// corners are antialiased and a soft shadow is drawn around the pop-up;
/// without one the Shape extension cuts them.
pub fn round_corners(h: HWND, small: bool) {
    with(|l| {
        let s = f64::from(l.dpi) / 96.0;
        let r = (if small { 4.0 } else { 8.0 } * s).round() as i32;
        let shape = l.x.shape.is_some();
        let Some(w) = l.windows.get_mut(&h) else {
            return;
        };
        if !matches!(w.kind, WinKind::Top(Kind::Popup | Kind::Tip)) {
            return;
        }
        if w.depth == 32 {
            w.radius = r;
            let (blur, dy) = shadow_metrics(small, s);
            w.inset = 3 * blur + dy;
            let (xid, i) = (w.xid, w.inset);
            let v = [w.x - i, w.y - i, w.w + 2 * i, w.h + 2 * i];
            let mask = x11::CFG_X | x11::CFG_Y | x11::CFG_W | x11::CFG_H;
            l.x.configure(xid, mask, &v);
        } else if shape {
            w.radius = r;
        }
    });
}

/// The shadow's blur radius and downward offset, in pixels.
fn shadow_metrics(small: bool, scale: f64) -> (i32, i32) {
    let px = |v: f64| (v * scale).round().max(1.0) as i32;
    if small {
        (px(2.0), px(1.0))
    } else {
        (px(4.0), px(2.0))
    }
}

/// Ask for the dark window decoration (GTK-themed window managers).
pub fn dark_title_bar(h: HWND, dark: bool) {
    with(|l| {
        if let Some(xid) = l.windows.get(&h).map(|w| w.xid) {
            let v: &[u8] = if dark { b"dark" } else { b"light" };
            let (prop, utf8) = (l.a.gtk_variant, l.a.utf8);
            l.x.change_property(xid, prop, utf8, 8, v);
        }
    });
}

pub fn set_title(h: HWND, title: &str) {
    if edit::is_edit(h) {
        edit::set_text(h, title);
        return;
    }
    with(|l| {
        if let Some(xid) = l.windows.get(&h).map(|w| w.xid) {
            l.set_title_props(xid, title);
        }
    });
}

pub fn set_min_track_size(lparam: isize, w: i32, h: i32) {
    let hwnd = HWND(lparam as usize);
    with(|l| {
        if let Some(win) = l.windows.get(&hwnd) {
            let (xid, x, y) = (win.xid, win.x, win.y);
            let mut hints = [0u32; 18];
            hints[0] = 16 | 512;
            hints[1] = x as u32;
            hints[2] = y as u32;
            hints[5] = w as u32;
            hints[6] = h as u32;
            hints[17] = 1;
            l.x.change_property32(xid, 40, 41, &hints);
        }
    });
}

pub fn key_down(vk: u16) -> bool {
    core::key_down(vk)
}

pub fn double_click_ms() -> u64 {
    400
}

pub fn beep() {
    with(|l| {
        l.x.bell();
        l.x.flush();
    });
}

pub fn tick_ms() -> u64 {
    core::tick_ms()
}

pub fn post_quit() {
    core::post_quit();
}

// ---------------------------------------------------------------------------
// Geometry and DPI
// ---------------------------------------------------------------------------

/// Where the calendar hangs from when not at the pointer (macOS only).
pub fn tray_anchor() -> Option<(i32, i32)> {
    None
}

pub fn cursor_pos() -> (i32, i32) {
    with(Lx::pointer).unwrap_or((0, 0))
}

impl Lx {
    /// Where the pop-ups open: the pointer. Under Wayland the X server only
    /// sees the pointer while it is over X windows, so there it is the
    /// point the panel reported for a click on the tray icon just now, or
    /// the XEmbed icon that was clicked (if it lies on the panel), else the
    /// end of the panel where trays sit.
    fn pointer(&mut self) -> (i32, i32) {
        let (x, y, _) = self.x.query_pointer();
        if !on_wayland() {
            return (x, y);
        }
        let Some((mon, work, reach)) = self.primary_panel() else {
            return (x, y);
        };
        match self.anchor {
            Some((ax, ay, at))
                if at.elapsed() < std::time::Duration::from_secs(3)
                    && on_panel((ax, ay), mon, work, reach) =>
            {
                (ax, ay)
            }
            _ => panel_corner(mon, work),
        }
    }

    /// The primary monitor, its work area, and how far from its edges a
    /// point still counts as on a panel.
    fn primary_panel(&mut self) -> Option<(Rect, Rect, i32)> {
        let mons = self.x.monitors();
        let m = mons.iter().find(|m| m.primary).or(mons.first()).copied()?;
        let mon = Rect {
            left: m.x,
            top: m.y,
            right: m.x + m.w,
            bottom: m.y + m.h,
        };
        let work = self.work_area(mon);
        Some((mon, work, (64 * self.dpi / 96) as i32))
    }

    /// Whether a screen point lies on a panel of the primary monitor.
    fn on_a_panel(&mut self, p: (i32, i32)) -> bool {
        self.primary_panel()
            .is_some_and(|(mon, work, reach)| on_panel(p, mon, work, reach))
    }
}

/// Whether a reported click lies on a panel: on the monitor, outside the
/// work area (or within `reach` of an edge when no panel reserves space).
/// Coordinates in another scale (a HiDPI panel's logical pixels) fail this
/// and are not trusted.
fn on_panel(p: (i32, i32), mon: Rect, work: Rect, reach: i32) -> bool {
    let (x, y) = p;
    let on_mon = x >= mon.left && x < mon.right && y >= mon.top && y < mon.bottom;
    let in_work = x >= work.left && x < work.right && y >= work.top && y < work.bottom;
    let near_edge = x - mon.left < reach
        || mon.right - x <= reach
        || y - mon.top < reach
        || mon.bottom - y <= reach;
    on_mon && if work == mon { near_edge } else { !in_work }
}

/// The tray end of the panel: the right end of the edge the work area
/// leaves the most room at (the top bar on GNOME, the bottom panel on
/// KDE Plasma, Cinnamon and Xfce); top right when no panel reserves space.
fn panel_corner(mon: Rect, work: Rect) -> (i32, i32) {
    let gaps = [
        (work.top - mon.top, (mon.right - 1, mon.top)),
        (mon.bottom - work.bottom, (mon.right - 1, mon.bottom - 1)),
        (work.left - mon.left, (mon.left, mon.bottom - 1)),
        (mon.right - work.right, (mon.right - 1, mon.bottom - 1)),
    ];
    gaps.iter()
        .filter(|g| g.0 > 0)
        .max_by_key(|g| g.0)
        .map_or((mon.right - 1, mon.top), |g| g.1)
}

/// The monitor under the pointer, with its share of the work area.
pub fn cursor_monitor() -> MonitorGeom {
    with(|l| {
        let (px, py) = l.pointer();
        let mons = l.x.monitors();
        let m = mons
            .iter()
            .find(|m| px >= m.x && py >= m.y && px < m.x + m.w && py < m.y + m.h)
            .or_else(|| mons.iter().find(|m| m.primary))
            .or(mons.first())
            .copied()
            .unwrap_or(x11::Monitor {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                primary: true,
            });
        let monitor = Rect {
            left: m.x,
            top: m.y,
            right: m.x + m.w,
            bottom: m.y + m.h,
        };
        let work = l.work_area(monitor);
        MonitorGeom {
            monitor,
            work,
            dpi: l.dpi,
        }
    })
    .unwrap_or(MonitorGeom {
        monitor: Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
        work: Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
        dpi: 96,
    })
}

impl Lx {
    /// The monitor's work area: Mutter's per-monitor list when it exists,
    /// else the desktop work area clipped to the monitor.
    fn work_area(&mut self, monitor: Rect) -> Rect {
        let root = self.x.setup.root;
        let rects = |d: &[u8]| -> Vec<Rect> {
            d.as_chunks::<16>()
                .0
                .iter()
                .map(|c| {
                    let v = |i: usize| i32::from_le_bytes(c[i..i + 4].try_into().unwrap_or([0; 4]));
                    Rect {
                        left: v(0),
                        top: v(4),
                        right: v(0) + v(8),
                        bottom: v(4) + v(12),
                    }
                })
                .collect()
        };
        let candidates = self
            .x
            .get_property(root, self.a.gtk_workareas, false, 4096)
            .map(|(_, _, d)| rects(&d))
            .filter(|r| !r.is_empty())
            .or_else(|| {
                self.x
                    .get_property(root, self.a.workarea, false, 16)
                    .map(|(_, _, d)| rects(&d))
            })
            .unwrap_or_default();
        candidates
            .into_iter()
            .map(|r| unix::surface::intersect(r, monitor))
            .filter(|r| r.right > r.left && r.bottom > r.top)
            .max_by_key(|r| (r.right - r.left) * (r.bottom - r.top))
            .unwrap_or(monitor)
    }
}

pub fn system_dpi() -> u32 {
    with(|l| l.dpi).unwrap_or(96)
}

pub fn window_dpi(_h: HWND) -> u32 {
    system_dpi()
}

pub use super::unix::{SM_CXEDGE_, SM_CXICON_, SM_CXSMICON_};

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
        SM_CXSMICON_ => tray_icon_size(),
        SM_CXICON_ => metric_for_dpi(SM_CXICON_, system_dpi()),
        SM_CXEDGE_ => 2,
        // SM_CXDOUBLECLK, SM_CYDOUBLECLK
        36 | 37 => 4,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

pub use super::unix::{Dc, Surface};

/// Hand `f` the window as a drawing target (its back buffer's present).
pub fn paint(h: HWND, f: impl FnOnce(Dc<'_>)) {
    f(Dc::Window(h));
    core::validate(h);
}

/// Copy a finished back buffer to the window.
pub fn present_pixels(h: HWND, px: &[u32], w: usize, hgt: usize) {
    with(|l| {
        let dpi = l.dpi;
        let Some(win) = l.windows.get_mut(&h) else {
            return;
        };
        let (xid, depth, radius, inset) = (win.xid, win.depth, win.radius, win.inset);
        let gc = if depth == 32 { l.gc32 } else { Some(l.gc24) };
        let Some(gc) = gc else { return };
        let mut buf: Vec<u32> = px.iter().map(|p| 0xFF00_0000 | p).collect();
        if radius > 0 && depth == 32 {
            frame::round(&mut buf, w, hgt, radius);
        }
        if inset > 0 {
            let (cw, ch) = (w as i32, hgt as i32);
            if win.shadow.as_ref().is_none_or(|s| (s.0, s.1) != (cw, ch)) {
                let small = radius < (6.0 * f64::from(dpi) / 96.0) as i32;
                let (blur, dy) = shadow_metrics(small, f64::from(dpi) / 96.0);
                let opacity = if small { 0.22 } else { 0.30 };
                let sh = frame::shadow(
                    w,
                    hgt,
                    radius,
                    inset as usize,
                    blur as usize,
                    dy as usize,
                    opacity,
                );
                win.shadow = Some((cw, ch, sh));
            }
            if let Some((_, _, sh)) = &win.shadow {
                let i = inset as usize;
                let out = frame::compose(&buf, w, hgt, i, sh);
                l.x.put_image(xid, gc, depth, &out, w + 2 * i, hgt + 2 * i, 0, 0);
                l.x.flush();
                return;
            }
        }
        if radius > 0 && depth != 32 && win.shaped != (w as i32, hgt as i32) {
            win.shaped = (w as i32, hgt as i32);
            let rows = frame::shape_rows(w as i32, hgt as i32, radius);
            l.x.set_shape(xid, &rows);
        }
        l.x.put_image(xid, gc, depth, &buf, w, hgt, 0, 0);
        l.x.flush();
    });
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

pub use super::unix::{FontMetrics, Quality};

pub fn font_families() -> Vec<String> {
    font::families()
}

/// The desktop's default text font for drop-down lists: the UI face at
/// 9 pt for the display DPI.
pub fn tk_text_font() -> Option<Font> {
    let px = (12.0 * f64::from(system_dpi()) / 96.0).round() as i32;
    Font::new("", px, false, Quality::Default)
}

pub fn glyph_outline(font: &Font, ch: char, _hinted: bool) -> Option<Vec<crate::gfx::Contour>> {
    font.outline_px(ch)
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// An icon: frames of (side, straight-alpha RGBA).
#[derive(Clone)]
pub struct Icon {
    frames: Vec<(usize, Vec<[u8; 4]>)>,
}

pub fn icon_from_rgba(w: usize, h: usize, rgba: &[[u8; 4]]) -> Option<Icon> {
    (w == h && rgba.len() == w * h).then(|| Icon {
        frames: vec![(w, rgba.to_vec())],
    })
}

/// Several renderings of one icon, smallest first.
pub fn icon_from_frames(frames: Vec<crate::gfx::Rgba>) -> Option<Icon> {
    let frames: Vec<(usize, Vec<[u8; 4]>)> = frames
        .into_iter()
        .filter(|f| f.w == f.h)
        .map(|f| (f.w, f.px))
        .collect();
    (!frames.is_empty()).then_some(Icon { frames })
}

/// The app icon at `size` px.
pub fn app_icon(size: i32) -> Option<Icon> {
    let side = size.clamp(8, 256) as usize;
    let img = crate::png::ico_image(unix::APP_ICO, side as u32)?;
    let img = if img.w == side {
        img
    } else {
        img.resize(side, side, crate::gfx::Filter::Lanczos)
    };
    icon_from_rgba(side, side, &img.px)
}

pub fn set_window_icons(h: HWND, small: &Icon, big: &Icon) {
    with(|l| {
        if let Some(xid) = l.windows.get(&h).map(|w| w.xid) {
            l.icon_props(xid, &[big, small]);
        }
    });
}

pub use super::unix::app_icon_image;

// ---------------------------------------------------------------------------
// Tray
// ---------------------------------------------------------------------------

pub use super::unix::{TrayOp, WM_TRAY};

/// Pixel sizes to render the tray icon at: panels pick the best one.
pub fn tray_icon_sizes(size: usize) -> Vec<usize> {
    match with(|l| matches!(l.tray, TrayHost::Sni(_))) {
        Some(true) => vec![16, 22, 24, 32, 48, 64],
        _ => vec![size],
    }
}

/// The font size for the week number in a `size` px tray icon. An XEmbed
/// tray shows the icon at the size it gives the icon window, 16-24 px (more
/// on HiDPI) beside its own text, so there the number is set at the size of
/// panel text -- its digits as tall as the clock's -- instead of filling
/// the icon as on Windows' 16 px tray.
///
/// A D-Bus panel (StatusNotifierItem) picks one of the frames and scales it
/// to its own icon size, which the app is not told: there the number fills
/// each frame, as native tray icons fill theirs, so it keeps its proportion
/// whatever frame the panel picks and however it scales it.
pub fn tray_text_height(size: usize) -> usize {
    match with(|l| (matches!(l.tray, TrayHost::Sni(_)), l.dpi)) {
        Some((true, _)) => size,
        Some((false, dpi)) => text_height_for(size, dpi),
        None => text_height_for(size, 96),
    }
}

fn text_height_for(size: usize, dpi: u32) -> usize {
    let px = (PANEL_FONT_PX * f64::from(dpi) / 96.0).round() as usize;
    px.clamp(8, size.max(8))
}

/// Panel text at 96 dpi: 13 px (about 10 pt), digits about 9 px tall --
/// between Xfce's small clock and GNOME's larger one.
const PANEL_FONT_PX: f64 = 13.0;

fn tray_icon_size() -> i32 {
    with(|l| match l.tray {
        TrayHost::Xembed { size, .. } => size,
        _ => (22.0 * f64::from(l.dpi) / 96.0).round() as i32,
    })
    .unwrap_or(22)
    .clamp(16, 128)
}

/// Whether to dock in the XEmbed tray before offering a D-Bus item: on
/// GNOME Shell. Its AppIndicator extension opens a D-Bus item's menu on a
/// left click and sends Activate only on a double click, and never sends
/// ContextMenu; its legacy tray, which also shows XEmbed icons, hands each
/// click to the icon as it is. Docked there, a left click opens the
/// calendar and a right click the menu, as on every other desktop. Where
/// that tray is off, the D-Bus item is used as before.
fn prefers_xembed() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| is_gnome_shell(&d))
}

/// XDG_CURRENT_DESKTOP of GNOME Shell sessions: "GNOME", "ubuntu:GNOME",
/// "GNOME-Classic:GNOME" -- not desktops that only name GNOME as their
/// base ("Budgie:GNOME", "GNOME-Flashback:GNOME", "Pantheon").
fn is_gnome_shell(desktop: &str) -> bool {
    let names: Vec<&str> = desktop.split(':').map(str::trim).collect();
    names.iter().any(|n| n.eq_ignore_ascii_case("GNOME"))
        && !names.iter().any(|n| {
            [
                "Budgie",
                "GNOME-Flashback",
                "Pantheon",
                "Unity",
                "X-Cinnamon",
                "MATE",
                "XFCE",
            ]
            .iter()
            .any(|o| n.eq_ignore_ascii_case(o))
        })
}

/// A colormap for the ARGB visual `visual` and a GC for depth-32 drawables.
fn argb_resources(x: &mut Conn, visual: u32) -> (u32, u32) {
    let root = x.setup.root;
    let cmap = x.gen_id();
    let mut b = Vec::new();
    b.extend_from_slice(&cmap.to_le_bytes());
    b.extend_from_slice(&root.to_le_bytes());
    b.extend_from_slice(&visual.to_le_bytes());
    x.request(78, 0, &b); // CreateColormap, alloc None
    // A GC for depth 32 needs a depth-32 drawable: a 1x1 pixmap.
    let pix = x.gen_id();
    let mut b = Vec::new();
    b.extend_from_slice(&pix.to_le_bytes());
    b.extend_from_slice(&root.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    x.request(53, 32, &b); // CreatePixmap
    let gc = x.create_gc(pix);
    x.request(54, 0, &pix.to_le_bytes()); // FreePixmap
    (gc, cmap)
}

/// Whether the session is Wayland (the X server is XWayland). A Flatpak
/// without the Wayland socket has no WAYLAND_DISPLAY, but still the
/// session type.
fn on_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

/// The tray menu as the panel draws it (D-Bus menus); ids are tray
/// commands, `checked` None for plain items.
pub fn set_tray_menu(items: Vec<(u32, String, Option<bool>)>) {
    let items = items
        .into_iter()
        .map(|(id, label, checked)| {
            if id == 0 {
                sni::MenuItem::Separator
            } else {
                sni::MenuItem::Item { id, label, checked }
            }
        })
        .collect::<Vec<_>>();
    with(|l| {
        l.tray_menu = items.clone();
        if let TrayHost::Sni(s) = &mut l.tray {
            s.set_menu(items);
        }
    });
}

/// Add, update or remove the tray icon.
pub fn tray_icon(_h: HWND, op: TrayOp, icon: Option<&Icon>, tip: &str) -> bool {
    with(|l| {
        match op {
            TrayOp::Delete => {
                if let TrayHost::Xembed { xid, .. } = l.tray {
                    l.x.destroy_window(xid);
                    l.by_xid.remove(&xid);
                    l.windows.remove(&HWND(usize::MAX - 1));
                }
                l.tray = TrayHost::None;
                l.x.flush();
                return true;
            }
            TrayOp::Add | TrayOp::Modify => {
                if let Some(i) = icon {
                    l.tray_icon = Some(i.clone());
                }
                l.tray_tip = tip.to_string();
                if matches!(l.tray, TrayHost::None) {
                    l.attach_tray();
                }
                l.update_tray();
            }
        }
        !matches!(l.tray, TrayHost::None)
    })
    .unwrap_or(false)
}

impl Lx {
    /// StatusNotifierItem when a panel hosts it, else the XEmbed tray (on
    /// GNOME Shell the XEmbed tray first, see prefers_xembed).
    fn attach_tray(&mut self) {
        if prefers_xembed() && self.dock_xembed() {
            return;
        }
        if let Some(s) = sni::Sni::start() {
            let mut s = Box::new(s);
            if s.registered() {
                s.set_menu(self.tray_menu.clone());
                self.tray = TrayHost::Sni(s);
                return;
            }
            if self.dock_xembed() {
                return;
            }
            // Nothing hosts the icon yet: keep the bus connection, a panel
            // may start later (and the X tray is watched for as well).
            self.tray = TrayHost::Sni(s);
            return;
        }
        self.dock_xembed();
    }

    fn dock_xembed(&mut self) -> bool {
        let manager = self.x.get_selection_owner(self.a.tray_selection);
        if manager == 0 {
            return false;
        }
        let size = self.xembed_size;
        let xid = self.x.gen_id();
        let root = self.x.setup.root;
        // The tray's preferred visual, when it offers an ARGB one.
        let visual_atom = self.x.atom("_NET_SYSTEM_TRAY_VISUAL");
        let tray_visual = self
            .x
            .get_property(manager, visual_atom, false, 4)
            .and_then(|(_, _, d)| {
                d.get(..4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            });
        // An ARGB icon wherever the tray asks for one, with or without a
        // compositing manager: the tray blends it over the panel itself.
        // (Opaque, the icon would show the colour the tray fills it with:
        // a black square on GNOME Shell's top bar.)
        let argb = match self.x.setup.argb.filter(|v| Some(v.id) == tray_visual) {
            Some(v) => match (self.gc32, self.argb_colormap) {
                (Some(_), Some(cmap)) => Some((v, cmap)),
                _ => {
                    let (_, cmap) = *self
                        .tray_argb
                        .get_or_insert_with(|| argb_resources(&mut self.x, v.id));
                    Some((v, cmap))
                }
            },
            None => None,
        };
        let events = x11::EV_BUTTON_PRESS
            | x11::EV_BUTTON_RELEASE
            | x11::EV_EXPOSURE
            | x11::EV_STRUCTURE
            | x11::EV_POINTER_MOTION;
        match argb {
            Some((v, cmap)) => self.x.create_window(
                xid,
                root,
                0,
                0,
                size,
                size,
                32,
                v.id,
                x11::CW_BACK_PIXEL | x11::CW_BORDER_PIXEL | x11::CW_EVENT_MASK | x11::CW_COLORMAP,
                &[0, 0, events, cmap],
            ),
            None => self.x.create_window(
                xid,
                root,
                0,
                0,
                size,
                size,
                0,
                0,
                0x1 /* CWBackPixmap */ | x11::CW_EVENT_MASK,
                &[1 /* ParentRelative */, events],
            ),
        }
        self.x
            .change_property32(xid, self.a.xembed_info, self.a.xembed_info, &[0, 1]);
        self.set_title_props(xid, "WeekNumber");
        self.x
            .change_property(xid, 67, 31, 8, b"weeknumber\0WeekNumber\0");
        let e = Conn::client_message(manager, self.a.tray_opcode, [0, 0, xid, 0, 0]);
        self.x.send_event(manager, 0, &e);
        self.x.flush();
        self.tray = TrayHost::Xembed { xid, size };
        self.windows.insert(
            HWND(usize::MAX - 1),
            Win {
                xid,
                kind: WinKind::Top(Kind::Tip),
                x: 0,
                y: 0,
                w: size,
                h: size,
                depth: if argb.is_some() {
                    32
                } else {
                    self.x.setup.root_depth
                },
                mapped: true,
                radius: 0,
                inset: 0,
                shadow: None,
                shaped: (0, 0),
                cursor: unix::Cursor::Arrow,
            },
        );
        self.by_xid.insert(xid, HWND(usize::MAX - 1));
        true
    }

    fn update_tray(&mut self) {
        let tip = self.tray_tip.clone();
        let icon = self.tray_icon.clone();
        // An icon rendered for another kind of tray (before the tray was
        // found, or before it changed): the app renders it anew, at every
        // size a D-Bus panel picks from or at the XEmbed icon's own size.
        let frames = icon.as_ref().map_or(0, |i| i.frames.len());
        let stale = match &self.tray {
            TrayHost::Sni(s) => s.registered() && frames == 1,
            TrayHost::Xembed { .. } => frames > 1,
            TrayHost::None => false,
        };
        if stale && let Some(h) = self.tray_hwnd {
            core::post(h, WM_TRAY, 0, TRAY_ICON_REFRESH as isize);
        }
        match &mut self.tray {
            TrayHost::Sni(s) if s.registered() => {
                if let Some(i) = &icon {
                    s.set_icon(&i.frames);
                }
                s.set_tooltip(&tip);
            }
            TrayHost::Xembed { xid, size } => {
                let (xid, size) = (*xid, *size);
                self.paint_xembed(xid, size);
            }
            _ => {}
        }
        self.x.flush();
    }

    fn paint_xembed(&mut self, xid: u32, size: i32) {
        let Some(icon) = &self.tray_icon else { return };
        let side = size.max(1) as usize;
        let frame = icon
            .frames
            .iter()
            .min_by_key(|(s, _)| s.abs_diff(side))
            .cloned();
        let Some((fs, px)) = frame else { return };
        let img = crate::gfx::Rgba { w: fs, h: fs, px };
        let img = if fs == side {
            img
        } else {
            img.resize(side, side, crate::gfx::Filter::Lanczos)
        };
        let depth = self
            .windows
            .get(&HWND(usize::MAX - 1))
            .map_or(self.x.setup.root_depth, |w| w.depth);
        let buf: Vec<u32> = if depth == 32 {
            img.px
                .iter()
                .map(|p| {
                    let a = u32::from(p[3]);
                    let pm = |c: u8| (u32::from(c) * a + 127) / 255;
                    (a << 24) | (pm(p[0]) << 16) | (pm(p[1]) << 8) | pm(p[2])
                })
                .collect()
        } else {
            // Blend over the panel behind the icon window.
            let mut b = Vec::new();
            b.extend_from_slice(&xid.to_le_bytes());
            b.extend_from_slice(&[0, 0, 0, 0]);
            b.extend_from_slice(&0u32.to_le_bytes());
            self.x.request(61, 0, &b); // ClearArea
            let bg = self.get_image(xid, side as u16);
            img.px
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let mut d = bg.as_ref().and_then(|v| v.get(i).copied()).unwrap_or(0);
                    unix::surface::blend_pixel(
                        &mut d,
                        (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]),
                        u32::from(p[3]),
                    );
                    d
                })
                .collect()
        };
        let gc = if depth == 32 {
            self.gc32.or(self.tray_argb.map(|(gc, _)| gc))
        } else {
            Some(self.gc24)
        };
        if let Some(gc) = gc {
            self.x.put_image(xid, gc, depth, &buf, side, side, 0, 0);
        }
    }

    fn get_image(&mut self, xid: u32, side: u16) -> Option<Vec<u32>> {
        let mut b = Vec::new();
        b.extend_from_slice(&xid.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(&side.to_le_bytes());
        b.extend_from_slice(&side.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        let s = self.x.request(73, 2, &b); // GetImage, ZPixmap
        let r = self.x.reply(s)?;
        Some(
            r[32..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c) & 0xFF_FFFF)
                .collect(),
        )
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

pub fn set_clipboard_text(_owner: HWND, text: &str) -> bool {
    with(|l| {
        l.clipboard = Some(text.to_string());
        let (w, sel, t) = (l.utility, l.a.clipboard, l.time);
        l.x.set_selection_owner(w, sel, t);
        l.x.flush();
        true
    })
    .unwrap_or(false)
}

/// Ask the clipboard owner for text; it arrives for `target` (an edit).
fn request_paste(target: HWND) {
    with(|l| {
        l.paste_into = Some(target);
        let (w, sel, utf8, prop, t) = (l.utility, l.a.clipboard, l.a.utf8, l.a.paste, l.time);
        l.x.convert_selection(w, sel, utf8, prop, t);
        l.x.flush();
    });
}

impl Lx {
    fn serve_selection(
        &mut self,
        time: u32,
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
    ) {
        let property = if property == 0 { target } else { property };
        let ok = match (&self.clipboard, selection == self.a.clipboard) {
            (Some(text), true) => {
                if target == self.a.targets {
                    let list = [self.a.targets, self.a.utf8, 31 /* STRING */];
                    self.x.change_property32(requestor, property, 4, &list);
                    true
                } else if target == self.a.utf8 || target == 31 {
                    let data = text.clone().into_bytes();
                    self.x
                        .change_property(requestor, property, target, 8, &data);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        let mut e = [0u8; 32];
        e[0] = 31;
        e[4..8].copy_from_slice(&time.to_le_bytes());
        e[8..12].copy_from_slice(&requestor.to_le_bytes());
        e[12..16].copy_from_slice(&selection.to_le_bytes());
        e[16..20].copy_from_slice(&target.to_le_bytes());
        e[20..24].copy_from_slice(&(if ok { property } else { 0 }).to_le_bytes());
        self.x.send_event(requestor, 0, &e);
        self.x.flush();
    }
}

// ---------------------------------------------------------------------------
// Edit control and message box (drawn by the backend itself)
// ---------------------------------------------------------------------------

pub use super::unix::{
    Brush, EN_CHANGE_, EN_KILLFOCUS_, EN_SETFOCUS_, command_parts, rect_from_lparam,
};
pub use edit::{create_edit, ctl_color, get_text, move_child, select_all, set_font};

pub fn message_box_error(text: &str) {
    msgbox::show(text);
}

/// A question with `buttons` (the first is the default): the index pressed.
pub fn message_box_choose(text: &str, buttons: &[&str]) -> Option<usize> {
    msgbox::choose(text, buttons)
}

/// Messages for the backend's own windows, before the app sees them.
fn filter(h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
    if edit::is_edit(h) {
        return edit::on_message(h, msg, wp, lp);
    }
    if msgbox::owns(h) {
        return msgbox::on_message(h, msg, wp, lp);
    }
    None
}

// ---------------------------------------------------------------------------
// Time, instance and keys shared with macOS
// ---------------------------------------------------------------------------

pub use super::unix::{
    TzInfo, VK_CONTROL_, VK_ESCAPE_, VK_RETURN_, VK_SHIFT_, VK_TAB_, WM_APP_KEY, WS_DIALOG,
    WS_EX_NOACTIVATE_, WS_EX_TOOLWINDOW_, WS_EX_TOPMOST_, WS_POPUP_, WS_RESIZABLE, WallTime,
    current_time_zone, enum_time_zones, local_now, utc_now, utc_to_zone,
};

/// X keysym -> Win32 virtual key and the character it types.
fn translate_key(sym: u32) -> (u16, Option<char>) {
    let vk = match sym {
        0xFF1B => 0x1B,
        0xFF0D | 0xFF8D => 0x0D,
        0xFF09 | 0xFE20 => 0x09,
        0xFF08 => 0x08,
        0xFF51 => 0x25,
        0xFF52 => 0x26,
        0xFF53 => 0x27,
        0xFF54 => 0x28,
        0xFF55 => 0x21,
        0xFF56 => 0x22,
        0xFF50 => 0x24,
        0xFF57 => 0x23,
        0xFFFF => 0x2E,
        0x20 => 0x20,
        0x30..=0x39 => sym as u16,
        0x41..=0x5A => sym as u16,
        0x61..=0x7A => (sym - 0x20) as u16,
        0xFFE1 | 0xFFE2 => 0x10,
        0xFFE3 | 0xFFE4 => 0x11,
        _ => 0,
    };
    let ch = match sym {
        0x20..=0x7E | 0xA0..=0xFF => char::from_u32(sym),
        0x0100_0100..=0x0110_FFFF => char::from_u32(sym - 0x0100_0000),
        _ => None,
    };
    (vk, ch)
}

// ---------------------------------------------------------------------------
// The message loop
// ---------------------------------------------------------------------------

pub fn run_message_loop(instance: &Instance, mut on_event: impl FnMut()) -> i32 {
    use std::os::fd::AsRawFd;
    loop {
        core::pump();
        if let Some(code) = core::quit_requested() {
            return code;
        }
        let fds = with(|l| {
            l.x.flush();
            l.x.discard_stale();
            let mut fds = vec![l.x.fd()];
            if let TrayHost::Sni(s) = &l.tray {
                fds.push(s.fd());
            }
            (fds, l.x.has_events())
        });
        let Some((mut fds, pending)) = fds else {
            return 1;
        };
        let listener = instance.listener().map(|l| l.as_raw_fd());
        if let Some(fd) = listener {
            fds.push(fd);
        }
        if !pending {
            let timeout =
                core::next_timeout().map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as i32);
            unix::sys::wait_readable(&fds, timeout);
        }
        let dead = with(|l| {
            l.x.read_input(false);
            l.x.dead
        });
        if dead != Some(false) {
            return 0;
        }
        while let Some(e) = with(|l| l.x.next_event()).flatten() {
            on_x_event(e);
        }
        handle_tray();
        if instance.quit_requested() {
            on_event();
        }
    }
}

fn handle_tray() {
    let (events, hwnd) = match with(|l| {
        let ev = match &mut l.tray {
            TrayHost::Sni(s) => {
                let was = s.registered();
                let ev = s.handle();
                if !was && s.registered() {
                    // A panel appeared: move over from XEmbed, if docked there.
                    let menu = l.tray_menu.clone();
                    s.set_menu(menu);
                }
                ev
            }
            _ => Vec::new(),
        };
        if !ev.is_empty() {
            l.update_tray();
        }
        (ev, l.tray_hwnd)
    }) {
        Some((ev, Some(h))) => (ev, h),
        _ => return,
    };
    for e in events {
        match e {
            sni::TrayEvent::Activate(x, y) => {
                with(|l| {
                    l.anchor = ((x, y) != (0, 0)).then(|| (x, y, std::time::Instant::now()));
                });
                core::post(hwnd, WM_TRAY, 0, wm::WM_LBUTTONUP as isize);
            }
            sni::TrayEvent::ContextMenu => {
                core::post(hwnd, WM_TRAY, 0, wm::WM_RBUTTONUP as isize);
            }
            sni::TrayEvent::Menu(id) => {
                core::post(hwnd, WM_TRAY, id as usize, wm::WM_COMMAND as isize)
            }
            sni::TrayEvent::AboutToShow => core::post(hwnd, WM_TRAY, 0, TRAY_MENU_REFRESH as isize),
        }
    }
}

/// The shadow margin of a pop-up (0 for other windows).
fn inset_of(h: HWND) -> i32 {
    with(|l| l.windows.get(&h).map_or(0, |w| w.inset)).unwrap_or(0)
}

/// WM_TRAY lparam: the panel is about to show the menu.
pub const TRAY_MENU_REFRESH: u32 = 0x7F00;
/// WM_TRAY lparam: render the icon anew for the tray now hosting it.
pub const TRAY_ICON_REFRESH: u32 = 0x7F01;

fn on_x_event(e: Event) {
    match e {
        Event::Expose { window, count } => {
            if count == 0
                && let Some(h) = hwnd_of(window)
            {
                if h == HWND(usize::MAX - 1) {
                    with(Lx::update_tray);
                } else {
                    core::invalidate(h);
                }
            }
        }
        Event::Configure { window, x, y, w, h } => {
            let changed = with(|l| {
                if let TrayHost::Xembed { xid, size } = &mut l.tray
                    && *xid == window
                {
                    let s = w.min(h).max(16);
                    let changed = s != l.xembed_size;
                    *size = s;
                    l.xembed_size = s;
                    return changed.then_some((None, true));
                }
                let hwnd = *l.by_xid.get(&window)?;
                let win = l.windows.get_mut(&hwnd)?;
                let i = win.inset;
                let (w, h) = (w - 2 * i, h - 2 * i);
                let resized = (win.w, win.h) != (w, h);
                if let WinKind::Top(Kind::Popup | Kind::Tip) = win.kind {
                    win.x = x + i;
                    win.y = y + i;
                }
                win.w = w;
                win.h = h;
                Some((resized.then_some((hwnd, w, h)), false))
            })
            .flatten();
            match changed {
                Some((Some((hwnd, w, h)), _)) => {
                    core::send(hwnd, wm::WM_SIZE, 0, pack_xy(w, h));
                    core::invalidate(hwnd);
                }
                Some((None, true)) => {
                    // The tray icon has a new size: the app renders anew.
                    if let Some(t) = with(|l| l.tray_hwnd).flatten() {
                        core::post(t, wm::WM_DISPLAYCHANGE, 0, 0);
                    }
                }
                _ => {}
            }
        }
        Event::Client { window, kind, data } => {
            let (delete, protocols, manager, sel) =
                with(|l| (l.a.delete, l.a.protocols, l.a.manager, l.a.tray_selection))
                    .unwrap_or((0, 0, 0, 0));
            if kind == protocols && data[0] == delete {
                if let Some(h) = hwnd_of(window) {
                    core::send(h, wm::WM_CLOSE, 0, 0);
                }
            } else if kind == manager && data[1] == sel {
                // A system tray started: dock if nothing hosts the icon yet
                // (on GNOME Shell, move there from the D-Bus item).
                let docked = with(|l| {
                    let none = match &l.tray {
                        TrayHost::None => true,
                        TrayHost::Sni(s) => !s.registered() || prefers_xembed(),
                        TrayHost::Xembed { .. } => false,
                    };
                    if none && l.dock_xembed() {
                        l.update_tray();
                    }
                });
                let _ = docked;
            }
        }
        Event::Key {
            press,
            code,
            state,
            window,
            time,
        } => {
            if !press {
                return;
            }
            let shift = state & x11::STATE_SHIFT != 0;
            let ctrl = state & x11::STATE_CONTROL != 0;
            core::set_modifiers(shift, ctrl);
            let Some((sym, h, grab)) = with(|l| {
                l.time = time;
                let sym = l.x.keysym(code, shift);
                let h = l.by_xid.get(&window).copied();
                (sym, h, l.grab)
            }) else {
                return;
            };
            // Keys go to the grabbing pop-up, else the focused window.
            let Some(h) = grab.or(h) else { return };
            let (vk, ch) = translate_key(sym);
            if edit::is_edit(h) || edit::focused_in(h).is_some() {
                let target = edit::focused_in(h).unwrap_or(h);
                if edit::on_key(target, vk, ch, shift, ctrl) {
                    return;
                }
            }
            if vk != 0 {
                core::send(h, wm::WM_KEYDOWN, usize::from(vk), 0);
            }
            if let Some(c) = ch
                && !ctrl
            {
                core::send(h, wm::WM_CHAR, c as usize, 0);
            }
        }
        Event::Button {
            press,
            button,
            state,
            window,
            x,
            y,
            root_x,
            root_y,
            time,
        } => {
            core::set_modifiers(
                state & x11::STATE_SHIFT != 0,
                state & x11::STATE_CONTROL != 0,
            );
            with(|l| l.time = time);
            if let Some(tray) = with(|l| match l.tray {
                TrayHost::Xembed { xid, .. } if xid == window => l.tray_hwnd,
                _ => None,
            })
            .flatten()
            {
                if !press {
                    let m = match button {
                        1 => wm::WM_LBUTTONUP,
                        3 => wm::WM_RBUTTONUP,
                        _ => return,
                    };
                    // The pop-ups open at the icon: its centre on the
                    // screen (GNOME Shell keeps the icon window where it
                    // shows the icon), or where the click was reported.
                    with(|l| {
                        let root = l.x.setup.root;
                        let centre = match l.tray {
                            TrayHost::Xembed { xid, size } => {
                                l.x.translate(xid, root, size / 2, size / 2)
                            }
                            _ => None,
                        };
                        let at = centre
                            .into_iter()
                            .chain([(root_x, root_y)])
                            .find(|p| l.on_a_panel(*p));
                        l.anchor = at.map(|(x, y)| (x, y, std::time::Instant::now()));
                    });
                    core::post(tray, WM_TRAY, 0, m as isize);
                }
                return;
            }
            let Some((h, grab)) = with(|l| (l.by_xid.get(&window).copied(), l.grab)) else {
                return;
            };
            // A press outside the grabbing pop-up dismisses it.
            if let Some(g) = grab
                && press
            {
                let inside = h == Some(g)
                    && with(|l| {
                        l.windows.get(&g).is_some_and(|w| {
                            root_x >= w.x
                                && root_y >= w.y
                                && root_x < w.x + w.w
                                && root_y < w.y + w.h
                        })
                    })
                    .unwrap_or(false);
                if !inside {
                    let old = with(Lx::deactivate).flatten();
                    if let Some(old) = old {
                        core::send(old, wm::WM_ACTIVATE, wm::WA_INACTIVE as usize, 0);
                    }
                    return;
                }
            }
            let Some(h) = h else { return };
            if edit::is_edit(h) {
                if press && button == 1 {
                    edit::click(h, x);
                }
                return;
            }
            let i = inset_of(h);
            let (x, y) = (x - i, y - i);
            let msg = match (button, press) {
                (1, true) => wm::WM_LBUTTONDOWN,
                (1, false) => wm::WM_LBUTTONUP,
                (3, true) => wm::WM_RBUTTONDOWN,
                (3, false) => wm::WM_RBUTTONUP,
                (4, true) => {
                    core::send(h, wm::WM_MOUSEWHEEL, 120 << 16, pack_xy(root_x, root_y));
                    return;
                }
                (5, true) => {
                    core::send(
                        h,
                        wm::WM_MOUSEWHEEL,
                        ((-120i32 as u16) as usize) << 16,
                        pack_xy(root_x, root_y),
                    );
                    return;
                }
                _ => return,
            };
            core::send(h, msg, 0, pack_xy(x, y));
        }
        Event::Motion {
            window,
            x,
            y,
            state,
            time,
        } => {
            core::set_modifiers(
                state & x11::STATE_SHIFT != 0,
                state & x11::STATE_CONTROL != 0,
            );
            let Some(h) = with(|l| {
                l.time = time;
                let h = l.by_xid.get(&window).copied();
                if h.is_some() {
                    l.pointer_in = h;
                }
                h
            })
            .flatten() else {
                return;
            };
            if h == HWND(usize::MAX - 1) {
                if let Some(t) = with(|l| l.tray_hwnd).flatten() {
                    core::post(t, WM_TRAY, 0, wm::WM_MOUSEMOVE as isize);
                }
                return;
            }
            core::send(
                h,
                wm::WM_SETCURSOR,
                h.0,
                (wm::HTCLIENT | (wm::WM_MOUSEMOVE << 16)) as isize,
            );
            let i = inset_of(h);
            core::send(h, wm::WM_MOUSEMOVE, 0, pack_xy(x - i, y - i));
        }
        Event::Crossing {
            enter,
            window,
            mode,
            time,
        } => {
            with(|l| l.time = time);
            let Some(h) = hwnd_of(window) else { return };
            if enter {
                with(|l| l.pointer_in = Some(h));
            } else if mode == 0 {
                with(|l| {
                    if l.pointer_in == Some(h) {
                        l.pointer_in = None;
                    }
                });
                core::mouse_left(h);
            }
        }
        Event::Focus {
            gained,
            window,
            mode,
            detail,
        } => {
            // Ignore grab bookkeeping and focus moving inside the window.
            if mode != 0 || detail == 2 || detail == 5 || detail == 7 {
                return;
            }
            let Some(h) = hwnd_of(window) else { return };
            if edit::is_edit(h) {
                if !gained {
                    edit::blur(h);
                }
                return;
            }
            let kind = with(|l| l.windows.get(&h).map(|w| w.kind)).flatten();
            match (gained, kind) {
                (true, Some(WinKind::Top(Kind::Dialog { .. }) | WinKind::MessageBox)) => {
                    let old = with(|l| {
                        if l.active == Some(h) {
                            return None;
                        }
                        let old = l.deactivate();
                        l.active = Some(h);
                        Some(old)
                    })
                    .flatten();
                    if let Some(old) = old {
                        if let Some(o) = old {
                            core::send(o, wm::WM_ACTIVATE, wm::WA_INACTIVE as usize, 0);
                        }
                        core::send(h, wm::WM_ACTIVATE, wm::WA_ACTIVE as usize, 0);
                    }
                }
                (false, Some(WinKind::Top(Kind::Dialog { .. }) | WinKind::MessageBox)) => {
                    let lost = with(|l| {
                        if l.active == Some(h) {
                            l.active = None;
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                    if lost {
                        core::send(h, wm::WM_ACTIVATE, wm::WA_INACTIVE as usize, 0);
                    }
                }
                _ => {}
            }
        }
        Event::SelectionRequest {
            time,
            requestor,
            selection,
            target,
            property,
        } => {
            with(|l| l.serve_selection(time, requestor, selection, target, property));
        }
        Event::SelectionClear { selection } => {
            with(|l| {
                if selection == l.a.clipboard {
                    l.clipboard = None;
                }
            });
        }
        Event::SelectionNotify {
            requestor,
            property,
        } => {
            let pasted = with(|l| {
                let target = l.paste_into.take()?;
                if requestor != l.utility || property == 0 {
                    return None;
                }
                let (w, p) = (l.utility, l.a.paste);
                let (_, _, data) = l.x.get_property(w, p, true, 4096)?;
                Some((target, String::from_utf8_lossy(&data).into_owned()))
            })
            .flatten();
            if let Some((target, text)) = pasted {
                edit::paste(target, &text);
            }
        }
        Event::Property { window, atom } => {
            let root_change =
                with(|l| window == l.x.setup.root && atom == l.x.atom("RESOURCE_MANAGER"))
                    .unwrap_or(false);
            if root_change {
                with(|l| l.dpi = read_dpi(&mut l.x));
            }
        }
        Event::Mapping => {
            with(|l| l.x.reload_keymap());
        }
        Event::Structure | Event::Other => {}
    }
}

/// For the edit control: request a paste into `h`.
pub(super) fn paste_request(h: HWND) {
    request_paste(h);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(translate_key(0xFF1B), (0x1B, None));
        assert_eq!(translate_key(0x61), (0x41, Some('a')));
        assert_eq!(translate_key(0x41), (0x41, Some('A')));
        assert_eq!(translate_key(0x23), (0, Some('#')));
        assert_eq!(translate_key(0x0100_20AC), (0, Some('\u{20AC}')));
        assert_eq!(translate_key(0xFF51), (0x25, None));
    }

    #[test]
    fn gnome_shell_sessions() {
        for d in ["GNOME", "ubuntu:GNOME", "GNOME-Classic:GNOME", "gnome"] {
            assert!(is_gnome_shell(d), "{d}");
        }
        for d in [
            "KDE",
            "X-Cinnamon",
            "Budgie:GNOME",
            "GNOME-Flashback:GNOME",
            "XFCE",
            "",
        ] {
            assert!(!is_gnome_shell(d), "{d}");
        }
    }

    #[test]
    fn tray_digits_match_panel_text() {
        // GNOME (16), KDE Plasma and Cinnamon (22), Xfce (24), at 100 %.
        assert_eq!(text_height_for(16, 96), 13);
        assert_eq!(text_height_for(22, 96), 13);
        assert_eq!(text_height_for(24, 96), 13);
        // At 200 % the number doubles with the panel's text.
        assert_eq!(text_height_for(48, 192), 26);
        // A small icon at a high DPI: never taller than the icon.
        assert_eq!(text_height_for(16, 192), 16);
    }

    #[test]
    fn panel_corners() {
        let r = |l, t, r, b| Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let mon = r(0, 0, 1920, 1080);
        // GNOME: top bar.
        assert_eq!(panel_corner(mon, r(0, 32, 1920, 1080)), (1919, 0));
        // KDE Plasma, Cinnamon: bottom panel.
        assert_eq!(panel_corner(mon, r(0, 0, 1920, 1036)), (1919, 1079));
        // Unity-style dock on the left: the tray is still at the bottom.
        assert_eq!(panel_corner(mon, r(64, 0, 1920, 1080)), (0, 1079));
        // Nothing reserved.
        assert_eq!(panel_corner(mon, mon), (1919, 0));
        // A click on the bottom panel counts; one in the middle of the
        // screen (half-scale coordinates of a HiDPI panel) does not.
        let work = r(0, 0, 1920, 1036);
        assert!(on_panel((1800, 1060), mon, work, 64));
        assert!(!on_panel((900, 530), mon, work, 64));
        assert!(on_panel((1900, 10), mon, mon, 64));
        assert!(!on_panel((900, 530), mon, mon, 64));
    }

    #[test]
    fn mouse_lparam() {
        let lp = pack_xy(-3, 70);
        assert_eq!(crate::ui::mouse_xy(lp), (-3, 70));
    }
}
