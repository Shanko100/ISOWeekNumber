//! Icon Color: saturation/brightness field, hue slider, 36 presets, a hex
//! field and two previews (weeknumber_color_dialog.py). Apply changes the
//! icon; Cancel and Escape leave it alone.

use super::dialog::{self, DialogBase};
use super::{Metrics, Pill, Variant, contains, rect, rgb, rounded, text_at, tk_label};
use crate::app::{self, later, with_app};
use crate::gfx::{self, Color, Filter, Gray, Rgba};
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Brush, Rect};
use crate::pil;
use crate::settings::{self, Settings};

/// Where the week number is shown, in each platform's words.
#[cfg(windows)]
const SUBTITLE: &str = "The colour of the week number in the taskbar.";
#[cfg(target_os = "macos")]
const SUBTITLE: &str = "The colour of the week number in the menu bar.";
#[cfg(target_os = "linux")]
const SUBTITLE: &str = "The colour of the week number in the panel.";

const PRESETS: [(&str, &str); 36] = [
    ("#000000", "Black"),
    ("#334155", "Ink"),
    ("#0369A1", "Ocean"),
    ("#1E3A8A", "Navy"),
    ("#5B21B6", "Purple"),
    ("#BE185D", "Magenta"),
    ("#B91C1C", "Crimson"),
    ("#C2410C", "Rust"),
    ("#A16207", "Gold"),
    ("#15803D", "Forest"),
    ("#0F766E", "Pine"),
    ("#5C3317", "Chocolate"),
    ("#9CA3AF", "Silver"),
    ("#64748B", "Slate"),
    ("#4CC2FF", "Sky"),
    ("#2F6FED", "Blue"),
    ("#8B5CF6", "Violet"),
    ("#EC4899", "Pink"),
    ("#EF4444", "Red"),
    ("#F59E0B", "Amber"),
    ("#FACC15", "Yellow"),
    ("#22C55E", "Green"),
    ("#14B8A6", "Teal"),
    ("#8B5A2B", "Brown"),
    ("#FFFFFF", "White"),
    ("#CBD5E1", "Mist"),
    ("#BAE6FD", "Ice"),
    ("#93C5FD", "Light blue"),
    ("#C4B5FD", "Lavender"),
    ("#F9A8D4", "Rose"),
    ("#FCA5A5", "Salmon"),
    ("#FDBA74", "Peach"),
    ("#FEF08A", "Cream"),
    ("#86EFAC", "Mint"),
    ("#99F6E4", "Aqua"),
    ("#D2B48C", "Tan"),
];
const COLUMNS: usize = 12;
const ROWS: usize = 3;
const PREVIEW_DARK: Color = Color(0x20, 0x20, 0x20);
const PREVIEW_LIGHT: Color = Color(0xF3, 0xF3, 0xF3);

const MARGIN_PX: f64 = 24.0;
const FIELD_W_PX: f64 = 236.0;
const FIELD_H_PX: f64 = 164.0;
const HUE_H_PX: f64 = 14.0;
const GAP_PX: f64 = 14.0;
const KNOB_PX: f64 = 18.0;
const PREVIEW_W_PX: f64 = 112.0;
const PREVIEW_H_PX: f64 = 62.0;
const SWATCH_PX: f64 = 22.0;
const SWATCH_GAP_PX: f64 = 6.0;

const EDIT_ID: u16 = 100;

fn hex_to_hsv(hex: &str) -> (f64, f64, f64) {
    let (r, g, b) = settings::hex_to_rgb(hex);
    gfx::rgb_to_hsv(
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    )
}

fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
    let (r, g, b) = gfx::hsv_to_rgb(h.rem_euclid(1.0), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = |x: f64| gfx::py_round(x * 255.0).clamp(0.0, 255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", c(r), c(g), c(b))
}

#[derive(Default, Clone, Copy)]
struct Layout {
    title: (i32, i32),
    subtitle: (i32, i32),
    canvas: Rect,
    pad: i32,
    hue_y: i32,
    preview: [Rect; 2],
    preview_label: [(i32, i32); 2],
    swatches: Rect,
    hex_label: (i32, i32),
    entry: Rect,
    entry_inset: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Field,
    Hue,
}

pub struct ColorDialog {
    base: DialogBase,
    color: String,
    h: f64,
    s: f64,
    v: f64,
    week: u32,
    bold: bool,
    l: Layout,
    reset: Pill,
    cancel: Pill,
    apply: Pill,
    drag: Option<Drag>,
    edit: HWND,
    edit_focused: bool,
    brush: Option<Brush>,
    field_cache: Option<(u64, Rgba)>,
    hue_cache: Option<Rgba>,
    pressed: Option<u8>,
    /// Suppress EN_CHANGE while the program sets the field's text.
    setting_text: bool,
}

impl ColorDialog {
    pub fn build(settings: &Settings, family: String) -> Option<Box<ColorDialog>> {
        let base = DialogBase::new(settings, family, "Icon Color");
        let color = settings::normalize_hex(&settings.selected_color);
        let (h, s, v) = hex_to_hsv(&color);
        let reset = Pill::new(Variant::Chip, "Default", 10.0, true, 0.0, &base.m);
        let cancel = Pill::new(Variant::Secondary, "Cancel", 11.0, true, 0.0, &base.m);
        let apply = Pill::new(Variant::Primary, "Apply", 11.0, true, 0.0, &base.m);
        let mut d = Box::new(ColorDialog {
            base,
            color,
            h,
            s,
            v,
            week: app::today().iso_week().1,
            bold: settings.is_bold,
            l: Layout::default(),
            reset,
            cancel,
            apply,
            drag: None,
            edit: HWND::default(),
            edit_focused: false,
            brush: None,
            field_cache: None,
            hue_cache: None,
            pressed: None,
            setting_text: false,
        });
        d.layout();
        Some(d)
    }

    fn layout(&mut self) {
        let b = &mut self.base;
        let m: &mut Metrics = &mut b.m;
        let margin = m.px(MARGIN_PX);
        let pady = m.px(20.0);
        let bd = m.label_bd;
        let (fw, fh) = (m.px(FIELD_W_PX), m.px(FIELD_H_PX));
        let knob = m.px(KNOB_PX);
        let pad = knob / 2 + 1;
        let hue_y = pad + fh + m.px(GAP_PX);
        let canvas_w = fw + 2 * pad;
        let canvas_h = hue_y + m.px(HUE_H_PX) + pad;
        let (pw, ph) = (m.px(PREVIEW_W_PX), m.px(PREVIEW_H_PX));
        let prev_label_h = m.font(10.0, false).metrics().linespace() + 2 + 2 * bd;
        let (p3, p10, p16) = (m.px(3.0), m.px(10.0), m.px(16.0));
        let preview_box_w = (pw + 2).max(m.font(10.0, false).measure("Light") + 2 + 2 * bd);
        let previews_h = (ph + 2) + p3 + prev_label_h + p10 + (ph + 2) + p3 + prev_label_h;
        let body_w = canvas_w + p16 + preview_box_w;
        let body_h = canvas_h.max(previews_h);
        let (size, gap) = (m.px(SWATCH_PX), m.px(SWATCH_GAP_PX));
        let sw_w = COLUMNS as i32 * (size + gap) - gap + 4;
        let sw_h = ROWS as i32 * (size + gap) - gap + 4;
        let title_w = m.font(19.0, true).measure("Icon color") + 2 + 2 * bd;
        let title_h = m.font(19.0, true).metrics().linespace() + 2 + 2 * bd;
        let sub = SUBTITLE;
        let sub_w = m.font(11.0, false).measure(sub) + 2 + 2 * bd;
        let sub_h = m.font(11.0, false).metrics().linespace() + 2 + 2 * bd;
        // Hex row: label, entry (9 chars), Default chip.
        let hex_lw = m.font(11.0, true).measure("Hex") + 2 + 2 * bd;
        let hex_lh = m.font(11.0, true).metrics().linespace() + 2 + 2 * bd;
        let avg = m.font(11.0, false).measure("0");
        let inset = 1 + m.px(4.0) + 1;
        let entry_w = 9 * avg + 2 * inset;
        let entry_h = m.font(11.0, false).metrics().linespace() + 2 * inset;
        let (cw, ch) = self.reset.size(m);
        let hex_w = hex_lw + p10 + entry_w + p10 + cw;
        let hex_h = hex_lh.max(entry_h).max(ch);
        let (aw, ah) = self.apply.size(m);
        let (cnw, _) = self.cancel.size(m);
        let footer_w = cnw + m.px(8.0) + aw;
        let inner_w = [title_w, sub_w, body_w, sw_w, hex_w, footer_w]
            .into_iter()
            .max()
            .unwrap_or(0);
        let (p4, p14, p20) = (m.px(4.0), m.px(14.0), m.px(20.0));

        let x0 = margin;
        let mut y = pady;
        self.l.title = (x0 + bd + 1, y + 1 + bd);
        y += title_h + p4;
        self.l.subtitle = (x0 + bd + 1, y + 1 + bd);
        y += sub_h + p16;
        self.l.canvas = rect(x0, y + (body_h - canvas_h) / 2, canvas_w, canvas_h);
        self.l.pad = pad;
        self.l.hue_y = hue_y;
        let px0 = x0 + canvas_w + p16;
        let mut py = y;
        for i in 0..2 {
            self.l.preview[i] = rect(px0 + (preview_box_w - (pw + 2)) / 2, py, pw + 2, ph + 2);
            py += ph + 2 + p3;
            let label = if i == 0 { "Dark" } else { "Light" };
            let lw = m.font(10.0, false).measure(label) + 2 + 2 * bd;
            self.l.preview_label[i] = (px0 + (preview_box_w - lw) / 2 + 1 + bd, py + 1 + bd);
            py += prev_label_h + p10;
        }
        y += body_h + p16;
        self.l.swatches = rect(x0, y, sw_w, sw_h);
        y += sw_h + p14;
        self.l.hex_label = (x0 + bd + 1, y + (hex_h - hex_lh) / 2 + 1 + bd);
        let ex = x0 + hex_lw + p10;
        self.l.entry = rect(ex, y + (hex_h - entry_h) / 2, entry_w, entry_h);
        self.l.entry_inset = inset;
        self.reset.rect = rect(ex + entry_w + p10, y + (hex_h - ch) / 2, cw, ch);
        y += hex_h + p20;
        self.apply.rect = rect(x0 + inner_w - aw, y, aw, ah);
        self.cancel.rect = rect(self.apply.rect.left - m.px(8.0) - cnw, y, cnw, ah);
        y += ah;
        b.client = (inner_w + 2 * margin, y + pady);
        self.field_cache = None;
        self.hue_cache = None;
    }

    pub fn hwnd(&self) -> HWND {
        self.base.hwnd
    }

    pub fn create_window(&mut self) -> Option<HWND> {
        let h = self.base.create(0.4)?;
        let font = self.base.m.font(11.0, false);
        self.edit = os::create_edit(h, EDIT_ID, font, &self.color)?;
        self.place_edit();
        Some(h)
    }

    fn place_edit(&self) {
        let e = self.l.entry;
        let i = self.l.entry_inset;
        let ls = e.bottom - e.top - 2 * i;
        os::move_child(
            self.edit,
            e.left + i,
            e.top + i,
            e.right - e.left - 2 * i,
            ls,
        );
    }

    pub fn after_create(hwnd: HWND) {
        with_app(|a| {
            let dark = a.settings.dark_mode;
            if let Some(d) = a.color.as_mut().filter(|d| d.base.hwnd == hwnd) {
                d.base.show(dark);
            }
        });
    }

    pub fn destroy(self: Box<Self>) {
        os::destroy_window(self.base.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        if self.base.retheme(settings) {
            for p in [&mut self.reset, &mut self.cancel, &mut self.apply] {
                p.theme_changed();
            }
            self.field_cache = None;
            self.hue_cache = None;
            self.brush = None;
        }
        self.bold = settings.is_bold;
        os::invalidate(self.edit);
    }

    // -- artwork ---------------------------------------------------------------
    /// `image` with antialiased rounded corners on the card colour.
    fn clip(&self, image: &Rgba, radius: f64) -> Rgba {
        let (w, h) = (image.w, image.h);
        let ss = 4;
        let mut mask = Gray::new(w * ss, h * ss, 0);
        mask.fill(
            &pil::RoundedRectangle(
                0.0,
                0.0,
                (w * ss) as f64 - 1.0,
                (h * ss) as f64 - 1.0,
                radius * ss as f64,
            ),
            255,
        );
        let mut out = Rgba::new(w, h, self.base.theme.card_bg.rgba(255));
        out.paste_masked(image, 0, 0, &mask.resize(w, h, Filter::Lanczos));
        out
    }

    fn field_image(&mut self) -> Rgba {
        let key = self.h.to_bits();
        if let Some((k, img)) = &self.field_cache
            && *k == key
        {
            return img.clone();
        }
        let m = &self.base.m;
        let (w, h) = (m.px(FIELD_W_PX) as usize, m.px(FIELD_H_PX) as usize);
        let (r, g, b) = gfx::hsv_to_rgb(self.h, 1.0, 1.0);
        let c = |x: f64| gfx::py_round(x * 255.0) as u8;
        let hue = Color(c(r), c(g), c(b));
        let across = gfx::rotate90(&gfx::linear_gradient()).resize(w, h, Filter::Bicubic);
        let down = gfx::linear_gradient().resize(w, h, Filter::Bicubic);
        let tinted = Rgba::composite(
            &Rgba::new(w, h, hue.rgba(255)),
            &Rgba::new(w, h, [255, 255, 255, 255]),
            &across,
        );
        let shaded = Rgba::composite(&Rgba::new(w, h, [0, 0, 0, 255]), &tinted, &down);
        let img = self.clip(&shaded, f64::from(m.px(8.0)));
        self.field_cache = Some((key, img.clone()));
        img
    }

    fn hue_image(&mut self) -> Rgba {
        if let Some(img) = &self.hue_cache {
            return img.clone();
        }
        let m = &self.base.m;
        let (w, h) = (m.px(FIELD_W_PX) as usize, m.px(HUE_H_PX) as usize);
        let mut strip = Rgba::new(w, h, [0, 0, 0, 255]);
        for x in 0..w {
            let (r, g, b) = gfx::hsv_to_rgb(x as f64 / (w.max(2) - 1) as f64, 1.0, 1.0);
            let c = |v: f64| gfx::py_round(v * 255.0) as u8;
            for y in 0..h {
                strip.px[y * w + x] = [c(r), c(g), c(b), 255];
            }
        }
        let img = self.clip(&strip, h as f64 / 2.0);
        self.hue_cache = Some(img.clone());
        img
    }

    fn knob(&self, fill: Color) -> Rgba {
        let size = self.base.m.px(KNOB_PX) as usize;
        let ss = 4usize;
        let n = size * ss;
        let nf = n as f64 - 1.0;
        let mut big = Rgba::new(n, n, [0, 0, 0, 0]);
        big.fill(&pil::Ellipse(0.0, 0.0, nf, nf), [0, 0, 0, 90]);
        big.fill(
            &pil::Ellipse(ss as f64, ss as f64, nf - ss as f64, nf - ss as f64),
            [255, 255, 255, 255],
        );
        let inset = gfx::py_round(n as f64 * 0.24);
        big.fill(
            &pil::Ellipse(inset, inset, nf - inset, nf - inset),
            fill.rgba(255),
        );
        big.resize(size, size, Filter::Lanczos)
    }

    fn swatch(&self, color: Color, selected: bool) -> Rgba {
        let t = self.base.theme;
        let size = self.base.m.px(SWATCH_PX) as usize;
        let ss = 4usize;
        let n = size * ss;
        let nf = n as f64 - 1.0;
        let mut big = Rgba::new(n, n, t.card_bg.rgba(255));
        let inset = if selected {
            big.fill(&pil::Ellipse(0.0, 0.0, nf, nf), t.accent.rgba(255));
            let inner = gfx::py_round(n as f64 * 0.12);
            big.fill(
                &pil::Ellipse(inner, inner, nf - inner, nf - inner),
                t.card_bg.rgba(255),
            );
            gfx::py_round(n as f64 * 0.2)
        } else {
            gfx::py_round(n as f64 * 0.08)
        };
        // ellipse(fill=color, outline=INPUT_BORDER, width=ss)
        big.fill(
            &pil::Ellipse(inset, inset, nf - inset, nf - inset),
            color.rgba(255),
        );
        big.fill(
            &pil::EllipseOutline(inset, inset, nf - inset, nf - inset, ss as i32),
            t.input_border.rgba(255),
        );
        big.resize_rgb(size, size, Filter::Lanczos)
    }

    fn paint(&mut self) {
        let Some(mut s) = self.base.begin_paint() else {
            return;
        };
        let t = self.base.theme;
        {
            let m = &mut self.base.m;
            text_at(
                &mut s,
                self.l.title.0,
                self.l.title.1,
                "Icon color",
                m.font(19.0, true),
                t.title_fg,
            );
            text_at(
                &mut s,
                self.l.subtitle.0,
                self.l.subtitle.1,
                SUBTITLE,
                m.font(11.0, false),
                t.year_fg,
            );
        }
        // Field and hue slider with their knobs.
        let c = self.l.canvas;
        let (pad, hue_y) = (self.l.pad, self.l.hue_y);
        let (fw, fh) = (self.base.px(FIELD_W_PX), self.base.px(FIELD_H_PX));
        let field = self.field_image();
        s.blit(&field, c.left + pad, c.top + pad);
        let hue = self.hue_image();
        s.blit(&hue, c.left + pad, c.top + hue_y);
        let knob = self.knob(Color::hex(&self.color));
        let k = knob.w as i32;
        let kx = (f64::from(pad) + self.s * f64::from(fw - 1)).round() as i32;
        let ky = (f64::from(pad) + (1.0 - self.v) * f64::from(fh - 1)).round() as i32;
        s.blend(&knob, c.left + kx - k / 2, c.top + ky - k / 2);
        let hue_knob = self.knob(Color::hex(&hsv_to_hex(self.h, 1.0, 1.0)));
        let hx = (f64::from(pad) + self.h * f64::from(fw - 1)).round() as i32;
        let hy = (f64::from(hue_y) + f64::from(self.base.px(HUE_H_PX)) / 2.0).round() as i32;
        s.blend(&hue_knob, c.left + hx - k / 2, c.top + hy - k / 2);
        // Previews: the week number on a dark and a light taskbar chip.
        let (pw, ph) = (self.base.px(PREVIEW_W_PX), self.base.px(PREVIEW_H_PX));
        let (r10, one) = (self.base.px(10.0), self.base.px(1.0));
        let week = self.week.to_string();
        let fg = Color::hex(&self.color);
        for (i, bg) in [PREVIEW_DARK, PREVIEW_LIGHT].into_iter().enumerate() {
            let img = rounded(
                pw,
                ph,
                f64::from(r10),
                bg,
                t.card_bg,
                Some(t.card_border),
                one,
            );
            let bold = self.bold;
            let font = self.base.m.font(26.0, bold);
            tk_label(
                &mut s,
                self.l.preview[i],
                t.card_bg,
                Some(&img),
                &week,
                Some(font),
                fg,
            );
            let label = if i == 0 { "Dark" } else { "Light" };
            let (lx, ly) = self.l.preview_label[i];
            text_at(
                &mut s,
                lx,
                ly,
                label,
                self.base.m.font(10.0, false),
                t.year_fg,
            );
        }
        // Presets.
        let (size, gap) = (self.base.px(SWATCH_PX), self.base.px(SWATCH_GAP_PX));
        for (i, (hex, _)) in PRESETS.iter().enumerate() {
            let (row, col) = (i / COLUMNS, i % COLUMNS);
            let img = self.swatch(Color::hex(hex), *hex == self.color);
            s.blit(
                &img,
                self.l.swatches.left + 2 + col as i32 * (size + gap),
                self.l.swatches.top + 2 + row as i32 * (size + gap),
            );
        }
        // Hex row.
        text_at(
            &mut s,
            self.l.hex_label.0,
            self.l.hex_label.1,
            "Hex",
            self.base.m.font(11.0, true),
            t.title_fg,
        );
        let e = self.l.entry;
        s.fill_rect(
            e,
            rgb(if self.edit_focused {
                t.accent
            } else {
                t.input_border
            }),
        );
        s.fill_rect(
            Rect {
                left: e.left + 1,
                top: e.top + 1,
                right: e.right - 1,
                bottom: e.bottom - 1,
            },
            rgb(t.input_bg),
        );
        for p in [&mut self.reset, &mut self.cancel, &mut self.apply] {
            p.draw(&mut s, &mut self.base.m, t);
        }
        self.base.end_paint(s);
    }

    // -- state -------------------------------------------------------------------
    fn set_entry_text(&mut self, text: &str) {
        self.setting_text = true;
        os::set_title(self.edit, text);
        self.setting_text = false;
    }

    fn set_hsv(&mut self, h: f64, s: f64, v: f64) {
        self.h = h.rem_euclid(1.0);
        self.s = s.clamp(0.0, 1.0);
        self.v = v.clamp(0.0, 1.0);
        self.color = hsv_to_hex(self.h, self.s, self.v);
        if os::get_text(self.edit).to_uppercase() != self.color {
            let c = self.color.clone();
            self.set_entry_text(&c);
        }
        os::invalidate(self.base.hwnd);
    }

    fn set_color(&mut self, color: &str, from_entry: bool) {
        let color = settings::normalize_hex(color);
        let (mut h, s, v) = hex_to_hsv(&color);
        if s == 0.0 || v == 0.0 {
            h = self.h; // grey, white, black: keep the hue the slider shows
        }
        (self.h, self.s, self.v) = (h, s, v);
        self.color = color.clone();
        if !from_entry {
            self.set_entry_text(&color);
        }
        os::invalidate(self.base.hwnd);
    }

    fn region(&self, x: i32, y: i32) -> Option<Drag> {
        let c = self.l.canvas;
        let (x, y) = (x - c.left, y - c.top);
        let (fw, fh) = (self.base.px(FIELD_W_PX), self.base.px(FIELD_H_PX));
        let pad = self.l.pad;
        if !(0..c.right - c.left).contains(&x) || !(0..c.bottom - c.top).contains(&y) {
            return None;
        }
        if pad - 4 <= x && x <= pad + fw + 4 && pad - 4 <= y && y <= pad + fh + 4 {
            return Some(Drag::Field);
        }
        let hue_h = self.base.px(HUE_H_PX);
        if self.l.hue_y - pad <= y && y <= self.l.hue_y + hue_h + pad {
            return Some(Drag::Hue);
        }
        None
    }

    fn pick(&mut self, x: i32, y: i32) {
        let c = self.l.canvas;
        let (x, y) = (f64::from(x - c.left), f64::from(y - c.top));
        let pad = f64::from(self.l.pad);
        let (fw, fh) = (
            f64::from(self.base.px(FIELD_W_PX)),
            f64::from(self.base.px(FIELD_H_PX)),
        );
        let fx = ((x - pad) / (fw - 1.0).max(1.0)).clamp(0.0, 1.0);
        match self.drag {
            Some(Drag::Field) => {
                let fy = ((y - pad) / (fh - 1.0).max(1.0)).clamp(0.0, 1.0);
                self.set_hsv(self.h, fx, 1.0 - fy);
            }
            Some(Drag::Hue) => self.set_hsv(fx.min(0.9999), self.s, self.v),
            None => {}
        }
    }

    fn swatch_at(&self, x: i32, y: i32) -> Option<usize> {
        let (size, gap) = (self.base.px(SWATCH_PX), self.base.px(SWATCH_GAP_PX));
        let (x, y) = (x - self.l.swatches.left, y - self.l.swatches.top);
        if x < 2
            || y < 2
            || !contains(
                rect(
                    0,
                    0,
                    self.l.swatches.right - self.l.swatches.left,
                    self.l.swatches.bottom - self.l.swatches.top,
                ),
                x,
                y,
            )
        {
            return None;
        }
        let (col, cx) = ((x - 2) / (size + gap), (x - 2) % (size + gap));
        let (row, cy) = ((y - 2) / (size + gap), (y - 2) % (size + gap));
        let idx = row as usize * COLUMNS + col as usize;
        (col < COLUMNS as i32 && cx <= size && cy <= size && idx < PRESETS.len()).then_some(idx)
    }

    /// Return or focus loss in the hex field: take a valid value, or put
    /// the current colour back (with a beep for Return).
    fn hex_commit(&mut self, keyboard: bool) {
        match settings::parse_hex(&os::get_text(self.edit)) {
            Some(c) => self.set_color(&c, false),
            None => {
                let c = self.color.clone();
                self.set_entry_text(&c);
                if keyboard {
                    os::beep();
                }
            }
        }
    }

    fn apply(&mut self) {
        let color =
            settings::parse_hex(&os::get_text(self.edit)).unwrap_or_else(|| self.color.clone());
        let color = settings::normalize_hex(&color);
        later(move || app::apply_color(color));
        later(app::close_color);
    }

    fn pills(&mut self) -> [&mut Pill; 3] {
        [&mut self.reset, &mut self.cancel, &mut self.apply]
    }

    pub fn on_message(
        &mut self,
        settings: &mut Settings,
        msg: u32,
        wp: usize,
        lp: isize,
    ) -> Option<isize> {
        let _ = settings;
        match msg {
            wm::WM_PAINT => {
                self.paint();
                Some(0)
            }
            wm::WM_ERASEBKGND => Some(1),
            wm::WM_CLOSE => {
                later(app::close_color);
                Some(0)
            }
            wm::WM_CTLCOLOREDIT => {
                let t = self.base.theme;
                let brush = self
                    .brush
                    .get_or_insert_with(|| Brush::new(t.input_bg.colorref()));
                Some(os::ctl_color(
                    wp,
                    t.title_fg.colorref(),
                    t.input_bg.colorref(),
                    brush,
                ))
            }
            wm::WM_COMMAND => {
                let (child, code, _) = os::command_parts(wp, lp);
                if child == self.edit {
                    match code {
                        os::EN_CHANGE_ if !self.setting_text => {
                            if let Some(c) = settings::parse_hex(&os::get_text(self.edit))
                                && c != self.color
                            {
                                self.set_color(&c, true);
                            }
                        }
                        os::EN_SETFOCUS_ => {
                            self.edit_focused = true;
                            os::invalidate(self.base.hwnd);
                        }
                        os::EN_KILLFOCUS_ => {
                            self.edit_focused = false;
                            self.hex_commit(false);
                            os::invalidate(self.base.hwnd);
                        }
                        _ => {}
                    }
                }
                Some(0)
            }
            os::WM_APP_KEY | wm::WM_KEYDOWN => {
                let in_edit = msg == os::WM_APP_KEY;
                match wp as u16 {
                    os::VK_ESCAPE_ => later(app::close_color),
                    os::VK_RETURN_ if in_edit => self.hex_commit(true),
                    os::VK_RETURN_ => self.apply(),
                    os::VK_TAB_ if in_edit => {
                        let h = self.base.hwnd;
                        later(move || os::set_focus(h));
                    }
                    os::VK_TAB_ => {
                        let e = self.edit;
                        later(move || {
                            os::set_focus(e);
                            os::select_all(e);
                        });
                    }
                    _ => {}
                }
                Some(0)
            }
            wm::WM_MOUSEMOVE => {
                if !self.base.tracking {
                    os::track_leave(self.base.hwnd);
                    self.base.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                if self.drag.is_some() {
                    self.pick(x, y);
                }
                let mut changed = false;
                for p in self.pills() {
                    let hv = p.hit(x, y);
                    if hv != p.hover {
                        p.hover = hv;
                        changed = true;
                    }
                }
                if changed {
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            0x02A3 => {
                self.base.tracking = false;
                for p in self.pills() {
                    p.hover = false;
                }
                os::invalidate(self.base.hwnd);
                Some(0)
            }
            wm::WM_SETCURSOR => {
                if (lp & 0xFFFF) as u32 != wm::HTCLIENT || lp as usize as u64 == 0 {
                    return None;
                }
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.base.hwnd, cx, cy);
                if contains(self.l.entry, x, y) {
                    return None;
                }
                let cur = if contains(self.l.canvas, x, y) {
                    os::Cursor::Cross
                } else if contains(self.l.swatches, x, y)
                    || self.pills().iter().any(|p| p.hit(x, y))
                {
                    os::Cursor::Hand
                } else {
                    os::Cursor::Arrow
                };
                os::set_cursor(cur);
                Some(1)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                let h = self.base.hwnd;
                later(move || os::set_focus(h));
                if contains(self.l.canvas, x, y) {
                    self.drag = self.region(x, y);
                    if self.drag.is_some() {
                        os::capture(self.base.hwnd);
                        self.pick(x, y);
                    }
                } else if let Some(i) = self.swatch_at(x, y) {
                    self.set_color(PRESETS[i].0, false);
                } else if let Some(i) = self.pills().iter().position(|p| p.hit(x, y)) {
                    self.pressed = Some(i as u8);
                    self.pills()[i].pressed = true;
                    os::capture(self.base.hwnd);
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_LBUTTONUP => {
                let (x, y) = super::mouse_xy(lp);
                if self.drag.take().is_some() {
                    os::release_capture();
                }
                if let Some(i) = self.pressed.take() {
                    os::release_capture();
                    let i = i as usize;
                    self.pills()[i].pressed = false;
                    if self.pills()[i].hit(x, y) {
                        match i {
                            0 => self.set_color("#FFFFFF", false),
                            1 => later(app::close_color),
                            _ => self.apply(),
                        }
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
                self.reset = Pill::new(Variant::Chip, "Default", 10.0, true, 0.0, &self.base.m);
                self.cancel =
                    Pill::new(Variant::Secondary, "Cancel", 11.0, true, 0.0, &self.base.m);
                self.apply = Pill::new(Variant::Primary, "Apply", 11.0, true, 0.0, &self.base.m);
                self.layout();
                let font = self.base.m.font(11.0, false).handle();
                os::set_font(self.edit, font);
                self.place_edit();
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
