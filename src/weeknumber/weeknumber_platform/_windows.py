"""Windows backend: Win32 interop for DPI, window chrome, the single-instance
mutex, the eviction event and the Run-key startup entry.

* Per-Monitor V2 DPI awareness is enabled before Tk starts; the calendar
  popup sizes fonts/paddings in physical pixels for the monitor it appears
  on, so it renders sharp on every monitor.
* The Run registry key is the single source of truth for "run at startup".
  StartupApproved is only written on an explicit user enable, so a
  "Disabled" choice made in Task Manager is respected.
* The DLL search path is restricted to System32 before any native extension
  loads (see the hardening block below), so a DLL planted next to the .exe in
  a writable directory cannot be loaded in place of the genuine system copy.
"""

import ctypes

# Harden the DLL search path to System32 BEFORE importing anything that loads
# native extensions or their dependent DLLs (tkinter -> Tcl/Tk, Pillow,
# pystray, hashlib -> OpenSSL). This removes the loading directory, the current
# working directory and PATH from the default search, so a DLL planted next to
# the .exe in a writable directory cannot be loaded in place of the genuine
# system copy. kernel32 is a KnownDLL -- section-mapped, never path-searched --
# so loading it by name here is itself safe. No-op before Win8 (no KB2533623).
# Note: this cannot protect the PyInstaller bootloader's own loads, which run
# before any of this code; sign the binary and install it to an ACL-protected
# directory to cover that surface.
LOAD_LIBRARY_SEARCH_SYSTEM32 = 0x00000800
try:
    ctypes.WinDLL("kernel32", use_last_error=True).SetDefaultDllDirectories(
        LOAD_LIBRARY_SEARCH_SYSTEM32)
except (OSError, AttributeError):
    pass

import hashlib
import os
import sys
import threading
import time
import winreg
from ctypes import wintypes
from pathlib import Path

from weeknumber_platform._common import (
    POLICY_KEY,
    PolicyStore,
    RunStore,
    containing_root,
    disable_startup,
    enable_startup,
    launch_argv,
    log,
    migrate_legacy,
    parse_policy,
    quote_command,
    refresh_startup,
    startup_enabled,
    temp_dir,
)

__all__ = [
    "LOAD_LIBRARY_SEARCH_SYSTEM32", "kernel32", "user32",
    "POINT", "RECT",
    "CONTEXT_MENU_EVENTS", "ICO_WINDOW_ICONS", "UI_FONT_FAMILIES",
    "settings_dir", "enable_dpi_awareness", "system_dpi",
    "cursor_monitor_geometry", "tray_icon_size", "tray_edge",
    "set_rounded_corners", "set_dark_titlebar", "toplevel_hwnd", "frame_hwnd",
    "owns_foreground", "bring_app_to_front", "prepare_app", "message_box",
    "tray_font_files", "SingleInstance", "QuitSignal",
    "set_startup", "is_startup_enabled", "refresh_startup_path",
    "GLASS_BACKDROP", "attach_glass_backdrop",
    "read_policy", "request_attention", "reduce_motion", "play_sound", "open_file",
    "trim_memory", "trim_working_set", "set_app_identity", "apply_window_icon",
]

#: Right-click on every Windows mouse.
CONTEXT_MENU_EVENTS = ("<Button-3>",)
#: Tk on Windows takes .ico files for window icons directly.
ICO_WINDOW_ICONS = True
#: "Segoe UI Variable Text" is the Win11 body-optical face; plain "Segoe UI"
#: is the Win10 fallback. Never hard-code the Variable name without probing:
#: on systems lacking it Tk silently substitutes an arbitrary default, which
#: looks worse than Segoe UI.
UI_FONT_FAMILIES = ("Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI")


def settings_dir() -> Path:
    return Path(os.getenv("APPDATA", str(Path.home()))) / "WeekNumberPy"


HKCU = winreg.HKEY_CURRENT_USER
RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
STARTUP_APPROVED_KEY = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"


def _safe_object_tag() -> str:
    """Per-user/per-install tag for named Win32 objects.

    Static object names are easy for another same-session process to pre-create
    or signal. This tag does not authenticate callers by itself, but it removes
    the predictable global name and separates different users/install paths.
    """
    try:
        # Seeded from weeknumber_core.py's path in source mode, as before the
        # platform split, so the object names stay stable across the upgrade.
        launch = (sys.executable if getattr(sys, "frozen", False)
                  else Path(__file__).resolve().parents[1] / "weeknumber_core.py")
        seed = "|".join((
            os.environ.get("USERDOMAIN", ""),
            os.environ.get("USERNAME", ""),
            str(Path.home()),
            str(Path(launch).resolve()),
        ))
    except Exception:
        seed = f"{os.getpid()}|{time.time()}"
    return hashlib.sha256(seed.encode("utf-8", "ignore")).hexdigest()[:16]


