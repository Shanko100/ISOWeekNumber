"""Build-time tool: writes Licenses.txt, the legal notices and every license
a release build carries.

The build scripts bundle the file into WeekNumber.exe and WeekNumber.app,
where About > Legal notices opens it, and put a copy next to the app in the
macOS disk image. It holds, in order:

* docs/LEGAL_NOTICES.md (license and warranty, trademarks, privacy,
  encryption), WeekNumber's own MIT License and docs/THIRD_PARTY_NOTICES.md;
* the license texts in licenses/ that apply to this platform's build;
* the licenses of the build interpreter itself (CPython, Tcl, Tk) and of
  Pillow's wheel with the native libraries it bundles, copied from the
  machine that builds, since they depend on that installation;
* the exact package versions of the build.

A source checkout shows the first two groups in the app instead
(repository_sections; weeknumber_license_dialog).

Usage, with the build's Python: python collect_licenses.py OUTPUT
"""

from __future__ import annotations

import importlib.metadata
import platform
import re
import sys
import sysconfig
import tkinter
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Callable

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
LICENSES = ROOT / "licenses"

#: License texts for components that one platform's build never bundles.
NOT_BUNDLED: "dict[str, set[str]]" = {
    "darwin": set(),
    "win32": set(),
}
#: Taken from the build interpreter instead (below). licenses/ holds these
#: only as fallbacks: Python for Windows ships Tk's text but not Tcl's, and
#: the python.org macOS framework ships Tcl's but not Tk's.
FROM_BUILD = {"Tcl-license.terms", "Tk-license.terms"}

def _version() -> str:
    source = (HERE / "weeknumber_core.py").read_text(encoding="utf-8")
    return re.search(r'APP_VERSION = "([^"]+)"', source).group(1)


def section(title: str, text: str) -> str:
    """One titled section: the title between two rule lines (what the Legal
    notices window colours as a heading)."""
    rule = "=" * 76
    return f"{rule}\n{title}\n{rule}\n\n{text.strip()}\n\n\n"


def repository_sections(target: str = sys.platform,
                        read: "Callable[[Path], str] | None" = None) -> "list[tuple[str, str]]":
    """(title, text) for what the repository itself holds: the legal
    notices, WeekNumber's license, the third-party notices and the texts in
    licenses/ that apply to `target`'s build. `read(path)` reads a file
    (the app passes its size-capped reader)."""
    read = read or (lambda path: path.read_text(encoding="utf-8"))
    sections = [("Legal notices", _strip_title(read(ROOT / "docs" / "LEGAL_NOTICES.md"))),
                ("WeekNumber: LICENSE", read(ROOT / "LICENSE")),
                ("Third-party notices",
                 _strip_title(read(ROOT / "docs" / "THIRD_PARTY_NOTICES.md")))]
    skip = NOT_BUNDLED.get(target, set()) | FROM_BUILD
    for path in sorted(LICENSES.iterdir(), key=lambda p: p.name.lower()):
        if path.is_file() and path.name not in skip:
            sections.append((f"licenses/{path.name}", read(path)))
    return sections


def _strip_title(text: str) -> str:
    first, _, rest = text.partition("\n")
    return rest if first.startswith("# ") else text


def _first(paths: "list[Path]") -> "Path | None":
    return next((path for path in paths if path.is_file()), None)


def _interpreter_licenses() -> "list[tuple[str, Path]]":
    """CPython's, Tcl's and Tk's license files from the build interpreter."""
    stdlib = Path(sysconfig.get_paths()["stdlib"])
    base = Path(sys.base_prefix)
    cpython = _first([base / "LICENSE.txt", stdlib / "LICENSE.txt"])
    # Tcl's library is .../tcl/tcl8.6 on Windows and
    # .../Tcl.framework/Versions/8.6/Resources/Scripts in a macOS framework.
    library = Path(tkinter.Tcl().eval("info library"))
    tcl = _first([library / "license.terms", library.parent / "license.terms",
                  LICENSES / "Tcl-license.terms"])
    tk_framework = Path(str(library).replace("Tcl.framework", "Tk.framework"))
    tk = (_first([library.parent / f"tk{tkinter.TkVersion}" / "license.terms",
                  tk_framework.parent / "license.terms"])
          or _search_tk(base, library) or _first([LICENSES / "Tk-license.terms"]))
    found = [("CPython", cpython), ("Tcl", tcl), ("Tk", tk)]
    missing = [name for name, path in found if path is None]
    if missing:
        raise SystemExit(f"License file not found for: {', '.join(missing)} "
                         f"(Python in {base}, Tcl library {library})")
    return found


def _search_tk(*roots: Path) -> "Path | None":
    """Tk's license.terms anywhere under the Python install or beside Tcl,
    for layouts the paths above don't cover."""
    for root in dict.fromkeys(roots):
        for path in sorted(root.rglob("license.terms")):
            if path.parent.name.lower().startswith("tk") or "Tk.framework" in path.parts:
                return path
    return None


def _package_license(name: str) -> Path:
    files = importlib.metadata.distribution(name).files or []
    for file in files:
        if file.name.upper().startswith(("LICENSE", "COPYING")):
            path = Path(file.locate())
            if path.is_file():
                return path
    raise SystemExit(f"License file not found in the {name} package")


def _environment() -> str:
    packages = sorted((dist.metadata["Name"], dist.version)
                      for dist in importlib.metadata.distributions())
    lines = [f"Python {platform.python_version()} ({platform.machine()})", ""]
    lines += [f"{name}=={version}" for name, version in packages]
    return "\n".join(lines)


def build(target: str = sys.platform) -> str:
    system = "Windows" if target == "win32" else "macOS"
    title = f"WeekNumber {_version()} for {system} - legal notices and licenses"
    out = [f"{title}\n{'=' * len(title)}\n\n"
           "The legal notices come first: license and warranty, trademarks,\n"
           "privacy and encryption. Then WeekNumber's MIT license, the third-party\n"
           "components this build bundles with their licenses, and those of the\n"
           "Python, Tcl and Tk the build was made with.\n\n\n"]
    out += [section(name, text) for name, text in repository_sections(target)]
    for name, path in _interpreter_licenses():
        out.append(section(f"{name} (from the build's Python)",
                            path.read_text(encoding="utf-8", errors="replace")))
    out.append(section("Pillow, with the native libraries in its wheel",
                        _package_license("pillow").read_text(encoding="utf-8", errors="replace")))
    out.append(section("Build environment", _environment()))
    return "".join(out).rstrip() + "\n"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python collect_licenses.py OUTPUT")
    output = Path(sys.argv[1])
    output.parent.mkdir(parents=True, exist_ok=True)
    # A BOM so Notepad and TextEdit both read it as UTF-8.
    output.write_text(build(), encoding="utf-8-sig", newline="\r\n" if sys.platform == "win32" else "\n")
    print(f"Wrote {output} ({output.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
