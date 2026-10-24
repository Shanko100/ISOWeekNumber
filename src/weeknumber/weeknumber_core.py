"""Core infrastructure for WeekNumber.

Requirements: Windows 10+ or macOS 12+, Python 3.15, Pillow. The Windows
tray icon (weeknumber_platform._wintray) and the macOS menu-bar item reach
the OS through ctypes; chat uses PyNaCl when it is installed.

Design notes
------------
* Everything that differs per operating system -- DPI, monitor geometry,
  window chrome, the single-instance guard, run-at-startup -- lives in the
  ``weeknumber_platform`` package and is re-exported here, so UI modules keep
  importing one stable set of names.
* On Windows the tray icon is rendered at the shell's exact SM_CXSMICON
  size, 8x supersampled, and loaded at that size (see NativeSizeIcon) so
  neither GDI nor Explorer ever resamples it. On macOS the menu bar draws the
  week number itself as text (see weeknumber_menubar).
* The calendar grid is built once per popup; month navigation only
  reconfigures label text/colours.
* This module deliberately owns only shared, GUI-toolkit-agnostic
  infrastructure. Tk and Pillow are imported by the modules that
  use them, so each file's dependency surface is explicit and analysable.
"""

import os
import sys

# First import that can load native code: on Windows the platform backend
# restricts the DLL search path to System32 before anything else loads.
import weeknumber_platform as platform_
from weeknumber_platform import (  # noqa: F401  (re-exported for UI modules)
    CONTEXT_MENU_EVENTS,
    GLASS_BACKDROP,
    ICO_WINDOW_ICONS,
    IS_MACOS,
    IS_WINDOWS,
    POINT,
    RECT,
    QuitSignal,
    SingleInstance,
    UI_FONT_FAMILIES,
    attach_glass_backdrop,
    bring_app_to_front,
    cursor_monitor_geometry,
    enable_dpi_awareness,
    frame_hwnd,
    is_startup_enabled,
    open_file,
    owns_foreground,
    play_sound,
    prepare_app,
    read_policy,
    reduce_motion,
    refresh_startup_path,
    request_attention,
    set_dark_titlebar,
    set_rounded_corners,
    set_startup,
    system_dpi,
    toplevel_hwnd,
    tray_edge,
    tray_font_files,
    tray_icon_size,
    trim_memory,
    trim_working_set,
    set_app_identity,
    apply_window_icon,
)

import datetime as dt
import functools
import gc
import json
import logging
import queue
import re
import tempfile
import time
from collections.abc import Callable, Iterable
from pathlib import Path


# Offline public-holiday tables. Security note: do not import holiday data as
# executable code from a potentially writable application directory. The
# tables are non-executable JSON Lines next to this script/executable: an
# index line {"METADATA": {...}, "COUNTRIES": [["NL", "Netherlands", length],
# ...]}, then one {"code": "NL", "years": {...}} line per country.

# ---------------------------------------------------------------------------
# Metadata & constants
# ---------------------------------------------------------------------------
APP_NAME = "WeekNumber"
APP_VERSION = "1.0"
APP_COPYRIGHT = "Shanko100"
APP_URL = "https://github.com/Shanko100/ISOWeekNumber"
#: Clock refresh for the calendar chip and the time-zone dialog. One second
#: keeps the displayed minute correct to within a second of the change.
CLOCK_TICK_MS = 1000


# Palette
#
# Every window consumes colours exclusively through theme_colors(settings).
LIGHT_THEME = {
    # Fluent-leaning light palette: one accent family, quiet neutral text
    # tiers, and a hairline surface stroke. Public-holiday purple is
    # intentionally distinct from the accent family.
    "CARD_BG": "#F9FAFC",
    "CARD_BORDER": "#E1E6EF",       # 1px surface stroke, as on Win11 flyouts
    "ACCENT": "#2F6FED",
    "TITLE_FG": "#1F2937",
    "YEAR_FG": "#6B7280",
    "DAY_NAME_FG": "#6E7A8A",       # secondary text, not accent: headers are
    "WEEK_NUM_FG": "#8A94A6",       # wayfinding, only *today* carries accent
    "CW_LABEL_FG": "#8A94A6",
    "TODAY_FG": "#FFFFFF",
    "CURRENT_WEEK_BG": "#EAF2FF",
    "SELECTED_WEEK_BG": "#F1F6FF",
    "SELECTED_DAY_FILL": "#E3EEFF",
    "HOLIDAY_BG": "#E9D8FD",
    "HOLIDAY_SECONDARY_BG": "#CDEFE8",
    # Saturated companions used for holiday outlines when Today/selection
    # already owns the cell fill.
    "HOLIDAY_BORDER": "#8B5CF6",
    "HOLIDAY_SECONDARY_BORDER": "#0F9D8A",
    "HOLIDAY_FG": "#312044",
    "TOOLTIP_BG": "#FFFFFF",
    "TOOLTIP_BORDER": "#D6DEEA",
    "HOVER_BG": "#EAF0F9",
    "SEPARATOR_BG": "#E7EBF2",
    "MENU_BG": "#FFFFFF",
    "MENU_FG": "#1F2937",
    "MENU_ACTIVE_BG": "#E7F0FF",
    "MENU_ACTIVE_FG": "#1F2937",
    "INPUT_BG": "#FFFFFF",
    "INPUT_BORDER": "#D6DEEA",
    "TOGGLE_TRACK_OFF": "#C9D3E3",
    "TOGGLE_KNOB": "#FFFFFF",
    # Time-zone picker map. Bands are a five-hue cycle indexed by position in
    # the offset-sorted list, so neighbouring zones never share a colour; the
    # SEA tints are washed-out twins of the LAND fills, exactly as a printed
    # time-zone map treats open water.
    "MAP_OCEAN": "#EFF3F8",
    "MAP_EDGE": "#D2DAE5",
    # Country hairlines, drawn over the land fills.
    "MAP_BORDER_LINE": "#FFFFFF",
    # Five band colours, cycled so neighbouring zones never share one. Held at
    # a similar lightness so no zone reads as heavier than its neighbours; the
    # paler sea version of each is derived from these at draw time.
    "MAP_BAND_0": "#8FB3D9",
    "MAP_BAND_1": "#8FC4A8",
    "MAP_BAND_2": "#E0C285",
    "MAP_BAND_3": "#D79E9E",
    "MAP_BAND_4": "#AFA3D1",
    "MAP_SELECT_LAND": "#2F6FED",
    "MAP_RULER_FG": "#39445A",
    # Timezone Dominator board. Player colours are saturated for army badges and legends;
    # the board washes them toward the ocean for territory fills, so owned
    # land sits at the same lightness as the time-zone map's band colours.
    "TZD_PLAYER_0": "#2F6FED",
    "TZD_PLAYER_1": "#D9483B",
    "TZD_PLAYER_2": "#2E9E5B",
    "TZD_PLAYER_3": "#D99A1E",
    "TZD_PLAYER_4": "#8257C9",
    "TZD_PLAYER_5": "#1A9AA6",
    "TZD_NEUTRAL": "#8A94A6",
    #: Land outside every territory (Antarctica, remote islands).
    "TZD_OFFBOARD_LAND": "#DCE2EA",
    # Continents, in board order: each territory is outlined on the inside
    # in its continent's colour, and the legend cells carry the same hue.
    # Deep tones, so the outline reads against every washed player fill.
    "TZD_CONTINENT_0": "#C58F00",   # North America, gold
    "TZD_CONTINENT_1": "#C2410C",   # South America, burnt orange
    "TZD_CONTINENT_2": "#2557D6",   # Europe, blue
    "TZD_CONTINENT_3": "#8A4B16",   # Africa, umber
    "TZD_CONTINENT_4": "#15803D",   # Asia, green
    "TZD_CONTINENT_5": "#7E22CE",   # Australia, purple
    # Card symbols.
    "TZD_CARD_INFANTRY": "#0F9F77",
    "TZD_CARD_CAVALRY": "#D97706",
    "TZD_CARD_ARTILLERY": "#DB2D55",
    "TZD_CARD_WILD": "#7C5CFA",
}

