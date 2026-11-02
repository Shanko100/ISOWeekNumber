//! The read-only combobox's drop-down list, as ttk::combobox posts it in the
//! clam theme: a borderless topmost window with a 2px frame, a Tk listbox
//! (TkTextFont, one row per value, the selection following the pointer) and,
//! when the list is longer than the visible rows, a clam scrollbar.

use super::{contains, rect, rgb};
use crate::gfx::Color;
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Font, Rect, Surface};
use crate::theme::Theme;

const BORDER: Color = Color(0x9e, 0x9a, 0x91); // clam -darkest
const FRAME: Color = Color(0xdc, 0xda, 0xd5); // clam -frame
const TROUGH: Color = Color(0xba, 0xb5, 0xab); // clam -darker
const LIGHT: Color = Color(0xee, 0xeb, 0xe7); // clam -lighter
const DARK: Color = Color(0xcf, 0xcd, 0xc8); // clam -dark
const ARROW: Color = Color(0, 0, 0);

pub enum Outcome {
    /// Nothing for the owner to do.
    None,
    /// A value was chosen; the list is closing.
    Chosen(usize),
    /// Closed without a choice.
    Cancelled,
}

pub struct Dropdown {
    pub hwnd: HWND,
    values: Vec<String>,
    selected: usize,
    top: usize,
    rows: usize,
    font: Font,
    line_h: i32,
    size: (i32, i32),
    scrollbar: bool,
    sb_w: i32,
    typeahead: Option<String>,
    drag: Option<(i32, usize)>,
    tracking: bool,
}

impl Dropdown {
    /// Lay out the list under (or above) `combo` (screen coordinates).
    pub fn create(
        values: Vec<String>,
        current: Option<usize>,
        height_rows: usize,
        combo: Rect,
        work: Rect,
        typeahead: bool,
    ) -> Option<Dropdown> {
        let font = os::tk_text_font()?;
        let line_h = font.metrics().linespace() + 1;
        let sb_w = (10.5 * f64::from(os::system_dpi()) / 72.0 + 0.5) as i32;
        let mut rows = values.len().clamp(1, height_rows);
        let scrollbar = values.len() > height_rows;
        let w = combo.right - combo.left;
        let extra = 2 * 2 + 2; // frame padding + grid pady
        let mut h = rows as i32 * line_h + 2 + extra;
        let below = work.bottom - combo.bottom;
        let above = combo.top - work.top;
        if h > below && h > above {
            let room = below.max(above);
            rows = (((room - extra - 2) / line_h).max(1)) as usize;
            h = rows as i32 * line_h + 2 + extra;
        }
        let y = if h <= below {
            combo.bottom
        } else {
            combo.top - h
        };
        let selected = current.unwrap_or(0).min(values.len().saturating_sub(1));
        let hwnd = os::create_window(&os::WindowSpec {
            ex_style: os::WS_EX_TOPMOST_ | os::WS_EX_TOOLWINDOW_,
            style: os::WS_POPUP_,
            title: "",
            x: combo.left,
            y,
            w,
            h,
        })?;
        let mut d = Dropdown {
            hwnd,
            values,
            selected,
            top: 0,
            rows,
            font,
            line_h,
            size: (w, h),
            scrollbar,
            sb_w,
            typeahead: typeahead.then(String::new),
            drag: None,
            tracking: false,
        };
        d.see(selected);
        Some(d)
    }

    pub fn show(&self) {
        os::show_window(self.hwnd, true);
        os::set_foreground(self.hwnd);
        os::set_focus(self.hwnd);
    }

    fn list_rect(&self) -> Rect {
        let (w, h) = self.size;
        let right = if self.scrollbar {
            w - 2 - 1 - self.sb_w
        } else {
            w - 3
        };
        Rect {
            left: 3,
            top: 3,
            right,
            bottom: h - 3,
        }
    }

    fn sb_rect(&self) -> Rect {
        let (w, h) = self.size;
        Rect {
            left: w - 3 - self.sb_w,
            top: 3,
            right: w - 3,
            bottom: h - 3,
        }
    }

    fn max_top(&self) -> usize {
        self.values.len().saturating_sub(self.rows)
    }

