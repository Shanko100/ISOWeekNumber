//! Application state and lifecycle: single instance with hand-over, the
//! tray icon and its menu, and the windows, each created when opened and
//! freed when closed.
//!
//! Message handling never calls back into Windows in a way that re-enters
//! the window procedure while the state is borrowed: anything that creates,
//! shows, moves, focuses or destroys a window, or runs a modal loop, is
//! queued with `later()` and runs after the handler has returned.

use crate::dates::Date;
use crate::gfx::Color;
use crate::os::HWND;
use crate::os::wm;
use crate::os::{self, Icon, TrayOp};
use crate::settings::Settings;
use crate::ui::menu::{self, Menu};
use crate::ui::{
    about::AboutDialog, calendar::Calendar, color_dialog::ColorDialog,
    holiday_dialog::HolidayDialog, legal_dialog::LegalDialog, tz_dialog::TzDialog,
};
use crate::{holidays, startup, tray};
use std::cell::RefCell;

pub const APP_NAME: &str = "WeekNumber";
pub const APP_VERSION: &str = "1.0";

/// A click on the tray icon takes the focus first, which closes the open
/// flyout; a click this soon after such a close is that click.
const TOGGLE_GRACE_MS: u64 = 400;

const TIMER_MIDNIGHT: usize = 1;

// Tray menu command ids.
const CMD_STARTUP: u32 = 1;
const CMD_BOLD: u32 = 2;
const CMD_COLOR: u32 = 3;
const CMD_DARK: u32 = 4;
const CMD_HOLIDAYS: u32 = 5;
const CMD_TIMEZONE: u32 = 6;
const CMD_ABOUT: u32 = 7;
const CMD_EXIT: u32 = 8;
#[cfg(target_os = "linux")]
const CMD_INSTALL: u32 = 9;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static LATER: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    static RUNNING_LATER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Queue `f` to run once the current message has been handled.
pub fn later(f: impl FnOnce() + 'static) {
    LATER.with(|q| q.borrow_mut().push(Box::new(f)));
}

/// Run `f` on the app state, unless it is already borrowed.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut a| a.as_mut().map(f))
    })
}

fn run_later() {
    // A message sent while a handler still holds the state (ReleaseCapture
    // sends WM_CAPTURECHANGED, for one) must not run the queue: its work
    // could not reach the state and would be lost. The outer dispatch runs
    // it once the handler has returned.
    if APP.with(|cell| cell.try_borrow_mut().is_err()) {
        return;
    }
    if RUNNING_LATER.with(|r| r.replace(true)) {
        return;
    }
    loop {
        let next = LATER.with(|q| {
            let mut q = q.borrow_mut();
            if q.is_empty() {
                None
            } else {
                Some(q.remove(0))
            }
        });
        match next {
            Some(f) => f(),
            None => break,
        }
    }
    RUNNING_LATER.with(|r| r.set(false));
}

fn dispatch(h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
    let r = with_app(|app| app.on_message(h, msg, wp, lp)).flatten();
    run_later();
    r
}

pub struct App {
    pub settings: Settings,
    tray_hwnd: HWND,
    taskbar_created: u32,
    icon: Option<Icon>,
    icon_size: usize,
    icon_key: Option<(u32, String, bool)>,
    week: u32,
    updated: os::WallTime,
    pub calendar: Option<Box<Calendar>>,
    pub holiday: Option<Box<HolidayDialog>>,
    pub color: Option<Box<ColorDialog>>,
    pub tz: Option<Box<TzDialog>>,
    pub about: Option<Box<AboutDialog>>,
    pub legal: Option<Box<LegalDialog>>,
    pub menu: Option<Box<Menu>>,
    /// When the flyout last closed because it lost the focus (ms).
    calendar_dismissed_at: Option<u64>,
    family: Option<String>,
}

pub fn today() -> Date {
    let now = os::local_now();
    Date::new(now.year, now.month, now.day).unwrap_or(Date {
        year: 2000,
        month: 1,
        day: 1,
    })
}

