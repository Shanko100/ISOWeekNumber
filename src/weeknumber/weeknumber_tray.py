"""System tray integration for WeekNumber.

The tray is the only part of WeekNumber that runs all day, so it imports only
what drawing the week number needs. Every window -- calendar, dialogs, Timezone Dominator
-- is imported when it is first opened, and its modules are unloaded again
once no window is open (see weeknumber_core.ModuleSweeper): the idle app
holds the tray icon and nothing else.
"""

import sys
import tkinter as tk
from typing import TYPE_CHECKING, NamedTuple

if sys.platform == "win32":
    # The tray icon is drawn with Pillow; on macOS the menu bar draws the
    # number as text and Pillow loads with the first window instead.
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        print("Missing dependency: Pillow\nInstall with: pip install pillow")
        sys.exit(1)
    from weeknumber_platform._wintray import NotifyIcon
else:
    # Checked now, loaded with the first window: a missing Pillow should
    # stop the app at launch with the install hint, not on the first click.
    import importlib.util
    if importlib.util.find_spec("PIL") is None:
        print("Missing dependency: Pillow\nInstall with: pip install pillow")
        sys.exit(1)

from weeknumber_core import (
    APP_NAME,
    IS_MACOS,
    ModuleSweeper,
    network,
    WeekNumber,
    add_idle_hook,
    is_startup_enabled,
    log,
    normalize_hex,
    policy,
    set_idle_probe,
    set_startup,
    show_error,
    tray_callback,
    tray_font_files,
    tray_icon_size,
)

if TYPE_CHECKING:
    # Application lives in weeknumber.py, which imports this module --
    # resolvable only under TYPE_CHECKING, where the circularity is harmless.
    from weeknumber import Application
    from weeknumber_menubar import MenuBarItem
    from weeknumber_timezone_dialog import TimeZoneDialog

#: Window modules and their window classes. A module that is not imported
#: has no window open, so these are looked up in sys.modules, never imported,
#: by everything that only needs to reach an open window.
WINDOW_CLASSES = (
    ("weeknumber_calendar", "CalendarPopup"),
    ("weeknumber_timezone_dialog", "TimeZoneDialog"),
    ("weeknumber_holiday_dialog", "HolidayDialog"),
    ("weeknumber_about", "AboutPopup"),
    ("weeknumber_license_dialog", "LicenseDialog"),
    ("weeknumber_color_dialog", "ColorPickerDialog"),
    ("weeknumber_tzd_lobby", "TzdLobbyDialog"),
    ("weeknumber_tzd_dialog", "TzdGameWindow"),
)


def open_window_classes():
    """The window classes whose module is loaded, in WINDOW_CLASSES order."""
    for module_name, class_name in WINDOW_CLASSES:
        module = sys.modules.get(module_name)
        cls = getattr(module, class_name, None) if module is not None else None
        if cls is not None:
            yield cls


