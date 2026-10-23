# Data and Asset Provenance

Recorded on 2026-10-17; icon entry updated on 2026-10-21 (twice); screenshot
entry added on 2026-10-22.

## `src/weeknumber/holiday_data.jsonl`

- Generator: `src/weeknumber/generate_holiday_data.py`
- Generator dependency: python-holidays 0.106
- Format: JSON Lines; an index line (metadata and each country's line
  length), then one line per country
- Coverage: 2000 through 2050
- Scope: nationwide public holidays without subdivisions, subject to the
  generator's validation and filtering rules
- Generated timestamp embedded in file:
  `2026-10-17T16:50:56+00:00`
- License notice: `licenses/python-holidays-MIT.txt`

The generated dataset records no omitted target countries.

## `src/weeknumber/country_names.json`

Maintainer declaration supplied for this release:

- Source basis: python-holidays country entities/naming information
- Source version: python-holidays 0.106
- Transformation: normalized into an English display-name JSON table for the
  WeekNumber user interface
- License notice: `licenses/python-holidays-MIT.txt`

This repository does not contain a separate reproducible generator for this
file. The declaration above therefore records maintainer-supplied provenance;
it was not independently reconstructed from the final JSON.

## `src/weeknumber/timezone_data.json`

Maintainer-requested source baselines:

1. Natural Earth Vector 5.1.2
   - `ne_10m_time_zones`
   - `ne_50m_land`
   - `ne_50m_admin_0_boundary_lines_land`
   - Public domain

2. Unicode CLDR 48.2
   - `windowsZones.xml`
   - Unicode License v3

3. IANA Time Zone Database 2026e
   - zone table data used for identifiers and city coordinates
   - Public domain

The repository does not include the generator or source archives for
`src/weeknumber/timezone_data.json`. Consequently, these versions are recorded
as the maintainer-declared provenance baseline and cannot be independently
verified from the compiled JSON alone. Do not replace this statement with a
claim of reproducibility unless the generator and source archives are added.

## `src/weeknumber/tzd_data.json`

- Generator: `src/weeknumber/generate_tzd_data.py` (build-time; needs shapely)
- Sources: Natural Earth Vector 5.1.2, `ne_50m_admin_0_countries` and
  `ne_50m_admin_1_states_provinces` (GeoJSON from the `v5.1.2` tag of
  `nvkelso/natural-earth-vector`), public domain
- Transformation: countries (and, for the United States, Canada, Russia,
  China and Australia, first-level subdivisions) assigned to the 42 classic
  game territories, dissolved, simplified to 0.08 degrees and clipped to the
  time-zone map's latitude window; adjacency is the classic board's 83 links
- Reproducible: download the two files into one directory and run
  `python src/weeknumber/generate_tzd_data.py <that directory>`
- License notice: `licenses/Natural-Earth-Public-Domain.txt`

## `src/weeknumber/app.ico` and `src/weeknumber/app.icns`

- Generator: `src/weeknumber/generate_icons.py` (build-time; Pillow only)
- Source: none. The artwork is drawn from code -- a violet plate holding a
  white calendar page with the app's highlighted week band -- so no image,
  font or third-party asset goes into it.
- Output: `app.icns` on Apple's icon grid, 16-1024 px at 1x and 2x; it
  matches what the generator draws.
- License: the project's MIT license, like the code.
- Reproducible: run the generator; it overwrites both files.

`app.ico` as shipped (recorded 2026-10-21): its artwork is **not** the
generator's drawing -- a simpler page with three bars on a rounded violet
square -- so running the generator would replace it with a different icon.

- Origin: made by the maintainer with Claude, Anthropic's AI assistant
  (stated by the maintainer on 2026-10-21). No third-party image, logo or
  font was used as input.
- Rights: Anthropic's Consumer Terms (effective 2025-10-08, section 4)
  assign to the user "all of our right, title, and interest--if any--in
  Outputs", so nothing restricts WeekNumber's use of it. Purely AI-generated
  imagery may not be protected by copyright in some jurisdictions (the US
  Copyright Office's current position), so others may be free to reuse it.
  It is distributed under the project's MIT license to the extent any
  rights exist.
- Frames: the 16, 24, 32, 48, 64, 128 and 256 px frames are kept byte for
  byte; the 20, 28, 40, 56, 72, 80 and 96 px frames, which Windows asks for at
  125-300 % scaling, were added with `python src/weeknumber/generate_icons.py
  --add-frames`, which resamples the 256 px frame and leaves the others alone.

## `docs/screenshots/color_picker_*.png`

- Source: WeekNumber's own colour picker, drawn by the 1.0 source code
  (`weeknumber_color_dialog.py`) and captured on 2026-10-22.
- Environment: not a Windows or macOS desktop. The window was rendered on a
  Linux test harness (Xvfb, Tk 8.6.14) with the Windows backend stubbed, at
  150 % scaling. Text uses Inter instead of Segoe UI, and there is no title
  bar.
- Post-processing: rounded corners and a 2 px border in the theme's
  `CARD_BORDER` colour; nothing else was edited.
- License: the project's MIT license, like the code.
- Replace them with captures from a Windows build when one is available.
