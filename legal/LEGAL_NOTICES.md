# Legal notices

<!-- Blocks marked "only:" are kept in the builds for the platforms they
name and left out of the others (build.rs). -->

WeekNumber Workplace Edition (Rust), Copyright (c) 2026 Shanko100. These
notices apply to every copy of this edition, wherever it is used. In the
app they open from About -> Legal notices, followed by the full license
texts.

## A not-for-profit project

WeekNumber is a not-for-profit project, built to take a little friction
out of the working week: the week number is always there when a meeting,
a deadline or a planning sheet asks for it. It is free of charge and
always will be. There is no advertising, no subscription, no in-app
purchase and no data collection; nobody earns money from it, and it asks
nothing of you in return. If it spares you one glance at a calendar on a
busy Monday morning, it has done its job.

## License and warranty

WeekNumber's own source code is licensed under the MIT License; its full
text follows these notices. WeekNumber is provided "as is", without
warranty of any kind. To the extent the law allows, the authors are not
liable for any claim, damage or other liability arising from its use.
Nothing in these notices limits rights that the consumer law of your
country gives you and that cannot be waived.

## Third-party software and data

<!-- only: windows -->
This edition includes software and data made by others: the Rust standard
library, Microsoft's windows crates, miniz_oxide and adler2, code ported
from Pillow and Tk, code adapted from font-rs, holiday data from the
holidays library, time-zone data from Unicode CLDR and the IANA tz
database, and map outlines from Natural Earth.
<!-- end -->
<!-- only: macos -->
This edition includes software and data made by others: the Rust standard
library, miniz_oxide and adler2, code ported from Pillow and Tk, code
adapted from font-rs, holiday data from the holidays library, time-zone
data from Unicode CLDR and the IANA tz database, and map outlines from
Natural Earth.
<!-- end -->
<!-- only: linux -->
This edition includes software and data made by others: the Rust standard
library, the musl C library, miniz_oxide and adler2, code ported from
Pillow and Tk, code adapted from font-rs, holiday data from the holidays
library, time-zone data from Unicode CLDR and the IANA tz database, and
map outlines from Natural Earth.
<!-- end -->
Each keeps its own license. They are listed under "Third-party notices",
and their complete license texts follow.

## Trademarks

WeekNumber is an independent project and is not affiliated with, endorsed
by or sponsored by any company named here.
<!-- only: windows -->
Windows is a trademark of Microsoft Corporation.
<!-- end -->
<!-- only: macos -->
Apple, Mac and macOS are trademarks of Apple Inc.,
registered in the U.S. and other countries and regions.
<!-- end -->
<!-- only: linux -->
Linux is the registered trademark of Linus Torvalds in the U.S. and other
countries.
<!-- end -->
Rust is a trademark of the Rust Foundation.

All other names are used only to identify the product they belong to.

## Privacy

WeekNumber has no account, no advertising, no analytics and no telemetry,
and it does not use the network: the program contains no network code.

<!-- only: windows -->
It keeps its settings on your computer, in %LOCALAPPDATA%\WeekNumber. When
you turn on Run at Startup, it also writes the value WeekNumber under
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run, and
Windows' matching Task Manager entry. Turn Run at Startup off and delete
that folder to remove them.
<!-- end -->
<!-- only: macos -->
It keeps its settings on your Mac, in ~/Library/Application
Support/WeekNumber. When you turn on Run at Startup, macOS adds WeekNumber
to System Settings > General > Login Items. Turn Run at Startup off and
delete that folder to remove them.
<!-- end -->
<!-- only: linux -->
It keeps its settings on your computer, in ~/.config/weeknumber. When you
turn on Run at Startup, it also writes ~/.config/autostart/
weeknumber.desktop, which your desktop reads at login. Turn Run at Startup
off and delete that folder to remove them. To show its icon it talks to
your desktop's panel over the local session bus (D-Bus) or the X11 system
tray; nothing leaves your computer.
<!-- end -->

## Encryption

This edition contains no encryption software. It computes SHA-256 hashes
only to name its own single-instance lock.

## App icon and artwork

The app icon was made by the maintainer with Claude, Anthropic's AI
assistant; Anthropic's terms assign any rights in that output to the user.
All other graphics are drawn from code when the app runs. No third-party
images or fonts are bundled:
<!-- only: windows -->
the interface uses the fonts of Windows.
<!-- end -->
<!-- only: macos -->
the interface uses the system fonts of macOS.
<!-- end -->
<!-- only: linux -->
the interface uses the fonts installed on your system.
<!-- end -->
