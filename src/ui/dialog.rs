//! What every titled window shares (weeknumber_dialog.TitledDialog): a
//! fixed-size captioned window on the monitor under the cursor, sized for
//! that monitor's DPI, dark or light caption, a DPI-exact window icon, one
//! instance at a time, raised to the front when reopened.

use super::{Metrics, rect, rgb};
use crate::app;
use crate::os::HWND;
use crate::os::{self, Icon, MonitorGeom, Rect, Surface};
use crate::settings::Settings;
use crate::theme::{self, Theme};

/// Breathing room left around a monitor-sized window, px @ 96 DPI.
pub const SCREEN_MARGIN_PX: f64 = 60.0;

pub struct DialogBase {
    pub hwnd: HWND,
    pub m: Metrics,
    pub theme: &'static Theme,
    pub title: String,
    pub client: (i32, i32),
    pub mon: MonitorGeom,
    icons: Option<(Icon, Icon)>,
    surface: Option<Surface>,
    pub tracking: bool,
}

impl DialogBase {
    pub fn new(settings: &Settings, family: String, title: &str) -> DialogBase {
        let mon = os::cursor_monitor();
        DialogBase {
            hwnd: HWND::default(),
            m: Metrics::new(mon.dpi, family),
            theme: theme::theme(settings.dark_mode),
            title: format!("{} \u{2013} {title}", app::APP_NAME),
            client: (0, 0),
            mon,
            icons: None,
            surface: None,
            tracking: false,
        }
    }

    pub fn px(&self, v: f64) -> i32 {
        self.m.px(v)
    }

    /// Create the window with its frame's top-left `vertical` of the way
    /// down the work area, centred horizontally by the client size -- as Tk
    /// places "+x+y" from winfo_reqwidth/reqheight.
    pub fn create(&mut self, vertical: f64) -> Option<HWND> {
        let (cw, ch) = self.client;
        let work = self.mon.work;
        let x = work.left + ((work.right - work.left - cw) / 2).max(0);
        let y = work.top + ((f64::from(work.bottom - work.top - ch) * vertical) as i32).max(0);
        self.create_at(x, y, os::WS_DIALOG)
    }

    /// The outer size of a window of `style` around the client size.
    pub fn outer(&self, style: u32) -> (i32, i32) {
        let (cw, ch) = self.client;
        let (_, _, ow, oh) = os::outer_size(style, 0, cw, ch, self.mon.dpi);
        (ow, oh)
    }

    /// Create the window with its frame's top-left at (x, y).
    pub fn create_at(&mut self, x: i32, y: i32, style: u32) -> Option<HWND> {
        let (ow, oh) = self.outer(style);
        let h = os::create_window(&os::WindowSpec {
            ex_style: 0,
            style,
            title: &self.title,
            x,
            y,
            w: ow,
            h: oh,
        })?;
        self.hwnd = h;
        Some(h)
    }

    /// After creation: caption colour and icons, then shown and raised.
    pub fn show(&mut self, dark: bool) {
        os::dark_title_bar(self.hwnd, dark);
        self.sync_icons();
        os::show_window(self.hwnd, true);
        raise(self.hwnd);
    }

    /// Window icons at the exact sizes for the window's monitor DPI.
    pub fn sync_icons(&mut self) {
        let dpi = os::window_dpi(self.hwnd);
        let small = os::metric_for_dpi(os::SM_CXSMICON_, dpi);
        let big = os::metric_for_dpi(os::SM_CXICON_, dpi);
        let small = if (8..=256).contains(&small) {
            small
        } else {
            (16.0 * f64::from(dpi) / 96.0).round() as i32
        };
        let big = if (16..=256).contains(&big) {
            big
        } else {
            (32.0 * f64::from(dpi) / 96.0).round() as i32
        };
        if let (Some(s), Some(b)) = (os::app_icon(small), os::app_icon(big)) {
            os::set_window_icons(self.hwnd, &s, &b);
            self.icons = Some((s, b));
        }
    }

    pub fn retheme(&mut self, settings: &Settings) -> bool {
        let t = theme::theme(settings.dark_mode);
        let changed = !std::ptr::eq(t, self.theme);
        self.theme = t;
        os::dark_title_bar(self.hwnd, settings.dark_mode);
        os::invalidate(self.hwnd);
        changed
    }

    /// A back buffer the size of the client area, cleared to the card colour.
    pub fn begin_paint(&mut self) -> Option<Surface> {
        let (w, h) = self.client;
        let mut s = match self.surface.take() {
            Some(s) if (s.w, s.h) == (w as usize, h as usize) => s,
            _ => Surface::new(w as usize, h as usize)?,
        };
        s.fill_rect(rect(0, 0, w, h), rgb(self.theme.card_bg));
        Some(s)
    }

    pub fn end_paint(&mut self, s: Surface) {
        os::paint(self.hwnd, |dc| s.present(dc));
        self.surface = Some(s);
    }

    /// Resize to a new client size (after a DPI change), keeping the
    /// top-left the system suggests.
    pub fn apply_dpi_rect(&mut self, suggested: Rect) {
        let (cw, ch) = self.client;
        let (_, _, ow, oh) = os::outer_size(os::WS_DIALOG, 0, cw, ch, self.mon.dpi);
        os::set_window_pos(
            self.hwnd,
            suggested.left,
            suggested.top,
            ow,
            oh,
            None,
            false,
            false,
        );
        self.surface = None;
        self.sync_icons();
    }
}

/// Bring a window forward from a tray click without leaving it on top:
/// set the topmost bit, take the foreground, drop the bit again.
pub fn raise(h: HWND) {
    os::restore_window(h);
    os::set_window_pos(h, 0, 0, 0, 0, Some(true), true, true);
    os::set_foreground(h);
    os::set_focus(h);
    os::set_window_pos(h, 0, 0, 0, 0, Some(false), true, true);
}

/// WM_DPICHANGED: the new DPI and the suggested window rectangle.
pub fn dpi_changed(wp: usize, lp: isize) -> (u32, Option<Rect>) {
    (u32::from(wp as u16), os::rect_from_lparam(lp))
}