    fn see(&mut self, i: usize) {
        if i < self.top {
            self.top = i;
        } else if i >= self.top + self.rows {
            self.top = i + 1 - self.rows;
        }
        self.top = self.top.min(self.max_top());
    }

    fn row_at(&self, y: i32) -> Option<usize> {
        let l = self.list_rect();
        let i = (y - l.top - 1).div_euclid(self.line_h);
        let idx = self.top as i64 + i64::from(i);
        (idx >= 0 && (idx as usize) < self.values.len() && y >= l.top).then_some(idx as usize)
    }

    /// (trough, thumb) rectangles of the scrollbar.
    fn sb_parts(&self) -> (Rect, Rect) {
        let sb = self.sb_rect();
        let a = self.sb_w;
        let trough = Rect {
            left: sb.left,
            top: sb.top + a,
            right: sb.right,
            bottom: sb.bottom - a,
        };
        let n = self.values.len().max(1) as f64;
        let first = self.top as f64 / n;
        let last = ((self.top + self.rows) as f64 / n).min(1.0);
        let min = a;
        let size = f64::from((trough.bottom - trough.top - min).max(0));
        let y = trough.top + (size * first) as i32;
        let h = (size * last) as i32 + min - (size * first) as i32;
        (
            trough,
            Rect {
                left: trough.left,
                top: y,
                right: trough.right,
                bottom: y + h,
            },
        )
    }

    pub fn paint(&self, t: &Theme) {
        let (w, h) = self.size;
        let Some(mut s) = Surface::new(w as usize, h as usize) else {
            return;
        };
        s.fill_rect(rect(0, 0, w, h), rgb(FRAME));
        // ComboboxPopdownFrame: solid 2px clam border, outer corners open.
        let b = rgb(BORDER);
        s.fill_rect(rect(1, 0, w - 2, 1), b);
        s.fill_rect(rect(1, h - 1, w - 2, 1), b);
        s.fill_rect(rect(0, 1, 1, h - 2), b);
        s.fill_rect(rect(w - 1, 1, 1, h - 2), b);
        s.fill_rect(rect(1, 1, w - 2, 1), b);
        s.fill_rect(rect(1, 1, 1, h - 2), b);
        s.fill_rect(rect(1, h - 2, w - 2, 1), b);
        s.fill_rect(rect(w - 2, 1, 1, h - 2), b);
        // Listbox: flat 1px border in its own background.
        let l = self.list_rect();
        s.fill_rect(l, rgb(t.menu_bg));
        for row in 0..self.rows {
            let i = self.top + row;
            let Some(v) = self.values.get(i) else { break };
            let y = l.top + 1 + row as i32 * self.line_h;
            let fg = if i == self.selected {
                s.fill_rect(
                    Rect {
                        left: l.left + 1,
                        top: y,
                        right: l.right - 1,
                        bottom: y + self.line_h,
                    },
                    rgb(t.menu_active_bg),
                );
                t.menu_active_fg
            } else {
                t.menu_fg
            };
            let clip_w = l.right - 1 - (l.left + 1);
            let text: String = v.clone();
            if clip_w > 0 {
                s.dc().text(&self.font, l.left + 1, y, &text, fg.colorref());
            }
        }
        // Re-draw the 1px border over any text overhang.
        let mbg = rgb(t.menu_bg);
        s.fill_rect(rect(l.right - 1, l.top, 1, l.bottom - l.top), mbg);
        if self.scrollbar {
            self.paint_scrollbar(&mut s);
        }
        os::paint(self.hwnd, |dc| s.present(dc));
    }

