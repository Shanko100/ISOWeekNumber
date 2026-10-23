"""Offline time-zone support for WeekNumber.

No network access, ever. Two data sources, both local:

* The operating system owns the rules. On Windows that is the dynamic-DST
  zone table behind ``SystemTimeToTzSpecificLocalTimeEx``; on macOS it is the
  system tz database through ``zoneinfo``. Either way standard/summer time and
  rule changes come from the OS and stay current with its updates. Accuracy is
  bounded by the OS clock and the OS time-zone data; a de-synchronised system
  clock is out of scope for this application. See
  ``weeknumber_platform/_windows_tz.py`` and ``_macos_tz.py``.

* ``timezone_data.json`` is non-executable bundled data: the dropdown entries
  (Windows time-zone key + label + representative city + its band index) plus
  the picker map itself -- Natural Earth's standard-offset time-zone polygons,
  coastlines and country borders, simplified. It is read with the same
  distrust as the holiday tables -- size-capped, schema- and range-validated --
  because it can sit in a user-writable directory next to the executable. A
  zone entry survives only if the OS can also convert with it (its Windows
  key on Windows, its IANA name on macOS), so a tampered or stale file can
  add labels but never a working zone.

  Bands are Natural Earth's *standard* (winter) offset regions, so a city is
  always inside the band drawn for it; the band is a political region, not the
  live UTC offset, which the clock reports separately.

This module is deliberately GUI-toolkit-free: it holds the time logic, the
zone table and the map projection. The dialog lives in
``weeknumber_timezone_dialog.py``.
"""

from __future__ import annotations

import datetime as dt
import json
import math
import re
import weakref
from array import array

from weeknumber_core import (
    IS_WINDOWS,
    bundled_file,
    load_bounded_json,
    log,
    read_bounded_text,
)
from weeknumber_platform._common import MAX_KEY_CHARS

if IS_WINDOWS:
    from weeknumber_platform import _windows_tz as _backend
else:
    from weeknumber_platform import _macos_tz as _backend


def _load_system_zones() -> dict:
    """Every zone key the active backend can convert with."""
    return _backend.load_system_zones()


def system_zone_key() -> "str | None":
    """Key of the zone the machine is currently set to."""
    return _backend.system_zone_key()


def _local_from_utc(key: str, utc: dt.datetime) -> "dt.datetime | None":
    return _backend.local_from_utc(key, utc)




def _utc_now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc).replace(tzinfo=None, microsecond=0)


def zone_now_and_offset(key: str) -> "tuple[dt.datetime, int] | None":
    """Wall-clock time and UTC offset in minutes for one instant.

    Both come from a single UTC sample, so they can never disagree across a
    minute boundary, and the offset is whatever the OS actually applied --
    summer time included.
    """
    utc = _utc_now()
    local = _local_from_utc(key, utc)
    if local is None:
        return None
    return local, int(round((local - utc).total_seconds() / 60.0))


def zone_standard_offset_minutes(key: str) -> int:
    """Standard-time (winter) offset in minutes; the stable sort key for the
    dropdown, so the list does not reorder itself at a DST transition."""
    return _backend.standard_offset_minutes(key)


def format_offset(minutes: "int | None") -> str:
    """Full form for the dropdown: UTC+01:00."""
    if minutes is None:
        return "UTC"
    sign = "-" if minutes < 0 else "+"
    minutes = abs(int(minutes))
    return f"UTC{sign}{minutes // 60:02d}:{minutes % 60:02d}"


def format_offset_short(minutes: "int | None") -> str:
    """Compact form for the calendar footer: UTC+2, UTC+5:30, UTC."""
    if not minutes:
        return "UTC"
    sign = "-" if minutes < 0 else "+"
    hours, rest = divmod(abs(int(minutes)), 60)
    return f"UTC{sign}{hours}" if rest == 0 else f"UTC{sign}{hours}:{rest:02d}"


