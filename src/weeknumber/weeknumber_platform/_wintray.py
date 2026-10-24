"""The Windows notification-area icon, over the Win32 API.

WeekNumber's own replacement for pystray, doing only what the tray needs and
behaving as pystray did: a left click runs the default menu entry, a right
click opens the menu at the cursor, the tooltip shows the week, and the icon
comes back when Explorer restarts (TaskbarCreated) or the display changes.

* One background thread owns a hidden top-level window and its message loop;
  menu actions run on that thread, exactly as they did under pystray, and the
  tray hands Tk work to the Tk thread itself (Application.run_on_ui).
* The icon is built in memory with CreateIconIndirect from a 32-bit BGRA DIB
  at the image's exact pixel size, so the shell draws it 1:1 with no
  resampling. Nothing is written to disk (pystray went through a temporary
  .ico file).
* The menu is rebuilt each time it opens, so checkmarks always show the
  current state.
* Only the callback message, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE and
  TaskbarCreated are handled; anything else goes to DefWindowProcW. A menu
  command is accepted only if it is the index of an entry of the menu that
  was just shown.

Never imported on macOS; weeknumber_tray imports it on Windows only.
"""

from __future__ import annotations

import ctypes
import os
import threading
from collections.abc import Callable, Sequence
from ctypes import wintypes

from weeknumber_platform._common import log

__all__ = ["NotifyIcon"]

user32 = ctypes.WinDLL("user32", use_last_error=True)
shell32 = ctypes.WinDLL("shell32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

LRESULT = ctypes.c_ssize_t
WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT,
                             wintypes.WPARAM, wintypes.LPARAM)

WM_DESTROY = 0x0002
WM_CLOSE = 0x0010
WM_NULL = 0x0000
WM_DISPLAYCHANGE = 0x007E
WM_LBUTTONUP = 0x0202
WM_RBUTTONUP = 0x0205
WM_APP = 0x8000
#: The shell's callback message for our icon.
WM_TRAY = WM_APP + 1

WS_POPUP = 0x80000000
NIM_ADD, NIM_MODIFY, NIM_DELETE = 0, 1, 2
NIF_MESSAGE, NIF_ICON, NIF_TIP = 0x1, 0x2, 0x4
MF_STRING, MF_SEPARATOR, MF_CHECKED = 0x0000, 0x0800, 0x0008
TPM_RIGHTALIGN, TPM_BOTTOMALIGN, TPM_RETURNCMD = 0x0008, 0x0020, 0x0100
MSGFLT_ALLOW = 1
BI_BITFIELDS = 3
DIB_RGB_COLORS = 0
#: szTip holds 128 characters including the terminator.
MAX_TIP_CHARS = 127
#: Icon edge lengths accepted (IconRenderer clamps to the same range).
MIN_ICON_PX, MAX_ICON_PX = 16, 256
#: How long run_detached waits for the window, and stop for the thread.
START_TIMEOUT_S = 10.0
STOP_TIMEOUT_S = 5.0


class GUID(ctypes.Structure):
    _fields_ = [("Data1", wintypes.DWORD), ("Data2", wintypes.WORD),
                ("Data3", wintypes.WORD), ("Data4", wintypes.BYTE * 8)]


class NOTIFYICONDATAW(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.DWORD), ("hWnd", wintypes.HWND), ("uID", wintypes.UINT),
        ("uFlags", wintypes.UINT), ("uCallbackMessage", wintypes.UINT),
        ("hIcon", wintypes.HICON), ("szTip", wintypes.WCHAR * 128),
        ("dwState", wintypes.DWORD), ("dwStateMask", wintypes.DWORD),
        ("szInfo", wintypes.WCHAR * 256), ("uVersion", wintypes.UINT),
        ("szInfoTitle", wintypes.WCHAR * 64), ("dwInfoFlags", wintypes.DWORD),
        ("guidItem", GUID), ("hBalloonIcon", wintypes.HICON),
    ]