DARK_THEME = {
    # Neutral dark-gray dark mode. Keep the accent family limited to blue and
    # the public-holiday marker purple, so the palette does not pick up a
    # separate blue-tinted background hue.
    "CARD_BG": "#1F1F1F",
    "CARD_BORDER": "#3A3A3A",
    "ACCENT": "#6EA8FF",
    "TITLE_FG": "#F5F5F5",
    "YEAR_FG": "#B8B8B8",
    "DAY_NAME_FG": "#9AA3B0",
    "WEEK_NUM_FG": "#8B94A3",
    "CW_LABEL_FG": "#8B94A3",
    "TODAY_FG": "#FFFFFF",
    "CURRENT_WEEK_BG": "#26364E",
    "SELECTED_WEEK_BG": "#232B3A",
    "SELECTED_DAY_FILL": "#2D4B73",
    "HOLIDAY_BG": "#3E2F50",
    "HOLIDAY_SECONDARY_BG": "#174A48",
    "HOLIDAY_BORDER": "#B794F4",
    "HOLIDAY_SECONDARY_BORDER": "#4FD1C5",
    "HOLIDAY_FG": "#FFFFFF",
    "TOOLTIP_BG": "#262626",
    "TOOLTIP_BORDER": "#3A3A3A",
    "HOVER_BG": "#2C333D",
    "SEPARATOR_BG": "#333333",
    "MENU_BG": "#242424",
    "MENU_FG": "#F5F5F5",
    "MENU_ACTIVE_BG": "#2D4B73",
    "MENU_ACTIVE_FG": "#FFFFFF",
    "INPUT_BG": "#2A2A2A",
    "INPUT_BORDER": "#3F3F3F",
    "TOGGLE_TRACK_OFF": "#4A4A4A",
    "TOGGLE_KNOB": "#F0F0F0",
    "MAP_OCEAN": "#15181D",
    "MAP_EDGE": "#333A44",
    "MAP_BORDER_LINE": "#1A1E25",
    "MAP_BAND_0": "#43678D",
    "MAP_BAND_1": "#3F7A62",
    "MAP_BAND_2": "#8A7237",
    "MAP_BAND_3": "#8A5551",
    "MAP_BAND_4": "#5F5686",
    "MAP_SELECT_LAND": "#5B9DFF",
    "MAP_RULER_FG": "#12161B",
    "TZD_PLAYER_0": "#5B9DFF",
    "TZD_PLAYER_1": "#F0705F",
    "TZD_PLAYER_2": "#4CC27F",
    "TZD_PLAYER_3": "#F0B545",
    "TZD_PLAYER_4": "#A387E6",
    "TZD_PLAYER_5": "#3EC4CC",
    "TZD_NEUTRAL": "#7B8494",
    "TZD_OFFBOARD_LAND": "#2A3038",
    # Light tones on the dark board, for the same reason.
    "TZD_CONTINENT_0": "#F2C94C",
    "TZD_CONTINENT_1": "#FB8C5A",
    "TZD_CONTINENT_2": "#6EA8FF",
    "TZD_CONTINENT_3": "#D39A6A",
    "TZD_CONTINENT_4": "#5BD08A",
    "TZD_CONTINENT_5": "#C79BFF",
    "TZD_CARD_INFANTRY": "#2BC79A",
    "TZD_CARD_CAVALRY": "#F0A53A",
    "TZD_CARD_ARTILLERY": "#F2557A",
    "TZD_CARD_WILD": "#9D85FF",
}


