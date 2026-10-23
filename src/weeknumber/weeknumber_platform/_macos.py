"""macOS backend.

* Geometry comes from AppKit (``NSScreen``/``NSEvent``) and is converted to
  Tk's coordinate space: points, origin at the primary screen's top-left.
  Tk on Aqua already works in points, so the UI scale is 1.0 and Retina
  backing is handled by the toolkit.
* The single-instance guard is an advisory ``flock`` on a file in the settings
  directory -- released by the kernel when the process dies, so a crash never
  leaves a stale lock behind.
* The eviction signal is a user-only (0600) AF_UNIX socket next to it. Like
  the Windows named event it has no network surface.
* "Run at startup" is a per-user LaunchAgent plist. Its presence is the single
  source of truth, the way the Run value is on Windows.

AppKit is called through the small ctypes bridge in ``_cocoa``, not PyObjC,
which cost about 20 MB of memory for these few calls. Loading the framework
connects to no window server, so tests without one can import this module.
"""

import ctypes
import fcntl
import os
import plistlib
import socket
import tempfile
import threading
from ctypes import c_bool, c_double, c_long, c_ulong
from pathlib import Path

from weeknumber_platform import _cocoa as cocoa
from weeknumber_platform._cocoa import c_id
from weeknumber_platform._common import (
    POLICY_DOMAIN,
    Policy,
    PolicyStore,
    containing_root,
    launch_argv,
    launch_location,
    log,
    parse_policy,
    temp_dir,
)

__all__ = [
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
    "LAUNCH_AGENT_LABEL", "launch_agent_path",
]

#: Aqua reports the secondary button as Button-2; Control-click is the
#: one-button-mouse and trackpad equivalent.
CONTEXT_MENU_EVENTS = ("<Button-2>", "<Control-Button-1>")
#: Tk on Aqua cannot read .ico; an accessory app shows no window icons anyway.
ICO_WINDOW_ICONS = False
#: Tk exposes the system face under these names depending on the macOS and Tk
#: release; ui_font_family() falls back to TkDefaultFont's family.
UI_FONT_FAMILIES = ("SF Pro Text", "SF Pro", ".AppleSystemUIFont", "Helvetica Neue")

LAUNCH_AGENT_LABEL = "io.github.shanko100.weeknumber"


class POINT:
    __slots__ = ("x", "y")

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = int(x)
        self.y = int(y)


class RECT:
    __slots__ = ("left", "top", "right", "bottom")

    def __init__(self, left: int = 0, top: int = 0, right: int = 0,
                 bottom: int = 0) -> None:
        self.left = int(left)
        self.top = int(top)
        self.right = int(right)
        self.bottom = int(bottom)


def settings_dir() -> Path:
    return Path.home() / "Library" / "Application Support" / "WeekNumber"


def enable_dpi_awareness() -> None:
    """Aqua is always resolution-independent; nothing to opt into.

    This is also the app's one call before Tk starts AppKit, so it turns off
    window restoration: a menu-bar app has no windows to reopen, and after a
    crash AppKit would otherwise block startup on a modal "reopen windows?"
    alert inside Tk_Init."""
    try:
        with cocoa.autorelease_pool():
            defaults = cocoa.send(cocoa.cls("NSUserDefaults"), "standardUserDefaults")
            ignore = cocoa.send(cocoa.cls("NSNumber"), "numberWithBool:", True,
                                argtypes=(c_bool,))
            registration = cocoa.send(
                cocoa.cls("NSDictionary"), "dictionaryWithObject:forKey:",
                ignore, cocoa.nsstring("ApplePersistenceIgnoreState"),
                argtypes=(c_id, c_id))
            cocoa.send(defaults, "registerDefaults:", registration,
                       restype=None, argtypes=(c_id,))
    except Exception:
        log.warning("Could not turn off window restoration")


def system_dpi() -> int:
    return 72


def _app() -> int:
    return cocoa.send(cocoa.cls("NSApplication"), "sharedApplication")


def _to_tk_rect(frame, primary_height: float) -> RECT:
    """Cocoa rect (bottom-left origin, y up) -> Tk rect (top-left, y down)."""
    x, y = frame.origin.x, frame.origin.y
    w, h = frame.size.width, frame.size.height
    return RECT(round(x), round(primary_height - (y + h)),
                round(x + w), round(primary_height - y))


