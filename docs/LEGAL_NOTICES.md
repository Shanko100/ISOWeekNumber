# Legal notices

WeekNumber, Copyright (c) 2026 Shanko100. These notices apply to every copy
of WeekNumber, wherever it is used. In the app they open from
About -> Legal notices, followed by the full license texts.

## License and warranty

WeekNumber's own source code is licensed under the MIT License; its full
text follows these notices. WeekNumber is provided "as is", without
warranty of any kind. To the extent the law allows, the authors are not
liable for any claim, damage or other liability arising from its use.
Nothing in these notices limits rights that the consumer law of your
country gives you and that cannot be waived.

## Third-party software and data

WeekNumber includes software and data made by others, such as Python,
Tcl/Tk, Pillow, PyNaCl and libsodium, holiday data from the holidays
library, time-zone data from Unicode CLDR and the IANA tz database, and map
outlines from Natural Earth. Each keeps its own license. They are listed
under "Third-party notices", and their complete license texts follow.

## Trademarks

WeekNumber is an independent project and is not affiliated with, endorsed
by or sponsored by any company named here. Windows is a trademark of
Microsoft Corporation. macOS is a trademark of Apple Inc. Cloudflare is a
trademark of Cloudflare, Inc.

Any game included in WeekNumber is an independent work. It is not
affiliated with, endorsed by or sponsored by Hasbro, Inc. RISK is a
trademark of Hasbro, Inc.

All other names are used only to identify the product they belong to.

## Privacy

WeekNumber has no account, no advertising, no analytics and no telemetry.
Its settings and a small log file stay on your computer, in
%APPDATA%\WeekNumberPy on Windows and in
~/Library/Application Support/WeekNumber on macOS. Delete that folder to
remove them.

WeekNumber does not use the network. The one exception is an optional
multiplayer feature, which asks for your permission every time it opens;
until you allow it, the app blocks every network connection, and the block
returns as soon as the feature closes.

When you allow it:

- Local network play exchanges player names, game moves and chat only with
  the computers taking part, directly over your network.
- Online play goes through a relay run for this project on Cloudflare
  Workers. Your IP address is never stored as is: a salted hash is kept for
  abuse limits, for at most 15 minutes after the limit applies or until the
  lobby closes. Lobby and host names are shown in a public lobby list until
  the lobby closes (at most about 12 hours). Player names and game moves are
  passed on and not stored. Cloudflare keeps request logs, which may include
  your IP address, for up to 3 days (see Cloudflare's privacy policy).
- Chat is end-to-end encrypted between each player and the host. The relay
  sees only that a sealed message was sent, when, and roughly how long it
  is. Chat is never written to disk.

Do not put personal information in player or lobby names: other players and
the relay can read them.

The relay is operated by the WeekNumber maintainer, Shanko100. Where data
protection law such as the GDPR applies, the data above is processed to
provide the online game you asked to join and, for the hashed address, in
the legitimate interest of keeping the relay free of abuse. You can ask
about your data, or ask for it to be deleted, by opening an issue at
https://github.com/Shanko100/ISOWeekNumber; because addresses are only kept
as short-lived hashes and names are gone within about 12 hours, there is
usually nothing left to look up. You may also complain to your data
protection authority.

Organisations can switch the multiplayer feature off, or point it at their
own relay, by policy (docs/ADMINISTRATION.md in the repository).

## Encryption

WeekNumber contains encryption software: TLS for the connection to the
relay, and X25519, HKDF-SHA256 and ChaCha20-Poly1305 (through the
open-source PyNaCl and libsodium libraries) for chat. It uses only
publicly available, standard algorithms. Some countries regulate the
import, use or export of encryption software; you are responsible for
following the rules where you use or pass on WeekNumber.

## App icon and artwork

The app icon was made by the maintainer with Claude, Anthropic's AI
assistant; Anthropic's terms assign any rights in that output to the user.
All other graphics, the game's sounds and the card symbols are drawn from
code when the app runs. No third-party images, fonts or sounds are bundled.