def theme_colors(settings: "SettingsStore | None" = None) -> dict[str, str]:
    """Return a copy of the active calendar palette.

    A copy keeps the palette immutable from the caller's perspective. The
    popup stores one snapshot per instance because its rounded Tk PhotoImages
    are colour-specific and must be rebuilt when the theme changes.
    """
    return dict(DARK_THEME if settings is not None and settings.is_dark_mode else LIGHT_THEME)

SETTINGS_DIR = platform_.settings_dir()
SETTINGS_FILE = SETTINGS_DIR / "settings.json"
LOG_FILE = SETTINGS_DIR / "weeknumber.log"
MAX_SETTINGS_BYTES = 64 * 1024

log = logging.getLogger("weeknumber")
#: The log file's size limit; one older file is kept beside it (.1).
MAX_LOG_BYTES = 256 * 1024


class SizeLimitedLog(logging.FileHandler):
    """The log file, kept under `max_bytes`: when the next record would pass
    it, the file becomes ``<name>.1`` (replacing the older one) and a new
    file starts -- what logging.handlers.RotatingFileHandler does with
    backupCount=1. Written out here because that module imports socket,
    select and pickle into the idle app (about 0.5 MB) for nothing."""

    def __init__(self, path: Path, max_bytes: int) -> None:
        super().__init__(path, mode="a", encoding="utf-8")
        self.max_bytes = max_bytes

    def emit(self, record: logging.LogRecord) -> None:
        # handle() holds the handler's lock around emit.
        try:
            if self.stream is None:
                self.stream = self._open()
            size = len((self.format(record) + self.terminator).encode("utf-8"))
            if self.stream.tell() and self.stream.tell() + size >= self.max_bytes:
                self._roll_over()
        except Exception:
            self.handleError(record)
            return
        super().emit(record)

    def _roll_over(self) -> None:
        self.stream.close()
        self.stream = None
        path = Path(self.baseFilename)
        try:
            if path.is_file():
                os.replace(path, path.with_name(path.name + ".1"))
        finally:
            self.stream = self._open()


def setup_logging() -> None:
    SETTINGS_DIR.mkdir(parents=True, exist_ok=True)
    handler = SizeLimitedLog(LOG_FILE, MAX_LOG_BYTES)
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s: %(message)s"))
    log.addHandler(handler)
    log.setLevel(logging.INFO)

def load_bounded_json(path: Path, max_bytes: int):
    """Read and decode JSON without a stat/read race or unbounded allocation."""
    return json.loads(read_bounded_text(path, max_bytes))


def read_bounded_text(path: Path, max_bytes: int) -> str:
    """A UTF-8 file of at most `max_bytes`, read in one bounded call.

    The read is sized by the open file, not by the cap: asking for the cap
    makes Python allocate all of it first, and the allocator keeps most of
    that after the bytes object shrinks -- about 6 MB per 6 MB file."""
    if not isinstance(max_bytes, int) or max_bytes <= 0:
        raise ValueError("max_bytes must be a positive integer")
    with path.open("rb") as handle:
        size = os.fstat(handle.fileno()).st_size
        if size > max_bytes:
            raise ValueError(f"{path.name} exceeds the {max_bytes}-byte limit")
        payload = handle.read(size + 1)
    if not payload:
        raise ValueError(f"{path.name} is empty")
    if len(payload) > size:
        raise ValueError(f"{path.name} changed while it was read")
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError(f"{path.name} is not valid UTF-8") from exc
    return text


class UiDispatcher:
    """Runs callables on the Tk thread on behalf of any thread.

    Worker threads (the tray, the eviction waiter, Timezone Dominator sockets) only append
    to a thread-safe queue; a short `after` timer on the Tk thread drains it.
    No Tcl call is ever made off the Tk thread. Calling `after` from another
    thread works on Windows but does not wake Aqua's Cocoa run loop on macOS,
    where queued work would sit until the next user event.

    `post` must never call Tcl, not even on the Tk thread: the macOS menu
    bar posts from inside Cocoa callbacks, and a tkinter call made there
    clears the thread state tkinter keeps for its own callbacks, so the next
    Tk timer aborts the interpreter ("PyEval_RestoreThread: ... the GIL is
    released").

    The timer runs every ACTIVE_POLL_MS while work is flowing (a Timezone Dominator game,
    a burst of clicks) and backs off to IDLE_POLL_MS once the queue has been
    quiet for IDLE_AFTER_S: an idle app wakes 20 times a second rather than
    33, and a click still waits at most 50 ms.
    """

    ACTIVE_POLL_MS = 30
    IDLE_POLL_MS = 50
    IDLE_AFTER_S = 2.0

    def __init__(self, widget) -> None:
        self._widget = widget
        self._queue: "queue.SimpleQueue" = queue.SimpleQueue()
        self._running = False
        self._last_work = 0.0

    def start(self) -> None:
        if not self._running:
            self._running = True
            self._widget.after(self.ACTIVE_POLL_MS, self._tick)

    def stop(self) -> None:
        self._running = False

    def post(self, func) -> None:
        self._queue.put(func)  # only this: see the class docstring

    def _run_pending(self) -> None:
        if not self._running:
            return
        while True:
            try:
                func = self._queue.get_nowait()
            except queue.Empty:
                break
            self._last_work = time.monotonic()
            try:
                func()
            except Exception:
                log.exception("UI callback failed")

    def _tick(self) -> None:
        if not self._running:
            return
        self._run_pending()
        idle = time.monotonic() - self._last_work > self.IDLE_AFTER_S
        try:
            self._widget.after(self.IDLE_POLL_MS if idle else self.ACTIVE_POLL_MS,
                               self._tick)
        except Exception:
            self._running = False