def _frame(window_or_screen, selector: str = "frame") -> "cocoa.NSRect":
    return cocoa.send(window_or_screen, selector, restype=cocoa.NSRect)


def cursor_monitor_geometry() -> "tuple[POINT, RECT, RECT, float]":
    """(cursor, screen rect, visible rect, scale) for the screen under the
    pointer, in Tk coordinates."""
    fallback = (POINT(0, 0), RECT(0, 0, 1440, 900), RECT(0, 25, 1440, 900), 1.0)
    try:
        with cocoa.autorelease_pool():
            screens = cocoa.items(cocoa.send(cocoa.cls("NSScreen"), "screens"))
            if not screens:
                return fallback
            frames = [_frame(screen) for screen in screens]
            primary_height = frames[0].size.height
            mouse = cocoa.send(cocoa.cls("NSEvent"), "mouseLocation", restype=cocoa.NSPoint)
            chosen = 0
            for index, f in enumerate(frames):
                if (f.origin.x <= mouse.x <= f.origin.x + f.size.width
                        and f.origin.y <= mouse.y <= f.origin.y + f.size.height):
                    chosen = index
                    break
            cursor = POINT(round(mouse.x), round(primary_height - mouse.y))
            return (cursor, _to_tk_rect(frames[chosen], primary_height),
                    _to_tk_rect(_frame(screens[chosen], "visibleFrame"), primary_height),
                    1.0)
    except Exception:
        log.exception("Screen geometry lookup failed")
        return fallback


def tray_icon_size() -> int:
    """Menu-bar extra height in points."""
    return 22


def tray_edge(monitor: RECT, work: RECT) -> str:
    """Menu-bar extras always sit on the top edge. The work-area heuristic
    Windows uses would pick the Dock, which is usually the larger inset."""
    return "top"


def set_rounded_corners(hwnd, small: bool = False) -> None:
    """Borderless Aqua windows are square; nothing to request."""


def set_dark_titlebar(window, dark: bool) -> None:
    """Match the title bar to the app palette (Tk 8.6.10+ on Aqua)."""
    try:
        window.tk.call("::tk::unsupported::MacWindowStyle", "appearance",
                       window._w, "darkaqua" if dark else "aqua")
    except Exception:
        pass  # older Tk: the system appearance is used, no harm


def toplevel_hwnd(top):
    """Aqua APIs here take the Tk toplevel itself as the window handle."""
    return top


def frame_hwnd(top):
    return top


def owns_foreground(window) -> bool:
    """True while WeekNumber is the active application. Per-window focus is
    covered by the popup's own <FocusOut> binding."""
    try:
        return bool(cocoa.send(_app(), "isActive", restype=c_bool))
    except Exception:
        return True


def bring_app_to_front() -> None:
    """A menu-bar click does not activate the app, so a window opened from it
    would appear behind the frontmost application without this."""
    try:
        cocoa.send(_app(), "activateIgnoringOtherApps:", True,
                   restype=None, argtypes=(c_bool,))
    except Exception:
        pass


def prepare_app(root) -> None:
    """Run as a menu-bar accessory: no Dock icon, no app menu. The frozen app
    sets LSUIElement in Info.plist; this covers running from source."""
    try:
        cocoa.send(_app(), "setActivationPolicy:", _ACTIVATION_POLICY_ACCESSORY,
                   restype=c_bool, argtypes=(c_long,))
    except Exception:
        log.exception("Could not switch to the accessory activation policy")


# AppKit enumeration values used below.
_ACTIVATION_POLICY_ACCESSORY = 1
_STYLE_BORDERLESS = 0
_STYLE_NONACTIVATING_PANEL = 1 << 7
_BACKING_BUFFERED = 2
_WINDOW_BELOW = -1
_IMAGE_SCALE_AXES_INDEPENDENTLY = 1
_VIEW_WIDTH_SIZABLE = 1 << 1
_VIEW_HEIGHT_SIZABLE = 1 << 4
_MATERIAL_POPOVER = 6
_BLENDING_BEHIND_WINDOW = 0
_STATE_ACTIVE = 1


