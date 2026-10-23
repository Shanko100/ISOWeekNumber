import json
from pathlib import Path

import pytest

import weeknumber_tzd_rules as rules

ROOT = Path(__file__).resolve().parents[1]
APP_DIR = ROOT / "src" / "weeknumber"
LAT_MIN, LAT_MAX = -58.0, 84.0


def load_document():
    with (APP_DIR / "tzd_data.json").open("r", encoding="utf-8") as handle:
        return json.load(handle)


@pytest.fixture(scope="module")
def board():
    return rules.Board.from_document(load_document())


def test_document_shape_and_provenance():
    document = load_document()
    assert document["format"] == "weeknumber-tzd-1"
    assert any("Natural Earth 5.1.2" in source for source in document["sources"])


def test_classic_board_dimensions(board):
    assert len(board.order) == 42
    assert {c.id: c.bonus for c in board.continents.values()} == {
        "north_america": 5, "south_america": 2, "europe": 5,
        "africa": 3, "asia": 7, "australia": 2,
    }
    sizes = {c.id: len(c.territories) for c in board.continents.values()}
    assert sizes == {"north_america": 9, "south_america": 4, "europe": 7,
                     "africa": 6, "asia": 12, "australia": 4}


def test_adjacency_is_symmetric_and_classic(board):
    for tid, neighbours in board.neighbours.items():
        for other in neighbours:
            assert tid in board.neighbours[other]
    edges = sum(len(n) for n in board.neighbours.values()) // 2
    assert edges == 83
    for a, b in [("alaska", "kamchatka"), ("brazil", "north_africa"),
                 ("greenland", "iceland"), ("east_africa", "middle_east"),
                 ("siam", "indonesia"), ("western_europe", "north_africa"),
                 ("ural", "china"), ("ukraine", "afghanistan")]:
        assert board.adjacent(a, b)
    assert not board.adjacent("alaska", "greenland")


def test_shapes_sit_inside_the_map_window(board):
    for territory in board.territories.values():
        assert territory.rings
        for ring in territory.rings:
            assert len(ring) >= 6 and len(ring) % 2 == 0
            assert all(LAT_MIN - 0.01 <= ring[i] <= LAT_MAX + 0.01
                       for i in range(1, len(ring), 2))
        lon, lat = territory.label
        assert -180 <= lon <= 180 and LAT_MIN <= lat <= LAT_MAX


def test_sea_routes_reference_linked_territories(board):
    assert board.routes
    for a, b, line in board.routes:
        assert board.adjacent(a, b)
        assert len(line) == 4


@pytest.mark.parametrize("mutate", [
    lambda d: d.update(format="other"),
    lambda d: d["territories"].append(dict(d["territories"][0])),
    lambda d: d["territories"][0].update(continent="atlantis"),
    lambda d: d["territories"][0].update(rings=[]),
    lambda d: d["links"].append({"a": "alaska", "b": "nowhere"}),
    lambda d: d["continents"][0].update(bonus=True),
])
def test_board_validation_fails_closed(mutate):
    document = load_document()
    mutate(document)
    with pytest.raises(rules.BoardError):
        rules.Board.from_document(document)


def test_fingerprint_is_stable_and_rules_sensitive():
    first = rules.Board.from_document(load_document()).fingerprint
    assert first == rules.Board.from_document(load_document()).fingerprint
    document = load_document()
    document["continents"][0]["bonus"] += 1
    assert rules.Board.from_document(document).fingerprint != first


def test_every_continent_has_its_own_outline_colour():
    import weeknumber_core as core
    from weeknumber_tzd_board import CONTINENT_COLORS, continent_color, continent_tint
    board = rules.load_board()
    assert len(board.continent_order) <= CONTINENT_COLORS
    for theme in (core.LIGHT_THEME, core.DARK_THEME):
        colors = [continent_color(theme, i) for i in range(len(board.continent_order))]
        assert len(set(colors)) == len(colors)
        assert all(continent_tint(theme, i) != colors[i] for i in range(len(colors)))


def test_both_palettes_define_the_same_colours():
    import weeknumber_core as core
    assert set(core.LIGHT_THEME) == set(core.DARK_THEME)