#: Quiet time after a window closes before its memory is handed back, so a
#: burst of closes (or close and reopen) costs one pass.
RELEASE_DELAY_MS = 1500
_release_after: "tuple[object, str] | None" = None
#: Asked at the start of every release pass whether no window is open; the
#: tray installs it (see set_idle_probe). Idle passes also unload modules and
#: trim the working set.
_idle_probe: "Callable[[], bool] | None" = None
#: Called on idle passes before the collection, e.g. ModuleSweeper.sweep.
_idle_hooks: "list[Callable[[], object]]" = []


def release_memory_soon(widget) -> None:
    """Hand freed memory back to the system shortly after a window closes.

    Closed windows leave reference cycles -- widgets, images, callbacks --
    that Python's collector would only find later, and the allocators keep
    freed pages for reuse. The pass collects the cycles on the Tk thread
    (where their Tk objects must be finalised), then asks the platform to
    return the free pages. Call it with any live widget, before destroying
    it if it is the window that closes."""
    global _release_after
    try:
        root = widget.nametowidget(".")
        if _release_after is not None and _release_after[0] is root:
            root.after_cancel(_release_after[1])
        _release_after = (root, root.after(RELEASE_DELAY_MS, _release_memory))
    except Exception:
        log.debug("Could not schedule a memory release", exc_info=True)


def set_idle_probe(probe: "Callable[[], bool] | None") -> None:
    """Install the check that says no WeekNumber window is open."""
    global _idle_probe
    _idle_probe = probe


def add_idle_hook(hook: "Callable[[], object]") -> None:
    """Run `hook` on every release pass that finds the app idle."""
    if hook not in _idle_hooks:
        _idle_hooks.append(hook)


def _is_idle() -> bool:
    if _idle_probe is None:
        return False
    try:
        return bool(_idle_probe())
    except Exception:
        log.debug("Idle probe failed", exc_info=True)
        return False


def _release_memory() -> None:
    """One release pass, on the Tk thread.

    Every pass collects cycles and hands free heap pages back. When no window
    is open it first runs the idle hooks -- the tray unloads every module a
    window brought in -- and afterwards trims the working set, so the idle
    tray app holds only the pages it actually touches."""
    global _release_after
    _release_after = None
    idle = _is_idle()
    if idle:
        for hook in list(_idle_hooks):
            try:
                hook()
            except Exception:
                log.exception("Idle memory hook failed")
    gc.collect()
    platform_.trim_memory()
    if idle and _is_idle():  # a hook could in theory have opened something
        platform_.trim_working_set()


class ModuleSweeper:
    """Unloads the modules that windows imported once no window is open.

    `mark()` records what the always-running tray needs (the modules loaded
    at startup). `sweep()` later removes from sys.modules every module that
    is not part of that baseline and can be imported again cleanly, so the
    code and data of the calendar, the dialogs and Timezone Dominator leave memory with
    their last window and load again (a few milliseconds) on the next open.

    What stays, because unloading it is not safe or frees nothing:

    * native extension modules, and every module of a package that contains
      one: a native module cannot be unloaded, and it may cache the package's
      Python classes (PyNaCl's libsodium core raises its own CryptoError), so
      a fresh copy of the package beside the old native one could break
      `except` clauses;
    * modules without a normal file loader (built-in, frozen stdlib, entries
      a native module registered itself);
    * KEEP: interpreter and toolkit infrastructure that holds process-wide
      state (subprocess reaps its children, logging holds the handlers, Tk
      and Pillow register native commands and plugins).

    A module may define ``__weeknumber_unload__()`` to release what it
    registered outside Python (e.g. Tk class bindings) before it goes."""

    #: Top-level names that are never unloaded.
    KEEP = frozenset((
        "weeknumber", "weeknumber_core", "weeknumber_platform", "weeknumber_tray",
        "weeknumber_timezone", "weeknumber_menubar",
        "tkinter", "_tkinter", "PIL", "six",
        "encodings", "codecs", "importlib", "zipimport", "ctypes", "_ctypes",
        "logging", "threading", "subprocess", "signal", "atexit", "weakref",
        "multiprocessing", "concurrent", "asyncio", "locale", "gc", "site",
        "sitecustomize", "usercustomize", "typing", "abc", "copyreg", "__main__",
        "_pytest", "pytest", "pluggy",
    ))
    #: Top-level name prefixes that are never unloaded (PyInstaller's own).
    KEEP_PREFIXES = ("pyimod", "pyi_", "_pyi")
    #: Loader class names whose modules can be imported again from source
    #: or bytecode: CPython's, and PyInstaller's archive loader ("Pyi...").
    _RELOADABLE = ("SourceFileLoader", "SourcelessFileLoader")

    def __init__(self) -> None:
        self._baseline: "set[str] | None" = None

    def mark(self) -> None:
        self._baseline = set(sys.modules)

    @classmethod
    def _kept_prefix(cls, name: str) -> bool:
        top = name.split(".", 1)[0]
        return top in cls.KEEP or top.startswith(cls.KEEP_PREFIXES)

    @classmethod
    def _reloadable(cls, module) -> bool:
        spec = getattr(module, "__spec__", None)
        loader = getattr(spec, "loader", None)
        if loader is None:
            return False
        kind = type(loader).__name__
        return kind in cls._RELOADABLE or kind.startswith("Pyi")

    def candidates(self) -> "list[str]":
        """The modules sweep() would remove now."""
        if self._baseline is None:
            return []
        added = [name for name in list(sys.modules) if name not in self._baseline]
        pinned: set[str] = set()   # top-level packages with a native part
        names = []
        for name in added:
            module = sys.modules.get(name)
            if module is None:
                continue
            if not self._reloadable(module):
                pinned.add(name.split(".", 1)[0])
                continue
            names.append(name)
        # A native module anywhere in a package pins the whole package, also
        # when that native part was already loaded before the baseline.
        for name in self._baseline:
            module = sys.modules.get(name)
            if module is not None and not self._reloadable(module) and "." in name:
                pinned.add(name.split(".", 1)[0])
        return [name for name in names
                if name.split(".", 1)[0] not in pinned and not self._kept_prefix(name)]

    def sweep(self) -> "list[str]":
        """Unload the candidates; returns their names. Call only when no
        window is open and no worker thread of theirs runs."""
        names = self.candidates()
        for name in names:
            hook = getattr(sys.modules.get(name), "__weeknumber_unload__", None)
            if callable(hook):
                try:
                    hook()
                except Exception:
                    log.exception("Unload hook of %s failed", name)
        for name in names:
            module = sys.modules.pop(name, None)
            parent, _, child = name.rpartition(".")
            # A kept parent package must not hand out the unloaded child.
            parent_module = sys.modules.get(parent) if parent else None
            if parent_module is not None and getattr(parent_module, child, None) is module:
                try:
                    delattr(parent_module, child)
                except AttributeError:
                    pass
        if names:
            self._forget_registrations({name.split(".", 1)[0] for name in names})
            log.info("Unloaded %d modules no open window needs", len(names))
        return names

    @staticmethod
    def _forget_registrations(packages: "set[str]") -> None:
        """Drop what the unloaded modules left in modules that stay.

        A module object can die while its globals live on: any function of
        it that something still references keeps the whole namespace. The
        standard library keeps such references in a few process-wide places,
        and each new import would add another copy:

        * typing's lookup caches hold ``Optional[SomeClass]`` and the like
          for every annotation a module evaluated at import (websocket-client
          has many), and its overload registry holds ``@overload`` stubs
          (truststore). Clearing them costs a few microseconds on the next
          lookup; nothing at runtime depends on their contents.
        * a library's ``logging.NullHandler``, added on every import, piles
          up on its logger, which the logging module keeps for good."""
        import logging as _logging
        import typing as _typing
        try:
            _typing.clear_overloads()
        except Exception:
            log.debug("typing.clear_overloads failed", exc_info=True)
        for clear in list(getattr(_typing, "_cleanups", ())):
            try:
                clear()
            except Exception:
                log.debug("A typing cache could not be cleared", exc_info=True)
        loggers = _logging.Logger.manager.loggerDict
        for name, logger in list(loggers.items()):
            if name.split(".", 1)[0] in packages and isinstance(logger, _logging.Logger):
                for handler in list(logger.handlers):
                    if type(handler) is _logging.NullHandler:
                        logger.removeHandler(handler)