# ---------------------------------------------------------------------------
# Map projection: equirectangular (plate carree) with the polar caps cropped
# ---------------------------------------------------------------------------
#: Latitude window. Cropping the caps is what every printed time-zone chart
#: does: the full range spends a third of the height on Antarctica and squeezes
#: the bands that actually need reading. Geometry outside the window is clamped
#: to the edge, so Antarctica still appears as the coloured strip along the
#: bottom instead of vanishing.
LAT_MAX = 84.0
LAT_MIN = -58.0
_LAT_MID = (LAT_MAX + LAT_MIN) / 2.0
_LAT_SPAN = (LAT_MAX - LAT_MIN) / 2.0
#: Width / height of the full map.
ASPECT = 360.0 / (LAT_MAX - LAT_MIN)


def project(lon: float, lat: float) -> tuple[float, float]:
    """(lon, lat) in degrees -> (x, y) normalized to -1..1 over the full map.

    Longitude maps straight to x, which is the whole point of this projection
    here: a time zone is a range of longitudes, so it comes out as an upright
    band the eye can follow from pole to pole. y grows north; callers flip it
    for screen coordinates.
    """
    nx = max(-1.0, min(1.0, lon / 180.0))
    ny = max(-1.0, min(1.0, (lat - _LAT_MID) / _LAT_SPAN))
    return nx, ny


def unproject(nx: float, ny: float) -> "tuple[float, float] | None":
    """Inverse of project(). None when the point lies outside the map."""
    if abs(nx) > 1.0 + 1e-9 or abs(ny) > 1.0 + 1e-9:
        return None
    return nx * 180.0, _LAT_MID + ny * _LAT_SPAN


def great_circle_km(lon1: float, lat1: float, lon2: float, lat2: float) -> float:
    phi1, phi2 = math.radians(lat1), math.radians(lat2)
    d_phi = phi2 - phi1
    d_lam = math.radians(lon2 - lon1)
    a = (math.sin(d_phi / 2.0) ** 2
         + math.cos(phi1) * math.cos(phi2) * math.sin(d_lam / 2.0) ** 2)
    return 12742.0 * math.asin(min(1.0, math.sqrt(a)))  # 2 * mean earth radius


# ---------------------------------------------------------------------------
# Bundled zone table and world outline
# ---------------------------------------------------------------------------
DATA_FILE = "timezone_data.json"
MAX_DATA_BYTES = 4 * 1024 * 1024
MAX_ZONES = 400
MAX_BANDS = 200
MAX_RINGS = 8000
MAX_RING_POINTS = 4000
MAX_TOTAL_POINTS = 400_000


class TimeZoneEntry:
    """One dropdown row: a Windows zone key, the single city the marker and the
    calendar footer name, its coordinates, and the index of the time-zone band
    the marker sits in (-1 when the file gave none).

    `city` is carried explicitly rather than derived from the label: the label
    lists several places, and the one the footer shows must be the one the
    marker stands on.
    """

    __slots__ = ("key", "label", "city", "lat", "lon", "band")

    def __init__(self, key: str, label: str, city: str, lat: float, lon: float,
                 band: int) -> None:
        self.key = key
        self.label = label
        self.city = city
        self.lat = lat
        self.lon = lon
        self.band = band


#: Populated by load_timezone_data() for the windows that hold it (see
#: use_zones); empty otherwise, and after a bad file.
ZONES: list[TimeZoneEntry] = []
#: Standard-offset time-zone regions, sorted west to east. Each entry is
#: {"sea": [rings], "land": [rings]} -- the whole band and the part of it that
#: is dry land, pre-clipped, because a Tk canvas cannot intersect polygons.
ZONE_BANDS: list["TimeZoneBand"] = []
#: Distinct fills the band palette cycles through.
BAND_TINTS = 5
#: Country borders as flat polylines.
WORLD_BORDERS: "list[array]" = []
#: The windows drawing the map. ZONE_BANDS and WORLD_BORDERS are in memory
#: only while one is open (see use_map).
_map_users: "weakref.WeakSet" = weakref.WeakSet()
#: The windows that need the zone table (see use_zones): ZONES is in memory
#: only while one is open. Nothing at startup needs it.
_zone_users: "weakref.WeakSet" = weakref.WeakSet()
#: Whether load_timezone_data succeeded; the map is not loaded otherwise.
_zones_loaded = False


