//! MessageBox for Linux: a small titled window with the message and an OK
//! button (or a choice of two), drawn by the backend, shown modally (other
//! windows keep working; the call returns when it is closed).

use super::{Font, Lx, WinKind, with};
use crate::os::unix::{self, HWND, Rect, Surface, core, wm};
use crate::theme::LIGHT;
use std::cell::RefCell;
use std::collections::HashMap;

struct MsgBox {
    lines: Vec<String>,
    font: Font,
    bold: Font,
    /// The buttons, the default (accent) one first.
    buttons: Vec<(Rect, String)>,
    pad: i32,
    hover: Option<usize>,
    size: (i32, i32),
}

thread_local! {
    static BOXES: RefCell<HashMap<HWND, MsgBox>> = RefCell::new(HashMap::new());
    /// The button that closed each box.
    static CHOSEN: RefCell<HashMap<HWND, usize>> = RefCell::new(HashMap::new());
}

pub fn owns(h: HWND) -> bool {
    BOXES.with(|b| b.borrow().contains_key(&h))
}

/// Greedy word wrap to `width` pixels.
fn wrap(font: &Font, text: &str, width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if font.measure(&candidate) > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

pub fn show(text: &str) {
    choose(text, &["OK"]);
}

/// Show `text` with `buttons` (the first is the default) and return the
/// index of the one pressed; None when the box was closed otherwise.
pub fn choose(text: &str, buttons: &[&str]) -> Option<usize> {
    if with(|_| ()).is_none() {
        eprintln!("WeekNumber: {text}");
        return None;
    }
    let mon = super::cursor_monitor();
    let s = f64::from(mon.dpi) / 96.0;
    let px = |v: f64| (v * s).round() as i32;
    let Some(font) = Font::new("", px(12.0), false, unix::Quality::Default) else {
        eprintln!("WeekNumber: {text}");
        return None;
    };
    let bold = Font::new("", px(12.0), true, unix::Quality::Default)?;
    let pad = px(20.0);
    let lines = wrap(&font, text, px(380.0));
    let ls = font.metrics().linespace();
    let text_w = lines.iter().map(|l| font.measure(l)).max().unwrap_or(0);
    let bh = px(30.0);
    let widths: Vec<i32> = buttons
        .iter()
        .map(|b| (bold.measure(b) + px(32.0)).max(px(88.0)))
        .collect();
    let gap = px(8.0);
    let row_w: i32 = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1).max(0);
    let w = (text_w + 2 * pad).max(row_w + 2 * pad).max(px(300.0));
    let h = pad + ls * lines.len() as i32 + pad + bh + pad;
    // Right-aligned, the default button rightmost.
    let mut right = w - pad;
    let mut rects = Vec::new();
    for (label, bw) in buttons.iter().zip(&widths) {
        rects.push((
            Rect {
                left: right - bw,
                top: h - pad - bh,
                right,
                bottom: h - pad,
            },
            (*label).to_string(),
        ));
        right -= bw + gap;
    }
    let x = mon.work.left + (mon.work.right - mon.work.left - w) / 2;
    let y = mon.work.top + (mon.work.bottom - mon.work.top - h) / 3;
    let hwnd = core::new_hwnd();
    let created = with(|l: &mut Lx| {
        l.create(
            hwnd,
            WinKind::MessageBox,
            &unix::WindowSpec {
                ex_style: 0,
                style: unix::WS_DIALOG,
                title: "WeekNumber",
                x,
                y,
                w,
                h,
            },
            0,
        )
    })
    .flatten();
    if created.is_none() {
        eprintln!("WeekNumber: {text}");
        return None;
    }
    BOXES.with(|b| {
        b.borrow_mut().insert(
            hwnd,
            MsgBox {
                lines,
                font,
                bold,
                buttons: rects,
                pad,
                hover: None,
                size: (w, h),
            },
        )
    });
    super::show_window(hwnd, true);
    super::set_foreground(hwnd);
    core::invalidate(hwnd);
    // Modal: run the loop until the box is closed.
    while owns(hwnd) && core::quit_requested().is_none() {
        core::pump();
        let fd = with(|l| {
            l.x.flush();
            l.x.fd()
        });
        let Some(fd) = fd else { break };
        let pending = with(|l| l.x.has_events()).unwrap_or(false);
        if !pending {
            let t = core::next_timeout().map_or(1000, |d| d.as_millis().min(1000) as i32);
            unix::sys::wait_readable(&[fd], t);
        }
        if with(|l| {
            l.x.read_input(false);
            l.x.dead
        }) != Some(false)
        {
            break;
        }
        while let Some(e) = with(|l| l.x.next_event()).flatten() {
            super::on_x_event(e);
        }
    }
    CHOSEN.with(|c| c.borrow_mut().remove(&hwnd))
}

