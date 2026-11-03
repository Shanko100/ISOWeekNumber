# Third-Party Notices

<!-- Blocks marked "only:" are kept in the builds for the platforms they
name and left out of the others (build.rs). -->

The MIT License in the repository root applies to original WeekNumber
material owned by Shanko100. It does not relicense third-party software or
data. The license texts named below follow these notices in the app, and
are kept in `legal/licenses/` in the repository.

## Rust standard library

- Version: Rust 1.99.1 (pinned in `rust-toolchain.toml`)
<!-- only: windows -->
- Use: compiled into the program (core, alloc and std, with the crates.io
  crates cfg-if, hashbrown and rustc-demangle they include on Windows)
- License: MIT OR Apache-2.0, used under MIT; the Unicode tables in core
  under the Unicode License v3
- Notice: `licenses/Rust-standard-library.txt`
<!-- end -->
<!-- only: macos, linux -->
- Use: compiled into the program (core, alloc and std, with the crates.io
  crates cfg-if, hashbrown, rustc-demangle, libc, addr2line, gimli, object
  and memchr they include on macOS and Linux; miniz_oxide and adler2 are
  listed below)
- License: MIT OR Apache-2.0 (memchr: MIT OR Unlicense), used under MIT;
  the Unicode tables in core under the Unicode License v3
- Notice: `licenses/Rust-standard-library-unix.txt`
<!-- end -->

<!-- only: windows -->
## windows crates

- Project: windows-rs (Microsoft): windows, windows-core, windows-result,
  windows-strings, windows-link and the other windows-* crates in
  `Cargo.lock`
- Version: windows 0.62.2
- Use: bindings to the Win32 API
- License: MIT OR Apache-2.0, used under MIT
- Notice: `licenses/windows-rs-MIT.txt`

The procedural macros windows-implement and windows-interface, and the
crates they use (proc-macro2, quote, syn, unicode-ident), run only while
compiling; none of their code is in the program.
<!-- end -->

<!-- only: linux -->
## musl C library

- Project: musl libc (Rich Felker and contributors)
- Version: the one the Rust toolchain ships for its musl targets
- Use: the C library, linked statically so that one program runs on every
  Linux distribution; Rust's musl targets also link LLVM's libunwind and
  compiler-rt builtins, which are under Apache-2.0 WITH LLVM-exception (the
  exception waives attribution for such linked object code)
- License: MIT (some files BSD-2-Clause, BSD-3-Clause, ISC or public
  domain, as listed in the notice)
- Notice: `licenses/musl-MIT.txt`
<!-- end -->

<!-- only: macos -->
## macOS system frameworks

The program calls AppKit, Foundation, Core Graphics, Core Text, Core
Foundation, Quartz Core and Service Management, which are part of macOS
and are not bundled with it.
<!-- end -->

## miniz_oxide and adler2

- Versions: miniz_oxide 0.9.1, adler2 2.0.1
- Use: inflating the embedded holiday, map and legal texts (and deflating
  them at build time)
- Licenses: miniz_oxide MIT (also offered under Zlib and Apache-2.0),
  adler2 0BSD (also offered under MIT and Apache-2.0)
- Notices: `licenses/miniz_oxide-MIT.txt`, `licenses/adler2-0BSD.txt`

## Pillow

- Version: 13.0.0
- Use: its drawing and resampling routines (rounded rectangles, ellipses,
  polygons, Lanczos and other filters, alpha compositing) are ported from
  its C core; no Pillow code is bundled as such
- License: MIT-CMU (HPND)
- Notice: `licenses/Pillow-MIT-CMU.txt`

## Tk

- Version: 9.0
- Use: the layout rules of its labels, menus, canvas ovals and the clam
  combobox (tkWinButton.c, tkWinMenu.c, tkWinDraw.c, ttk) are ported; no
  Tk code is bundled as such
- License: Tcl/Tk license
- Notice: `licenses/Tk-license.terms`

The tray icon's text is filled by this edition's own rasterizer, written to
give the same coverage as FreeType's; it contains no FreeType code. Its
edge-accumulation routine is adapted from font-rs (below).

## font-rs

- Project: font-rs (Raph Levien), Copyright 2015 Google Inc.
- Version: 0.1.3
- Use: `accumulate_line` in `src/gfx.rs` is derived from `draw_line` in
  font-rs `src/raster.rs`, modified for this edition (f64 coordinates, a
  padded row stride, clamping to the image width); no font-rs crate is
  linked
- License: Apache-2.0
- Notice: `licenses/font-rs-Apache-2.0.txt`

## holidays

- Project: Open World Holidays Framework (`holidays`)
- Copyright: Vacanza Team and individual contributors; dr-prodigy
  (2017-2023); ryanss (2014-2017)
- Version: 0.106
- Use: the source of the holiday data embedded in the program
- License: MIT
- Notice: `licenses/holidays-MIT.txt`

The holiday data remains subject to applicable rights and notices in its
source material.

## Unicode CLDR

- Project: Unicode Common Locale Data Repository
- Stable version recorded for provenance: 48.2
- Use: the Windows/IANA time-zone mappings embedded in the program
- License: Unicode License v3
- Notice: `licenses/Unicode-License-v3.txt`

Copyright © 1991–2026 Unicode, Inc.

## Natural Earth

- Project: Natural Earth Vector
- Latest tagged release recorded for provenance: 5.1.2
- Use: the time-zone regions, land, coastlines and borders of the map
- Status: public domain
- Notice: `licenses/Natural-Earth-Public-Domain.txt`

Made with Natural Earth.

## IANA Time Zone Database

- Project: IANA Time Zone Database
- Release recorded for provenance: 2026e
- Use: the zone identifiers and city coordinates embedded in the program
- Status: public domain for the database files used here
- Notice: `licenses/IANA-tzdb-Public-Domain.txt`

<!-- only: windows -->
## Microsoft C runtime

The program links the Visual C++ runtime and the Universal C Runtime
statically from the Visual Studio Build Tools on the build machine, as
Microsoft's license terms for that distributable code allow.
<!-- end -->

## Sounds, icons and other artwork

There are no sounds. The calendar, switches, buttons, scroll bar, map and
every other control are drawn from code at run time. No third-party image
or font assets are included:
<!-- only: windows -->
the interface uses the fonts of Windows, which are not bundled.
<!-- end -->
<!-- only: macos -->
the interface uses the system fonts of macOS, which are not bundled.
<!-- end -->
<!-- only: linux -->
the interface uses the fonts installed on the system (Noto Sans, DejaVu
Sans and the like), which are not bundled.
<!-- end -->
The app icon is described under Legal notices.

## Trademarks

See the Legal notices, which the app shows first under About → Legal
notices.

## Build-only tools

cargo-deny checks dependencies during CI. It is not part of the program.
<!-- only: linux -->
nfpm (MIT) builds the .deb and .rpm packages, and Flatpak's own tools
(LGPL-2.1-or-later) the Flatpak bundle, during CI; neither is part of the
program or of the packages.
<!-- end -->
