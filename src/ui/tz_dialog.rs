//! Time Zone: a switch for the calendar clock, a world map coloured by
//! standard offset with an offset ruler, and the zone dropdown
//! (weeknumber_timezone_dialog.py and weeknumber_map.py).

use super::dialog::{self, DialogBase, SCREEN_MARGIN_PX};
use super::dropdown::{Dropdown, Outcome};
use super::{Metrics, Pill, Switch, Variant, combo_geom, contains, draw_combo, rect, rgb, text_at};
use crate::app::{self, later, with_app};
use crate::gfx::{self, Rgba};
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Font, Quality, Rect, TzInfo};
use crate::pack::{self, Map, ZoneRow};
use crate::pil;
use crate::settings::Settings;
use crate::tz;

const MAP_WIDTH_PX: f64 = 860.0;
const RULER_PX: f64 = 18.0;
const MIN_MAP_WIDTH_PX: f64 = 300.0;
const SUPERSAMPLE: usize = 2;
const SEA_MIX: f64 = 0.74;
const LIST_ROWS: usize = 18;
const TIMER_CLOCK: usize = 20;

#[derive(Default, Clone, Copy)]
struct Layout {
    switch_label: (i32, i32),
    switch_label_rect: Rect,
    map_border: Rect,
    canvas: Rect,
    combo_label: (i32, i32),
    combo: Rect,
    status: (i32, i32),
    /// Requested (width, height) of the switch row, combo row, footer row
    /// and combobox, as Tk's geometry managers would report them.
    rows: [(i32, i32); 4],
}

pub struct TzDialog {
    base: DialogBase,
    zones: Vec<TzInfo>,
    rows: Vec<ZoneRow>,
    values: Vec<String>,
    map: Option<Map>,
    selected: Option<String>,
    map_w: i32,
    map_h: i32,
    ruler_h: i32,
    l: Layout,
    switch: Switch,
    close: Pill,
    status: String,
    base_map: Option<Rgba>,
    composed: Option<Rgba>,
    dropdown: Option<Dropdown>,
    pressed: bool,
}

impl TzDialog {
    pub fn build(settings: &Settings, family: String) -> Option<Box<TzDialog>> {
        let base = DialogBase::new(settings, family, "Time Zone");
        let zones = tz::system_zones();
        let rows = tz::listed_zones(&zones);
        let values = rows
            .iter()
            .map(|z| {
                let off = tz::find(&zones, &z.key)
                    .and_then(tz::now_and_offset)
                    .map(|(_, o)| o);
                format!("({})  {}", tz::format_offset(off), z.label)
            })
            .collect();
        let map = if rows.is_empty() { None } else { pack::map() };
        let close = Pill::new(Variant::Primary, "Close", 11.0, true, 0.0, &base.m);
        let mut d = Box::new(TzDialog {
            base,
            zones,
            rows,
            values,
            map,
            selected: None,
            map_w: 10,
            map_h: 10,
            ruler_h: 0,
            l: Layout::default(),
            switch: Switch::new(settings.timezone_enabled),
            close,
            status: String::new(),
            base_map: None,
            composed: None,
            dropdown: None,
            pressed: false,
        });
        d.fit_map();
        Some(d)
    }

    /// Validate the saved zone against this system (the Python use_zones),
    /// then pick the zone shown.
    fn init_selection(&mut self, settings: &mut Settings) {
        let zones = &self.zones;
        settings.validate_timezone_key(&|k| tz::find(zones, k).is_some());
        self.selected = tz::resolve_zone_key(settings.timezone_key.as_deref(), &self.zones);
        self.render_status();
    }

