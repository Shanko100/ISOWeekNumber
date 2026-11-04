//! Public Holidays: choose up to two countries (weeknumber_holiday_dialog.py).

use super::dialog::{self, DialogBase};
use super::dropdown::{Dropdown, Outcome};
use super::{Pill, Variant, canvas_oval, combo_geom, contains, draw_combo, rect, text_at};
use crate::app::{self, later, with_app};
use crate::holidays;
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Rect};
use crate::settings::Settings;

const WIDTH_PX: f64 = 520.0;
const HEIGHT_PX: f64 = 300.0;
const MARGIN_PX: f64 = 28.0;
const LIST_ROWS: usize = 16;
/// The small note beside Save: when the bundled holiday data was made
/// (build.rs takes it from the data's metadata).
const UPDATED: &str = concat!("Holidays last updated on ", env!("WN_HOLIDAYS_UPDATED"));
/// Point size of that note.
const NOTE_PT: f64 = 9.0;

#[derive(Default, Clone, Copy)]
struct Layout {
    title: (i32, i32),
    subtitle: (i32, i32),
    swatch: [Rect; 2],
    label: [(i32, i32); 2],
    combo: [Rect; 2],
    note: (i32, i32),
}

pub struct HolidayDialog {
    base: DialogBase,
    countries: Vec<(String, String)>,
    /// Index into `countries`, None = Off.
    choice: [Option<usize>; 2],
    l: Layout,
    save: Pill,
    dropdown: Option<(usize, Dropdown)>,
    focused: Option<usize>,
}

impl HolidayDialog {
    pub fn build(settings: &Settings, family: String) -> Option<Box<HolidayDialog>> {
        let base = DialogBase::new(settings, family, "Public Holidays");
        let countries = holidays::countries_by_name();
        let find = |c: &Option<String>| {
            c.as_ref()
                .and_then(|c| countries.iter().position(|(k, _)| k == c))
        };
        let choice = [
            find(&settings.holiday_primary),
            find(&settings.holiday_secondary),
        ];
        let save = Pill::new(Variant::Primary, "Save", 11.0, true, 0.0, &base.m);
        let mut d = Box::new(HolidayDialog {
            base,
            countries,
            choice,
            l: Layout::default(),
            save,
            dropdown: None,
            focused: None,
        });
        d.layout();
        Some(d)
    }

    fn layout(&mut self) {
        let b = &mut self.base;
        let (w, h) = (b.px(WIDTH_PX), b.px(HEIGHT_PX));
        b.client = (w, h);
        let margin = b.px(MARGIN_PX);
        let pady = b.px(22.0);
        let bd = b.m.label_bd;
        let inner_w = w - 2 * margin;
        let mut y = pady;
        let title_h = b.m.font(19.0, true).metrics().linespace() + 2 + 2 * bd;
        self.l.title = (margin + bd + 1, y + 1 + bd);
        y += title_h + b.px(4.0);
        let sub_h = b.m.font(11.0, false).metrics().linespace() + 2 + 2 * bd;
        self.l.subtitle = (margin + bd + 1, y + 1 + bd);
        y += sub_h + b.px(20.0);
        let sw = b.px(14.0);
        let label_h = b.m.font(11.0, true).metrics().linespace() + 2 + 2 * bd;
        let row_h = sw.max(label_h);
        let combo_h = combo_geom(&mut b.m).height;
        for i in 0..2 {
            self.l.swatch[i] = rect(margin, y + (row_h - sw) / 2, sw, sw);
            self.l.label[i] = (
                margin + sw + b.px(9.0) + bd + 1,
                y + (row_h - label_h) / 2 + 1 + bd,
            );
            y += row_h + b.px(5.0);
            self.l.combo[i] = rect(margin, y, inner_w, combo_h);
            y += combo_h + if i == 0 { b.px(15.0) } else { 0 };
        }
        let (pw, ph) = self.save.size(&mut b.m);
        self.save.rect = rect(margin + inner_w - pw, h - pady - ph, pw, ph);
        let note_h = b.m.font(NOTE_PT, false).metrics().linespace() + 2 + 2 * bd;
        self.l.note = (
            margin + bd + 1,
            self.save.rect.top + (ph - note_h) / 2 + 1 + bd,
        );
    }

