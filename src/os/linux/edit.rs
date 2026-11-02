//! A single-line edit field (the colour dialog's hex entry), drawn by the
//! backend in its own child window and speaking the Win32 EDIT control's
//! protocol: WM_CTLCOLOREDIT for its colours, WM_COMMAND with EN_CHANGE,
//! EN_SETFOCUS and EN_KILLFOCUS to its parent, and Tab, Return and Escape
//! handed to the top-level window as WM_APP_KEY.

use super::font::{self, Font};
use super::{Lx, WinKind, with};
use crate::os::unix::{
    self, EN_CHANGE_, EN_KILLFOCUS_, EN_SETFOCUS_, HWND, Rect, Surface, core, wm,
};
use std::cell::RefCell;
use std::collections::HashMap;

const MAX_CHARS: usize = 64;
const SELECTION_BG: u32 = 0x3584E4;
const SELECTION_FG: u32 = 0xFFFFFF;

struct Edit {
    parent: HWND,
    id: u16,
    text: Vec<char>,
    caret: usize,
    anchor: usize,
    font: Option<Font>,
    fg: u32,
    bg: u32,
    focused: bool,
    scroll: i32,
}

thread_local! {
    static EDITS: RefCell<HashMap<HWND, Edit>> = RefCell::new(HashMap::new());
}

fn with_edit<R>(h: HWND, f: impl FnOnce(&mut Edit) -> R) -> Option<R> {
    EDITS.with(|e| e.borrow_mut().get_mut(&h).map(f))
}

pub fn is_edit(h: HWND) -> bool {
    EDITS.with(|e| e.borrow().contains_key(&h))
}

pub fn forget(h: HWND) {
    EDITS.with(|e| e.borrow_mut().retain(|k, v| *k != h && v.parent != h));
}