    fn paint_scrollbar(&self, s: &mut Surface) {
        let sb = self.sb_rect();
        let a = self.sb_w;
        let (trough, thumb) = self.sb_parts();
        // Trough: fill, then a 1px rectangle in the border colour.
        s.fill_rect(trough, rgb(TROUGH));
        outline(s, trough, rgb(BORDER));
        for (r, up) in [
            (rect(sb.left, sb.top, a, a), true),
            (rect(sb.left, sb.bottom - a, a, a), false),
        ] {
            smooth(s, r);
            let pad = (3.0 * super::tk_scaling_level()).round() as i32;
            let inner = Rect {
                left: r.left + pad,
                top: r.top + pad,
                right: r.right - pad,
                bottom: r.bottom - pad,
            };
            let bw = inner.right - inner.left;
            let half = bw / 2;
            let (cw, mut ch) = (2 * half + 1, half + 1);
            if (inner.bottom - inner.top - ch) % 2 == 1 {
                ch += 1;
            }
            let ax = inner.left + (bw - cw) / 2;
            let ay = inner.top + (inner.bottom - inner.top - ch) / 2;
            let hh = ((cw - 1) / 2).min(ch - 1);
            let cx = ax + (cw - 1) / 2;
            for row in 0..=hh {
                let y = if up { ay + row } else { ay + ch - 1 - row };
                s.fill_rect(rect(cx - row, y, 2 * row + 1, 1), rgb(ARROW));
            }
        }
        smooth(s, thumb);
        // Grip: alternating dark and light lines across the thumb's centre.
        let grip = (7.5 * f64::from(os::system_dpi()) / 72.0 + 0.5) as i32;
        let y0 = thumb.top + (thumb.bottom - thumb.top - grip) / 2;
        for i in 0..grip {
            let c = if i & 1 == 1 { LIGHT } else { BORDER };
            s.fill_rect(
                rect(thumb.left + 1, y0 + i, thumb.right - thumb.left - 2, 1),
                rgb(c),
            );
        }
    }

    fn choose(&self) -> Outcome {
        Outcome::Chosen(self.selected)
    }

    /// Handle a message for the list window.
    pub fn on_message(
        &mut self,
        t: &Theme,
        msg: u32,
        wp: usize,
        lp: isize,
    ) -> (Option<isize>, Outcome) {
        match msg {
            wm::WM_PAINT => {
                self.paint(t);
                (Some(0), Outcome::None)
            }
            wm::WM_ERASEBKGND => (Some(1), Outcome::None),
            wm::WM_MOUSEMOVE => {
                if !self.tracking {
                    os::track_leave(self.hwnd);
                    self.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                if let Some((y0, top0)) = self.drag {
                    let (trough, thumb) = self.sb_parts();
                    let span = f64::from((trough.bottom - trough.top) - (thumb.bottom - thumb.top))
                        .max(1.0);
                    let delta = f64::from(y - y0) / span * self.max_top() as f64;
                    self.top =
                        ((top0 as f64 + delta).round().max(0.0) as usize).min(self.max_top());
                    os::invalidate(self.hwnd);
                } else if contains(self.list_rect(), x, y)
                    && let Some(i) = self.row_at(y)
                    && i != self.selected
                {
                    self.selected = i;
                    os::invalidate(self.hwnd);
                }
                (Some(0), Outcome::None)
            }
            0x02A3 => {
                self.tracking = false;
                (Some(0), Outcome::None)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if self.scrollbar && contains(self.sb_rect(), x, y) {
                    let sb = self.sb_rect();
                    let (_, thumb) = self.sb_parts();
                    if y < sb.top + self.sb_w {
                        self.top = self.top.saturating_sub(1);
                    } else if y >= sb.bottom - self.sb_w {
                        self.top = (self.top + 1).min(self.max_top());
                    } else if y < thumb.top {
                        self.top = self.top.saturating_sub(self.rows);
                    } else if y >= thumb.bottom {
                        self.top = (self.top + self.rows).min(self.max_top());
                    } else {
                        self.drag = Some((y, self.top));
                        os::capture(self.hwnd);
                    }
                    os::invalidate(self.hwnd);
                }
                (Some(0), Outcome::None)
            }
            wm::WM_LBUTTONUP => {
                if self.drag.take().is_some() {
                    os::release_capture();
                    return (Some(0), Outcome::None);
                }
                let (x, y) = super::mouse_xy(lp);
                if contains(self.list_rect(), x, y) && self.row_at(y).is_some() {
                    return (Some(0), self.choose());
                }
                (Some(0), Outcome::None)
            }
            wm::WM_MOUSEWHEEL => {
                let delta = i32::from((wp >> 16) as u16 as i16);
                let units = -(delta as f64 / 40.0) as i64;
                self.top = (self.top as i64 + units).clamp(0, self.max_top() as i64) as usize;
                os::invalidate(self.hwnd);
                (Some(0), Outcome::None)
            }
            wm::WM_KEYDOWN => {
                let n = self.values.len();
                match wp as u16 {
                    0x1B => return (Some(0), Outcome::Cancelled),
                    0x0D | 0x09 => return (Some(0), self.choose()),
                    0x26 => self.selected = self.selected.saturating_sub(1),
                    0x28 => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
                    0x21 => self.selected = self.selected.saturating_sub(self.rows),
                    0x22 => self.selected = (self.selected + self.rows).min(n.saturating_sub(1)),
                    0x24 => self.selected = 0,
                    0x23 => self.selected = n.saturating_sub(1),
                    0x08 => {
                        if let Some(s) = self.typeahead.as_mut() {
                            s.pop();
                            let s = s.clone();
                            self.match_prefix(&s);
                        }
                    }
                    _ => return (Some(0), Outcome::None),
                }
                let sel = self.selected;
                self.see(sel);
                os::invalidate(self.hwnd);
                (Some(0), Outcome::None)
            }
            wm::WM_CHAR => {
                let Some(c) = char::from_u32(wp as u32) else {
                    return (Some(0), Outcome::None);
                };
                if let Some(s) = self.typeahead.as_mut()
                    && !c.is_control()
                {
                    if s.is_empty() && c.is_whitespace() {
                        return (Some(0), Outcome::None);
                    }
                    s.push(c);
                    let s = s.clone();
                    self.match_prefix(&s);
                }
                (Some(0), Outcome::None)
            }
            wm::WM_ACTIVATE => {
                if (wp & 0xFFFF) as u32 == wm::WA_INACTIVE {
                    return (None, Outcome::Cancelled);
                }
                (None, Outcome::None)
            }
            _ => (None, Outcome::None),
        }
    }

    /// Holidays dialog type-ahead: move to the first case-insensitive match.
    fn match_prefix(&mut self, search: &str) {
        if search.is_empty() {
            return;
        }
        let folded = search.to_lowercase();
        if let Some(i) = self
            .values
            .iter()
            .position(|v| v.to_lowercase().starts_with(&folded))
        {
            self.selected = i;
            self.see(i);
            os::invalidate(self.hwnd);
        }
    }
}

fn outline(s: &mut Surface, r: Rect, c: u32) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    s.fill_rect(rect(r.left, r.top, w, 1), c);
    s.fill_rect(rect(r.left, r.bottom - 1, w, 1), c);
    s.fill_rect(rect(r.left, r.top, 1, h), c);
    s.fill_rect(rect(r.right - 1, r.top, 1, h), c);
}