pub fn run() -> i32 {
    if !os::register_class(dispatch) {
        return 1;
    }
    let Some(instance) = os::Instance::claim() else {
        return 0;
    };

    let mut settings = Settings::load(os::settings_dir());
    let codes = holidays::country_codes();
    settings.validate_holiday_countries(&|c| codes.iter().any(|x| x == c));
    if !settings.startup_initialized {
        // A genuine first run starts with autostart off; a settings file
        // that exists but can't be read must not wipe the user's choice.
        if !settings.file_existed {
            startup::disable_on_first_run();
        }
        settings.mark_startup_initialized();
    }
    startup::refresh_at_launch();
    // Linux: started from Downloads (a double-clicked AppImage)? Offer to
    // install; on yes the installed program takes over this process.
    #[cfg(target_os = "linux")]
    crate::install::offer_at_launch();

    let Some(tray_hwnd) = os::create_hidden_window() else {
        return 1;
    };
    let taskbar_created = os::register_window_message("TaskbarCreated");
    os::allow_message(tray_hwnd, taskbar_created);
    let mut app = App {
        settings,
        tray_hwnd,
        taskbar_created,
        icon: None,
        icon_size: tray::tray_icon_size(),
        icon_key: None,
        week: today().iso_week().1,
        updated: os::local_now(),
        calendar: None,
        holiday: None,
        color: None,
        tz: None,
        about: None,
        legal: None,
        menu: None,
        calendar_dismissed_at: None,
        family: None,
    };
    app.refresh_icon();
    os::tray_icon(tray_hwnd, TrayOp::Add, app.icon.as_ref(), &app.tooltip());
    later(publish_tray_menu);
    // For the test workflows, which cannot always click a tray icon:
    // WEEKNUMBER_OPEN names a window to open right away.
    if let Some(open) = std::env::var("WEEKNUMBER_OPEN")
        .ok()
        .and_then(|w| test_opener(&w))
    {
        later(open);
    }
    app.arm_midnight();
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    // Run what start-up queued (publishing the menu, the smoke tests'
    // calendar) now rather than after the first message.
    run_later();

    let code = os::run_message_loop(&instance, || {
        // A newly launched copy asked us to hand over.
        later(shutdown);
        run_later();
    });
    APP.with(|cell| {
        if let Ok(mut a) = cell.try_borrow_mut() {
            *a = None;
        }
    });
    code
}

/// The window WEEKNUMBER_OPEN names.
fn test_opener(name: &str) -> Option<fn()> {
    Some(match name {
        "calendar" => open_calendar,
        "menu" => show_tray_menu,
        "color" => open_color,
        "holidays" => open_holiday,
        "timezone" => open_tz,
        "about" => open_about,
        "legal" => open_legal,
        _ => return None,
    })
}

/// Close every window, remove the icon, end the message loop.
pub fn shutdown() {
    for close in [
        close_calendar_now as fn(),
        close_holiday,
        close_color,
        close_tz,
        close_legal,
        close_about,
        close_menu,
    ] {
        close();
    }
    with_app(|a| {
        os::tray_icon(a.tray_hwnd, TrayOp::Delete, None, "");
        os::kill_timer(a.tray_hwnd, TIMER_MIDNIGHT);
    });
    let h = with_app(|a| a.tray_hwnd);
    if let Some(h) = h {
        os::destroy_window(h);
    }
    os::post_quit();
}

impl App {
    pub fn family(&mut self) -> String {
        self.family
            .get_or_insert_with(crate::ui::ui_font_family)
            .clone()
    }

    fn tooltip(&self) -> String {
        let u = self.updated;
        let text = format!(
            "Week {} \u{b7} updated {:02}-{:02}-{} {:02}:{:02}",
            self.week, u.day, u.month, u.year, u.hour, u.minute
        );
        text.chars().take(63).collect()
    }

    fn refresh_week(&mut self) {
        self.updated = os::local_now();
        self.week = today().iso_week().1;
    }

    /// Re-render the icon only when its inputs changed; update the tooltip.
    fn refresh_icon(&mut self) {
        let key = (
            self.week,
            self.settings.selected_color.clone(),
            self.settings.is_bold,
        );
        if self.icon_key.as_ref() != Some(&key)
            && let Some(icon) =
                tray::make_icon(self.week, Color::hex(&key.1), key.2, self.icon_size)
        {
            self.icon = Some(icon);
            self.icon_key = Some(key);
        }
    }

