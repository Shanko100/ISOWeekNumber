# Third-Party Notices

The MIT License in the repository root applies to original WeekNumber material
owned by Shanko100. It does not relicense third-party software or data.

Versions below were checked on 2026-10-18; this file was last reviewed on 2026-10-21.

## python-holidays

- Project: Open World Holidays Framework (`holidays`)
- Version: 0.106
- Use: build-time generation of `src/weeknumber/holiday_data.jsonl`; source basis for the
  country-code/name table declared by the maintainer
- License: MIT
- Notice: `licenses/python-holidays-MIT.txt`

`src/weeknumber/holiday_data.jsonl` contains generated holiday information and remains subject
to applicable rights and notices in its source material.

## Unicode CLDR

- Project: Unicode Common Locale Data Repository
- Stable version recorded for provenance: 48.2
- Use: Windows/IANA time-zone mappings in `src/weeknumber/timezone_data.json`
- License: Unicode License v3
- Notice: `licenses/Unicode-License-v3.txt`

Copyright © 1991–2026 Unicode, Inc.

## Natural Earth

- Project: Natural Earth Vector
- Latest tagged release recorded for provenance: 5.1.2
- Use: time-zone regions, land geometry, coastlines, and borders represented
  in `src/weeknumber/timezone_data.json`; country and subdivision shapes
  represented in `src/weeknumber/tzd_data.json`
- Status: public domain
- Notice: `licenses/Natural-Earth-Public-Domain.txt`

Suggested attribution: “Made with Natural Earth.”

## IANA Time Zone Database

- Project: IANA Time Zone Database
- Release recorded for provenance: 2026e
- Use: IANA zone identifiers and city coordinates represented in
  `src/weeknumber/timezone_data.json`
- Status: public domain for the database files used here
- Notice: `licenses/IANA-tzdb-Public-Domain.txt`

## pystray

- Version: 0.19.5
- Use: system-tray integration on Windows (not used or bundled on macOS)
- License: GNU Lesser General Public License version 3
- License texts:
  - `licenses/pystray-LGPL-3.0.txt`
  - `licenses/GPL-3.0.txt`

pystray is not relicensed under the WeekNumber MIT License. Binary distributors
must satisfy LGPL requirements, including allowing replacement or relinking of
the LGPL-covered library and providing the corresponding library source or a
valid written/source offer as applicable.

## websocket-client

- Version: 1.9.2
- Use: WebSocket connections to the game relay for online play
- License: Apache License 2.0
- Notice: `licenses/websocket-client-Apache-2.0.txt`

## truststore

- Version: 0.10.5
- Use: TLS certificate verification against the operating system's trust
  store for online play
- License: MIT
- Notice: `licenses/truststore-MIT.txt`

## cryptography

- Version: 50.0.2
- Use: end-to-end encryption of in-game chat (X25519 key agreement, HKDF,
  ChaCha20-Poly1305)
- License: Apache License 2.0 or BSD-3-Clause, at the recipient's choice
- Notices: `licenses/cryptography-Apache-2.0.txt`,
  `licenses/cryptography-BSD-3-Clause.txt`

## OpenSSL

- Version: the one statically linked into the cryptography 50.0.2 wheels
- Use: the primitives behind cryptography
- License: Apache License 2.0
- Notice: `licenses/OpenSSL-Apache-2.0.txt`

## cffi and pycparser

- Versions: cffi 2.1.1, pycparser 3.11
- Use: cryptography's C bindings
- Licenses: cffi MIT-0, pycparser BSD-3-Clause
- Notices: `licenses/cffi-MIT-0.txt`, `licenses/pycparser-BSD-3-Clause.txt`

## Pillow

- Version: 13.0.0
- Use: image and icon rendering
- License: HPND/PIL-style permissive license
- Notice: `licenses/Pillow.txt`; the full texts of the native libraries in
  the installed wheel are copied into each build's `Licenses.txt`

Pillow's wheels bundle native libraries whose licenses ask for a credit in
the documentation of a binary distribution:

- Portions of this software are copyright © The FreeType Project
  (www.freetype.org). All rights reserved.
- This software is based in part on the work of the Independent JPEG Group.

Where a Pillow wheel bundles an LGPL-licensed library as a separate shared
library (FriBiDi on some platforms), that library can be replaced the same
way as pystray: rebuild from the published source and build scripts with a
modified copy installed.

## PyInstaller

- Version: 6.22.3
- Use: build tooling and executable bootloader
- License: GPL-2.0-or-later with a special exception for distributing bundled
  applications
- Notice: `licenses/PyInstaller-Notice.txt`

The exception does not override licenses of libraries bundled into an
executable.

## CPython and Tcl/Tk

The Windows and macOS builds bundle CPython and Tcl/Tk from the Python used
to build them, and Pillow's wheel bundles native libraries (libjpeg, zlib,
FreeType and others). Their exact licenses depend on that installation, so
`src/weeknumber/collect_licenses.py` copies them from the build environment
into each build's `Licenses.txt`, which About → Legal notices opens. Tcl's text is
also kept in `licenses/Tcl-license.terms`, because Python for Windows ships
only Tk's copy.

## Sounds, icons and other artwork

The game's sound effects are synthesised at run time by
`weeknumber_tzd_sound.py`; the card symbols, colour picker, scroll bar and
every other control are drawn from code at run time. No third-party audio,
image or font assets are included: the interface uses the operating
system's own fonts, which are not bundled. For the app icon files see
`docs/PROVENANCE.md`.

## Trademarks

See `docs/LEGAL_NOTICES.md`, which the app shows first under About → Legal
notices.

## Development-only dependencies

The test suite uses pytest 9.1.1 and pytest-cov 7.1.0, and
`generate_tzd_data.py` uses shapely 2.2.0 (BSD-3-Clause). They are development
tools and are not bundled into normal application releases.

The game relay in `relay/` is built with wrangler 4.151.1, TypeScript 7.0.2
and @cloudflare/workers-types; it runs on Cloudflare and is not part of the
application binaries.