# ---------------------------------------------------------------------------
# Liquid Glass backdrop for floating panels
# ---------------------------------------------------------------------------
#: Floating panels (the menu-bar calendar) can sit on system glass.
GLASS_BACKDROP = bool(cocoa.cls("NSGlassEffectView") or cocoa.cls("NSVisualEffectView"))


def _ns_window_for(top) -> "int | None":
    """The NSWindow behind a mapped Tk toplevel, found by its frame. Tk 8.6
    exports no Python-reachable accessor, and frames are unique enough: the
    match must agree on position and size to within a point."""
    top.update_idletasks()
    x, y = top.winfo_rootx(), top.winfo_rooty()
    w, h = top.winfo_width(), top.winfo_height()
    with cocoa.autorelease_pool():
        screens = cocoa.items(cocoa.send(cocoa.cls("NSScreen"), "screens"))
        if not screens:
            return None
        primary_height = _frame(screens[0]).size.height
        for window in cocoa.items(cocoa.send(_app(), "windows")):
            frame = _frame(window)
            if (abs(frame.origin.x - x) <= 1 and abs(frame.size.width - w) <= 1
                    and abs(frame.size.height - h) <= 1
                    and abs(primary_height - (frame.origin.y + frame.size.height) - y) <= 1):
                return window  # Tk owns it and keeps it alive while mapped
    return None


def _png(image) -> bytes:
    import io
    buffer = io.BytesIO()
    image.save(buffer, "PNG", compress_level=1)
    return buffer.getvalue()


def _ns_image(png: bytes, width: float, height: float) -> "int | None":
    """An owned NSImage of `png`, drawn at `width` x `height` points."""
    image = cocoa.new("NSImage", "initWithData:", cocoa.nsdata(png), argtypes=(c_id,))
    if image:
        cocoa.send(image, "setSize:", cocoa.NSSize(width, height),
                   restype=None, argtypes=(cocoa.NSSize,))
    return image


class GlassBackdrop:
    """A borderless, click-through child panel holding the system glass,
    kept exactly behind a transparent Tk window. Tk keeps drawing its own
    content view untouched; the glass only lives in the window below it,
    which also supplies the rounded corners and the shadow."""

    def __init__(self, window: int, panel: int) -> None:
        self._window = window       # Tk's; not ours to release
        self._panel = panel         # ours: allocated in attach_glass_backdrop
        self._image_view = None     # held by the panel's view hierarchy

    def sync(self) -> None:
        """Follow the Tk window after it moved or resized."""
        try:
            cocoa.send(self._panel, "setFrame:display:", _frame(self._window), True,
                       restype=None, argtypes=(cocoa.NSRect, c_bool))
        except Exception:
            log.exception("Glass backdrop sync failed")

    def set_dark(self, dark: bool) -> None:
        with cocoa.autorelease_pool():
            name = cocoa.constant("NSAppearanceNameDarkAqua" if dark else "NSAppearanceNameAqua")
            appearance = cocoa.send(cocoa.cls("NSAppearance"), "appearanceNamed:", name,
                                    argtypes=(c_id,))
            cocoa.send(self._panel, "setAppearance:", appearance,
                       restype=None, argtypes=(c_id,))

    def set_overlay(self, image, width: float, height: float) -> None:
        """Show a PIL RGBA image natively inside the glass, stretched to
        `width` x `height` points: Retina-sharp when the image carries more
        pixels than points. Tk cannot draw images on a transparent window,
        so artwork that must sit on the glass is drawn here, under Tk's text."""
        try:
            with cocoa.autorelease_pool():
                ns_image = _ns_image(_png(image), width, height)
                if not ns_image:
                    return
                if self._image_view is None:
                    content = cocoa.send(self._panel, "contentView")
                    view = cocoa.new("NSImageView", "initWithFrame:",
                                     _frame(content, "bounds"), argtypes=(cocoa.NSRect,))
                    cocoa.send(view, "setImageScaling:", _IMAGE_SCALE_AXES_INDEPENDENTLY,
                               restype=None, argtypes=(c_ulong,))
                    cocoa.send(view, "setAutoresizingMask:",
                               _VIEW_WIDTH_SIZABLE | _VIEW_HEIGHT_SIZABLE,
                               restype=None, argtypes=(c_ulong,))
                    if cocoa.responds(content, "setContentView:"):  # NSGlassEffectView
                        cocoa.send(content, "setContentView:", view,
                                   restype=None, argtypes=(c_id,))
                    else:
                        cocoa.send(content, "addSubview:", view,
                                   restype=None, argtypes=(c_id,))
                    cocoa.release(view)  # the glass view holds it now
                    self._image_view = view
                cocoa.send(self._image_view, "setImage:", ns_image,
                           restype=None, argtypes=(c_id,))
                cocoa.release(ns_image)
        except Exception:
            log.exception("Glass overlay update failed")

    def close(self) -> None:
        panel, self._panel = self._panel, None
        if not panel:
            return
        try:
            cocoa.send(self._window, "removeChildWindow:", panel,
                       restype=None, argtypes=(c_id,))
            cocoa.send(panel, "orderOut:", None, restype=None, argtypes=(c_id,))
            cocoa.send(panel, "close", restype=None)
        except Exception:
            pass
        cocoa.release(panel)
        self._image_view = None


