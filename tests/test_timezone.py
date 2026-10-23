import pytest

import weeknumber_timezone as timezone


@pytest.mark.parametrize(
    ("minutes", "full", "short"),
    [
        (None, "UTC", "UTC"),
        (0, "UTC+00:00", "UTC"),
        (60, "UTC+01:00", "UTC+1"),
        (-330, "UTC-05:30", "UTC-5:30"),
        (345, "UTC+05:45", "UTC+5:45"),
    ],
)
def test_offset_formatting(minutes, full, short):
    assert timezone.format_offset(minutes) == full
    assert timezone.format_offset_short(minutes) == short


@pytest.mark.parametrize(
    ("lon", "lat"),
    [(-180.0, -58.0), (0.0, 0.0), (180.0, 84.0), (4.9, 52.37)],
)
def test_projection_round_trip(lon, lat):
    projected = timezone.project(lon, lat)
    restored = timezone.unproject(*projected)
    assert restored is not None
    assert restored[0] == pytest.approx(lon)
    assert restored[1] == pytest.approx(lat)


def test_unproject_rejects_outside_map():
    assert timezone.unproject(1.1, 0.0) is None
    assert timezone.unproject(0.0, -1.1) is None


def test_great_circle_distance():
    assert timezone.great_circle_km(0, 0, 0, 0) == pytest.approx(0)
    amsterdam_to_london = timezone.great_circle_km(4.9041, 52.3676, -0.1276, 51.5072)
    assert 340 < amsterdam_to_london < 380


def test_point_in_ring():
    square = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0]
    assert timezone._point_in_ring(5.0, 5.0, square)
    assert not timezone._point_in_ring(20.0, 5.0, square)


def test_point_budget_fails_closed():
    budget = timezone._PointBudget(3)
    assert budget.take(2)
    assert not budget.take(2)
    assert budget.remaining == 1


class _MapWindow:
    """Stands in for a window that draws the map."""


def test_bundled_zones_load_and_convert():
    timezone.load_timezone_data()
    assert timezone.ZONES
    assert timezone.ZONE_BANDS == []  # the map geometry waits for a map window
    tokyo = timezone.find_zone("Tokyo Standard Time")
    assert tokyo is not None and tokyo.city == "Tokyo"
    now, offset = timezone.zone_now_and_offset("Tokyo Standard Time")
    assert offset == 9 * 60
    assert timezone.zone_standard_offset_minutes("Tokyo Standard Time") == 9 * 60
    assert timezone.zone_standard_offset_minutes("W. Europe Standard Time") == 60


def test_map_geometry_lives_while_a_map_window_is_open():
    timezone.load_timezone_data()
    bands, borders = timezone.ZONE_BANDS, timezone.WORLD_BORDERS
    first, second = _MapWindow(), _MapWindow()
    timezone.use_map(first)
    assert bands and borders
    timezone.use_map(second)
    timezone.drop_map(first)
    assert bands  # the other window still draws it
    timezone.drop_map(second)
    assert bands == [] and borders == []
    # Filled and emptied in place: other modules import the lists by name.
    assert timezone.ZONE_BANDS is bands and timezone.WORLD_BORDERS is borders


def test_startup_decodes_only_the_zone_table():
    text = '{"format": "x", "zones": [{"key": "UTC"}], "bands": [ broken'
    assert timezone._read_zone_table(text) == {"format": "x", "zones": [{"key": "UTC"}]}


def test_map_rings_are_packed_floats():
    timezone.load_timezone_data()
    window = _MapWindow()
    timezone.use_map(window)
    try:
        ring = timezone.ZONE_BANDS[0].sea[0]
        assert ring.typecode == "f" and ring.itemsize == 4
        assert abs(ring[2] - round(ring[2], 2)) < 1e-4  # the file's two decimals
    finally:
        timezone.drop_map(window)


class _Settings:
    """Records the zone check a window's first use of the table runs."""

    def __init__(self):
        self.checked = None

    def validate_timezone_key(self, keys):
        self.checked = set(keys)


def test_the_zone_table_lives_while_a_window_holds_it():
    timezone.drop_zones(_MapWindow())  # nothing held: start from an empty table
    assert timezone.ZONES == []
    first, second, settings = _MapWindow(), _MapWindow(), _Settings()
    assert timezone.use_zones(first, settings)
    assert timezone.ZONES and settings.checked  # loaded, saved zone checked
    zones = timezone.ZONES
    timezone.use_zones(second)
    timezone.drop_zones(first)
    assert timezone.ZONES  # the other window still needs it
    timezone.drop_zones(second)
    assert timezone.ZONES == [] and timezone.ZONES is zones


def test_a_map_window_holds_the_zone_table_too():
    window = _MapWindow()
    timezone.drop_zones(_MapWindow())
    timezone.use_map(window)
    try:
        assert timezone.ZONES and timezone.ZONE_BANDS
    finally:
        timezone.drop_map(window)
    assert timezone.ZONES == [] and timezone.ZONE_BANDS == []
