//! Legal notices (weeknumber_license_dialog.LicenseDialog): WeekNumber's
//! legal notices and every license text this build carries, as one long,
//! read-only text in the app's own style -- section titles in the accent
//! colour, the notices' sub-headings in bold, the opening paragraph as prose,
//! the license texts in a monospaced face -- with the slim themed scroll bar
//! (weeknumber_widgets.ScrollBar). Opened from About, cascaded from it.
//!
//! The text widget is drawn the way Tk's text widget draws it: word wrap,
//! per-tag fonts and line spacing, selection backgrounds over the full line
//! height and to the right edge for a selected line end, and Tk's scrolling
//! steps for the wheel, the keys and the scroll bar.

use super::dialog::{self, DialogBase};
use super::{Metrics, Pill, Variant, contains, rect, rgb, text_at};
use crate::app::{self, later, with_app};
use crate::gfx::{self, Color, Filter, Rgba};
use crate::legal::{self, Style, wrap};
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Font, Quality, Rect, Surface};
use crate::pil;
use crate::settings::Settings;
use crate::theme::Theme;
use std::collections::HashMap;

/// Window and text metrics in px @ 96 DPI.
const WIDTH_PX: f64 = 680.0;
const HEIGHT_PX: f64 = 600.0;
const MARGIN_PX: f64 = 24.0;
const MIN_W_PX: f64 = 420.0;
const MIN_H_PX: f64 = 320.0;
/// Offset from the About window it opens from.
const CASCADE_PX: f64 = 28.0;
const TEXT_PADX_PX: f64 = 14.0;
const TEXT_PADY_PX: f64 = 10.0;
const MONO_PX: f64 = 11.5;
/// ScrollBar: canvas width, thumb width at rest and active, shortest thumb.
const SB_WIDTH_PX: f64 = 12.0;
const SB_REST_PX: f64 = 4.0;
const SB_ACTIVE_PX: f64 = 8.0;
const SB_MIN_THUMB_PX: f64 = 28.0;

/// Sections shown as prose (GUI improvement), their paragraph and list
/// item spacing in px @ 96 DPI, and the prose font size.
const PROSE_SECTIONS: [&str; 2] = ["Legal notices", "Third-party notices"];
const PARAGRAPH_GAP_PX: f64 = 8.0;
const ITEM_GAP_PX: f64 = 3.0;
const PROSE_PX: f64 = 12.0;

const MONO_FAMILIES: [&str; 3] = ["Cascadia Mono", "Consolas", "Lucida Console"];
const SUBTITLE: &str = "WeekNumber's license and warranty, trademarks, privacy and encryption \
                        notices, followed by the license of every component it includes.";
const TIMER_AUTOSCAN: usize = 1;
const AUTOSCAN_MS: u32 = 50;

const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;

/// One logical line of the text (no newline) and its style.
struct Line {
    chars: Vec<char>,
    style: Style,
    /// A list item: lines after the first start under its text.
    hang: bool,
    /// Space below the line (px @ 96 DPI), instead of the style's.
    after: Option<f64>,
}

/// One display line: chars [start, end) of a logical line, at content
/// offset y, `h` high including its spacing, the text `above` px below y.
#[derive(Clone, Copy)]
struct DLine {
    line: usize,
    start: usize,
    end: usize,
    y: i32,
    h: i32,
    above: i32,
    last: bool,
    /// Left offset of the text (a list item's hanging indent).
    indent: i32,
}

/// A text position: column `col` of a logical line; col == len is its
/// newline, len + 1 just after it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Pos {
    line: usize,
    col: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
}

#[derive(Default, Clone, Copy)]
struct Layout {
    title: (i32, i32),
    subtitle: (i32, i32),
    frame: Rect,
    text: Rect,
    view: Rect,
    bar: Rect,
}

