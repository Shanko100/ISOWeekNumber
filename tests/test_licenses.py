"""Licenses.txt, the license file every build bundles."""

import pytest

import collect_licenses

SECTIONS = ["WeekNumber: LICENSE", "Third-party notices", "CPython (from the build's Python)",
            "Tcl (from the build's Python)", "Tk (from the build's Python)",
            "Pillow, with the native libraries in its wheel", "Build environment"]


@pytest.fixture(autouse=True)
def interpreter(monkeypatch, tmp_path):
    """Stand-ins for the build Python's own license files, whose location
    differs between Python distributions; the build scripts check the real
    ones."""
    found = []
    for name in ("CPython", "Tcl", "Tk"):
        path = tmp_path / f"{name}.txt"
        path.write_text(f"{name} license text", encoding="utf-8")
        found.append((name, path))
    monkeypatch.setattr(collect_licenses, "_interpreter_licenses", lambda: found)


@pytest.mark.parametrize("target", ["win32", "darwin"])
def test_every_license_text_that_applies_is_included(target):
    text = collect_licenses.build(target)
    for section in SECTIONS:
        assert f"\n{section}\n" in text
    assert "Tk license text" in text
    skipped = collect_licenses.NOT_BUNDLED[target] | collect_licenses.FROM_BUILD
    for path in collect_licenses.LICENSES.iterdir():
        assert (f"licenses/{path.name}\n" in text) == (path.name not in skipped), path.name


def test_only_the_windows_build_carries_pystray():
    windows, mac = collect_licenses.build("win32"), collect_licenses.build("darwin")
    assert "\npystray source\n" in windows and "\nlicenses/pystray-LGPL-3.0.txt\n" in windows
    assert "\npystray source\n" not in mac and "\nlicenses/pystray-LGPL-3.0.txt\n" not in mac
