"""The ctypes bridge to AppKit that replaced PyObjC. Needs no window server:
nothing here touches the screen or the menu bar."""

import subprocess
import sys
from ctypes import c_bool, c_long
from pathlib import Path

import pytest

if sys.platform != "darwin":
    pytest.skip("macOS only", allow_module_level=True)

from weeknumber_platform import _cocoa as cocoa  # noqa: E402

pytestmark = pytest.mark.macos
APP_DIR = Path(__file__).resolve().parents[1] / "src" / "weeknumber"


def test_strings_survive_the_round_trip():
    with cocoa.autorelease_pool():
        assert cocoa.to_str(cocoa.nsstring("Week 43 · Café ✓")) == "Week 43 · Café ✓"
        assert cocoa.to_str(None) is None


def test_struct_arguments_and_returns():
    rect = cocoa.NSRect(cocoa.NSPoint(1.5, 2.5), cocoa.NSSize(300, 200))
    with cocoa.autorelease_pool():
        value = cocoa.send(cocoa.cls("NSValue"), "valueWithRect:", rect,
                           argtypes=(cocoa.NSRect,))
        back = cocoa.send(value, "rectValue", restype=cocoa.NSRect)
    assert (back.origin.x, back.origin.y, back.size.width, back.size.height) == (1.5, 2.5, 300, 200)


def test_appkit_constants_and_classes():
    with cocoa.autorelease_pool():
        assert cocoa.to_str(cocoa.constant("NSFontAttributeName")) == "NSFont"
    assert 0.3 < cocoa.float_constant("NSFontWeightBold") < 0.5
    assert cocoa.cls("NSStatusBar") and cocoa.cls("NoSuchClassHere") is None


def test_defined_class_calls_back_once_defined():
    received = []
    target_class = cocoa.define_class("WeekNumberTestTarget",
                                      {"poke:": lambda this, arg: received.append(cocoa.to_str(arg))})
    assert cocoa.define_class("WeekNumberTestTarget", {}) == target_class
    target = cocoa.new("WeekNumberTestTarget")
    try:
        with cocoa.autorelease_pool():
            cocoa.send(target, "performSelector:withObject:", cocoa.sel("poke:"),
                       cocoa.nsstring("hello"), argtypes=(cocoa.c_id, cocoa.c_id))
        assert cocoa.responds(target, "poke:") and not cocoa.responds(target, "nope:")
    finally:
        cocoa.release(target)
    assert received == ["hello"]


def test_property_list_values_convert_like_pyobjc_did():
    with cocoa.autorelease_pool():
        number = cocoa.send(cocoa.cls("NSNumber"), "numberWithLong:", 7, argtypes=(c_long,))
        flag = cocoa.send(cocoa.cls("NSNumber"), "numberWithBool:", True, argtypes=(c_bool,))
        assert cocoa._python_value(number) == 7
        assert cocoa._python_value(flag) is True
        assert cocoa._python_value(cocoa.nsstring("x")) == "x"
    assert cocoa.preference("NoSuchPolicy", "io.github.shanko100.weeknumber.test") is None
    assert not cocoa.preference_forced("NoSuchPolicy", "io.github.shanko100.weeknumber.test")


def test_menu_item_clicks_reach_their_owner(monkeypatch):
    import weeknumber_menubar as menubar
    menubar._target_class()
    chosen = []
    owner = type("Owner", (), {"_on_item": lambda self, sender: chosen.append(
        cocoa.send(sender, "tag", restype=c_long))})()
    target = cocoa.new("WeekNumberMenuBarTarget")
    monkeypatch.setitem(menubar._owners, target, owner)
    try:
        with cocoa.autorelease_pool():
            item = cocoa.new("NSMenuItem")
            cocoa.send(item, "setTag:", 5, restype=None, argtypes=(c_long,))
            cocoa.send(target, "performSelector:withObject:", cocoa.sel("itemChosen:"), item,
                       argtypes=(cocoa.c_id, cocoa.c_id))
            cocoa.release(item)
    finally:
        cocoa.release(target)
    assert chosen == [5]


def test_the_app_never_loads_pyobjc():
    probe = ("import sys, weeknumber_core, weeknumber_menubar, weeknumber_tray; "
             "print(sorted(m for m in sys.modules "
             "if m.split('.')[0] in {'objc', 'AppKit', 'Foundation', 'CoreFoundation'}))")
    result = subprocess.run([sys.executable, "-c", probe], cwd=APP_DIR,
                            capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "[]"
