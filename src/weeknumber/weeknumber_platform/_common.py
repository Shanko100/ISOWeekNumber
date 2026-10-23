"""Backend-neutral helpers shared by the Windows and macOS layers.

The run-at-startup *decisions* live here as plain functions over a tiny
registry interface, so they are tested on every OS. The Windows backend only
supplies the winreg-backed store and the Win32 lookups.
"""

import logging
import os
import re
import sys
import tempfile
import urllib.parse
from pathlib import Path
from typing import NamedTuple

log = logging.getLogger("weeknumber")

#: Time-zone keys are Windows key names on every platform, so settings files
#: stay portable. DYNAMIC_TIME_ZONE_INFORMATION.TimeZoneKeyName is WCHAR[128]
#: including the terminator, so 127 characters is the longest key there is.
MAX_KEY_CHARS = 127


def launch_argv() -> "list[Path] | None":
    """Resolved paths that relaunch this app: the frozen executable, or the
    interpreter plus the script in source mode. Registering only the .py file
    would rely on file associations, which are easier to hijack."""
    try:
        if getattr(sys, "frozen", False):
            return [Path(sys.executable).resolve(strict=True)]
        return [Path(sys.executable).resolve(strict=True),
                Path(sys.argv[0]).resolve(strict=True)]
    except OSError:
        return None


def launch_location() -> "Path | None":
    try:
        return Path(sys.executable if getattr(sys, "frozen", False)
                    else sys.argv[0]).resolve(strict=True)
    except OSError:
        return None


def _root_prefix(root: str, sep: str) -> str:
    # A drive root such as "d:\" already ends in the separator; adding a
    # second one would make nothing on that drive match.
    return root if root.endswith(sep) else root + sep


def containing_root(path: Path, roots: "list[tuple[Path, str]]") -> "str | None":
    """The label of the deepest (root, label) directory containing `path`,
    compared case-insensitively on whole path components -- so Downloads
    inside a temp tree is reported as Downloads. Both sides are resolved
    first: Windows may name the same folder by its 8.3 short form
    (C:\\Users\\LONGNA~1\\...), and the roots are resolved to the long form."""
    try:
        target = str(path.resolve()).casefold()
    except OSError:
        target = str(path).casefold()
    sep = os.sep.casefold()
    best: "tuple[int, str] | None" = None
    for candidate, label in roots:
        try:
            root = str(candidate.resolve()).casefold()
        except OSError:
            continue
        if target == root or target.startswith(_root_prefix(root, sep)):
            if best is None or len(root) > best[0]:
                best = (len(root), label)
    return best[1] if best else None


def temp_dir() -> Path:
    return Path(tempfile.gettempdir())


# ---------------------------------------------------------------------------
# Run-at-startup decisions (used by the Windows Run key)
# ---------------------------------------------------------------------------
#: Our own Run value. The settings folder already uses this name; plain
#: "WeekNumber" is shared with other week-number apps.
RUN_VALUE = "WeekNumberPy"
#: Written by earlier WeekNumber builds, migrated only when it points at this app.
LEGACY_RUN_VALUE = "WeekNumber"
#: Windows ignores Run commands of MAX_PATH characters or more.
MAX_RUN_COMMAND = 259
#: StartupApproved data: a 4-byte flag (bit 0 set = disabled in Task Manager)
#: followed by an 8-byte FILETIME.
APPROVED_ENABLED = b"\x02" + b"\x00" * 11


def quote_command(parts: "list[Path]") -> str:
    return " ".join('"' + str(part) + '"' for part in parts)


def command_paths(command: str) -> "list[str]":
    """The paths in a Run command: every double-quoted token, or the first
    bare token of an unquoted command."""
    quoted = re.findall(r'"([^"]*)"', command)
    if quoted:
        return quoted
    return command.split()[:1]


def same_path(a: str, b: str) -> bool:
    return os.path.normcase(os.path.normpath(a)) == os.path.normcase(os.path.normpath(b))


def points_to(command: "str | None", argv: "list[Path]") -> bool:
    """Whether a Run command launches this app: its program (the frozen exe)
    or, in source mode, its script is ours."""
    if not command or not argv:
        return False
    paths = command_paths(command)
    if not paths:
        return False
    if len(argv) == 1:
        return same_path(paths[0], str(argv[0]))
    return len(paths) >= 2 and same_path(paths[-1], str(argv[-1]))


def approved_disabled(data: "bytes | None") -> bool:
    """Task Manager marks a disabled entry by setting bit 0 of the first
    byte (0x03 or 0x07 seen in practice); an absent value means enabled."""
    return bool(data) and bool(data[0] & 0x01)


class RunStore:
    """What the startup logic needs from the registry, so a fake can stand in
    for winreg in tests. `run_*` act on ...\\CurrentVersion\\Run,
    `approved_*` on ...\\Explorer\\StartupApproved\\Run."""

    def run_get(self, name: str) -> "str | None": ...
    def run_set(self, name: str, command: str) -> None: ...
    def run_delete(self, name: str) -> None: ...
    def approved_get(self, name: str) -> "bytes | None": ...
    def approved_set(self, name: str, data: bytes) -> None: ...
    def approved_delete(self, name: str) -> None: ...


def startup_enabled(store: RunStore, argv: "list[Path] | None") -> bool:
    """True when our value exists, launches this very app, and Task Manager
    has not disabled it."""
    if argv is None:
        return False
    command = store.run_get(RUN_VALUE)
    return points_to(command, argv) and not approved_disabled(store.approved_get(RUN_VALUE))