_OBJECT_TAG = _safe_object_tag()
MUTEX_NAME = f"Local\\WeekNumber_{_OBJECT_TAG}_SingleInstance"
#: Auto-reset event a launching instance sets to evict the running one.
QUIT_EVENT_NAME = f"Local\\WeekNumber_{_OBJECT_TAG}_QuitEvent"


# ---------------------------------------------------------------------------
# Win32 interop
# ---------------------------------------------------------------------------
# user32/kernel32/gdi32/shell32/ole32 are KnownDLLs (section-mapped, immune to
# planting). dwmapi and shcore are NOT KnownDLLs, so pin them to System32 with
# an explicit load flag -- redundant with the process-wide
# SetDefaultDllDirectories above, but robust to any dependency that mutates the
# default search (SetDllDirectory/AddDllDirectory) before these loads run.
user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
shell32 = ctypes.WinDLL("shell32", use_last_error=True)
ole32 = ctypes.WinDLL("ole32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi", use_last_error=True, winmode=LOAD_LIBRARY_SEARCH_SYSTEM32)
try:
    shcore = ctypes.WinDLL("shcore", use_last_error=True, winmode=LOAD_LIBRARY_SEARCH_SYSTEM32)
except OSError:
    shcore = None


class POINT(ctypes.Structure):
    _fields_ = (("x", ctypes.c_long), ("y", ctypes.c_long))


class RECT(ctypes.Structure):
    _fields_ = (
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    )


class MONITORINFO(ctypes.Structure):
    _fields_ = (
        ("cbSize", wintypes.DWORD),
        ("rcMonitor", RECT),
        ("rcWork", RECT),
        ("dwFlags", wintypes.DWORD),
    )


class GUID(ctypes.Structure):
    """Win32 GUID, built from its "{...}" string form via ole32.IIDFromString."""
    _fields_ = (
        ("Data1", wintypes.DWORD),
        ("Data2", wintypes.WORD),
        ("Data3", wintypes.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    )

    def __init__(self, guid: str) -> None:
        super().__init__()
        # S_OK is 0. On failure the GUID stays zeroed; callers observe the
        # subsequent shell lookup failing and fall back. Log it anyway --
        # a malformed constant is a programming error worth surfacing.
        if ole32.IIDFromString(guid, ctypes.byref(self)) != 0:
            log.warning("IIDFromString rejected GUID literal %r", guid)


user32.GetCursorPos.argtypes = (ctypes.POINTER(POINT),)
user32.GetCursorPos.restype = wintypes.BOOL
user32.MonitorFromPoint.argtypes = (POINT, wintypes.DWORD)
user32.MonitorFromPoint.restype = ctypes.c_void_p
user32.GetMonitorInfoW.argtypes = (ctypes.c_void_p, ctypes.POINTER(MONITORINFO))
user32.GetMonitorInfoW.restype = wintypes.BOOL
user32.GetForegroundWindow.restype = ctypes.c_void_p
user32.GetParent.argtypes = (ctypes.c_void_p,)
user32.GetParent.restype = ctypes.c_void_p
user32.GetDC.restype = ctypes.c_void_p
user32.ReleaseDC.argtypes = (ctypes.c_void_p, ctypes.c_void_p)
user32.MessageBoxW.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_wchar_p, wintypes.UINT)
user32.SystemParametersInfoW.argtypes = (wintypes.UINT, wintypes.UINT, ctypes.c_void_p, wintypes.UINT)
kernel32.CreateMutexW.argtypes = (ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR)
kernel32.CreateMutexW.restype = ctypes.c_void_p
kernel32.CloseHandle.argtypes = (ctypes.c_void_p,)
kernel32.CreateEventW.argtypes = (ctypes.c_void_p, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR)
kernel32.CreateEventW.restype = ctypes.c_void_p
kernel32.SetEvent.argtypes = (ctypes.c_void_p,)
kernel32.SetEvent.restype = wintypes.BOOL
kernel32.WaitForSingleObject.argtypes = (ctypes.c_void_p, wintypes.DWORD)
kernel32.WaitForSingleObject.restype = wintypes.DWORD
kernel32.GetWindowsDirectoryW.argtypes = (ctypes.c_wchar_p, wintypes.UINT)
kernel32.GetWindowsDirectoryW.restype = wintypes.UINT
kernel32.GetProcessHeap.restype = ctypes.c_void_p
kernel32.HeapCompact.argtypes = (ctypes.c_void_p, wintypes.DWORD)
kernel32.HeapCompact.restype = ctypes.c_size_t
kernel32.GetCurrentProcess.restype = ctypes.c_void_p
kernel32.SetProcessWorkingSetSize.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t)
kernel32.SetProcessWorkingSetSize.restype = wintypes.BOOL
user32.LoadImageW.argtypes = (ctypes.c_void_p, wintypes.LPCWSTR, wintypes.UINT,
                              ctypes.c_int, ctypes.c_int, wintypes.UINT)
