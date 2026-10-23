"""Generate WeekNumber's bundled offline nationwide holiday table.

This is a build-time tool only. The installed application reads the generated
JSON Lines and never imports python-holidays or accesses the network.
"""

from __future__ import annotations

import datetime as dt
import importlib.metadata
import json
import os
import re
import sys
import tempfile
from collections import Counter
from pathlib import Path

START_YEAR = 2000
END_YEAR = 2050
YEARS = tuple(range(START_YEAR, END_YEAR + 1))
EXPECTED_HOLIDAYS_VERSION = "0.106"
BASE_DIR = Path(__file__).resolve().parent
OUTPUT_PATH = BASE_DIR / "holiday_data.jsonl"
COUNTRY_NAMES_PATH = BASE_DIR / "country_names.json"
MAX_OUTPUT_BYTES = 64 * 1024 * 1024
MAX_HOLIDAYS_PER_YEAR = 366
MAX_NAMES_PER_DATE = 16
MAX_NAME_CHARS = 240
REQUIRED_CODES = frozenset({"CN", "KR", "TW"})
_COUNTRY_CODE_RE = re.compile(r"^[A-Z]{2}$")


def _library_version() -> str:
    """Return the installed python-holidays distribution version."""
    try:
        return importlib.metadata.version("holidays")
    except importlib.metadata.PackageNotFoundError as exc:
        raise RuntimeError(
            "The required build dependency holidays is not installed"
        ) from exc


# Country names are maintained as data rather than inferred from implementation
# class names. This guarantees that the UI always shows a complete, searchable
# English name while settings continue to store stable two-letter codes.
def _load_country_display_names() -> dict[str, str]:
    try:
        payload = COUNTRY_NAMES_PATH.read_bytes()
    except OSError as exc:
        raise RuntimeError(
            f"Cannot read country-name data: {COUNTRY_NAMES_PATH}"
        ) from exc
    if len(payload) > 64 * 1024:
        raise RuntimeError("country_names.json exceeds the 64 KiB safety limit")
    try:
        document = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RuntimeError("country_names.json is not valid UTF-8 JSON") from exc

    raw_names = document.get("countries") if isinstance(document, dict) else None
    if not isinstance(raw_names, dict):
        raise RuntimeError("country_names.json must contain a countries object")

    result: dict[str, str] = {}
    for code, name in raw_names.items():
        if (
            not isinstance(code, str)
            or not _COUNTRY_CODE_RE.fullmatch(code)
            or not isinstance(name, str)
        ):
            raise RuntimeError("country_names.json contains an invalid entry")
        clean = name.strip()
        if not 2 <= len(clean) <= 80 or clean.casefold() == code.casefold():
            raise RuntimeError(f"Invalid full country name for {code}")
        if clean in result.values():
            raise RuntimeError(f"Duplicate country display name: {clean}")
        result[code] = clean
    return result


def _country_display_name(code: str, names: dict[str, str]) -> str:
    try:
        return names[code]
    except KeyError as exc:
        raise ValueError(
            f"No full English country name is configured for {code}"
        ) from exc


def _names_for_date(calendar, day: dt.date, combined_name: str) -> list[str]:
    get_list = getattr(calendar, "get_list", None)
    values = get_list(day) if callable(get_list) else [combined_name]
    result: list[str] = []
    for value in values:
        if not isinstance(value, str):
            raise ValueError(f"Non-string holiday name for {day}")
        clean = value.strip()
        if not clean or len(clean) > MAX_NAME_CHARS:
            raise ValueError(f"Invalid holiday name for {day}")
        if clean not in result:
            result.append(clean)
    if not result or len(result) > MAX_NAMES_PER_DATE:
        raise ValueError(f"Invalid holiday-name count for {day}")
    return result


def _remove_weekly_rest_labels(
    days: dict[str, list[str]],
) -> dict[str, list[str]]:
    """Remove labels repeated weekly, while retaining any coincident holiday.

    Some national calendars model ordinary weekly rest days as holidays.
    A label occurring at least 40 times in one year cannot be a named annual
    public holiday, so only that repeated label is removed.
    """
    counts = Counter(name for names in days.values() for name in names)
    repeated = {name for name, count in counts.items() if count >= 40}
    if not repeated:
        return days
    cleaned: dict[str, list[str]] = {}
    for date_text, names in days.items():
        kept = [name for name in names if name not in repeated]
        if kept:
            cleaned[date_text] = kept
    return cleaned