/// clam DrawSmoothBorder(border, light, dark) plus the frame-colour fill.
fn smooth(s: &mut Surface, b: Rect) {
    let (x1, x2, y1, y2) = (b.left, b.right - 1, b.top, b.bottom - 1);
    s.fill_rect(
        Rect {
            left: x1 + 2,
            top: y1 + 2,
            right: x2 - 1,
            bottom: y2 - 1,
        },
        rgb(FRAME),
    );
    let bc = rgb(BORDER);
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y1,
            right: x2,
            bottom: y1 + 1,
        },
        bc,
    );
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y2,
            right: x2,
            bottom: y2 + 1,
        },
        bc,
    );
    s.fill_rect(
        Rect {
            left: x1,
            top: y1 + 1,
            right: x1 + 1,
            bottom: y2,
        },
        bc,
    );
    s.fill_rect(
        Rect {
            left: x2,
            top: y1 + 1,
            right: x2 + 1,
            bottom: y2,
        },
        bc,
    );
    let (lc, dc) = (rgb(LIGHT), rgb(DARK));
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y1 + 1,
            right: x2,
            bottom: y1 + 2,
        },
        lc,
    );
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y1 + 1,
            right: x1 + 2,
            bottom: y2,
        },
        lc,
    );
    s.fill_rect(
        Rect {
            left: x1 + 1,
            top: y2 - 1,
            right: x2,
            bottom: y2,
        },
        dc,
    );
    s.fill_rect(
        Rect {
            left: x2 - 1,
            top: y1 + 1,
            right: x2,
            bottom: y2,
        },
        dc,
    );
}