@functools.lru_cache(maxsize=1)
def policy():
    """The organisation's IT policy (see docs/ADMINISTRATION.md), read once
    per run: administrators expect a restart to apply a change."""
    return read_policy()


# ---------------------------------------------------------------------------
# Network gate
# ---------------------------------------------------------------------------
class NetworkGate:
    """Whether WeekNumber may use the network right now.

    Closed unless the player has just agreed, in the Timezone Dominator notice, to what
    playing with others connects to; it closes again when the Timezone Dominator windows
    close, and the notice asks again the next time. Once install() has run,
    the process itself enforces this: a Python audit hook refuses every
    Internet socket operation -- DNS lookups, connects, binds, sends --
    while the gate is closed, whatever code asks for it. Local sockets
    (AF_UNIX, the macOS single-instance signal) are not network traffic and
    stay allowed.

    The refusal is a PermissionError, an OSError, so the game's network code
    reports it like any other connection failure."""

    #: Socket operations that reach (or prepare to reach) the network.
    _SOCKET_EVENTS = frozenset((
        "socket.connect", "socket.bind", "socket.sendto", "socket.sendmsg",
        "socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr",
        "socket.getnameinfo",
    ))
    #: AF_UNIX has the value 1 on Windows, macOS and Linux.
    _AF_UNIX = 1

    def __init__(self) -> None:
        self._open = False
        self._installed = False

    @property
    def is_open(self) -> bool:
        return self._open

    def allow(self) -> None:
        self._open = True
        log.info("Network access allowed for Timezone Dominator")

    def deny(self) -> None:
        if self._open:
            log.info("Network access closed")
        self._open = False

    def install(self) -> None:
        """Enforce the gate for the rest of the process. Audit hooks cannot
        be removed, so nothing can switch this off again."""
        if self._installed:
            return
        self._installed = True
        sys.addaudithook(self._audit)

    def _audit(self, event: str, args: tuple) -> None:
        # Runs for every audited event in the process: keep it cheap, and
        # import nothing here.
        if self._open or event not in self._SOCKET_EVENTS:
            return
        if event in ("socket.connect", "socket.bind", "socket.sendto", "socket.sendmsg"):
            family = getattr(args[0] if args else None, "family", None)
            if family is not None and int(family) == self._AF_UNIX:
                return
        raise PermissionError(
            "Network access is off: WeekNumber only connects while you play "
            "Timezone Dominator with others, after you allow it.")


#: The one gate for the whole app (see NetworkGate).
network = NetworkGate()


def tray_callback(func):
    """Error barrier for tray menu callbacks, which run on the tray thread
    on Windows (an uncaught exception there would only reach the log as a
    window-procedure failure). The menu calls them without arguments; the
    optional (icon, item) pair stays for callers that pass them."""
    @functools.wraps(func)
    def wrapper(self, icon=None, item=None):
        try:
            return func(self, icon, item)
        except Exception:
            log.exception("Unhandled error in %s", func.__name__)
    return wrapper



def bundled_file(name: str) -> "Path | None":
    """Locate a data file shipped with the app, or None.

    Covers all three layouts: PyInstaller --onefile (sys._MEIPASS), a loose
    file beside the .exe, and a plain source checkout.
    """
    candidates: list[Path] = []
    if getattr(sys, "frozen", False):
        meipass = getattr(sys, "_MEIPASS", None)
        if meipass:
            candidates.append(Path(meipass) / name)
        candidates.append(Path(sys.executable).resolve().parent / name)
    else:
        candidates.append(Path(__file__).resolve().parent / name)
    for path in candidates:
        try:
            if path.exists():
                return path
        except OSError:
            continue
    return None



