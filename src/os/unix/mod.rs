//! What the macOS and Linux backends share: the Win32 types and message
//! numbers the app's windows are written against, the message core, the
//! back buffer, time zones from the IANA database and single-instance
//! hand-over.

pub mod core;
pub mod instance;
pub mod surface;
pub mod sys;
pub mod tzif;

pub use instance::Instance;
pub use surface::{Dc, Surface};

use crate::dates::Date;
use std::cell::RefCell;
use std::rc::Rc;

/// A window handle: a number the backend maps to its native window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(clippy::upper_case_acronyms)]
pub struct HWND(pub usize);

impl HWND {
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// The Win32 message numbers and constants the windows use, with Win32's
/// values.
#[allow(dead_code)]
pub mod wm {
    pub const WM_DESTROY: u32 = 0x0002;
    pub const WM_SIZE: u32 = 0x0005;
    pub const WM_ACTIVATE: u32 = 0x0006;
    pub const WM_PAINT: u32 = 0x000F;
    pub const WM_CLOSE: u32 = 0x0010;
    pub const WM_ERASEBKGND: u32 = 0x0014;
    pub const WM_TIMECHANGE: u32 = 0x001E;
    pub const WM_SETCURSOR: u32 = 0x0020;
    pub const WM_MOUSEACTIVATE: u32 = 0x0021;
    pub const WM_GETMINMAXINFO: u32 = 0x0024;
    pub const WM_DISPLAYCHANGE: u32 = 0x007E;
    pub const WM_KEYDOWN: u32 = 0x0100;
    pub const WM_CHAR: u32 = 0x0102;
    pub const WM_COMMAND: u32 = 0x0111;
    pub const WM_TIMER: u32 = 0x0113;
    pub const WM_CTLCOLOREDIT: u32 = 0x0133;
    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_RBUTTONUP: u32 = 0x0205;
    pub const WM_MOUSEWHEEL: u32 = 0x020A;
    pub const WM_POWERBROADCAST: u32 = 0x0218;
    pub const WM_MOUSELEAVE: u32 = 0x02A3;
    pub const WM_DPICHANGED: u32 = 0x02E0;
    pub const WM_APP: u32 = 0x8000;
    pub const WA_INACTIVE: u32 = 0;
    pub const WA_ACTIVE: u32 = 1;
    pub const HTCLIENT: u32 = 1;
    pub const PBT_APMRESUMEAUTOMATIC: u32 = 0x12;

