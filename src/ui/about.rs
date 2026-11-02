//! About WeekNumber (weeknumber_about.py): the app icon, the name, the
//! edition and version, and the Legal notices link. A fixed window centred
//! in the work area of the monitor under the cursor; Escape closes it.
//!
//! The Python window also had a "View on GitHub" button below this row.
//! This edition opens nothing outside the app, so the button is gone and the
//! window ends where the button's slot began: the row sits exactly where it
//! sat before.

use super::dialog::{self, DialogBase};
use super::{Metrics, contains, rect, text_at};
use crate::app::{self, later, with_app};
use crate::gfx::{Filter, Rgba};
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Rect};
use crate::settings::Settings;

/// Client-area metrics in px @ 96 DPI (AboutPopup).
const WIDTH_PX: f64 = 380.0;
const HEIGHT_PX: f64 = 180.0;
const PAD_PX: f64 = 28.0;
const ICON_PX: f64 = 80.0;
const ICON_TEXT_GAP_PX: f64 = 20.0;
const NAME_VERSION_GAP_PX: f64 = 4.0;
const BUTTON_H_PX: f64 = 36.0;
const BUTTON_MARGIN_BOTTOM_PX: f64 = 16.0;

const DOT_TEXT: &str = "  \u{2013}  ";
const LINK_TEXT: &str = "Legal notices";

#[derive(Default, Clone, Copy)]
struct Layout {
    icon: (i32, i32),
    name: (i32, i32),
    version: (i32, i32),
    dot: (i32, i32),
    link: Rect,
}

pub struct AboutDialog {
    base: DialogBase,
    l: Layout,
    /// The icon over the card colour, for the current DPI and theme.
    icon: Option<Rgba>,
    /// The pointer is on the link: it is underlined (GUI improvement; the
    /// Python version shows no underline).
    link_hover: bool,
}

fn version_text() -> String {
    format!("Workplace Edition {}", app::APP_VERSION)
}

/// _load_icon_image: Lanczos to `size`, pasted onto transparency with its
/// own alpha as the mask (which Pillow applies to the alpha too), shown by
/// Tk over the card colour.
fn icon_image(size: usize, bg: crate::gfx::Color) -> Option<Rgba> {
    let src = os::app_icon_image()?;
    let scaled = if (src.w, src.h) == (size, size) {
        src
    } else {
        src.resize(size, size, Filter::Lanczos)
    };
    let mut out = Rgba::new(size, size, [0, 0, 0, 0]);
    out.paste_masked(&scaled, 0, 0, &scaled.alpha());
    Some(out.over_bg(bg))
}

impl AboutDialog {
    pub fn build(settings: &Settings, family: String) -> Option<Box<AboutDialog>> {
        let mut base = DialogBase::new(settings, family, "About");
        base.title = format!("About {}", app::APP_NAME);
        let mut d = Box::new(AboutDialog {
            base,
            l: Layout::default(),
            icon: None,
            link_hover: false,
        });
        d.layout();
        Some(d)
    }

    fn layout(&mut self) {
        let b = &mut self.base;
        let m = &mut b.m;
        let client_w = m.px(WIDTH_PX);
        // The button's slot is gone: the window ends where it began.
        let client_h = m.px(HEIGHT_PX) - m.px(BUTTON_H_PX) - m.px(BUTTON_MARGIN_BOTTOM_PX);
        b.client = (client_w, client_h);
        self.icon = icon_image(m.px(ICON_PX) as usize, b.theme.card_bg);
        let icon_side = self.icon.as_ref().map_or(0, |i| i.h as i32);
        // Labels: text plus Tk's 1 px padding on each side.
        let label = |m: &mut Metrics, size: f64, bold: bool, text: &str| {
            let f = m.font(size, bold);
            (f.measure(text) + 2, f.metrics().linespace() + 2)
        };
        let name = label(m, 20.0, true, app::APP_NAME);
        let version = label(m, 12.0, false, &version_text());
        let dot = label(m, 12.0, false, DOT_TEXT);
        let link = label(m, 12.0, false, LINK_TEXT);
        let version_row_h = version.1.max(dot.1).max(link.1);
        let indent = m.px(2.0);
        let gap = m.px(NAME_VERSION_GAP_PX);
        let text_h = name.1 + gap + version_row_h;
        let text_gap = if self.icon.is_some() {
            m.px(ICON_TEXT_GAP_PX)
        } else {
            0
        };
        let row_h = icon_side.max(text_h);
        // place(x=pad, y=client_h // 2, anchor="w"); pack centres each child.
        let row_x = m.px(PAD_PX);
        let row_y = client_h / 2 - row_h / 2;
        self.l.icon = (row_x, row_y + (row_h - icon_side) / 2);
        let text_x = row_x + icon_side + text_gap;
        let text_y = row_y + (row_h - text_h) / 2;
        self.l.name = (text_x + 1, text_y + 1);
        let vy = text_y + name.1 + gap;
        let vx = text_x + indent;
        self.l.version = (vx + 1, vy + 1);
        self.l.dot = (vx + version.0 + 1, vy + 1);
        self.l.link = rect(vx + version.0 + dot.0, vy, link.0, link.1);
    }