user32.LoadImageW.restype = ctypes.c_void_p
user32.SendMessageW.argtypes = (ctypes.c_void_p, wintypes.UINT, ctypes.c_size_t, ctypes.c_ssize_t)
user32.SendMessageW.restype = ctypes.c_ssize_t
dwmapi.DwmSetWindowAttribute.argtypes = (ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD)
dwmapi.DwmSetWindowAttribute.restype = ctypes.c_long
ole32.IIDFromString.argtypes = (wintypes.LPCWSTR, ctypes.POINTER(GUID))
ole32.IIDFromString.restype = ctypes.c_long
ole32.CoTaskMemFree.argtypes = (ctypes.c_void_p,)
shell32.SHGetKnownFolderPath.argtypes = (
    ctypes.POINTER(GUID), wintypes.DWORD, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p))
shell32.SHGetKnownFolderPath.restype = ctypes.c_long

MONITOR_DEFAULTTONEAREST = 2
MDT_EFFECTIVE_DPI = 0
SPI_GETWORKAREA = 48
SM_CXSMICON = 49
LOGPIXELSX = 88
DWMWA_USE_IMMERSIVE_DARK_MODE = 20
#: Pre-20H1 builds carried the same flag at attribute 19.
DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1 = 19
DWMWA_WINDOW_CORNER_PREFERENCE = 33
DWMWCP_ROUND = 2
DWMWCP_ROUNDSMALL = 3
ERROR_ALREADY_EXISTS = 183
WAIT_OBJECT_0 = 0
INFINITE = 0xFFFFFFFF
MB_ICONERROR = 0x10


def enable_dpi_awareness() -> None:
    """Per-Monitor V2 -> Per-Monitor -> System. Must run before Tk is
    created, otherwise Windows bitmap-stretches the UI on scaled monitors."""
    try:
        if user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4)):
            return
    except (AttributeError, OSError):
        pass
    try:
        if shcore is not None and shcore.SetProcessDpiAwareness(2) == 0:
            return
    except OSError:
        pass
    try:
        user32.SetProcessDPIAware()
    except OSError:
        pass


def system_dpi() -> int:
    try:
        return int(user32.GetDpiForSystem())  # Win10 1607+
    except (AttributeError, OSError):
        pass
    try:
        hdc = user32.GetDC(None)
        try:
            return int(gdi32.GetDeviceCaps(hdc, LOGPIXELSX))
        finally:
            user32.ReleaseDC(None, hdc)
    except OSError:
        return 96


def windows_directory() -> Path:
    """Return the real Windows directory without trusting WINDIR."""
    buf = ctypes.create_unicode_buffer(32768)
    n = kernel32.GetWindowsDirectoryW(buf, len(buf))
    if 0 < n < len(buf):
        return Path(buf.value)
    return Path(r"C:\Windows")


def tray_font_files(bold: bool) -> "list[Path]":
    """TrueType candidates for the tray digits, best first."""
    fonts_dir = windows_directory() / "Fonts"
    names = ("segoeuib.ttf", "arialbd.ttf") if bold else ("segoeui.ttf", "arial.ttf")
    return [fonts_dir / name for name in names]


# Known Folders we refuse to autostart from. Resolved via the shell so
# redirected (OneDrive/Group Policy) Desktop and Downloads are handled --
# Path.home()/"Downloads" is simply the wrong path on the many systems where
# those folders are redirected, which would let the guardrail pass silently.
FOLDERID_DESKTOP = "{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}"
FOLDERID_DOWNLOADS = "{374DE290-123F-4565-9164-39C4925E467B}"
FOLDERID_PUBLIC = "{DFDF76A2-C82A-4D63-906A-5644AC457385}"


