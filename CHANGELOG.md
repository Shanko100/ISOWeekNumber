# Changelog

## [1.0 for every platform, refined] - 2026-10-25

### Added

- 🐧 Linux AppImages (`WeekNumber-1.0-linux-x64.AppImage`, `-arm64`) that
  need no FUSE: the executable is its own AppImage runtime. Started from
  Downloads, WeekNumber offers to install itself for the user (program,
  menu entry and sharp icons in the home folder); `--install` and
  `--uninstall` do the same from a script.
- 🧪 Test workflows for every platform (`test-windows.yml`,
  `test-macos.yml`, `test-linux.yml`) that run on each push to main and
  each pull request: lint, unit tests and UI tests of the release build on a real
  desktop, with screenshots of every window.
- 📸 Screenshots of every edition in the README (`screenshots.yml`).

### Changed

- Every release file is named `WeekNumber-<version>-<os>-<arch>.<ext>`.
- 🪟 Windows keeps its settings in `%LOCALAPPDATA%\WeekNumber` and Run at
  Startup under the value `WeekNumber`; the Python version's
  `%APPDATA%\WeekNumberPy` and `WeekNumberPy` value are no longer read.
- 🐧 The Linux calendar has rounded corners and a soft shadow like the
  Windows 11 flyout, and opens at the tray (also under Wayland).
- 🐧 The week number in the Linux panel is set at the size of panel text,
  as tall as the clock's digits, at every icon size and scaling, instead
  of filling the icon as on Windows, and in regular weight by default
  like the panel's own text (Bold Text remains an option).
- 🍎 The macOS calendar's glass has the Python version's rounder,
  concentric corners, and its shadow follows them on macOS 13 to 15.

## [1.0 for every platform] - 2026-10-25

### Added

- 🍎 macOS edition for Apple silicon: a menu bar app with the native menu,
  the calendar on Liquid Glass (the popover material before macOS 26), a
  login item through SMAppService, and a signed disk image
  (`build-macos.yml`).
- 🪟 Windows ARM64 edition, built natively (`build-windows-arm64.yml`).
- 🐧 Linux editions for x64 and ARM64: one static executable that runs on
  any distribution, with the tray icon over D-Bus (StatusNotifierItem and
  D-Bus menus) or the XEmbed tray, and XDG autostart
  (`build-linux.yaml`).
- Smoke tests and binary hardening checks in every build workflow.
- A not-for-profit statement in the legal notices; each edition shows only
  the notices and license texts of what it contains.

### Changed

- The code is split into a shared app and a thin platform layer
  (`src/os`), so the calendar, menus and dialogs are the same everywhere.
- The calendar's copy menu uses the app's own menu style (native on macOS).
- `build-rust.yml` is now `build-windows-x64.yml`.

## [Unreleased]

### Changed

- All documentation is in English.
- The tray icon's right-click menu is more compact: shorter rows, tighter
  padding and a narrower minimum width.
- The calendar's Today and time-zone buttons have more padding, and every
  pill button is now measured with the font it is drawn in, so the label no
  longer crowds the capsule at 125 % and 200 % display scaling.

### Removed

- The `verify` build feature and the ignored Pillow comparison tests, which
  depended on Python tooling that is no longer part of this repository.

## [Rust-only repository] - 2026-10-25

### Changed

- The Rust app is now a standard Cargo project at the repository root.
- Required application data lives under `assets/`.
- Legal notices: font-rs (Apache-2.0, source of the rasterisation routine in
  `src/gfx.rs`) added with its full license text; the copyright lines of
  `holidays` copied verbatim from the upstream LICENSE.
- The documentation and the GitHub Actions build describe only the Rust
  version.

### Removed

- The Python app, Python build files, dependencies and verification scripts.
- The former Python workflow `.github/workflows/build.yml`.
