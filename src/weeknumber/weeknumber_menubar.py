"""macOS menu-bar extra for WeekNumber.

pystray cannot be used here: its macOS backend runs its own NSApplication
loop on the main thread, which is where Tk's Aqua loop must run too. Instead
this creates an ``NSStatusItem`` on the Tk main thread after the root exists;
Tk's mainloop already pumps the Cocoa events, so clicks and menu actions
arrive on the main thread with no marshalling. AppKit is reached through the
small ctypes bridge in ``weeknumber_platform._cocoa``.

Behaviour mirrors the Windows tray: left-click runs the default entry (the
calendar), right-click or Control-click opens the menu. The week number is the
button's text title rather than a bitmap, so the menu bar renders it at native
resolution. White -- the default colour, chosen for a dark Windows taskbar --
follows the menu bar's own text colour instead, because the macOS menu bar is
light or dark depending on the wallpaper and appearance.
"""

from __future__ import annotations

from ctypes import c_bool, c_double, c_long, c_ulong
from typing import TYPE_CHECKING

from weeknumber_core import log, normalize_hex
from weeknumber_platform import _cocoa as cocoa
from weeknumber_platform._cocoa import c_id, send

if TYPE_CHECKING:
    from weeknumber_tray import MenuEntry

#: The settings default. Treated as "use the menu bar's text colour".
AUTO_COLOR = "#FFFFFF"

# AppKit enumeration values.
_VARIABLE_LENGTH = -1.0                  # NSVariableStatusItemLength
_LEFT_MOUSE_UP_MASK = 1 << 2             # NSEventMaskLeftMouseUp
_RIGHT_MOUSE_UP_MASK = 1 << 4            # NSEventMaskRightMouseUp
_RIGHT_MOUSE_UP = 4                      # NSEventTypeRightMouseUp
_CONTROL_KEY = 1 << 18                   # NSEventModifierFlagControl
_STATE_ON, _STATE_OFF = 1, 0             # NSControlStateValueOn/Off

#: The live item for each target object: the Objective-C class is defined
#: once per process, so its methods find their owner here.
_owners: "dict[int, MenuBarItem]" = {}


def _target_class() -> int:
    """The Objective-C receiver of the button, the menu items and the menu
    delegate callbacks."""
    def dispatch(method: str):
        def call(target, argument):
            owner = _owners.get(target)
            if owner is not None:
                getattr(owner, method)(argument)
        return call

    return cocoa.define_class("WeekNumberMenuBarTarget", {
        "buttonClicked:": dispatch("_on_button"),
        "itemChosen:": dispatch("_on_item"),
        "menuNeedsUpdate:": dispatch("_on_menu_update"),
    })