def _generate_country(
    code: str,
    country_names: dict[str, str],
) -> tuple[str, dict[str, dict[str, list[str]]]]:
    import holidays
    from holidays.constants import PUBLIC

    calendar = holidays.country_holidays(
        code,
        years=YEARS,
        expand=False,
        observed=True,
        language="en_US",
        categories=(PUBLIC,),
    )
    country_name = _country_display_name(code, country_names)
    years = {str(year): {} for year in YEARS}

    for day, combined_name in sorted(calendar.items()):
        if not isinstance(day, dt.date) or not START_YEAR <= day.year <= END_YEAR:
            continue
        years[str(day.year)][day.isoformat()] = _names_for_date(
            calendar, day, combined_name
        )

    for year_text, days in years.items():
        cleaned = _remove_weekly_rest_labels(days)
        if len(cleaned) > MAX_HOLIDAYS_PER_YEAR:
            raise ValueError(
                f"{code} {year_text} has {len(cleaned)} holiday dates"
            )
        years[year_text] = cleaned

    return country_name, years


def generate() -> dict:
    import holidays

    version = _library_version()
    if version != EXPECTED_HOLIDAYS_VERSION:
        raise RuntimeError(
            f"Expected holidays=={EXPECTED_HOLIDAYS_VERSION}, found {version}. "
            "Use the pinned version so the bundled data is reproducible."
        )

    country_names = _load_country_display_names()

    supported = holidays.list_supported_countries(include_aliases=False)
    supported_codes = sorted(
        code for code in supported
        if isinstance(code, str) and _COUNTRY_CODE_RE.fullmatch(code)
    )
    if not supported_codes:
        raise RuntimeError("python-holidays reported no supported country codes")

    holiday_table: dict[str, dict[str, dict[str, list[str]]]] = {}
    included: list[list[str]] = []
    omitted: dict[str, str] = {}
    used_names: set[str] = set()

    for code in supported_codes:
        try:
            country_name, years = _generate_country(code, country_names)
        except Exception as exc:
            omitted[code] = f"{type(exc).__name__}: {exc}"
            continue
        if country_name in used_names:
            country_name = f"{country_name} ({code})"
        used_names.add(country_name)
        holiday_table[code] = years
        included.append([code, country_name])

    missing_required = sorted(REQUIRED_CODES.difference(holiday_table))
    if missing_required:
        reasons = ", ".join(
            f"{code} ({omitted.get(code, 'not reported as supported')})"
            for code in missing_required
        )
        raise RuntimeError(f"Required holiday calendars were not generated: {reasons}")

    if not included:
        raise RuntimeError("No country holiday calendars were generated")

    included.sort(key=lambda item: (item[1].casefold(), item[0]))
    result = {
        "METADATA": {
            "schema_version": 3,
            "coverage": {
                "years": [START_YEAR, END_YEAR],
                "countries": len(included),
                "supported_country_codes": len(supported_codes),
                "omitted": omitted,
            },
            "scope": (
                "Nationwide PUBLIC-category holidays generated without a "
                "subdivision. Official observed/substitute dates are included. "
                "Regional and optional categories are excluded."
            ),
            "names_language": "English",
            "country_names": {
                "source": COUNTRY_NAMES_PATH.name,
                "display": "full names",
            },
            "runtime": (
                "Fully offline. python-holidays is used only by the build-time "
                "generator; the application reads this non-executable JSON Lines "
                "file, one country's line at a time."
            ),
            "provenance": {
                "project": "python-holidays / Open World Holidays Framework",
                "package": "holidays",
                "version": version,
                "source": "https://github.com/vacanza/holidays",
                "generated_utc": dt.datetime.now(dt.timezone.utc)
                    .replace(microsecond=0).isoformat(),
                "rules": [
                    "Every canonical two-letter country/territory code reported by python-holidays",
                    "No subdivision/province/state parameter",
                    "PUBLIC category only",
                    "Observed/substitute dates enabled",
                    "Years 2000-2050 inclusive",
                    "Labels recurring at least 40 times in one year are removed as weekly rest-day labels",
                ],
            },
            "licence": {
                "name": "MIT License",
                "copyright": "python-holidays contributors",
                "url": "https://github.com/vacanza/holidays/blob/main/LICENSE",
            },
        },
        "COUNTRIES": included,
        "HOLIDAYS": holiday_table,
    }
    return result


