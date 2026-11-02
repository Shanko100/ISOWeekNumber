//! The app's pop-up menus -- the tray icon's right-click menu and the
//! calendar's copy menu -- drawn in the calendar flyout's style (the plain
//! system menu stays light in Dark Mode): the same card colour, 1 px card
//! border and rounded corners, rows that light up with the calendar's hover
//! colour in a rounded highlight, and an accent check mark.
//!
//! It behaves like the system menu: the tray menu opens with its
//! bottom-right corner at the pointer, the copy menu with its top-left
//! corner there; it closes when it loses the focus or on Escape, follows
//! the pointer and the Up/Down keys, and runs an item on a left or right
//! click or on Enter or Space. On macOS the native menu is used instead.
#![cfg_attr(target_os = "macos", allow(dead_code, unused_imports))]

use super::{Metrics, contains, rect, rgb, rounded};
use crate::app::{self, later, with_app};
use crate::gfx::{Filter, Gray, Rgba};
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Rect, Surface};
use crate::pil;
use crate::settings::Settings;
use crate::theme::{self, Theme};

/// Metrics in px @ 96 DPI: compact rows like the Windows 11 context menu.
const FONT_PX: f64 = 12.0;
const PAD_Y_PX: f64 = 4.0;
const ROW_H_PX: f64 = 26.0;
const SEP_H_PX: f64 = 7.0;
const CHECK_COL_PX: f64 = 30.0;
const CHECK_PX: f64 = 11.0;
const TEXT_RIGHT_PX: f64 = 20.0;
const MIN_W_PX: f64 = 180.0;
const HOVER_INSET_PX: f64 = 4.0;
const HOVER_RADIUS_PX: f64 = 4.0;
const SEP_INSET_PX: f64 = 10.0;

const VK_SPACE: u16 = 0x20;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;

pub enum Row {
    Item {
        id: u32,
        label: String,
        checked: bool,
    },
    Separator,
}

/// What a chosen item does.
pub enum Action {
    /// Run the tray command with the item's id.
    Tray,
    /// Copy `copies[id - 1]` to the clipboard, then give `owner` (the
    /// calendar) the focus back.
    Copy { owner: HWND, copies: Vec<String> },
}

impl Action {
    fn anchored_at_corner(&self) -> bool {
        matches!(self, Action::Tray)
    }
}

pub struct Menu {
    pub hwnd: HWND,
    action: Option<Action>,
    m: Metrics,
    theme: &'static Theme,
    rows: Vec<Row>,
    rects: Vec<Rect>,
    size: (i32, i32),
    pos: (i32, i32),
    hover: Option<usize>,
    tracking: bool,
    check: Option<Rgba>,
    highlight: Option<Rgba>,
    chosen: bool,
}

impl Menu {
    pub fn build(settings: &Settings, family: String, rows: Vec<Row>, action: Action) -> Box<Menu> {
        let at = os::cursor_pos();
        let mon = os::cursor_monitor();
        let mut m = Metrics::new(mon.dpi, family);
        let font_w = rows
            .iter()
            .map(|r| match r {
                Row::Item { label, .. } => m.font(FONT_PX, false).measure(label),
                Row::Separator => 0,
            })
            .max()
            .unwrap_or(0);
        let w = (1 + m.px(CHECK_COL_PX) + font_w + m.px(TEXT_RIGHT_PX) + 1).max(m.px(MIN_W_PX));
        let mut y = 1 + m.px(PAD_Y_PX);
        let mut rects = Vec::with_capacity(rows.len());
        for r in &rows {
            let h = match r {
                Row::Item { .. } => m.px(ROW_H_PX),
                Row::Separator => m.px(SEP_H_PX),
            };
            rects.push(rect(1, y, w - 2, h));
            y += h;
        }
        let h = y + m.px(PAD_Y_PX) + 1;
        // The tray menu's bottom-right corner at the pointer, like the
        // system tray menu, the copy menu's top-left corner; flipped and
        // clamped to stay inside the work area.
        let work = mon.work;
        let (mut x, mut top) = if action.anchored_at_corner() {
            (at.0 - w, at.1 - h)
        } else {
            (at.0, at.1)
        };
        if x < work.left {
            x = at.0;
        } else if x + w > work.right {
            x = at.0 - w;
        }
        if top < work.top {
            top = at.1;
        } else if top + h > work.bottom {
            top = at.1 - h;
        }
        let x = x.clamp(work.left, (work.right - w).max(work.left));
        let top = top.clamp(work.top, (work.bottom - h).max(work.top));
        Box::new(Menu {
            hwnd: HWND::default(),
            action: Some(action),
            m,
            theme: theme::theme(settings.dark_mode),
            rows,
            rects,
            size: (w, h),
            pos: (x, top),
            hover: None,
            tracking: false,
            check: None,
            highlight: None,
            chosen: false,
        })
    }