pub fn create_edit(parent: HWND, id: u16, font: &Font, text: &str) -> Option<HWND> {
    let h = core::new_hwnd();
    with(|l: &mut Lx| {
        let pxid = l.windows.get(&parent)?.xid;
        l.create(
            h,
            WinKind::Edit(parent),
            &unix::WindowSpec {
                ex_style: 0,
                style: 0,
                title: "",
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            pxid,
        )?;
        let xid = l.windows.get(&h)?.xid;
        if let Some(w) = l.windows.get_mut(&h) {
            w.mapped = true;
        }
        l.x.map_window(xid);
        Some(())
    })??;
    let text: Vec<char> = text.chars().take(MAX_CHARS).collect();
    let caret = text.len();
    EDITS.with(|e| {
        e.borrow_mut().insert(
            h,
            Edit {
                parent,
                id,
                text,
                caret,
                anchor: caret,
                font: font::from_handle(font.handle()),
                fg: 0,
                bg: 0xFFFFFF,
                focused: false,
                scroll: 0,
            },
        )
    });
    Some(h)
}

pub fn set_font(h: HWND, handle: isize) {
    with_edit(h, |e| e.font = font::from_handle(handle));
    core::invalidate(h);
}

pub fn get_text(h: HWND) -> String {
    with_edit(h, |e| e.text.iter().collect()).unwrap_or_default()
}

pub fn select_all(h: HWND) {
    with_edit(h, |e| {
        e.anchor = 0;
        e.caret = e.text.len();
    });
    core::invalidate(h);
}

pub fn move_child(h: HWND, x: i32, y: i32, w: i32, hgt: i32) {
    super::set_window_pos(h, x, y, w, hgt, None, false, false);
    core::invalidate(h);
}

/// WM_CTLCOLOREDIT's answer: the colours the parent chose (COLORREFs).
pub fn ctl_color(wparam: usize, fg: u32, bg: u32, _brush: &unix::Brush) -> isize {
    let swap = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
    with_edit(HWND(wparam), |e| {
        e.fg = swap(fg);
        e.bg = swap(bg);
    });
    0
}

fn notify(h: HWND, code: u16) {
    if let Some((parent, id)) = with_edit(h, |e| (e.parent, e.id)) {
        core::send(
            parent,
            wm::WM_COMMAND,
            (usize::from(code) << 16) | usize::from(id),
            h.0 as isize,
        );
    }
}

pub fn set_text(h: HWND, text: &str) {
    with_edit(h, |e| {
        e.text = text.chars().take(MAX_CHARS).collect();
        e.caret = e.text.len();
        e.anchor = e.caret;
    });
    core::invalidate(h);
    notify(h, EN_CHANGE_);
}

pub fn focus(h: HWND) {
    let Some(parent) = with_edit(h, |e| e.parent) else {
        return;
    };
    let others: Vec<HWND> = EDITS.with(|e| {
        e.borrow()
            .iter()
            .filter(|(k, v)| **k != h && v.parent == parent && v.focused)
            .map(|(k, _)| *k)
            .collect()
    });
    for o in others {
        blur(o);
    }
    let was = with_edit(h, |e| std::mem::replace(&mut e.focused, true)).unwrap_or(true);
    with(|l| {
        if let Some(xid) = l.windows.get(&h).map(|w| w.xid) {
            l.x.set_input_focus(xid, l.time);
            l.x.flush();
        }
    });
    if !was {
        core::invalidate(h);
        notify(h, EN_SETFOCUS_);
    }
}

pub fn blur(h: HWND) {
    let was = with_edit(h, |e| std::mem::replace(&mut e.focused, false)).unwrap_or(false);
    if was {
        core::invalidate(h);
        notify(h, EN_KILLFOCUS_);
    }
}

pub fn blur_all_in(parent: HWND) {
    let list: Vec<HWND> = EDITS.with(|e| {
        e.borrow()
            .iter()
            .filter(|(_, v)| v.parent == parent && v.focused)
            .map(|(k, _)| *k)
            .collect()
    });
    for h in list {
        blur(h);
    }
}

/// The focused edit inside top-level window `h`, if any.
pub fn focused_in(h: HWND) -> Option<HWND> {
    EDITS.with(|e| {
        e.borrow()
            .iter()
            .find(|(_, v)| v.parent == h && v.focused)
            .map(|(k, _)| *k)
    })
}

impl Edit {
    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn x_of(&self, i: usize) -> i32 {
        let s: String = self.text[..i.min(self.text.len())].iter().collect();
        self.font.as_ref().map_or(0, |f| f.measure(&s))
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.text.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    fn insert(&mut self, s: &str) -> bool {
        let mut changed = self.delete_selection();
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.text.len() >= MAX_CHARS {
                break;
            }
            self.text.insert(self.caret, c);
            self.caret += 1;
            changed = true;
        }
        self.anchor = self.caret;
        changed
    }
}

pub fn click(h: HWND, x: i32) {
    focus(h);
    with_edit(h, |e| {
        let target = x + e.scroll - 1;
        let mut best = (i32::MAX, 0);
        for i in 0..=e.text.len() {
            let d = (e.x_of(i) - target).abs();
            if d < best.0 {
                best = (d, i);
            }
        }
        e.caret = best.1;
        e.anchor = best.1;
    });
    core::invalidate(h);
}

pub fn paste(h: HWND, text: &str) {
    let line = text.lines().next().unwrap_or("").trim().to_string();
    if with_edit(h, |e| e.insert(&line)).unwrap_or(false) {
        core::invalidate(h);
        notify(h, EN_CHANGE_);
    }
}

/// A key for the focused edit; true when it was handled here.
pub fn on_key(h: HWND, vk: u16, ch: Option<char>, shift: bool, ctrl: bool) -> bool {
    if matches!(vk, 0x09 | 0x0D | 0x1B) {
        if let Some(parent) = with_edit(h, |e| e.parent) {
            core::post(parent, unix::WM_APP_KEY, usize::from(vk), 0);
        }
        return true;
    }
    if ctrl {
        match vk {
            0x41 => select_all(h),
            0x43 | 0x58 => {
                let text: Option<String> = with_edit(h, |e| {
                    let (a, b) = e.selection();
                    (a < b).then(|| e.text[a..b].iter().collect())
                })
                .flatten();
                if let Some(t) = text {
                    super::set_clipboard_text(h, &t);
                    if vk == 0x58 && with_edit(h, Edit::delete_selection).unwrap_or(false) {
                        core::invalidate(h);
                        notify(h, EN_CHANGE_);
                    }
                }
            }
            0x56 => super::paste_request(h),
            _ => {}
        }
        return true;
    }
    let changed = with_edit(h, |e| {
        let n = e.text.len();
        let moved = |e: &mut Edit, to: usize| {
            e.caret = to.min(n);
            if !shift {
                e.anchor = e.caret;
            }
        };
        match vk {
            0x25 => {
                let (a, b) = e.selection();
                let to = if a != b && !shift {
                    a
                } else {
                    e.caret.saturating_sub(1)
                };
                moved(e, to);
                false
            }
            0x27 => {
                let (a, b) = e.selection();
                let to = if a != b && !shift { b } else { e.caret + 1 };
                moved(e, to);
                false
            }
            0x24 => {
                moved(e, 0);
                false
            }
            0x23 => {
                moved(e, n);
                false
            }
            0x08 => {
                if !e.delete_selection() && e.caret > 0 {
                    e.caret -= 1;
                    e.text.remove(e.caret);
                    e.anchor = e.caret;
                    true
                } else {
                    true
                }
            }
            0x2E => {
                if !e.delete_selection() && e.caret < n {
                    e.text.remove(e.caret);
                    true
                } else {
                    true
                }
            }
            _ => match ch {
                Some(c) if !c.is_control() => e.insert(&c.to_string()),
                _ => false,
            },
        }
    });
    core::invalidate(h);
    if changed == Some(true) {
        notify(h, EN_CHANGE_);
    }
    true
}

/// Messages for an edit window (painting; the rest is input above).
pub fn on_message(h: HWND, msg: u32, _wp: usize, _lp: isize) -> Option<isize> {
    if msg != wm::WM_PAINT {
        return Some(0);
    }
    if let Some(parent) = with_edit(h, |e| e.parent) {
        core::send(parent, wm::WM_CTLCOLOREDIT, h.0, h.0 as isize);
    }
    let (w, hgt) = super::client_size(h);
    let Some(mut s) = Surface::new(w.max(1) as usize, hgt.max(1) as usize) else {
        return Some(0);
    };
    with_edit(h, |e| {
        s.fill_rect(
            Rect {
                left: 0,
                top: 0,
                right: w,
                bottom: hgt,
            },
            e.bg,
        );
        let Some(f) = e.font.as_ref() else { return };
        let ls = f.metrics().linespace();
        let y = (hgt - ls) / 2;
        // Keep the caret in view.
        let cx = e.x_of(e.caret);
        if cx - e.scroll > w - 2 {
            e.scroll = cx - (w - 2);
        } else if cx - e.scroll < 0 {
            e.scroll = cx;
        }
        let x0 = 1 - e.scroll;
        let (a, b) = e.selection();
        let text: String = e.text.iter().collect();
        let rgb = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
        s.dc().text(f, x0, y, &text, rgb(e.fg));
        if a < b && e.focused {
            let (xa, xb) = (x0 + e.x_of(a), x0 + e.x_of(b));
            s.fill_rect(
                Rect {
                    left: xa,
                    top: y,
                    right: xb,
                    bottom: y + ls,
                },
                SELECTION_BG,
            );
            let sel: String = e.text[a..b].iter().collect();
            s.dc().text(f, xa, y, &sel, rgb(SELECTION_FG));
        }
        if e.focused {
            let x = x0 + cx;
            s.fill_rect(
                Rect {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + ls,
                },
                e.fg,
            );
        }
    });
    super::paint(h, |dc| s.present(dc));
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str) -> Edit {
        let t: Vec<char> = text.chars().collect();
        Edit {
            parent: HWND(1),
            id: 1,
            caret: t.len(),
            anchor: t.len(),
            text: t,
            font: None,
            fg: 0,
            bg: 0,
            focused: true,
            scroll: 0,
        }
    }

    #[test]
    fn editing() {
        let mut e = edit("#12");
        assert!(e.insert("34"));
        assert_eq!(e.text.iter().collect::<String>(), "#1234");
        e.anchor = 1;
        e.caret = 3;
        assert!(e.insert("A"));
        assert_eq!(e.text.iter().collect::<String>(), "#A34");
        assert!(!e.delete_selection());
        let long = "x".repeat(100);
        e.insert(&long);
        assert_eq!(e.text.len(), MAX_CHARS);
        assert!(!e.insert("\n\t"));
    }
}