    /// Lay out the chrome around a `map_w` x `map_h` map; returns the
    /// window's (width, height).
    fn layout(&mut self, map_w: i32, canvas_h: i32) -> (i32, i32) {
        let m = &mut self.base.m;
        let pad = m.px(16.0);
        let bd = m.label_bd;
        let (p10, p12, p14) = (m.px(10.0), m.px(12.0), m.px(14.0));
        let (sw, sh) = Switch::size(m);
        let lab = "Show a time zone clock in the calendar";
        let f11 = m.font(11.0, false);
        let ls = f11.metrics().linespace();
        let lab_w = f11.measure(lab) + 2 + 2 * bd;
        let lab_h = ls + 2 + 2 * bd;
        let switch_row_w = sw + p10 + lab_w;
        let switch_row_h = sh.max(lab_h);
        let combo_label_w = f11.measure("Time zone") + 2 + 2 * bd;
        let avg = f11.measure("0");
        let status_w = if self.status.is_empty() {
            0
        } else {
            f11.measure(&self.status)
        } + 2
            + 2 * bd;
        let g = combo_geom(m);
        let combo_req_w = 20 * avg + 2 * g.pad + 4 + g.arrow_w;
        let combo_row_w = combo_label_w + p10 + combo_req_w;
        let combo_row_h = lab_h.max(g.height);
        let (cw, ch) = self.close.size(m);
        let footer_w = status_w + cw;
        let footer_h = lab_h.max(ch);
        self.l.rows = [
            (switch_row_w, switch_row_h),
            (combo_row_w, combo_row_h),
            (footer_w, footer_h),
            (combo_req_w, g.height),
        ];
        let inner_w = [switch_row_w, map_w + 2, combo_row_w, footer_w]
            .into_iter()
            .max()
            .unwrap_or(0);
        let x0 = pad;
        let mut y = pad;
        self.switch.rect = rect(x0, y + (switch_row_h - sh) / 2, sw, sh);
        self.l.switch_label = (
            x0 + sw + p10 + bd + 1,
            y + (switch_row_h - lab_h) / 2 + 1 + bd,
        );
        self.l.switch_label_rect =
            rect(x0 + sw + p10, y + (switch_row_h - lab_h) / 2, lab_w, lab_h);
        y += switch_row_h + p12;
        self.l.map_border = rect(x0 + (inner_w - (map_w + 2)) / 2, y, map_w + 2, canvas_h + 2);
        self.l.canvas = rect(self.l.map_border.left + 1, y + 1, map_w, canvas_h);
        y += canvas_h + 2 + p14;
        self.l.combo_label = (x0 + bd + 1, y + (combo_row_h - lab_h) / 2 + 1 + bd);
        let cx = x0 + combo_label_w + p10;
        self.l.combo = rect(
            cx,
            y + (combo_row_h - g.height) / 2,
            inner_w - (cx - x0),
            g.height,
        );
        y += combo_row_h + p14;
        self.l.status = (x0 + bd + 1, y + (footer_h - lab_h) / 2 + 1 + bd);
        self.close.rect = rect(x0 + inner_w - cw, y + (footer_h - ch) / 2, cw, ch);
        y += footer_h;
        (inner_w + 2 * pad, y + pad)
    }

