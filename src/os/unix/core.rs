//! The Win32 message model, for the macOS and Linux backends: one handler
//! for every window, messages sent synchronously or posted to a queue,
//! per-window timers, coalesced repaints (WM_PAINT once per invalidation)
//! and WM_QUIT. The backends turn native events into these messages; the
//! app's windows cannot tell the difference.
//!
//! Nothing here holds a borrow while the handler runs: the handler may call
//! straight back into the os layer.

use super::{HWND, wm};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub type Handler = fn(HWND, u32, usize, isize) -> Option<isize>;

struct Timer {
    hwnd: HWND,
    id: usize,
    period: Duration,
    due: Instant,
}

struct Core {
    handler: Option<Handler>,
    /// The backend's own windows (edit fields, message boxes) answer first.
    filter: Option<Handler>,
    next_hwnd: usize,
    posted: VecDeque<(HWND, u32, usize, isize)>,
    dirty: Vec<HWND>,
    timers: Vec<Timer>,
    tracking: Vec<HWND>,
    quit: Option<i32>,
    shift: bool,
    ctrl: bool,
}

thread_local! {
    static CORE: RefCell<Core> = const {
        RefCell::new(Core {
            handler: None,
            filter: None,
            next_hwnd: 0x10,
            posted: VecDeque::new(),
            dirty: Vec::new(),
            timers: Vec::new(),
            tracking: Vec::new(),
            quit: None,
            shift: false,
            ctrl: false,
        })
    };
}

fn with<R>(f: impl FnOnce(&mut Core) -> R) -> R {
    CORE.with(|c| f(&mut c.borrow_mut()))
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Milliseconds on a monotonic clock (GetTickCount64).
pub fn tick_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn register_class(handler: Handler) -> bool {
    START.get_or_init(Instant::now);
    with(|c| c.handler = Some(handler));
    true
}

/// A fresh, never reused window handle.
pub fn new_hwnd() -> HWND {
    with(|c| {
        c.next_hwnd += 1;
        HWND(c.next_hwnd)
    })
}

/// Let the backend see every message before the app does; a Some answer
/// stops it there.
pub fn set_filter(filter: Handler) {
    with(|c| c.filter = Some(filter));
}

/// SendMessage: run the handler now and return its answer.
pub fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
    let (filter, handler) = with(|c| (c.filter, c.handler));
    if let Some(f) = filter
        && let Some(r) = f(h, msg, wp, lp)
    {
        return Some(r);
    }
    handler?(h, msg, wp, lp)
}

/// PostMessage: queue for the next turn of the loop.
pub fn post(h: HWND, msg: u32, wp: usize, lp: isize) {
    with(|c| c.posted.push_back((h, msg, wp, lp)));
}

pub fn post_quit() {
    with(|c| c.quit = Some(0));
}

pub fn quit_requested() -> Option<i32> {
    with(|c| c.quit)
}

pub fn invalidate(h: HWND) {
    with(|c| {
        if !c.dirty.contains(&h) {
            c.dirty.push(h);
        }
    });
}

/// The window was painted outside the loop (an expose with a fresh
/// buffer): no further WM_PAINT is owed.
pub fn validate(h: HWND) {
    with(|c| c.dirty.retain(|&d| d != h));
}

pub fn set_timer(h: HWND, id: usize, ms: u32) {
    let period = Duration::from_millis(u64::from(ms.max(10)));
    with(|c| {
        c.timers.retain(|t| !(t.hwnd == h && t.id == id));
        c.timers.push(Timer {
            hwnd: h,
            id,
            period,
            due: Instant::now() + period,
        });
    });
}

pub fn kill_timer(h: HWND, id: usize) {
    with(|c| c.timers.retain(|t| !(t.hwnd == h && t.id == id)));
}

/// TrackMouseEvent(TME_LEAVE): one WM_MOUSELEAVE when the pointer leaves.
pub fn track_leave(h: HWND) {
    with(|c| {
        if !c.tracking.contains(&h) {
            c.tracking.push(h);
        }
    });
}

/// The pointer left `h`: WM_MOUSELEAVE if it was asked for.
pub fn mouse_left(h: HWND) {
    let tracked = with(|c| {
        let had = c.tracking.contains(&h);
        c.tracking.retain(|&t| t != h);
        had
    });
    if tracked {
        send(h, wm::WM_MOUSELEAVE, 0, 0);
    }
}

