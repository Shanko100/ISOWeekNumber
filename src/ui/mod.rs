//! Shared pieces of every window: DPI scale, fonts, Tk-equivalent label
//! drawing, and the hand-drawn controls (capsule button, switch, combobox).
//! Sizes follow the Python/Tk version's rules exactly: px() rounds the way
//! Python's round() does, labels size and centre their text the way Tk's
//! label does (tkWinButton.c), and every shape is drawn supersampled and
//! resampled down as Pillow did.

pub mod about;
pub mod calendar;
pub mod color_dialog;
pub mod dialog;
pub mod dropdown;
pub mod holiday_dialog;
pub mod legal_dialog;
pub mod menu;
pub mod tz_dialog;

use crate::gfx::{self, Color, Filter, Rgba};
use crate::os::{self, Font, Quality, Rect, Surface};
use crate::pil;
use crate::theme::Theme;
use std::collections::HashMap;

pub use dialog::raise as raise_window;

/// UI font candidates, best first, as the Python version's UI_FONT_FAMILIES:
/// Windows 11's optical body face, then the Windows 10 face.
const UI_FONT_FAMILIES: [&str; 3] = ["Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI"];

/// The first installed candidate (Tk silently substitutes a missing family,
/// so each one is probed). Looked up once per window open.
pub fn ui_font_family() -> String {
    let installed = os::font_families();
    UI_FONT_FAMILIES
        .iter()
        .find(|f| installed.iter().any(|i| i == *f))
        .unwrap_or(&"Segoe UI")
        .to_string()
}

/// Physical pixels for a size given at 96 DPI: max(1, round(v * scale)),
/// with Python's round-half-to-even.
pub fn px(scale: f64, v: f64) -> i32 {
    (gfx::py_round(v * scale) as i32).max(1)
}

/// Per-window metrics and fonts (created on open, freed on close).
pub struct Metrics {
    pub scale: f64,
    pub family: String,
    fonts: HashMap<(i32, bool), Font>,
    /// Tk's default label border: GetSystemMetrics(SM_CXEDGE) at start-up.
    pub label_bd: i32,
}

impl Metrics {
    pub fn new(dpi: u32, family: String) -> Metrics {
        Metrics {
            scale: f64::from(dpi) / 96.0,
            family,
            fonts: HashMap::new(),
            label_bd: os::system_metric(os::SM_CXEDGE_).max(0),
        }
    }
    pub fn px(&self, v: f64) -> i32 {
        px(self.scale, v)
    }
    /// The font Tk's (family, -px(size)[, bold]) names.
    pub fn font(&mut self, size: f64, bold: bool) -> &Font {
        let p = self.px(size);
        self.font_px(p, bold)
    }
    fn font_px(&mut self, p: i32, bold: bool) -> &Font {
        let family = self.family.clone();
        self.fonts.entry((p, bold)).or_insert_with(|| {
            Font::new(&family, p, bold, Quality::Default).expect("CreateFontIndirectW")
        })
    }
}