    fn refresh_tray(&mut self) {
        self.refresh_week();
        self.refresh_icon();
        os::tray_icon(
            self.tray_hwnd,
            TrayOp::Modify,
            self.icon.as_ref(),
            &self.tooltip(),
        );
    }

    /// Wake at the next local midnight: the only time the week can change.
    fn arm_midnight(&mut self) {
        let now = os::local_now();
        let elapsed = (now.hour * 3600 + now.minute * 60 + now.second) * 1000 + now.millis;
        let remaining = 86_400_000u32.saturating_sub(elapsed) + 1000;
        os::set_timer(self.tray_hwnd, TIMER_MIDNIGHT, remaining);
    }

    fn on_message(&mut self, h: HWND, msg: u32, wp: usize, lp: isize) -> Option<isize> {
        if h == self.tray_hwnd {
            return self.on_tray_message(msg, wp, lp);
        }
        let settings = &mut self.settings;
        if let Some(c) = self.calendar.as_mut().filter(|c| c.hwnd == h) {
            return c.on_message(settings, msg, wp, lp);
        }
        if let Some(c) = self.holiday.as_mut().filter(|c| c.hwnd() == h) {
            return c.on_message(settings, msg, wp, lp);
        }
        if let Some(c) = self.color.as_mut().filter(|c| c.hwnd() == h) {
            return c.on_message(settings, msg, wp, lp);
        }
        if let Some(c) = self.menu.as_mut().filter(|c| c.hwnd == h) {
            return c.on_message(msg, wp, lp);
        }
        if let Some(c) = self.about.as_mut().filter(|c| c.hwnd() == h) {
            return c.on_message(settings, msg, wp, lp);
        }
        if let Some(c) = self.legal.as_mut().filter(|c| c.hwnd() == h) {
            return c.on_message(settings, msg, wp, lp);
        }
        if let Some(c) = self.tz.as_mut()
            && c.owns(h)
        {
            return c.on_message(settings, h, msg, wp, lp);
        }
        if let Some(c) = self.holiday.as_mut()
            && c.owns(h)
        {
            return c.on_popup_message(h, msg, wp, lp);
        }
        if let Some(c) = self.calendar.as_mut()
            && c.owns_tooltip(h)
        {
            return c.on_tooltip_message(msg, wp, lp);
        }
        None
    }

    fn on_tray_message(&mut self, msg: u32, wp: usize, lp: isize) -> Option<isize> {
        match msg {
            os::WM_TRAY => {
                match (lp & 0xFFFF) as u32 {
                    wm::WM_LBUTTONUP => {
                        self.refresh_tray();
                        later(toggle_calendar);
                    }
                    wm::WM_RBUTTONUP => later(show_tray_menu),
                    // The pointer is on the icon: bring week and tooltip up
                    // to date before the shell shows the tip.
                    wm::WM_MOUSEMOVE => self.refresh_tray(),
                    // A command from a menu the panel or the system draws
                    // (Linux D-Bus menus, the macOS status menu).
                    wm::WM_COMMAND => {
                        let id = wp as u32;
                        later(move || run_tray_command(id));
                    }
                    // That menu is about to open: bring its check marks up to date.
                    TRAY_MENU_REFRESH => later(publish_tray_menu),
                    // The Linux backend found (or changed) the tray: render
                    // the icon for it.
                    TRAY_ICON_REFRESH => {
                        self.icon_key = None;
                        self.refresh_tray();
                    }
                    _ => {}
                }
                Some(0)
            }
            wm::WM_TIMER if wp == TIMER_MIDNIGHT => {
                self.refresh_tray();
                self.arm_midnight();
                Some(0)
            }
            wm::WM_TIMECHANGE => {
                self.refresh_tray();
                self.arm_midnight();
                None
            }
            wm::WM_POWERBROADCAST => {
                if wp == wm::PBT_APMRESUMEAUTOMATIC as usize {
                    self.refresh_tray();
                    self.arm_midnight();
                }
                None
            }
            wm::WM_DISPLAYCHANGE => {
                // The icon size may have changed with the display.
                self.icon_size = tray::tray_icon_size();
                self.icon_key = None;
                self.refresh_icon();
                os::tray_icon(self.tray_hwnd, TrayOp::Delete, None, "");
                os::tray_icon(
                    self.tray_hwnd,
                    TrayOp::Add,
                    self.icon.as_ref(),
                    &self.tooltip(),
                );
                None
            }
            m if m == self.taskbar_created && m != 0 => {
                os::tray_icon(
                    self.tray_hwnd,
                    TrayOp::Add,
                    self.icon.as_ref(),
                    &self.tooltip(),
                );
                Some(0)
            }
            _ => None,
        }
    }