def enable_startup(store: RunStore, command: "str | None",
                   problem: "str | None") -> "str | None":
    """Register `command`. Returns None on success or a message for the
    user. `problem` is the location check's verdict on this install."""
    if problem is not None:
        return problem
    if command is None:
        return "WeekNumber could not determine where it is installed."
    if len(command) > MAX_RUN_COMMAND:
        return ("WeekNumber's install path is too long for Windows to start it "
                "automatically. Move it to a shorter path, such as "
                "C:\\Program Files\\WeekNumber (needs administrator rights) or "
                "%LOCALAPPDATA%\\Programs\\WeekNumber.")
    store.run_set(RUN_VALUE, command)
    # Only an explicit enable clears a Task Manager "disabled" mark.
    store.approved_set(RUN_VALUE, APPROVED_ENABLED)
    return None


def disable_startup(store: RunStore) -> None:
    store.run_delete(RUN_VALUE)


def migrate_legacy(store: RunStore, argv: "list[Path] | None") -> bool:
    """Move an earlier build's "WeekNumber" value to our own name -- but only when it
    launches this app, so another program's entry is never touched. Its
    Task Manager state moves with it."""
    if argv is None or store.run_get(RUN_VALUE) is not None:
        return False
    legacy = store.run_get(LEGACY_RUN_VALUE)
    if not points_to(legacy, argv):
        return False
    store.run_set(RUN_VALUE, legacy)
    approved = store.approved_get(LEGACY_RUN_VALUE)
    if approved is not None:
        store.approved_set(RUN_VALUE, approved)
        store.approved_delete(LEGACY_RUN_VALUE)
    store.run_delete(LEGACY_RUN_VALUE)
    log.info("Migrated the startup entry to %s", RUN_VALUE)
    return True


def refresh_startup(store: RunStore, command: "str | None", problem: "str | None",
                    exists=os.path.exists) -> bool:
    """Repair our entry after the app was *moved*, never repoint it.

    The value is rewritten only when the program it names no longer exists
    (the app moved), the entry isn't disabled, and the new location passes
    the location check. Launching some other copy -- an old build, a dev
    checkout, a USB stick -- leaves autostart alone. Returns whether it
    rewrote the value."""
    current = store.run_get(RUN_VALUE)
    if current is None or command is None or current == command:
        return False
    paths = command_paths(current)
    registered = paths[-1] if paths else ""
    if registered and exists(registered):
        log.info("Startup entry points to another copy; leaving it unchanged")
        return False
    if approved_disabled(store.approved_get(RUN_VALUE)):
        return False
    if problem is not None or len(command) > MAX_RUN_COMMAND:
        log.warning("Not repointing startup: %s", problem or "command too long")
        return False
    store.run_set(RUN_VALUE, command)
    log.info("Startup entry repointed after the app moved")
    return True


# ---------------------------------------------------------------------------
# IT policy
# ---------------------------------------------------------------------------
#: Administrators set policy under HKLM (then HKCU) \Software\Policies\WeekNumber
#: on Windows, and as managed preferences for this domain on macOS.
POLICY_KEY = "Software\\Policies\\WeekNumber"
POLICY_DOMAIN = "io.github.shanko100.weeknumber"
POLICY_FLAGS = ("DisableOnline", "DisableLAN", "DisableChat", "DisableGame")
POLICY_RELAY_URL = "RelayURL"
MAX_RELAY_URL_CHARS = 200


class Policy(NamedTuple):
    """What the organisation allows. The defaults allow everything."""
    disable_online: bool = False
    disable_lan: bool = False
    disable_chat: bool = False
    disable_game: bool = False
    #: An organisation's own relay, replacing the public one.
    relay_url: "str | None" = None


class PolicyStore:
    """Raw policy values from the OS, strongest scope first, so a fake can
    stand in for the registry or managed preferences in tests. `flag(name)`
    is the raw value of a switch, `text(name)` of a string policy; both are
    None when the policy is not set."""

    def flag(self, name: str) -> object: ...
    def text(self, name: str) -> "str | None": ...


def _flag_on(value: object) -> bool:
    # A DWORD or a plist number or boolean; anything else is not a decision.
    if isinstance(value, bool):
        return value
    return isinstance(value, int) and value != 0


def _relay_url(value: "str | None") -> "str | None":
    if value is None:
        return None
    url = value.strip()
    parts = urllib.parse.urlsplit(url) if len(url) <= MAX_RELAY_URL_CHARS else None
    if parts is None or parts.scheme != "https" or not parts.hostname \
            or parts.username or parts.password or parts.query or parts.fragment:
        log.warning("Ignoring the RelayURL policy: it must be a plain https:// address")
        return None
    return url.rstrip("/")


def parse_policy(store: PolicyStore) -> Policy:
    try:
        flags = {name: _flag_on(store.flag(name)) for name in POLICY_FLAGS}
        return Policy(disable_online=flags["DisableOnline"],
                      disable_lan=flags["DisableLAN"],
                      disable_chat=flags["DisableChat"],
                      disable_game=flags["DisableGame"],
                      relay_url=_relay_url(store.text(POLICY_RELAY_URL)))
    except Exception:
        log.exception("Could not read the IT policy; using the defaults")
        return Policy()