def _rounded_mask(width: float, height: float, radius: float) -> "int | None":
    """An owned mask image that clips a vibrancy view to rounded corners (the
    documented way; a layer corner radius leaves the shadow square). Drawn
    at 2x for Retina; the cap insets keep the corners when it stretches."""
    from PIL import Image, ImageDraw
    scale = 2
    mask = Image.new("RGBA", (max(1, round(width * scale)), max(1, round(height * scale))))
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, mask.width - 1, mask.height - 1), radius=radius * scale, fill=(0, 0, 0, 255))
    image = _ns_image(_png(mask), width, height)
    if image:
        cocoa.send(image, "setCapInsets:", cocoa.NSEdgeInsets(radius, radius, radius, radius),
                   restype=None, argtypes=(cocoa.NSEdgeInsets,))
    return image


def attach_glass_backdrop(top, radius: float, dark: bool) -> "GlassBackdrop | None":
    """Put system glass behind a mapped, transparent Tk toplevel.

    NSGlassEffectView (Liquid Glass, macOS 26+) when available, otherwise an
    NSVisualEffectView popover material. Returns None when anything is
    missing, so the caller can fall back to an opaque surface."""
    try:
        window = _ns_window_for(top)
        if window is None:
            return None
        with cocoa.autorelease_pool():
            frame = _frame(window)
            panel = cocoa.new(
                "NSPanel", "initWithContentRect:styleMask:backing:defer:",
                frame, _STYLE_BORDERLESS | _STYLE_NONACTIVATING_PANEL, _BACKING_BUFFERED, False,
                argtypes=(cocoa.NSRect, c_ulong, c_ulong, c_bool))
            send = cocoa.send
            send(panel, "setOpaque:", False, restype=None, argtypes=(c_bool,))
            send(panel, "setBackgroundColor:", send(cocoa.cls("NSColor"), "clearColor"),
                 restype=None, argtypes=(c_id,))
            send(panel, "setHasShadow:", True, restype=None, argtypes=(c_bool,))
            send(panel, "setIgnoresMouseEvents:", True, restype=None, argtypes=(c_bool,))
            send(panel, "setReleasedWhenClosed:", False, restype=None, argtypes=(c_bool,))
            bounds = cocoa.NSRect(cocoa.NSPoint(0, 0), frame.size)
            if cocoa.cls("NSGlassEffectView"):
                view = cocoa.new("NSGlassEffectView", "initWithFrame:", bounds,
                                 argtypes=(cocoa.NSRect,))
                send(view, "setCornerRadius:", float(radius),
                     restype=None, argtypes=(c_double,))
            else:
                view = cocoa.new("NSVisualEffectView", "initWithFrame:", bounds,
                                 argtypes=(cocoa.NSRect,))
                send(view, "setMaterial:", _MATERIAL_POPOVER, restype=None, argtypes=(c_long,))
                send(view, "setBlendingMode:", _BLENDING_BEHIND_WINDOW,
                     restype=None, argtypes=(c_long,))
                send(view, "setState:", _STATE_ACTIVE, restype=None, argtypes=(c_long,))
                mask = _rounded_mask(frame.size.width, frame.size.height, radius)
                send(view, "setMaskImage:", mask, restype=None, argtypes=(c_id,))
                cocoa.release(mask)
            send(view, "setAutoresizingMask:", _VIEW_WIDTH_SIZABLE | _VIEW_HEIGHT_SIZABLE,
                 restype=None, argtypes=(c_ulong,))
            send(panel, "setContentView:", view, restype=None, argtypes=(c_id,))
            cocoa.release(view)  # the panel holds it now
            backdrop = GlassBackdrop(window, panel)
            backdrop.set_dark(dark)
            # Transparent Tk pixels would otherwise pass clicks to the app below
            # and close the flyout; the glass behind is click-through on purpose.
            send(window, "setIgnoresMouseEvents:", False, restype=None, argtypes=(c_bool,))
            send(window, "addChildWindow:ordered:", panel, _WINDOW_BELOW,
                 restype=None, argtypes=(c_id, c_long))
            send(panel, "orderFront:", None, restype=None, argtypes=(c_id,))
            send(window, "orderFront:", None, restype=None, argtypes=(c_id,))
            return backdrop
    except Exception:
        log.exception("Could not attach the glass backdrop")
        return None


