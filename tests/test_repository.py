import ast
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP_DIR = ROOT / "src" / "weeknumber"

PROGRAM_FILES = [
    "weeknumber.py",
    "weeknumber_about.py",
    "weeknumber_license_dialog.py",
    "weeknumber_color_dialog.py",
    "weeknumber_calendar.py",
    "weeknumber_clipboard.py",
    "weeknumber_core.py",
    "weeknumber_holiday_dialog.py",
    "weeknumber_timezone.py",
    "weeknumber_timezone_dialog.py",
    "weeknumber_tray.py",
    "weeknumber_menubar.py",
    "weeknumber_map.py",
    "weeknumber_widgets.py",
    "weeknumber_dialog.py",
    "weeknumber_tzd_rules.py",
    "weeknumber_tzd_net.py",
    "weeknumber_tzd_relay.py",
    "weeknumber_tzd_board.py",
    "weeknumber_tzd_lobby.py",
    "weeknumber_tzd_chat.py",
    "weeknumber_tzd_chat_view.py",
    "weeknumber_tzd_fx.py",
    "weeknumber_tzd_sound.py",
    "weeknumber_tzd_dialog.py",
    "weeknumber_platform/__init__.py",
    "weeknumber_platform/_common.py",
    "weeknumber_platform/_windows.py",
    "weeknumber_platform/_windows_tz.py",
    "weeknumber_platform/_macos.py",
    "weeknumber_platform/_cocoa.py",
    "weeknumber_platform/_macos_tz.py",
    "generate_holiday_data.py",
    "generate_tzd_data.py",
    "generate_icons.py",
    "collect_licenses.py",
]


def test_all_python_sources_parse():
    for name in PROGRAM_FILES:
        source = (APP_DIR / name).read_text(encoding="utf-8")
        ast.parse(source, filename=name)


def test_required_release_documents_exist():
    required = [
        ROOT / "LICENSE",
        ROOT / "README.md",
        ROOT / "docs" / "THIRD_PARTY_NOTICES.md",
        ROOT / "docs" / "PROVENANCE.md",
        ROOT / "docs" / "BINARY_RELEASE_CHECKLIST.md",
        ROOT / "requirements-release.txt",
        ROOT / "CHANGELOG.md",
        ROOT / "docs" / "RELEASE_NOTES_v1.0.md",
    ]
    assert all(path.is_file() for path in required)


def test_copyright_owners_are_documented():
    # Whole-line match: a substring check would also accept extra owners.
    expected = "Copyright (c) 2026 Shanko100"
    for document in ("LICENSE", "README.md"):
        lines = (ROOT / document).read_text(encoding="utf-8").splitlines()
        assert expected in (line.strip() for line in lines), document


def test_embedded_project_url_is_documented():
    source = (APP_DIR / "weeknumber_core.py").read_text(encoding="utf-8")
    assert 'APP_URL = "https://github.com/Shanko100/ISOWeekNumber"' in source


def test_release_version_is_1_0():
    source = (APP_DIR / "weeknumber_core.py").read_text(encoding="utf-8")
    assert 'APP_VERSION = "1.0"' in source
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    assert "## [1.0] - 2026-10-23" in changelog
