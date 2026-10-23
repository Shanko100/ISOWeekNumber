"""The chat panel shared by the Timezone Dominator lobby and game windows.

It shows the session's chat history (TzdSession.chat_log), an entry with a
Send pill and a quiet "Encrypted" chip. Clicking the chip shows the safety
code in the message list's place, so the window keeps its size. Everything
it shows comes from the session, so a window can rebuild the panel at any
time without losing messages; `append` adds one message without a rebuild.
"""

from __future__ import annotations

import tkinter as tk
from typing import TYPE_CHECKING

from weeknumber_tzd_board import player_color
from weeknumber_widgets import PillButton

if TYPE_CHECKING:
    from weeknumber_tzd_lobby import TzdSession


class ChatPanel(tk.Frame):
    """Header, message list, entry. `lines` is the list's height."""

    def __init__(self, master: tk.Misc, session: "TzdSession", theme: dict[str, str],
                 scale: float, family: str, variable: tk.StringVar, lines: int = 5,
                 on_status=None) -> None:
        super().__init__(master, bg=theme["CARD_BG"])
        self.session = session
        self.theme = theme
        self.scale = scale
        self.family = family
        self.variable = variable
        self._on_status = on_status or (lambda text: None)
        self.unread = 0

        header = tk.Frame(self, bg=theme["CARD_BG"])
        header.pack(fill="x", pady=(0, self._px(4)))
        self.title = tk.Label(header, text="Chat", font=self._font(11, True), anchor="w",
                              bg=theme["CARD_BG"], fg=theme["TITLE_FG"])
        self.title.pack(side="left")
        self.badge = tk.Label(header, text="", font=self._font(10, True),
                              bg=theme["CARD_BG"], fg=theme["ACCENT"])
        self.badge.pack(side="left", padx=(self._px(6), 0))
        self.lock = tk.Label(header, text="Not secured", font=self._font(10), anchor="e",
                             bg=theme["CARD_BG"], fg=theme["TITLE_FG"])
        self.lock_chip = PillButton(header, "", self._font(10, True), scale,
                                    self._toggle_code, variant="chip")
        self.lock_chip.apply_theme(theme)

        # width=1: the list takes the window's width instead of asking for 80
        # characters of its own.
        self.log = tk.Text(self, height=lines, width=1, wrap="word", font=self._font(10),
                           relief="flat", bd=0, highlightthickness=1, padx=self._px(6),
                           pady=self._px(4), cursor="arrow", takefocus=False,
                           bg=theme["INPUT_BG"], fg=theme["TITLE_FG"],
                           highlightbackground=theme["INPUT_BORDER"],
                           highlightcolor=theme["INPUT_BORDER"])
        self.log.pack(fill="x")
        self.log.tag_configure("note", foreground=theme["YEAR_FG"])
        self.log.tag_configure("code", font=self._font(12, True))

        row = tk.Frame(self, bg=theme["CARD_BG"])
        row.pack(fill="x", pady=(self._px(6), 0))
        self.entry = tk.Entry(row, textvariable=variable, width=1, font=self._font(11), relief="flat",
                              bd=self._px(4), bg=theme["INPUT_BG"], fg=theme["TITLE_FG"],
                              insertbackground=theme["TITLE_FG"],
                              disabledbackground=theme["CARD_BG"],
                              disabledforeground=theme["YEAR_FG"], highlightthickness=1,
                              highlightbackground=theme["INPUT_BORDER"],
                              highlightcolor=theme["ACCENT"])
        self.entry.pack(side="left", fill="x", expand=True, padx=(0, self._px(8)))
        self.entry.bind("<Return>", lambda e: self._send())
        self.entry.bind("<FocusIn>", lambda e: self.mark_read(), add="+")
        self.send_button = PillButton(row, "Send", self._font(11, True), scale, self._send,
                                      primary=False)
        self.send_button.apply_theme(theme)
        self.send_button.pack(side="right")
        self.refresh()

    def _px(self, value: float) -> int:
        return max(1, round(value * self.scale))

    def _font(self, size: float, bold: bool = False):
        return (self.family, -self._px(size), "bold") if bold else (self.family, -self._px(size))

    # -- content ------------------------------------------------------------------
    def refresh(self) -> None:
        """Rebuild the list (or the safety code) and the chip from the session."""
        session = self.session
        secure = session.chat_state == "secure" and bool(session.safety_code)
        showing_code = secure and session.code_shown
        self.log.configure(state="normal")
        self.log.delete("1.0", "end")
        if showing_code:
            self._write_code()
        else:
            for entry in session.chat_log:
                self._write(entry)
            if not session.chat_log:
                self.log.insert("end", self._empty_text(), "note")
        self.log.configure(state="disabled")
        self.log.see("1.0" if showing_code else "end")
        self.entry.configure(state="normal" if secure else "disabled")
        self.send_button.set_enabled(secure)
        self.lock.pack_forget()
        self.lock_chip.pack_forget()
        if secure:
            self.lock_chip.set_text("Hide code" if showing_code else "\U0001F512 Encrypted")
            self.lock_chip.pack(side="right")
        elif session.chat_state == "failed":
            self.lock.pack(side="right")
        self._show_unread()

    def _toggle_code(self) -> None:
        self.session.code_shown = not self.session.code_shown
        self.refresh()

    def _write_code(self) -> None:
        """The safety code and what to do with it, in the list's place."""
        self.log.insert("end", "Safety code  ", "note")
        self.log.insert("end", self.session.safety_code, "code")
        self.log.insert("end", "\n" + self.code_hint(), "note")

    def code_hint(self) -> str:
        # Two lines at most on the narrowest board, so the code and the hint
        # fit the game window's three-line chat.
        if self.session.is_host:
            return ("Every player sees this same code. If anyone's code is different, "
                    "someone else may be reading the chat.")
        return ("The host sees this same code. If theirs is different, someone else "
                "may be reading the chat.")

    def append(self, entry: dict) -> None:
        if self.session.code_shown and self.session.chat_state == "secure":
            pass  # the code is showing; folding it back rebuilds the list
        elif len(self.session.chat_log) == 1:  # the first message replaces the hint
            self.refresh()
        else:
            self.log.configure(state="normal")
            self._write(entry)
            self.log.configure(state="disabled")
            self.log.see("end")
        if not entry.get("mine") and self.focus_get() is not self.entry:
            self.unread += 1
            self._show_unread()

    def mark_read(self) -> None:
        self.unread = 0
        self._show_unread()

    def _show_unread(self) -> None:
        self.badge.configure(text=f"• {self.unread} new" if self.unread else "")

    def _write(self, entry: dict) -> None:
        tag = f"player-{entry['from']}"
        self.log.tag_configure(tag, foreground=self._color_of(entry["from"]),
                               font=self._font(10, True))
        if self.log.index("end-1c") != "1.0":
            self.log.insert("end", "\n")
        name = "You" if entry.get("mine") else entry["name"]
        self.log.insert("end", name, tag)
        try:
            self.log.insert("end", "  " + entry["text"])
        except tk.TclError:  # a Tk that cannot take a character (some emoji)
            self.log.insert("end", "  " + "".join(
                ch if ord(ch) <= 0xFFFF else "\uFFFD" for ch in entry["text"]))

    def _color_of(self, pid: int) -> str:
        players = (self.session.lobby or {}).get("players", [])
        slot = next((p["color"] for p in players if p["id"] == pid), None)
        return player_color(self.theme, -1 if slot is None else slot)

    def _empty_text(self) -> str:
        state = self.session.chat_state
        if state == "secure":
            return "No messages yet. Only the players can read this chat."
        if state == "pending":
            return "Securing chat…"
        if state == "failed":
            return ("Chat is off: the host's key did not check out, so someone may be "
                    "listening in.")
        return "Chat is unavailable in this lobby."

    def _send(self) -> None:
        text = self.variable.get()
        problem = self.session.send_chat(text)
        if problem is None:
            self.variable.set("")
            if self.session.code_shown:  # back to the messages, to see it arrive
                self._toggle_code()
        self._on_status(problem or "")