def _parse_zones(raw: object, available: "set[str]",
                 band_count: int) -> list[TimeZoneEntry]:
    """Dropdown rows, keeping only zones Windows also knows, sorted by standard
    offset so the list does not reorder itself at a DST transition."""
    entries: list[TimeZoneEntry] = []
    seen: set[str] = set()
    if not isinstance(raw, list):
        return entries
    for item in raw[:MAX_ZONES]:
        if not isinstance(item, dict):
            continue
        key = item.get("key")
        label = item.get("label")
        lat = item.get("lat")
        lon = item.get("lon")
        if not isinstance(key, str) or not isinstance(label, str):
            continue
        if not isinstance(lat, (int, float)) or not isinstance(lon, (int, float)):
            continue
        if isinstance(lat, bool) or isinstance(lon, bool):
            continue
        key = key[:MAX_KEY_CHARS]
        if key in seen or key not in available:
            continue  # unknown to this Windows install -> not offerable
        if not (-90.0 <= float(lat) <= 90.0 and -180.0 <= float(lon) <= 180.0):
            continue
        city = item.get("city")
        if not isinstance(city, str) or not city.strip():
            city = label.split(",")[0].strip() or label
        band = item.get("band")
        if not isinstance(band, int) or isinstance(band, bool):
            band = -1
        elif not 0 <= band < band_count:
            band = -1  # out of range -> marker only, no highlighted region
        seen.add(key)
        entries.append(TimeZoneEntry(key, label[:80], city[:40],
                                     float(lat), float(lon), band))
    entries.sort(key=lambda z: (zone_standard_offset_minutes(z.key), z.label))
    return entries


class _PointBudget:
    """Shared cap across every geometry list in one file, so a hostile file
    cannot exhaust memory by spreading points over many small lists."""

    def __init__(self, limit: int) -> None:
        self.remaining = limit

    def take(self, points: int) -> bool:
        if points > self.remaining:
            return False
        self.remaining -= points
        return True


