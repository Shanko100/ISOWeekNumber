"""The Windows tray icon (weeknumber_platform._wintray) that replaced pystray:
the menu it builds, what a click runs, and -- on real Windows -- the window,
the icon handle and the message loop."""

import os
import sys
import threading
from types import SimpleNamespace

import pytest

if sys.platform != "win32":
    pytest.skip("Windows tray", allow_module_level=True)

from PIL import Image  # noqa: E402

from weeknumber_platform import _wintray  # noqa: E402

pytestmark = pytest.mark.windows
REAL_WINDOWS = os.name == "nt"


def row(label, action, checked=None, default=False, visible=True):
    return SimpleNamespace(label=label, action=action, checked=checked,
                           default=default, visible=visible)


@pytest.fixture
def calls():
    return []


@pytest.fixture
def entries(calls):
    return [
        row("Show Calendar", lambda: calls.append("calendar"), default=True, visible=False),
        row("Run at Startup", lambda: calls.append("startup"), checked=lambda: True),
        row("Bold Text", lambda: calls.append("bold"), checked=lambda: False),
        None,
        row("Exit", lambda: calls.append("exit")),
    ]


class FakeMenuApi:
    """Records the menu the tray builds and picks command `choose`."""

    def __init__(self, monkeypatch, choose):
        self.items = []
        self.destroyed = False
        monkeypatch.setattr(_wintray, "_CreatePopupMenu", lambda: 7)
        monkeypatch.setattr(_wintray, "_AppendMenuW", self._append)
        monkeypatch.setattr(_wintray, "_SetMenuDefaultItem", lambda *a: True)
        monkeypatch.setattr(_wintray, "_GetCursorPos", lambda p: True)
        monkeypatch.setattr(_wintray, "_SetForegroundWindow", lambda h: True)
        monkeypatch.setattr(_wintray, "_PostMessageW", lambda *a: True)
        monkeypatch.setattr(_wintray, "_TrackPopupMenuEx", lambda *a: choose)
        monkeypatch.setattr(_wintray, "_DestroyMenu", self._destroy)

    def _append(self, menu, flags, command, text):
        self.items.append((flags, command, text))
        return True

    def _destroy(self, menu):
        self.destroyed = True
        return True


def icon_for(entries):
    tray = _wintray.NotifyIcon.__new__(_wintray.NotifyIcon)
    tray._entries = lambda: entries
    return tray


def test_the_menu_shows_visible_rows_with_their_checkmarks(monkeypatch, entries, calls):
    api = FakeMenuApi(monkeypatch, choose=2)
    icon_for(entries)._show_menu(hwnd=1)
    assert api.items == [
        (_wintray.MF_STRING | _wintray.MF_CHECKED, 1, "Run at Startup"),
        (_wintray.MF_STRING, 2, "Bold Text"),
        (_wintray.MF_SEPARATOR, 0, None),
        (_wintray.MF_STRING, 3, "Exit"),
    ]
    assert calls == ["bold"] and api.destroyed


@pytest.mark.parametrize("choose", [0, 4, -1, 99])
def test_a_dismissed_or_unknown_command_runs_nothing(monkeypatch, entries, calls, choose):
    api = FakeMenuApi(monkeypatch, choose=choose)
    icon_for(entries)._show_menu(hwnd=1)
    assert calls == [] and api.destroyed


def test_a_left_click_runs_the_hidden_default_entry(entries, calls):
    icon_for(entries)._run_default()
    assert calls == ["calendar"]


def test_a_failing_checkmark_still_shows_the_row(monkeypatch, calls):
    def broken():
        raise RuntimeError("registry unavailable")
    api = FakeMenuApi(monkeypatch, choose=1)
    icon_for([row("Run at Startup", lambda: calls.append("startup"), checked=broken)])._show_menu(1)
    assert api.items == [(_wintray.MF_STRING, 1, "Run at Startup")] and calls == ["startup"]


def test_only_the_tray_messages_are_handled(monkeypatch, entries, calls):
    tray = icon_for(entries)
    tray._taskbar_created = 0xC123
    monkeypatch.setattr(_wintray, "_DefWindowProcW", lambda *a: 42)
    assert tray._window_proc(1, _wintray.WM_TRAY, 0, _wintray.WM_LBUTTONUP) == 0
    assert calls == ["calendar"]
    assert tray._window_proc(1, 0x0111, 3, 0) == 42  # WM_COMMAND: not ours
    assert calls == ["calendar"]


@pytest.mark.parametrize("size", [(15, 15), (257, 257), (16, 20)])
def test_odd_icon_sizes_are_refused(size):
    with pytest.raises(ValueError):
        _wintray.create_hicon(Image.new("RGBA", size))


@pytest.mark.skipif(not REAL_WINDOWS, reason="needs the Win32 API")
def test_icon_handles_are_created_and_freed():
    handle = _wintray.create_hicon(Image.new("RGBA", (20, 20), (47, 111, 237, 255)))
    assert handle
    assert _wintray._DestroyIcon(handle)


@pytest.mark.skipif(not REAL_WINDOWS, reason="needs the Win32 API")
def test_the_tray_window_runs_clicks_and_stops():
    clicked = threading.Event()
    entries = [row("Show Calendar", clicked.set, default=True, visible=False),
               row("Exit", lambda: None)]
    tray = _wintray.NotifyIcon("WeekNumberTest", Image.new("RGBA", (16, 16), "white"),
                               "Week 1", lambda: entries)
    tray.run_detached()
    try:
        assert tray._hwnd and tray._thread.is_alive()
        tray.icon = Image.new("RGBA", (16, 16), "black")
        tray.title = "Week 2"
        assert _wintray._PostMessageW(tray._hwnd, _wintray.WM_TRAY, 0, _wintray.WM_LBUTTONUP)
        assert clicked.wait(5)
    finally:
        tray.stop()
    assert not tray._thread.is_alive()
    assert tray._hwnd is None and tray._hicon is None
    tray.stop()  # a second stop is harmless