def _validate_document(document: dict) -> None:
    countries = document["COUNTRIES"]
    holidays_data = document["HOLIDAYS"]
    codes = [item[0] for item in countries]
    names = [item[1] for item in countries]
    if len(codes) != len(set(codes)):
        raise ValueError("Duplicate country code")
    if len(names) != len(set(names)):
        raise ValueError("Duplicate country display name")
    for code, name in countries:
        if not isinstance(name, str) or name.strip().casefold() == code.casefold():
            raise ValueError(f"Full country name is missing for {code}")
    if set(codes) != set(holidays_data):
        raise ValueError("COUNTRIES and HOLIDAYS do not match")
    if not REQUIRED_CODES.issubset(codes):
        raise ValueError("Required country calendars are missing")

    expected_years = {str(year) for year in YEARS}
    for code in codes:
        years = holidays_data[code]
        if set(years) != expected_years:
            raise ValueError(f"{code} does not contain every required year")
        for year_text, days in years.items():
            year = int(year_text)
            if len(days) > MAX_HOLIDAYS_PER_YEAR:
                raise ValueError(f"{code} {year} exceeds the date limit")
            for date_text, names_for_day in days.items():
                day = dt.date.fromisoformat(date_text)
                if day.year != year:
                    raise ValueError(f"{date_text} is stored under {year}")
                if (not isinstance(names_for_day, list)
                        or not 1 <= len(names_for_day) <= MAX_NAMES_PER_DATE):
                    raise ValueError(f"Invalid names for {code} {date_text}")


def _json_line(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":"))
            + "\n").encode("utf-8")


def encode_holiday_file(document: dict) -> bytes:
    """The holiday_data.jsonl bytes of a generated document.

    Line 1 is the index: METADATA, and COUNTRIES as [code, name, length of
    that country's line in bytes]. Then one {"code", "years"} line per
    country, in COUNTRIES order, so the app finds a country by adding up the
    lengths before it and reads just that line. JSON escapes newlines inside
    strings, so every table is exactly one line."""
    tables = [_json_line({"code": code, "years": document["HOLIDAYS"][code]})
              for code, _name in document["COUNTRIES"]]
    index = {
        "METADATA": document["METADATA"],
        "COUNTRIES": [[code, name, len(table)]
                      for (code, name), table in zip(document["COUNTRIES"], tables)],
    }
    return _json_line(index) + b"".join(tables)


def main() -> int:
    try:
        document = generate()
        _validate_document(document)
        payload = encode_holiday_file(document)
        if len(payload) > MAX_OUTPUT_BYTES:
            raise ValueError(
                f"Generated data is {len(payload)} bytes; limit is {MAX_OUTPUT_BYTES}"
            )

        fd, temp_name = tempfile.mkstemp(
            prefix="holiday_data.", suffix=".tmp", dir=OUTPUT_PATH.parent
        )
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(payload)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temp_name, OUTPUT_PATH)
        except Exception:
            try:
                os.unlink(temp_name)
            except OSError:
                pass
            raise

        omitted = document["METADATA"]["coverage"]["omitted"]
        print(
            f"Generated {OUTPUT_PATH.name}: "
            f"{len(document['COUNTRIES'])} countries/territories, "
            f"{START_YEAR}-{END_YEAR}."
        )
        if omitted:
            print("Omitted calendars that failed generation or validation:")
            for code, reason in sorted(omitted.items()):
                print(f"  {code}: {reason}")
        return 0
    except Exception as exc:
        print(f"ERROR: holiday generation failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