    /// A GetSystemMetrics index, as the windows crate wraps it.
    #[derive(Clone, Copy)]
    pub struct SystemMetric(pub i32);
    pub const SM_CXDOUBLECLK: SystemMetric = SystemMetric(36);
    pub const SM_CYDOUBLECLK: SystemMetric = SystemMetric(37);
}

pub const WM_TRAY: u32 = wm::WM_APP + 1;
pub const WM_APP_KEY: u32 = wm::WM_APP + 0x20;
pub const VK_TAB_: u16 = 0x09;
pub const VK_RETURN_: u16 = 0x0D;
pub const VK_ESCAPE_: u16 = 0x1B;
pub const VK_SHIFT_: u16 = 0x10;
pub const VK_CONTROL_: u16 = 0x11;
pub const SM_CXSMICON_: i32 = 49;
pub const SM_CXICON_: i32 = 11;
pub const SM_CXEDGE_: i32 = 45;
pub const EN_SETFOCUS_: u16 = 0x0100;
pub const EN_KILLFOCUS_: u16 = 0x0200;
pub const EN_CHANGE_: u16 = 0x0300;

pub const WS_POPUP_: u32 = 0x8000_0000;
pub const WS_DIALOG: u32 = 0x00CA_0000;
/// A dialog the user can resize and maximise (the Legal notices window).
pub const WS_RESIZABLE: u32 = WS_DIALOG | 0x0005_0000;
pub const WS_EX_TOPMOST_: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW_: u32 = 0x0000_0080;
pub const WS_EX_NOACTIVATE_: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorGeom {
    pub monitor: Rect,
    pub work: Rect,
    pub dpi: u32,
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

/// What kind of native window a WindowSpec asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Borderless, above the others, takes the keyboard (flyout, menus,
    /// drop-down lists).
    Popup,
    /// Borderless and never focused (the holiday tooltip).
    Tip,
    /// A titled window the window manager decorates.
    Dialog { resizable: bool },
}

impl Kind {
    pub fn of(spec: &WindowSpec<'_>) -> Kind {
        if spec.style & WS_POPUP_ != 0 {
            if spec.ex_style & WS_EX_NOACTIVATE_ != 0 {
                Kind::Tip
            } else {
                Kind::Popup
            }
        } else {
            Kind::Dialog {
                resizable: spec.style & WS_RESIZABLE == WS_RESIZABLE,
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cursor {
    Arrow,
    Hand,
    Cross,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayOp {
    Add,
    Modify,
    Delete,
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    Default,
    Antialiased,
}

/// The edit control's background brush (WM_CTLCOLOREDIT): only its colour
/// matters here.
pub struct Brush(#[allow(dead_code)] pub u32);

impl Brush {
    pub fn new(c: u32) -> Brush {
        Brush(c)
    }
}

// ---------------------------------------------------------------------------
// Messages, timers, input state
// ---------------------------------------------------------------------------

pub fn is_null(h: HWND) -> bool {
    h.is_null()
}

/// The child window of a WM_COMMAND (lparam) and its notification code.
pub fn command_parts(wp: usize, lp: isize) -> (HWND, u16, u16) {
    (HWND(lp as usize), (wp >> 16) as u16, wp as u16)
}

thread_local! {
    static SUGGESTED: RefCell<Option<Rect>> = const { RefCell::new(None) };
}

/// Set the rectangle WM_DPICHANGED suggests, before sending it.
#[cfg(target_os = "macos")]
pub fn set_suggested_rect(r: Rect) {
    SUGGESTED.with(|s| *s.borrow_mut() = Some(r));
}

/// WM_DPICHANGED's suggested rectangle.
pub fn rect_from_lparam(_lp: isize) -> Option<Rect> {
    SUGGESTED.with(|s| s.borrow_mut().take())
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

impl WallTime {
    fn from_epoch_millis(ms: i64) -> WallTime {
        let secs = ms.div_euclid(1000);
        let d = Date::from_days(secs.div_euclid(86_400));
        let s = secs.rem_euclid(86_400) as u32;
        WallTime {
            year: d.year,
            month: d.month,
            day: d.day,
            hour: s / 3600,
            minute: s / 60 % 60,
            second: s % 60,
            millis: ms.rem_euclid(1000) as u32,
        }
    }

    fn epoch_secs(self) -> Option<i64> {
        let d = Date::new(self.year, self.month, self.day)?;
        Some(
            d.to_days() * 86_400
                + i64::from(self.hour) * 3600
                + i64::from(self.minute) * 60
                + i64::from(self.second),
        )
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The machine's zone and its IANA name, if known.
type LocalZone = (Option<String>, Rc<tzif::Zone>);

/// The machine's zone, re-read at most every 10 s (the user may change it).
fn local_zone() -> Option<LocalZone> {
    thread_local! {
        static LOCAL: RefCell<Option<(u64, Option<LocalZone>)>> = const { RefCell::new(None) };
    }
    let now = core::tick_ms();
    LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        if l.as_ref()
            .is_none_or(|(at, _)| now.saturating_sub(*at) > 10_000)
        {
            let zone = tzif::Zone::local().map(|(n, z)| (n, Rc::new(z)));
            *l = Some((now, zone));
        }
        l.as_ref().and_then(|(_, z)| z.clone())
    })
}

pub fn utc_now() -> WallTime {
    WallTime::from_epoch_millis(now_millis())
}

pub fn local_now() -> WallTime {
    let ms = now_millis();
    let off = local_zone().map_or(0, |(_, z)| z.offset_at(ms.div_euclid(1000)));
    WallTime::from_epoch_millis(ms + i64::from(off) * 1000)
}

/// One listed time zone: the table's key and the IANA zone that keeps its
/// clock.
#[derive(Clone)]
pub struct TzInfo {
    key: String,
    zone: Rc<tzif::Zone>,
}

impl TzInfo {
    pub fn key(&self) -> String {
        self.key.clone()
    }
    /// Standard-time offset in minutes.
    pub fn standard_offset(&self) -> i32 {
        self.zone.standard_offset_minutes()
    }
}

/// Every listed zone the system database has.
pub fn enum_time_zones() -> Vec<TzInfo> {
    crate::pack::zone_rows()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            Some(TzInfo {
                zone: Rc::new(tzif::Zone::load(&r.iana)?),
                key: r.key,
            })
        })
        .collect()
}

/// The machine's zone as one of the listed ones: the row naming the same
/// IANA zone, else the first whose clock agrees with it all year.
pub fn current_time_zone() -> Option<TzInfo> {
    let (name, local) = local_zone()?;
    let rows = crate::pack::zone_rows()?;
    if let Some(n) = &name
        && let Some(r) = rows.iter().find(|r| &r.iana == n)
    {
        return Some(TzInfo {
            key: r.key.clone(),
            zone: local,
        });
    }
    let t = now_millis() / 1000;
    rows.into_iter().find_map(|r| {
        let z = tzif::Zone::load(&r.iana)?;
        z.same_clock(&local, t).then(|| TzInfo {
            key: r.key,
            zone: Rc::new(z),
        })
    })
}

/// UTC -> wall clock in `zone`.
pub fn utc_to_zone(zone: &TzInfo, utc: WallTime) -> Option<WallTime> {
    let secs = utc.epoch_secs()?;
    let off = zone.zone.offset_at(secs);
    Some(WallTime::from_epoch_millis(
        (secs + i64::from(off)) * 1000 + i64::from(utc.millis),
    ))
}

/// The app's icon (app.ico), embedded for the windows and the About box.
pub static APP_ICO: &[u8] = include_bytes!("../../../assets/app.ico");

/// The largest frame of the app icon, decoded.
pub fn app_icon_image() -> Option<crate::gfx::Rgba> {
    crate::png::ico_image(APP_ICO, 256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_time_round_trip() {
        let t = WallTime::from_epoch_millis(1_761_350_400_123);
        assert_eq!((t.year, t.month, t.day, t.hour), (2025, 10, 25, 0));
        assert_eq!(t.millis, 123);
        assert_eq!(t.epoch_secs(), Some(1_761_350_400));
        let before = WallTime::from_epoch_millis(-1);
        assert_eq!((before.year, before.hour, before.second), (1969, 23, 59));
    }

    #[test]
    fn kinds() {
        let spec = |style, ex_style| WindowSpec {
            ex_style,
            style,
            title: "",
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        assert_eq!(Kind::of(&spec(WS_POPUP_, WS_EX_TOPMOST_)), Kind::Popup);
        assert_eq!(Kind::of(&spec(WS_POPUP_, WS_EX_NOACTIVATE_)), Kind::Tip);
        assert_eq!(
            Kind::of(&spec(WS_DIALOG, 0)),
            Kind::Dialog { resizable: false }
        );
        assert_eq!(
            Kind::of(&spec(WS_RESIZABLE, 0)),
            Kind::Dialog { resizable: true }
        );
        assert_eq!(command_parts(0x0300_0007, 42), (HWND(42), EN_CHANGE_, 7));
    }

    #[test]
    fn listed_zones_resolve() {
        let zones = enum_time_zones();
        if zones.is_empty() {
            return; // no system database
        }
        assert!(zones.len() > 50);
        let utc = WallTime::from_epoch_millis(1_782_820_800_000); // 2026-06-30 12:00 UTC
        let tokyo = zones
            .iter()
            .find(|z| z.key() == "Tokyo Standard Time")
            .unwrap();
        assert_eq!(utc_to_zone(tokyo, utc).unwrap().hour, 21);
        assert_eq!(tokyo.standard_offset(), 540);
        assert!(app_icon_image().is_some_and(|i| i.w == 256));
    }
}
