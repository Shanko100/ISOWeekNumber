"""The Legal notices window's text: what it shows and how it is styled.
Tk-free."""

import collect_licenses
import weeknumber_license_dialog as licenses


def test_a_source_checkout_shows_the_repository_texts(monkeypatch):
    monkeypatch.setattr(licenses, "bundled_file", lambda name: None)
    assert licenses.licenses_available()
    text = licenses.license_text()
    assert "MIT License" in text
    titles = [title for title, _body in licenses.split_sections(text) if title]
    assert titles[:4] == ["WeekNumber legal notices and licenses (source checkout)",
                          "Legal notices", "WeekNumber: LICENSE", "Third-party notices"]
    assert any(title.startswith("licenses/") for title in titles)


def test_the_legal_notices_come_first_and_cover_the_essentials(monkeypatch):
    monkeypatch.setattr(licenses, "bundled_file", lambda name: None)
    sections = dict(licenses.split_sections(licenses.license_text()))
    notices = sections["Legal notices"]
    for heading in ("## License and warranty", "## Trademarks", "## Privacy",
                    "## Encryption", "## App icon and artwork"):
        assert heading in notices
    assert "not affiliated with, endorsed by or sponsored by Hasbro" in " ".join(notices.split())
    assert "Timezone Dominator" not in notices  # the game stays a secret
    assert not notices.startswith("# ")       # the section title says it


def test_a_build_file_is_preferred_and_read_as_utf8(monkeypatch, tmp_path):
    path = tmp_path / "Licenses.txt"
    path.write_text("WeekNumber licenses\n===================\n\nhello\n", encoding="utf-8-sig")
    monkeypatch.setattr(licenses, "bundled_file", lambda name: path)
    text = licenses.license_text()
    assert text.startswith("WeekNumber licenses")  # no byte-order mark
    assert licenses.split_sections(text) == [("WeekNumber licenses", "hello")]


def test_an_oversized_or_broken_file_shows_nothing(monkeypatch, tmp_path):
    path = tmp_path / "Licenses.txt"
    path.write_bytes(b"\xff\xfe not utf-8")
    monkeypatch.setattr(licenses, "bundled_file", lambda name: path)
    assert licenses.license_text() is None
    monkeypatch.setattr(licenses, "MAX_LICENSE_BYTES", 4)
    path.write_text("far too long", encoding="utf-8")
    assert licenses.license_text() is None


def test_the_sections_of_a_real_build_file(monkeypatch, tmp_path):
    found = []
    for name in ("CPython", "Tcl", "Tk"):
        stub = tmp_path / f"{name}.txt"
        stub.write_text(f"{name} license text", encoding="utf-8")
        found.append((name, stub))
    monkeypatch.setattr(collect_licenses, "_interpreter_licenses", lambda: found)
    sections = licenses.split_sections(collect_licenses.build("win32"))
    titles = [title for title, _body in sections]
    assert titles[0].startswith("WeekNumber ")
    assert titles[0].endswith(" - legal notices and licenses")
    assert titles[1] == "Legal notices"
    for expected in ("WeekNumber: LICENSE", "Third-party notices", "pystray source",
                     "Tk (from the build's Python)", "Build environment"):
        assert expected in titles
    assert dict(sections)["Tk (from the build's Python)"] == "Tk license text"


def test_headings_are_styled_and_nothing_is_lost():
    text = ("Title\n=====\n\nOpening words.\n\n"
            + collect_licenses.section("Legal notices", "## Privacy\n\nNo telemetry.\n## Encryption\nTLS.")
            + collect_licenses.section("licenses/x.txt", "Terms\n  indented\n#notaheading"))
    runs = licenses.styled_runs(text)
    tags = [tag for _chunk, tag in runs]
    assert tags == ["heading", "intro", "heading", "subheading", "body", "subheading", "body",
                    "heading", "body"]
    assert [c for c, t in runs if t == "subheading"] == ["Privacy\n", "Encryption\n"]
    shown = "".join(chunk for chunk, _tag in runs)
    for words in ("Opening words.", "No telemetry.", "TLS.", "  indented", "#notaheading"):
        assert words in shown


def test_the_window_has_no_section_list():
    source = open(licenses.__file__, encoding="utf-8").read()
    assert "Combobox" not in source
