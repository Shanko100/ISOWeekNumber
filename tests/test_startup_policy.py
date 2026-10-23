"""Run-at-startup decisions (weeknumber_platform._common), tested on every OS
with an in-memory stand-in for the Windows registry."""

import os
from pathlib import Path, PureWindowsPath

import pytest

from weeknumber_platform import _common as c


class FakeStore(c.RunStore):
    def __init__(self, run=None, approved=None):
        self.run = dict(run or {})
        self.approved = dict(approved or {})

    def run_get(self, name):
        return self.run.get(name)

    def run_set(self, name, command):
        self.run[name] = command

    def run_delete(self, name):
        self.run.pop(name, None)

    def approved_get(self, name):
        return self.approved.get(name)

    def approved_set(self, name, data):
        self.approved[name] = data

    def approved_delete(self, name):
        self.approved.pop(name, None)


HERE = Path(__file__).resolve()
EXE = [HERE]                                   # "frozen": one program path
SOURCE = [Path(os.__file__).resolve(), HERE]   # interpreter + script
DISABLED = b"\x03" + b"\x00" * 11


def test_containing_root_matches_whole_components(tmp_path):
    downloads = tmp_path / "Downloads"
    (downloads / "app").mkdir(parents=True)
    (tmp_path / "Downloads2").mkdir()
    roots = [(downloads, "your Downloads folder")]
    assert c.containing_root(downloads / "app" / "x.exe", roots) == "your Downloads folder"
    assert c.containing_root(tmp_path / "Downloads2" / "x.exe", roots) is None


def test_drive_root_contains_everything_on_it():
    # The old check appended a separator to "d:\" and never matched.
    assert c._root_prefix("d:\\", "\\") == "d:\\"
    assert c._root_prefix("d:\\downloads", "\\") == "d:\\downloads\\"
    root = Path(Path.cwd().anchor)
    assert c.containing_root(Path.cwd(), [(root, "drive")]) == "drive"


@pytest.mark.parametrize(("command", "paths"), [
    ('"C:\\Program Files\\WeekNumber\\WeekNumber.exe"', ["C:\\Program Files\\WeekNumber\\WeekNumber.exe"]),
    ('"C:\\py\\pythonw.exe" "C:\\src\\weeknumber.py"', ["C:\\py\\pythonw.exe", "C:\\src\\weeknumber.py"]),
    ("C:\\Tools\\other.exe --tray", ["C:\\Tools\\other.exe"]),
])
def test_command_paths(command, paths):
    assert c.command_paths(command) == paths


def test_points_to_this_app():
    assert c.points_to(c.quote_command(EXE), EXE)
    assert c.points_to(c.quote_command(SOURCE), SOURCE)
    # Source mode matches on the script, whatever interpreter was registered.
    assert c.points_to(f'"{Path("/elsewhere/pythonw")}" "{HERE}"', SOURCE)
    assert not c.points_to('"C:\\Tools\\WeekNumber.exe"', EXE)
    assert not c.points_to(None, EXE)


@pytest.mark.parametrize(("data", "disabled"), [
    (None, False), (b"", False), (c.APPROVED_ENABLED, False),
    (DISABLED, True), (b"\x07" + b"\x00" * 11, True), (b"\x06" + b"\x00" * 11, False),
])
def test_approved_disabled_uses_the_low_bit(data, disabled):
    assert c.approved_disabled(data) is disabled


def test_enable_writes_our_value_and_clears_a_disable():
    store = FakeStore(approved={c.RUN_VALUE: DISABLED})
    command = c.quote_command(EXE)
    assert c.enable_startup(store, command, None) is None
    assert store.run[c.RUN_VALUE] == command
    assert store.approved[c.RUN_VALUE] == c.APPROVED_ENABLED
    assert c.startup_enabled(store, EXE)
    c.disable_startup(store)
    assert c.RUN_VALUE not in store.run


def test_enable_refuses_with_a_reason():
    store = FakeStore()
    assert c.enable_startup(store, c.quote_command(EXE), "WeekNumber is in Temp.") \
        == "WeekNumber is in Temp."
    long_command = '"C:\\' + "x" * 300 + '.exe"'
    assert "too long" in c.enable_startup(store, long_command, None)
    assert store.run == {}


def test_enabled_requires_our_target_and_no_disable():
    command = c.quote_command(EXE)
    assert not c.startup_enabled(FakeStore(run={c.RUN_VALUE: '"C:\\other.exe"'}), EXE)
    assert not c.startup_enabled(FakeStore(run={c.RUN_VALUE: command},
                                           approved={c.RUN_VALUE: DISABLED}), EXE)
    assert c.startup_enabled(FakeStore(run={c.RUN_VALUE: command}), EXE)


def test_legacy_value_migrates_only_when_it_is_ours():
    ours = c.quote_command(EXE)
    store = FakeStore(run={c.LEGACY_RUN_VALUE: ours},
                      approved={c.LEGACY_RUN_VALUE: DISABLED})
    assert c.migrate_legacy(store, EXE)
    assert store.run == {c.RUN_VALUE: ours}
    assert store.approved == {c.RUN_VALUE: DISABLED}  # the user's choice survives

    foreign = FakeStore(run={c.LEGACY_RUN_VALUE: '"C:\\Tools\\WeekNumber.exe"'})
    assert not c.migrate_legacy(foreign, EXE)
    assert foreign.run == {c.LEGACY_RUN_VALUE: '"C:\\Tools\\WeekNumber.exe"'}


def test_refresh_never_repoints_to_another_existing_copy():
    store = FakeStore(run={c.RUN_VALUE: '"C:\\Program Files\\WeekNumber\\WeekNumber.exe"'})
    moved = c.refresh_startup(store, '"D:\\dev\\WeekNumber.exe"', None, exists=lambda p: True)
    assert not moved
    assert store.run[c.RUN_VALUE] == '"C:\\Program Files\\WeekNumber\\WeekNumber.exe"'


def test_refresh_repairs_an_entry_whose_app_moved():
    store = FakeStore(run={c.RUN_VALUE: '"C:\\Old\\WeekNumber.exe"'})
    assert c.refresh_startup(store, '"C:\\New\\WeekNumber.exe"', None, exists=lambda p: False)
    assert store.run[c.RUN_VALUE] == '"C:\\New\\WeekNumber.exe"'


@pytest.mark.parametrize(("approved", "problem"), [(DISABLED, None), (None, "risky")])
def test_refresh_respects_disable_and_location(approved, problem):
    store = FakeStore(run={c.RUN_VALUE: '"C:\\Old\\WeekNumber.exe"'},
                      approved={c.RUN_VALUE: approved} if approved else {})
    assert not c.refresh_startup(store, '"C:\\New\\WeekNumber.exe"', problem,
                                 exists=lambda p: False)
    assert store.run[c.RUN_VALUE] == '"C:\\Old\\WeekNumber.exe"'


def test_windows_style_paths_compare_case_insensitively_on_windows():
    a, b = "C:\\Program Files\\WeekNumber.exe", "c:\\program files\\WEEKNUMBER.EXE"
    assert c.same_path(a, b) is (os.name == "nt")
    assert str(PureWindowsPath(a)).casefold() == str(PureWindowsPath(b)).casefold()
