"""The Legal notices window: WeekNumber's legal notices and every license
text the build carries, in the app.

About > Legal notices opens it beside the About window, which stays open. It
shows one long text in WeekNumber's own style -- no list of sections to pick
from: the section titles in the accent colour, the sub-headings of the
notices in bold, the opening paragraph as prose and the license texts in a
monospaced face so their layout survives, with a slim themed scroll bar.

Builds bundle Licenses.txt (collect_licenses.py). A source run has no such
file, so it shows the repository's own documents through the same
collect_licenses.repository_sections the build uses -- which is what a
build's file starts with.

The text is read with the same distrust as the other data files (size-capped,
strict UTF-8) and only ever inserted as plain text.
"""

from __future__ import annotations

import re
import sys
import tkinter as tk
import tkinter.font as tkfont
from pathlib import Path
from typing import TYPE_CHECKING

from weeknumber_core import (
    APP_NAME,
    IS_MACOS,
    SettingsStore,
    bundled_file,
    log,
    read_bounded_text,
)
from weeknumber_dialog import TitledDialog
from weeknumber_widgets import PillButton, ScrollBar

if TYPE_CHECKING:
    from weeknumber_core import RECT

#: Every notice and license the build carries, written by collect_licenses.py.
LICENSES_FILE = "Licenses.txt"
MAX_LICENSE_BYTES = 4 * 1024 * 1024
#: A rule line of collect_licenses.section: 76 equals signs.
_RULE_MIN = 20
#: A Markdown heading inside a section ("## Privacy"), shown in bold.
_SUBHEADING = re.compile(r"#{1,3} (\S.*)")

MONO_FAMILIES = (("SF Mono", "Menlo", "Monaco") if IS_MACOS
                 else ("Cascadia Mono", "Consolas", "Lucida Console"))


def _repository_root() -> "Path | None":
    if getattr(sys, "frozen", False):
        return None
    root = Path(__file__).resolve().parents[2]
    return root if (root / "LICENSE").is_file() else None


def licenses_available() -> bool:
    """Whether there are texts to show, without reading them."""
    return bundled_file(LICENSES_FILE) is not None or _repository_root() is not None


def license_text() -> "str | None":
    """The notices and license texts to show, or None when none can be found."""
    path = bundled_file(LICENSES_FILE)
    if path is not None:
        try:
            return read_bounded_text(path, MAX_LICENSE_BYTES).lstrip("\ufeff")
        except Exception:
            log.exception("Could not read %s", LICENSES_FILE)
            return None
    if _repository_root() is None:
        return None
    try:
        import collect_licenses  # beside this file in a checkout, never in a build
        sections = collect_licenses.repository_sections(
            read=lambda p: read_bounded_text(p, MAX_LICENSE_BYTES))
    except Exception:
        log.exception("Could not read the repository's license texts")
        return None
    title = f"{APP_NAME} legal notices and licenses (source checkout)"
    return (f"{title}\n{'=' * len(title)}\n\n"
            "A release build also lists the exact Python, Tcl/Tk and Pillow\n"
            "licenses of the machine that built it here.\n\n\n"
            + "".join(collect_licenses.section(name, text) for name, text in sections))


