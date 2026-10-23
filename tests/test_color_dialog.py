"""The icon colour picker's colour maths. Tk-free."""

import pytest

from types import SimpleNamespace

from weeknumber_color_dialog import (
    PRESET_COLUMNS,
    PRESETS,
    ColorPickerDialog,
    hex_to_hsv,
    hsv_to_hex,
    parse_hex,
)


@pytest.mark.parametrize("color", [c for c, _name in PRESETS] + ["#123456", "#ABCDEF"])
def test_hex_survives_the_round_trip_through_hsv(color):
    assert hsv_to_hex(*hex_to_hsv(color)) == color


@pytest.mark.parametrize("text, expected", [
    ("#ff8800", "#FF8800"), ("FF8800", "#FF8800"), ("  #00aaFF ", "#00AAFF"),
    ("#ff880", None), ("#gg8800", None), ("#1_2345", None), ("", None), ("#FF88001", None),
])
def test_only_six_hex_digits_parse(text, expected):
    assert parse_hex(text) == expected


def test_presets_are_valid_and_include_the_default():
    assert "#FFFFFF" in [color for color, _name in PRESETS]
    assert all(parse_hex(color) == color for color, _name in PRESETS)
    assert len({color for color, _name in PRESETS}) == len(PRESETS)


def test_hsv_is_clamped():
    assert hsv_to_hex(0.0, 2.0, 2.0) == "#FF0000"
    assert hsv_to_hex(1.5, 0.0, -1.0) == "#000000"


def test_there_are_three_full_rows_of_standard_colours():
    assert len(PRESETS) == 3 * PRESET_COLUMNS
    assert len({name for _color, name in PRESETS}) == len(PRESETS)


def test_a_click_finds_the_swatch_in_its_row_and_column():
    picker = SimpleNamespace(_px=lambda value: value, SWATCH_PX=22, SWATCH_GAP_PX=6)
    at = lambda x, y: ColorPickerDialog.swatch_at(picker, x, y)  # noqa: E731
    step = 22 + 6
    assert at(2, 2) == 0
    assert at(2 + 11 * step + 10, 2 + 10) == 11
    assert at(2 + 10, 2 + step + 10) == PRESET_COLUMNS
    assert at(2 + 11 * step + 22, 2 + 2 * step + 22) == len(PRESETS) - 1
    assert at(2 + 24, 10) is None          # the gap between two swatches
    assert at(10, 2 + 3 * step + 5) is None  # below the last row
    assert at(2 + 12 * step + 5, 10) is None  # right of the last column
    assert at(0, 0) is None


def _lightness(color):
    return hex_to_hsv(color)[2] * (1 - hex_to_hsv(color)[1] / 2)


def test_rows_go_dark_normal_light_within_every_column():
    rows = [PRESETS[i:i + PRESET_COLUMNS] for i in range(0, len(PRESETS), PRESET_COLUMNS)]
    for column in range(PRESET_COLUMNS):
        dark, normal, light = (_lightness(row[column][0]) for row in rows)
        assert dark < normal < light, rows[0][column]


def test_each_coloured_column_keeps_its_hue():
    rows = [PRESETS[i:i + PRESET_COLUMNS] for i in range(0, len(PRESETS), PRESET_COLUMNS)]
    for column in range(2, PRESET_COLUMNS):  # the first two are greys
        hues = [hex_to_hsv(row[column][0])[0] for row in rows]
        spread = max(min(abs(a - b), 1 - abs(a - b)) for a in hues for b in hues)
        assert spread < 0.06, [row[column] for row in rows]
