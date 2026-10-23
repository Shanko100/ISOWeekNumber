"""The tray icon / menu-bar number toggles the calendar flyout. Tk-free:
the open flyout and `show` are stand-ins."""

import pytest

import weeknumber_calendar as calendar
from weeknumber_calendar import CalendarPopup


class _OpenFlyout:
    def __init__(self):
        self.closed = False

    def close(self):
        self.closed = True


@pytest.fixture
def popup(monkeypatch):
    state = {"open": None, "shown": 0, "now": 100.0}
    monkeypatch.setattr(CalendarPopup, "_live", classmethod(lambda cls: state["open"]))
    monkeypatch.setattr(CalendarPopup, "show", classmethod(
        lambda cls, *a: state.__setitem__("shown", state["shown"] + 1)))
    monkeypatch.setattr(CalendarPopup, "_dismissed_at", float("-inf"))
    monkeypatch.setattr(calendar.time, "monotonic", lambda: state["now"])
    return state


def test_click_opens_a_closed_calendar(popup):
    CalendarPopup.toggle(None, None)
    assert popup["shown"] == 1


def test_click_closes_an_open_calendar_instead_of_stacking(popup):
    flyout = popup["open"] = _OpenFlyout()
    CalendarPopup.toggle(None, None)
    assert flyout.closed and popup["shown"] == 0


def test_click_that_just_dismissed_it_does_not_reopen(popup):
    # Windows: the click on the tray icon takes the focus first, closing the
    # flyout, and only then reaches the app.
    CalendarPopup._dismissed_at = popup["now"] - 0.1
    CalendarPopup.toggle(None, None)
    assert popup["shown"] == 0
    popup["now"] += CalendarPopup.TOGGLE_GRACE_S
    CalendarPopup.toggle(None, None)
    assert popup["shown"] == 1


class _Root:
    """Records class bindings the way Tk keeps them: on the root, for good."""

    def __init__(self):
        self.bindings = {}

    def nametowidget(self, name):
        assert name == "."
        return self

    def bind_class(self, tag, sequence, handler):
        self.bindings.setdefault((tag, sequence), []).append(handler)


class _Popup:
    def __init__(self):
        self.clicks = []

    def _on_day_click(self, event):
        self.clicks.append(event)


def test_grid_bindings_are_made_once_and_reach_the_open_calendar(monkeypatch):
    # Binding each popup's own methods kept every closed popup alive: Tk
    # holds class bindings on the root until the app quits.
    root = _Root()
    monkeypatch.setattr(CalendarPopup, "_bound_root", None)
    CalendarPopup._bind_grid_classes(root)
    CalendarPopup._bind_grid_classes(root)  # the next open, or a theme switch
    assert all(len(handlers) == 1 for handlers in root.bindings.values())

    (click,) = root.bindings[(CalendarPopup.DAY_TAG, "<Button-1>")]
    first, second = _Popup(), _Popup()
    monkeypatch.setattr(CalendarPopup, "_instance", first)
    click("event 1")
    monkeypatch.setattr(CalendarPopup, "_instance", second)
    click("event 2")
    monkeypatch.setattr(CalendarPopup, "_instance", None)
    assert click("after close") is None
    assert first.clicks == ["event 1"] and second.clicks == ["event 2"]
