"""The Windows plumbing under the startup policy: the winreg store and the
location check. The decisions themselves are covered portably by
test_startup_policy.py."""

import os
import tempfile
from pathlib import Path

import pytest

if os.name != "nt":
    pytest.skip("Windows registry and Win32 location checks", allow_module_level=True)

from weeknumber_platform import _windows  # noqa: E402

pytestmark = pytest.mark.windows
TEST_VALUE = "WeekNumberPyTest"


def test_winreg_store_round_trip():
    store = _windows._WinregStore()
    try:
        store.run_set(TEST_VALUE, '"C:\\WeekNumber\\WeekNumber.exe"')
        assert store.run_get(TEST_VALUE) == '"C:\\WeekNumber\\WeekNumber.exe"'
        store.approved_set(TEST_VALUE, b"\x03" + b"\x00" * 11)
        assert store.approved_get(TEST_VALUE)[0] == 0x03
    finally:
        store.run_delete(TEST_VALUE)
        store.approved_delete(TEST_VALUE)
    assert store.run_get(TEST_VALUE) is None
    store.run_delete(TEST_VALUE)  # deleting a missing value is fine


@pytest.mark.parametrize(("path", "fragment"), [
    (Path("\\\\server\\share\\WeekNumber.exe"), "network share"),
    (Path(tempfile.gettempdir()) / "WeekNumber.exe", "temporary folder"),
    (Path(os.environ.get("ProgramData", "C:\\ProgramData")) / "WeekNumber.exe", "ProgramData"),
])
def test_location_problem_refuses_risky_places(path, fragment):
    problem = _windows._location_problem(path)
    assert problem is not None and fragment in problem


def test_location_problem_allows_a_per_user_install():
    local = Path(os.environ["LOCALAPPDATA"]) / "Programs" / "WeekNumber" / "WeekNumber.exe"
    assert _windows._location_problem(local) is None


def test_location_problem_allows_program_files():
    installed = Path(os.environ.get("ProgramFiles", "C:\\Program Files")) / "WeekNumber" / "WeekNumber.exe"
    assert _windows._location_problem(installed) is None


def test_move_hint_leads_with_program_files():
    assert "Program Files" in _windows._MOVE_HINT
    assert _windows._MOVE_HINT.index("Program Files") < _windows._MOVE_HINT.index("LOCALAPPDATA")
