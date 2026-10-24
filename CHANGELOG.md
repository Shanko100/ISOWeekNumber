# Changelog

All notable changes to WeekNumber are documented in this file.

## [Unreleased]

### Smaller downloads

- `WeekNumber.exe` (x64) 23.1 → 16.9 MB, `WeekNumber-arm64.exe` 17.9 →
  15.8 MB, `WeekNumber.dmg` 20.8 → 14.2 MB, with every feature kept.
- The builds leave out Pillow's image-file readers and their codecs (AVIF,
  WebP, colour management): the app draws its images and opens only
  `app.ico`, which keeps its ICO, PNG and BMP readers.
- Python's zstd module is left out (nothing uses zstd archives), and on
  Windows the C runtime copy PyInstaller packs for systems older than
  Windows 10, which always use their own.
- The disk image is LZMA-compressed (opens on macOS 10.15 and later).

### Memory

- About 1.8 MB less at idle (a frozen build of the tray's start-up
  imports, 26.4 → 24.6 MB): the frozen app no longer imports `inspect`,
  `zipfile` and `ast` at every launch for a PyInstaller fix-up it never
  needs, and `tempfile` (with `shutil`, `bz2`, `lzma`, `random`) loads only
  when a file is written. On Windows the startup-entry location check runs
  only when there is an entry to repair.

### Security

- A host or player sending a list or an object where the game expects a
  territory, player or card is refused instead of ending that connection's
  thread (a host could leave a joiner's window showing "connected").
- A LAN lobby code or resume token with non-ASCII characters is refused and
  counted like any wrong code; before, the handshake thread ended unanswered.
- `WEEKNUMBER_TZD_RELAY` takes the same plain `https://` addresses as the
  `RelayURL` policy; plain `http://` reaches only a relay on this machine.
- No redirects from the relay are followed, so the host's token can't be
  sent to another address; lobby ids and tokens must be ASCII.
- Relay: a lobby's status reaches the public list at most once a second, one
  network may hold 50 of the 2000 lobby-list sockets, IPv6 callers are
  limited per /64, lobby-creation bodies are capped while they are read, and
  ids and tokens are uniformly random. Redeploy `relay/` to apply.

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
