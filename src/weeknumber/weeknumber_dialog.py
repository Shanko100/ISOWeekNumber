"""Shared scaffolding for WeekNumber's titled windows.

About, Public Holidays, Time Zone and the two Timezone Dominator windows are all the same
kind of window: titled, fixed-size, one instance at a time, sized for the
monitor under the cursor, themed from the settings, and raised to the front
when reopened. TitledDialog owns that; subclasses build their content and
implement _apply_theme.

The menu-bar calendar is not a TitledDialog: it is a borderless flyout with
its own placement and lifetime rules.
"""

from __future__ import annotations

import tkinter as tk
from typing import TYPE_CHECKING

from weeknumber_calendar import ui_font_family
from weeknumber_widgets import PillButton
from weeknumber_core import (
    APP_NAME,
    ICO_WINDOW_ICONS,
    apply_window_icon,
    bring_app_to_front,
    bundled_file,
    cursor_monitor_geometry,
    frame_hwnd,
    log,
    release_memory_soon,
    set_dark_titlebar,
    theme_colors,
)

if TYPE_CHECKING:
    from weeknumber_core import RECT, SettingsStore


class TitledDialog:
    """Base for single-instance titled windows.

    Lifecycle: ``show()`` builds the window or raises the open one;
    ``refresh_open()`` re-themes it after a settings change; ``close()``
    (also Escape and the close button) destroys it. Subclasses call
    ``_init_window()`` first in ``__init__``, override ``_apply_theme`` and,
    when they own timers, ``_on_close``.
    """

    #: Caption after "WeekNumber – ".
    TITLE = ""
    #: Breathing room left around a monitor-sized window, px @ 96 DPI.
    SCREEN_MARGIN_PX = 60
    #: The open instance; each subclass gets its own on first assignment.
    _instance: "TitledDialog | None" = None

    # -- single instance ------------------------------------------------------
    @classmethod
    def _live(cls) -> "TitledDialog | None":
        instance = cls._instance
        if instance is None:
            return None
        try:
            if instance.top.winfo_exists():
                return instance
        except tk.TclError:
            pass
        cls._instance = None
        return None

    @classmethod
    def show(cls, master: tk.Misc, *args, **kwargs) -> None:
        """Open the window, or raise the one already open."""
        existing = cls._live()
        if existing is not None:
            existing._raise()
            return
        try:
            cls._instance = cls(master, *args, **kwargs)
        except Exception:
            cls._instance = None
            log.exception("Failed to open the %s window", cls.__name__)

    @classmethod
    def raise_open(cls) -> bool:
        """Raise the open window, if any; whether there was one."""
        existing = cls._live()
        if existing is None:
            return False
        existing._raise()
        return True

    @classmethod
    def refresh_open(cls) -> None:
        """Re-apply the palette to an open window after a theme change."""
        existing = cls._live()
        if existing is not None:
            existing._apply_theme()

    # -- construction -----------------------------------------------------------
    def _init_window(self, master: tk.Misc, settings: "SettingsStore") -> "RECT":
        """Create the withdrawn toplevel and the shared state; returns the
        work area of the monitor the window will open on."""
        _, _, work, scale = cursor_monitor_geometry()
        self.scale = scale
        self.settings = settings
        self.theme = theme_colors(settings)
        self.top = tk.Toplevel(master)
        self.top.withdraw()
        self.top.title(f"{APP_NAME} – {self.TITLE}" if self.TITLE else APP_NAME)
        self.top.resizable(False, False)
        self.top.protocol("WM_DELETE_WINDOW", self.close)
        self.top.bind("<Escape>", lambda e: self.close())
        self.top.bind("<Destroy>", self._on_destroy)
        self._family = ui_font_family(self.top)
        return work

    def _apply_theme(self) -> None:
        """Repaint for the active palette; subclasses extend this."""
        self.theme = theme_colors(self.settings)
        self._sync_titlebar()

    # -- metrics ------------------------------------------------------------------
    def _px(self, value: float) -> int:
        return max(1, round(value * self.scale))

    def _font(self, px: float, bold: bool = False):
        # Negative Tk font size == pixels: exact sizing on every monitor.
        font = (self._family, -self._px(px))
        return font + ("bold",) if bold else font

    def _color(self, name: str) -> str:
        return self.theme[name]

    # -- placement & chrome -------------------------------------------------------------
    def _center(self, work: "RECT", vertical: float = 0.5) -> None:
        """Place the window in `work`, horizontally centred and `vertical`
        of the way down the free space."""
        self.top.update_idletasks()
        width, height = self.top.winfo_reqwidth(), self.top.winfo_reqheight()
        x = work.left + max(0, (work.right - work.left - width) // 2)
        y = work.top + max(0, int((work.bottom - work.top - height) * vertical))
        self.top.geometry(f"+{int(x)}+{int(y)}")

    def _sync_titlebar(self) -> None:
        """Dark or light caption to match the palette, and a window icon that
        is sharp at the monitor's scaling. `frame_hwnd` is the decorated
        frame, the window the caption belongs to."""
        try:
            self.top.update_idletasks()
            set_dark_titlebar(frame_hwnd(self.top), self.settings.is_dark_mode)
        except tk.TclError:
            pass
        self._sync_icon()

    def _sync_icon(self) -> None:
        if ICO_WINDOW_ICONS:
            try:
                apply_window_icon(self.top, bundled_file("app.ico"))
            except tk.TclError:
                pass

    def _raise(self) -> None:
        """Bring the window forward from a tray click without leaving it on
        top: Windows only reliably raises a window that is topmost at the
        moment of the request, so set the bit and drop it again."""
        try:
            self.top.deiconify()
            bring_app_to_front()
            self.top.attributes("-topmost", True)
            self.top.lift()
            self.top.focus_force()
            self.top.after(200, self._drop_topmost)
        except tk.TclError:
            pass
        self._sync_icon()

    def _drop_topmost(self) -> None:
        try:
            if self.top.winfo_exists():
                self.top.attributes("-topmost", False)
        except tk.TclError:
            pass

    # -- teardown ---------------------------------------------------------------------------
    def _on_close(self) -> None:
        """Hook: cancel timers, close sessions. Runs once, before destroy."""

    def _forget(self) -> None:
        if type(self)._instance is self:
            type(self)._instance = None
            self._on_close()
            release_memory_soon(self.top)

    def _on_destroy(self, event) -> None:
        # Fires for every descendant; only the toplevel's own event ends us.
        if event.widget is self.top:
            self._forget()

    def close(self) -> None:
        self._forget()
        try:
            self.top.destroy()
        except tk.TclError:
            pass


class ConfirmDialog(TitledDialog):
    """A small question in the app's style: a heading, an explanation and
    two pill buttons. Modal to the window it is asked over (`parent`), so
    that window waits for the answer; `on_result(True)` runs for the confirm
    button, `on_result(False)` for the other one, Escape or the close box --
    exactly once either way.

    Used instead of the system message box, which ignores the app's palette
    and blocks the whole app while it is open."""

    WIDTH_PX = 440
    MARGIN_PX = 24

    @classmethod
    def ask(cls, master: tk.Misc, settings: "SettingsStore", *, title: str, heading: str,
            message: str, confirm: str, cancel: str, on_result,
            parent: "tk.Misc | None" = None, details: "list[tuple[str, str]] | None" = None
            ) -> None:
        existing = cls._live()
        if existing is not None:  # one question at a time: the older one is cancelled
            existing._answer(False)
        cls.show(master, settings, title, heading, message, confirm, cancel, on_result,
                 parent, details or [])

    def __init__(self, master: tk.Misc, settings: "SettingsStore", title: str, heading: str,
                 message: str, confirm: str, cancel: str, on_result,
                 parent: "tk.Misc | None", details: "list[tuple[str, str]]") -> None:
        work = self._init_window(master, settings)
        self.top.title(f"{APP_NAME} – {title}")
        self._on_result = on_result
        self._answered = False
        theme = self.theme
        bg = theme["CARD_BG"]
        wrap = self._px(self.WIDTH_PX - 2 * self.MARGIN_PX)
        self.top.configure(bg=bg)
        card = tk.Frame(self.top, bg=bg, padx=self._px(self.MARGIN_PX), pady=self._px(22))
        card.pack(fill="both", expand=True)
        tk.Frame(card, bg=bg, width=wrap, height=0).pack()
        tk.Label(card, text=heading, font=self._font(17, bold=True), bg=bg,
                 fg=theme["TITLE_FG"], anchor="w", justify="left",
                 wraplength=wrap).pack(fill="x")
        tk.Label(card, text=message, font=self._font(11), bg=bg, fg=theme["YEAR_FG"],
                 anchor="w", justify="left", wraplength=wrap).pack(
            fill="x", pady=(self._px(6), self._px(4)))
        for label, text in details:
            tk.Label(card, text=label, font=self._font(11, bold=True), bg=bg,
                     fg=theme["TITLE_FG"], anchor="w").pack(fill="x", pady=(self._px(10), 0))
            tk.Label(card, text=text, font=self._font(11), bg=bg, fg=theme["YEAR_FG"],
                     anchor="w", justify="left", wraplength=wrap).pack(fill="x")
        footer = tk.Frame(card, bg=bg)
        footer.pack(fill="x", pady=(self._px(20), 0))
        font = self._font(11, bold=True)
        self.confirm_button = PillButton(footer, confirm, font, self.scale,
                                         lambda: self._answer(True))
        self.confirm_button.apply_theme(theme)
        self.confirm_button.pack(side="right")
        self.cancel_button = PillButton(footer, cancel, font, self.scale,
                                        lambda: self._answer(False), primary=False)
        self.cancel_button.apply_theme(theme)
        self.cancel_button.pack(side="right", padx=(0, self._px(8)))
        if parent is not None:
            try:
                self.top.transient(parent.winfo_toplevel())
            except tk.TclError:
                pass
        self._center_over(work, parent)
        self.top.deiconify()
        self._sync_titlebar()
        self._raise()
        self.cancel_button.focus_set()  # Enter on its own never confirms
        try:
            self.top.grab_set()
        except tk.TclError:
            pass

    def _center_over(self, work: "RECT", parent: "tk.Misc | None") -> None:
        self.top.update_idletasks()
        width, height = self.top.winfo_reqwidth(), self.top.winfo_reqheight()
        x = work.left + (work.right - work.left - width) // 2
        y = work.top + (work.bottom - work.top - height) // 3
        if parent is not None:
            try:
                top = parent.winfo_toplevel()
                x = top.winfo_rootx() + (top.winfo_width() - width) // 2
                y = top.winfo_rooty() + (top.winfo_height() - height) // 3
            except tk.TclError:
                pass
        x = max(work.left, min(x, work.right - width))
        y = max(work.top, min(y, work.bottom - height))
        self.top.geometry(f"+{int(x)}+{int(y)}")

    def _answer(self, value: bool) -> None:
        if self._answered:
            return
        callback = self._take_callback()
        try:
            self.top.grab_release()
        except tk.TclError:
            pass
        super().close()  # gone before the answer acts, e.g. opens a window
        self._run(callback, value)

    def _take_callback(self):
        """The callback, handed out once: every question is answered once."""
        self._answered = True
        callback, self._on_result = self._on_result, None
        return callback

    @staticmethod
    def _run(callback, value: bool) -> None:
        if callback is not None:
            try:
                callback(value)
            except Exception:
                log.exception("A confirmation callback failed")

    def close(self) -> None:
        self._answer(False)

    def _on_close(self) -> None:
        # Destroyed some other way (the app quitting): an unanswered question
        # is a no.
        if not self._answered:
            self._run(self._take_callback(), False)

    def _apply_theme(self) -> None:
        super()._apply_theme()  # opened briefly; a theme switch keeps its colours