    pub fn create_window(&mut self) -> Option<HWND> {
        let hwnd = os::create_window(&os::WindowSpec {
            ex_style: os::WS_EX_TOPMOST_ | os::WS_EX_TOOLWINDOW_,
            style: os::WS_POPUP_,
            title: app::APP_NAME,
            x: self.pos.0,
            y: self.pos.1,
            w: self.size.0,
            h: self.size.1,
        })?;
        self.hwnd = hwnd;
        os::round_corners(hwnd, false);
        Some(hwnd)
    }

    pub fn show(hwnd: HWND) {
        os::show_window(hwnd, true);
        os::set_foreground(hwnd);
        os::set_focus(hwnd);
    }

    /// Close the window; a copy menu closed without a choice hands the
    /// focus back to the calendar.
    pub fn destroy(mut self: Box<Self>) {
        os::destroy_window(self.hwnd);
        if let Some(Action::Copy { owner, .. }) = self.action.take() {
            later(move || app::copy_menu_closed(owner, None));
        }
    }

    fn item(&self, i: usize) -> Option<u32> {
        match self.rows.get(i)? {
            Row::Item { id, .. } => Some(*id),
            Row::Separator => None,
        }
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.rows.len()).find(|&i| self.item(i).is_some() && contains(self.rects[i], x, y))
    }

    fn set_hover(&mut self, hover: Option<usize>) {
        if hover != self.hover {
            self.hover = hover;
            os::invalidate(self.hwnd);
        }
    }

    /// Up/Down: the next item in `dir`, wrapping round, skipping separators.
    fn step(&mut self, dir: i32) {
        let n = self.rows.len() as i32;
        let mut i = self
            .hover
            .map_or(if dir > 0 { -1 } else { n }, |h| h as i32);
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.item(i as usize).is_some() {
                self.set_hover(Some(i as usize));
                return;
            }
        }
    }

    fn choose(&mut self, i: usize) {
        let Some(id) = self.item(i) else { return };
        if self.chosen {
            return;
        }
        self.chosen = true;
        let action = self.action.take();
        later(move || {
            app::close_menu();
            match action {
                Some(Action::Tray) => app::run_tray_command(id),
                Some(Action::Copy { owner, copies }) => {
                    let text = copies.get(id as usize - 1).cloned();
                    app::copy_menu_closed(owner, text);
                }
                None => {}
            }
        });
    }

    /// The check mark: two strokes with round ends in the accent colour,
    /// drawn 4x and resampled down like the calendar's artwork.
    fn check_image(&mut self) -> Rgba {
        if let Some(c) = &self.check {
            return c.clone();
        }
        let side = self.m.px(CHECK_PX);
        let ss = 4.0;
        let big = side as usize * 4;
        let s = f64::from(side) * ss;
        let stroke = (f64::from(self.m.px(1.6)) * ss).round();
        let pts = [
            (0.12 * s, 0.55 * s),
            (0.40 * s, 0.82 * s),
            (0.88 * s, 0.22 * s),
        ];
        let mut mask = Gray::new(big, big, 0);
        mask.fill(&pil::Line(pts.to_vec(), stroke as i32), 255);
        for (x, y) in pts {
            let r = stroke / 2.0;
            mask.fill(&pil::Ellipse(x - r, y - r, x + r, y + r), 255);
        }
        let small = mask.resize(side as usize, side as usize, Filter::Lanczos);
        let img = Rgba::layer(&small, self.theme.accent);
        self.check = Some(img.clone());
        img
    }

    fn paint(&mut self) {
        let (w, h) = self.size;
        let Some(mut s) = Surface::new(w as usize, h as usize) else {
            return;
        };
        let t = self.theme;
        s.fill_rect(rect(0, 0, w, h), rgb(t.card_border));
        s.fill_rect(rect(1, 1, w - 2, h - 2), rgb(t.card_bg));
        let inset = self.m.px(HOVER_INSET_PX);
        let check = self.check_image();
        for i in 0..self.rows.len() {
            let r = self.rects[i];
            match &self.rows[i] {
                Row::Separator => {
                    let sx = self.m.px(SEP_INSET_PX);
                    let sy = r.top + (r.bottom - r.top) / 2;
                    s.fill_rect(
                        Rect {
                            left: sx,
                            top: sy,
                            right: w - sx,
                            bottom: sy + self.m.px(1.0),
                        },
                        rgb(t.separator_bg),
                    );
                }
                Row::Item { label, checked, .. } => {
                    if self.hover == Some(i) {
                        let (hw, hh) = (r.right - r.left - 2 * inset, r.bottom - r.top - 2);
                        let radius = f64::from(self.m.px(HOVER_RADIUS_PX));
                        let img = match &self.highlight {
                            Some(img) if (img.w, img.h) == (hw as usize, hh as usize) => {
                                img.clone()
                            }
                            _ => {
                                let img = rounded(hw, hh, radius, t.hover_bg, t.card_bg, None, 0);
                                self.highlight = Some(img.clone());
                                img
                            }
                        };
                        s.blit(&img, r.left + inset, r.top + 1);
                    }
                    if *checked {
                        let img = &check;
                        let col = self.m.px(CHECK_COL_PX);
                        s.blend(
                            img,
                            r.left + (col - img.w as i32) / 2,
                            r.top + (r.bottom - r.top - img.h as i32) / 2,
                        );
                    }
                    let x = r.left + self.m.px(CHECK_COL_PX);
                    let font = self.m.font(FONT_PX, false);
                    let ls = font.metrics().linespace();
                    let y = r.top + (r.bottom - r.top - ls) / 2;
                    s.dc().text(font, x, y, label, t.title_fg.colorref());
                }
            }
        }
        os::paint(self.hwnd, |dc| s.present(dc));
    }

    pub fn on_message(&mut self, msg: u32, wp: usize, lp: isize) -> Option<isize> {
        match msg {
            wm::WM_PAINT => {
                self.paint();
                Some(0)
            }
            wm::WM_ERASEBKGND => Some(1),
            wm::WM_ACTIVATE => {
                if (wp & 0xFFFF) as u32 == wm::WA_INACTIVE {
                    later(app::close_menu);
                }
                None
            }
            wm::WM_MOUSEMOVE => {
                if !self.tracking {
                    os::track_leave(self.hwnd);
                    self.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                let hover = self.row_at(x, y);
                self.set_hover(hover);
                Some(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                self.tracking = false;
                self.set_hover(None);
                Some(0)
            }
            wm::WM_LBUTTONUP | wm::WM_RBUTTONUP => {
                let (x, y) = super::mouse_xy(lp);
                if let Some(i) = self.row_at(x, y) {
                    self.choose(i);
                }
                Some(0)
            }
            wm::WM_KEYDOWN => {
                match wp as u16 {
                    os::VK_ESCAPE_ => later(app::close_menu),
                    VK_UP => self.step(-1),
                    VK_DOWN => self.step(1),
                    os::VK_RETURN_ | VK_SPACE => {
                        if let Some(i) = self.hover {
                            self.choose(i);
                        }
                    }
                    _ => {}
                }
                Some(0)
            }
            _ => None,
        }
    }
}

/// Open a menu at the pointer (closing one already open).
pub fn open(rows: Vec<Row>, action: Action) {
    app::close_menu();
    // macOS: the system's own menu, modal, at the pointer.
    #[cfg(target_os = "macos")]
    {
        let items: Vec<(u32, String)> = rows
            .iter()
            .map(|r| match r {
                Row::Item { id, label, .. } => (*id, label.clone()),
                Row::Separator => (0, String::new()),
            })
            .collect();
        match action {
            Action::Copy { owner, copies } => {
                let chosen = os::popup_menu(owner, &items);
                let text = chosen.and_then(|id| copies.get(id as usize - 1).cloned());
                app::copy_menu_closed(owner, text);
            }
            // The menu bar item's own menu, as a right-click opens it.
            Action::Tray => os::show_status_menu(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    open_drawn(rows, action);
}

#[cfg(not(target_os = "macos"))]
fn open_drawn(rows: Vec<Row>, action: Action) {
    let built = with_app(|a| {
        let family = a.family();
        Menu::build(&a.settings, family, rows, action)
    });
    let Some(mut menu) = built else { return };
    let Some(hwnd) = menu.create_window() else {
        return;
    };
    with_app(|a| a.menu = Some(menu));
    Menu::show(hwnd);
}
