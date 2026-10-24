"""The tray starts without any window: calendar, dialogs and Timezone Dominator load on
first use, and on macOS so does Pillow."""

import subprocess
import sys
from pathlib import Path

APP_DIR = Path(__file__).resolve().parents[1] / "src" / "weeknumber"

WINDOW_MODULES = {
    "weeknumber_calendar", "weeknumber_dialog", "weeknumber_widgets",
    "weeknumber_about", "weeknumber_holiday_dialog", "weeknumber_timezone_dialog",
    "weeknumber_map", "weeknumber_license_dialog", "weeknumber_color_dialog",
}


def _loaded_after(statement: str) -> "set[str]":
    probe = f"import sys; {statement}; print(','.join(sorted(sys.modules)))"
    result = subprocess.run([sys.executable, "-c", probe], cwd=APP_DIR,
                            capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, result.stderr
    return set(result.stdout.strip().split(","))


def test_the_tray_imports_no_window_or_game_module():
    loaded = _loaded_after("import weeknumber_tray")
    assert not {m for m in loaded if m.startswith("weeknumber_tzd")}
    assert not loaded & WINDOW_MODULES
    assert "tkinter.colorchooser" not in loaded


def test_the_tray_imports_no_temp_file_machinery():
    # tempfile brings shutil, bz2, lzma and random: only a settings save
    # or a startup-entry check needs it, so it loads then.
    loaded = _loaded_after("import weeknumber_tray")
    assert "tempfile" not in loaded


def test_startup_reads_no_zone_table():
    loaded = _loaded_after("import weeknumber")
    assert "weeknumber_timezone" not in loaded


def test_every_window_class_the_tray_reaches_exists():
    import importlib
    import weeknumber_tray
    for module_name, class_name in weeknumber_tray.WINDOW_CLASSES:
        module = importlib.import_module(module_name)
        cls = getattr(module, class_name)
        assert callable(getattr(cls, "refresh_open")) and callable(getattr(cls, "_live"))