class IconRenderer:
    """Renders the week number at the exact physical tray size: 8x
    supersampled with a measured font fit, exact ink-box centring, and
    horizontal condensing so bold keeps the same glyph height as regular."""

    _SS = 8  # supersampling factor

    def __init__(self) -> None:
        self._image_cache: "dict[tuple[int, str, bool, int], Image.Image]" = {}

    def render(self, number: int, color: str, bold: bool, size: int) -> "Image.Image":
        number = max(1, min(53, int(number)))
        color = normalize_hex(color)
        bold = bool(bold)
        size = max(16, min(256, int(size)))
        key = (number, color, bold, size)
        cached = self._image_cache.get(key)
        if cached is not None:
            return cached
        image = self._render(str(number), color, bold, size)
        # The icon changes once a week or on a settings change: only the
        # current image is worth keeping.
        self._image_cache.clear()
        self._image_cache[key] = image
        return image

    #: Bold may overshoot the width budget by this factor and is then
    #: condensed horizontally to fit. Bold digits are wider at equal height,
    #: so without this, two-digit bold weeks fit at a visibly smaller point
    #: size than regular. A <=15% squeeze keeps full height at full weight;
    #: single digits never need it.
    _BOLD_MAX_CONDENSE = 1.15

    def _render(self, text: str, color: str, bold: bool, size: int) -> "Image.Image":
        canvas = size * self._SS
        max_width = int(canvas * self._BOLD_MAX_CONDENSE) if bold else canvas
        font, (left, top, right, bottom) = self._fit_font(text, bold, max_width, canvas)
        width, height = right - left, bottom - top

        glyph = Image.new("RGBA", (width, height), (0, 0, 0, 0))
        ImageDraw.Draw(glyph).text((-left, -top), text, fill=color, font=font)
        if width > canvas:  # condense bold instead of shrinking it
            glyph = glyph.resize((canvas, height), Image.LANCZOS)
            width = canvas

        image = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
        image.paste(glyph, ((canvas - width) // 2, (canvas - height) // 2), glyph)
        # No post-downscale alpha sharpening: it was tried in an earlier build and
        # reverted -- hardening edge alpha thins the strokes, which reads as
        # worse on a dark taskbar despite measuring "crisper" by fringe count.
        return image.resize((size, size), Image.LANCZOS)

    def _fit_font(self, text: str, bold: bool, max_width: int, max_height: int):
        # Faces are loaded per render, not cached: a render tries each size
        # once, and the icon is redrawn about once a week.
        draw = ImageDraw.Draw(Image.new("RGBA", (4, 4)))
        size = max_height
        while size > 8:
            font = self._load_font(size, bold)
            box = draw.textbbox((0, 0), text, font=font)
            if box[2] - box[0] <= max_width and box[3] - box[1] <= max_height:
                return font, box
            size = int(size * 0.9)
        font = self._load_font(8, bold)
        return font, draw.textbbox((0, 0), text, font=font)

    @staticmethod
    def _load_font(size: int, bold: bool):
        for path in tray_font_files(bold):
            if path.exists():
                try:
                    return ImageFont.truetype(str(path), size=size)
                except OSError:
                    pass
        log.warning("No system TrueType font found; falling back to PIL default")
        try:
            return ImageFont.load_default(size=size)  # Pillow >= 10.1
        except TypeError:
            return ImageFont.load_default()

class MenuEntry(NamedTuple):
    """One tray-menu row, independent of the tray backend. `checked` is
    evaluated each time the menu opens; `default` marks the click action."""
    label: str
    action: "object"
    checked: "object | None" = None
    default: bool = False
    visible: bool = True


#: Marks a separator row in the menu definition.
SEPARATOR = None


class TrayIcon:
    def __init__(self, app: "Application") -> None:
        self.app = app
        self.week = WeekNumber()
        self.renderer = IconRenderer()
        self.icon_size = tray_icon_size()
        #: NotifyIcon (weeknumber_platform._wintray) on Windows, MenuBarItem
        #: on macOS.
        self.icon: "NotifyIcon | MenuBarItem | None" = None
        self._last_image_key: tuple | None = None
        self._sweeper = ModuleSweeper()

    # -- memory -----------------------------------------------------------------
    def start_memory_management(self) -> None:
        """Record what the idle tray needs and unload everything else once no
        window is open (see weeknumber_core._release_memory). Call once the
        tray runs, on the Tk thread."""
        self._sweeper.mark()
        set_idle_probe(self._no_window_open)
        # With nothing open, nothing may use the network (normally closed
        # already by the Timezone Dominator windows themselves).
        add_idle_hook(network.deny)
        add_idle_hook(self._sweeper.sweep)

    def _no_window_open(self) -> bool:
        """True when WeekNumber shows nothing but the tray icon: no window
        class has a live instance and the root has no other toplevel (a
        tooltip, a message box)."""
        for cls in open_window_classes():
            try:
                if cls._live() is not None:
                    return False
            except Exception:
                return False
        root = self.app.root
        if root is None:
            return False
        try:
            return not any(isinstance(child, tk.Toplevel) for child in root.winfo_children())
        except tk.TclError:
            return False

    def _tooltip(self) -> str:
        text = (f"Week {self.week.number} \u00b7 updated "
                f"{self.week.last_updated.strftime('%d-%m-%Y %H:%M')}")
        return text[:63]  # NOTIFYICONDATA szTip limit

    def _image(self) -> "Image.Image":
        settings = self.app.settings
        return self.renderer.render(self.week.number, settings.selected_color,
                                    settings.is_bold, self.icon_size)

    def _image_key(self) -> tuple:
        settings = self.app.settings
        return (self.week.number, settings.selected_color, settings.is_bold)

    def refresh(self) -> None:
        """Update tooltip always, the icon bitmap only when its inputs
        changed. The menu needs nothing: both backends read the entries, and
        evaluate `checked`, each time it opens."""
        if self.icon is None:
            return
        key = self._image_key()
        if IS_MACOS:
            # The menu bar draws the digits itself as a crisp text title.
            if key != self._last_image_key:
                self._last_image_key = key
                self.icon.set_title(str(self.week.number),
                                    self.app.settings.selected_color,
                                    self.app.settings.is_bold)
            self.icon.set_tooltip(self._tooltip())
            return
        if key != self._last_image_key:
            self._last_image_key = key
            self.icon.icon = self._image()
        self.icon.title = self._tooltip()

    # -- menu callbacks (tray thread on Windows, Tk thread on macOS) ---------
    def _toggle_calendar(self) -> None:
        from weeknumber_calendar import CalendarPopup
        CalendarPopup.toggle(self.app.root, self.app.settings, self._open_timezone)

    @staticmethod
    def _refresh_calendar() -> None:
        module = sys.modules.get("weeknumber_calendar")
        if module is not None:
            module.CalendarPopup.refresh_open()

    @tray_callback
    def on_show_calendar(self, icon=None, item=None) -> None:
        self.week.refresh()
        self.refresh()
        self.app.run_on_ui(self._toggle_calendar)

    @tray_callback
    def on_toggle_startup(self, icon=None, item=None) -> None:
        target = not is_startup_enabled()  # toggle from registry truth, not a cached flag
        problem = set_startup(target)
        if problem is not None:
            self.app.run_on_ui(lambda: show_error(problem))

    @tray_callback
    def on_toggle_bold(self, icon=None, item=None) -> None:
        self.app.settings.is_bold = not self.app.settings.is_bold
        self.app.settings.save()
        self.refresh()

    @tray_callback
    def on_toggle_dark_mode(self, icon=None, item=None) -> None:
        self.app.settings.is_dark_mode = not self.app.settings.is_dark_mode
        self.app.settings.save()
        self.app.run_on_ui(self._refresh_windows)

    def _refresh_windows(self) -> None:
        """Repaint every open WeekNumber window for the active theme. Window
        modules load on first use; a window can only be open once its module
        has, so the ones not imported are skipped."""
        for cls in open_window_classes():
            try:
                cls.refresh_open()
            except Exception:
                log.exception("Could not re-theme %s", cls.__name__)

    def _open_timezone(self) -> None:
        """Single entry point for the dialog, used by the tray menu and by the
        calendar's offset chip, so only one window can ever exist. The dialog
        hides the way into Timezone Dominator (see TimeZoneDialog._play_tzd)."""
        from weeknumber_timezone_dialog import TimeZoneDialog
        play = None if policy().disable_game else self._open_tzd
        TimeZoneDialog.show(self.app.root, self.app.settings,
                            self._refresh_calendar, on_play=play)

    @tray_callback
    def on_show_timezone(self, icon=None, item=None) -> None:
        self.app.run_on_ui(self._open_timezone)

    def _open_holidays(self) -> None:
        from weeknumber_holiday_dialog import HolidayDialog
        HolidayDialog.show(self.app.root, self.app.settings, self._holidays_changed)

    def _holidays_changed(self) -> None:
        # The calendar loads the new countries' years itself when it redraws.
        self._refresh_calendar()

    @tray_callback
    def on_show_holidays(self, icon=None, item=None) -> None:
        self.app.run_on_ui(self._open_holidays)

    def _open_tzd(self, backdrop: "TimeZoneDialog | None" = None) -> None:
        """One Timezone Dominator window at a time: the running game, else the lobby,
        opened over the time-zone window whose map turned into the board.

        Before a new lobby opens, the player is asked -- every time -- whether
        Timezone Dominator may use the network; until they allow it, and again once the
        Timezone Dominator windows close, WeekNumber cannot connect at all (NetworkGate).

        The game modules are imported here, not at the top: most people never
        find the way in, and they cost about 5 MB once loaded."""
        if policy().disable_game:
            return
        from weeknumber_tzd_dialog import TzdGameWindow
        from weeknumber_tzd_lobby import TzdLobbyDialog

        lobby = TzdLobbyDialog._live()
        if TzdGameWindow.raise_open() or TzdLobbyDialog.raise_open():
            # Already playing elsewhere: this map turns back, unless the
            # open lobby is the one over it (the die pressed again).
            if backdrop is not None and (lobby is None or not lobby.is_over(backdrop)):
                backdrop.end_tzd()
            return
        root, settings = self.app.root, self.app.settings

        def answered(allowed: bool) -> None:
            if not allowed:
                network.deny()
                if backdrop is not None:
                    backdrop.end_tzd()  # back to the map; nothing connected
                return
            from weeknumber_tzd_lobby import network_needed
            if network_needed():
                network.allow()
            TzdLobbyDialog.show(root, settings, self.app.run_on_ui,
                                  lambda session: TzdGameWindow.show(root, settings, session),
                                  backdrop=backdrop)
            if TzdLobbyDialog._live() is None:
                network.deny()
                if backdrop is not None:
                    backdrop.end_tzd()  # the lobby could not open: back to the map

        self._confirm_network(backdrop, answered)

    def _confirm_network(self, backdrop, on_result) -> None:
        """The network notice (weeknumber_tzd_lobby.network_notice), over
        the board it was opened from. Skipped when the IT policy allows no
        network play at all: the lobby then only says so."""
        from weeknumber_dialog import ConfirmDialog
        from weeknumber_tzd_lobby import network_needed, network_notice
        if not network_needed():
            on_result(True)  # the lobby says play is off; the gate stays shut
            return
        parent = backdrop.top if backdrop is not None else None
        ConfirmDialog.ask(self.app.root, self.app.settings, parent=parent,
                          on_result=on_result, **network_notice())

    def _open_about(self) -> None:
        from weeknumber_about import AboutPopup
        AboutPopup.show(self.app.root, self.app.settings)

    @tray_callback
    def on_show_about(self, icon=None, item=None) -> None:
        self.app.run_on_ui(self._open_about)

    def _open_color_picker(self) -> None:
        """The app's own colour window: it opens (or comes to the front) over
        whatever else is open, like every other WeekNumber window, and looks
        like them, which the system colour dialog did not."""
        from weeknumber_color_dialog import ColorPickerDialog
        ColorPickerDialog.show(self.app.root, self.app.settings,
                               self.app.settings.selected_color, self._color_chosen)

    def _color_chosen(self, color: str) -> None:
        self.app.settings.selected_color = color
        self.app.settings.save()
        self.refresh()

    @tray_callback
    def on_change_color(self, icon=None, item=None) -> None:
        self.app.run_on_ui(self._open_color_picker)

    @tray_callback
    def on_exit(self, icon=None, item=None) -> None:
        self.app.request_shutdown()

    def _menu_entries(self) -> "list[MenuEntry | None]":
        return [
            # Windows hides this row: it stays the left-click default action.
            # The macOS menu lists it, as menu-bar extras conventionally do.
            MenuEntry("Show Calendar", self.on_show_calendar,
                      default=True, visible=IS_MACOS),
            MenuEntry("Run at Startup", self.on_toggle_startup,
                      checked=is_startup_enabled),
            MenuEntry("Bold Text", self.on_toggle_bold,
                      checked=lambda: self.app.settings.is_bold),
            MenuEntry("Change Color\u2026", self.on_change_color),
            MenuEntry("Dark Mode", self.on_toggle_dark_mode,
                      checked=lambda: self.app.settings.is_dark_mode),
            MenuEntry("Public Holidays\u2026", self.on_show_holidays),
            MenuEntry("Time Zone\u2026", self.on_show_timezone),
            SEPARATOR,
            MenuEntry("About\u2026", self.on_show_about),
            SEPARATOR,
            MenuEntry("Quit" if IS_MACOS else "Exit", self.on_exit),
        ]

    def run(self) -> None:
        self._last_image_key = self._image_key()
        if IS_MACOS:
            # Created on the Tk main thread; Tk's Aqua event loop pumps it.
            from weeknumber_menubar import MenuBarItem
            self.icon = MenuBarItem(self._menu_entries())
            self.icon.set_title(str(self.week.number),
                                self.app.settings.selected_color,
                                self.app.settings.is_bold)
            self.icon.set_tooltip(self._tooltip())
            return
        self.icon = NotifyIcon(APP_NAME, self._image(), self._tooltip(),
                               self._menu_entries)
        self.icon.run_detached()

    def stop(self) -> None:
        if self.icon is not None:
            try:
                self.icon.stop()
            except Exception:
                log.exception("Tray icon stop failed")
            self.icon = None
