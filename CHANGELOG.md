# Changelog

All notable changes to WeekNumber are documented in this file.

## [1.0] - 2026-10-23

The first public release.

### Calendar

- The current ISO-8601 week sits in the Windows notification area or the
  macOS menu bar. A click opens a two-month calendar; another click closes
  it.
- Week, date and keyboard navigation; right-click a date or a week number to
  copy it.
- Nationwide public holidays for up to two countries, from bundled offline
  data (python-holidays 0.106, 2000–2050).
- An optional time-zone clock, picked from a world map.
- Light and dark mode, bold digits, and an icon colour chosen in
  WeekNumber's own colour picker (36 standard colours, hex field, dark and
  light previews).
- macOS: the calendar floats on Liquid Glass (a frosted panel before
  macOS 26); the app has no Dock icon.
- Windows: window icons match the monitor's scaling, from 100 % to 300 %.
- About → Legal notices shows the license and warranty, trademarks, privacy
  and encryption notices (`docs/LEGAL_NOTICES.md`), then every bundled
  license.

### Privacy and security

- No network unless you allow it: from the first line of start-up, a Python
  audit hook (`NetworkGate`) refuses every Internet socket operation -- DNS
  lookups, connects, binds, sends. It opens only after you accept the notice
  the one optional network extra shows every time it opens, and closes again
  with it. Nothing about the answer is saved.
- That extra's network code is bounded: messages are capped at 64 KiB,
  connections are rate-limited and cut off when they flood, sends time out,
  failed attempts are counted, code comparison is constant-time and
  local-network discovery is capped.
- Run at Startup is per user. On Windows it uses its own `WeekNumberPy` Run
  value, is refused from Downloads, temporary, synced, network and removable
  locations, and respects Task Manager's "Disabled"; on macOS it is a
  LaunchAgent.
- IT administrators can turn off the optional network features or point them
  at their own relay with a policy (`docs/ADMINISTRATION.md`).
- Settings are validated and written through a temporary file; bundled data
  files are read size-capped and never executed.

### Memory

- Only the tray starts. The calendar, the dialogs and every optional extra
  load when they first open and are unloaded again once no window is open
  (`ModuleSweeper`).
- The calendar reads only the holiday years it shows
  (`holiday_data.jsonl`), and the time-zone map's outlines exist only while
  a map window is open.
- Shortly after a window closes, freed memory goes back to the system
  (malloc zones on macOS, the process heap and working set on Windows).
- macOS: AppKit is reached through a small ctypes bridge instead of PyObjC;
  the app starts at about 40 MB.

### Platforms and build

- Windows 10 and 11 on x64, Windows 11 on ARM (native ARM64 exe, or the x64
  one emulated) and macOS 12+ on Apple silicon, all with every feature.
- Built with Python 3.15. `requirements-release.lock` pins every dependency,
  indirect ones included, with its hash; the `build` workflow builds the x64
  and ARM64 exes and the disk image from it.
- The Windows tray icon talks to the Win32 API directly
  (`weeknumber_platform._wintray`): no third-party tray library, and the icon
  is built in memory, so nothing is written to disk.
- Chat encryption uses PyNaCl and libsodium, which ship for every platform
  the app supports.
- `build_exe.bat` builds for the running Python's architecture and refuses
  32-bit Python. `build_app.sh` builds the app and `WeekNumber.dmg`, and
  warns about Rosetta.
- Both builds bundle `Licenses.txt`, written by `collect_licenses.py` from
  the build's own environment.
- The online relay is a Cloudflare Worker in `relay/`.

### Easter egg

- There is one. Finding it is up to you. 🥚

[1.0]: https://github.com/Shanko100/ISOWeekNumber/releases/tag/v1.0