def _parse_rings(raw: object, budget: "_PointBudget",
                 min_values: int = 6) -> "list[array]":
    """Flat [lon, lat, ...] sequences, range-checked. Used for closed rings
    (min 3 points) and open polylines (min 2).

    Each is kept as a packed array of 32-bit floats: 4 bytes a value instead
    of a 32-byte Python float and its list slot, and exact to well under a
    metre at the two decimals the data files carry."""
    rings: list[array] = []
    if not isinstance(raw, list):
        return rings
    for ring in raw[:MAX_RINGS]:
        if not isinstance(ring, list) or len(ring) < min_values or len(ring) % 2:
            continue
        if len(ring) // 2 > MAX_RING_POINTS:
            continue
        if not all(isinstance(v, (int, float)) and not isinstance(v, bool)
                   for v in ring):
            continue
        if any(abs(float(ring[i])) > 180.0 for i in range(0, len(ring), 2)):
            continue
        if any(abs(float(ring[i])) > 90.0 for i in range(1, len(ring), 2)):
            continue
        if not budget.take(len(ring) // 2):
            log.warning("Map geometry exceeds the point budget; truncating")
            break
        rings.append(array("f", ring))
    return rings


class TimeZoneBand:
    """One standard-offset region, pre-cut into its sea part and its land part
    so the map can shade them differently with no geometry work at runtime.

    `tint` is the palette slot: bands arrive ordered west to east, so cycling
    the index guarantees neighbours never share a fill -- the convention every
    printed world time-zone chart uses.
    """

    __slots__ = ("utc", "sea", "land", "tint")

    def __init__(self, utc: float, sea: list, land: list, tint: int) -> None:
        self.utc = utc
        self.sea = sea
        self.land = land
        self.tint = tint


def _parse_bands(raw: object, budget: "_PointBudget") -> list[TimeZoneBand]:
    """Map regions, positionally: a band that parses to nothing keeps its slot
    because zone entries index into this list."""
    bands: list[TimeZoneBand] = []
    if not isinstance(raw, list):
        return bands
    for band in raw[:MAX_BANDS]:
        entry = band if isinstance(band, dict) else {}
        utc = entry.get("utc")
        if not isinstance(utc, (int, float)) or isinstance(utc, bool):
            utc = 0.0
        # Bands are positional: zone entries index into this list, so a band
        # that parses to nothing must still occupy its slot.
        bands.append(TimeZoneBand(
            max(-24.0, min(24.0, float(utc))),
            _parse_rings(entry.get("sea"), budget),
            _parse_rings(entry.get("land"), budget),
            len(bands) % BAND_TINTS))
    return bands


_JSON = json.JSONDecoder()
_JSON_SPACE = re.compile(r"[ \t\n\r]*")


def _read_zone_table(text: str) -> dict:
    """The data file's top-level members up to and including "zones",
    decoded one at a time. The map geometry after them -- some 64,000
    numbers -- stays undecoded text: as Python floats it would leave about
    2 MB behind for the rest of the run, and only a map window needs it."""
    space = _JSON_SPACE.match
    idx = space(text, 0).end()
    if text[idx:idx + 1] != "{":
        raise ValueError("expected an object")
    members: dict = {}
    idx += 1
    while "zones" not in members:
        idx = space(text, idx).end()
        if text[idx:idx + 1] != '"':
            break  # "}" -- or damage, which use_map's full decode reports
        key, idx = json.decoder.scanstring(text, idx + 1)
        idx = space(text, idx).end()
        if text[idx:idx + 1] != ":":
            raise ValueError(f"expected ':' at offset {idx}")
        members[key], idx = _JSON.raw_decode(text, space(text, idx + 1).end())
        idx = space(text, idx).end()
        if text[idx:idx + 1] != ",":
            break
        idx += 1
    return members


def _data_path():
    path = bundled_file(DATA_FILE)
    if path is None:
        log.info("%s not found; time-zone support disabled", DATA_FILE)
    return path


def load_timezone_data() -> None:
    """Load the bundled zone table now; windows call use_zones instead, so
    the table is only in memory while one needs it. Never raises: on any
    problem the feature degrades to "no zones", which the UI reports. The map
    geometry is left out: use_map loads it for the windows that draw it."""
    global _zones_loaded
    ZONES.clear()
    _zones_loaded = False
    try:
        path = _data_path()
        if path is None:
            return
        raw = _read_zone_table(read_bounded_text(path, MAX_DATA_BYTES))

        # macOS resolves each entry's IANA name here; Windows ignores it.
        _backend.register_zones(raw.get("zones"))
        available = set(_load_system_zones())
        if not available:
            log.warning("No system time zones enumerated; feature disabled")
            return

        # A band index past the real bands is caught where the map is drawn.
        ZONES.extend(_parse_zones(raw.get("zones"), available, MAX_BANDS))
        _zones_loaded = True
        log.info("Loaded %d time zones from %s", len(ZONES), DATA_FILE)
    except Exception:
        log.exception("Failed to load %s; time-zone support disabled", DATA_FILE)
        ZONES.clear()


def use_zones(owner: object, settings=None) -> bool:
    """Keep the zone table in memory while `owner` is open; the first holder
    loads it (a few ms). With `settings`, a saved zone this system no longer
    knows is cleared, as the startup check used to do. Pair with drop_zones.
    Returns whether zones are available."""
    _zone_users.add(owner)
    if not _zones_loaded:
        load_timezone_data()
    if settings is not None and _zones_loaded:
        try:
            settings.validate_timezone_key(available_zone_keys())
        except Exception:
            log.exception("Could not validate the saved time zone")
    return _zones_loaded


def drop_zones(owner: object) -> None:
    """`owner` closed; the table goes when the last holder does."""
    global _zones_loaded
    _zone_users.discard(owner)
    if not _zone_users and not _map_users:
        ZONES.clear()
        _backend.register_zones(None)  # macOS: the resolved ZoneInfo objects
        _zones_loaded = False


def use_clock(owner: object, settings=None) -> None:
    """What the calendar's clock chip needs while `owner` is open. Windows
    converts with the OS's own zone list, so nothing is loaded there; macOS
    maps the saved Windows key to its IANA zone through the zone table.
    Pair with drop_zones."""
    if IS_WINDOWS:
        if settings is not None:
            try:
                settings.validate_timezone_key(available_zone_keys())
            except Exception:
                log.exception("Could not validate the saved time zone")
        return
    use_zones(owner, settings)


def use_map(owner: object) -> None:
    """Keep the map geometry in memory while `owner`, a window that draws
    the map, is open; the first one loads it (about 15 ms). Pair with
    drop_map. The lists are filled in place: other modules import them.
    The map needs the zone table, so `owner` holds that too."""
    _map_users.add(owner)
    use_zones(owner)
    if ZONE_BANDS or not _zones_loaded:
        return
    try:
        path = _data_path()
        if path is None:
            return
        raw = load_bounded_json(path, MAX_DATA_BYTES)
        if not isinstance(raw, dict):
            log.warning("%s has invalid schema; ignoring", DATA_FILE)
            return
        budget = _PointBudget(MAX_TOTAL_POINTS)
        bands = _parse_bands(raw.pop("bands", None), budget)
        borders = _parse_rings(raw.pop("borders", None), budget, min_values=4)
    except Exception:
        log.exception("Failed to load the map from %s", DATA_FILE)
        return
    ZONE_BANDS[:] = bands
    WORLD_BORDERS[:] = borders


def drop_map(owner: object) -> None:
    """`owner` closed; the geometry goes when the last map window does."""
    _map_users.discard(owner)
    if not _map_users:
        ZONE_BANDS.clear()
        WORLD_BORDERS.clear()
    drop_zones(owner)


def available_zone_keys() -> set[str]:
    """Every zone key this system can convert with -- a superset of
    zone_keys(), and the right set to validate a saved setting against: the
    machine's own zone is legitimate even when the picker does not list it."""
    return set(_load_system_zones())


def find_zone(key: "str | None") -> "TimeZoneEntry | None":
    if not key:
        return None
    return next((zone for zone in ZONES if zone.key == key), None)


def resolve_zone_key(key: "str | None") -> "str | None":
    """The zone to actually display: the saved one when Windows still knows
    it, otherwise the machine's own zone, otherwise nothing."""
    if key and key in _load_system_zones():
        return key
    return system_zone_key()


def _point_in_ring(lon: float, lat: float, ring: list[float]) -> bool:
    """Ray casting on a flat [lon, lat, ...] ring."""
    inside = False
    count = len(ring) // 2
    for i in range(count):
        x1, y1 = ring[2 * i], ring[2 * i + 1]
        j = (i + 1) % count
        x2, y2 = ring[2 * j], ring[2 * j + 1]
        if (y1 > lat) != (y2 > lat):
            crossing = x1 + (lat - y1) * (x2 - x1) / (y2 - y1)
            if lon < crossing:
                inside = not inside
    return inside


def band_at(lon: float, lat: float) -> int:
    """Index of the time-zone band containing a point, or -1.

    Ray casting over the simplified rings: exact enough to route a click to the
    right region, and a miss near a simplified boundary costs nothing because
    the caller falls back to the nearest city.
    """
    for index, band in enumerate(ZONE_BANDS):
        # The sea rings are the whole band, land included.
        if any(_point_in_ring(lon, lat, ring) for ring in band.sea):
            return index
    return -1


def nearest_zone(lon: float, lat: float) -> "TimeZoneEntry | None":
    """Zone for a clicked point: the closest listed city inside the band that
    was clicked, so clicking Spain lands on Madrid rather than on London
    across the border. Falls back to the closest city anywhere."""
    if not ZONES:
        return None
    band = band_at(lon, lat)
    candidates = [z for z in ZONES if z.band == band] if band >= 0 else []
    return min(candidates or ZONES,
               key=lambda z: great_circle_km(lon, lat, z.lon, z.lat))
