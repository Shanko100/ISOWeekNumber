import datetime as dt
import json

import pytest

import weeknumber_core as core


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("#abcdef", "#ABCDEF"),
        ("abcdef", "#ABCDEF"),
        ("  #123456  ", "#123456"),
        ("#12345", "#FFFFFF"),
        ("#12_456", "#FFFFFF"),
        (None, "#FFFFFF"),
    ],
)
def test_normalize_hex(value, expected):
    assert core.normalize_hex(value) == expected


def test_theme_colors_returns_copy():
    first = core.theme_colors()
    second = core.theme_colors()
    first["ACCENT"] = "#000000"
    assert second["ACCENT"] != "#000000"


def test_load_bounded_json(tmp_path):
    path = tmp_path / "data.json"
    path.write_text('{"ok": true}', encoding="utf-8")
    assert core.load_bounded_json(path, 32) == {"ok": True}

    with pytest.raises(ValueError):
        core.load_bounded_json(path, 2)


def test_settings_load_and_validation(monkeypatch, tmp_path):
    settings_file = tmp_path / "settings.json"
    settings_file.write_text(
        json.dumps(
            {
                "SelectedColor": "112233",
                "IsBold": False,
                "HolidayCountryPrimary": "NL",
                "HolidayCountrySecondary": "NL",
                "DarkMode": True,
                "TimeZoneEnabled": True,
                "TimeZoneKey": "Unknown Zone",
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(core, "SETTINGS_DIR", tmp_path)
    monkeypatch.setattr(core, "SETTINGS_FILE", settings_file)

    settings = core.SettingsStore()
    assert settings.selected_color == "#112233"
    assert settings.is_bold is False
    assert settings.is_dark_mode is True

    settings.validate_holiday_countries({"NL", "BE"})
    assert settings.holiday_country_primary == "NL"
    assert settings.holiday_country_secondary is None

    settings.validate_timezone_key({"W. Europe Standard Time"})
    assert settings.timezone_key is None


@pytest.fixture
def no_held_holidays():
    core.release_holidays()
    yield
    core.release_holidays()


def test_holiday_countries_lists_every_country_by_name(no_held_holidays):
    countries = core.holiday_countries()
    assert len(countries) > 200
    assert ("NL", "Netherlands") in countries
    names = [name for _code, name in countries]
    assert names == sorted(names, key=str.casefold)
    assert core._HOLIDAY_TABLE == {}  # listing the countries loads no table


def test_hold_holidays_keeps_only_the_years_on_screen(no_held_holidays):
    christmas = dt.date(2026, 12, 25)
    core.hold_holidays(["NL", None], {2026})
    names = core.holiday_names("NL", christmas)
    assert isinstance(names, tuple) and names
    assert set(core._HOLIDAY_TABLE["NL"]) == {2026}
    assert core.holiday_names("NL", dt.date(2027, 12, 25)) == ()  # not held
    assert core.holiday_country_name("NL") == "Netherlands"
    core.hold_holidays(["NL"], {2026, 2027})  # December shows January too
    assert set(core._HOLIDAY_TABLE["NL"]) == {2026, 2027}
    assert core.holiday_names("NL", dt.date(2027, 1, 1))


def test_hold_holidays_drops_what_is_no_longer_shown(no_held_holidays):
    core.hold_holidays(["NL", "BE"], {2026})
    core.hold_holidays(["BE"], {2030})
    assert set(core._HOLIDAY_TABLE) == {"BE"}
    assert set(core._HOLIDAY_TABLE["BE"]) == {2030}
    assert core.holiday_names("NL", dt.date(2026, 12, 25)) == ()
    assert core.holiday_country_name("NL") is None
    core.release_holidays()
    assert core._HOLIDAY_TABLE == {}
    assert core._HOLIDAY_COUNTRY_NAMES == {}


def test_hold_holidays_reads_nothing_when_nothing_is_missing(no_held_holidays, monkeypatch):
    core.hold_holidays(["NL"], {2026})
    monkeypatch.setattr(core, "bundled_file", lambda name: pytest.fail("read again"))
    core.hold_holidays(["NL"], {2026})
    core.hold_holidays(["NL", None], {2026, 1999})  # years without data are ignored


def test_every_bundled_holiday_table_validates(no_held_holidays):
    # The app validates only the countries and years on screen; the shipped
    # file is checked in full here.
    codes = [code for code, _name in core.holiday_countries()]
    assert len(codes) > 200
    core.hold_holidays(codes, range(2000, 2051))
    assert set(core._HOLIDAY_TABLE) == set(codes)
    assert not core._HOLIDAY_FAILED


def _holiday_document(nl_years=None):
    empty = {str(year): {} for year in range(2000, 2051)}
    years = dict(empty, **{"2026": {"2026-04-27": ["King's Day"]}})
    return {
        "METADATA": {"schema_version": 3},
        "COUNTRIES": [["NL", "Netherlands"], ["BE", "Belgium"]],
        "HOLIDAYS": {"NL": years if nl_years is None else nl_years, "BE": dict(years)},
    }


@pytest.fixture
def holiday_file(tmp_path, monkeypatch, no_held_holidays):
    path = tmp_path / core.HOLIDAY_DATA_FILE
    monkeypatch.setattr(core, "bundled_file",
                        lambda name: path if name == core.HOLIDAY_DATA_FILE else None)
    return path


def test_holiday_file_is_read_one_country_at_a_time(holiday_file):
    from generate_holiday_data import encode_holiday_file
    holiday_file.write_bytes(encode_holiday_file(_holiday_document()))
    assert core.holiday_countries() == [("BE", "Belgium"), ("NL", "Netherlands")]
    core.hold_holidays(["NL"], {2026})
    assert core.holiday_names("NL", dt.date(2026, 4, 27)) == ("King's Day",)
    assert set(core._HOLIDAY_TABLE) == {"NL"}


def _swap_tables(payload: bytes) -> bytes:
    index, nl, be, _end = payload.split(b"\n")
    return b"\n".join([index, be, nl, b""])  # same lengths, wrong codes


def _with_length(payload: bytes, delta: int) -> bytes:
    head, rest = payload.split(b"\n", 1)
    index = json.loads(head)
    index["COUNTRIES"][0][2] += delta
    return json.dumps(index).encode() + b"\n" + rest


@pytest.mark.parametrize("corrupt", [
    lambda payload: payload.replace(b"\n", b"\r\n"),     # a CRLF checkout
    lambda payload: payload + b"\n",                       # trailing data
    lambda payload: _with_length(payload, 1),              # lengths off by one
    _swap_tables,
    lambda payload: b" " * 70_000 + payload,              # oversize index line
    lambda payload: payload[:-1],                         # cut short
])
def test_holiday_file_reader_is_strict(holiday_file, corrupt):
    from generate_holiday_data import encode_holiday_file
    holiday_file.write_bytes(corrupt(encode_holiday_file(_holiday_document())))
    core.hold_holidays(["NL"], {2026})
    assert core._HOLIDAY_TABLE == {}
    assert core._HOLIDAY_FAILED == {"NL"}  # not retried on every redraw


def test_holiday_table_missing_a_year_is_refused(holiday_file):
    from generate_holiday_data import encode_holiday_file
    years = _holiday_document()["HOLIDAYS"]["NL"]
    del years["2031"]
    holiday_file.write_bytes(encode_holiday_file(_holiday_document(nl_years=years)))
    core.hold_holidays(["NL", "BE"], {2026})
    assert set(core._HOLIDAY_TABLE) == {"BE"}  # one bad table spoils no other
    assert core.holiday_countries()  # and the list still opens


def test_read_bounded_text_allocates_the_file_not_the_cap(tmp_path):
    import tracemalloc
    path = tmp_path / "data.json"
    path.write_text('{"a": 1}', encoding="utf-8")
    tracemalloc.start()
    try:
        assert core.read_bounded_text(path, 64 * 1024 * 1024) == '{"a": 1}'
        _current, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    assert peak < 1024 * 1024


def test_read_bounded_text_refuses_oversize_and_empty_files(tmp_path):
    path = tmp_path / "data.json"
    path.write_bytes(b"x" * 11)
    with pytest.raises(ValueError):
        core.read_bounded_text(path, 10)
    path.write_bytes(b"")
    with pytest.raises(ValueError):
        core.read_bounded_text(path, 10)


def test_week_number_is_in_iso_range():
    week = core.WeekNumber()
    assert 1 <= week.number <= 53


class _FakeWidget:
    def __init__(self):
        self.timers = []
        self.idle = []

    def after(self, ms, func):
        self.timers.append((ms, func))

    def after_idle(self, func):
        self.idle.append(func)


def test_dispatcher_post_never_calls_tk():
    # Posting from a Cocoa callback must not touch Tk (see UiDispatcher).
    widget = _FakeWidget()
    dispatcher = core.UiDispatcher(widget)
    dispatcher.start()
    ran = []
    dispatcher.post(lambda: ran.append(1))
    assert ran == [] and widget.idle == [] and len(widget.timers) == 1
    widget.timers.pop()[1]()
    assert ran == [1]


def test_dispatcher_worker_posts_wait_for_the_timer():
    import threading
    widget = _FakeWidget()
    dispatcher = core.UiDispatcher(widget)
    dispatcher.start()
    ran = []
    worker = threading.Thread(target=lambda: dispatcher.post(lambda: ran.append(1)))
    worker.start()
    worker.join()
    assert widget.idle == []  # no Tcl call from the worker thread
    _ms, tick = widget.timers.pop()
    tick()
    assert ran == [1]
    assert widget.timers[-1][0] == core.UiDispatcher.ACTIVE_POLL_MS


def test_dispatcher_backs_off_when_idle(monkeypatch):
    widget = _FakeWidget()
    dispatcher = core.UiDispatcher(widget)
    dispatcher.start()
    clock = [1000.0]
    monkeypatch.setattr(core.time, "monotonic", lambda: clock[0])
    widget.timers.pop()[1]()
    assert widget.timers[-1][0] == core.UiDispatcher.IDLE_POLL_MS
    dispatcher.post(lambda: None)
    widget.timers.pop()[1]()
    assert widget.timers[-1][0] == core.UiDispatcher.ACTIVE_POLL_MS
    clock[0] += core.UiDispatcher.IDLE_AFTER_S + 0.1
    widget.timers.pop()[1]()
    assert widget.timers[-1][0] == core.UiDispatcher.IDLE_POLL_MS


class _FakeRoot:
    def __init__(self):
        self.timers = {}

    def nametowidget(self, name):
        return self

    def after(self, ms, func):
        timer = f"after#{len(self.timers) + 1}"
        self.timers[timer] = func
        return timer

    def after_cancel(self, timer):
        del self.timers[timer]


def test_memory_is_released_once_after_a_burst_of_closes(monkeypatch):
    root = _FakeRoot()
    trims = []
    monkeypatch.setattr(core, "_release_after", None)
    monkeypatch.setattr(core.platform_, "trim_memory", lambda: trims.append(1))
    core.release_memory_soon(root)
    core.release_memory_soon(root)  # close, reopen, close: one pass
    assert len(root.timers) == 1
    (release,) = root.timers.values()
    release()
    assert trims == [1] and core._release_after is None


def test_trim_memory_is_safe_to_call():
    core.trim_memory()


def test_the_log_file_stays_under_its_limit_and_keeps_one_older_file(tmp_path):
    import logging
    import weeknumber_core as core
    path = tmp_path / "weeknumber.log"
    handler = core.SizeLimitedLog(path, 300)
    handler.setFormatter(logging.Formatter("%(message)s"))
    logger = logging.getLogger("weeknumber.test-size-limit")
    logger.propagate = False
    logger.addHandler(handler)
    try:
        for i in range(100):
            logger.warning("record %03d with some padding text", i)
    finally:
        logger.removeHandler(handler)
        handler.close()
    current = path.read_text(encoding="utf-8")
    older = (tmp_path / "weeknumber.log.1").read_text(encoding="utf-8")
    assert len(current.encode()) < 300 and len(older.encode()) < 300
    assert current.splitlines()[-1] == "record 099 with some padding text"
    lines = older.splitlines() + current.splitlines()
    numbers = [int(line.split()[1]) for line in lines]
    assert numbers == list(range(numbers[0], 100))  # nothing lost in between
    assert sorted(p.name for p in tmp_path.iterdir()) == ["weeknumber.log", "weeknumber.log.1"]


def test_the_idle_app_does_not_import_logging_handlers(tmp_path):
    import os
    import subprocess
    import sys
    import weeknumber_core as core
    code = ("import sys; sys.path.insert(0, sys.argv[1]); import weeknumber_core as c; "
            "c.SizeLimitedLog(sys.argv[2], 1000).close(); "
            "print('logging.handlers' in sys.modules)")
    result = subprocess.run([sys.executable, "-c", code, os.path.dirname(core.__file__),
                             str(tmp_path / "x.log")],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    assert result.stdout.split() == ["False"]