class WNDCLASSEXW(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.UINT), ("style", wintypes.UINT), ("lpfnWndProc", WNDPROC),
        ("cbClsExtra", ctypes.c_int), ("cbWndExtra", ctypes.c_int),
        ("hInstance", wintypes.HINSTANCE), ("hIcon", wintypes.HICON),
        ("hCursor", wintypes.HANDLE), ("hbrBackground", wintypes.HBRUSH),
        ("lpszMenuName", wintypes.LPCWSTR), ("lpszClassName", wintypes.LPCWSTR),
        ("hIconSm", wintypes.HICON),
    ]


class ICONINFO(ctypes.Structure):
    _fields_ = [("fIcon", wintypes.BOOL), ("xHotspot", wintypes.DWORD),
                ("yHotspot", wintypes.DWORD), ("hbmMask", wintypes.HBITMAP),
                ("hbmColor", wintypes.HBITMAP)]


class BITMAPV5HEADER(ctypes.Structure):
    _fields_ = [
        ("bV5Size", wintypes.DWORD), ("bV5Width", wintypes.LONG),
        ("bV5Height", wintypes.LONG), ("bV5Planes", wintypes.WORD),
        ("bV5BitCount", wintypes.WORD), ("bV5Compression", wintypes.DWORD),
        ("bV5SizeImage", wintypes.DWORD), ("bV5XPelsPerMeter", wintypes.LONG),
        ("bV5YPelsPerMeter", wintypes.LONG), ("bV5ClrUsed", wintypes.DWORD),
        ("bV5ClrImportant", wintypes.DWORD), ("bV5RedMask", wintypes.DWORD),
        ("bV5GreenMask", wintypes.DWORD), ("bV5BlueMask", wintypes.DWORD),
        ("bV5AlphaMask", wintypes.DWORD), ("bV5CSType", wintypes.DWORD),
        ("bV5Endpoints", wintypes.LONG * 9), ("bV5GammaRed", wintypes.DWORD),
        ("bV5GammaGreen", wintypes.DWORD), ("bV5GammaBlue", wintypes.DWORD),
        ("bV5Intent", wintypes.DWORD), ("bV5ProfileData", wintypes.DWORD),
        ("bV5ProfileSize", wintypes.DWORD), ("bV5Reserved", wintypes.DWORD),
    ]


def _bind(dll, name: str, argtypes: tuple, restype) -> Callable:
    function = getattr(dll, name)
    function.argtypes = argtypes
    function.restype = restype
    return function


_RegisterClassExW = _bind(user32, "RegisterClassExW", (ctypes.POINTER(WNDCLASSEXW),), wintypes.ATOM)
_UnregisterClassW = _bind(user32, "UnregisterClassW", (wintypes.LPCWSTR, wintypes.HINSTANCE), wintypes.BOOL)
_CreateWindowExW = _bind(user32, "CreateWindowExW", (
    wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_int,
    ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.HWND, wintypes.HMENU,
    wintypes.HINSTANCE, wintypes.LPVOID), wintypes.HWND)
_DestroyWindow = _bind(user32, "DestroyWindow", (wintypes.HWND,), wintypes.BOOL)
_DefWindowProcW = _bind(user32, "DefWindowProcW", (
    wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM), LRESULT)
_GetMessageW = _bind(user32, "GetMessageW", (
    ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT), wintypes.BOOL)
_TranslateMessage = _bind(user32, "TranslateMessage", (ctypes.POINTER(wintypes.MSG),), wintypes.BOOL)
_DispatchMessageW = _bind(user32, "DispatchMessageW", (ctypes.POINTER(wintypes.MSG),), LRESULT)
_PostMessageW = _bind(user32, "PostMessageW", (
    wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM), wintypes.BOOL)
_PostQuitMessage = _bind(user32, "PostQuitMessage", (ctypes.c_int,), None)
_RegisterWindowMessageW = _bind(user32, "RegisterWindowMessageW", (wintypes.LPCWSTR,), wintypes.UINT)
_ChangeWindowMessageFilterEx = _bind(user32, "ChangeWindowMessageFilterEx", (
    wintypes.HWND, wintypes.UINT, wintypes.DWORD, wintypes.LPVOID), wintypes.BOOL)
_CreatePopupMenu = _bind(user32, "CreatePopupMenu", (), wintypes.HMENU)
_AppendMenuW = _bind(user32, "AppendMenuW", (
    wintypes.HMENU, wintypes.UINT, ctypes.c_size_t, wintypes.LPCWSTR), wintypes.BOOL)