def known_folder_path(folder_id: str) -> "Path | None":
    """Resolve a Known Folder to its real filesystem path, honouring
    redirection. Returns None if the shell lookup fails."""
    ptr = ctypes.c_void_p()
    try:
        hr = shell32.SHGetKnownFolderPath(
            ctypes.byref(GUID(folder_id)), 0, None, ctypes.byref(ptr))
        if hr != 0 or not ptr.value:
            return None
        return Path(ctypes.wstring_at(ptr))
    except OSError:
        return None
    finally:
        if ptr.value:
            ole32.CoTaskMemFree(ptr)


def monitor_scale(hmonitor) -> float:
    if shcore is not None:
        try:
            x, y = ctypes.c_uint(), ctypes.c_uint()
            if shcore.GetDpiForMonitor(ctypes.c_void_p(hmonitor), MDT_EFFECTIVE_DPI,
                                       ctypes.byref(x), ctypes.byref(y)) == 0:
                return x.value / 96.0
        except OSError:
            pass
    return system_dpi() / 96.0


def cursor_monitor_geometry() -> tuple[POINT, RECT, RECT, float]:
    """(cursor, monitor rect, work-area rect, scale) for the monitor under
    the cursor -- not the primary monitor."""
    pt = POINT()
    user32.GetCursorPos(ctypes.byref(pt))
    hmon = user32.MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    mi = MONITORINFO()
    mi.cbSize = ctypes.sizeof(MONITORINFO)
    if hmon and user32.GetMonitorInfoW(hmon, ctypes.byref(mi)):
        return pt, mi.rcMonitor, mi.rcWork, monitor_scale(hmon)
    work = RECT()
    user32.SystemParametersInfoW(SPI_GETWORKAREA, 0, ctypes.byref(work), 0)
    mon = RECT(0, 0, user32.GetSystemMetrics(0), user32.GetSystemMetrics(1))
    return pt, mon, work, system_dpi() / 96.0


def tray_icon_size() -> int:
    """Physical pixel size the shell draws tray icons at (SM_CXSMICON)."""
    size = user32.GetSystemMetrics(SM_CXSMICON)
    if 8 <= size <= 256:
        return size
    return max(16, min(64, round(16 * system_dpi() / 96.0)))


def tray_edge(monitor: RECT, work: RECT) -> str:
    """The taskbar lives on the side where the work area is inset."""
    gaps = {
        "left": work.left - monitor.left,
        "top": work.top - monitor.top,
        "right": monitor.right - work.right,
        "bottom": monitor.bottom - work.bottom,
    }
    return max(gaps, key=gaps.get) if max(gaps.values()) > 0 else "bottom"


def set_rounded_corners(hwnd, small: bool = False) -> None:
    """Ask DWM to round a top-level window. `small` selects the tighter
    radius Win11 uses for tooltips/flyout-adjacent surfaces."""
    try:
        pref = ctypes.c_int(DWMWCP_ROUNDSMALL if small else DWMWCP_ROUND)
        dwmapi.DwmSetWindowAttribute(ctypes.c_void_p(hwnd), DWMWA_WINDOW_CORNER_PREFERENCE,
                                     ctypes.byref(pref), ctypes.sizeof(pref))
    except OSError:
        pass  # pre-Win11: square corners, no harm


def set_dark_titlebar(hwnd, dark: bool) -> None:
    """Ask DWM to draw a window's caption in dark mode, so a titled dialog
    matches the app's own dark palette instead of pairing a dark body with a
    white caption bar. Attribute 20 is the documented flag; builds between
    1809 and 1903 used 19. No-op on Windows 10 builds older than that."""
    value = ctypes.c_int(1 if dark else 0)
    for attribute in (DWMWA_USE_IMMERSIVE_DARK_MODE,
                      DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1):
        try:
            if dwmapi.DwmSetWindowAttribute(ctypes.c_void_p(hwnd), attribute,
                                            ctypes.byref(value),
                                            ctypes.sizeof(value)) == 0:
                return
        except OSError:
            return


def toplevel_hwnd(top) -> int:
    """Native HWND for a Tk toplevel. Tk's winfo_id is the client child; for
    an overrideredirect window GetParent yields the actual top-level frame
    (falling back to the child when there is none)."""
    child = top.winfo_id()
    return user32.GetParent(child) or child


def frame_hwnd(top) -> int:
    """HWND of a decorated frame. `wm frame` is Tk's documented accessor
    for it; toplevel_hwnd() is the overrideredirect-window helper and
    resolves an owner, not a caption frame, for a normal dialog."""
    try:
        return int(top.wm_frame(), 16)
    except (Exception, ValueError):
        return toplevel_hwnd(top)


