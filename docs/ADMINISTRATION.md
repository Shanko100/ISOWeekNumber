# WeekNumber for IT administrators

WeekNumber is a per-user tray (Windows) and menu-bar (macOS) app showing the
ISO week number. It runs as the signed-in user and never asks for elevation.

## Install

- **Windows:** deploy `WeekNumber.exe` to `C:\Program Files\WeekNumber`, where
  only administrators can change it. Users without admin rights can run it
  from `%LOCALAPPDATA%\Programs\WeekNumber` instead.
- **macOS:** deploy `WeekNumber.app`, from `WeekNumber.dmg`, to
  `/Applications`. The 1.0 build is ad-hoc signed, not notarized; allow it
  through your management tool or sign it with your own Developer ID.

The app stores only its settings and a rotating log:

| | Windows | macOS |
|---|---|---|
| Settings and log | `%APPDATA%\WeekNumberPy` | `~/Library/Application Support/WeekNumber` |
| Run at Startup | `HKCU\...\CurrentVersion\Run`, value `WeekNumberPy` | `~/Library/LaunchAgents/io.github.shanko100.weeknumber.plist` |

On Windows, Run at Startup is refused while WeekNumber runs from Downloads,
the Desktop, temporary or cloud-synced folders, network shares or removable
drives.

## Network access

The calendar itself never uses the network. The only feature that does is a
hidden multiplayer board game, Timezone Dominator, opened from the Time Zone window. It
connects nothing until a user opens it and agrees to a notice that says what
is sent where. The notice is shown every time Timezone Dominator opens; nothing about the
answer is saved.

The process enforces this itself. From the first line of `main()` a Python
audit hook (`weeknumber_core.NetworkGate`) refuses every Internet socket
operation -- DNS lookups, connects, binds and sends, TCP and UDP -- unless
the user has just allowed Timezone Dominator; it closes again when the Timezone Dominator windows
close. Only local AF_UNIX sockets (the macOS single-instance signal) are
exempt. Declining the notice, or never opening Timezone Dominator, means WeekNumber makes
no network connection at all. The gate covers code running in the app's
Python interpreter; it is not a firewall, so keep using your firewall and
the policies below for enforcement against other software.

| Mode | What it does |
|---|---|
| Online | HTTPS and WebSocket (port 443) to the public relay `tzd-relay.weeknumber.workers.dev` on Cloudflare, or to the relay set by policy. TLS is checked against the operating system's trust store. |
| Local network | Hosting listens on TCP 47842 (or a free port) and answers discovery on UDP 47843; joining sends one UDP broadcast and opens one TCP connection. Windows Firewall and macOS ask the user first. |

Chat in the game is end-to-end encrypted between each player and the
hosting player (X25519, ChaCha20-Poly1305): the relay and the network see
only sealed boxes. Nothing from chat is written to disk.

A player whose connection drops mid-game reconnects automatically within
about 100 seconds; the host keeps the seat for 120 seconds. The player
proves it is them with a box sealed on that encrypted channel (which the
relay cannot forge or replay) or, where the encryption library is missing,
a one-time token the host replaces on every return. Every message is
size-capped (64 KiB) and rate-limited per connection; a connection that
floods is cut off and loses its seat, so a misbehaving peer cannot make the
app's memory grow. What the public
relay keeps is in its [privacy notes](../relay/README.md#privacy).

## Policies

Policies are read when WeekNumber starts; restart it after a change. The
switches only ever turn things off.

| Policy | Type | Effect |
|---|---|---|
| `DisableGame` | DWORD / Boolean | Removes the game entirely: nothing in the Time Zone window leads to it. |
| `DisableOnline` | DWORD / Boolean | No connections to any relay. |
| `DisableLAN` | DWORD / Boolean | No listening sockets, broadcasts or LAN connections. |
| `DisableChat` | DWORD / Boolean | No chat; the encryption library is never loaded. |
| `RelayURL` | String | An `https://` address of your own relay instead of the public one. Deploy `relay/` from this repository to your Cloudflare account to run one. |

### Windows

Values under `HKEY_LOCAL_MACHINE\Software\Policies\WeekNumber` win over the
same values under `HKEY_CURRENT_USER\Software\Policies\WeekNumber`. Switches
are `REG_DWORD` (1 = on), `RelayURL` is `REG_SZ`. Deploy them with Group
Policy Preferences or Intune, for example:

```bat
reg add "HKLM\Software\Policies\WeekNumber" /v DisableOnline /t REG_DWORD /d 1 /f
reg add "HKLM\Software\Policies\WeekNumber" /v RelayURL /t REG_SZ /d "https://relay.example.com" /f
```

### macOS

Use a configuration profile with custom settings for the preference domain
`io.github.shanko100.weeknumber`, for example in Jamf, Intune or Kandji:

```xml
<dict>
  <key>DisableOnline</key>
  <true/>
  <key>RelayURL</key>
  <string>https://relay.example.com</string>
</dict>
```

`RelayURL` only counts when a profile enforces it; the switches count from
any source.

## Reporting a security problem

See the [security policy](../.github/SECURITY.md).