#: Process-wide UI lookups that must survive the window modules being
#: unloaded (see ModuleSweeper): probing Tk's font families costs tens of
#: milliseconds on a machine with many fonts, and the answer never changes.
UI_CACHE: dict[str, str] = {}


def show_error(message: str) -> None:
    log.error(message)
    platform_.message_box(APP_NAME, message)


# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------

#: Exactly '#' + six hex digits. fullmatch (not int(x, 16)) is deliberate:
#: Python's int parsing also accepts '_', '+', '-' and surrounding
#: whitespace, so e.g. "#1_2345" would slip through a length+int check and
#: later crash the icon renderer during startup, outside any error barrier.
_HEX_COLOR = re.compile(r"#[0-9A-Fa-f]{6}")


def normalize_hex(value: "str | None") -> str:
    """Strict #RRGGBB validation; anything else collapses to white."""
    if not isinstance(value, str):
        return "#FFFFFF"
    value = value.strip()
    if not value.startswith("#"):
        value = "#" + value
    if _HEX_COLOR.fullmatch(value) is None:
        return "#FFFFFF"
    return value.upper()


class WeekNumber:
    def __init__(self) -> None:
        self.refresh()

    def refresh(self) -> None:
        self.last_updated = dt.datetime.now()
        self.number: int = self.last_updated.isocalendar()[1]

# ---------------------------------------------------------------------------
# Settings
# ---------------------------------------------------------------------------
class SettingsStore:
    """Bounded, type-checked JSON settings stored per user."""

    _DEFAULTS = {
        "SelectedColor": "#FFFFFF",
        "IsBold": True,
        "HolidayCountry": None,  # compatibility mirror of primary
        "HolidayCountryPrimary": None,
        "HolidayCountrySecondary": None,
        "DarkMode": False,
        "StartupInitialized": False,
        "TimeZoneEnabled": False,
        "TimeZoneKey": None,
        "SoundEnabled": True,
    }

    def __init__(self) -> None:
        self._data = dict(self._DEFAULTS)
        #: Whether a settings file was found, readable or not.
        self.file_existed = False
        self.load()

    @staticmethod
    def _country_value(value) -> "str | None":
        return value if isinstance(value, str) and len(value) == 2 else None

    def load(self) -> None:
        self.file_existed = SETTINGS_FILE.exists()
        try:
            raw = load_bounded_json(SETTINGS_FILE, MAX_SETTINGS_BYTES)
        except FileNotFoundError:
            return
        except Exception:
            log.exception("Failed to load settings; using defaults")
            return
        if not isinstance(raw, dict):
            log.warning("Settings root is not an object; using defaults")
            return

        color = raw.get("SelectedColor")
        if isinstance(color, str):
            self._data["SelectedColor"] = normalize_hex(color)
        if isinstance(raw.get("IsBold"), bool):
            self._data["IsBold"] = raw["IsBold"]

        # New settings take precedence. A valid legacy HolidayCountry migrates
        # to primary when the new key is absent.
        primary_raw = raw.get("HolidayCountryPrimary", raw.get("HolidayCountry"))
        primary = self._country_value(primary_raw)
        secondary = self._country_value(raw.get("HolidayCountrySecondary"))
        self._data["HolidayCountryPrimary"] = primary
        self._data["HolidayCountrySecondary"] = secondary
        self._data["HolidayCountry"] = primary

        for key in ("DarkMode", "StartupInitialized", "TimeZoneEnabled", "SoundEnabled"):
            if isinstance(raw.get(key), bool):
                self._data[key] = raw[key]
        # "NetworkConsent", a remembered agreement from older versions, is
        # dropped: the Timezone Dominator notice now asks every time (see NetworkGate).
        timezone_key = raw.get("TimeZoneKey")
        if timezone_key is None:
            self._data["TimeZoneKey"] = None
        elif isinstance(timezone_key, str) and len(timezone_key) <= 127:
            self._data["TimeZoneKey"] = timezone_key

    def save(self) -> None:
        try:
            SETTINGS_DIR.mkdir(parents=True, exist_ok=True)
            fd, tmp_name = tempfile.mkstemp(
                prefix="settings-", suffix=".json.tmp", dir=str(SETTINGS_DIR))
            tmp = Path(tmp_name)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as f:
                    json.dump(self._data, f, indent=2)
                    f.flush()
                    os.fsync(f.fileno())
                os.replace(tmp, SETTINGS_FILE)
            finally:
                try:
                    if tmp.exists():
                        tmp.unlink()
                except OSError:
                    pass
        except Exception:
            log.exception("Failed to save settings")

    @property
    def selected_color(self) -> str:
        return normalize_hex(self._data.get("SelectedColor"))

    @selected_color.setter
    def selected_color(self, value: str) -> None:
        self._data["SelectedColor"] = normalize_hex(value)

    @property
    def is_bold(self) -> bool:
        return bool(self._data.get("IsBold", True))

    @is_bold.setter
    def is_bold(self, value: bool) -> None:
        self._data["IsBold"] = bool(value)

    @property
    def holiday_country_primary(self) -> "str | None":
        return self._country_value(self._data.get("HolidayCountryPrimary"))

    @holiday_country_primary.setter
    def holiday_country_primary(self, value: "str | None") -> None:
        clean = self._country_value(value)
        self._data["HolidayCountryPrimary"] = clean
        self._data["HolidayCountry"] = clean

    @property
    def holiday_country_secondary(self) -> "str | None":
        return self._country_value(self._data.get("HolidayCountrySecondary"))

    @holiday_country_secondary.setter
    def holiday_country_secondary(self, value: "str | None") -> None:
        self._data["HolidayCountrySecondary"] = self._country_value(value)

    @property
    def is_dark_mode(self) -> bool:
        return bool(self._data.get("DarkMode", False))

    @is_dark_mode.setter
    def is_dark_mode(self, value: bool) -> None:
        self._data["DarkMode"] = bool(value)

    @property
    def timezone_enabled(self) -> bool:
        return bool(self._data.get("TimeZoneEnabled", False))

    @timezone_enabled.setter
    def timezone_enabled(self, value: bool) -> None:
        self._data["TimeZoneEnabled"] = bool(value)

    @property
    def timezone_key(self) -> "str | None":
        value = self._data.get("TimeZoneKey")
        return value if isinstance(value, str) else None

    @timezone_key.setter
    def timezone_key(self, value: "str | None") -> None:
        self._data["TimeZoneKey"] = value if isinstance(value, str) else None

    @property
    def startup_initialized(self) -> bool:
        return bool(self._data.get("StartupInitialized", False))

    def mark_startup_initialized(self) -> None:
        self._data["StartupInitialized"] = True
        self.save()

    @property
    def sound_enabled(self) -> bool:
        return bool(self._data.get("SoundEnabled", True))

    @sound_enabled.setter
    def sound_enabled(self, value: bool) -> None:
        self._data["SoundEnabled"] = bool(value)

    def validate_timezone_key(self, valid_keys) -> None:
        value = self.timezone_key
        if value is not None and value not in set(valid_keys):
            self._data["TimeZoneKey"] = None
            self.save()

    def validate_holiday_countries(self, valid_codes) -> None:
        valid = set(valid_codes)
        primary = self.holiday_country_primary
        secondary = self.holiday_country_secondary
        changed = False
        if primary is not None and primary not in valid:
            primary = None
            changed = True
        if secondary is not None and secondary not in valid:
            secondary = None
            changed = True
        if secondary is not None and secondary == primary:
            secondary = None
            changed = True
        if changed:
            self.holiday_country_primary = primary
            self.holiday_country_secondary = secondary
            self.save()


