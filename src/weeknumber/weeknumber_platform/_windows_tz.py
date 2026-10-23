"""Windows time-zone backend: the OS owns the rules.

Zone identities come from ``EnumDynamicTimeZoneInformation``; UTC -> local
conversion goes through ``SystemTimeToTzSpecificLocalTimeEx``, which applies
*dynamic* DST, i.e. the per-year transition rules under
``HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Time Zones\\<key>\\Dynamic DST``.
Standard/summer time, mid-year rule changes and historical rules are
therefore handled by the same code path Windows uses for its own clock, and
stay correct as Windows Update refreshes the tables.
"""

from __future__ import annotations

import ctypes
import datetime as dt
import struct
import winreg
from ctypes import wintypes

from weeknumber_platform._common import MAX_KEY_CHARS, log
from weeknumber_platform._windows import LOAD_LIBRARY_SEARCH_SYSTEM32, kernel32

# ---------------------------------------------------------------------------
# Win32 time-zone interop
# ---------------------------------------------------------------------------
ERROR_NO_MORE_ITEMS = 259
TIME_ZONE_ID_INVALID = 0xFFFFFFFF


class SYSTEMTIME(ctypes.Structure):
    _fields_ = (
        ("wYear", wintypes.WORD),
        ("wMonth", wintypes.WORD),
        ("wDayOfWeek", wintypes.WORD),
        ("wDay", wintypes.WORD),
        ("wHour", wintypes.WORD),
        ("wMinute", wintypes.WORD),
        ("wSecond", wintypes.WORD),
        ("wMilliseconds", wintypes.WORD),
    )


class TIME_ZONE_INFORMATION(ctypes.Structure):
    _fields_ = (
        ("Bias", ctypes.c_long),
        ("StandardName", ctypes.c_wchar * 32),
        ("StandardDate", SYSTEMTIME),
        ("StandardBias", ctypes.c_long),
        ("DaylightName", ctypes.c_wchar * 32),
        ("DaylightDate", SYSTEMTIME),
        ("DaylightBias", ctypes.c_long),
    )


class DYNAMIC_TIME_ZONE_INFORMATION(ctypes.Structure):
    _fields_ = TIME_ZONE_INFORMATION._fields_ + (
        ("TimeZoneKeyName", ctypes.c_wchar * 128),
        ("DynamicDaylightTimeDisabled", ctypes.c_ubyte),
    )


#: Where the time-zone entry points actually live. Documentation lists
#: Kernel32.dll for all of them, but the Windows 8 additions -- notably
#: EnumDynamicTimeZoneInformation -- are only exported by KernelBase.dll and
#: the api-set stub on several builds; kernel32 does not forward them, so
#: GetProcAddress against kernel32 alone fails. Both extras are already mapped
#: into every process (kernel32 depends on KernelBase), and both are pinned to
#: System32 to match the module's DLL-hardening policy.
def _extra_dlls() -> list:
    dlls = []
    for name in ("kernelbase", "api-ms-win-core-timezone-l1-1-0"):
        try:
            dlls.append(ctypes.WinDLL(name, use_last_error=True,
                                      winmode=LOAD_LIBRARY_SEARCH_SYSTEM32))
        except Exception:
            # Optional extras: kernel32 alone still covers the pre-Windows 8
            # entry points, and the registry path covers enumeration.
            log.info("Optional time-zone DLL %s not loaded", name)
    return dlls


_TIME_DLLS = [kernel32] + _extra_dlls()


def _bind(name: str, argtypes: tuple, restype):
    """Resolve one export across the candidate DLLs, or None.

    Absence must degrade the feature, never the process: an import-time
    AttributeError would kill the app before any error barrier exists.
    """
    for dll in _TIME_DLLS:
        try:
            function = getattr(dll, name)
        except AttributeError:
            continue
        function.argtypes = argtypes
        function.restype = restype
        return function
    log.warning("Time-zone export %s not found in kernel32/kernelbase/api-set", name)
    return None


_enum_zones = _bind("EnumDynamicTimeZoneInformation",
                    (wintypes.DWORD, ctypes.POINTER(DYNAMIC_TIME_ZONE_INFORMATION)),
                    wintypes.DWORD)
_convert_ex = _bind("SystemTimeToTzSpecificLocalTimeEx",
                    (ctypes.POINTER(DYNAMIC_TIME_ZONE_INFORMATION),
                     ctypes.POINTER(SYSTEMTIME), ctypes.POINTER(SYSTEMTIME)),
                    wintypes.BOOL)