_SetMenuDefaultItem = _bind(user32, "SetMenuDefaultItem", (
    wintypes.HMENU, wintypes.UINT, wintypes.UINT), wintypes.BOOL)
_DestroyMenu = _bind(user32, "DestroyMenu", (wintypes.HMENU,), wintypes.BOOL)
_TrackPopupMenuEx = _bind(user32, "TrackPopupMenuEx", (
    wintypes.HMENU, wintypes.UINT, ctypes.c_int, ctypes.c_int, wintypes.HWND,
    wintypes.LPVOID), wintypes.BOOL)
_SetForegroundWindow = _bind(user32, "SetForegroundWindow", (wintypes.HWND,), wintypes.BOOL)
_GetCursorPos = _bind(user32, "GetCursorPos", (ctypes.POINTER(wintypes.POINT),), wintypes.BOOL)
_CreateIconIndirect = _bind(user32, "CreateIconIndirect", (ctypes.POINTER(ICONINFO),), wintypes.HICON)
_DestroyIcon = _bind(user32, "DestroyIcon", (wintypes.HICON,), wintypes.BOOL)
_Shell_NotifyIconW = _bind(shell32, "Shell_NotifyIconW", (
    wintypes.DWORD, ctypes.POINTER(NOTIFYICONDATAW)), wintypes.BOOL)
_CreateDIBSection = _bind(gdi32, "CreateDIBSection", (
    wintypes.HDC, ctypes.c_void_p, wintypes.UINT, ctypes.POINTER(ctypes.c_void_p),
    wintypes.HANDLE, wintypes.DWORD), wintypes.HBITMAP)
_CreateBitmap = _bind(gdi32, "CreateBitmap", (
    ctypes.c_int, ctypes.c_int, wintypes.UINT, wintypes.UINT, ctypes.c_void_p), wintypes.HBITMAP)
_DeleteObject = _bind(gdi32, "DeleteObject", (wintypes.HGDIOBJ,), wintypes.BOOL)
_GetModuleHandleW = _bind(kernel32, "GetModuleHandleW", (wintypes.LPCWSTR,), wintypes.HMODULE)


def create_hicon(image) -> int:
    """An HICON from a square RGBA Pillow image, at its own pixel size.

    The pixels go into a top-down 32-bit DIB section with an alpha mask, the
    format Windows draws with per-pixel transparency (straight alpha, as in
    an .ico). The caller owns the handle and frees it with DestroyIcon."""
    width, height = image.size
    if width != height or not MIN_ICON_PX <= width <= MAX_ICON_PX:
        raise ValueError(f"tray icon must be square, {MIN_ICON_PX}-{MAX_ICON_PX} px")
    pixels = image.convert("RGBA").tobytes("raw", "BGRA")
    header = BITMAPV5HEADER(
        bV5Size=ctypes.sizeof(BITMAPV5HEADER), bV5Width=width, bV5Height=-height,
        bV5Planes=1, bV5BitCount=32, bV5Compression=BI_BITFIELDS,
        bV5RedMask=0x00FF0000, bV5GreenMask=0x0000FF00, bV5BlueMask=0x000000FF,
        bV5AlphaMask=0xFF000000)
    bits = ctypes.c_void_p()
    color = _CreateDIBSection(None, ctypes.byref(header), DIB_RGB_COLORS,
                              ctypes.byref(bits), None, 0)
    if not color or not bits.value:
        raise OSError(ctypes.get_last_error(), "CreateDIBSection failed")
    mask = None
    try:
        ctypes.memmove(bits.value, pixels, len(pixels))  # exactly width*height*4
        mask = _CreateBitmap(width, height, 1, 1, None)
        if not mask:
            raise OSError(ctypes.get_last_error(), "CreateBitmap failed")
        info = ICONINFO(fIcon=True, xHotspot=0, yHotspot=0, hbmMask=mask, hbmColor=color)
        icon = _CreateIconIndirect(ctypes.byref(info))
        if not icon:
            raise OSError(ctypes.get_last_error(), "CreateIconIndirect failed")
        return icon
    finally:
        # The icon holds its own copies of both bitmaps.
        _DeleteObject(color)
        if mask:
            _DeleteObject(mask)


