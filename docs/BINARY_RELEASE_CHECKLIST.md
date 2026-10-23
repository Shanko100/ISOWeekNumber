# Binary Release Checklist

Complete this checklist for every published executable. A source-repository
license review is not enough because PyInstaller bundles the actual interpreter,
libraries, and native components present in the build environment.

## Reproducible environment

- [ ] Build on Windows from a clean virtual environment.
- [ ] Install `requirements-release.txt`.
- [ ] Record `python --version`.
- [ ] Record `python -m pip freeze --all`.
- [ ] `build_exe.bat --venv` creates such an environment from
      `requirements-release.txt`; delete an old `.venv-build` first.

## Required release contents

Both build scripts run `src/weeknumber/collect_licenses.py`, which writes
`Licenses.txt` from the build's own environment and bundles it; **About →
Legal notices** shows it in a scrollable window. Check that it holds:

- [ ] `docs/LEGAL_NOTICES.md` first, then `LICENSE` and
      `docs/THIRD_PARTY_NOTICES.md`
- [ ] every text in `licenses/` that applies to the platform (pystray's
      LGPL-3.0 and GPL-3.0 on Windows only)
- [ ] the exact CPython `LICENSE.txt` from the interpreter used to build
- [ ] the Tcl and Tk `license.terms` files from that interpreter/distribution
- [ ] Pillow's installed license file, with the notices for the native
      libraries in its wheel
- [ ] the build's Python and package versions

Attach to the release:

- [ ] `WeekNumber.exe`
- [ ] `WeekNumber.dmg`
- [ ] `pystray-0.19.5.tar.gz`, the exact pystray 0.19.5 source

## LGPL handling for pystray

PyInstaller one-file packaging makes LGPL compliance more involved. The release
must provide a practical way for recipients to replace or modify pystray and
rebuild/recombine the application. At minimum, publish:

- [ ] the complete WeekNumber source corresponding to the executable
- [ ] the exact build instructions
- [ ] the exact dependency versions
- [ ] the exact pystray 0.19.5 source
- [ ] all scripts/specification material needed to rebuild the executable

If counsel or your distribution policy does not accept this arrangement,
replace pystray in a future code version before distributing binaries.

## macOS application bundle

- [ ] Build on macOS from a clean virtual environment with
      `requirements-release.txt`, then run `src/weeknumber/build_app.sh`.
- [ ] The script also writes `dist/WeekNumber.dmg`. Open it and check that
      it shows `WeekNumber.app`, the Applications shortcut and
      `Licenses.txt`, and that the app runs after dragging it over.
- [ ] Replace the ad-hoc signature with a Developer ID signature using the
      hardened runtime, notarize with `notarytool`, and staple the ticket.
- [ ] On a clean account, verify the menu-bar item, calendar, dialogs, Run
      at Startup (LaunchAgent), and that macOS asks for Local Network access
      the first time Timezone Dominator searches for lobbies.
- [ ] On macOS 26, confirm the calendar flyout shows Liquid Glass; on macOS
      15 or earlier, that it falls back to vibrancy.

## Online relay

- [ ] Deploy `relay/` (`npx wrangler deploy`) and set `RELAY_URL` in
      `src/weeknumber/weeknumber_tzd_relay.py` to its address before building.
- [ ] `Licenses.txt` includes websocket-client's and truststore's licenses.
- [ ] From two different networks, create a public lobby on one machine and
      join it from the other; play a few turns; confirm the lobby leaves the
      list when the host closes it.

## Timezone Dominator network check

- [ ] Confirm the menu has no Timezone Dominator entry and nothing visible in the Time
      Zone window leads to it. Open it the hidden way (see `_egg_click` in
      `weeknumber_timezone_dialog.py`): the map turns into the board with
      the lobby on top, the previously chosen time zone is kept, clicking the
      board brings the lobby forward, and closing the lobby turns the board
      back into the map. With a game running, it raises that game instead.
- [ ] Every time Timezone Dominator opens, the "Timezone Dominator wants to connect" notice comes
      first; nothing connects until "Allow and play", and "Don’t connect"
      turns the board back into the map.