#: Windows 2000 fallback for the conversion above.
_convert = _bind("SystemTimeToTzSpecificLocalTime",
                 (ctypes.POINTER(TIME_ZONE_INFORMATION),
                  ctypes.POINTER(SYSTEMTIME), ctypes.POINTER(SYSTEMTIME)),
                 wintypes.BOOL)
#: Resolves a zone's per-year rules by TimeZoneKeyName, keeping dynamic DST
#: available even when the Ex conversion is not.
_tz_for_year = _bind("GetTimeZoneInformationForYear",
                     (wintypes.WORD,
                      ctypes.POINTER(DYNAMIC_TIME_ZONE_INFORMATION),
                      ctypes.POINTER(TIME_ZONE_INFORMATION)),
                     wintypes.BOOL)
_current_zone = _bind("GetDynamicTimeZoneInformation",
                      (ctypes.POINTER(DYNAMIC_TIME_ZONE_INFORMATION),),
                      wintypes.DWORD)


#: Windows key -> its DYNAMIC_TIME_ZONE_INFORMATION, enumerated once. The
#: registry set does not change while the process runs (a Windows Update that
#: rewrites it does not retro-fit running processes either), and the structs
#: are the documented input to the conversion API.
_SYSTEM_ZONES: dict[str, DYNAMIC_TIME_ZONE_INFORMATION] = {}
_SYSTEM_ZONES_LOADED = False


TIME_ZONES_KEY = r"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Time Zones"
#: REG_TZI_FORMAT: three LONG biases followed by two SYSTEMTIMEs.
_REG_TZI = struct.Struct("<3l16H")
MAX_SYSTEM_ZONES = 1000


def _zone_from_registry(root, name: str) -> "DYNAMIC_TIME_ZONE_INFORMATION | None":
    try:
        with winreg.OpenKey(root, name) as key:
            blob, blob_type = winreg.QueryValueEx(key, "TZI")
            standard = _reg_str(key, "Std")
            daylight = _reg_str(key, "Dlt")
    except OSError:
        return None
    if blob_type != winreg.REG_BINARY or not isinstance(blob, bytes):
        return None
    if len(blob) != _REG_TZI.size:
        return None
    values = _REG_TZI.unpack(blob)
    info = DYNAMIC_TIME_ZONE_INFORMATION()
    info.Bias, info.StandardBias, info.DaylightBias = values[0:3]
    info.StandardDate = SYSTEMTIME(*values[3:11])
    info.DaylightDate = SYSTEMTIME(*values[11:19])
    info.StandardName = standard[:31]
    info.DaylightName = daylight[:31]
    info.TimeZoneKeyName = name[:MAX_KEY_CHARS]
    info.DynamicDaylightTimeDisabled = 0
    return info


def _reg_str(key, name: str) -> str:
    try:
        value, value_type = winreg.QueryValueEx(key, name)
    except OSError:
        return ""
    return value if value_type == winreg.REG_SZ and isinstance(value, str) else ""


def _zones_from_registry() -> dict:
    """Enumerate HKLM's time-zone table directly.

    Used when EnumDynamicTimeZoneInformation cannot be resolved. Read-only,
    machine-wide, no elevation. Only the key name and the base TZI blob are
    taken; the per-year Dynamic DST rules stay with Windows, which still
    applies them because the conversion is driven by TimeZoneKeyName.
    """
    zones: dict = {}
    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, TIME_ZONES_KEY) as root:
            index = 0
            while index < MAX_SYSTEM_ZONES:
                try:
                    name = winreg.EnumKey(root, index)
                except OSError:
                    break
                index += 1
                if not name or len(name) > MAX_KEY_CHARS:
                    continue
                info = _zone_from_registry(root, name)
                if info is not None:
                    zones[name] = info
    except OSError:
        log.exception("Could not read the Windows time-zone registry table")
    return zones


def _zones_from_api() -> dict:
    """Preferred path: ask Windows to walk its own zone table."""
    zones: dict = {}
    if _enum_zones is None:
        return zones
    index = 0
    while index < MAX_SYSTEM_ZONES:
        info = DYNAMIC_TIME_ZONE_INFORMATION()
        try:
            result = _enum_zones(index, ctypes.byref(info))
        except OSError:
            log.exception("EnumDynamicTimeZoneInformation failed")
            break
        if result == ERROR_NO_MORE_ITEMS:
            break
        if result == 0 and info.TimeZoneKeyName:
            zones[info.TimeZoneKeyName] = info
        index += 1
    return zones