pub fn rgb(c: Color) -> u32 {
    (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
}

/// A rectangle helper.
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

pub fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Draw what a Tk label with -compound center shows in `r`: background,
/// the image (alpha-blended over the background, as Tk does) and the text,
/// each centred with C integer division.
pub fn tk_label(
    s: &mut Surface,
    r: Rect,
    bg: Color,
    image: Option<&Rgba>,
    text: &str,
    font: Option<&Font>,
    fg: Color,
) {
    s.fill_rect(r, rgb(bg));
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let (tw, th) = match (font, text.is_empty()) {
        (Some(f), false) => (f.measure(text), f.metrics().linespace()),
        _ => (0, 0),
    };
    let (iw, ih) = image.map_or((0, 0), |i| (i.w as i32, i.h as i32));
    let full_w = iw.max(tw);
    let full_h = ih.max(th);
    let x = (w - full_w) / 2;
    let y = (h - full_h) / 2;
    if let Some(img) = image {
        let shown = if img.px.iter().all(|p| p[3] == 255) {
            img.clone()
        } else {
            img.over_bg(bg)
        };
        s.blit(
            &shown,
            r.left + x + (full_w - iw) / 2,
            r.top + y + (full_h - ih) / 2,
        );
    }
    if let (Some(f), false) = (font, text.is_empty()) {
        s.dc().text(
            f,
            r.left + x + (full_w - tw) / 2,
            r.top + y + (full_h - th) / 2,
            text,
            rgb(fg).swap_rb(),
        );
    }
}

/// Left-anchored label text (Tk anchor "w" with the label's insets).
pub fn text_at(s: &mut Surface, x: i32, y: i32, text: &str, font: &Font, fg: Color) {
    s.dc().text(font, x, y, text, rgb(fg).swap_rb());
}

trait SwapRb {
    fn swap_rb(self) -> u32;
}

impl SwapRb for u32 {
    /// 0x00RRGGBB (our pixel order) -> COLORREF 0x00BBGGRR.
    fn swap_rb(self) -> u32 {
        ((self & 0xFF) << 16) | (self & 0xFF00) | ((self >> 16) & 0xFF)
    }
}

pub fn colorref(c: Color) -> u32 {
    c.colorref()
}

/// Size of a Tk label showing `text` (no image): text + 2*padx + 2*inset.
pub fn label_size(
    m: &mut Metrics,
    font_size: f64,
    bold: bool,
    text: &str,
    padx: i32,
    pady: i32,
    bd: i32,
) -> (i32, i32) {
    let f = m.font(font_size, bold);
    (
        f.measure(text) + 2 * padx + 2 * bd,
        f.metrics().linespace() + 2 * pady + 2 * bd,
    )
}

/// weeknumber_widgets.rounded(): a supersampled rounded rectangle on a known
/// background, resampled down with Lanczos. Opaque.
pub fn rounded(
    w: i32,
    h: i32,
    radius: f64,
    fill: Color,
    bg: Color,
    outline: Option<Color>,
    outline_width: i32,
) -> Rgba {
    let ss = 4usize;
    let (bw, bh) = (w as usize * ss, h as usize * ss);
    let mut img = Rgba::new(bw, bh, bg.rgba(255));
    let r = radius * ss as f64;
    let (x1, y1) = (bw as f64 - 1.0, bh as f64 - 1.0);
    img.fill(&pil::RoundedRectangle(0.0, 0.0, x1, y1, r), fill.rgba(255));
    if let Some(o) = outline {
        let width = ss as i32 * outline_width.max(1);
        img.fill(
            &pil::RoundedRectangleOutline(0.0, 0.0, x1, y1, r, width),
            o.rgba(255),
        );
    }
    img.resize_rgb(w as usize, h as usize, Filter::Lanczos)
}

// ---------------------------------------------------------------------------
// Capsule button (weeknumber_widgets.PillButton)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Primary,
    Secondary,
    Chip,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PillState {
    Rest,
    Hover,
    Pressed,
    Disabled,
}

pub struct Pill {
    pub variant: Variant,
    pub text: String,
    font_size: f64,
    bold: bool,
    min_width: i32,
    pub rect: Rect,
    pub hover: bool,
    pub pressed: bool,
    pub enabled: bool,
    cache: HashMap<(PillState, i32, i32, Color), Rgba>,
}

impl Pill {
    pub fn new(
        variant: Variant,
        text: &str,
        font_size: f64,
        bold: bool,
        min_width_px: f64,
        m: &Metrics,
    ) -> Pill {
        Pill {
            variant,
            text: text.to_string(),
            font_size,
            bold,
            min_width: if min_width_px > 0.0 {
                m.px(min_width_px)
            } else {
                0
            },
            rect: Rect::default(),
            hover: false,
            pressed: false,
            enabled: true,
            cache: HashMap::new(),
        }
    }

    fn pad(&self) -> (f64, f64) {
        match self.variant {
            Variant::Primary | Variant::Secondary => (18.0, 7.0),
            Variant::Chip => (14.0, 5.0),
        }
    }

    /// (width, height): the label in the font it is drawn with, plus the
    /// padding on each side.
    pub fn size(&self, m: &mut Metrics) -> (i32, i32) {
        let (px_, py_) = self.pad();
        let (pxx, pyy) = (m.px(px_), m.px(py_));
        let f = m.font(self.font_size, self.bold);
        (
            (f.measure(&self.text) + 2 * pxx).max(self.min_width),
            f.metrics().linespace() + 2 * pyy,
        )
    }

    pub fn set_text(&mut self, text: &str) {
        if text != self.text {
            self.text = text.to_string();
        }
    }

    fn state(&self) -> PillState {
        if !self.enabled {
            PillState::Disabled
        } else if self.pressed {
            PillState::Pressed
        } else if self.hover {
            PillState::Hover
        } else {
            PillState::Rest
        }
    }

    fn fill(&self, t: &Theme, state: PillState) -> Option<Color> {
        match self.variant {
            Variant::Primary => Some(match state {
                PillState::Disabled => t.separator_bg,
                PillState::Pressed => gfx::shade(t.accent, 0.78),
                PillState::Hover => gfx::shade(t.accent, 0.88),
                PillState::Rest => t.accent,
            }),
            Variant::Secondary => Some(match state {
                PillState::Pressed => t.selected_day_fill,
                PillState::Hover => t.hover_bg,
                _ => t.card_bg,
            }),
            Variant::Chip => match state {
                PillState::Pressed => Some(t.selected_day_fill),
                PillState::Hover => Some(t.hover_bg),
                _ => None,
            },
        }
    }

    fn text_color(&self, t: &Theme) -> Color {
        if !self.enabled {
            t.week_num_fg
        } else {
            match self.variant {
                Variant::Primary => t.today_fg,
                Variant::Chip => t.accent,
                Variant::Secondary => t.title_fg,
            }
        }
    }

    pub fn theme_changed(&mut self) {
        self.cache.clear();
    }

    /// Draw at self.rect (the label's full cell; the capsule is centred).
    pub fn draw(&mut self, s: &mut Surface, m: &mut Metrics, t: &Theme) {
        let (w, h) = self.size(m);
        let state = self.state();
        let outline_w = m.px(1.0);
        let fill = self.fill(t, state).unwrap_or(t.card_bg);
        let outline = (self.variant == Variant::Secondary).then_some(t.input_border);
        let img = self
            .cache
            .entry((state, w, h, t.card_bg))
            .or_insert_with(|| {
                rounded(
                    w,
                    h,
                    f64::from(h) / 2.0,
                    fill,
                    t.card_bg,
                    outline,
                    outline_w,
                )
            })
            .clone();
        let fg = self.text_color(t);
        let text = self.text.clone();
        let font = m.font(self.font_size, self.bold);
        tk_label(s, self.rect, t.card_bg, Some(&img), &text, Some(font), fg);
    }

    /// The capsule's own rectangle inside self.rect.
    pub fn hit(&self, x: i32, y: i32) -> bool {
        contains(self.rect, x, y)
    }
}

// ---------------------------------------------------------------------------
// Switch (weeknumber_widgets.Switch)
// ---------------------------------------------------------------------------

pub struct Switch {
    pub value: bool,
    pub rect: Rect,
    cache: HashMap<bool, Rgba>,
}

impl Switch {
    pub fn new(value: bool) -> Switch {
        Switch {
            value,
            rect: Rect::default(),
            cache: HashMap::new(),
        }
    }
    pub fn size(m: &Metrics) -> (i32, i32) {
        (m.px(40.0), m.px(22.0))
    }
    pub fn theme_changed(&mut self) {
        self.cache.clear();
    }
    pub fn draw(&mut self, s: &mut Surface, m: &Metrics, t: &Theme) {
        let (w, h) = Switch::size(m);
        let value = self.value;
        let img = self
            .cache
            .entry(value)
            .or_insert_with(|| {
                let ss = 4.0;
                let (bw, bh) = (w as usize * 4, h as usize * 4);
                let mut big = Rgba::new(bw, bh, t.card_bg.rgba(255));
                let track = if value { t.accent } else { t.toggle_track_off };
                big.fill(
                    &pil::RoundedRectangle(
                        0.0,
                        0.0,
                        bw as f64 - 1.0,
                        bh as f64 - 1.0,
                        f64::from(h) * ss / 2.0,
                    ),
                    track.rgba(255),
                );
                let inset = f64::from(h) * ss * 0.16;
                let diameter = f64::from(h) * ss - 2.0 * inset;
                let left = if value {
                    f64::from(w) * ss - inset - diameter
                } else {
                    inset
                };
                big.fill(
                    &pil::Ellipse(left, inset, left + diameter, inset + diameter),
                    t.toggle_knob.rgba(255),
                );
                big.resize_rgb(w as usize, h as usize, Filter::Lanczos)
            })
            .clone();
        tk_label(s, self.rect, t.card_bg, Some(&img), "", None, t.card_bg);
    }
}

// ---------------------------------------------------------------------------
// Combobox field (ttk clam, styled by weeknumber_widgets.style_combobox)
// ---------------------------------------------------------------------------

/// Geometry of a read-only ttk::combobox in the clam theme with
/// padding=px(4) and arrowsize=px(12).
pub struct ComboGeom {
    pub height: i32,
    pub arrow_w: i32,
    pub pad: i32,
}

/// Tk's scaling level (::tk::scalingPct / 100): system DPI to 25% steps.
pub fn tk_scaling_level() -> f64 {
    let pct = f64::from(os::system_dpi()) / 96.0 * 100.0;
    let mut level = 100.0;
    while pct >= level + 12.5 {
        level += 25.0;
    }
    level / 100.0
}

pub fn combo_geom(m: &mut Metrics) -> ComboGeom {
    let pad = m.px(4.0);
    let arrowsize = m.px(12.0);
    let apad = gfx::py_round(3.0 * tk_scaling_level()) as i32;
    let size = arrowsize - 2 * apad;
    let (aw, ah) = (2 * (size / 2) + 1 + 2 * apad, size / 2 + 1 + 2 * apad);
    let arrow_w = aw.max(ah);
    let ls = m.font(11.0, false).metrics().linespace();
    // field padding 2 + style padding + textarea (one line)
    let height = (ls + 2 * pad + 4).max(arrow_w);
    ComboGeom {
        height,
        arrow_w,
        pad,
    }
}

/// Draw the combobox at `r` showing `text`; `focused` colours the border.
pub fn draw_combo(s: &mut Surface, m: &mut Metrics, t: &Theme, r: Rect, text: &str, focused: bool) {
    let g = combo_geom(m);
    let border = rgb(if focused { t.accent } else { t.input_border });
    let field = rgb(t.input_bg);
    // The clam field element fills its whole box with the field colour
    // before drawing borders without corner pixels: the corners show it.
    s.fill_rect(r, field);
    let arrow = Rect {
        left: r.right - g.arrow_w,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    };
    // Field: box up to the arrow, widened by one pixel to overlap it.
    let f = Rect {
        left: r.left,
        top: r.top,
        right: arrow.left + 1,
        bottom: r.bottom,
    };
    smooth_border(s, f, border);
    s.fill_rect(
        Rect {
            left: f.left + 1,
            top: f.top + 1,
            right: f.right - 1,
            bottom: f.bottom - 1,
        },
        field,
    );
    // ComboboxFieldElementDraw: a full-height line at the field's right edge.
    s.fill_rect(
        Rect {
            left: f.right - 1,
            top: f.top,
            right: f.right,
            bottom: f.bottom,
        },
        border,
    );
    // Arrow element: smooth border, background, triangle.
    smooth_border(s, arrow, border);
    s.fill_rect(
        Rect {
            left: arrow.left + 1,
            top: arrow.top + 1,
            right: arrow.right - 1,
            bottom: arrow.bottom - 1,
        },
        field,
    );
    let apad = gfx::py_round(3.0 * tk_scaling_level()) as i32;
    let inner = Rect {
        left: arrow.left + apad,
        top: arrow.top + apad,
        right: arrow.right - apad,
        bottom: arrow.bottom - apad,
    };
    let bw = inner.right - inner.left;
    let bh = inner.bottom - inner.top;
    let half = bw / 2;
    let (cw, mut ch) = (2 * half + 1, half + 1);
    if (bh - ch) % 2 == 1 {
        ch += 1;
    }
    let ax = inner.left + (bw - cw) / 2;
    let ay = inner.top + (bh - ch) / 2;
    let h = (cw - 1) / 2;
    let h = if ch <= h { ch - 1 } else { h };
    let cx = ax + (cw - 1) / 2;
    let cy = ay + ch - 1;
    let ac = rgb(t.year_fg);
    for row in 0..=h {
        // Row `row` above the tip spans cx-row .. cx+row.
        let y = cy - row;
        s.fill_rect(
            Rect {
                left: cx - row,
                top: y,
                right: cx + row + 1,
                bottom: y + 1,
            },
            ac,
        );
    }
    // Text in the textarea: field padding 2 + style padding.
    let font = m.font(11.0, false);
    let ls = font.metrics().linespace();
    let ty = r.top + (r.bottom - r.top - ls) / 2;
    s.dc()
        .text(font, f.left + 2 + g.pad, ty, text, colorref(t.title_fg));
}

/// clam's DrawSmoothBorder with only the outer colour: a 1px frame whose
/// four corner pixels are left out.
fn smooth_border(s: &mut Surface, b: Rect, c: u32) {
    let (x1, x2, y1, y2) = (b.left, b.right - 1, b.top, b.bottom - 1);
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y1,
            right: x2,
            bottom: y1 + 1,
        },
        c,
    );
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y2,
            right: x2,
            bottom: y2 + 1,
        },
        c,
    );
    s.fill_rect(
        Rect {
            left: x1,
            top: y1 + 1,
            right: x1 + 1,
            bottom: y2,
        },
        c,
    );
    s.fill_rect(
        Rect {
            left: x2,
            top: y1 + 1,
            right: x2 + 1,
            bottom: y2,
        },
        c,
    );
}

/// A small filled circle with a 1px outline (Tk canvas create_oval), drawn
/// with the same GDI calls Tk makes.
pub fn canvas_oval(
    s: &mut Surface,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    fill: Color,
    outline: Color,
) {
    s.dc()
        .tk_oval(x0, y0, x1, y1, fill.colorref(), outline.colorref());
}

/// Client coordinates from a mouse message's lparam.
pub fn mouse_xy(lparam: isize) -> (i32, i32) {
    (
        i32::from(lparam as u16 as i16),
        i32::from((lparam >> 16) as u16 as i16),
    )
}
