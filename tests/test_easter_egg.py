"""The way into the game: three quick clicks on Afghanistan, the one country
on UTC+4:30. Tk-free: the point test and the click counter."""

import pytest

import weeknumber_timezone as tz
from weeknumber_timezone_dialog import EGG_WINDOW_MS, ClickCounter, is_egg_point


class _MapWindow:
    """Stands in for the time-zone window, which holds the map geometry."""


@pytest.fixture(autouse=True, scope="module")
def timezone_data():
    tz.load_timezone_data()
    window = _MapWindow()
    tz.use_map(window)
    yield
    tz.drop_map(window)


@pytest.mark.parametrize(("place", "lon", "lat", "hit"), [
    ("Kabul", 69.2, 34.5, True),
    ("Kandahar", 65.7, 31.6, True),
    ("Herat", 62.2, 34.3, True),
    ("Tehran", 51.4, 35.7, False),
    ("Islamabad", 73.0, 33.7, False),
    ("Dushanbe", 68.8, 38.6, False),
    ("open sea", -30.0, 0.0, False),
])
def test_only_afghanistan_counts(place, lon, lat, hit):
    assert is_egg_point(lon, lat) is hit, place


def test_three_quick_clicks_fire_once():
    counter = ClickCounter()
    assert counter.hit(1000) is False
    assert counter.hit(1300) is False
    assert counter.hit(1600) is True
    assert not counter.started  # the next click starts a new count


def test_slow_clicks_do_not_fire():
    counter = ClickCounter()
    counter.hit(0)
    counter.hit(400)
    assert counter.hit(EGG_WINDOW_MS + 1) is False  # too late: counts as a new first click
    assert counter.hit(EGG_WINDOW_MS + 300) is False
    assert counter.hit(EGG_WINDOW_MS + 600) is True


def test_reset_and_clock_wrap_start_over():
    counter = ClickCounter()
    counter.hit(5000)
    counter.reset()
    counter.hit(6000)
    assert counter.hit(10) is False  # Tk's clock wrapped: treat as a fresh start
    assert counter.hit(20) is False
    assert counter.hit(30) is True