def owns_foreground(hwnd) -> bool:
    """True while `hwnd` is the foreground window."""
    return user32.GetForegroundWindow() == hwnd


def bring_app_to_front() -> None:
    """Windows raises per window (see the dialogs' _raise); nothing to do."""


def prepare_app(root) -> None:
    """Process-level setup once the Tk root exists; nothing on Windows."""


#: The calendar flyout keeps its opaque Fluent surface on Windows.
GLASS_BACKDROP = False


def attach_glass_backdrop(top, radius: float, dark: bool):
    """No system glass on Windows; the flyout stays opaque."""
    return None


class _RegistryPolicyStore(PolicyStore):
    """Group Policy values: HKLM wins over HKCU, and only the expected value
    type counts. Standard users cannot write either Policies key."""

    def _value(self, name: str, wanted: int):
        for hive in (winreg.HKEY_LOCAL_MACHINE, winreg.HKEY_CURRENT_USER):
            try:
                with winreg.OpenKey(hive, POLICY_KEY) as key:
                    value, value_type = winreg.QueryValueEx(key, name)
            except OSError:
                continue
            if value_type == wanted:
                return value
        return None

    def flag(self, name: str) -> object:
        return self._value(name, winreg.REG_DWORD)

    def text(self, name: str) -> "str | None":
        value = self._value(name, winreg.REG_SZ)
        return value if isinstance(value, str) else None


def read_policy():
    return parse_policy(_RegistryPolicyStore())


class FLASHWINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.UINT), ("hwnd", wintypes.HWND),
                ("dwFlags", wintypes.DWORD), ("uCount", wintypes.UINT),
                ("dwTimeout", wintypes.DWORD)]


FLASHW_TRAY = 0x2
FLASHW_TIMERNOFG = 0xC
SPI_GETCLIENTAREAANIMATION = 0x1042


def request_attention(top) -> None:
    """Flash the window's taskbar button until it is brought forward."""
    try:
        info = FLASHWINFO(ctypes.sizeof(FLASHWINFO), frame_hwnd(top),
                          FLASHW_TRAY | FLASHW_TIMERNOFG, 0, 0)
        user32.FlashWindowEx(ctypes.byref(info))
    except Exception:
        log.debug("FlashWindowEx failed", exc_info=True)


def reduce_motion() -> bool:
    """Windows' "Animation effects" switched off (Accessibility)."""
    enabled = wintypes.BOOL(True)
    try:
        if user32.SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0,
                                        ctypes.byref(enabled), 0):
            return not enabled.value
    except Exception:
        pass
    return False


def play_sound(wav: bytes) -> None:
    """Play an in-memory WAV without blocking. winsound cannot play memory
    asynchronously, so a short-lived thread waits for it instead; a new
    sound replaces one still playing."""
    import winsound

    def play() -> None:
        try:
            winsound.PlaySound(wav, winsound.SND_MEMORY | winsound.SND_NODEFAULT)
        except Exception:
            log.debug("PlaySound failed", exc_info=True)

    threading.Thread(target=play, name="weeknumber-sound", daemon=True).start()


def trim_memory() -> None:
    """Give free memory back after windows close: the C runtime allocates
    from the process heap, and HeapCompact decommits its large free blocks."""
    try:
        kernel32.HeapCompact(kernel32.GetProcessHeap(), 0)
    except Exception:
        log.debug("HeapCompact failed", exc_info=True)


_TRIM_ALL = ctypes.c_size_t(-1).value


def trim_working_set() -> None:
    """Release the physical pages of an idle app (no window open).

    Only called once the last window has closed and its modules are gone, so
    what leaves the working set is mostly memory nothing touches until the
    next window: Tcl's and Python's allocator caches, startup-only code. The
    pages stay on the system's standby list, so the next click soft-faults
    the few it needs back in a few milliseconds; under memory pressure the
    system can reuse them instead, which is the point for a tray utility that
    runs all day. Commit size is unchanged; this is what Windows itself does
    to idle processes, only sooner."""
    try:
        kernel32.SetProcessWorkingSetSize(kernel32.GetCurrentProcess(), _TRIM_ALL, _TRIM_ALL)
    except Exception:
        log.debug("SetProcessWorkingSetSize failed", exc_info=True)


#: An explicit AppUserModelID gives a source run its own taskbar group, which
#: is what makes Windows show the window's WeekNumber icon instead of
#: python.exe's. The frozen exe keeps the default ID derived from its path, so
#: a pinned WeekNumber.exe still matches its running windows.
APP_USER_MODEL_ID = "Shanko100.WeekNumber"