# ---------------------------------------------------------------------------
# Holiday data
# ---------------------------------------------------------------------------
HOLIDAY_DATA_FILE = "holiday_data.jsonl"
_MIN_HOLIDAY_YEAR = 2000
_MAX_HOLIDAY_YEAR = 2050
_MAX_HOLIDAY_INDEX_BYTES = 64 * 1024
_MAX_HOLIDAY_TABLE_BYTES = 512 * 1024
_MAX_COUNTRIES = 300
_MAX_HOLIDAYS_PER_YEAR = 366
_MAX_NAMES_PER_DATE = 16
_MAX_HOLIDAY_NAME_CHARS = 240
_COUNTRY_CODE_RE = re.compile(r"^[A-Z]{2}$")
#: What the calendar shows: only its countries, only the years on screen,
#: only while it is open (see hold_holidays).
_HOLIDAY_TABLE: dict[str, dict[int, dict[str, tuple[str, ...]]]] = {}
#: English names of the countries in _HOLIDAY_TABLE.
_HOLIDAY_COUNTRY_NAMES: dict[str, str] = {}
#: Countries whose table failed to load; not retried until release_holidays.
_HOLIDAY_FAILED: set[str] = set()


def _read_holiday_index(handle) -> "list[tuple[str, str, int, int]]":
    """Line 1 of holiday_data.jsonl, checked: (code, name, offset, length)
    of each country's line. The lengths must tile the rest of the file
    exactly, so one size check covers the layout of the whole file."""
    head = handle.readline(_MAX_HOLIDAY_INDEX_BYTES + 1)
    if len(head) > _MAX_HOLIDAY_INDEX_BYTES or not head.endswith(b"\n"):
        raise ValueError("the index line is missing or too long")
    index = json.loads(head.decode("utf-8"))
    if not isinstance(index, dict) or set(index) != {"METADATA", "COUNTRIES"}:
        raise ValueError("the index must hold exactly METADATA and COUNTRIES")
    countries = index["COUNTRIES"]
    if not isinstance(index["METADATA"], dict):
        raise ValueError("METADATA must be an object")
    if not isinstance(countries, list) or not 1 <= len(countries) <= _MAX_COUNTRIES:
        raise ValueError("country count is outside allowed bounds")

    rows: list[tuple[str, str, int, int]] = []
    seen: set[str] = set()
    offset = len(head)
    for item in countries:
        if not (isinstance(item, list) and len(item) == 3):
            raise ValueError("each country entry must be [code, English name, length]")
        code, name, length = item
        if (not isinstance(code, str) or not _COUNTRY_CODE_RE.fullmatch(code)
                or code in seen):
            raise ValueError(f"invalid or duplicate country code: {code!r}")
        if (
            not isinstance(name, str)
            or not 2 <= len(name.strip()) <= 80
            or name.strip().casefold() == code.casefold()
        ):
            raise ValueError(f"invalid full country name for {code}")
        if (not isinstance(length, int) or isinstance(length, bool)
                or not 2 <= length <= _MAX_HOLIDAY_TABLE_BYTES):
            raise ValueError(f"invalid table length for {code}")
        seen.add(code)
        rows.append((code, name.strip(), offset, length))
        offset += length
    if offset != os.fstat(handle.fileno()).st_size:
        raise ValueError("the table lengths do not add up to the file size")
    return rows


def _read_holiday_table(handle, code: str, offset: int, length: int) -> object:
    """The years of one country's line: {"code": code, "years": {...}}."""
    handle.seek(offset)
    line = handle.read(length)
    if len(line) != length or not line.endswith(b"\n"):
        raise ValueError(f"the line of {code} is cut short")
    table = json.loads(line.decode("utf-8"))
    if not isinstance(table, dict) or set(table) != {"code", "years"} \
            or table["code"] != code:
        raise ValueError(f"the line at offset {offset} is not the table of {code}")
    return table["years"]