    /// Repaint every open window for the active theme.
    pub fn retheme(&mut self) {
        let settings = &self.settings;
        if let Some(c) = self.calendar.as_mut() {
            c.retheme(settings);
        }
        if let Some(c) = self.holiday.as_mut() {
            c.retheme(settings);
        }
        if let Some(c) = self.color.as_mut() {
            c.retheme(settings);
        }
        if let Some(c) = self.tz.as_mut() {
            c.retheme(settings);
        }
        if let Some(c) = self.about.as_mut() {
            c.retheme(settings);
        }
        if let Some(c) = self.legal.as_mut() {
            c.retheme(settings);
        }
    }
}

// ---------------------------------------------------------------------------
// Tray menu
// ---------------------------------------------------------------------------

/// WM_TRAY lparam from the macOS and Linux backends: the menu they draw
/// natively is about to open.
const TRAY_MENU_REFRESH: u32 = 0x7F00;
/// WM_TRAY lparam from the Linux backend: the icon was rendered for another
/// kind of tray than the one now hosting it.
const TRAY_ICON_REFRESH: u32 = 0x7F01;

/// The tray menu: (command, label, check state for the toggles); command 0
/// is a separator.
fn tray_menu_items() -> Vec<(u32, &'static str, Option<bool>)> {
    let (bold, dark) =
        with_app(|a| (a.settings.is_bold, a.settings.dark_mode)).unwrap_or((false, false));
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut items = vec![
        (CMD_STARTUP, "Run at Startup", Some(startup::is_enabled())),
        (CMD_BOLD, "Bold Text", Some(bold)),
        (CMD_COLOR, "Change Color\u{2026}", None),
        (CMD_DARK, "Dark Mode", Some(dark)),
        (CMD_HOLIDAYS, "Public Holidays\u{2026}", None),
        (CMD_TIMEZONE, "Time Zone\u{2026}", None),
        (0, "", None),
        (CMD_ABOUT, "About\u{2026}", None),
        (0, "", None),
        (CMD_EXIT, EXIT_LABEL, None),
    ];
    // Linux: a copy started from Downloads can install itself.
    #[cfg(target_os = "linux")]
    if crate::install::worth_installing() {
        items.splice(
            0..0,
            [
                (CMD_INSTALL, "Install WeekNumber\u{2026}", None),
                (0, "", None),
            ],
        );
    }
    items
}

/// macOS names the last item after the app, as every menu bar app does.
#[cfg(target_os = "macos")]
const EXIT_LABEL: &str = "Quit WeekNumber";
#[cfg(not(target_os = "macos"))]
const EXIT_LABEL: &str = "Exit";

fn show_tray_menu() {
    let rows = tray_menu_items()
        .into_iter()
        .map(|(id, label, checked)| {
            if id == 0 {
                menu::Row::Separator
            } else {
                menu::Row::Item {
                    id,
                    label: label.to_string(),
                    checked: checked.unwrap_or(false),
                }
            }
        })
        .collect();
    menu::open(rows, menu::Action::Tray);
}

/// Hand the menu to a backend that draws it natively (Linux panels via
/// D-Bus, the macOS status item); Windows draws its own.
fn publish_tray_menu() {
    #[cfg(not(windows))]
    os::set_tray_menu(
        tray_menu_items()
            .into_iter()
            .map(|(id, label, checked)| (id, label.to_string(), checked))
            .collect(),
    );
}