class NotifyIcon:
    """A notification-area icon with a tooltip and a context menu.

    `entries` is called each time the menu opens and returns the rows: items
    with `label`, `action`, `checked` (None or a callable), `default` and
    `visible`, and None for a separator. A left click runs the default
    entry's action, visible or not; a right click shows the visible ones."""

    def __init__(self, name: str, image, title: str,
                 entries: "Callable[[], Sequence[object | None]]") -> None:
        self.name = name
        self._entries = entries
        self._image = image
        self._title = title
        self._hicon = None
        self._hwnd = None
        self._lock = threading.Lock()
        self._ready = threading.Event()
        self._error: "BaseException | None" = None
        self._thread: "threading.Thread | None" = None
        self._class_name = f"WeekNumberTray-{os.getpid()}-{id(self):x}"
        self._taskbar_created = _RegisterWindowMessageW("TaskbarCreated")
        # Kept for the window's lifetime: ctypes must not free the callback
        # while Windows can still call it.
        self._wndproc = WNDPROC(self._window_proc)

    # -- public API (any thread) -------------------------------------------------
    def run_detached(self) -> None:
        """Start the tray thread; returns once the icon is set up."""
        if self._thread is not None:
            return
        self._thread = threading.Thread(target=self._run, name="tray", daemon=True)
        self._thread.start()
        if not self._ready.wait(START_TIMEOUT_S):
            raise RuntimeError("The tray icon did not start in time")
        if self._error is not None:
            raise RuntimeError("The tray icon could not be created") from self._error

    @property
    def icon(self):
        return self._image

    @icon.setter
    def icon(self, image) -> None:
        handle = create_hicon(image)  # built before taking the lock
        with self._lock:
            old, self._hicon, self._image = self._hicon, handle, image
            if self._hwnd:
                self._notify(NIM_MODIFY, NIF_ICON)
        if old:
            _DestroyIcon(old)

    @property
    def title(self) -> str:
        return self._title

    @title.setter
    def title(self, text: str) -> None:
        with self._lock:
            self._title = str(text)
            if self._hwnd:
                self._notify(NIM_MODIFY, NIF_TIP)

    def stop(self) -> None:
        """Remove the icon and end the tray thread. Safe to call twice, and
        from the tray thread itself (it then does not wait for itself)."""
        thread = self._thread
        if thread is None:
            return
        with self._lock:
            hwnd = self._hwnd
        if hwnd:
            _PostMessageW(hwnd, WM_CLOSE, 0, 0)
        if thread is not threading.current_thread():
            thread.join(STOP_TIMEOUT_S)
            if thread.is_alive():
                log.warning("The tray thread did not stop in time")

    # -- tray thread -------------------------------------------------------------
    def _run(self) -> None:
        instance = _GetModuleHandleW(None)
        atom = 0
        try:
            self._hicon = create_hicon(self._image)
            atom = _RegisterClassExW(ctypes.byref(WNDCLASSEXW(
                cbSize=ctypes.sizeof(WNDCLASSEXW), lpfnWndProc=self._wndproc,
                hInstance=instance, lpszClassName=self._class_name)))
            if not atom:
                raise OSError(ctypes.get_last_error(), "RegisterClassExW failed")
            # Top-level, never shown: only top-level windows receive the
            # TaskbarCreated broadcast, which message-only windows miss.
            hwnd = _CreateWindowExW(0, self._class_name, self.name, WS_POPUP,
                                    0, 0, 0, 0, None, None, instance, None)
            if not hwnd:
                raise OSError(ctypes.get_last_error(), "CreateWindowExW failed")
            _ChangeWindowMessageFilterEx(hwnd, self._taskbar_created, MSGFLT_ALLOW, None)
            with self._lock:
                self._hwnd = hwnd
                if not self._notify(NIM_ADD, NIF_MESSAGE | NIF_ICON | NIF_TIP):
                    # Explorer not up yet: TaskbarCreated adds the icon later.
                    log.warning("Shell_NotifyIcon(NIM_ADD) failed; waiting for the taskbar")
        except BaseException as exc:
            self._error = exc
            log.exception("Tray icon setup failed")
            self._ready.set()
            self._cleanup(atom, instance)
            return
        self._ready.set()

        msg = wintypes.MSG()
        try:
            while True:
                result = _GetMessageW(ctypes.byref(msg), None, 0, 0)
                if result in (0, -1):
                    break
                _TranslateMessage(ctypes.byref(msg))
                _DispatchMessageW(ctypes.byref(msg))
        finally:
            self._cleanup(atom, instance)

    def _cleanup(self, atom: int, instance) -> None:
        with self._lock:
            hwnd, self._hwnd = self._hwnd, None
            icon, self._hicon = self._hicon, None
        if hwnd:
            _DestroyWindow(hwnd)  # no-op error if WM_CLOSE already destroyed it
        if icon:
            _DestroyIcon(icon)
        if atom:
            _UnregisterClassW(self._class_name, instance)

    def _notify(self, message: int, flags: int) -> bool:
        """Shell_NotifyIconW for our icon. Call with the lock held."""
        data = NOTIFYICONDATAW(cbSize=ctypes.sizeof(NOTIFYICONDATAW), hWnd=self._hwnd,
                               uID=1, uFlags=flags, uCallbackMessage=WM_TRAY,
                               hIcon=self._hicon)
        data.szTip = self._title[:MAX_TIP_CHARS]
        return bool(_Shell_NotifyIconW(message, ctypes.byref(data)))

    def _window_proc(self, hwnd, message, wparam, lparam) -> int:
        try:
            if message == WM_TRAY:
                event = lparam & 0xFFFF
                if event == WM_LBUTTONUP:
                    self._run_default()
                elif event == WM_RBUTTONUP:
                    self._show_menu(hwnd)
                return 0
            if message == self._taskbar_created or message == WM_DISPLAYCHANGE:
                with self._lock:
                    if self._hwnd:
                        if message == WM_DISPLAYCHANGE:
                            self._notify(NIM_DELETE, 0)
                        self._notify(NIM_ADD, NIF_MESSAGE | NIF_ICON | NIF_TIP)
                return 0 if message == self._taskbar_created else \
                    _DefWindowProcW(hwnd, message, wparam, lparam)
            if message == WM_CLOSE:
                with self._lock:
                    if self._hwnd:
                        self._notify(NIM_DELETE, 0)
                        self._hwnd = None
                _DestroyWindow(hwnd)
                return 0
            if message == WM_DESTROY:
                _PostQuitMessage(0)
                return 0
        except Exception:
            log.exception("Tray window message %#x failed", message)
            return 0
        return _DefWindowProcW(hwnd, message, wparam, lparam)

    def _rows(self) -> "list[object | None]":
        try:
            return list(self._entries())
        except Exception:
            log.exception("Could not build the tray menu")
            return []

    def _run_default(self) -> None:
        for row in self._rows():
            if row is not None and getattr(row, "default", False):
                row.action()
                return

    def _show_menu(self, hwnd) -> None:
        menu = _CreatePopupMenu()
        if not menu:
            return
        actions: "list[Callable[[], object]]" = []
        command = 0
        try:
            for row in self._rows():
                if row is None:
                    if actions:
                        _AppendMenuW(menu, MF_SEPARATOR, 0, None)
                    continue
                if not row.visible:
                    continue
                checked = row.checked
                try:
                    flags = MF_STRING | (MF_CHECKED if checked is not None and checked() else 0)
                except Exception:
                    log.exception("Could not read the state of %s", row.label)
                    flags = MF_STRING
                actions.append(row.action)
                _AppendMenuW(menu, flags, len(actions), str(row.label))
                if row.default:
                    _SetMenuDefaultItem(menu, len(actions), 0)
            if not actions:
                return
            point = wintypes.POINT()
            _GetCursorPos(ctypes.byref(point))
            # The menu only closes on an outside click while our window is the
            # foreground window, and needs a message afterwards (KB135788).
            _SetForegroundWindow(hwnd)
            command = _TrackPopupMenuEx(menu, TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
                                        point.x, point.y, hwnd, None)
            _PostMessageW(hwnd, WM_NULL, 0, 0)
        finally:
            _DestroyMenu(menu)
        if 1 <= command <= len(actions):
            actions[command - 1]()