    /// weeknumber_timezone_dialog._fit_map: the largest map that fits the
    /// monitor next to the measured chrome.
    ///
    /// Python measures the chrome before _apply_theme has given the switch
    /// and the Close pill their images, so both are still empty labels then
    /// (Tk: 2 * padx wide, 4 px as measured on Windows), the status label
    /// is empty and the combobox still has the native (vista) style, which
    /// is narrower than the switch row (Python's 733 px map on a 1024 px work
    /// area pins the switch row as the widest). The map is sized against
    /// that chrome, as here.
    fn fit_map(&mut self) {
        const EMPTY_IMAGE_LABEL_W: i32 = 4;
        let placeholder = self.base.px(10.0);
        let (_, rh) = self.layout(placeholder, placeholder);
        let [(switch_w, switch_h), _, (_, footer_h), _] = self.l.rows;
        let m = &mut self.base.m;
        let (pad, bd) = (m.px(16.0), m.label_bd);
        let (sw, _) = Switch::size(m);
        let lab_h = m.font(11.0, false).metrics().linespace() + 2 + 2 * bd;
        let empty_label_w = 2 + 2 * bd;
        let inner_w = [
            switch_w - sw + EMPTY_IMAGE_LABEL_W,
            placeholder + 2,
            empty_label_w + EMPTY_IMAGE_LABEL_W,
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        let chrome_w = (2 * pad + inner_w - placeholder).max(0);
        let chrome_h = (rh - placeholder - switch_h - footer_h + 2 * lab_h).max(0);
        let margin = self.base.px(SCREEN_MARGIN_PX);
        let work = self.base.mon.work;
        let avail_w = f64::from(work.right - work.left - margin - chrome_w);
        let avail_h = f64::from(work.bottom - work.top - margin - chrome_h);
        let preferred = f64::from(self.base.px(MAP_WIDTH_PX));
        let minimum = f64::from(self.base.px(MIN_MAP_WIDTH_PX));
        let width = preferred
            .min(avail_w)
            .min(avail_h.max(1.0) * tz::ASPECT)
            .max(minimum);
        self.map_w = gfx::py_round(width) as i32;
        self.map_h = (gfx::py_round(width / tz::ASPECT) as i32).max(1);
        self.ruler_h = self.base.px(RULER_PX);
        self.relayout();
    }

    fn relayout(&mut self) {
        let (w, h) = self.layout(self.map_w, self.map_h + self.ruler_h);
        self.base.client = (w, h);
    }

    pub fn hwnd(&self) -> HWND {
        self.base.hwnd
    }

    pub fn owns(&self, h: HWND) -> bool {
        h == self.base.hwnd || self.dropdown.as_ref().is_some_and(|d| d.hwnd == h)
    }

    pub fn create_window(&mut self) -> Option<HWND> {
        self.base.create(0.5)
    }

    pub fn after_create(hwnd: HWND) {
        with_app(|a| {
            let dark = a.settings.dark_mode;
            let (settings, tzd) = (&mut a.settings, a.tz.as_mut());
            if let Some(d) = tzd.filter(|d| d.base.hwnd == hwnd) {
                d.init_selection(settings);
                d.relayout();
                d.base.show(dark);
                d.arm_clock();
            }
        });
    }

    pub fn destroy(mut self: Box<Self>) {
        if let Some(d) = self.dropdown.take() {
            os::destroy_window(d.hwnd);
        }
        os::destroy_window(self.base.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        if self.base.retheme(settings) {
            self.switch.theme_changed();
            self.close.theme_changed();
            self.base_map = None;
            self.composed = None;
        }
    }

    // -- map ----------------------------------------------------------------------
    fn hi_poly(&self, ring: &[(f32, f32)]) -> Vec<(f64, f64)> {
        let half_w = f64::from(self.map_w) * SUPERSAMPLE as f64 / 2.0;
        let half_h = f64::from(self.map_h) * SUPERSAMPLE as f64 / 2.0;
        ring.iter()
            .map(|&(lon, lat)| {
                let (nx, ny) = tz::project(f64::from(lon), f64::from(lat));
                (half_w + nx * half_w, half_h - ny * half_h)
            })
            .collect()
    }

    fn render_base(&self) -> Option<Rgba> {
        let map = self.map.as_ref()?;
        let t = self.base.theme;
        let (w, h) = (
            self.map_w as usize * SUPERSAMPLE,
            self.map_h as usize * SUPERSAMPLE,
        );
        let mut img = Rgba::new(w, h, t.map_ocean.rgba(255));
        for band in &map.bands {
            let fill = gfx::blend(t.map_band[band.tint], t.map_ocean, SEA_MIX);
            for ring in band.sea.iter().filter(|r| r.len() >= 3) {
                img.fill(&pil::Polygon(self.hi_poly(ring)), fill.rgba(255));
            }
        }
        for band in &map.bands {
            for ring in band.land.iter().filter(|r| r.len() >= 3) {
                img.fill(
                    &pil::Polygon(self.hi_poly(ring)),
                    t.map_band[band.tint].rgba(255),
                );
            }
        }
        // weeknumber_map.paint_borders: line(width=hairline), hairline =
        // max(1, round(SUPERSAMPLE * 0.6)) = 1, so no joints.
        let border = t.map_border_line.rgba(255);
        let hairline = gfx::py_round(SUPERSAMPLE as f64 * 0.6).max(1.0) as i32;
        for line in &map.borders {
            let pts = self.hi_poly(line);
            if pts.len() >= 2 {
                img.fill(&pil::Line(pts, hairline), border);
            }
        }
        Some(img)
    }

    fn compose(&mut self) {
        if self.base_map.is_none() {
            self.base_map = self.render_base();
        }
        let Some(mut img) = self.base_map.clone() else {
            return;
        };
        let t = self.base.theme;
        let row = self
            .selected
            .as_ref()
            .and_then(|k| self.rows.iter().find(|z| &z.key == k));
        if let (Some(z), Some(map)) = (row, self.map.as_ref()) {
            let edge = t.map_select_land;
            let sea = gfx::blend(edge, t.map_ocean, SEA_MIX);
            // _compose_map: stroke = max(1, round(supersample)); line()
            // adds joints only above width 4.
            let stroke = (SUPERSAMPLE as i32).max(1);
            if let Some(band) = z.band.and_then(|b| map.bands.get(b)) {
                for ring in &band.sea {
                    let pts = self.hi_poly(ring);
                    if pts.len() < 3 {
                        continue;
                    }
                    img.fill(&pil::Polygon(pts.clone()), sea.rgba(255));
                    let mut closed = pts;
                    closed.push(closed[0]);
                    img.fill(&pil::Line(closed, stroke), edge.rgba(255));
                }
                for ring in &band.land {
                    let pts = self.hi_poly(ring);
                    if pts.len() >= 3 {
                        img.fill(&pil::Polygon(pts), edge.rgba(255));
                    }
                }
            }
            let (x, y) = self.hi_poly(&[(z.lon as f32, z.lat as f32)])[0];
            let radius = (f64::from(self.map_w) * SUPERSAMPLE as f64 * 0.0065).max(3.0);
            // ellipse(fill=edge, outline=TODAY_FG, width=max(1, round(ss * 1.5)))
            let width = gfx::py_round(SUPERSAMPLE as f64 * 1.5).max(1.0) as i32;
            let (x0, y0, x1, y1) = (x - radius, y - radius, x + radius, y + radius);
            img.fill(&pil::Ellipse(x0, y0, x1, y1), edge.rgba(255));
            img.fill(
                &pil::EllipseOutline(x0, y0, x1, y1, width),
                t.today_fg.rgba(255),
            );
        }
        self.composed = Some(img.reduce2());
    }

    fn paint(&mut self) {
        if self.composed.is_none() && self.map.is_some() {
            self.compose();
        }
        let Some(mut s) = self.base.begin_paint() else {
            return;
        };
        let t = self.base.theme;
        self.switch.draw(&mut s, &self.base.m, t);
        let (lx, ly) = self.l.switch_label;
        text_at(
            &mut s,
            lx,
            ly,
            "Show a time zone clock in the calendar",
            self.base.m.font(11.0, false),
            t.title_fg,
        );
        s.fill_rect(self.l.map_border, rgb(t.card_border));
        let c = self.l.canvas;
        s.fill_rect(c, rgb(t.card_bg));
        if let Some(img) = &self.composed {
            s.blit(img, c.left, c.top + self.ruler_h);
        }
        // Map frame.
        let edge = rgb(t.map_edge);
        let (fx, fy, fw, fh) = (c.left, c.top + self.ruler_h, self.map_w, self.map_h);
        s.fill_rect(rect(fx, fy, fw, 1), edge);
        s.fill_rect(rect(fx, fy + fh - 1, fw, 1), edge);
        s.fill_rect(rect(fx, fy, 1, fh), edge);
        s.fill_rect(rect(fx + fw - 1, fy, 1, fh), edge);
        self.paint_ruler(&mut s);
        text_at(
            &mut s,
            self.l.combo_label.0,
            self.l.combo_label.1,
            "Time zone",
            self.base.m.font(11.0, false),
            t.title_fg,
        );
        let current = self
            .selected
            .as_ref()
            .and_then(|k| self.rows.iter().position(|z| &z.key == k))
            .map(|i| self.values[i].clone())
            .unwrap_or_default();
        draw_combo(&mut s, &mut self.base.m, t, self.l.combo, &current, false);
        let (sx, sy) = self.l.status;
        let status = self.status.clone();
        text_at(
            &mut s,
            sx,
            sy,
            &status,
            self.base.m.font(11.0, false),
            t.year_fg,
        );
        self.close.draw(&mut s, &mut self.base.m, t);
        self.base.end_paint(s);
    }

    fn paint_ruler(&mut self, s: &mut os::Surface) {
        let Some(map) = self.map.as_ref() else { return };
        let t = self.base.theme;
        let c = self.l.canvas;
        let half_w = f64::from(self.map_w) / 2.0;
        let font_px = self.base.px(9.0).max(7);
        let family = self.base.m.family.clone();
        let Some(font) = Font::new(&family, font_px, false, Quality::Default) else {
            return;
        };
        let ls = font.metrics().linespace();
        for hour in -12..=12 {
            let west = (f64::from(hour) * 15.0 - 7.5).max(-180.0);
            let east = (f64::from(hour) * 15.0 + 7.5).min(180.0);
            let x0 = half_w + west / 180.0 * half_w;
            let x1 = half_w + east / 180.0 * half_w;
            let tint = map
                .bands
                .iter()
                .filter(|b| b.utc.fract() == 0.0 && b.utc as i32 == hour)
                .map(|b| b.tint)
                .next_back();
            let fill = tint.map_or(t.map_ocean, |i| t.map_band[i]);
            let (a, b) = (x0.round() as i32, x1.round() as i32);
            s.fill_rect(rect(c.left + a, c.top, b - a, self.ruler_h), rgb(fill));
            if x1 - x0 >= f64::from(self.base.px(14.0)) {
                let label = if hour == 0 {
                    "UTC".to_string()
                } else {
                    format!("{hour:+}")
                };
                let tw = font.measure(&label);
                let cx = ((x0 + x1) / 2.0).round() as i32;
                let cy = (f64::from(self.ruler_h) / 2.0).round() as i32;
                s.dc().text(
                    &font,
                    c.left + cx - tw / 2,
                    c.top + cy - ls / 2,
                    &label,
                    t.map_ruler_fg.colorref(),
                );
            }
        }
    }

    // -- selection and status ---------------------------------------------------------
    fn render_status(&mut self) {
        self.status = if self.rows.is_empty() {
            "No time zones available on this system.".into()
        } else if let Some(key) = &self.selected {
            match tz::find(&self.zones, key).and_then(tz::now_and_offset) {
                None => "Time unavailable for this zone.".into(),
                Some((now, off)) => {
                    let name = self
                        .rows
                        .iter()
                        .find(|z| &z.key == key)
                        .map_or("System time zone".to_string(), |z| z.city.clone());
                    format!(
                        "{name}  \u{b7}  {}  \u{b7}  {:02}:{:02}",
                        tz::format_offset(Some(off)),
                        now.hour,
                        now.minute
                    )
                }
            }
        } else {
            "Select a time zone.".into()
        };
        let old = self.base.client;
        self.relayout();
        if self.base.client != old && !os::is_null(self.base.hwnd) {
            let (cw, ch) = self.base.client;
            let (_, _, ow, oh) = os::outer_size(os::WS_DIALOG, 0, cw, ch, self.base.mon.dpi);
            os::set_window_pos(self.base.hwnd, 0, 0, ow, oh, None, false, true);
        }
        os::invalidate(self.base.hwnd);
    }

    fn arm_clock(&self) {
        let now = os::local_now();
        os::set_timer(
            self.base.hwnd,
            TIMER_CLOCK,
            (60 - now.second) * 1000 - now.millis + 50,
        );
    }

    fn select(&mut self, settings: &mut Settings, key: String) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        settings.timezone_key = Some(key.clone());
        settings.save();
        self.selected = Some(key);
        self.composed = None;
        self.render_status();
        later(app::refresh_calendar);
    }

    fn toggle(&mut self, settings: &mut Settings) {
        self.switch.value = !self.switch.value;
        settings.timezone_enabled = self.switch.value;
        if self.switch.value && settings.timezone_key.is_none() {
            // Turning the clock on with nothing chosen adopts the machine's zone.
            if let Some(k) = self
                .selected
                .clone()
                .or_else(|| tz::system_zone_key(&self.zones))
            {
                settings.timezone_key = Some(k.clone());
                self.selected = Some(k);
                self.composed = None;
            }
        }
        settings.save();
        self.render_status();
        later(app::refresh_calendar);
    }

    fn map_click(&mut self, settings: &mut Settings, x: i32, y: i32) {
        let c = self.l.canvas;
        let (x, y) = (x - c.left, y - c.top);
        if self.rows.is_empty() || y < self.ruler_h {
            return;
        }
        let half_w = f64::from(self.map_w) / 2.0;
        let half_h = f64::from(self.map_h) / 2.0;
        let Some((lon, lat)) = tz::unproject(
            (f64::from(x) - half_w) / half_w,
            (half_h - f64::from(y - self.ruler_h)) / half_h,
        ) else {
            return;
        };
        let Some(map) = self.map.as_ref() else { return };
        if let Some(z) = tz::nearest_zone(&self.rows, map, lon, lat) {
            let key = z.key.clone();
            self.select(settings, key);
        }
    }

    fn open_dropdown(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|k| self.rows.iter().position(|z| &z.key == k));
        let r = self.l.combo;
        let (x0, y0) = os::client_to_screen(self.base.hwnd, r.left, r.top);
        let screen = rect(x0, y0, r.right - r.left, r.bottom - r.top);
        let (values, work, hwnd) = (self.values.clone(), self.base.mon.work, self.base.hwnd);
        later(move || {
            let Some(d) = Dropdown::create(values, current, LIST_ROWS, screen, work, false) else {
                return;
            };
            let h = d.hwnd;
            let kept = with_app(|a| match a.tz.as_mut().filter(|x| x.base.hwnd == hwnd) {
                Some(x) => {
                    x.dropdown = Some(d);
                    true
                }
                None => false,
            });
            if kept == Some(true) {
                with_app(|a| {
                    if let Some(d) = a.tz.as_ref().and_then(|x| x.dropdown.as_ref()) {
                        d.show();
                    }
                });
            } else {
                os::destroy_window(h);
            }
        });
    }