/// Run a tray menu command (after the menu has closed).
pub fn run_tray_command(cmd: u32) {
    later(publish_tray_menu);
    match cmd {
        CMD_STARTUP => {
            if let Err(problem) = startup::set(!startup::is_enabled()) {
                os::message_box_error(&problem);
            }
        }
        CMD_BOLD => {
            with_app(|a| {
                a.settings.is_bold = !a.settings.is_bold;
                a.settings.save();
                a.refresh_tray();
            });
        }
        CMD_COLOR => open_color(),
        CMD_DARK => {
            with_app(|a| {
                a.settings.dark_mode = !a.settings.dark_mode;
                a.settings.save();
            });
            later(|| {
                with_app(App::retheme);
            });
        }
        CMD_HOLIDAYS => open_holiday(),
        CMD_TIMEZONE => open_tz(),
        CMD_ABOUT => open_about(),
        CMD_EXIT => later(shutdown),
        #[cfg(target_os = "linux")]
        CMD_INSTALL => later(crate::install::offer_accepted),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Opening and closing windows (always outside a state borrow)
// ---------------------------------------------------------------------------

pub fn toggle_calendar() {
    let open = with_app(|a| a.calendar.is_some()).unwrap_or(false);
    if open {
        close_calendar(false);
        return;
    }
    let recently = with_app(|a| {
        a.calendar_dismissed_at
            .is_some_and(|t| os::tick_ms().saturating_sub(t) < TOGGLE_GRACE_MS)
    });
    if recently != Some(true) {
        open_calendar();
    }
}

pub fn open_calendar() {
    let built = with_app(|a| {
        let family = a.family();
        Calendar::build(&a.settings, family)
    });
    let Some(Some(mut cal)) = built else { return };
    let Some(hwnd) = cal.create_window() else {
        return;
    };
    with_app(|a| a.calendar = Some(cal));
    Calendar::show(hwnd);
}

/// Close the flyout; `dismissed` when it lost the focus (see toggle).
pub fn close_calendar(dismissed: bool) {
    let taken = with_app(|a| {
        if dismissed {
            a.calendar_dismissed_at = Some(os::tick_ms());
        }
        a.calendar.take()
    })
    .flatten();
    if let Some(cal) = taken {
        cal.destroy();
    }
}

fn close_calendar_now() {
    close_calendar(false);
}

/// Repaint the open calendar after a settings change made elsewhere.
pub fn refresh_calendar() {
    with_app(|a| {
        let settings = &a.settings;
        if let Some(c) = a.calendar.as_mut() {
            c.refresh(settings);
        }
    });
}

macro_rules! dialog_open_close {
    ($open:ident, $close:ident, $field:ident, $ty:ty) => {
        pub fn $open() {
            let existing = with_app(|a| a.$field.as_ref().map(|d| d.hwnd())).flatten();
            if let Some(h) = existing {
                crate::ui::raise_window(h);
                return;
            }
            let built = with_app(|a| {
                let family = a.family();
                <$ty>::build(&a.settings, family)
            });
            let Some(Some(mut dlg)) = built else { return };
            let Some(hwnd) = dlg.create_window() else {
                return;
            };
            with_app(|a| a.$field = Some(dlg));
            <$ty>::after_create(hwnd);
        }

        pub fn $close() {
            let taken = with_app(|a| a.$field.take()).flatten();
            if let Some(d) = taken {
                d.destroy();
            }
        }
    };
}

dialog_open_close!(open_holiday, close_holiday, holiday, HolidayDialog);
dialog_open_close!(open_color, close_color, color, ColorDialog);
dialog_open_close!(open_tz, close_tz, tz, TzDialog);
dialog_open_close!(open_about, close_about, about, AboutDialog);
dialog_open_close!(open_legal, close_legal, legal, LegalDialog);

pub fn close_menu() {
    let taken = with_app(|a| a.menu.take()).flatten();
    if let Some(m) = taken {
        m.destroy();
    }
}

/// The calendar's copy menu closed: copy the chosen text, then give the
/// calendar the focus back (the Python version's focus_force).
pub fn copy_menu_closed(owner: HWND, text: Option<String>) {
    with_app(|a| {
        if let Some(c) = a.calendar.as_mut() {
            c.menu_closed();
        }
    });
    if let Some(text) = text {
        os::set_clipboard_text(owner, &text);
    }
    if os::is_window(owner) {
        os::set_foreground(owner);
        os::set_focus(owner);
    }
}

/// A new icon colour from the colour dialog.
pub fn apply_color(color: String) {
    with_app(|a| {
        a.settings.set_color(&color);
        a.settings.save();
        a.refresh_tray();
    });
}