/// Everything queued for a destroyed window is dropped.
pub fn forget(h: HWND) {
    with(|c| {
        c.posted.retain(|p| p.0 != h);
        c.dirty.retain(|&d| d != h);
        c.timers.retain(|t| t.hwnd != h);
        c.tracking.retain(|&t| t != h);
    });
}

pub fn set_modifiers(shift: bool, ctrl: bool) {
    with(|c| {
        c.shift = shift;
        c.ctrl = ctrl;
    });
}

/// GetKeyState for Shift and Ctrl, from the last input event.
pub fn key_down(vk: u16) -> bool {
    with(|c| match vk {
        super::VK_SHIFT_ => c.shift,
        super::VK_CONTROL_ => c.ctrl,
        _ => false,
    })
}

/// How long the loop may sleep before a timer is due (None: no timer).
pub fn next_timeout() -> Option<Duration> {
    with(|c| {
        c.timers
            .iter()
            .map(|t| t.due.saturating_duration_since(Instant::now()))
            .min()
    })
}

/// One turn of work: due timers, posted messages, then repaints, until
/// nothing is left. Returns early once WM_QUIT is posted.
pub fn pump() {
    for _ in 0..64 {
        if quit_requested().is_some() {
            return;
        }
        let now = Instant::now();
        let due: Vec<(HWND, usize)> = with(|c| {
            let mut due = Vec::new();
            for t in &mut c.timers {
                if t.due <= now {
                    due.push((t.hwnd, t.id));
                    t.due = now + t.period;
                }
            }
            due
        });
        for (h, id) in &due {
            send(*h, wm::WM_TIMER, *id, 0);
        }
        let posted: Vec<(HWND, u32, usize, isize)> = with(|c| c.posted.drain(..).collect());
        for (h, msg, wp, lp) in &posted {
            send(*h, *msg, *wp, *lp);
        }
        let dirty: Vec<HWND> = with(|c| std::mem::take(&mut c.dirty));
        for h in &dirty {
            send(*h, wm::WM_PAINT, 0, 0);
        }
        if due.is_empty() && posted.is_empty() && dirty.is_empty() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell as Cell;

    thread_local! {
        static LOG: Cell<Vec<(usize, u32, usize)>> = const { Cell::new(Vec::new()) };
    }

    fn record(h: HWND, msg: u32, wp: usize, _lp: isize) -> Option<isize> {
        LOG.with(|l| l.borrow_mut().push((h.0, msg, wp)));
        if msg == wm::WM_PAINT {
            // Painting while painting must not loop forever.
            invalidate(h);
            validate(h);
        }
        Some(0)
    }

    #[test]
    fn messages_timers_and_paints() {
        register_class(record);
        let a = new_hwnd();
        let b = new_hwnd();
        assert_ne!(a, b);
        post(a, wm::WM_CLOSE, 7, 0);
        invalidate(b);
        invalidate(b);
        set_timer(a, 3, 10);
        assert!(next_timeout().is_some_and(|d| d <= Duration::from_millis(10)));
        pump();
        let log = LOG.with(|l| l.borrow_mut().drain(..).collect::<Vec<_>>());
        assert_eq!(log, vec![(a.0, wm::WM_CLOSE, 7), (b.0, wm::WM_PAINT, 0)]);
        std::thread::sleep(Duration::from_millis(15));
        pump();
        let log = LOG.with(|l| l.borrow_mut().drain(..).collect::<Vec<_>>());
        assert_eq!(log, vec![(a.0, wm::WM_TIMER, 3)]);
        forget(a);
        assert!(next_timeout().is_none());
        track_leave(b);
        mouse_left(b);
        mouse_left(b);
        let log = LOG.with(|l| l.borrow_mut().drain(..).collect::<Vec<_>>());
        assert_eq!(log, vec![(b.0, wm::WM_MOUSELEAVE, 0)]);
        set_modifiers(true, false);
        assert!(key_down(super::super::VK_SHIFT_) && !key_down(super::super::VK_CONTROL_));
        post_quit();
        assert_eq!(quit_requested(), Some(0));
    }
}