    fn close_dropdown(&mut self) {
        if let Some(d) = self.dropdown.take() {
            let (h, owner) = (d.hwnd, self.base.hwnd);
            later(move || {
                os::destroy_window(h);
                os::set_foreground(owner);
            });
        }
    }

    pub fn on_message(
        &mut self,
        settings: &mut Settings,
        h: HWND,
        msg: u32,
        wp: usize,
        lp: isize,
    ) -> Option<isize> {
        if h != self.base.hwnd {
            let t = self.base.theme;
            let (r, outcome) = self.dropdown.as_mut()?.on_message(t, msg, wp, lp);
            match outcome {
                Outcome::None => {}
                Outcome::Cancelled => self.close_dropdown(),
                Outcome::Chosen(i) => {
                    self.close_dropdown();
                    if let Some(key) = self.rows.get(i).map(|z| z.key.clone()) {
                        self.select(settings, key);
                    }
                }
            }
            return r;
        }
        match msg {
            wm::WM_PAINT => {
                self.paint();
                Some(0)
            }
            wm::WM_ERASEBKGND => Some(1),
            wm::WM_CLOSE => {
                later(app::close_tz);
                Some(0)
            }
            wm::WM_KEYDOWN | os::WM_APP_KEY => {
                match wp as u16 {
                    os::VK_ESCAPE_ => later(app::close_tz),
                    0x20 | os::VK_RETURN_ => {}
                    _ => {}
                }
                Some(0)
            }
            wm::WM_TIMER if wp == TIMER_CLOCK => {
                self.render_status();
                self.arm_clock();
                Some(0)
            }
            wm::WM_MOUSEMOVE => {
                if !self.base.tracking {
                    os::track_leave(self.base.hwnd);
                    self.base.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                let hv = self.close.hit(x, y);
                if hv != self.close.hover {
                    self.close.hover = hv;
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            0x02A3 => {
                self.base.tracking = false;
                if self.close.hover {
                    self.close.hover = false;
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_SETCURSOR => {
                if (lp & 0xFFFF) as u32 != wm::HTCLIENT {
                    return None;
                }
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.base.hwnd, cx, cy);
                let hand = self.close.hit(x, y)
                    || contains(self.switch.rect, x, y)
                    || contains(self.l.switch_label_rect, x, y)
                    || (!self.rows.is_empty() && contains(self.l.canvas, x, y));
                os::set_cursor(if hand {
                    os::Cursor::Hand
                } else {
                    os::Cursor::Arrow
                });
                Some(1)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if contains(self.switch.rect, x, y) || contains(self.l.switch_label_rect, x, y) {
                    self.toggle(settings);
                    os::invalidate(self.base.hwnd);
                } else if contains(self.l.canvas, x, y) {
                    self.map_click(settings, x, y);
                } else if contains(self.l.combo, x, y) {
                    self.open_dropdown();
                } else if self.close.hit(x, y) {
                    self.pressed = true;
                    self.close.pressed = true;
                    os::capture(self.base.hwnd);
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_LBUTTONUP => {
                if self.pressed {
                    self.pressed = false;
                    self.close.pressed = false;
                    os::release_capture();
                    let (x, y) = super::mouse_xy(lp);
                    if self.close.hit(x, y) {
                        later(app::close_tz);
                    }
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_DPICHANGED => {
                let (dpi, suggested) = dialog::dpi_changed(wp, lp);
                let family = self.base.m.family.clone();
                self.base.m = Metrics::new(dpi, family);
                self.base.mon.dpi = dpi;
                self.close = Pill::new(Variant::Primary, "Close", 11.0, true, 0.0, &self.base.m);
                self.switch.theme_changed();
                self.base_map = None;
                self.composed = None;
                self.fit_map();
                if let Some(r) = suggested {
                    self.base.apply_dpi_rect(r);
                }
                os::invalidate(self.base.hwnd);
                Some(0)
            }
            wm::WM_DESTROY => {
                os::kill_timer(self.base.hwnd, TIMER_CLOCK);
                None
            }
            _ => None,
        }
    }
}