def request_attention(top) -> None:
    """Bring the window in front of other apps' windows without taking the
    focus: an accessory app has no Dock icon to bounce."""
    try:
        window = _ns_window_for(top)
        if window is not None:
            cocoa.send(window, "orderFrontRegardless", restype=None)
    except Exception:
        log.debug("orderFrontRegardless failed", exc_info=True)


def _workspace() -> int:
    return cocoa.send(cocoa.cls("NSWorkspace"), "sharedWorkspace")


def reduce_motion() -> bool:
    """System Settings > Accessibility > Display > Reduce motion."""
    try:
        return bool(cocoa.send(_workspace(), "accessibilityDisplayShouldReduceMotion",
                               restype=c_bool))
    except Exception:
        return False


#: Sounds still playing, owned here: NSSound stops when it is deallocated.
_PLAYING: "list[int]" = []


def _forget_finished_sounds() -> None:
    playing = []
    for sound in _PLAYING:
        if cocoa.send(sound, "isPlaying", restype=c_bool):
            playing.append(sound)
        else:
            cocoa.release(sound)
    _PLAYING[:] = playing


def play_sound(wav: bytes) -> None:
    """Play an in-memory WAV without blocking."""
    try:
        with cocoa.autorelease_pool():
            sound = cocoa.new("NSSound", "initWithData:", cocoa.nsdata(wav), argtypes=(c_id,))
            if not sound:
                return
            _forget_finished_sounds()
            _PLAYING.append(sound)
            cocoa.send(sound, "play", restype=c_bool)
    except Exception:
        log.debug("NSSound failed", exc_info=True)


def trim_working_set() -> None:
    """Nothing beyond trim_memory: macOS has no call that pages a process
    out, and malloc_zone_pressure_relief already returned the free pages."""


def set_app_identity() -> None:
    """Nothing to do: the menu-bar accessory has no Dock or taskbar entry."""


def apply_window_icon(top, path) -> None:
    """Nothing to do: an accessory app's windows carry no icon on macOS."""


def trim_memory() -> None:
    """Give free memory back after windows close: drop finished sounds, then
    ask every malloc zone to return its free pages to the system -- what
    memory pressure would make it do, only sooner."""
    try:
        _forget_finished_sounds()
    except Exception:
        log.debug("Could not release finished sounds", exc_info=True)
    try:
        relief = ctypes.CDLL("/usr/lib/libSystem.B.dylib").malloc_zone_pressure_relief
        relief.argtypes = (ctypes.c_void_p, ctypes.c_size_t)
        relief.restype = ctypes.c_size_t
        relief(None, 0)  # every zone, as much as it can
    except Exception:
        log.debug("malloc_zone_pressure_relief failed", exc_info=True)


def open_file(path: Path) -> bool:
    """Open a file in its default app (TextEdit for text)."""
    try:
        with cocoa.autorelease_pool():
            url = cocoa.send(cocoa.cls("NSURL"), "fileURLWithPath:",
                             cocoa.nsstring(str(path)), argtypes=(c_id,))
            return bool(cocoa.send(_workspace(), "openURL:", url,
                                   restype=c_bool, argtypes=(c_id,)))
    except Exception:
        log.debug("Could not open %s", path, exc_info=True)
        return False


