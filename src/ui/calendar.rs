//! The two-month calendar flyout: borderless, anchored to the taskbar edge of
//! the monitor under the cursor, scaled to that monitor's DPI, closed when it
//! loses the focus. Layout and artwork follow weeknumber_calendar.py: the Tk
//! grid it builds, and the Pillow pills, notches, stripes and chevrons.

use super::{Metrics, Pill, Variant, contains, menu, rect, rgb, tk_label};
use crate::app::{self, later, with_app};
use crate::dates::{self, Date, MONTH_NAMES};
use crate::gfx::{self, Color, Filter, Gray, Rgba};
use crate::holidays;
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Rect, Surface, TzInfo};
use crate::pil;
use crate::settings::Settings;
use crate::theme::{self, Theme};
use crate::tz;
use std::collections::HashMap;

const MONTHS_SHOWN: usize = 2;
const ROWS: usize = 6;
const CELL_PX: f64 = 25.0;
const TITLE_WEEKDAY_GAP_PX: f64 = 6.0;
const TOOLTIP_DELAY_MS: u32 = 450;
const CARD_PAD_PX: f64 = 10.0;

const TIMER_TIP: usize = 10;
const TIMER_CLOCK: usize = 11;

fn min_month() -> Date {
    Date {
        year: 2000,
        month: 1,
        day: 1,
    }
}
/// The first pane stops at November so the second pane is December 2050.
fn max_month() -> Date {
    Date {
        year: 2050,
        month: 11,
        day: 1,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    Week,
    Day,
}

struct Cell {
    rect: Rect,
    kind: CellKind,
    text: String,
    band: Color,
    rest: String,
    hover: String,
    fg: Color,
    hoverable: bool,
    date: Option<Date>,
    iso: Option<(i32, u32)>,
    tip: Option<String>,
}

struct Pane {
    month_rect: Rect,
    year_rect: Rect,
    month_text: String,
    year_text: String,
    prev: Option<Rect>,
    next: Option<Rect>,
    headers: Vec<(Rect, &'static str, bool)>,
    separator: Option<Rect>,
}

pub struct Calendar {
    pub hwnd: HWND,
    m: Metrics,
    theme: &'static Theme,
    display_month: Date,
    selected: Option<Date>,
    cells: Vec<Cell>,
    panes: Vec<Pane>,
    size: (i32, i32),
    footer_rule: Rect,
    footer_row: Rect,
    today_pill: Pill,
    tz_pill: Pill,
    tz_text: String,
    art: Option<Art>,
    /// macOS: the artwork over the black and the white card (paint_glass).
    #[cfg(target_os = "macos")]
    glass_art: Option<(Art, Art)>,
    store: holidays::Store,
    zones: Option<Vec<TzInfo>>,
    surface: Option<Surface>,
    hover: Option<usize>,
    hover_nav: Option<i32>,
    pressed_pill: Option<bool>,
    tip_pending: Option<usize>,
    tooltip: Option<Tooltip>,
    /// The copy menu is open (losing the focus to it keeps the flyout).
    menu: bool,
    work: Rect,
    monitor: Rect,
    cursor: (i32, i32),
    tracking: bool,
}

/// The rounded state pills and notches for one palette and scale.
struct Art {
    d: usize,
    parts: HashMap<&'static str, Rgba>,
    composed: HashMap<String, Rgba>,
}

impl Art {
    fn new(s: f64, t: &Theme) -> Art {
        let px = |v: f64| super::px(s, v) as usize;
        let d = px(CELL_PX);
        let r = px(5.0);
        let ss = 4usize;
        let n = d * ss;
        let rad = (r * ss) as f64;
        let nf = n as f64 - 1.0;
        let mask = |side: &str| {
            let mut big = Gray::new(n, n, 0);
            big.fill(&pil::RoundedRectangle(0.0, 0.0, nf, nf, rad), 255);
            match side {
                "left" => big.fill(&pil::Rectangle(n as f64 - rad - 1.0, 0.0, nf, nf), 255),
                "right" => big.fill(&pil::Rectangle(0.0, 0.0, rad, nf), 255),
                _ => {}
            }
            big.resize(d, d, Filter::Lanczos)
        };
        let ring_mask = |width_px: f64| {
            let width = (gfx::py_round(width_px * s).max(1.0) * ss as f64).max(1.0);
            let mut big = Gray::new(n, n, 0);
            big.fill(
                &pil::RoundedRectangleOutline(0.0, 0.0, nf, nf, rad, width as i32),
                255,
            );
            big.resize(d, d, Filter::Lanczos)
        };
        let full = mask("full");
        let left = mask("left");
        let right = mask("right");
        let ring = ring_mask(1.25);
        let holiday_ring = ring_mask(2.0);
        let layer = |m: &Gray, c: Color| Rgba::layer(m, c);
        let mut parts = HashMap::new();
        parts.insert("notch_left", layer(&left.point(|p| 255 - p), t.card_bg));
        parts.insert("notch_right", layer(&right.point(|p| 255 - p), t.card_bg));
        parts.insert("today", layer(&full, t.accent));
        parts.insert("selected", layer(&full, t.selected_day_fill));
        parts.insert("sel_ring", layer(&ring, t.accent));
        parts.insert("holiday", layer(&full, t.holiday_bg));
        parts.insert("holiday_secondary", layer(&full, t.holiday_secondary_bg));
        // Diagonal stripes at the supersampled size, clipped once.
        let mut big = Rgba::new(n, n, t.holiday_bg.rgba(255));
        let stripe = (gfx::py_round(4.0 * s * ss as f64)).max(ss as f64);
        let mut x = -(n as f64);
        while x < 2.0 * n as f64 {
            big.fill(
                &pil::Line(vec![(x, 0.0), (x - n as f64, n as f64)], stripe as i32),
                t.holiday_secondary_bg.rgba(255),
            );
            x += stripe * 2.0;
        }
        let mut clip = Gray::new(n, n, 0);
        clip.fill(&pil::RoundedRectangle(0.0, 0.0, nf, nf, rad), 255);
        big.putalpha(&clip);
        parts.insert("holiday_dual", big.resize(d, d, Filter::Lanczos));
        parts.insert(
            "holiday_primary_ring",
            layer(&holiday_ring, t.holiday_border),
        );
        parts.insert(
            "holiday_secondary_ring",
            layer(&holiday_ring, t.holiday_secondary_border),
        );
        let mut out = Rgba::new(d, d, t.holiday_border.rgba(255));
        let stripe = gfx::py_round(3.0 * s).max(1.0);
        let mut x = -(d as f64);
        while x < 2.0 * d as f64 {
            out.fill(
                &pil::Line(vec![(x, 0.0), (x - d as f64, d as f64)], stripe as i32),
                t.holiday_secondary_border.rgba(255),
            );
            x += stripe * 2.0;
        }
        out.putalpha(&holiday_ring);
        parts.insert("holiday_dual_ring", out);
        parts.insert("hover", layer(&full, t.hover_bg));
        let mut circle = Gray::new(n, n, 0);
        circle.fill(&pil::Ellipse(0.0, 0.0, nf, nf), 255);
        parts.insert(
            "nav_hover",
            layer(&circle.resize(d, d, Filter::Lanczos), t.hover_bg),
        );
        for (name, dir) in [("chev_prev", -1), ("chev_next", 1)] {
            let c = (n as f64 - 1.0) / 2.0;
            let (half_h, half_w) = (0.16 * n as f64, 0.09 * n as f64);
            let stroke = (1.5 * s).max(1.0) * ss as f64;
            let cap = stroke / 2.0;
            let back = (c - half_w, c - half_h);
            let tip = (c + half_w, c);
            let mut arm = Gray::new(n, n, 0);
            arm.fill(
                &pil::Line(vec![back, tip], gfx::py_round(stroke) as i32),
                255,
            );
            for (ex, ey) in [back, tip] {
                arm.fill(&pil::Ellipse(ex - cap, ey - cap, ex + cap, ey + cap), 255);
            }
            let mut glyph = arm.lighter(&arm.flip());
            if dir < 0 {
                glyph = glyph.mirror();
            }
            parts.insert(name, layer(&glyph.resize(d, d, Filter::Lanczos), t.accent));
        }
        Art {
            d,
            parts,
            composed: HashMap::new(),
        }
    }

    /// The image for a state name of weeknumber_calendar._build_cell_images.
    fn image(&mut self, key: &str) -> Option<&Rgba> {
        if key == "blank" {
            return None;
        }
        if !self.composed.contains_key(key) {
            let (base, right) = match key.strip_suffix("_R") {
                Some(b) => (b, true),
                None => (key, false),
            };
            let mut layers: Vec<&str> = Vec::new();
            if right && base != "notch" {
                layers.push("notch_right");
            }
            match base {
                "notch_L" => layers.push("notch_left"),
                "notch" => layers.push("notch_right"),
                "today" => layers.push("today"),
                "selected" => layers.extend(["selected", "sel_ring"]),
                "hover" => layers.push("hover"),
                "holiday_primary" => layers.push("holiday"),
                "holiday_secondary" => layers.push("holiday_secondary"),
                "holiday_dual" => layers.push("holiday_dual"),
                "nav_prev" => layers.push("chev_prev"),
                "nav_prev_hover" => layers.extend(["nav_hover", "chev_prev"]),
                "nav_next" => layers.push("chev_next"),
                "nav_next_hover" => layers.extend(["nav_hover", "chev_next"]),
                other => {
                    // today_holiday_* / selected_holiday_*
                    let (state, kind) = other.split_once("_holiday_")?;
                    layers.push(state);
                    layers.push(match kind {
                        "primary" => "holiday_primary_ring",
                        "secondary" => "holiday_secondary_ring",
                        _ => "holiday_dual_ring",
                    });
                }
            }
            let mut out = Rgba::new(self.d, self.d, [0, 0, 0, 0]);
            for l in layers {
                out.alpha_composite(self.parts.get(l)?);
            }
            self.composed.insert(key.to_string(), out);
        }
        self.composed.get(key)
    }
}

/// weeknumber_calendar._pick: the image key for a cell in `state` on a row
/// with `band`, at side L (week), M (Mon-Sat) or R (Sunday).
fn pick(state: &str, band_is_card: bool, side: char) -> String {
    if band_is_card {
        return state.to_string();
    }
    match side {
        'L' => {
            if state == "blank" {
                "notch_L".into()
            } else {
                state.into()
            }
        }
        'R' => {
            if state == "blank" {
                "notch_R".into()
            } else {
                format!("{state}_R")
            }
        }
        _ => state.into(),
    }
}

struct Tooltip {
    hwnd: HWND,
    lines: Vec<String>,
    size: (i32, i32),
}

impl Calendar {
    pub fn build(settings: &Settings, family: String) -> Option<Box<Calendar>> {
        let mon = os::cursor_monitor();
        let mut m = Metrics::new(mon.dpi, family);
        let today = app::today();
        let display_month = today.first_of_month().clamp(min_month(), max_month());
        let today_pill = Pill::new(Variant::Chip, " ", 11.0, true, 0.0, &m);
        let tz_pill = Pill::new(Variant::Chip, " ", 11.0, true, 0.0, &m);
        let _ = m.font(11.0, false);
        let mut cal = Box::new(Calendar {
            hwnd: HWND::default(),
            m,
            theme: theme::theme(settings.dark_mode),
            display_month,
            selected: None,
            cells: Vec::new(),
            panes: Vec::new(),
            size: (0, 0),
            footer_rule: Rect::default(),
            footer_row: Rect::default(),
            today_pill,
            tz_pill,
            tz_text: String::new(),
            art: None,
            #[cfg(target_os = "macos")]
            glass_art: None,
            store: holidays::Store::default(),
            zones: None,
            surface: None,
            hover: None,
            hover_nav: None,
            pressed_pill: None,
            tip_pending: None,
            tooltip: None,
            menu: false,
            work: mon.work,
            monitor: mon.monitor,
            cursor: os::tray_anchor().unwrap_or_else(os::cursor_pos),
            tracking: false,
        });
        cal.art = Some(Art::new(cal.m.scale, cal.theme));
        cal.update_clock(settings);
        cal.layout_and_fill(settings);
        Some(cal)
    }

    pub fn create_window(&mut self) -> Option<HWND> {
        let (w, h) = self.size;
        let (x, y) = self.place();
        let hwnd = os::create_window(&os::WindowSpec {
            ex_style: os::WS_EX_TOPMOST_ | os::WS_EX_TOOLWINDOW_,
            style: os::WS_POPUP_,
            title: app::APP_NAME,
            x,
            y,
            w,
            h,
        })?;
        self.hwnd = hwnd;
        os::round_corners(hwnd, false);
        // The glass behind the macOS flyout takes the light or dark tint,
        // and corners concentric with the footer chips: their radius plus
        // their inset from the edge (the 1px stroke slot and the padding),
        // as in the Python version.
        #[cfg(target_os = "macos")]
        {
            os::dark_title_bar(hwnd, std::ptr::eq(self.theme, &theme::DARK));
            let (_, chip_h) = self.today_pill.size(&mut self.m);
            let radius = f64::from(chip_h) / 2.0 + 1.0 + f64::from(self.px(CARD_PAD_PX));
            os::set_glass_radius(hwnd, radius);
        }
        Some(hwnd)
    }

    pub fn show(hwnd: HWND) {
        os::show_window(hwnd, true);
        os::set_foreground(hwnd);
        os::set_focus(hwnd);
        with_app(|a| {
            if let Some(c) = a.calendar.as_mut() {
                c.arm_clock();
            }
        });
    }

    pub fn destroy(self: Box<Self>) {
        if let Some(t) = &self.tooltip {
            os::destroy_window(t.hwnd);
        }
        os::destroy_window(self.hwnd);
    }

    fn px(&self, v: f64) -> i32 {
        self.m.px(v)
    }

    // -- placement ------------------------------------------------------------
    fn place(&self) -> (i32, i32) {
        let (w, h) = self.size;
        let margin = self.px(8.0);
        let (work, mon) = (self.work, self.monitor);
        let gaps = [
            ("left", work.left - mon.left),
            ("top", work.top - mon.top),
            ("right", mon.right - work.right),
            ("bottom", mon.bottom - work.bottom),
        ];
        // macOS: under the menu bar, where the status item is, wherever the
        // Dock sits.
        let edge = if cfg!(target_os = "macos") {
            "top"
        } else {
            gaps.iter()
                .max_by_key(|g| g.1)
                .filter(|g| g.1 > 0)
                .map_or("bottom", |g| g.0)
        };
        let (cx, cy) = self.cursor;
        let (mut x, mut y) = match edge {
            "bottom" => (cx - w / 2, work.bottom - h - margin),
            "top" => (cx - w / 2, work.top + margin),
            "left" => (work.left + margin, cy - h / 2),
            _ => (work.right - w - margin, cy - h / 2),
        };
        x = x.min(work.right - w - margin).max(work.left + margin);
        y = y.min(work.bottom - h - margin).max(work.top + margin);
        (x, y)
    }

    // -- layout and data --------------------------------------------------------
    /// The Tk geometry of weeknumber_calendar._build_skeleton, and the data
    /// of _refresh.
    fn layout_and_fill(&mut self, settings: &Settings) {
        let d = self.px(CELL_PX);
        let pad = self.px(CARD_PAD_PX);
        let gap = self.px(TITLE_WEEKDAY_GAP_PX);
        let (p6, p4, p8, p10) = (self.px(6.0), self.px(4.0), self.px(8.0), self.px(10.0));
        let bd = self.m.label_bd;
        let l11 = self.m.font(11.0, false).metrics().linespace();
        let month_ls = self.m.font(14.0, true).metrics().linespace();
        let year_ls = self.m.font(14.0, false).metrics().linespace();
        let title_text_h = (month_ls + 2 + 2 * bd).max(year_ls + 2 + 2 * bd);
        let title_row_h = d.max(title_text_h);
        let pane_w = 8 * d;
        let pane_h = title_row_h + gap + l11 + ROWS as i32 * d;
        let wrapper_w = MONTHS_SHOWN as i32 * (p6 + pane_w + p6) + (MONTHS_SHOWN as i32 - 1);

        // Footer: Today chip, plus the clock chip when shown; uniform columns.
        let today = app::today();
        self.today_pill.set_text(&format!(
            "Today \u{b7} {:02}-{:02}-{}",
            today.day, today.month, today.year
        ));
        let (tw, chip_h) = self.today_pill.size(&mut self.m);
        let show_tz = !self.tz_text.is_empty();
        self.tz_pill
            .set_text(if show_tz { &self.tz_text } else { " " });
        let (zw, _) = self.tz_pill.size(&mut self.m);
        let footer_req = if show_tz { 2 * tw.max(zw) } else { tw };
        let inner_w = wrapper_w.max(footer_req);
        let card_w = inner_w + 2 * pad;
        let client_w = card_w + 2;
        let client_h = 2 + pad + pane_h + p8 + 1 + p4 + chip_h + pad;
        self.size = (client_w, client_h);

        let inner_x = 1 + pad;
        let inner_y = 1 + pad;
        let wrapper_x = inner_x + (inner_w - wrapper_w) / 2;
        self.footer_rule = rect(inner_x, inner_y + pane_h + p8, inner_w, 1);
        self.footer_row = rect(inner_x, self.footer_rule.bottom + p4, inner_w, chip_h);
        let col0 = inner_w / 2;
        if show_tz {
            self.today_pill.rect = rect(inner_x + (col0 - tw) / 2, self.footer_row.top, tw, chip_h);
            self.tz_pill.rect = rect(
                inner_x + col0 + (inner_w - col0 - zw) / 2,
                self.footer_row.top,
                zw,
                chip_h,
            );
        } else {
            self.today_pill.rect = rect(
                inner_x + (inner_w - tw) / 2,
                self.footer_row.top,
                tw,
                chip_h,
            );
            self.tz_pill.rect = Rect::default();
        }

        // Data (weeknumber_calendar._refresh).
        let t = self.theme;
        let sel = self.selected;
        let today_iso = today.iso_week();
        let sel_iso = sel.map(Date::iso_week);
        let primary = settings.holiday_primary.clone();
        let secondary = settings.holiday_secondary.clone();
        let years: Vec<i32> = (0..MONTHS_SHOWN as i32)
            .map(|k| self.display_month.shift_months(k).year)
            .collect();
        self.store
            .hold(&[primary.as_deref(), secondary.as_deref()], &years);

        self.cells.clear();
        self.panes.clear();
        for mi in 0..MONTHS_SHOWN {
            let first = self.display_month.shift_months(mi as i32);
            let (year, month) = (first.year, first.month);
            let last = first.shift_months(1).add_days(-1);
            let pane_x = wrapper_x + mi as i32 * (p6 + pane_w + p6 + 1) + p6;
            let pane_y = inner_y;

            let month_text = MONTH_NAMES[month as usize - 1].to_string();
            let year_text = year.to_string();
            let (mw, mh) = super::label_size(&mut self.m, 14.0, true, &month_text, 1, 1, bd);
            let (yw, yh) = super::label_size(&mut self.m, 14.0, false, &year_text, 1, 1, bd);
            let title_w = mw + p4 + yw;
            let tx = pane_x + d + (pane_w - 2 * d - title_w) / 2;
            let ty = pane_y + (title_row_h - title_text_h) / 2;
            let month_rect = rect(tx, ty + (title_text_h - mh) / 2, mw, mh);
            let year_rect = rect(tx + mw + p4, ty + (title_text_h - yh) / 2, yw, yh);
            let prev = (mi == 0).then(|| rect(pane_x, pane_y, d, title_row_h));
            let next =
                (mi == MONTHS_SHOWN - 1).then(|| rect(pane_x + pane_w - d, pane_y, d, title_row_h));
            let header_y = pane_y + title_row_h + gap;
            let headers = ["CW", "Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
                .iter()
                .enumerate()
                .map(|(c, name)| (rect(pane_x + c as i32 * d, header_y, d, l11), *name, c == 0))
                .collect();
            let separator = (mi + 1 < MONTHS_SHOWN)
                .then(|| rect(pane_x + pane_w + p6, pane_y + p10, 1, pane_h - 2 * p10));
            self.panes.push(Pane {
                month_rect,
                year_rect,
                month_text,
                year_text,
                prev,
                next,
                headers,
                separator,
            });

            let pane_today = (year, month) == (today.year, today.month);
            let pane_sel = sel.is_some_and(|s| (s.year, s.month) == (year, month));
            let grid_start = first.add_days(-i64::from(first.weekday()));
            let rows_y = header_y + l11;
            let mut bands = Vec::new();
            for r in 0..ROWS {
                let row_start = grid_start.add_days(7 * r as i64);
                let row_end = row_start.add_days(6);
                let has_day = row_start <= last && row_end >= first;
                let row_iso = has_day.then(|| row_start.iso_week());
                let band = if row_iso.is_some() && pane_today && row_iso == Some(today_iso) {
                    t.current_week_bg
                } else if row_iso.is_some() && pane_sel && row_iso == sel_iso {
                    t.selected_week_bg
                } else {
                    t.card_bg
                };
                bands.push(band);
                let is_card = band == t.card_bg;
                let y = rows_y + r as i32 * d;
                self.cells.push(Cell {
                    rect: rect(pane_x, y, d, d),
                    kind: CellKind::Week,
                    text: row_iso.map(|(_, w)| w.to_string()).unwrap_or_default(),
                    band,
                    rest: pick("blank", is_card, 'L'),
                    hover: pick("blank", is_card, 'L'),
                    fg: t.week_num_fg,
                    hoverable: false,
                    date: None,
                    iso: row_iso,
                    tip: None,
                });
                for c in 0..7 {
                    let side = if c == 6 { 'R' } else { 'M' };
                    self.cells.push(Cell {
                        rect: rect(pane_x + (c + 1) * d, y, d, d),
                        kind: CellKind::Day,
                        text: String::new(),
                        band,
                        rest: pick("blank", is_card, side),
                        hover: pick("hover", is_card, side),
                        fg: t.title_fg,
                        hoverable: false,
                        date: None,
                        iso: None,
                        tip: None,
                    });
                }
            }
            let pane_base = self.cells.len() - ROWS * 8;
            let (mut row, mut col) = (0usize, first.weekday() as usize);
            for day in 1..=dates::days_in_month(year, month) {
                let date = Date { year, month, day };
                let band = bands[row];
                let is_card = band == t.card_bg;
                let is_today = date == today;
                let is_selected = sel == Some(date) && !is_today;
                let p_names = self.store.names(primary.as_deref(), date);
                let s_names = self.store.names(secondary.as_deref(), date);
                let mut lines = Vec::new();
                if !p_names.is_empty() {
                    let c = primary.as_deref().unwrap_or("");
                    lines.push(format!(
                        "{}: {}",
                        self.store.country_name(c).unwrap_or(c),
                        p_names.join("; ")
                    ));
                }
                if !s_names.is_empty() {
                    let c = secondary.as_deref().unwrap_or("");
                    lines.push(format!(
                        "{}: {}",
                        self.store.country_name(c).unwrap_or(c),
                        s_names.join("; ")
                    ));
                }
                let kind = match (!p_names.is_empty(), !s_names.is_empty()) {
                    (true, true) => Some("dual"),
                    (true, false) => Some("primary"),
                    (false, true) => Some("secondary"),
                    _ => None,
                };
                let side = if col == 6 { 'R' } else { 'M' };
                let (rest, fg, hoverable) = if is_today {
                    (
                        kind.map_or("today".to_string(), |k| format!("today_holiday_{k}")),
                        t.today_fg,
                        false,
                    )
                } else if is_selected {
                    (
                        kind.map_or("selected".to_string(), |k| format!("selected_holiday_{k}")),
                        t.title_fg,
                        kind.is_none(),
                    )
                } else if let Some(k) = kind {
                    (format!("holiday_{k}"), t.holiday_fg, false)
                } else {
                    ("blank".to_string(), t.title_fg, true)
                };
                let cell = &mut self.cells[pane_base + row * 8 + 1 + col];
                cell.text = day.to_string();
                cell.rest = pick(&rest, is_card, side);
                cell.hover = pick("hover", is_card, side);
                cell.fg = fg;
                cell.hoverable = hoverable;
                cell.date = Some(date);
                cell.iso = Some(date.iso_week());
                cell.tip = (!lines.is_empty()).then(|| lines.join("\n"));
                col += 1;
                if col == 7 {
                    col = 0;
                    row += 1;
                }
            }
        }
    }

    /// The time-zone chip text (empty: chip hidden).
    fn update_clock(&mut self, settings: &Settings) {
        let mut text = String::new();
        if settings.timezone_enabled {
            if self.zones.is_none() {
                self.zones = Some(tz::system_zones());
            }
            let zones = self.zones.as_deref().unwrap_or(&[]);
            if let Some(key) = tz::resolve_zone_key(settings.timezone_key.as_deref(), zones)
                && let Some((now, off)) = tz::find(zones, &key).and_then(tz::now_and_offset)
            {
                text = format!(
                    "{} \u{b7} {:02}:{:02}",
                    tz::format_offset_short(off),
                    now.hour,
                    now.minute
                );
            }
        } else {
            self.zones = None;
        }
        self.tz_text = text;
    }

    fn arm_clock(&mut self) {
        if self.tz_text.is_empty() {
            os::kill_timer(self.hwnd, TIMER_CLOCK);
            return;
        }
        let now = os::local_now();
        let ms = (60 - now.second) * 1000 - now.millis + 50;
        os::set_timer(self.hwnd, TIMER_CLOCK, ms);
    }

    /// Re-read settings-driven data (holidays, clock) and repaint.
    pub fn refresh(&mut self, settings: &Settings) {
        self.update_clock(settings);
        let old = self.size;
        self.layout_and_fill(settings);
        if self.size != old {
            let (x, y) = self.place();
            os::set_window_pos(
                self.hwnd,
                x,
                y,
                self.size.0,
                self.size.1,
                None,
                false,
                false,
            );
            self.surface = None;
        }
        self.arm_clock();
        os::invalidate(self.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        let t = theme::theme(settings.dark_mode);
        if !std::ptr::eq(t, self.theme) {
            self.theme = t;
            self.art = Some(Art::new(self.m.scale, t));
            #[cfg(target_os = "macos")]
            {
                self.glass_art = None;
            }
            #[cfg(target_os = "macos")]
            os::dark_title_bar(self.hwnd, settings.dark_mode);
            self.today_pill.theme_changed();
            self.tz_pill.theme_changed();
            self.hide_tip();
        }
        self.refresh(settings);
    }

    fn shift_month(&mut self, settings: &Settings, delta: i32) {
        let bounded = self
            .display_month
            .shift_months(delta)
            .clamp(min_month(), max_month());
        if bounded != self.display_month {
            self.display_month = bounded;
            self.hide_tip();
            self.layout_and_fill(settings);
            os::invalidate(self.hwnd);
        }
    }

    fn go_today(&mut self, settings: &Settings) {
        self.display_month = app::today()
            .first_of_month()
            .clamp(min_month(), max_month());
        self.layout_and_fill(settings);
        os::invalidate(self.hwnd);
    }

    // -- painting --------------------------------------------------------------
    fn paint(&mut self) {
        #[cfg(target_os = "macos")]
        if os::is_glass(self.hwnd) {
            self.paint_glass();
            return;
        }
        let (w, h) = self.size;
        if self
            .surface
            .as_ref()
            .is_none_or(|s| (s.w, s.h) != (w as usize, h as usize))
        {
            self.surface = Surface::new(w as usize, h as usize);
        }
        let Some(mut s) = self.surface.take() else {
            return;
        };
        let t = self.theme;
        let mut art = self.art.take();
        self.draw(&mut s, t, &mut art);
        self.art = art;
        os::paint(self.hwnd, |dc| s.present(dc));
        self.surface = Some(s);
    }

    /// macOS: the flyout on Liquid Glass. The frame is drawn over a black
    /// and over a white card; the backend recovers every pixel's coverage
    /// from the two, so the glass shows wherever the card was.
    #[cfg(target_os = "macos")]
    fn paint_glass(&mut self) {
        let (w, h) = self.size;
        let base = *self.theme;
        let black = Color(0, 0, 0);
        let white = Color(255, 255, 255);
        let on_black = Theme {
            card_bg: black,
            card_border: black,
            ..base
        };
        let on_white = Theme {
            card_bg: white,
            card_border: white,
            ..base
        };
        let (mut art_b, mut art_w) = match self.glass_art.take() {
            Some(pair) => (Some(pair.0), Some(pair.1)),
            None => (
                Some(Art::new(self.m.scale, &on_black)),
                Some(Art::new(self.m.scale, &on_white)),
            ),
        };
        let (Some(mut sb), Some(mut sw)) = (
            Surface::new(w as usize, h as usize),
            Surface::new(w as usize, h as usize),
        ) else {
            return;
        };
        self.draw(&mut sb, &on_black, &mut art_b);
        self.draw(&mut sw, &on_white, &mut art_w);
        if let (Some(b), Some(wa)) = (art_b, art_w) {
            self.glass_art = Some((b, wa));
        }
        os::present_matte(self.hwnd, &mut sb, &mut sw);
        os::validate(self.hwnd);
    }

    /// Draw the whole flyout into `s` with the colours of `t` (the card
    /// colour may differ from the theme's on macOS, see paint_glass).
    fn draw(&mut self, s: &mut Surface, t: &Theme, art: &mut Option<Art>) {
        let (w, h) = self.size;
        let card = self.theme.card_bg;
        s.fill_rect(rect(0, 0, w, h), rgb(t.card_border));
        s.fill_rect(rect(1, 1, w - 2, h - 2), rgb(t.card_bg));
        for pi in 0..self.panes.len() {
            let pane = &self.panes[pi];
            let (month_rect, year_rect) = (pane.month_rect, pane.year_rect);
            let (mt, yt) = (pane.month_text.clone(), pane.year_text.clone());
            let (prev, next, sep) = (pane.prev, pane.next, pane.separator);
            let headers = pane.headers.clone();
            tk_label(
                s,
                month_rect,
                t.card_bg,
                None,
                &mt,
                Some(self.m.font(14.0, true)),
                t.title_fg,
            );
            tk_label(
                s,
                year_rect,
                t.card_bg,
                None,
                &yt,
                Some(self.m.font(14.0, false)),
                t.year_fg,
            );
            for (r, dir) in [(prev, -1), (next, 1)] {
                let Some(r) = r else { continue };
                let key = match (dir, self.hover_nav == Some(dir)) {
                    (-1, false) => "nav_prev",
                    (-1, true) => "nav_prev_hover",
                    (_, false) => "nav_next",
                    (_, true) => "nav_next_hover",
                };
                let img = art.as_mut().and_then(|a| a.image(key).cloned());
                tk_label(s, r, t.card_bg, img.as_ref(), "", None, t.card_bg);
            }
            for (r, name, cw) in headers {
                let fg = if cw { t.cw_label_fg } else { t.day_name_fg };
                tk_label(
                    s,
                    r,
                    t.card_bg,
                    None,
                    name,
                    Some(self.m.font(11.0, false)),
                    fg,
                );
            }
            if let Some(r) = sep {
                s.fill_rect(r, rgb(t.separator_bg));
            }
        }
        for i in 0..self.cells.len() {
            let c = &self.cells[i];
            let key = if self.hover == Some(i) && c.hoverable {
                c.hover.clone()
            } else {
                c.rest.clone()
            };
            let (r, band, text, fg) = (c.rect, c.band, c.text.clone(), c.fg);
            let band = if band == card { t.card_bg } else { band };
            let img = art.as_mut().and_then(|a| a.image(&key).cloned());
            tk_label(
                s,
                r,
                band,
                img.as_ref(),
                &text,
                Some(self.m.font(11.0, false)),
                fg,
            );
        }
        s.fill_rect(self.footer_rule, rgb(t.separator_bg));
        self.today_pill.draw(s, &mut self.m, t);
        if !self.tz_text.is_empty() {
            self.tz_pill.draw(s, &mut self.m, t);
        }
    }

    // -- messages ----------------------------------------------------------------
    fn cell_at(&self, x: i32, y: i32) -> Option<usize> {
        self.cells.iter().position(|c| contains(c.rect, x, y))
    }

    fn nav_at(&self, x: i32, y: i32) -> Option<i32> {
        for p in &self.panes {
            if p.prev.is_some_and(|r| contains(r, x, y)) {
                return Some(-1);
            }
            if p.next.is_some_and(|r| contains(r, x, y)) {
                return Some(1);
            }
        }
        None
    }

    fn pill_at(&self, x: i32, y: i32) -> Option<bool> {
        if self.today_pill.hit(x, y) {
            Some(false)
        } else if !self.tz_text.is_empty() && self.tz_pill.hit(x, y) {
            Some(true)
        } else {
            None
        }
    }

    fn set_hover(&mut self, x: i32, y: i32) {
        let cell = self
            .cell_at(x, y)
            .filter(|&i| self.cells[i].kind == CellKind::Day);
        if cell != self.hover {
            self.hover = cell;
            self.hide_tip();
            if let Some(i) = cell
                && self.cells[i].tip.is_some()
            {
                self.tip_pending = Some(i);
                os::set_timer(self.hwnd, TIMER_TIP, TOOLTIP_DELAY_MS);
            }
            os::invalidate(self.hwnd);
        }
        let nav = self.nav_at(x, y);
        if nav != self.hover_nav {
            self.hover_nav = nav;
            os::invalidate(self.hwnd);
        }
        let pill = self.pill_at(x, y);
        let (th, zh) = (pill == Some(false), pill == Some(true));
        if th != self.today_pill.hover || zh != self.tz_pill.hover {
            self.today_pill.hover = th;
            self.tz_pill.hover = zh;
            os::invalidate(self.hwnd);
        }
    }

    pub fn on_message(
        &mut self,
        settings: &mut Settings,
        msg: u32,
        wp: usize,
        lp: isize,
    ) -> Option<isize> {
        match msg {
            wm::WM_PAINT => {
                self.paint();
                Some(0)
            }
            wm::WM_ERASEBKGND => Some(1),
            wm::WM_MOUSEMOVE => {
                if !self.tracking {
                    os::track_leave(self.hwnd);
                    self.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                self.set_hover(x, y);
                Some(0)
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                self.tracking = false;
                self.set_hover(-1, -1);
                Some(0)
            }
            wm::WM_SETCURSOR => {
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.hwnd, cx, cy);
                let hand = self.nav_at(x, y).is_some() || self.pill_at(x, y).is_some();
                os::set_cursor(if hand { os::Cursor::Hand } else { os::Cursor::Arrow });
                Some(1)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if let Some(dir) = self.nav_at(x, y) {
                    self.shift_month(settings, dir);
                } else if let Some(p) = self.pill_at(x, y) {
                    self.pressed_pill = Some(p);
                    if p { self.tz_pill.pressed = true } else { self.today_pill.pressed = true }
                    os::capture(self.hwnd);
                    os::invalidate(self.hwnd);
                } else if let Some(i) = self.cell_at(x, y)
                    && let Some(date) = self.cells[i].date.filter(|_| self.cells[i].kind == CellKind::Day) {
                        self.selected = Some(date);
                        self.layout_and_fill(settings);
                        os::invalidate(self.hwnd);
                    }
                Some(0)
            }
            wm::WM_LBUTTONUP => {
                if let Some(p) = self.pressed_pill.take() {
                    os::release_capture();
                    self.today_pill.pressed = false;
                    self.tz_pill.pressed = false;
                    let (x, y) = super::mouse_xy(lp);
                    if self.pill_at(x, y) == Some(p) {
                        if p {
                            later(app::open_tz);
                        } else {
                            self.go_today(settings);
                        }
                    }
                    os::invalidate(self.hwnd);
                }
                Some(0)
            }
            wm::WM_RBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if let Some(i) = self.cell_at(x, y) {
                    let c = &self.cells[i];
                    let items: Vec<String> = match (c.kind, c.date, c.iso) {
                        (CellKind::Day, Some(d), _) => vec![
                            format!("Copy date ({})", dates::format_gui_date(d)),
                            format!("Copy week ({})", dates::format_calendar_week(d)),
                        ],
                        (CellKind::Week, _, Some((y_, w_))) => vec![format!("Copy week ({})", dates::format_iso_week(y_, w_))],
                        _ => Vec::new(),
                    };
                    if !items.is_empty() {
                        let copies: Vec<String> = match (c.kind, c.date, c.iso) {
                            (CellKind::Day, Some(d), _) => vec![dates::format_gui_date(d), dates::format_calendar_week(d)],
                            (_, _, Some((y_, w_))) => vec![dates::format_iso_week(y_, w_)],
                            _ => Vec::new(),
                        };
                        self.hide_tip();
                        self.menu = true;
                        let owner = self.hwnd;
                        let rows = items
                            .into_iter()
                            .enumerate()
                            .map(|(i, label)| menu::Row::Item { id: i as u32 + 1, label, checked: false })
                            .collect();
                        later(move || menu::open(rows, menu::Action::Copy { owner, copies }));
                    }
                }
                Some(0)
            }
            wm::WM_MOUSEWHEEL => {
                let delta = (wp >> 16) as u16 as i16;
                self.shift_month(settings, if delta > 0 { -1 } else { 1 });
                Some(0)
            }
            wm::WM_KEYDOWN => {
                match wp as u16 {
                    0x1B => later(|| app::close_calendar(false)),
                    0x25 => self.shift_month(settings, -1),
                    0x27 => self.shift_month(settings, 1),
                    0x24 => self.go_today(settings),
                    _ => {}
                }
                Some(0)
            }
            wm::WM_ACTIVATE => {
                if (wp & 0xFFFF) as u32 == wm::WA_INACTIVE && !self.menu {
                    later(|| app::close_calendar(true));
                }
                None
            }
            wm::WM_TIMER => {
                if wp == TIMER_TIP {
                    os::kill_timer(self.hwnd, TIMER_TIP);
                    if let Some(i) = self.tip_pending.take()
                        && let Some(text) = self.cells.get(i).and_then(|c| c.tip.clone()) {
                            let anchor = self.cells[i].rect;
                            let hwnd = self.hwnd;
                            later(move || show_tooltip(hwnd, text, anchor));
                        }
                } else if wp == TIMER_CLOCK {
                    let old = self.tz_text.clone();
                    self.update_clock(settings);
                    if old != self.tz_text {
                        self.refresh(settings);
                    } else {
                        self.arm_clock();
                    }
                }
                Some(0)
            }
            wm::WM_DESTROY => {
                os::kill_timer(self.hwnd, TIMER_TIP);
                os::kill_timer(self.hwnd, TIMER_CLOCK);
                None
            }
            _ => None,
        }
    }

    // -- holiday tooltip --------------------------------------------------------------
    fn hide_tip(&mut self) {
        os::kill_timer(self.hwnd, TIMER_TIP);
        self.tip_pending = None;
        if let Some(t) = self.tooltip.take() {
            let h = t.hwnd;
            later(move || os::destroy_window(h));
        }
    }

    /// The copy menu has closed.
    pub fn menu_closed(&mut self) {
        self.menu = false;
    }

    pub fn owns_tooltip(&self, h: HWND) -> bool {
        self.tooltip.as_ref().is_some_and(|t| t.hwnd == h)
    }

    pub fn on_tooltip_message(&mut self, msg: u32, _wp: usize, _lp: isize) -> Option<isize> {
        match msg {
            wm::WM_PAINT => {
                let t = self.theme;
                let tip = self.tooltip.as_ref()?;
                let (w, h) = tip.size;
                let lines = tip.lines.clone();
                let hwnd = tip.hwnd;
                let mut s = Surface::new(w as usize, h as usize)?;
                s.fill_rect(rect(0, 0, w, h), rgb(t.tooltip_border));
                s.fill_rect(rect(1, 1, w - 2, h - 2), rgb(t.tooltip_bg));
                let bd = self.m.label_bd;
                let (padx, pady) = (self.px(8.0), self.px(4.0));
                let font = self.m.font(11.0, false);
                let ls = font.metrics().linespace();
                let widths: Vec<i32> = lines.iter().map(|l| font.measure(l)).collect();
                let max_w = widths.iter().copied().max().unwrap_or(0);
                for (i, (line, lw)) in lines.iter().zip(&widths).enumerate() {
                    let x = 1 + bd + padx + (max_w - lw) / 2;
                    let y = 1 + bd + pady + i as i32 * ls;
                    s.dc().text(font, x, y, line, t.title_fg.colorref());
                }
                os::paint(hwnd, |dc| s.present(dc));
                Some(0)
            }
            wm::WM_MOUSEACTIVATE => Some(3 /* MA_NOACTIVATE */),
            _ => None,
        }
    }
}

fn show_tooltip(owner: HWND, text: String, anchor: Rect) {
    let geom = with_app(|a| {
        let c = a.calendar.as_mut().filter(|c| c.hwnd == owner)?;
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let bd = c.m.label_bd;
        let (padx, pady, p3) = (c.px(8.0), c.px(4.0), c.px(3.0));
        let font = c.m.font(11.0, false);
        let ls = font.metrics().linespace();
        let max_w = lines.iter().map(|l| font.measure(l)).max().unwrap_or(0);
        let (tw, th) = (
            max_w + 2 * padx + 2 * bd + 2,
            lines.len() as i32 * ls + 2 * pady + 2 * bd + 2,
        );
        let (rx, ry) = os::client_to_screen(c.hwnd, anchor.left, anchor.top);
        let w = c.work;
        let x = rx.min(w.right - tw).max(w.left);
        let mut y = ry + (anchor.bottom - anchor.top) + p3;
        if y + th > w.bottom {
            y = ry - th - p3;
        }
        Some((lines, x, y.max(w.top), tw, th))
    })
    .flatten();
    let Some((lines, x, y, w, h)) = geom else {
        return;
    };
    let Some(hwnd) = os::create_window(&os::WindowSpec {
        ex_style: os::WS_EX_TOPMOST_ | os::WS_EX_TOOLWINDOW_ | os::WS_EX_NOACTIVATE_,
        style: os::WS_POPUP_,
        title: "",
        x,
        y,
        w,
        h,
    }) else {
        return;
    };
    os::round_corners(hwnd, true);
    let kept = with_app(|a| match a.calendar.as_mut().filter(|c| c.hwnd == owner) {
        Some(c) if c.hover.is_some() => {
            c.tooltip = Some(Tooltip {
                hwnd,
                lines,
                size: (w, h),
            });
            true
        }
        _ => false,
    });
    if kept == Some(true) {
        os::show_window(hwnd, false);
    } else {
        os::destroy_window(hwnd);
    }
}
