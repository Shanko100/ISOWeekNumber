"""macOS time-zone backend: ``zoneinfo`` over the system tz database.

The bundled table keys every zone by its Windows name and also carries the
IANA name of its representative city. Settings keep storing the Windows key,
so a settings file moves between machines unchanged; this backend only maps
each key to a ``ZoneInfo`` for the conversion.

The machine's own zone comes from the ``/etc/localtime`` symlink. When the
table lists that exact IANA zone it is used directly; otherwise the listed
zone with identical rules this year is chosen (Amsterdam -> the Berlin entry),
and as a last resort the system zone gets a synthetic ``IANA:`` key so the
clock still works without a highlighted region -- the same behaviour as a
Windows zone the picker does not list.
"""

from __future__ import annotations

import datetime as dt
import os
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from weeknumber_platform._common import MAX_KEY_CHARS, log

MAX_IANA_CHARS = 64
MAX_ZONES = 400
SYSTEM_PREFIX = "IANA:"
LOCALTIME = "/etc/localtime"

#: Windows key -> ZoneInfo, for every table entry the system can resolve.
_ZONES: dict[str, ZoneInfo] = {}


def register_zones(raw: object) -> None:
    """Resolve the IANA name of each bundled entry. Invalid or unknown names
    are skipped, so a tampered file can never add a working zone."""
    _ZONES.clear()
    if not isinstance(raw, list):
        return
    for item in raw[:MAX_ZONES]:
        if not isinstance(item, dict):
            continue
        key, iana = item.get("key"), item.get("iana")
        if not isinstance(key, str) or not isinstance(iana, str):
            continue
        if not key or len(key) > MAX_KEY_CHARS or len(iana) > MAX_IANA_CHARS:
            continue
        try:
            _ZONES[key] = ZoneInfo(iana)
        except (ZoneInfoNotFoundError, ValueError, OSError):
            continue
    log.info("Resolved %d bundled time zones through zoneinfo", len(_ZONES))
    system_zone_key()  # registers the synthetic key when one is needed


def load_system_zones() -> dict:
    return _ZONES


def _system_iana() -> "str | None":
    try:
        target = os.readlink(LOCALTIME)
    except OSError:
        return None
    marker = "zoneinfo/"
    index = target.find(marker)
    if index < 0:
        return None
    name = target[index + len(marker):]
    return name if 0 < len(name) <= MAX_IANA_CHARS else None


def _signature(zone: ZoneInfo, year: int) -> tuple:
    probes = [dt.datetime(year, month, 15, 12, tzinfo=dt.timezone.utc)
              for month in range(1, 13)]
    return tuple(probe.astimezone(zone).utcoffset() for probe in probes)


def system_zone_key() -> "str | None":
    name = _system_iana()
    if name is None:
        return None
    for key, zone in _ZONES.items():
        if zone.key == name:
            return key
    synthetic = SYSTEM_PREFIX + name
    if synthetic in _ZONES:
        return synthetic
    try:
        local = ZoneInfo(name)
    except (ZoneInfoNotFoundError, ValueError, OSError):
        return None
    year = dt.date.today().year
    wanted = _signature(local, year)
    region = name.split("/", 1)[0]
    matches = [key for key, zone in _ZONES.items()
               if not key.startswith(SYSTEM_PREFIX)
               and _signature(zone, year) == wanted]
    if matches:
        matches.sort(key=lambda k: (_ZONES[k].key.split("/", 1)[0] != region, k))
        return matches[0]
    _ZONES[synthetic] = local
    return synthetic


def local_from_utc(key: str, utc: dt.datetime) -> "dt.datetime | None":
    zone = _ZONES.get(key)
    if zone is None:
        return None
    return utc.replace(tzinfo=dt.timezone.utc).astimezone(zone).replace(tzinfo=None)


def standard_offset_minutes(key: str) -> int:
    """Standard-time offset: summer time only ever adds, so it is the
    smallest offset the zone uses during the year."""
    zone = _ZONES.get(key)
    if zone is None:
        return 0
    offsets = _signature(zone, dt.date.today().year)
    return int(min(offsets).total_seconds() // 60)