class _ManagedPolicyStore(PolicyStore):
    """Managed preferences for the app's domain, read by explicit domain so a
    source run reads the same values as the bundle. Switches only ever turn
    things off, so any source counts; the relay address must be enforced by
    a configuration profile (MDM)."""

    def flag(self, name: str) -> object:
        return cocoa.preference(name, POLICY_DOMAIN)

    def text(self, name: str) -> "str | None":
        if not cocoa.preference_forced(name, POLICY_DOMAIN):
            return None
        value = cocoa.preference(name, POLICY_DOMAIN)
        return value if isinstance(value, str) else None


def read_policy() -> Policy:
    try:
        return parse_policy(_ManagedPolicyStore())
    except Exception:
        log.warning("Managed preferences are unavailable; no IT policy applies")
        return Policy()


def message_box(title: str, message: str) -> None:
    try:
        import tkinter as tk
        from tkinter import messagebox
    except Exception:
        return
    temporary = None
    try:
        parent = getattr(tk, "_default_root", None)
        if parent is None:
            temporary = parent = tk.Tk()
            temporary.withdraw()
        messagebox.showerror(title, message, parent=parent)
    except Exception:
        pass
    finally:
        if temporary is not None:
            try:
                temporary.destroy()
            except Exception:
                pass


def tray_font_files(bold: bool) -> "list[Path]":
    supplemental = Path("/System/Library/Fonts/Supplemental")
    if bold:
        return [supplemental / "Arial Bold.ttf", Path("/Library/Fonts/Arial Bold.ttf")]
    return [Path("/System/Library/Fonts/SFNS.ttf"), supplemental / "Arial.ttf"]


def _private_dir() -> Path:
    directory = settings_dir()
    directory.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(directory, 0o700)
    except OSError:
        pass
    return directory