pub struct LegalDialog {
    base: DialogBase,
    lines: Vec<Line>,
    dlines: Vec<DLine>,
    content_h: i32,
    wrap_w: i32,
    scroll: i32,
    mono_family: String,
    mono: Option<Font>,
    widths: HashMap<(Style, char), i32>,
    subtitle_lines: Vec<String>,
    l: Layout,
    close: Pill,
    sel: Option<(Pos, Pos)>,
    /// The range the current drag started from, and the unit it extends by.
    anchor: Option<(Pos, Pos, Unit)>,
    clicks: (u64, i32, i32, u32),
    pointer: (i32, i32),
    bar_hover: bool,
    bar_drag: Option<(i32, f64)>,
    bar_cache: Option<((bool, i32, i32, i32), Rgba)>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl LegalDialog {
    pub fn build(settings: &Settings, family: String) -> Option<Box<LegalDialog>> {
        let text = legal::text()?;
        let mut lines = Vec::new();
        // GUI improvement: the notices (and the opening paragraph) read as
        // prose that wraps to the window; licence texts keep their layout.
        let mut section = String::new();
        for (chunk, style) in legal::styled_runs(&text) {
            let body = chunk.strip_suffix('\n').unwrap_or(&chunk);
            if style == Style::Heading {
                section = body.trim().to_string();
                // No empty heading line above it: spacing1 separates.
                lines.push(Line {
                    chars: body.trim_start_matches('\n').chars().collect(),
                    style,
                    hang: false,
                    after: None,
                });
                continue;
            }
            let prose = style == Style::Intro
                || (style == Style::Body && PROSE_SECTIONS.contains(&section.as_str()));
            if prose {
                for (text, item, ends) in legal::reflow(body) {
                    lines.push(Line {
                        chars: text.chars().collect(),
                        style: if style == Style::Intro {
                            Style::Intro
                        } else {
                            Style::Prose
                        },
                        hang: item,
                        after: Some(if ends { PARAGRAPH_GAP_PX } else { ITEM_GAP_PX }),
                    });
                }
                continue;
            }
            for part in body.split('\n') {
                lines.push(Line {
                    chars: part.chars().collect(),
                    style,
                    hang: false,
                    after: None,
                });
            }
        }
        // Tk's text always ends with an empty line of its own.
        lines.push(Line {
            chars: Vec::new(),
            style: Style::Body,
            hang: false,
            after: None,
        });
        let installed = os::font_families();
        let mono_family = MONO_FAMILIES
            .iter()
            .find(|f| installed.iter().any(|i| i == *f))
            .unwrap_or(&"Courier New")
            .to_string();
        let base = DialogBase::new(settings, family, "Legal notices");
        let close = Pill::new(Variant::Primary, "Close", 11.0, true, 0.0, &base.m);
        let mut d = Box::new(LegalDialog {
            base,
            lines,
            dlines: Vec::new(),
            content_h: 0,
            wrap_w: -1,
            scroll: 0,
            mono_family,
            mono: None,
            widths: HashMap::new(),
            subtitle_lines: Vec::new(),
            l: Layout::default(),
            close,
            sel: None,
            anchor: None,
            clicks: (0, 0, 0, 0),
            pointer: (0, 0),
            bar_hover: false,
            bar_drag: None,
            bar_cache: None,
        });
        d.set_metrics();
        let (w, h) = d.initial_size();
        d.base.client = (w, h);
        d.layout();
        Some(d)
    }

    /// Fonts for the current DPI (the mono face is the text's own font).
    fn set_metrics(&mut self) {
        let px = self.base.px(MONO_PX);
        self.mono = Font::new(&self.mono_family, px, false, Quality::Default);
        self.widths.clear();
        self.wrap_w = -1;
        self.bar_cache = None;
    }

    fn initial_size(&self) -> (i32, i32) {
        let work = self.base.mon.work;
        let margin = self.base.px(dialog::SCREEN_MARGIN_PX);
        (
            self.base.px(WIDTH_PX).min(work.right - work.left - margin),
            self.base.px(HEIGHT_PX).min(work.bottom - work.top - margin),
        )
    }

    fn style_font<'a>(m: &'a mut Metrics, mono: Option<&'a Font>, style: Style) -> &'a Font {
        match style {
            Style::Heading => m.font(13.0, true),
            Style::Subheading => m.font(11.5, true),
            Style::Intro => m.font(11.0, false),
            Style::Prose => m.font(PROSE_PX, false),
            Style::Body => match mono {
                Some(f) => f,
                None => m.font(MONO_PX, false),
            },
        }
    }

    /// spacing1 and spacing3 of a style (the tags', else the widget's).
    fn spacing(&self, style: Style) -> (i32, i32) {
        match style {
            Style::Heading => (self.base.px(14.0), self.base.px(8.0)),
            Style::Subheading => (self.base.px(8.0), self.base.px(3.0)),
            Style::Intro | Style::Body | Style::Prose => (0, self.base.px(1.0)),
        }
    }

    fn color(t: &Theme, style: Style) -> Color {
        match style {
            Style::Heading => t.accent,
            Style::Intro => t.year_fg,
            Style::Subheading | Style::Body | Style::Prose => t.title_fg,
        }
    }

    fn char_width(&mut self, style: Style, c: char) -> i32 {
        if let Some(&w) = self.widths.get(&(style, c)) {
            return w;
        }
        let f = Self::style_font(&mut self.base.m, self.mono.as_ref(), style);
        let w = f.measure(c.encode_utf8(&mut [0; 4]));
        self.widths.insert((style, c), w);
        w
    }

    fn line_widths(&mut self, li: usize) -> Vec<i32> {
        let style = self.lines[li].style;
        let chars = self.lines[li].chars.clone();
        chars.iter().map(|&c| self.char_width(style, c)).collect()
    }

    fn linespace(&mut self, style: Style) -> i32 {
        Self::style_font(&mut self.base.m, self.mono.as_ref(), style)
            .metrics()
            .linespace()
    }

    /// Place everything for the current client size, and re-wrap the text
    /// when its width changed (keeping the first visible character on top).
    fn layout(&mut self) {
        let (w, h) = self.base.client;
        let margin = self.base.px(MARGIN_PX);
        let pady = self.base.px(20.0);
        let bd = self.base.m.label_bd;
        let mut y = pady;
        let title_h = self.base.m.font(19.0, true).metrics().linespace() + 2 + 2 * bd;
        self.l.title = (margin + bd + 1, y + 1 + bd);
        y += title_h + self.base.px(4.0);
        // The subtitle label wraps at a fixed length, as Tk's wraplength.
        let wrap_len = self.base.px(WIDTH_PX - 2.0 * MARGIN_PX);
        let sub_chars: Vec<char> = SUBTITLE.chars().collect();
        let sub_widths: Vec<i32> = {
            let f = self.base.m.font(11.0, false);
            sub_chars
                .iter()
                .map(|c| f.measure(c.encode_utf8(&mut [0; 4])))
                .collect()
        };
        self.subtitle_lines = wrap(&sub_chars, &sub_widths, wrap_len)
            .into_iter()
            .map(|(a, b)| {
                sub_chars[a..b]
                    .iter()
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let ls11 = self.base.m.font(11.0, false).metrics().linespace();
        let sub_h = ls11 * self.subtitle_lines.len() as i32 + 2 + 2 * bd;
        self.l.subtitle = (margin + bd + 1, y + 1 + bd);
        y += sub_h + self.base.px(12.0);
        let (pw, ph) = self.close.size(&mut self.base.m);
        self.close.rect = rect(w - margin - pw, h - pady - ph, pw, ph);
        let frame_bottom = (h - pady - ph - self.base.px(14.0)).max(y + 2);
        self.l.frame = Rect {
            left: margin,
            top: y,
            right: (w - margin).max(margin + 2),
            bottom: frame_bottom,
        };
        let f = self.l.frame;
        let sbw = self.base.px(SB_WIDTH_PX);
        self.l.bar = Rect {
            left: (f.right - 1 - sbw).max(f.left + 1),
            top: f.top + 1,
            right: f.right - 1,
            bottom: f.bottom - 1,
        };
        self.l.text = Rect {
            left: f.left + 1,
            top: f.top + 1,
            right: self.l.bar.left,
            bottom: f.bottom - 1,
        };
        let (padx, pady_t) = (self.base.px(TEXT_PADX_PX), self.base.px(TEXT_PADY_PX));
        let t = self.l.text;
        self.l.view = Rect {
            left: t.left + padx,
            top: t.top + pady_t,
            right: (t.right - padx).max(t.left + padx + 1),
            bottom: (t.bottom - pady_t).max(t.top + pady_t + 1),
        };
        let wrap_w = self.l.view.right - self.l.view.left;
        if wrap_w != self.wrap_w {
            let top = self.top_pos();
            self.wrap_w = wrap_w;
            self.rewrap();
            if let Some(p) = top {
                self.scroll = self.dline_of(p).map_or(0, |i| self.dlines[i].y);
            }
        }
        self.clamp_scroll();
    }

    fn rewrap(&mut self) {
        let avail = self.wrap_w.max(1);
        let mut dlines = Vec::new();
        let mut y = 0;
        for li in 0..self.lines.len() {
            let style = self.lines[li].style;
            let widths = self.line_widths(li);
            let (sp1, sp3) = self.spacing(style);
            let sp3 = self.lines[li].after.map_or(sp3, |v| self.base.px(v));
            let ls = self.linespace(style);
            // A list item's later lines start under its text, after "•  ".
            let hang = if self.lines[li].hang {
                widths.iter().take(3).sum::<i32>()
            } else {
                0
            };
            let segs =
                legal::wrap_hanging(&self.lines[li].chars, &widths, avail, (avail - hang).max(1));
            let n = segs.len();
            for (k, (start, end)) in segs.into_iter().enumerate() {
                let above = if k == 0 { sp1 } else { 0 };
                let below = if k + 1 == n { sp3 } else { 0 };
                let h = above + ls + below;
                dlines.push(DLine {
                    line: li,
                    start,
                    end,
                    y,
                    h,
                    above,
                    last: k + 1 == n,
                    indent: if k == 0 { 0 } else { hang },
                });
                y += h;
            }
        }
        self.dlines = dlines;
        self.content_h = y.max(1);
    }

    fn view_h(&self) -> i32 {
        (self.l.view.bottom - self.l.view.top).max(1)
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_h - self.view_h()).max(0);
        self.scroll = self.scroll.clamp(0, max);
    }

    /// The display line at content offset y (clamped into the text).
    fn dline_at(&self, y: i32) -> usize {
        let y = y.clamp(0, (self.content_h - 1).max(0));
        self.dlines
            .partition_point(|d| d.y + d.h <= y)
            .min(self.dlines.len().saturating_sub(1))
    }

    fn dline_of(&self, p: Pos) -> Option<usize> {
        self.dlines
            .iter()
            .position(|d| d.line == p.line && (p.col < d.end || d.last))
    }

    fn top_pos(&self) -> Option<Pos> {
        let d = self.dlines.get(self.dline_at(self.scroll))?;
        Some(Pos {
            line: d.line,
            col: d.start,
        })
    }

    /// The view's visible fraction (Tk's yview).
    fn fractions(&self) -> (f64, f64) {
        let total = f64::from(self.content_h);
        let first = f64::from(self.scroll) / total;
        let last = (f64::from(self.scroll + self.view_h()) / total).min(1.0);
        (first, last)
    }

    fn scroll_by(&mut self, px: i32) {
        self.scroll += px;
        self.clamp_scroll();
        os::invalidate(self.base.hwnd);
    }

    fn moveto(&mut self, fraction: f64) {
        self.scroll = gfx::py_round(fraction * f64::from(self.content_h)) as i32;
        self.clamp_scroll();
        os::invalidate(self.base.hwnd);
    }

    /// `yview scroll n units`: whole display lines.
    fn scroll_lines(&mut self, n: i32) {
        let top = self.dline_at(self.scroll);
        let target = if n < 0 && self.scroll > self.dlines[top].y {
            top as i64 + i64::from(n) + 1
        } else {
            top as i64 + i64::from(n)
        };
        let target = target.clamp(0, self.dlines.len() as i64 - 1) as usize;
        self.scroll = self.dlines[target].y;
        self.clamp_scroll();
        os::invalidate(self.base.hwnd);
    }

    /// `yview scroll n pages`: the window height less two lines of the
    /// text's font (tkTextDisp.c).
    fn scroll_pages(&mut self, n: i32) {
        let height = self.view_h();
        let ch = self.linespace(Style::Body);
        let step = if ch * 4 >= height {
            let p = 3 * height / 4;
            if p < ch { ch.min(height) } else { p }
        } else {
            height - 2 * ch
        };
        self.scroll_by(step * n);
    }

    // -- selection --------------------------------------------------------------

    /// The text position nearest to a client point.
    fn pos_at(&mut self, x: i32, y: i32) -> Pos {
        let di = self.dline_at(y - self.l.view.top + self.scroll);
        let d = self.dlines[di];
        let style = self.lines[d.line].style;
        let mut acc = self.l.view.left + d.indent;
        let mut col = d.start;
        let chars = self.lines[d.line].chars[d.start..d.end].to_vec();
        for c in chars {
            let w = self.char_width(style, c);
            if x < acc + w / 2 {
                break;
            }
            acc += w;
            col += 1;
        }
        Pos { line: d.line, col }
    }

    fn unit_range(&self, p: Pos, unit: Unit) -> (Pos, Pos) {
        let chars = &self.lines[p.line].chars;
        match unit {
            Unit::Char => (p, p),
            Unit::Line => (
                Pos {
                    line: p.line,
                    col: 0,
                },
                if p.line + 1 < self.lines.len() {
                    Pos {
                        line: p.line + 1,
                        col: 0,
                    }
                } else {
                    Pos {
                        line: p.line,
                        col: chars.len(),
                    }
                },
            ),
            Unit::Word => {
                if p.col >= chars.len() {
                    return (
                        p,
                        Pos {
                            line: p.line,
                            col: p.col + 1,
                        },
                    );
                }
                let kind = is_word(chars[p.col]);
                let mut a = p.col;
                while a > 0 && is_word(chars[a - 1]) == kind && kind {
                    a -= 1;
                }
                let mut b = p.col + 1;
                while b < chars.len() && is_word(chars[b]) == kind && kind {
                    b += 1;
                }
                (
                    Pos {
                        line: p.line,
                        col: a,
                    },
                    Pos {
                        line: p.line,
                        col: b,
                    },
                )
            }
        }
    }

    fn select_to(&mut self, p: Pos) {
        let Some((a0, a1, unit)) = self.anchor else {
            return;
        };
        let (b0, b1) = self.unit_range(p, unit);
        self.sel = Some((a0.min(b0), a1.max(b1)));
        os::invalidate(self.base.hwnd);
    }

    fn selected_text(&self) -> Option<String> {
        let (s, e) = self.sel?;
        if s >= e {
            return None;
        }
        let mut out = String::new();
        for li in s.line..=e.line.min(self.lines.len() - 1) {
            let chars = &self.lines[li].chars;
            let a = if li == s.line {
                s.col.min(chars.len())
            } else {
                0
            };
            let b = if li == e.line {
                e.col.min(chars.len())
            } else {
                chars.len()
            };
            out.extend(&chars[a..b.max(a)]);
            let newline = Pos {
                line: li,
                col: chars.len(),
            };
            if newline >= s && newline < e {
                out.push_str("\r\n");
            }
        }
        Some(out)
    }

    fn copy(&self) {
        if let Some(text) = self.selected_text() {
            os::set_clipboard_text(self.base.hwnd, &text);
        }
    }

    fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        self.sel = Some((
            Pos { line: 0, col: 0 },
            Pos {
                line: last,
                col: self.lines[last].chars.len(),
            },
        ));
        os::invalidate(self.base.hwnd);
    }

    // -- scroll bar ----------------------------------------------------------------

    fn bar_active(&self) -> bool {
        self.bar_hover || self.bar_drag.is_some()
    }

    fn scrollable(&self) -> bool {
        let (first, last) = self.fractions();
        last - first < 1.0
    }

    /// (top inset, height) of the track inside the scroll bar.
    fn track(&self) -> (i32, i32) {
        let inset = self.base.px(2.0);
        let h = self.l.bar.bottom - self.l.bar.top;
        (inset, (h - 2 * inset).max(1))
    }

    /// The thumb's top and bottom inside the track; a thumb held at its
    /// minimum length moves over the rest, so it still reaches both ends.
    fn thumb(&self) -> (f64, f64) {
        let (_, height) = self.track();
        let height = f64::from(height);
        let (first, last) = self.fractions();
        let visible = last - first;
        let span = visible.max((f64::from(self.base.px(SB_MIN_THUMB_PX)) / height).min(1.0));
        let travel = if visible < 1.0 {
            (1.0 - span) / (1.0 - visible)
        } else {
            0.0
        };
        let top = first * travel * height;
        (top, height.min(top + span * height))
    }

    fn draw_bar(&mut self, s: &mut Surface) {
        let r = self.l.bar;
        let t = self.base.theme;
        s.fill_rect(r, rgb(t.input_bg));
        let height = r.bottom - r.top;
        if !self.scrollable() || height < 8 {
            return;
        }
        let active = self.bar_active();
        let bar = self.base.px(if active { SB_ACTIVE_PX } else { SB_REST_PX });
        let (inset, track_h) = self.track();
        let (top, bottom) = self.thumb();
        let top = (gfx::py_round(top) as i32).max(0);
        let bottom = track_h.min((top + bar).max(gfx::py_round(bottom) as i32));
        let key = (active, top, bottom, track_h);
        if self.bar_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let ss = 4usize;
            let (bw, bh) = (bar as usize * ss, track_h as usize * ss);
            let mut big = Rgba::new(bw, bh, t.input_bg.rgba(255));
            let radius = f64::from(bar) * ss as f64 / 2.0;
            if active {
                big.fill(
                    &pil::RoundedRectangle(0.0, 0.0, bw as f64 - 1.0, bh as f64 - 1.0, radius),
                    t.hover_bg.rgba(255),
                );
            }
            let thumb = if active {
                t.year_fg
            } else {
                t.toggle_track_off
            };
            big.fill(
                &pil::RoundedRectangle(
                    0.0,
                    f64::from(top) * ss as f64,
                    bw as f64 - 1.0,
                    f64::from(bottom) * ss as f64 - 1.0,
                    radius,
                ),
                thumb.rgba(255),
            );
            let img = big.resize_rgb(bar as usize, track_h as usize, Filter::Lanczos);
            self.bar_cache = Some((key, img));
        }
        if let Some((_, img)) = &self.bar_cache {
            s.blit(img, r.right - inset - bar, r.top + inset);
        }
    }