class MenuBarItem:
    def __init__(self, entries: "list[MenuEntry | None]") -> None:
        self._entries = entries
        self._default = next((e for e in entries if e is not None and e.default), None)
        _target_class()
        with cocoa.autorelease_pool():
            # Owned for the life of the process, like the item itself.
            self._target = cocoa.new("WeekNumberMenuBarTarget")
            _owners[self._target] = self
            bar = send(cocoa.cls("NSStatusBar"), "systemStatusBar")
            self._item = cocoa.retain(send(bar, "statusItemWithLength:", _VARIABLE_LENGTH,
                                           argtypes=(c_double,)))
            button = self._button()
            send(button, "setTarget:", self._target, restype=None, argtypes=(c_id,))
            send(button, "setAction:", cocoa.sel("buttonClicked:"),
                 restype=None, argtypes=(c_id,))
            send(button, "sendActionOn:", _LEFT_MOUSE_UP_MASK | _RIGHT_MOUSE_UP_MASK,
                 restype=c_long, argtypes=(c_ulong,))
            self._menu = self._build_menu()

    def _button(self) -> int:
        return send(self._item, "button")

    # -- menu ---------------------------------------------------------------
    def _build_menu(self) -> int:
        menu = cocoa.new("NSMenu", "initWithTitle:", cocoa.nsstring("WeekNumber"),
                         argtypes=(c_id,))
        send(menu, "setAutoenablesItems:", False, restype=None, argtypes=(c_bool,))
        send(menu, "setDelegate:", self._target, restype=None, argtypes=(c_id,))
        self._menu_items: list[tuple[int, "MenuEntry"]] = []
        for index, entry in enumerate(self._entries):
            if entry is None:
                send(menu, "addItem:", send(cocoa.cls("NSMenuItem"), "separatorItem"),
                     restype=None, argtypes=(c_id,))
                continue
            if not entry.visible:
                continue
            item = cocoa.new("NSMenuItem", "initWithTitle:action:keyEquivalent:",
                             cocoa.nsstring(entry.label), cocoa.sel("itemChosen:"),
                             cocoa.nsstring(""), argtypes=(c_id, c_id, c_id))
            send(item, "setTarget:", self._target, restype=None, argtypes=(c_id,))
            send(item, "setTag:", index, restype=None, argtypes=(c_long,))
            send(menu, "addItem:", item, restype=None, argtypes=(c_id,))
            cocoa.release(item)  # the menu holds it, and the menu lives on
            self._menu_items.append((item, entry))
        return menu

    def _update_checks(self) -> None:
        for item, entry in self._menu_items:
            if entry.checked is None:
                continue
            try:
                state = bool(entry.checked())
            except Exception:
                log.exception("Menu check state for %s failed", entry.label)
                state = False
            send(item, "setState:", _STATE_ON if state else _STATE_OFF,
                 restype=None, argtypes=(c_long,))

    def _show_menu(self) -> None:
        self._update_checks()
        # Assigning the menu for the duration of one click is the supported
        # way to pop it from the item's own position.
        send(self._item, "setMenu:", self._menu, restype=None, argtypes=(c_id,))
        try:
            send(self._button(), "performClick:", None, restype=None, argtypes=(c_id,))
        finally:
            send(self._item, "setMenu:", None, restype=None, argtypes=(c_id,))

    # -- events -------------------------------------------------------------
    def _on_button(self, sender) -> None:
        try:
            with cocoa.autorelease_pool():
                app = send(cocoa.cls("NSApplication"), "sharedApplication")
                event = send(app, "currentEvent")
                secondary = bool(event) and (
                    send(event, "type", restype=c_ulong) == _RIGHT_MOUSE_UP
                    or bool(send(event, "modifierFlags", restype=c_ulong) & _CONTROL_KEY))
                if secondary or self._default is None:
                    self._show_menu()
                else:
                    self._default.action()
        except Exception:
            log.exception("Menu-bar click handling failed")

    def _on_item(self, sender) -> None:
        index = send(sender, "tag", restype=c_long) if sender else -1
        if not 0 <= index < len(self._entries):
            return
        entry = self._entries[index]
        if entry is None:
            return
        try:
            entry.action()
        except Exception:
            log.exception("Menu action %s failed", entry.label)

    def _on_menu_update(self, menu) -> None:
        self._update_checks()

    # -- appearance ---------------------------------------------------------
    def set_title(self, text: str, color: str, bold: bool) -> None:
        with cocoa.autorelease_pool():
            font_class = cocoa.cls("NSFont")
            size = send(font_class, "systemFontSize", restype=c_double)
            weight = cocoa.float_constant("NSFontWeightBold" if bold else "NSFontWeightRegular")
            # Monospaced digits keep the item from changing width between weeks.
            font = send(font_class, "monospacedDigitSystemFontOfSize:weight:", size, weight,
                        argtypes=(c_double, c_double))
            attributes = send(cocoa.cls("NSMutableDictionary"), "dictionary")
            send(attributes, "setObject:forKey:", font, cocoa.constant("NSFontAttributeName"),
                 restype=None, argtypes=(c_id, c_id))
            color = normalize_hex(color)
            if color != AUTO_COLOR:
                red, green, blue = (int(color[i:i + 2], 16) / 255.0 for i in (1, 3, 5))
                ns_color = send(cocoa.cls("NSColor"), "colorWithSRGBRed:green:blue:alpha:",
                                red, green, blue, 1.0, argtypes=(c_double,) * 4)
                send(attributes, "setObject:forKey:", ns_color,
                     cocoa.constant("NSForegroundColorAttributeName"),
                     restype=None, argtypes=(c_id, c_id))
            title = cocoa.new("NSAttributedString", "initWithString:attributes:",
                              cocoa.nsstring(text), attributes, argtypes=(c_id, c_id))
            send(self._button(), "setAttributedTitle:", title, restype=None, argtypes=(c_id,))
            cocoa.release(title)

    def set_tooltip(self, text: str) -> None:
        with cocoa.autorelease_pool():
            send(self._button(), "setToolTip:", cocoa.nsstring(text),
                 restype=None, argtypes=(c_id,))

    def stop(self) -> None:
        item, self._item = self._item, None
        if not item:
            return
        try:
            send(send(cocoa.cls("NSStatusBar"), "systemStatusBar"), "removeStatusItem:",
                 item, restype=None, argtypes=(c_id,))
        except Exception:
            log.exception("Removing the menu-bar item failed")
        cocoa.release(item)