    pub fn hwnd(&self) -> HWND {
        self.base.hwnd
    }

    pub fn owns(&self, h: HWND) -> bool {
        self.dropdown.as_ref().is_some_and(|(_, d)| d.hwnd == h)
    }

    pub fn create_window(&mut self) -> Option<HWND> {
        self.base.create(0.5)
    }

    pub fn after_create(hwnd: HWND) {
        with_app(|a| {
            let dark = a.settings.dark_mode;
            if let Some(d) = a.holiday.as_mut().filter(|d| d.base.hwnd == hwnd) {
                d.base.show(dark);
            }
        });
    }

    pub fn destroy(mut self: Box<Self>) {
        if let Some((_, d)) = self.dropdown.take() {
            os::destroy_window(d.hwnd);
        }
        os::destroy_window(self.base.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        if self.base.retheme(settings) {
            self.save.theme_changed();
        }
    }

    fn value_text(&self, i: usize) -> String {
        match self.choice[i] {
            Some(c) => self.countries[c].1.clone(),
            None => {
                if i == 0 {
                    "Off".into()
                } else {
                    "None / Off".into()
                }
            }
        }
    }

    fn paint(&mut self) {
        let Some(mut s) = self.base.begin_paint() else {
            return;
        };
        let t = self.base.theme;
        let m = &mut self.base.m;
        text_at(
            &mut s,
            self.l.title.0,
            self.l.title.1,
            "Public Holidays",
            m.font(19.0, true),
            t.title_fg,
        );
        text_at(
            &mut s,
            self.l.subtitle.0,
            self.l.subtitle.1,
            "Show nationwide non-working holidays from up to two countries.",
            m.font(11.0, false),
            t.year_fg,
        );
        let labels = ["Primary (purple)", "Secondary (teal)"];
        let fills = [t.holiday_bg, t.holiday_secondary_bg];
        let one = m.px(1.0);
        let thirteen = m.px(13.0);
        for i in 0..2 {
            let r = self.l.swatch[i];
            canvas_oval(
                &mut s,
                r.left + one,
                r.top + one,
                r.left + thirteen,
                r.top + thirteen,
                fills[i],
                t.card_border,
            );
            text_at(
                &mut s,
                self.l.label[i].0,
                self.l.label[i].1,
                labels[i],
                m.font(11.0, true),
                t.title_fg,
            );
        }
        for i in 0..2 {
            let v = self.value_text(i);
            draw_combo(
                &mut s,
                &mut self.base.m,
                t,
                self.l.combo[i],
                &v,
                self.focused == Some(i),
            );
        }
        text_at(
            &mut s,
            self.l.note.0,
            self.l.note.1,
            UPDATED,
            self.base.m.font(NOTE_PT, false),
            t.week_num_fg,
        );
        self.save.draw(&mut s, &mut self.base.m, t);
        self.base.end_paint(s);
    }

    fn open_dropdown(&mut self, i: usize) {
        self.focused = Some(i);
        let mut values = vec![if i == 0 {
            "Off".to_string()
        } else {
            "None / Off".to_string()
        }];
        values.extend(self.countries.iter().map(|(_, n)| n.clone()));
        let current = Some(self.choice[i].map_or(0, |c| c + 1));
        let r = self.l.combo[i];
        let (x0, y0) = os::client_to_screen(self.base.hwnd, r.left, r.top);
        let screen = rect(x0, y0, r.right - r.left, r.bottom - r.top);
        let work = self.base.mon.work;
        let hwnd = self.base.hwnd;
        os::invalidate(hwnd);
        later(move || {
            let Some(d) = Dropdown::create(values, current, LIST_ROWS, screen, work, true) else {
                return;
            };
            let h = d.hwnd;
            let kept = with_app(
                |a| match a.holiday.as_mut().filter(|x| x.base.hwnd == hwnd) {
                    Some(x) => {
                        x.dropdown = Some((i, d));
                        true
                    }
                    None => false,
                },
            );
            if kept == Some(true) {
                with_app(|a| {
                    if let Some((_, d)) = a.holiday.as_ref().and_then(|x| x.dropdown.as_ref()) {
                        d.show();
                    }
                });
            } else {
                os::destroy_window(h);
            }
        });
    }

    fn close_dropdown(&mut self) {
        if let Some((_, d)) = self.dropdown.take() {
            let h = d.hwnd;
            let owner = self.base.hwnd;
            later(move || {
                os::destroy_window(h);
                os::set_foreground(owner);
            });
        }
    }

    pub fn on_popup_message(&mut self, _h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
        let t = self.base.theme;
        let (r, outcome) = self.dropdown.as_mut()?.1.on_message(t, msg, wp, lp);
        match outcome {
            Outcome::None => {}
            Outcome::Cancelled => self.close_dropdown(),
            Outcome::Chosen(idx) => {
                let which = self.dropdown.as_ref().map_or(0, |d| d.0);
                self.choice[which] = idx.checked_sub(1);
                // The same country twice: the secondary goes back to off.
                if self.choice[0].is_some() && self.choice[0] == self.choice[1] {
                    self.choice[1] = None;
                    os::beep();
                }
                self.close_dropdown();
                os::invalidate(self.base.hwnd);
            }
        }
        r
    }

    fn save(&mut self, settings: &mut Settings) {
        let code = |c: Option<usize>| c.map(|i| self.countries[i].0.clone());
        let (p, s) = (code(self.choice[0]), code(self.choice[1]));
        if p.is_some() && p == s {
            os::beep();
            return;
        }
        let changed = p != settings.holiday_primary || s != settings.holiday_secondary;
        settings.holiday_primary = p;
        settings.holiday_secondary = s;
        settings.save();
        if changed {
            later(app::refresh_calendar);
        }
        later(app::close_holiday);
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
            wm::WM_CLOSE => {
                later(app::close_holiday);
                Some(0)
            }
            wm::WM_KEYDOWN | os::WM_APP_KEY => {
                if wp as u16 == os::VK_ESCAPE_ {
                    later(app::close_holiday);
                }
                Some(0)
            }
            wm::WM_MOUSEMOVE => {
                if !self.base.tracking {
                    os::track_leave(self.base.hwnd);
                    self.base.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                let hover = self.save.hit(x, y);
                if hover != self.save.hover {
                    self.save.hover = hover;
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            0x02A3 => {
                self.base.tracking = false;
                if self.save.hover {
                    self.save.hover = false;
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_SETCURSOR => {
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.base.hwnd, cx, cy);
                if (lp & 0xFFFF) as u32 != wm::HTCLIENT {
                    return None;
                }
                os::set_cursor(if self.save.hit(x, y) {
                    os::Cursor::Hand
                } else {
                    os::Cursor::Arrow
                });
                Some(1)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if let Some(i) = (0..2).find(|&i| contains(self.l.combo[i], x, y)) {
                    self.open_dropdown(i);
                } else if self.save.hit(x, y) {
                    self.save.pressed = true;
                    os::capture(self.base.hwnd);
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_LBUTTONUP => {
                if self.save.pressed {
                    self.save.pressed = false;
                    os::release_capture();
                    let (x, y) = super::mouse_xy(lp);
                    if self.save.hit(x, y) {
                        self.save(settings);
                    }
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_DPICHANGED => {
                let (dpi, suggested) = dialog::dpi_changed(wp, lp);
                let family = self.base.m.family.clone();
                self.base.m = super::Metrics::new(dpi, family);
                self.base.mon.dpi = dpi;
                self.save = Pill::new(Variant::Primary, "Save", 11.0, true, 0.0, &self.base.m);
                self.layout();
                if let Some(r) = suggested {
                    self.base.apply_dpi_rect(r);
                }
                os::invalidate(self.base.hwnd);
                Some(0)
            }
            _ => None,
        }
    }
}