def set_app_identity() -> None:
    if getattr(sys, "frozen", False):
        return
    try:
        shell32.SetCurrentProcessExplicitAppUserModelID.argtypes = (wintypes.LPCWSTR,)
        shell32.SetCurrentProcessExplicitAppUserModelID.restype = ctypes.c_long
        shell32.SetCurrentProcessExplicitAppUserModelID(APP_USER_MODEL_ID)
    except Exception:
        log.debug("SetCurrentProcessExplicitAppUserModelID failed", exc_info=True)


WM_SETICON = 0x0080
ICON_SMALL = 0
ICON_BIG = 1
IMAGE_ICON = 1
LR_LOADFROMFILE = 0x0010
SM_CXICON = 11
#: HICONs loaded from app.ico, by pixel size: a handful for the whole run
#: (one or two sizes per monitor DPI), shared by every window.
_window_icons: "dict[int, int]" = {}


def _icon_sizes(hwnd) -> "tuple[int, int]":
    """(small, big) window-icon sizes for the DPI of the monitor the window
    is on: the caption's icon and the one the taskbar and Alt+Tab draw."""
    dpi = 0
    try:
        dpi = int(user32.GetDpiForWindow(ctypes.c_void_p(hwnd)))  # Win10 1607+
    except (AttributeError, OSError):
        pass
    if not 48 <= dpi <= 960:
        dpi = system_dpi()
    try:
        small = int(user32.GetSystemMetricsForDpi(SM_CXSMICON, dpi))
        big = int(user32.GetSystemMetricsForDpi(SM_CXICON, dpi))
    except (AttributeError, OSError):
        small, big = 0, 0
    if not 8 <= small <= 256:
        small = round(16 * dpi / 96)
    if not 16 <= big <= 256:
        big = round(32 * dpi / 96)
    return small, big


def apply_window_icon(top, path: "Path | None") -> None:
    """Give a titled window app.ico frames that match its monitor's DPI.

    Tk 8.6 hands Windows the .ico's 16 and 32 px frames whatever the scaling
    (GetIcon in tk/win/tkWinWm.c; recheck when Tk changes), so at 125 % and
    above the taskbar and Alt+Tab upscale a 32 px icon and it looks blurred.
    LoadImage at the exact size chooses the matching frame -- app.ico carries
    one for every common scale -- and WM_SETICON hands it over 1:1."""
    if path is None:
        return
    try:
        hwnd = frame_hwnd(top)
        if not hwnd:
            return
        for which, size in zip((ICON_SMALL, ICON_BIG), _icon_sizes(hwnd)):
            handle = _window_icons.get(size)
            if handle is None:
                handle = user32.LoadImageW(None, str(path), IMAGE_ICON, size, size,
                                           LR_LOADFROMFILE)
                if not handle:
                    continue
                _window_icons[size] = handle
            user32.SendMessageW(ctypes.c_void_p(hwnd), WM_SETICON, which, handle)
    except Exception:
        log.debug("Could not set the window icon", exc_info=True)


def open_file(path: Path) -> bool:
    """Open a file in its default app (Notepad for text)."""
    try:
        os.startfile(str(path))
        return True
    except OSError:
        log.debug("Could not open %s", path, exc_info=True)
        return False


def message_box(title: str, message: str) -> None:
    try:
        user32.MessageBoxW(None, message, title, MB_ICONERROR)
    except OSError:
        pass


class SingleInstance:
    """Named-mutex single-instance guard. Unlike a localhost port bind, it
    cannot collide with unrelated software and has no network surface."""

    def __init__(self, name: str = MUTEX_NAME) -> None:
        self._name = name
        self._handle = None

    def acquire(self) -> bool:
        self._handle = kernel32.CreateMutexW(None, False, self._name)
        if not self._handle:
            log.warning("CreateMutexW failed (err=%d); continuing without guard",
                        ctypes.get_last_error())
            return True  # fail open: a broken guard must not brick the app
        if ctypes.get_last_error() == ERROR_ALREADY_EXISTS:
            self.release()
            return False
        return True

    def release(self) -> None:
        if self._handle:
            kernel32.CloseHandle(self._handle)
            self._handle = None


