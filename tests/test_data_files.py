import datetime as dt
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP_DIR = ROOT / "src" / "weeknumber"
COUNTRY_CODE = re.compile(r"^[A-Z]{2}$")


def load_json(name):
    with (APP_DIR / name).open("r", encoding="utf-8") as handle:
        return json.load(handle)


def test_country_names_schema():
    document = load_json("country_names.json")
    assert document["schema_version"] == 1
    assert document["language"] == "en"
    countries = document["countries"]
    assert isinstance(countries, dict)
    assert countries
    assert all(COUNTRY_CODE.fullmatch(code) for code in countries)
    assert all(isinstance(name, str) and name.strip() for name in countries.values())
    assert len(set(countries.values())) == len(countries)


def test_holiday_data_schema_and_provenance():
    payload = (APP_DIR / "holiday_data.jsonl").read_bytes()
    assert b"\r" not in payload  # a CRLF checkout would shift every offset
    lines = payload.split(b"\n")
    assert lines[-1] == b""  # every line ends in a newline
    index = json.loads(lines[0])
    assert set(index) == {"METADATA", "COUNTRIES"}
    metadata = index["METADATA"]
    assert metadata["schema_version"] == 3
    provenance = metadata["provenance"]
    assert provenance["project"] == "python-holidays / Open World Holidays Framework"
    assert provenance["version"] == "0.106"
    assert metadata["coverage"]["years"] == [2000, 2050]

    countries = index["COUNTRIES"]
    tables = lines[1:-1]
    codes = [row[0] for row in countries]
    assert len(codes) == len(set(codes)) == len(tables)
    # The index lengths tile the file: the app seeks by adding them up.
    assert len(lines[0]) + 1 + sum(row[2] for row in countries) == len(payload)

    expected_years = {str(year) for year in range(2000, 2051)}
    for (code, _name, length), line in zip(countries, tables):
        assert COUNTRY_CODE.fullmatch(code)
        assert len(line) + 1 == length
        table = json.loads(line)
        assert set(table) == {"code", "years"}
        assert table["code"] == code
        years = table["years"]
        assert set(years) == expected_years
        for year_text, days in years.items():
            for date_text, names in days.items():
                day = dt.date.fromisoformat(date_text)
                assert day.year == int(year_text)
                assert isinstance(names, list)
                assert names
                assert all(isinstance(name, str) and name.strip() for name in names)


def test_timezone_data_has_bounded_valid_shapes():
    document = load_json("timezone_data.json")
    assert document["format"] == "weeknumber-timezone-3"
    assert isinstance(document["sources"], list) and document["sources"]
    assert isinstance(document["zones"], list) and document["zones"]
    assert isinstance(document["bands"], list) and document["bands"]
    assert isinstance(document["borders"], list)

    keys = set()
    for zone in document["zones"]:
        assert isinstance(zone["key"], str) and zone["key"]
        assert zone["key"] not in keys
        keys.add(zone["key"])
        assert -90 <= float(zone["lat"]) <= 90
        assert -180 <= float(zone["lon"]) <= 180
        assert isinstance(zone["label"], str) and zone["label"]

    for band in document["bands"]:
        assert -24 <= float(band["utc"]) <= 24
        for group in ("sea", "land"):
            for ring in band.get(group, []):
                assert len(ring) >= 6
                assert len(ring) % 2 == 0


def test_icon_is_a_nonempty_windows_icon():
    payload = (APP_DIR / "app.ico").read_bytes()
    assert len(payload) > 6
    assert payload[:4] == b"\x00\x00\x01\x00"
    assert int.from_bytes(payload[4:6], "little") >= 1


def test_app_icons_carry_every_size():
    # A frame for every window-icon size Windows asks for from 100 % to
    # 300 % scaling, so the taskbar never has to resample one (blurry).
    from PIL import Image

    import generate_icons
    with Image.open(APP_DIR / "app.ico") as ico:
        assert sorted(ico.info["sizes"]) == [(n, n) for n in generate_icons.ICO_SIZES]
    for small, big in ((16, 32), (20, 40), (24, 48), (28, 56), (32, 64), (40, 80), (48, 96)):
        assert small in generate_icons.ICO_SIZES and big in generate_icons.ICO_SIZES
    with Image.open(APP_DIR / "app.icns") as icns:
        sizes = {(w, scale) for w, _h, scale in icns.info["sizes"]}
        assert {(512, 2), (256, 1), (128, 1), (16, 2)} <= sizes