class SingleInstance:
    """Advisory-lock single-instance guard. The kernel drops the lock with the
    process, so there is nothing stale to clean up after a crash."""

    def __init__(self, path: "Path | None" = None) -> None:
        self._path = path
        self._fd: "int | None" = None

    def acquire(self) -> bool:
        try:
            path = self._path or _private_dir() / "instance.lock"
            self._fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
        except OSError:
            log.exception("Could not open the instance lock; continuing without guard")
            return True  # fail open: a broken guard must not brick the app
        try:
            fcntl.flock(self._fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.release()
            return False
        return True

    def release(self) -> None:
        if self._fd is not None:
            try:
                os.close(self._fd)  # closing drops the flock
            except OSError:
                pass
            self._fd = None


class QuitSignal:
    """User-only AF_UNIX socket used to hand control to a newly launched copy.

    The incumbent listens; signal() connects and sends one fixed token. Only
    the instance holding SingleInstance ever binds, so a leftover socket file
    from a crash is safe to unlink before binding."""

    TOKEN = b"weeknumber-quit\n"
    POLL_S = 0.5

    def __init__(self, path: "Path | None" = None) -> None:
        self._path = path
        self._server: "socket.socket | None" = None
        self._stop = threading.Event()
        self._thread: "threading.Thread | None" = None

    #: sockaddr_un.sun_path is 104 bytes on macOS, terminator included.
    MAX_SOCKET_PATH = 103

    def _socket_path(self) -> Path:
        if self._path is not None:
            return self._path
        path = _private_dir() / "quit.sock"
        if len(os.fsencode(path)) > self.MAX_SOCKET_PATH:
            # A very long home path: macOS's $TMPDIR is per-user and 0700.
            path = temp_dir() / "weeknumber-quit.sock"
        return path

    def signal(self) -> bool:
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.settimeout(2.0)
                client.connect(str(self._socket_path()))
                client.sendall(self.TOKEN)
            return True
        except OSError:
            log.warning("Could not reach the running instance's quit socket")
            return False

    def host(self, on_quit) -> None:
        path = self._socket_path()
        try:
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            old_umask = os.umask(0o177)
            try:
                server.bind(str(path))
            finally:
                os.umask(old_umask)
            server.listen(2)
            server.settimeout(self.POLL_S)
        except OSError:
            log.exception("Could not create the quit socket; eviction disabled")
            return
        self._server = server
        self._thread = threading.Thread(
            target=self._wait, args=(on_quit,), name="weeknumber-evict", daemon=True)
        self._thread.start()

    def _wait(self, on_quit) -> None:
        server = self._server
        while server is not None and not self._stop.is_set():
            try:
                conn, _ = server.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with conn:
                try:
                    conn.settimeout(1.0)
                    payload = conn.recv(len(self.TOKEN))
                except OSError:
                    continue
            if payload == self.TOKEN:
                log.info("Eviction requested by a new instance; shutting down")
                try:
                    on_quit()
                except Exception:
                    log.exception("Eviction shutdown callback failed")
                return

    def release(self) -> None:
        self._stop.set()
        if self._server is not None:
            try:
                self._server.close()
            except OSError:
                pass
            self._server = None
            try:
                self._socket_path().unlink()
            except OSError:
                pass


# ---------------------------------------------------------------------------
# Run at login: ~/Library/LaunchAgents
# ---------------------------------------------------------------------------
def launch_agent_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{LAUNCH_AGENT_LABEL}.plist"


_MOVE_HINT = " Move WeekNumber.app to Applications first, then enable Run at Startup again."


def _startup_location_problem() -> "str | None":
    """Why autostart from here is refused, or None: drop locations (Temp,
    Downloads, Desktop), a mounted disk image, or a Gatekeeper App
    Translocation copy (which lives under the temp tree and vanishes on
    reboot anyway)."""
    launch = launch_location()
    if launch is None:
        return "WeekNumber could not determine where it is installed."
    if str(launch).startswith("/Volumes/"):
        return "WeekNumber is running from a disk image or external drive." + _MOVE_HINT
    if "/AppTranslocation/" in str(launch):
        return "macOS is running WeekNumber from a temporary copy." + _MOVE_HINT
    label = containing_root(launch, [
        (temp_dir(), "a temporary folder"), (Path("/private/tmp"), "a temporary folder"),
        (Path("/tmp"), "a temporary folder"),
        (Path.home() / "Downloads", "your Downloads folder"),
        (Path.home() / "Desktop", "your Desktop")])
    return f"WeekNumber is in {label}." + _MOVE_HINT if label else None


def _agent_document(argv: "list[Path]") -> dict:
    return {
        "Label": LAUNCH_AGENT_LABEL,
        "ProgramArguments": [str(part) for part in argv],
        "RunAtLoad": True,
        "ProcessType": "Interactive",
    }


def _read_agent() -> "dict | None":
    try:
        with launch_agent_path().open("rb") as handle:
            data = plistlib.loads(handle.read(64 * 1024))
    except (OSError, plistlib.InvalidFileException, ValueError):
        return None
    return data if isinstance(data, dict) else None


def _write_agent(document: dict) -> None:
    path = launch_agent_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix="weeknumber-", suffix=".plist.tmp",
                                    dir=str(path.parent))
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "wb") as handle:
            plistlib.dump(document, handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, path)
    finally:
        try:
            if tmp.exists():
                tmp.unlink()
        except OSError:
            pass


def set_startup(enable: bool) -> "str | None":
    """Returns None on success, otherwise a message to show the user. Takes
    effect at the next login; nothing is loaded into launchd now, because
    the app is already running."""
    try:
        if not enable:
            try:
                launch_agent_path().unlink()
            except FileNotFoundError:
                pass
            return None
        problem = _startup_location_problem()
        if problem is not None:
            return problem
        argv = launch_argv()
        if argv is None:
            return "WeekNumber could not determine where it is installed."
        _write_agent(_agent_document(argv))
        return None
    except OSError:
        log.exception("Failed to update the LaunchAgent")
        return "macOS refused to update the login item; see the log for details."


def is_startup_enabled() -> bool:
    agent = _read_agent()
    return bool(agent) and agent.get("Label") == LAUNCH_AGENT_LABEL


def refresh_startup_path() -> None:
    """Repoint an existing LaunchAgent after the app has moved."""
    agent = _read_agent()
    if agent is None:
        return
    argv = launch_argv()
    if argv is None:
        return
    wanted = _agent_document(argv)
    if agent == wanted:
        return
    if _startup_location_problem() is not None:
        log.warning("Refusing to repoint startup entry to a high-risk location")
        return
    try:
        _write_agent(wanted)
        log.info("Startup path updated to %s", " ".join(wanted["ProgramArguments"]))
    except OSError:
        log.exception("refresh_startup_path failed")