class QuitSignal:
    """Named auto-reset event used to hand control to a newly launched copy.

    The launcher calls signal() to wake the incumbent; the incumbent runs
    host(callback) on a daemon thread that fires `callback` (a graceful
    shutdown) when woken. Keyed on a name -- not a PID or image name -- so it
    behaves identically whether the process is python.exe under VS Code or the
    frozen .exe, and it never reaches into another process the way an external
    kill would (which is exactly what endpoint protection flags).

    Invariant: the incumbent both holds the event handle AND waits on it (see
    host()/_wait). If a future change ever holds the handle without an active
    waiter, a stale auto-reset signal would survive and shut down the next
    instance at startup -- keep the hold and the wait together."""

    def __init__(self, name: str = QUIT_EVENT_NAME) -> None:
        self._name = name
        self._handle = None
        self._thread: threading.Thread | None = None

    def signal(self) -> bool:
        """Wake a running instance. CreateEventW opens the existing named
        event (or creates it if we got here first); SetEvent stays latched on
        an auto-reset event until a waiter consumes it, so the signal is never
        lost to a race with the incumbent's host() call."""
        handle = kernel32.CreateEventW(None, False, False, self._name)
        if not handle:
            log.warning("CreateEventW (signal) failed (err=%d)", ctypes.get_last_error())
            return False
        try:
            return bool(kernel32.SetEvent(handle))
        finally:
            kernel32.CloseHandle(handle)

    def host(self, on_quit) -> None:
        """Incumbent side: block on the event off-thread, run on_quit when set."""
        self._handle = kernel32.CreateEventW(None, False, False, self._name)
        if not self._handle:
            log.warning("CreateEventW (host) failed (err=%d); eviction disabled",
                        ctypes.get_last_error())
            return
        self._thread = threading.Thread(
            target=self._wait, args=(on_quit,), name="weeknumber-evict", daemon=True)
        self._thread.start()

    def _wait(self, on_quit) -> None:
        if kernel32.WaitForSingleObject(self._handle, INFINITE) == WAIT_OBJECT_0:
            log.info("Eviction requested by a new instance; shutting down")
            try:
                on_quit()
            except Exception:
                log.exception("Eviction shutdown callback failed")

    def release(self) -> None:
        # Wake our own waiter so the daemon thread unblocks and ends, then close.
        if self._handle:
            kernel32.SetEvent(self._handle)
            kernel32.CloseHandle(self._handle)
            self._handle = None


# ---------------------------------------------------------------------------
# Run-at-startup: HKCU\...\Run
# ---------------------------------------------------------------------------
# The decisions (which value, when to repoint, Task Manager state) live in
# _common so they are tested on every OS; this is the winreg plumbing and the
# location policy, which needs Win32.
FOLDERID_SKYDRIVE = "{A52BBA46-E9E1-435F-B3D9-28DAA648C0F6}"
DRIVE_REMOVABLE, DRIVE_REMOTE, DRIVE_CDROM = 2, 4, 5
kernel32.GetDriveTypeW.argtypes = (wintypes.LPCWSTR,)
kernel32.GetDriveTypeW.restype = wintypes.UINT

#: Program Files first: only administrators can change files there, so
#: nothing else can swap the exe. The per-user folder is the fallback for
#: people who can't install software; OneDrive never syncs it.
_PROGRAM_FILES = os.environ.get("ProgramFiles", r"C:\Program Files")
_MOVE_HINT = (f" Move WeekNumber to {_PROGRAM_FILES}\\WeekNumber (needs administrator "
              "rights), or to %LOCALAPPDATA%\\Programs\\WeekNumber if you can't install "
              "software, then turn Run at Startup on again.")


class _WinregStore(RunStore):
    """RunStore over the current user's registry."""

    @staticmethod
    def _get(path: str, name: str):
        try:
            with winreg.OpenKey(HKCU, path) as key:
                return winreg.QueryValueEx(key, name)
        except OSError:
            return None

    @staticmethod
    def _set(path: str, name: str, kind: int, value) -> None:
        with winreg.CreateKeyEx(HKCU, path, 0, winreg.KEY_SET_VALUE) as key:
            winreg.SetValueEx(key, name, 0, kind, value)

    @staticmethod
    def _delete(path: str, name: str) -> None:
        try:
            with winreg.OpenKey(HKCU, path, 0, winreg.KEY_SET_VALUE) as key:
                winreg.DeleteValue(key, name)
        except FileNotFoundError:
            pass

    def run_get(self, name):
        found = self._get(RUN_KEY, name)
        # Anything but REG_SZ (e.g. a tampered REG_EXPAND_SZ) counts as foreign.
        return found[0] if found and found[1] == winreg.REG_SZ and isinstance(found[0], str) else None

    def run_set(self, name, command):
        self._set(RUN_KEY, name, winreg.REG_SZ, command)

    def run_delete(self, name):
        self._delete(RUN_KEY, name)

    def approved_get(self, name):
        found = self._get(STARTUP_APPROVED_KEY, name)
        return found[0] if found and found[1] == winreg.REG_BINARY else None

    def approved_set(self, name, data):
        self._set(STARTUP_APPROVED_KEY, name, winreg.REG_BINARY, data)

    def approved_delete(self, name):
        self._delete(STARTUP_APPROVED_KEY, name)