def _parse_holiday_years(code: str, years: object,
                         keep: "set[int]") -> dict[int, dict[str, tuple[str, ...]]]:
    """One country's table, validated in full: every year 2000-2050, ISO date
    keys inside their year, one to _MAX_NAMES_PER_DATE bounded names per
    date. Only the years in `keep` are returned."""
    expected_years = {str(year) for year in range(_MIN_HOLIDAY_YEAR, _MAX_HOLIDAY_YEAR + 1)}
    if not isinstance(years, dict) or set(years) != expected_years:
        raise ValueError(f"{code} must contain every year 2000-2050")
    parsed_years: dict[int, dict[str, tuple[str, ...]]] = {}
    for year_text, days in years.items():
        year = int(year_text)
        if not isinstance(days, dict) or len(days) > _MAX_HOLIDAYS_PER_YEAR:
            raise ValueError(f"invalid holiday map for {code} {year}")
        parsed_days: dict[str, tuple[str, ...]] = {}
        for date_text, names in days.items():
            if not isinstance(date_text, str) or len(date_text) != 10:
                raise ValueError(f"invalid date key for {code} {year}")
            try:
                holiday_date = dt.date.fromisoformat(date_text)
            except ValueError as exc:
                raise ValueError(f"invalid ISO date {date_text}") from exc
            if holiday_date.year != year:
                raise ValueError(f"{date_text} is outside its stated year")
            if isinstance(names, str):
                names = [names]  # backward-compatible source data
            if (not isinstance(names, list)
                    or not 1 <= len(names) <= _MAX_NAMES_PER_DATE):
                raise ValueError(f"invalid names for {code} {date_text}")
            clean: list[str] = []
            for name in names:
                if not isinstance(name, str) or not 1 <= len(name.strip()) <= _MAX_HOLIDAY_NAME_CHARS:
                    raise ValueError(f"invalid holiday name for {code} {date_text}")
                value = name.strip()
                if value not in clean:
                    clean.append(value)
            parsed_days[date_text] = tuple(clean)
        if year in keep:
            parsed_years[year] = parsed_days
    return parsed_years


def holiday_countries() -> list[tuple[str, str]]:
    """Every country with holiday data as (code, English name), sorted by
    name: read from the file's index line on each call (the Holidays dialog,
    the startup check) and not kept. Empty when the file is unusable."""
    try:
        path = bundled_file(HOLIDAY_DATA_FILE)
        if path is None:
            log.info("%s not found; holiday support disabled", HOLIDAY_DATA_FILE)
            return []
        with path.open("rb") as handle:
            rows = _read_holiday_index(handle)
    except Exception:
        log.exception("Failed to validate %s; holiday support disabled", HOLIDAY_DATA_FILE)
        return []
    return sorted(((code, name) for code, name, _offset, _length in rows),
                  key=lambda item: item[1].casefold())


def hold_holidays(countries: "Iterable[str | None]", years: "Iterable[int]") -> None:
    """Keep exactly `years` of `countries` in memory and drop the rest.

    The calendar calls this on every redraw with the years on screen -- one,
    or two in December -- so no whole table is ever held. A redraw that needs
    nothing new reads nothing; a new year or country seeks to that country's
    line, about a millisecond to read and validate."""
    wanted = {code for code in countries if code}
    keep = {year for year in years if _MIN_HOLIDAY_YEAR <= year <= _MAX_HOLIDAY_YEAR}
    for code in list(_HOLIDAY_TABLE):
        if code not in wanted:
            del _HOLIDAY_TABLE[code]
            _HOLIDAY_COUNTRY_NAMES.pop(code, None)
    for table in _HOLIDAY_TABLE.values():
        for year in list(table):
            if year not in keep:
                del table[year]
    missing = sorted(code for code in wanted - _HOLIDAY_FAILED
                     if not keep <= _HOLIDAY_TABLE.get(code, {}).keys())
    if not missing:
        return
    try:
        path = bundled_file(HOLIDAY_DATA_FILE)
        if path is None:
            log.info("%s not found; holiday support disabled", HOLIDAY_DATA_FILE)
            _HOLIDAY_FAILED.update(missing)
            return
        with path.open("rb") as handle:
            rows = {code: (name, offset, length)
                    for code, name, offset, length in _read_holiday_index(handle)}
            for code in missing:
                try:
                    name, offset, length = rows[code]
                    table = _parse_holiday_years(
                        code, _read_holiday_table(handle, code, offset, length), keep)
                except Exception:
                    log.exception("Failed to load the holidays of %s", code)
                    _HOLIDAY_FAILED.add(code)
                    _HOLIDAY_TABLE.pop(code, None)
                    _HOLIDAY_COUNTRY_NAMES.pop(code, None)
                    continue
                _HOLIDAY_TABLE[code] = table
                _HOLIDAY_COUNTRY_NAMES[code] = name
    except Exception:
        log.exception("Failed to validate %s; holiday support disabled", HOLIDAY_DATA_FILE)
        _HOLIDAY_FAILED.update(missing)


def release_holidays() -> None:
    """Forget every held table (the calendar closed)."""
    _HOLIDAY_TABLE.clear()
    _HOLIDAY_COUNTRY_NAMES.clear()
    _HOLIDAY_FAILED.clear()


def holiday_names(country: "str | None", on_date: dt.date) -> tuple[str, ...]:
    if not country:
        return ()
    return _HOLIDAY_TABLE.get(country, {}).get(on_date.year, {}).get(
        on_date.isoformat(), ())


def holiday_country_name(country: "str | None") -> "str | None":
    """The English name of a held country (see hold_holidays)."""
    return _HOLIDAY_COUNTRY_NAMES.get(country) if country else None