fn close(h: HWND) {
    BOXES.with(|b| b.borrow_mut().remove(&h));
    super::destroy_window(h);
}

pub fn on_message(h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
    match msg {
        wm::WM_PAINT => {
            paint(h);
            Some(0)
        }
        wm::WM_CLOSE => {
            close(h);
            Some(0)
        }
        wm::WM_KEYDOWN => {
            match wp as u16 {
                // Enter and Space press the default button; Escape cancels.
                0x0D | 0x20 => {
                    CHOSEN.with(|c| c.borrow_mut().insert(h, 0));
                    close(h);
                }
                0x1B => close(h),
                _ => {}
            }
            Some(0)
        }
        wm::WM_MOUSEMOVE | wm::WM_LBUTTONUP => {
            let (x, y) = crate::ui::mouse_xy(lp);
            let hit = BOXES.with(|b| {
                b.borrow_mut().get_mut(&h).map(|m| {
                    let at = m.buttons.iter().position(|(r, _)| {
                        x >= r.left && x < r.right && y >= r.top && y < r.bottom
                    });
                    let changed = at != m.hover;
                    m.hover = at;
                    (at, changed)
                })
            });
            match (msg, hit) {
                (wm::WM_LBUTTONUP, Some((Some(i), _))) => {
                    CHOSEN.with(|c| c.borrow_mut().insert(h, i));
                    close(h);
                }
                (_, Some((_, true))) => core::invalidate(h),
                _ => {}
            }
            Some(0)
        }
        wm::WM_ACTIVATE | wm::WM_SETCURSOR | wm::WM_DESTROY | wm::WM_SIZE => Some(0),
        _ => Some(0),
    }
}

fn paint(h: HWND) {
    let surface = BOXES.with(|b| {
        let b = b.borrow();
        let m = b.get(&h)?;
        let (w, hgt) = m.size;
        let mut s = Surface::new(w as usize, hgt as usize)?;
        let rgb =
            |c: crate::gfx::Color| (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2);
        let cref = |c: crate::gfx::Color| c.colorref();
        let t = &LIGHT;
        s.fill_rect(
            Rect {
                left: 0,
                top: 0,
                right: w,
                bottom: hgt,
            },
            rgb(t.card_bg),
        );
        let ls = m.font.metrics().linespace();
        for (i, line) in m.lines.iter().enumerate() {
            s.dc().text(
                &m.font,
                m.pad,
                m.pad + i as i32 * ls,
                line,
                cref(t.title_fg),
            );
        }
        for (i, (r, label)) in m.buttons.iter().enumerate() {
            // The default button in the accent colour, others as chips.
            let (base, fg) = if i == 0 {
                (t.accent, t.today_fg)
            } else {
                (t.hover_bg, t.title_fg)
            };
            let fill = if m.hover == Some(i) {
                crate::gfx::shade(base, 0.92)
            } else {
                base
            };
            let (bw, bh) = (r.right - r.left, r.bottom - r.top);
            let pill = crate::ui::rounded(bw, bh, f64::from(bh) / 2.0, fill, t.card_bg, None, 0);
            s.blit(&pill, r.left, r.top);
            let tw = m.bold.measure(label);
            let bls = m.bold.metrics().linespace();
            s.dc().text(
                &m.bold,
                r.left + (bw - tw) / 2,
                r.top + (bh - bls) / 2,
                label,
                cref(fg),
            );
        }
        Some(s)
    });
    if let Some(s) = surface {
        super::paint(h, |dc| s.present(dc));
    }
}