def _load_system_zones() -> dict:
    """Every usable zone, resolved once. API first, registry second, the
    machine's own zone as the last resort."""
    global _SYSTEM_ZONES_LOADED
    if _SYSTEM_ZONES_LOADED:
        return _SYSTEM_ZONES
    _SYSTEM_ZONES_LOADED = True
    zones = _zones_from_api()
    source = "EnumDynamicTimeZoneInformation"
    if not zones:
        zones = _zones_from_registry()
        source = "registry"
    if not zones:
        current = _current_zone_info()
        if current is not None and current.TimeZoneKeyName:
            zones[current.TimeZoneKeyName] = current
        source = "current zone only"
    _SYSTEM_ZONES.update(zones)
    log.info("Loaded %d Windows time zones via %s", len(_SYSTEM_ZONES), source)
    return _SYSTEM_ZONES


def _current_zone_info() -> "DYNAMIC_TIME_ZONE_INFORMATION | None":
    if _current_zone is None:
        return None
    info = DYNAMIC_TIME_ZONE_INFORMATION()
    try:
        if _current_zone(ctypes.byref(info)) == TIME_ZONE_ID_INVALID:
            return None
    except OSError:
        log.exception("GetDynamicTimeZoneInformation failed")
        return None
    return info


def system_zone_key() -> "str | None":
    """Windows key name of the zone the machine is currently set to."""
    info = _current_zone_info()
    key = info.TimeZoneKeyName if info is not None else None
    if not key:
        return None
    # Only usable if we can also convert with it.
    return key if key in _load_system_zones() else None


def _to_systemtime(when: dt.datetime) -> SYSTEMTIME:
    return SYSTEMTIME(when.year, when.month, 0, when.day,
                      when.hour, when.minute, when.second, 0)


def _from_systemtime(value: SYSTEMTIME) -> "dt.datetime | None":
    try:
        return dt.datetime(value.wYear, value.wMonth, value.wDay,
                           value.wHour, value.wMinute, value.wSecond)
    except ValueError:
        return None


def _local_from_utc(key: str, utc: dt.datetime) -> "dt.datetime | None":
    """UTC -> wall clock in `key`, via the best conversion this build offers.

    1. SystemTimeToTzSpecificLocalTimeEx  -- dynamic DST, resolved by key name.
    2. GetTimeZoneInformationForYear + SystemTimeToTzSpecificLocalTime -- still
       the per-year rules, through the Windows 2000 conversion entry point.
    3. SystemTimeToTzSpecificLocalTime with the zone's base rules. Correct for
       the current era, which is all this app ever asks about.
    """
    info = _load_system_zones().get(key)
    if info is None:
        return None
    universal = _to_systemtime(utc)
    local = SYSTEMTIME()

    if _convert_ex is not None:
        try:
            if _convert_ex(ctypes.byref(info), ctypes.byref(universal),
                           ctypes.byref(local)):
                return _from_systemtime(local)
        except OSError:
            log.exception("SystemTimeToTzSpecificLocalTimeEx failed for %s", key)

    if _convert is None:
        log.warning("No time-zone conversion entry point available")
        return None

    static = TIME_ZONE_INFORMATION()
    resolved = False
    if _tz_for_year is not None:
        try:
            # The year must be a local-time year; using the UTC year can pick
            # the wrong rule set within a few hours of 1 January, which at most
            # shifts a transition date that is itself months away.
            resolved = bool(_tz_for_year(utc.year, ctypes.byref(info),
                                         ctypes.byref(static)))
        except OSError:
            log.exception("GetTimeZoneInformationForYear failed for %s", key)
    if not resolved:
        # DYNAMIC_TIME_ZONE_INFORMATION starts with TIME_ZONE_INFORMATION's
        # exact field sequence, so the prefix copies verbatim.
        ctypes.memmove(ctypes.byref(static), ctypes.byref(info),
                       ctypes.sizeof(TIME_ZONE_INFORMATION))
    try:
        if _convert(ctypes.byref(static), ctypes.byref(universal),
                    ctypes.byref(local)):
            return _from_systemtime(local)
    except OSError:
        log.exception("SystemTimeToTzSpecificLocalTime failed for %s", key)
    log.warning("Time-zone conversion failed for %s (err=%d)",
                key, ctypes.get_last_error())
    return None


def load_system_zones() -> dict:
    return _load_system_zones()


def register_zones(raw: object) -> None:
    """Windows already knows its zones; the bundled table only labels them."""


def local_from_utc(key: str, utc: dt.datetime) -> "dt.datetime | None":
    return _local_from_utc(key, utc)


def standard_offset_minutes(key: str) -> int:
    """Standard-time (winter) offset in minutes; the stable sort key for the
    dropdown, so the list does not reorder itself at a DST transition."""
    info = _load_system_zones().get(key)
    if info is None:
        return 0
    return -(info.Bias + info.StandardBias)