    pub fn hwnd(&self) -> HWND {
        self.base.hwnd
    }

    /// Centre the outer frame in the work area (AboutPopup._center_outer).
    pub fn create_window(&mut self) -> Option<HWND> {
        let work = self.base.mon.work;
        let (ow, oh) = self.base.outer(os::WS_DIALOG);
        let x = work.left + ((work.right - work.left - ow) / 2).max(0);
        let y = work.top + ((work.bottom - work.top - oh) / 2).max(0);
        self.base.create_at(x, y, os::WS_DIALOG)
    }

    pub fn after_create(hwnd: HWND) {
        with_app(|a| {
            let dark = a.settings.dark_mode;
            if let Some(d) = a.about.as_mut().filter(|d| d.base.hwnd == hwnd) {
                d.base.show(dark);
            }
        });
    }

    pub fn destroy(self: Box<Self>) {
        os::destroy_window(self.base.hwnd);
    }

    pub fn retheme(&mut self, settings: &Settings) {
        if self.base.retheme(settings) {
            self.layout();
        }
    }

    /// The client area's top-left on the screen, for the Legal notices
    /// window to cascade from.
    pub fn client_origin(&self) -> (i32, i32) {
        os::client_to_screen(self.base.hwnd, 0, 0)
    }

    fn set_link_hover(&mut self, hover: bool) {
        if hover != self.link_hover {
            self.link_hover = hover;
            os::invalidate(self.base.hwnd);
        }
    }

    fn paint(&mut self) {
        let Some(mut s) = self.base.begin_paint() else {
            return;
        };
        let t = self.base.theme;
        if let Some(icon) = &self.icon {
            s.blit(icon, self.l.icon.0, self.l.icon.1);
        }
        let m = &mut self.base.m;
        text_at(
            &mut s,
            self.l.name.0,
            self.l.name.1,
            app::APP_NAME,
            m.font(20.0, true),
            t.title_fg,
        );
        let f = m.font(12.0, false);
        text_at(
            &mut s,
            self.l.version.0,
            self.l.version.1,
            &version_text(),
            f,
            t.year_fg,
        );
        text_at(&mut s, self.l.dot.0, self.l.dot.1, DOT_TEXT, f, t.year_fg);
        text_at(
            &mut s,
            self.l.link.left + 1,
            self.l.link.top + 1,
            LINK_TEXT,
            f,
            t.accent,
        );
        if self.link_hover {
            let r = self.l.link;
            let y = r.top + 1 + f.metrics().ascent + m.px(1.0);
            s.fill_rect(
                rect(r.left + 1, y, r.right - r.left - 2, m.px(1.0)),
                super::rgb(t.accent),
            );
        }
        self.base.end_paint(s);
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
                later(app::close_about);
                Some(0)
            }
            wm::WM_KEYDOWN => {
                if wp as u16 == os::VK_ESCAPE_ {
                    later(app::close_about);
                }
                Some(0)
            }
            wm::WM_SETCURSOR => {
                if (lp & 0xFFFF) as u32 != wm::HTCLIENT {
                    return None;
                }
                let (cx, cy) = os::cursor_pos();
                let (x, y) = os::screen_to_client(self.base.hwnd, cx, cy);
                os::set_cursor(if contains(self.l.link, x, y) {
                    os::Cursor::Hand
                } else {
                    os::Cursor::Arrow
                });
                Some(1)
            }
            wm::WM_MOUSEMOVE => {
                if !self.base.tracking {
                    os::track_leave(self.base.hwnd);
                    self.base.tracking = true;
                }
                let (x, y) = super::mouse_xy(lp);
                self.set_link_hover(contains(self.l.link, x, y));
                Some(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                self.base.tracking = false;
                self.set_link_hover(false);
                Some(0)
            }
            wm::WM_LBUTTONDOWN => {
                let (x, y) = super::mouse_xy(lp);
                if contains(self.l.link, x, y) {
                    later(app::open_legal);
                }
                Some(0)
            }
            wm::WM_DPICHANGED => {
                let (dpi, suggested) = dialog::dpi_changed(wp, lp);
                let family = self.base.m.family.clone();
                self.base.m = Metrics::new(dpi, family);
                self.base.mon.dpi = dpi;
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
