import os
import plistlib
import shutil
import sys
import tempfile
import threading
from pathlib import Path

import pytest

if sys.platform != "darwin":
    pytest.skip("macOS platform backend", allow_module_level=True)

from weeknumber_platform import _macos, _macos_tz  # noqa: E402

pytestmark = pytest.mark.macos


def test_single_instance_lock(tmp_path):
    first = _macos.SingleInstance(tmp_path / "instance.lock")
    second = _macos.SingleInstance(tmp_path / "instance.lock")
    assert first.acquire()
    assert not second.acquire()
    first.release()
    assert second.acquire()
    second.release()


@pytest.fixture
def socket_path():
    # pytest's tmp_path is too deep for AF_UNIX's 104-byte limit.
    directory = Path(tempfile.mkdtemp(prefix="wn"))
    yield directory / "q.sock"
    shutil.rmtree(directory, ignore_errors=True)


def test_quit_signal_wakes_the_incumbent(socket_path):
    path = socket_path
    incumbent = _macos.QuitSignal(path)
    woke = threading.Event()
    incumbent.host(woke.set)
    assert oct(os.stat(path).st_mode & 0o777) == oct(0o600)
    assert _macos.QuitSignal(path).signal()
    assert woke.wait(3)
    incumbent.release()
    assert not path.exists()


def test_quit_signal_ignores_other_payloads(socket_path):
    import socket
    path = socket_path
    incumbent = _macos.QuitSignal(path)
    woke = threading.Event()
    incumbent.host(woke.set)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.connect(str(path))
        client.sendall(b"hello\n")
    assert not woke.wait(0.8)
    incumbent.release()


def fake_install(monkeypatch, location):
    monkeypatch.setattr(_macos, "launch_location", lambda: location)
    monkeypatch.setattr(_macos, "launch_argv", lambda: [location])


def test_launch_agent_round_trip(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    app = Path("/Applications/WeekNumber.app/Contents/MacOS/WeekNumber")
    fake_install(monkeypatch, app)
    assert not _macos.is_startup_enabled()
    assert _macos.set_startup(True) is None
    agent = plistlib.loads(_macos.launch_agent_path().read_bytes())
    assert agent["Label"] == _macos.LAUNCH_AGENT_LABEL
    assert agent["RunAtLoad"] is True
    assert agent["ProgramArguments"] == [str(app)]
    assert _macos.is_startup_enabled()
    assert _macos.set_startup(False) is None
    assert not _macos.is_startup_enabled()


def test_launch_agent_refused_from_downloads(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    downloads = tmp_path / "Downloads"
    downloads.mkdir()
    fake_install(monkeypatch, downloads / "WeekNumber.app" / "Contents" / "MacOS" / "WeekNumber")
    problem = _macos.set_startup(True)
    assert problem is not None and "Downloads" in problem
    assert not _macos.launch_agent_path().exists()


def test_tk_rect_conversion_flips_y():
    class Size:
        def __init__(self, width, height):
            self.width, self.height = width, height

    class Origin:
        def __init__(self, x, y):
            self.x, self.y = x, y

    class Frame:
        def __init__(self, x, y, w, h):
            self.origin, self.size = Origin(x, y), Size(w, h)

    # A 1440x900 primary screen with a 25 pt menu bar on top.
    visible = _macos._to_tk_rect(Frame(0, 0, 1440, 875), 900)
    assert (visible.left, visible.top, visible.right, visible.bottom) == (0, 25, 1440, 900)


ZONES = [
    {"key": "W. Europe Standard Time", "iana": "Europe/Berlin"},
    {"key": "Tokyo Standard Time", "iana": "Asia/Tokyo"},
    {"key": "Bogus", "iana": "Not/AZone"},
    {"key": "x" * 200, "iana": "Europe/Paris"},
]


def test_zoneinfo_backend_maps_windows_keys(monkeypatch):
    monkeypatch.setattr(_macos_tz, "_system_iana", lambda: "Asia/Tokyo")
    _macos_tz.register_zones(ZONES)
    assert set(_macos_tz.load_system_zones()) == {"W. Europe Standard Time",
                                                  "Tokyo Standard Time"}
    assert _macos_tz.system_zone_key() == "Tokyo Standard Time"
    assert _macos_tz.standard_offset_minutes("W. Europe Standard Time") == 60


def test_system_zone_matches_rules_when_not_listed(monkeypatch):
    monkeypatch.setattr(_macos_tz, "_system_iana", lambda: "Europe/Amsterdam")
    _macos_tz.register_zones(ZONES)
    assert _macos_tz.system_zone_key() == "W. Europe Standard Time"


def test_unlisted_system_zone_gets_a_synthetic_key(monkeypatch):
    monkeypatch.setattr(_macos_tz, "_system_iana", lambda: "Asia/Kathmandu")
    _macos_tz.register_zones(ZONES)
    key = _macos_tz.system_zone_key()
    assert key == "IANA:Asia/Kathmandu"
    assert _macos_tz.standard_offset_minutes(key) == 5 * 60 + 45
