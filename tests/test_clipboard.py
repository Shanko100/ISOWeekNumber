import datetime as dt

import pytest

from weeknumber_clipboard import (
    format_calendar_week,
    format_gui_date,
    format_iso_week,
)


def test_format_gui_date():
    assert format_gui_date(dt.date(2026, 10, 17)) == "17-10-2026"


@pytest.mark.parametrize(
    ("day", "expected"),
    [
        (dt.date(2021, 1, 1), "2020 CW53"),
        (dt.date(2026, 1, 1), "2026 CW01"),
        (dt.date(2026, 12, 31), "2026 CW53"),
    ],
)
def test_format_calendar_week_uses_iso_year(day, expected):
    assert format_calendar_week(day) == expected


@pytest.mark.parametrize(
    ("year", "week", "expected"),
    [(2026, 1, "2026 CW01"), (2026, 9, "2026 CW09"), (2026, 53, "2026 CW53")],
)
def test_format_iso_week(year, week, expected):
    assert format_iso_week(year, week) == expected


@pytest.mark.parametrize("value", [None, "2026-10-17", dt.datetime(2026, 10, 17)])
def test_date_formatters_reject_non_date_values(value):
    if isinstance(value, dt.datetime):
        # datetime is a subclass of date and is intentionally accepted.
        assert format_gui_date(value) == "17-10-2026"
    else:
        with pytest.raises(TypeError):
            format_gui_date(value)