    fn bar_press(&mut self, y: i32) {
        if !self.scrollable() {
            return;
        }
        let (inset, _) = self.track();
        let (top, bottom) = self.thumb();
        let ty = f64::from(y - self.l.bar.top - inset);
        if top <= ty && ty <= bottom {
            self.bar_drag = Some((y, self.fractions().0));
            os::capture(self.base.hwnd);
        } else {
            self.scroll_pages(if ty < top { -1 } else { 1 });
        }
        os::invalidate(self.base.hwnd);
    }

    fn bar_motion(&mut self, y: i32) {
        let Some((start_y, start_first)) = self.bar_drag else {
            return;
        };
        let (_, height) = self.track();
        let (top, bottom) = self.thumb();
        // Pointer travel over the track's free length maps onto the view's.
        let free = (f64::from(height) - (bottom - top)).max(1.0);
        let (first, last) = self.fractions();
        let hidden = 1.0 - (last - first);
        let delta = f64::from(y - start_y) / free * hidden;
        self.moveto((start_first + delta).clamp(0.0, hidden));
    }

    // -- window ----------------------------------------------------------------------

    pub fn hwnd(&self) -> HWND {
        self.base.hwnd
    }

    /// Cascaded from the About window's client area, like a document opened
    /// from it; centred when About is gone. Always inside the work area.
    pub fn create_window(&mut self) -> Option<HWND> {
        let near = with_app(|a| a.about.as_ref().map(|d| d.client_origin())).flatten();
        self.create_near(near)
    }