def split_sections(text: str) -> "list[tuple[str | None, str]]":
    """(title, body) pairs in order. A title is the line between two rule
    lines, or a line underlined with "=" to its length; text before the
    first title has no title."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")

    def rule(index: int, minimum: int = _RULE_MIN) -> bool:
        line = lines[index].strip() if 0 <= index < len(lines) else ""
        return len(line) >= minimum and set(line) == {"="}

    sections: list[tuple[str | None, list[str]]] = [(None, [])]
    i = 0
    while i < len(lines):
        if rule(i) and rule(i + 2) and lines[i + 1].strip():
            sections.append((lines[i + 1].strip(), []))   # === / Title / ===
            i += 3
            continue
        title = lines[i].strip()
        if (title and not rule(i, 1) and rule(i + 1, min(_RULE_MIN, max(3, len(title))))
                and not rule(i - 1)):
            sections.append((title, []))                  # Title / ===
            i += 2
            continue
        sections[-1][1].append(lines[i])
        i += 1
    out = []
    for title, body in sections:
        joined = "\n".join(body).strip("\n")
        if title is not None or joined.strip():
            out.append((title, joined))
    return out


def styled_runs(text: str) -> "list[tuple[str, str]]":
    """The whole text as (chunk, tag) runs, in order: "heading" for section
    titles, "subheading" for Markdown headings inside them (shown without
    the #), "intro" for the opening paragraph, "body" for the rest."""
    runs: list[tuple[str, str]] = []
    first = True
    for title, body in split_sections(text):
        if title:
            runs.append((("" if first else "\n") + title + "\n", "heading"))
        if body:
            if first:  # the opening paragraph, before the notices
                runs.append((body + "\n", "intro"))
            else:
                chunk: list[str] = []
                for line in body.split("\n"):
                    match = _SUBHEADING.fullmatch(line.strip())
                    if match is None:
                        chunk.append(line)
                        continue
                    if chunk:
                        runs.append(("\n".join(chunk) + "\n", "body"))
                        chunk = []
                    runs.append((match.group(1) + "\n", "subheading"))
                if chunk:
                    runs.append(("\n".join(chunk) + "\n", "body"))
        first = False
    return runs


class LicenseDialog(TitledDialog):
    """Single-instance, scrollable viewer of the legal notices and licenses."""

    TITLE = "Legal notices"
    WIDTH_PX = 680
    HEIGHT_PX = 600
    MARGIN_PX = 24
    #: Offset from the About window it opens from, px @ 96 DPI.
    CASCADE_PX = 28

    def __init__(self, master: tk.Misc, settings: SettingsStore,
                 text: str, near: "tk.Misc | None" = None) -> None:
        work = self._init_window(master, settings)
        self.top.resizable(True, True)
        self._runs = styled_runs(text)
        self._mono = self._mono_family()
        self._build()
        self._apply_theme()
        self._place(work, near)
        self.top.deiconify()
        self._sync_titlebar()
        self._raise()
        self.text.focus_set()

    # -- construction ---------------------------------------------------------
    def _mono_family(self) -> str:
        try:
            families = set(tkfont.families(self.top))
        except tk.TclError:
            families = set()
        found = next((f for f in MONO_FAMILIES if f in families), None)
        if found is not None:
            return found
        try:
            return tkfont.nametofont("TkFixedFont", self.top).actual("family")
        except tk.TclError:
            return "Courier"

    def _build(self) -> None:
        margin = self._px(self.MARGIN_PX)
        self.card = tk.Frame(self.top, padx=margin, pady=self._px(20))
        self.card.pack(fill="both", expand=True)

        self.title = tk.Label(self.card, text="Legal notices", font=self._font(19, bold=True),
                              anchor="w")
        self.title.pack(fill="x")
        self.subtitle = tk.Label(
            self.card, anchor="w", justify="left", font=self._font(11),
            text=f"{APP_NAME}'s license and warranty, trademarks, privacy and encryption "
                 "notices, followed by the license of every component it includes.",
            wraplength=self._px(self.WIDTH_PX - 2 * self.MARGIN_PX))
        self.subtitle.pack(fill="x", pady=(self._px(4), self._px(12)))

        self.footer = tk.Frame(self.card)
        self.footer.pack(side="bottom", fill="x", pady=(self._px(14), 0))
        self.close_button = PillButton(self.footer, "Close", self._font(11, bold=True),
                                       self.scale, self.close)
        self.close_button.pack(side="right")

        # 1px hairline around the text, like the map and the chat list.
        self.frame = tk.Frame(self.card)
        self.frame.pack(fill="both", expand=True)
        self.text = tk.Text(self.frame, wrap="word", width=1, height=1, bd=0,
                            relief="flat", highlightthickness=0, cursor="arrow",
                            padx=self._px(14), pady=self._px(10),
                            font=(self._mono, -self._px(11.5)),
                            spacing1=0, spacing3=self._px(1))
        self.scrollbar = ScrollBar(self.frame, self.scale, self.text.yview)
        self.scrollbar.pack(side="right", fill="y", padx=(0, 1), pady=1)
        self.text.pack(side="left", fill="both", expand=True, padx=(1, 0), pady=1)
        self.text.configure(yscrollcommand=self._scrolled)
        self._fill()

        # The usual keys scroll the read-only text. Bound on the text itself
        # and ending in "break": its class bindings would move the hidden
        # insert mark instead and snap the view back to it.
        for sequence, action in (
                ("<Prior>", ("scroll", -1, "pages")), ("<Next>", ("scroll", 1, "pages")),
                ("<Up>", ("scroll", -3, "units")), ("<Down>", ("scroll", 3, "units")),
                ("<Home>", ("moveto", 0.0)), ("<End>", ("moveto", 1.0)),
                ("<Control-Home>", ("moveto", 0.0)), ("<Control-End>", ("moveto", 1.0))):
            self.text.bind(sequence, lambda e, a=action: self._scroll(*a))
        self.text.bind("<Command-a>" if IS_MACOS else "<Control-a>", self._select_all)

    def _fill(self) -> None:
        text = self.text
        text.configure(state="normal")
        text.delete("1.0", "end")
        for chunk, tag in self._runs:
            text.insert("end", chunk, tag)
        text.configure(state="disabled")

    def _place(self, work: "RECT", near: "tk.Misc | None") -> None:
        """Beside the About window when it is open, cascaded down and to the
        right of it like a document from it; otherwise centred. Always inside
        the work area."""
        margin = self._px(self.SCREEN_MARGIN_PX)
        width = min(self._px(self.WIDTH_PX), work.right - work.left - margin)
        height = min(self._px(self.HEIGHT_PX), work.bottom - work.top - margin)
        x = work.left + (work.right - work.left - width) // 2
        y = work.top + (work.bottom - work.top - height) // 2
        if near is not None:
            try:
                step = self._px(self.CASCADE_PX)
                x, y = near.winfo_rootx() + step, near.winfo_rooty() + step
            except tk.TclError:
                pass
        x = max(work.left, min(x, work.right - width - self._px(16)))
        y = max(work.top, min(y, work.bottom - height - self._px(48)))
        self.top.minsize(self._px(420), self._px(320))
        self.top.geometry(f"{width}x{height}+{int(x)}+{int(y)}")

    # -- theme ----------------------------------------------------------------------
    def _apply_theme(self) -> None:
        super()._apply_theme()
        theme = self.theme
        bg, fg, muted = theme["CARD_BG"], theme["TITLE_FG"], theme["YEAR_FG"]
        self.top.configure(bg=bg)
        for frame in (self.card, self.footer):
            frame.configure(bg=bg)
        self.title.configure(bg=bg, fg=fg)
        self.subtitle.configure(bg=bg, fg=muted)
        self.frame.configure(bg=theme["INPUT_BORDER"])
        self.text.configure(bg=theme["INPUT_BG"], fg=fg, insertbackground=fg,
                            selectbackground=theme["MENU_ACTIVE_BG"],
                            selectforeground=theme["MENU_ACTIVE_FG"],
                            inactiveselectbackground=theme["MENU_ACTIVE_BG"])
        self.text.tag_configure("heading", font=self._font(13, bold=True),
                                foreground=theme["ACCENT"], spacing1=self._px(14),
                                spacing3=self._px(8))
        self.text.tag_configure("subheading", font=self._font(11.5, bold=True),
                                foreground=fg, spacing1=self._px(8), spacing3=self._px(3))
        self.text.tag_configure("intro", font=self._font(11), foreground=muted)
        self.text.tag_configure("body", foreground=fg)
        self.scrollbar.apply_theme({**theme, "CARD_BG": theme["INPUT_BG"]})
        self.close_button.apply_theme(theme)

    # -- interaction --------------------------------------------------------------
    def _scrolled(self, first, last) -> None:
        self.scrollbar.set(first, last)

    def _scroll(self, *action) -> str:
        self.text.yview(*action)
        return "break"

    def _select_all(self, event=None) -> str:
        self.text.tag_add("sel", "1.0", "end-1c")
        return "break"


def show_licenses(master: tk.Misc, settings: SettingsStore,
                  near: "tk.Misc | None" = None) -> bool:
    """Open the Legal notices window (or bring it forward); False when there
    are no texts to show."""
    if LicenseDialog.raise_open():
        return True
    text = license_text()
    if text is None:
        return False
    LicenseDialog.show(master, settings, text, near)
    return LicenseDialog._live() is not None
