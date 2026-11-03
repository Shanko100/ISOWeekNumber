# WeekNumber Workplace Edition — administration

WeekNumber is a tray app written in Rust for Windows (x64 and ARM64), macOS
(Apple silicon) and Linux (x64 and ARM64). It runs as the signed-in user,
requests no elevation, starts no other processes and does not use the
network.

## Windows

Preferably place `WeekNumber-1.0-windows-x64.exe` (or
`WeekNumber-1.0-windows-arm64.exe`), renamed to `WeekNumber.exe` if you like, in
`C:\Program Files\WeekNumber`. Without administrator rights,
`%LOCALAPPDATA%\Programs\WeekNumber` can be used.

| Data | Location |
|---|---|
| Settings | `%LOCALAPPDATA%\WeekNumber\settings.json` |
| Run at startup | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`WeekNumber`) |

Code-sign the exe with the organisation's certificate and distribute it
from a folder that only administrators can write to.

## macOS

Deploy `WeekNumber.app` from `WeekNumber-1.0-macos-arm64.dmg` to `/Applications`
(MDM tools can install it from the disk image). It is a menu bar app
(`LSUIElement`) with no Dock icon, for macOS 13 and later on Apple silicon,
signed with the hardened runtime; sign and notarize it with the
organisation's Developer ID through the repository secrets described in
`build-macos.yml`.

| Data | Location |
|---|---|
| Settings | `~/Library/Application Support/WeekNumber/settings.json` |
| Run at startup | Login item registered with SMAppService (bundle `io.github.shanko100.weeknumber`) |

Login items can be managed centrally with the
`com.apple.servicemanagement` configuration profile payload.

## Linux

The .deb and .rpm packages install the same static executable as
`/usr/bin/weeknumber`, with the menu entry, icons, AppStream description and
legal notices, and have no dependencies or install scripts; deploy them
with the distribution's package tools (`apt install ./WeekNumber-1.0-linux-x64.deb`,
`dnf install ./WeekNumber-1.0-linux-x64.rpm`). The Flatpak bundle
(`flatpak install WeekNumber-1.0-linux-x64.flatpak`) runs on the
Freedesktop runtime from Flathub, in a sandbox limited to the X11 display
and the tray watcher on the session bus; it keeps its settings in
`~/.var/app/io.github.shanko100.weeknumber/config/weeknumber` and starts at
login through the desktop's Background portal. Packaged copies are never
offered the per-user install.

The AppImage is the static executable itself with a small payload (menu
entry and icons) appended; it needs no FUSE, no libraries and no runtime,
and runs on any distribution with an X11 session or XWayland. For all
users, install it where only root can change it:

```sh
sudo install -Dm755 WeekNumber-1.0-linux-x64.AppImage /usr/local/bin/weeknumber
```

A user who double-clicks the AppImage in Downloads is offered a per-user
install (`~/.local/bin/weeknumber`, a menu entry and icons under
`~/.local/share`); copies in `/usr/local/bin` or managed by an AppImage
tool (Gear Lever, AppImageLauncher) are never prompted. The same from a
script: `weeknumber --install` and `weeknumber --uninstall`. Its icon appears through the StatusNotifierItem D-Bus interface
(KDE Plasma, GNOME with the AppIndicator extension, Cinnamon, Budgie, Xfce,
LXQt, wlroots panels) or, failing that, the XEmbed system tray.

| Data | Location |
|---|---|
| Settings | `~/.config/weeknumber/settings.json` (`$XDG_CONFIG_HOME`) |
| Per-user install | `~/.local/bin/weeknumber`, `~/.local/share/applications/io.github.shanko100.weeknumber.desktop`, `~/.local/share/icons/hicolor/*/apps/` |
| Run at startup | `~/.config/autostart/weeknumber.desktop` |
| Single-instance lock | `$XDG_RUNTIME_DIR/weeknumber/` (0700) |

## All platforms

Run at startup is off on first launch. The app refuses to enable it while
it runs from Downloads, the Desktop, temporary or cloud-synced folders, disk
images, network shares, removable drives, or (Linux) any folder other users
can write to.

The build workflows in `.github/workflows/` build every release with the
pinned Rust toolchain and `Cargo.lock`, check the binary's hardening and
publish a SHA-256 file next to it. The test workflows (`test-windows.yml`,
`test-macos.yml`, `test-linux.yml`) run on every push and pull request
and exercise the release build on a real desktop.

## Properties

- One executable and one process, with no child processes.
- No services, scheduled tasks, scripts or browser launches.
- No network code, telemetry or accounts.
- Bundled data is read-only, bounded and validated.
- Removal: turn off **Run at Startup**, delete the program and, if desired,
  its settings folder.