    fn create_near(&mut self, near: Option<(i32, i32)>) -> Option<HWND> {
        let work = self.base.mon.work;
        let (w, h) = self.base.client;
        let (mut x, mut y) = (
            work.left + (work.right - work.left - w) / 2,
            work.top + (work.bottom - work.top - h) / 2,
        );
        if let Some((nx, ny)) = near {
            let step = self.base.px(CASCADE_PX);
            (x, y) = (nx + step, ny + step);
        }
        let x = work.left.max(x.min(work.right - w - self.base.px(16.0)));
        let y = work.top.max(y.min(work.bottom - h - self.base.px(48.0)));
        self.base.create_at(x, y, os::WS_RESIZABLE)
    }

    pub fn after_create(hwnd: HWND) {
        with_app(|a| {
            let dark = a.settings.dark_mode;
            if let Some(d) = a.legal.as_mut().filter(|d| d.base.hwnd == hwnd) {
                d.base.show(dark);
            }
        });
    }

    pub fn destroy(self: Box<Self>) {
        os::kill_timer(self.base.hwnd, TIMER_AUTOSCAN);
        os::destroy_window(self.base.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        if self.base.retheme(settings) {
            self.close.theme_changed();
            self.bar_cache = None;
        }
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
                "Legal notices",
                m.font(19.0, true),
                t.title_fg,
            );
            let f = m.font(11.0, false);
            let ls = f.metrics().linespace();
            for (i, line) in self.subtitle_lines.iter().enumerate() {
                text_at(
                    &mut s,
                    self.l.subtitle.0,
                    self.l.subtitle.1 + i as i32 * ls,
                    line,
                    f,
                    t.year_fg,
                );
            }
        }
        // 1 px hairline around the text, then the text area itself.
        s.fill_rect(self.l.frame, rgb(t.input_border));
        s.fill_rect(self.l.text, rgb(t.input_bg));
        self.draw_text(&mut s, t);
        self.draw_bar(&mut s);
        self.close.draw(&mut s, &mut self.base.m, t);
        self.base.end_paint(s);
    }

    fn draw_text(&mut self, s: &mut Surface, t: &Theme) {
        let view = self.l.view;
        let sel_bg = rgb(t.menu_active_bg);
        let first = self.dline_at(self.scroll);
        for di in first..self.dlines.len() {
            let d = self.dlines[di];
            let top = view.top + d.y - self.scroll;
            if top >= view.bottom {
                break;
            }
            let style = self.lines[d.line].style;
            let chars: Vec<char> = self.lines[d.line].chars[d.start..d.end].to_vec();
            let mut xs = Vec::with_capacity(chars.len() + 1);
            let mut x = view.left + d.indent;
            xs.push(x);
            for &c in &chars {
                x += self.char_width(style, c);
                xs.push(x);
            }
            // The selected part of this display line, if any.
            let mut span = None;
            if let Some((ss, se)) = self.sel {
                let len = self.lines[d.line].chars.len();
                let end_incl = if d.last { len + 1 } else { d.end };
                let lo = ss.max(Pos {
                    line: d.line,
                    col: d.start,
                });
                let hi = se.min(Pos {
                    line: d.line,
                    col: end_incl,
                });
                if lo < hi {
                    let a = lo.col - d.start;
                    let b = hi.col.min(d.end) - d.start;
                    let x0 = xs[a.min(chars.len())];
                    let x1 = if hi.col == end_incl {
                        view.right
                    } else {
                        xs[b]
                    };
                    let r = Rect {
                        left: x0.max(view.left),
                        top: top.max(view.top),
                        right: x1.min(view.right),
                        bottom: (top + d.h).min(view.bottom),
                    };
                    if r.left < r.right && r.top < r.bottom {
                        s.fill_rect(r, sel_bg);
                    }
                    span = Some((a, b.max(a)));
                }
            }
            if chars.is_empty() {
                continue;
            }
            let fg = Self::color(t, style);
            let ty = top + d.above;
            let font = Self::style_font(&mut self.base.m, self.mono.as_ref(), style);
            let piece = |from: usize, to: usize, color: Color| {
                if from < to {
                    let text: String = chars[from..to].iter().collect();
                    s.dc()
                        .text_clipped(font, xs[from], ty, &text, color.colorref(), view);
                }
            };
            match span {
                Some((a, b)) => {
                    piece(0, a, fg);
                    piece(a, b, t.menu_active_fg);
                    piece(b, chars.len(), fg);
                }
                None => piece(0, chars.len(), fg),
            }
        }
    }

    fn set_bar_hover(&mut self, hover: bool) {
        if hover != self.bar_hover {
            self.bar_hover = hover;
            os::invalidate(self.base.hwnd);
        }
    }

    /// Tk's TextAutoScan: while a selection drag is above or below the
    /// text, scroll toward the pointer by its distance, every 50 ms.
    fn autoscan(&mut self) {
        if self.anchor.is_none() {
            os::kill_timer(self.base.hwnd, TIMER_AUTOSCAN);
            return;
        }
        let (x, y) = self.pointer;
        let t = self.l.text;
        let wy = y - t.top;
        let h = t.bottom - t.top;
        if wy >= h {
            self.scroll_by(1 + wy - h);
        } else if wy < 0 {
            self.scroll_by(-1 + wy);
        } else {
            os::kill_timer(self.base.hwnd, TIMER_AUTOSCAN);
            return;
        }
        let p = self.pos_at(x, y);
        self.select_to(p);
    }

    fn press_text(&mut self, x: i32, y: i32) {
        let now = os::tick_ms();
        let (last, lx, ly, count) = self.clicks;
        let near = (x - lx).abs() <= os::system_metric(wm::SM_CXDOUBLECLK.0) / 2
            && (y - ly).abs() <= os::system_metric(wm::SM_CYDOUBLECLK.0) / 2;
        let count = if near && now.saturating_sub(last) <= os::double_click_ms() {
            count % 3 + 1
        } else {
            1
        };
        self.clicks = (now, x, y, count);
        let p = self.pos_at(x, y);
        let unit = match count {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        if unit == Unit::Char && os::key_down(os::VK_SHIFT_) && self.sel.is_some() {
            let (s, _) = self.sel.unwrap_or((p, p));
            self.anchor = Some((s, s, Unit::Char));
        } else {
            let (a, b) = self.unit_range(p, unit);
            self.anchor = Some((a, b, unit));
        }
        self.select_to(p);
        os::capture(self.base.hwnd);
    }

    pub fn on_message(
        &mut self,
        _settings: &mut Settings,
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
                later(app::close_legal);
                Some(0)
            }
            wm::WM_GETMINMAXINFO => {
                let saved = self.base.client;
                self.base.client = (self.base.px(MIN_W_PX), self.base.px(MIN_H_PX));
                let (ow, oh) = self.base.outer(os::WS_RESIZABLE);
                self.base.client = saved;
                os::set_min_track_size(lp, ow, oh);
                Some(0)
            }
            wm::WM_SIZE => {
                let (w, h) = os::client_size(self.base.hwnd);
                if w > 0 && h > 0 && (w, h) != self.base.client {
                    self.base.client = (w, h);
                    self.layout();
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_KEYDOWN => {
                let ctrl = os::key_down(os::VK_CONTROL_);
                match wp as u16 {
                    os::VK_ESCAPE_ => later(app::close_legal),
                    VK_PRIOR => self.scroll_pages(-1),
                    VK_NEXT => self.scroll_pages(1),
                    VK_UP => self.scroll_lines(-3),
                    VK_DOWN => self.scroll_lines(3),
                    VK_HOME => self.moveto(0.0),
                    VK_END => self.moveto(1.0),
                    0x41 if ctrl => self.select_all(),
                    0x43 if ctrl => self.copy(),
                    _ => {}
                }
                Some(0)
            }
            wm::WM_MOUSEWHEEL => {
                // tk::MouseWheel %W y [tk::ScaleNum %D] -4.0 pixels
                let delta = f64::from((wp >> 16) as u16 as i16);
                let amount = gfx::py_round(delta * self.base.m.scale);
                self.scroll_by(gfx::py_round(amount / -4.0) as i32);
                Some(0)
            }
            wm::WM_MOUSEMOVE => {
                if !self.base.tracking {
                    os::track_leave(self.base.hwnd);
                    self.base.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                self.pointer = (x, y);
                if self.bar_drag.is_some() {
                    self.bar_motion(y);
                } else if self.anchor.is_some() {
                    let p = self.pos_at(x, y);
                    self.select_to(p);
                    let t = self.l.text;
                    if y < t.top || y >= t.bottom {
                        os::set_timer(self.base.hwnd, TIMER_AUTOSCAN, AUTOSCAN_MS);
                    }
                } else {
                    let hover = self.close.hit(x, y);
                    if hover != self.close.hover {
                        self.close.hover = hover;
                        os::invalidate(self.base.hwnd);
                    }
                    self.set_bar_hover(contains(self.l.bar, x, y));
                }
                Some(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                self.base.tracking = false;
                if self.close.hover {
                    self.close.hover = false;
                    os::invalidate(self.base.hwnd);
                }
                if self.bar_drag.is_none() {
                    self.set_bar_hover(false);
                }
                Some(0)
            }
            wm::WM_SETCURSOR => {
                if (lp & 0xFFFF) as u32 != wm::HTCLIENT {
                    return None;
                }
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.base.hwnd, cx, cy);
                os::set_cursor(if self.close.hit(x, y) {
                    os::Cursor::Hand
                } else {
                    os::Cursor::Arrow
                });
                Some(1)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if contains(self.l.bar, x, y) {
                    self.bar_press(y);
                } else if contains(self.l.text, x, y) {
                    self.press_text(x, y);
                } else if self.close.hit(x, y) {
                    self.close.pressed = true;
                    os::capture(self.base.hwnd);
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_LBUTTONUP => {
                let (x, y) = super::mouse_xy(lp);
                if self.bar_drag.take().is_some() {
                    self.set_bar_hover(contains(self.l.bar, x, y));
                    os::invalidate(self.base.hwnd);
                }
                if self.anchor.take().is_some() {
                    os::kill_timer(self.base.hwnd, TIMER_AUTOSCAN);
                }
                os::release_capture();
                if self.close.pressed {
                    self.close.pressed = false;
                    if self.close.hit(x, y) {
                        later(app::close_legal);
                    }
                    os::invalidate(self.base.hwnd);
                }
                Some(0)
            }
            wm::WM_TIMER if wp == TIMER_AUTOSCAN => {
                self.autoscan();
                Some(0)
            }
            wm::WM_DPICHANGED => {
                let (dpi, suggested) = dialog::dpi_changed(wp, lp);
                let family = self.base.m.family.clone();
                self.base.m = Metrics::new(dpi, family);
                self.base.mon.dpi = dpi;
                self.close = Pill::new(Variant::Primary, "Close", 11.0, true, 0.0, &self.base.m);
                self.set_metrics();
                self.base.sync_icons();
                if let Some(r) = suggested {
                    os::set_window_pos(
                        self.base.hwnd,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        None,
                        false,
                        false,
                    );
                }
                self.base.client = os::client_size(self.base.hwnd);
                self.layout();
                os::invalidate(self.base.hwnd);
                Some(0)
            }
            _ => None,
        }
    }
}