_STORE = _WinregStore()


def _launch_parts() -> "list[Path] | None":
    """launch_argv, but with the console-less pythonw.exe in source mode so
    no console window opens at every logon."""
    argv = launch_argv()
    if argv is None or len(argv) == 1:
        return argv
    windowless = argv[0].with_name("pythonw.exe")
    if argv[0].name.lower() == "python.exe" and windowless.exists():
        argv = [windowless] + argv[1:]
    return argv


def _resolved_launch_command() -> str | None:
    """Resolved, quoted command for the Run value."""
    argv = _launch_parts()
    return quote_command(argv) if argv is not None else None


def _risky_roots() -> "list[tuple[Path, str]]":
    """Folders whose contents other people, other devices or a reboot can
    change -- autostarting from them hands that power the user's logon."""
    roots: list[tuple[Path, str]] = [(temp_dir(), "a temporary folder")]
    for folder, label in ((FOLDERID_DOWNLOADS, "your Downloads folder"),
                          (FOLDERID_DESKTOP, "your Desktop"),
                          (FOLDERID_PUBLIC, "the shared Public folder"),
                          (FOLDERID_SKYDRIVE, "a OneDrive folder")):
        path = known_folder_path(folder)
        if path is not None:
            roots.append((path, label))
    for variable in ("OneDrive", "OneDriveCommercial", "OneDriveConsumer"):
        if os.environ.get(variable):
            roots.append((Path(os.environ[variable]), "a OneDrive folder"))
    roots += [(Path.home() / "Downloads", "your Downloads folder"),
              (Path.home() / "Desktop", "your Desktop"),
              (Path.home() / "Dropbox", "a Dropbox folder"),
              (Path.home() / "Google Drive", "a Google Drive folder"),
              (Path(os.environ.get("ProgramData", r"C:\ProgramData")),
               "ProgramData, which every user can write to"),
              (windows_directory() / "Temp", "the Windows Temp folder")]
    return roots


def _location_problem(path: Path) -> "str | None":
    text = str(path)
    if text.startswith("\\\\"):
        return "WeekNumber is on a network share." + _MOVE_HINT
    drive = kernel32.GetDriveTypeW(path.anchor) if path.anchor else 0
    if drive == DRIVE_REMOTE:
        return "WeekNumber is on a network drive." + _MOVE_HINT
    if drive in (DRIVE_REMOVABLE, DRIVE_CDROM):
        return "WeekNumber is on a removable drive." + _MOVE_HINT
    label = containing_root(path, _risky_roots())
    if label is not None:
        return f"WeekNumber is in {label}." + _MOVE_HINT
    return None


def _startup_location_problem() -> "str | None":
    """Why autostart from here is refused, or None. Best-effort guardrail,
    not a substitute for signing and installing to an ACL-protected
    directory. Source mode checks the interpreter as well as the script."""
    argv = _launch_parts()
    if argv is None:
        return "WeekNumber could not determine where it is installed."  # fail closed
    for part in argv:
        problem = _location_problem(part)
        if problem is not None:
            return problem
    return None


def set_startup(enable: bool) -> "str | None":
    """Returns None on success, otherwise a message to show the user."""
    try:
        if not enable:
            disable_startup(_STORE)
            return None
        return enable_startup(_STORE, _resolved_launch_command(),
                              _startup_location_problem())
    except OSError:
        log.exception("Failed to update the startup entry")
        return "Windows refused to update the startup entry; see the log for details."


def is_startup_enabled() -> bool:
    return startup_enabled(_STORE, _launch_parts())


def refresh_startup_path() -> None:
    """At launch: adopt an earlier build's entry that launches this app, and repair
    our entry after the app moved. Never repoints to a different copy and
    never re-enables an entry the user disabled."""
    try:
        argv = _launch_parts()
        migrate_legacy(_STORE, argv)
        command = quote_command(argv) if argv is not None else None
        refresh_startup(_STORE, command, _startup_location_problem())
    except OSError:
        log.exception("refresh_startup_path failed")