- [ ] Chat: clicking Encrypted shows both players the same safety code (the
      host's) and Hide code goes back; messages arrive both ways with the
      right names; muting a player hides theirs.
- [ ] The turn banner and its phase steps, the turn announcement over the
      board, the board frame in the current player's colour, the move
      caption along the board's foot, the continent outlines and legend
      bars, coloured log, required-trade pulse, cards, Place all / Move all,
      effects and sounds work; the Sound switch mutes them and survives a
      restart.
- [ ] Host a lobby on Windows and join from macOS, and the reverse, on the
      same network. Windows Firewall prompts on first hosting; allow private
      networks only.
- [ ] Confirm nothing listens on TCP 47842 / UDP 47843 once the lobby or
      game window is closed.

## Final checks

- [ ] Run `python -m pytest`.
- [ ] Run `src\\weeknumber\\build_exe.bat --nopause` from `src\\weeknumber`.
- [ ] Start the resulting executable on a clean Windows test account.
- [ ] Verify tray icon, calendar, settings, holiday data, timezone dialog,
      startup toggle, About dialog, and clean exit.
- [ ] Run at Startup: run the exe from Downloads and confirm the toggle is
      refused with a message naming Program Files; move it to
      `C:\Program Files\WeekNumber`, and once to
      `%LOCALAPPDATA%\Programs\WeekNumber`, and confirm it registers
      `WeekNumberPy` under HKCU\...\Run and starts at the next sign-in.
      Disable it in Task Manager and confirm the menu shows it unchecked.
- [ ] Policies (docs/ADMINISTRATION.md): with `DisableOnline`, `DisableLAN`,
      `DisableChat` and `DisableGame` set in turn under
      `HKCU\Software\Policies\WeekNumber`, restart and confirm each one
      takes effect.
- [ ] The icon shows in the title bars, Alt+Tab and Explorer at every size,
      and is sharp on the taskbar at 100 %, 125 %, 150 % and 200 % scaling
      (open About on a monitor at each scaling).
- [ ] Memory: with Task Manager's details view showing "Memory (active
      private working set)", the idle app drops back to its startup figure
      a few seconds after the calendar, a dialog or a Timezone Dominator game closes.
- [ ] Change Color… opens WeekNumber's own picker while another WeekNumber
      window is open, and Apply recolours the tray number.
- [ ] About → Legal notices opens the Legal notices window beside About; both stay
      open, and the plain text scrolls with the wheel, the bar and the
      keyboard.
- [ ] `docs/PROVENANCE.md` names the AI image tool the app.ico artwork was
      made with, and its terms allow this use.
- [ ] Network gate: with a firewall log or Resource Monitor open, start the
      app, open the calendar and every dialog: no connection, no DNS lookup.
      Open Timezone Dominator and choose "Don’t connect": still none. Open it again and
      allow: the lobby connects. Close every Timezone Dominator window: connections stop.
- [ ] Timezone Dominator cards: clicking a card does nothing; with a set in
      hand on your Reinforce step, the set is shown lifted and "Trade a
      set" trades it.
- [ ] Neither README.md nor the release notes name or describe the game;
      only the in-app legal notices mention any game, without its name.
- [ ] Change Color…: the top row holds the dark shades, the middle the plain
      colours, the bottom the light tints; each column is one colour.
- [ ] Timezone Dominator give up: on your turn and on someone else's, "Give up…" asks
      first; "Keep playing" changes nothing, "Acknowledge defeat" passes the
      turn and you keep watching.
- [ ] Timezone Dominator reconnect, LAN and online: switch Wi-Fi off for ~10 s mid-game
      (or sleep the laptop briefly). The window shows "Reconnecting…", the
      others see "reconnecting…", and the game continues with your seat.
      Off for more than two minutes: you leave the game, and Reconnect says
      the seat is gone.
- [ ] Windows on ARM: the x64 exe runs on a Windows 11 ARM device with chat;
      if you ship a native ARM64 exe, build it with ARM64 Python and check
      that the lobby says chat is unavailable and play works.
- [ ] Check that every button, including About's and the calendar chips, is
      a pill, and that dark mode gives every dialog a dark title bar.
- [ ] Scan the final archive and executable with your normal release-security
      tooling.
- [ ] Verify that the published repository URL matches the URL embedded in the
      application.
