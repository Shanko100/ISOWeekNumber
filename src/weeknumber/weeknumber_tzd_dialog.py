"""Timezone Dominator game window for WeekNumber.

The board on top -- the time-zone map, repainted by owner (see
weeknumber_tzd_board) -- and a three-column panel below it: whose turn it
is and what to do next, the controls for the current step, and the players
with your cards; the chat runs underneath.

Every move is visible: a banner in the current player's colour with the turn's
steps beside it, an announcement over the board when the turn passes, a
frame around the map in the colour of whoever is playing, a caption along
the board's foot for each move, a log line per move with names in their
colours, and the action you have to take next drawn as the primary button.

Giving up is a small "Give up…" chip beside the turn banner, behind a
question (acknowledge defeat, or keep playing); afterwards the player keeps
watching. A dropped connection shows "Reconnecting…" while the app gets the
seat back on its own (weeknumber_tzd_net), and a Reconnect button if that
did not work in time.

The window never changes the game itself. It reads the session's latest
state, uses the rules module only to decide what is clickable, and sends
actions; the lobby host applies them and every window repaints from the
state that comes back.
"""

from __future__ import annotations

import tkinter as tk
import tkinter.font as tkfont
from collections import Counter, deque
from typing import TYPE_CHECKING

from PIL import ImageTk

import weeknumber_tzd_net as net
import weeknumber_tzd_rules as rules
import weeknumber_tzd_sound as sound
from weeknumber_core import (APP_NAME, SettingsStore, log, network, reduce_motion,
                              request_attention)
from weeknumber_dialog import ConfirmDialog, TitledDialog
from weeknumber_map import fit_map_size
from weeknumber_tzd_board import TzdBoardView, player_color, text_on
from weeknumber_tzd_chat_view import ChatPanel
from weeknumber_tzd_fx import BoardEffects, CardHand, DiceRow
from weeknumber_timezone import drop_map, use_map
from weeknumber_widgets import PillButton, blend, rounded

if TYPE_CHECKING:
    from weeknumber_tzd_lobby import TzdSession

PHASE_NAMES = {
    rules.PHASE_REINFORCE: "Reinforce",
    rules.PHASE_ATTACK: "Attack",
    rules.PHASE_OCCUPY: "Occupy",
    rules.PHASE_FORTIFY: "Fortify",
    rules.PHASE_OVER: "Game over",
}


class _TurnBanner(tk.Canvas):
    """A capsule with one line of mixed-colour text: whose turn it is, the
    player's name in their own colour."""

    def __init__(self, master: tk.Misc, family: str, scale: float) -> None:
        super().__init__(master, width=1, height=1, bd=0, highlightthickness=0)
        self._family = family
        self._scale = scale
        self._image = None
        self._shown = None

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def show(self, parts: "list[tuple[str, str]]", fill: str, bg: str) -> None:
        """`parts` are (text, colour) runs, drawn bold on a `fill` capsule."""
        key = (tuple(parts), fill, bg)
        if key == self._shown:
            return
        self._shown = key
        font = (self._family, -self._px(17), "bold")
        measure = tkfont.Font(root=self, font=font)
        pad_x, pad_y = self._px(16), self._px(7)
        widths = [measure.measure(text) for text, _ in parts]
        height = measure.metrics("linespace") + 2 * pad_y
        width = sum(widths) + 2 * pad_x
        self.delete("all")
        self.configure(width=width, height=height, bg=bg)
        self._image = ImageTk.PhotoImage(rounded(width, height, height / 2, fill, bg), master=self)
        self.create_image(0, 0, anchor="nw", image=self._image)
        x = pad_x
        for (text, color), w in zip(parts, widths):
            self.create_text(x, height / 2, text=text, fill=color, font=font, anchor="w")
            x += w


class _PhaseTrack(tk.Canvas):
    """The three steps of a turn, the current one filled in the player's
    colour: Reinforce > Attack > Fortify (occupying is part of Attack)."""

    STEPS = ((rules.PHASE_REINFORCE, "Reinforce"), (rules.PHASE_ATTACK, "Attack"),
             (rules.PHASE_FORTIFY, "Fortify"))

    def __init__(self, master: tk.Misc, family: str, scale: float) -> None:
        super().__init__(master, width=1, height=1, bd=0, highlightthickness=0)
        self._family = family
        self._scale = scale
        self._image = None
        self._shown = None

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def show(self, phase: "str | None", fill: str, theme: dict[str, str]) -> None:
        current = rules.PHASE_ATTACK if phase == rules.PHASE_OCCUPY else phase
        key = (current, fill, theme["CARD_BG"], theme["TITLE_FG"])
        if key == self._shown:
            return
        self._shown = key
        self.delete("all")
        self._image = None
        self.configure(bg=theme["CARD_BG"])
        steps = [name for name, _label in self.STEPS]
        if current not in steps:
            self.configure(width=1, height=1)
            return
        font = (self._family, -self._px(11), "bold")
        measure = tkfont.Font(root=self, font=font)
        pad_x, height = self._px(10), measure.metrics("linespace") + self._px(8)
        chevron = self._px(16)
        widths = [measure.measure(label) + 2 * pad_x for _name, label in self.STEPS]
        width = sum(widths) + chevron * (len(widths) - 1)
        self.configure(width=width, height=height)
        position = steps.index(current)
        x = 0
        for index, ((_name, label), w) in enumerate(zip(self.STEPS, widths)):
            if index == position:
                self._image = ImageTk.PhotoImage(
                    rounded(w, height, height / 2, fill, theme["CARD_BG"]), master=self)
                self.create_image(x, 0, anchor="nw", image=self._image)
                ink = text_on(fill)
            elif index < position:
                ink = theme["YEAR_FG"]
            else:
                ink = theme["WEEK_NUM_FG"]
            self.create_text(x + w / 2, height / 2, text=label, fill=ink, font=font)
            x += w
            if index < len(widths) - 1:
                self.create_text(x + chevron / 2, height / 2, text="\u203a",
                                 fill=theme["WEEK_NUM_FG"], font=font)
                x += chevron


class TzdGameWindow(TitledDialog):
    """Single-instance game window bound to one TzdSession."""

    TITLE = "Timezone Dominator"
    MAP_WIDTH_PX = 980
    MIN_MAP_WIDTH_PX = 520
    PANEL_COLUMN_PX = 300
    #: One width for the stacked action buttons, px @ 96 DPI.
    BUTTON_COLUMN_PX = 168
    LOG_LINES = 5
    CHAT_LINES = 5
    #: Map frame in the current player's colour, px @ 96 DPI: heavier on yours.
    FRAME_PX = 3
    MY_FRAME_PX = 5

    @classmethod
    def show(cls, master: tk.Misc, settings: SettingsStore, session: "TzdSession") -> None:
        """A new game replaces any open one (its session already ended)."""
        existing = cls._live()
        if existing is not None:
            existing.close(confirm=False)
        try:
            cls._instance = cls(master, settings, session)
        except Exception:
            cls._instance = None
            session.close()
            log.exception("Failed to open the Timezone Dominator game window")

    def __init__(self, master: tk.Misc, settings: SettingsStore,
                 session: "TzdSession") -> None:
        work = self._init_window(master, settings)
        use_map(self)  # the board view draws the world map under the board
        self.session = session
        self.board = session.board
        self.selected: "str | None" = None
        self.target: "str | None" = None
        self.amount = 1
        self.amount_range = (1, 1)
        self.notice = ""
        #: Log lines as (text, player id or None) runs, names drawn in colour.
        self.log_lines: "deque[list[tuple[str, int | None]]]" = deque(maxlen=self.LOG_LINES)
        self._after_blitz = False
        self._hover: "str | None" = None
        self._last_turn_key = None
        self._cued_turn = None          # the (turn, player) last announced
        self._focused = True
        self._trade_pulsed = None       # the turn the must-trade pulse ran in
        self._reduce_motion = reduce_motion()
        self._after_blitz_fx = False
        self.chat_var = tk.StringVar(master)
        self.chat: "ChatPanel | None" = None
        # What the control column and players panel last showed: both are
        # rebuilt only when that changes, not on every click.
        self._layout_key: "tuple | None" = None
        self._players_key: "tuple | None" = None
        state = session.state
        if state is not None:  # opened by the first state; start the log there
            self._record([{"type": "turn", "player": state.current_player["id"],
                           "reinforcements": state.reinforcements}])
        self._build()
        self._fit(work)
        self._apply_theme()
        self._center(work)
        session.subscribe(self._on_session)
        if settings.sound_enabled:
            sound.prepare()
        self.top.bind("<FocusIn>", lambda e: self._set_focused(True), add="+")
        self.top.bind("<FocusOut>", lambda e: self._set_focused(False), add="+")
        self.top.deiconify()
        self._raise()
        self._cue_turn()  # the lobby consumed the first turn event

    @property
    def state(self) -> "rules.GameState | None":
        return self.session.state

    def _name(self, pid: "int | None") -> str:
        state = self.state
        if state is None or pid is None:
            return "?"
        if pid == self.session.my_id:
            return "You"
        try:
            return state.player(pid)["name"]
        except rules.IllegalAction:
            return "?"

    def _player_color(self, pid: "int | None") -> str:
        state = self.state
        try:
            slot = state.player(pid)["color"] if state is not None and pid is not None else -1
        except rules.IllegalAction:
            slot = -1
        return player_color(self.theme, slot)

    def _territory_name(self, tid: "str | None") -> str:
        territory = self.board.territories.get(tid) if tid else None
        return territory.name if territory else "?"

    # -- construction ------------------------------------------------------------
    def _build(self) -> None:
        pad = self._px(16)
        self.card = tk.Frame(self.top, padx=pad, pady=pad)
        self.card.pack(fill="both", expand=True)
        self.view = TzdBoardView(self.card, self.board, self.theme, self.scale,
                                  self._family, self._on_territory, self._on_hover)
        self.view.border.pack()
        self.effects = BoardEffects(self.view, self.theme, self._reduce_motion)

        self.panel = tk.Frame(self.card)
        self.panel.pack(fill="x", pady=(self._px(14), 0))
        for column in range(3):
            self.panel.grid_columnconfigure(column, weight=1 if column == 0 else 0)
        wrap = self._px(self.PANEL_COLUMN_PX + 40)

        self.info = tk.Frame(self.panel)
        self.info.grid(row=0, column=0, sticky="new")
        self.banner_row = tk.Frame(self.info)
        self.banner_row.pack(fill="x")
        self.banner = _TurnBanner(self.banner_row, self._family, self.scale)
        self.banner.pack(side="left")
        self.phases = _PhaseTrack(self.banner_row, self._family, self.scale)
        self.phases.pack(side="left", padx=(self._px(12), 0))
        self.sound_button = PillButton(self.banner_row, self._sound_label(), self._font(10, bold=True),
                                       self.scale, self._toggle_sound, variant="chip")
        self.sound_button.pack(side="left", padx=(self._px(10), 0))
        # Deliberately small and out of the way of the turn's buttons.
        self.concede_button = PillButton(self.banner_row, "Give up\u2026",
                                         self._font(10, bold=True), self.scale,
                                         self._ask_concede, variant="chip")
        self._concede_shown = False
        self.instruction = tk.Label(self.info, font=self._font(11), anchor="w",
                                    justify="left", wraplength=wrap)
        self.instruction.pack(fill="x", pady=(self._px(3), 0))
        self.dice = DiceRow(self.info, self.scale, self._family)
        self.dice.pack(anchor="w", pady=(self._px(8), 0))
        self.log_text = tk.Text(self.info, height=self.LOG_LINES, width=1, wrap="word",
                                font=self._font(10), relief="flat", bd=0,
                                highlightthickness=0, padx=0, pady=0, cursor="arrow",
                                takefocus=False, state="disabled")
        self.log_text.pack(fill="x", pady=(self._px(6), 0))
        self.hover_label = tk.Label(self.info, font=self._font(10), anchor="w")
        self.hover_label.pack(fill="x", pady=(self._px(6), 0))

        self.controls = tk.Frame(self.panel)
        self.controls.grid(row=0, column=1, sticky="n", padx=self._px(18))
        self.stepper = tk.Frame(self.controls)
        self.stepper.pack(pady=(0, self._px(8)))
        small = self._font(12, bold=True)
        self.minus = PillButton(self.stepper, "−", small, self.scale,
                                lambda: self._step(-1), primary=False)
        self.minus.pack(side="left")
        self.amount_label = tk.Label(self.stepper, font=self._font(15, bold=True),
                                     width=4)
        self.amount_label.pack(side="left")
        self.plus = PillButton(self.stepper, "+", small, self.scale,
                               lambda: self._step(1), primary=False)
        self.plus.pack(side="left")
        self.amount_caption = tk.Label(self.controls, font=self._font(10))
        self.amount_caption.pack()
        button_font = self._font(11, bold=True)
        width = self.BUTTON_COLUMN_PX
        self.primary = PillButton(self.controls, "Place", button_font, self.scale,
                                  lambda: self._press("primary"), min_width_px=width)
        self.primary.pack(pady=(self._px(8), 0))
        self.secondary = PillButton(self.controls, "Blitz", button_font, self.scale,
                                    lambda: self._press("secondary"), primary=False,
                                    min_width_px=width)
        self.secondary.pack(pady=(self._px(6), 0))
        self.tertiary = PillButton(self.controls, "End turn", button_font, self.scale,
                                   lambda: self._press("tertiary"), primary=False,
                                   min_width_px=width)
        self.tertiary.pack(pady=(self._px(6), 0))
        self._actions: dict[str, "dict | str | None"] = {}
        self._visible: dict[str, bool] = {}
        self._show_amount = False

        self.side = tk.Frame(self.panel)
        self.side.grid(row=0, column=2, sticky="ne")
        self.players_frame = tk.Frame(self.side)
        self.players_frame.pack(anchor="e")
        self.hand = CardHand(self.side, self.scale, self._family)
        self.hand.pack(anchor="e", pady=(self._px(10), 0))
        self.cards_label = tk.Label(self.side, font=self._font(10), anchor="e",
                                    justify="right")
        self.cards_label.pack(anchor="e", pady=(self._px(4), 0))
        self.trade = PillButton(self.side, "Trade cards", button_font, self.scale,
                                self._trade, primary=False)
        self.trade.pack(anchor="e", pady=(self._px(6), 0))

        self.chat_holder = tk.Frame(self.card)
        self.chat_holder.pack(fill="x", pady=(self._px(14), 0))
        self._build_chat()  # before _fit, which measures the window with it

    def _build_chat(self) -> None:
        """(Re)build the chat row for the current palette; the history lives
        on the session, so nothing is lost."""
        if self.chat is not None:
            if self.chat.theme == self.theme:
                return
            self.chat.destroy()
        self.chat = ChatPanel(self.chat_holder, self.session, self.theme, self.scale,
                              self._family, self.chat_var, lines=self.CHAT_LINES,
                              on_status=self._chat_status)
        self.chat.pack(fill="x")

    def _chat_status(self, text: str) -> None:
        if text != self.notice:
            self.notice = text
            self._refresh()

    def _fit(self, work) -> None:
        """The largest board that fits this monitor together with the panel,
        measured rather than assumed, like the time-zone dialog."""
        self.view.resize(self._px(10), self._px(10))
        self._refresh()
        self.top.update_idletasks()
        placeholder = self._px(10)
        chrome_height = max(0, self.top.winfo_reqheight() - placeholder
                            - self.view.legend_height)
        chrome_width = self._px(32) + 2
        margin = self._px(self.SCREEN_MARGIN_PX)
        width, height = fit_map_size(
            self._px(self.MAP_WIDTH_PX), self._px(self.MIN_MAP_WIDTH_PX),
            (work.right - work.left) - margin - chrome_width,
            (work.bottom - work.top) - margin - chrome_height - self.view.legend_height)
        self.view.resize(width, height)

    # -- theme ----------------------------------------------------------------------
    def _apply_theme(self) -> None:
        super()._apply_theme()
        theme = self.theme
        bg = theme["CARD_BG"]
        self.top.configure(bg=bg)
        for frame in (self.card, self.panel, self.info, self.banner_row, self.controls,
                      self.stepper, self.side, self.players_frame, self.chat_holder):
            frame.configure(bg=bg)
        self.banner.configure(bg=bg)
        self.banner._shown = None  # repaint for the new palette
        self.phases.configure(bg=bg)
        self.phases._shown = None
        self.instruction.configure(bg=bg, fg=theme["TITLE_FG"])
        self.dice.apply_theme(theme)
        self.hand.apply_theme(theme)
        self.effects.theme = theme
        self.log_text.configure(bg=bg, fg=theme["YEAR_FG"])
        for label in (self.hover_label, self.amount_caption, self.cards_label):
            label.configure(bg=bg, fg=theme["YEAR_FG"])
        self.amount_label.configure(bg=bg, fg=theme["TITLE_FG"])
        for button in (self.minus, self.plus, self.primary, self.secondary,
                       self.tertiary, self.trade, self.sound_button, self.concede_button):
            button.apply_theme(theme)
        self.view.apply_theme(theme)
        self._players_key = None  # the rows bake in the palette
        self._build_chat()
        self._refresh()

    # -- session ----------------------------------------------------------------------
    def _on_session(self, kind: str, data: object) -> None:
        try:
            if not self.top.winfo_exists():
                return
        except tk.TclError:
            return
        if kind == "chat":
            if self.chat is not None:
                self.chat.append(data)
            return
        if kind == "secure":
            if self.chat is not None:
                self.chat.refresh()
            return
        if kind == "state":
            self.notice = ""
            events = data.get("events", [])
            self._record(events)
            self._sync_selection()
            self._refresh()
            self._animate(events)
            self._cue_turn()
            return
        elif kind == "error":
            self.notice = str(data)
        elif kind == "reconnecting":
            self.notice = ("Lost the connection to the host. Reconnecting\u2026"
                           if data.get("attempt", 1) <= 1 else
                           f"Lost the connection to the host. Reconnecting "
                           f"(try {data['attempt']}, {data.get('seconds_left', 0)} s left)\u2026")
        elif kind == "resumed":
            self.notice = "Reconnected."
        elif kind == "closed":
            self.notice = (f"{data} The host keeps a dropped player's seat for "
                           f"{round(net.RECONNECT_GRACE_S / 60)} minutes: try Reconnect."
                           if self.session.can_reconnect else f"Connection closed: {data}")
        self._refresh()

    def _record(self, events: list[dict]) -> None:
        for event in events:
            kind = event.get("type")
            if kind == "attack" and self._after_blitz:  # the blitz line said it already
                self._after_blitz = False
                continue
            self._after_blitz = kind == "blitz"
            line = self._describe(event)
            if line:
                self.log_lines.append(line)

    def _describe(self, event: dict) -> "list[tuple[str, int | None]]":
        """One log line as (text, player) runs; names get their colour."""
        kind = event.get("type")
        pid = event.get("player")
        you = pid == self.session.my_id
        who = (self._name(pid), pid)
        to = self._territory_name(event.get("to"))
        frm = self._territory_name(event.get("from"))
        n = event.get("n", 0)
        if kind == "start":
            return [("The game begins", None)]
        if kind == "turn":
            armies = event.get("reinforcements", 0)
            return [who, (f" {'are' if you else 'is'} up \u00b7 {armies} "
                          f"{'army' if armies == 1 else 'armies'} to place", None)]
        if kind == "place":
            return [who, (f" placed {n} on {self._territory_name(event.get('t'))}", None)]
        if kind == "attack":
            return [who, (f" attacked {to} from {frm}: "
                          f"{' '.join(map(str, event.get('att', [])))} vs "
                          f"{' '.join(map(str, event.get('def', [])))}", None)]
        if kind == "blitz":
            rolls = event.get("rolls", 0)
            return [who, (f" blitzed {to} from {frm} in {rolls} "
                          f"{'roll' if rolls == 1 else 'rolls'}: lost {event.get('att_loss', 0)}, "
                          f"defender lost {event.get('def_loss', 0)}", None)]
        if kind == "capture":
            return [who, (f" conquered {to}", None)]
        if kind == "occupy":
            return [who, (f" moved {n} into {to}", None)]
        if kind == "eliminated":
            return [who, (" was eliminated by ", None),
                    (self._name(event.get("by")), event.get("by"))]
        if kind == "trade":
            return [who, (f" traded cards for {event.get('armies', 0)} armies", None)]
        if kind == "fortify":
            return [who, (f" moved {n} from {frm} to {to}", None)]
        if kind == "left":
            return [who, (" left the game", None)]
        if kind == "conceded":
            return [who, (" gave up" if you else " gave up and is watching", None)]
        if kind == "win":
            return [who, (f" {'win' if you else 'wins'} the game!", None)]
        if kind == "card":
            return [who, (" earned a card", None)]
        return []

    def _render_log(self) -> None:
        """The last moves, oldest first; the newest in full contrast, so the
        eye lands on what just happened."""
        text = self.log_text
        text.configure(state="normal")
        text.delete("1.0", "end")
        text.tag_configure("latest", foreground=self.theme["TITLE_FG"])
        last = len(self.log_lines) - 1
        for row, line in enumerate(self.log_lines):
            if row:
                text.insert("end", "\n")
            for chunk, pid in line:
                if pid is None:
                    text.insert("end", chunk, ("latest",) if row == last else ())
                    continue
                tag = f"player-{pid}"
                text.tag_configure(tag, foreground=self._player_color(pid),
                                   font=self._font(10, bold=True))
                text.insert("end", chunk, tag)
        text.see("end")  # long entries wrap past the last line: keep the newest
        text.configure(state="disabled")

    # -- turn cues -----------------------------------------------------------------
    def _set_focused(self, focused: bool) -> None:
        self._focused = focused

    def _cue_turn(self) -> None:
        """Announce each new turn once; your own gets a sound and, when the
        window is in the background, a request for attention."""
        state = self.state
        if state is None or state.phase == rules.PHASE_OVER or self.session.closed:
            return
        key = (state.turn, state.current)
        if key == self._cued_turn:
            return
        self._cued_turn = key
        self._announce_turn(state)
        if state.current_player["id"] != self.session.my_id:
            return
        self._play("turn")
        if not self._focused:
            request_attention(self.top)

    def _announce_turn(self, state: "rules.GameState") -> None:
        """The board's turn announcement, in the player's colour."""
        current = state.current_player
        mine = current["id"] == self.session.my_id
        fill = self._player_color(current["id"])
        armies = state.reinforcements
        title = "Your turn" if mine else f"{current['name']}\u2019s turn"
        subtitle = f"{armies} {'army' if armies == 1 else 'armies'} to place"
        self.effects.announce(title, subtitle, fill, text_on(fill),
                              hold_ms=1700 if mine else 1300)

    def _play(self, name: str) -> None:
        """A game sound (see weeknumber_tzd_sound), unless muted."""
        if self.settings.sound_enabled:
            sound.play(name)

    def _sound_label(self) -> str:
        return "Sound on" if self.settings.sound_enabled else "Sound off"

    def _toggle_sound(self) -> None:
        self.settings.sound_enabled = not self.settings.sound_enabled
        self.settings.save()
        self.sound_button.set_text(self._sound_label())
        if self.settings.sound_enabled:
            sound.prepare()

    # -- effects ---------------------------------------------------------------------
    def _valid(self, *tids: object) -> bool:
        return all(isinstance(t, str) and t in self.board.territories for t in tids)

    @staticmethod
    def _dice_values(value: object) -> "list[int] | None":
        if not isinstance(value, list) or not 0 < len(value) <= 3:
            return None
        if not all(isinstance(d, int) and not isinstance(d, bool) and 1 <= d <= 6 for d in value):
            return None
        return value

    #: Moves the caption along the board's foot reports; the turn has its
    #: own announcement and the win its celebration.
    TOAST_EVENTS = frozenset(("place", "attack", "blitz", "capture", "occupy", "eliminated",
                              "trade", "fortify", "left", "conceded", "card"))

    def _toast(self, events: list[dict]) -> None:
        """Caption the latest move of this update, names in their colours.
        Your own placements are left out: you just clicked them."""
        me = self.session.my_id
        for event in reversed(events):
            kind = event.get("type")
            if kind not in self.TOAST_EVENTS:
                continue
            if kind in ("place", "card") and event.get("player") == me:
                continue
            runs = [(text, self._player_color(pid) if pid is not None else self.theme["TITLE_FG"])
                    for text, pid in self._describe(event)]
            if runs:
                self.effects.toast(runs)
            return

    def _animate(self, events: list[dict]) -> None:
        """Show each move on the board (and the dice), whoever made it.
        Events come from the host and are checked before use."""
        fx = self.effects
        self._toast(events)
        for event in events:
            kind = event.get("type")
            color = self._player_color(event.get("player"))
            if kind == "place" and self._valid(event.get("t")):
                fx.bump(event["t"], color)
            elif kind in ("attack", "blitz") and self._valid(event.get("from"), event.get("to")):
                # A blitz is followed by its final roll: that one only shows the dice.
                repeat = kind == "attack" and self._after_blitz_fx
                if not repeat:
                    fx.arrow(event["from"], event["to"], color)
                    for tid, loss in ((event["from"], event.get("att_loss")),
                                      (event["to"], event.get("def_loss"))):
                        if isinstance(loss, int) and not isinstance(loss, bool) and loss > 0:
                            fx.float_text(tid, f"\u2212{loss}", self.theme["TITLE_FG"])
                if kind == "attack":
                    attack, defence = (self._dice_values(event.get("att")),
                                       self._dice_values(event.get("def")))
                    if attack and defence:
                        self.dice.show(attack, defence, color,
                                       self._player_color(event.get("defender")),
                                       roll=not self._reduce_motion)
                        self._play("dice")
                self._after_blitz_fx = kind == "blitz"
            elif kind == "capture" and self._valid(event.get("to")):
                fx.ring(event["to"], color)
                self._play("capture")
            elif kind == "fortify" and self._valid(event.get("from"), event.get("to")):
                fx.arrow(event["from"], event["to"], color)
                fx.bump(event["to"], color)
            elif kind == "occupy" and self._valid(event.get("to")):
                fx.bump(event["to"], color)
            elif kind == "trade":
                self._play("trade")
            elif kind == "turn":
                self.dice.clear()
            elif kind == "win":
                fx.clear_announcement()
                fx.clear_toast()
                self._celebrate(event.get("player"), events)

    def _celebrate(self, winner: object, events: list[dict]) -> None:
        state = self.state
        if state is None:
            return
        theme = self.theme
        mine = winner == self.session.my_id
        color = self._player_color(winner)
        last = next((e.get("to") for e in reversed(events)
                     if e.get("type") == "capture" and self._valid(e.get("to"))), None)
        origin = self.view.label_xy(last) if last else (self.view.width / 2, self.view.height / 2)
        order = sorted(self.board.order, key=lambda t: (
            (self.view.label_xy(t)[0] - origin[0]) ** 2 + (self.view.label_xy(t)[1] - origin[1]) ** 2))
        owned = sum(1 for owner in state.owner.values() if owner == winner)
        rounds = state.turn + 1
        subtitle = f"Won in {rounds} {'round' if rounds == 1 else 'rounds'} \u00b7 {owned} territories"
        colors = [player_color(theme, p["color"]) for p in state.players if not p["neutral"]]
        if mine:
            title, fill, ink = "You win!", theme["ACCENT"], theme["TODAY_FG"]
        else:
            title, fill, ink = f"{self._name(winner)} wins", blend(color, theme["CARD_BG"], 0.8), color
        self.effects.celebrate(order, color, colors, title, subtitle, fill, ink, festive=mine)
        self._play("win" if mine else "lose")

    def _sync_selection(self) -> None:
        """Keep the selection while it is still meaningful -- repeated attacks
        from the same place -- and drop it when the situation changed."""
        state = self.state
        me = self.session.my_id
        if state is None or state.current_player["id"] != me:
            self.selected = self.target = None
            return
        turn_key = (state.turn, state.current, state.phase)
        if turn_key != self._last_turn_key:
            self._last_turn_key = turn_key
            if state.phase in (rules.PHASE_OCCUPY,):
                self.selected, self.target = state.occupy["from"], state.occupy["to"]
                return
            if state.phase == rules.PHASE_ATTACK and self.target is not None \
                    and state.owner.get(self.target) == me:
                # Just occupied: attacking on from the new territory is likely.
                self.selected, self.target = self.target, None
            elif state.phase != rules.PHASE_ATTACK:
                self.selected = self.target = None
        if self.selected and state.owner.get(self.selected) != me:
            self.selected = self.target = None
        if state.phase == rules.PHASE_ATTACK and self.selected:
            if self.target not in rules.attack_targets(self.board, state, self.selected):
                self.target = None

    # -- interaction -------------------------------------------------------------------
    def _my_turn(self) -> bool:
        state = self.state
        return (state is not None and not self.session.closed
                and self.session.reconnecting is None
                and state.phase != rules.PHASE_OVER
                and state.current_player["id"] == self.session.my_id)

    def _playing(self) -> bool:
        """Whether this player is still in a running game (not out, not
        gone, not watching after giving up)."""
        state = self.state
        if state is None or self.session.closed or state.phase == rules.PHASE_OVER:
            return False
        try:
            me = state.player(self.session.my_id)
        except rules.IllegalAction:
            return False
        return me["alive"] and not me["left"] and not me["neutral"]

    def _ask_concede(self) -> None:
        if not self._playing():
            return
        ConfirmDialog.ask(
            self.top, self.settings, parent=self.top, title="Timezone Dominator",
            heading="Acknowledge defeat?",
            message=("You give up this game. Your armies stay on the board for the "
                     "others to conquer, and you can keep watching until it ends."),
            confirm="Acknowledge defeat", cancel="Keep playing",
            on_result=self._conceded)

    def _conceded(self, confirmed: bool) -> None:
        if confirmed and self._playing():
            self.notice = ""
            self.session.send({"type": "concede"})

    def _show_concede(self, shown: bool) -> None:
        if shown == self._concede_shown:
            return
        self._concede_shown = shown
        if shown:
            self.concede_button.pack(side="left", padx=(self._px(6), 0))
        else:
            self.concede_button.pack_forget()

    def _on_territory(self, tid: str) -> None:
        if not self._my_turn():
            return
        state = self.state
        me = self.session.my_id
        mine = state.owner[tid] == me
        if state.phase == rules.PHASE_REINFORCE:
            if mine:
                self.selected, self.target = tid, None
        elif state.phase == rules.PHASE_ATTACK:
            if mine and state.armies[tid] >= 2:
                self.selected, self.target = tid, None
            elif self.selected and tid in rules.attack_targets(self.board, state, self.selected):
                self.target = tid
            elif mine:
                self.notice = "You need at least two armies there to attack."
        elif state.phase == rules.PHASE_FORTIFY:
            if self.selected and mine and tid in rules.reachable(self.board, state, self.selected):
                self.target = tid
            elif mine and state.armies[tid] > 1:
                self.selected, self.target = tid, None
        self.amount = None  # re-default for the new selection
        self._refresh()

    def _on_hover(self, tid: "str | None") -> None:
        self._hover = tid
        self._render_hover()

    def _render_hover(self) -> None:
        state = self.state
        tid = self._hover
        if tid is None or state is None:
            self.hover_label.configure(text="")
            return
        territory = self.board.territories[tid]
        continent = self.board.continents[territory.continent].name
        owner = self._name(state.owner[tid])
        armies = state.armies[tid]
        self.hover_label.configure(
            text=f"{territory.name} · {continent} · {owner} · "
                 f"{armies} {'army' if armies == 1 else 'armies'}")

    def _step(self, delta: int) -> None:
        low, high = self.amount_range
        self.amount = max(low, min(high, (self.amount or low) + delta))
        self._refresh()

    def _press(self, which: str) -> None:
        action = self._actions.get(which)
        if action is None:
            return
        if action == "close":
            self.close(confirm=False)
            return
        if action == "reconnect":
            if self.session.reconnect():
                self.notice = "Reconnecting\u2026"
                self._refresh()
            return
        self.notice = ""
        self.session.send(action)

    def _trade(self) -> None:
        """Trade in a set: the rules pick the best one in the hand (fewest
        wilds), so there is nothing to choose."""
        state = self.state
        if not self._my_turn() or state.phase != rules.PHASE_REINFORCE:
            return
        trio = rules.find_set(state.current_player["cards"])
        if trio is not None:
            self.session.send({"type": "trade", "cards": trio})

    # -- refresh ---------------------------------------------------------------------
    def _set_amount(self, low: int, high: int, default: int, caption: str) -> None:
        self.amount_range = (low, max(low, high))
        if self.amount is None or not low <= self.amount <= high:
            self.amount = max(low, min(high, default))
        self.amount_label.configure(text=str(self.amount))
        self.amount_caption.configure(text=caption)
        self.minus.set_enabled(self.amount > low)
        self.plus.set_enabled(self.amount < high)
        self._show_amount = True

    def _hide_amount(self) -> None:
        self._show_amount = False

    def _set_button(self, key: str, button: PillButton, text: "str | None",
                    action=None, enabled: bool = True) -> None:
        self._visible[key] = text is not None
        if text is None:
            self._actions[key] = None
            return
        button.set_text(text)
        button.set_enabled(enabled and action is not None)
        self._actions[key] = action if enabled else None

    def _layout_controls(self) -> None:
        """Repack the control column in its fixed order; rows come and go
        with the phase, and pack would otherwise append them at the end."""
        rows = [(self.stepper, self._show_amount, (0, self._px(2))),
                (self.amount_caption, self._show_amount, (0, 0)),
                (self.primary, self._visible.get("primary"), (self._px(8), 0)),
                (self.secondary, self._visible.get("secondary"), (self._px(6), 0)),
                (self.tertiary, self._visible.get("tertiary"), (self._px(6), 0))]
        key = tuple(bool(shown) for _widget, shown, _pady in rows)
        if key == self._layout_key:
            return
        self._layout_key = key
        for widget, _shown, _pady in rows:
            widget.pack_forget()
        for widget, shown, pady in rows:
            if shown:
                widget.pack(pady=pady)

    def _refresh(self) -> None:
        self._refresh_content()
        self._show_concede(self._playing() and self.session.reconnecting is None)
        self._layout_controls()

    def _refresh_content(self) -> None:
        state = self.state
        targets: set[str] = set()
        theme = self.theme
        if state is None:
            self.banner.show([("Waiting for the game to start\u2026", theme["TITLE_FG"])],
                             theme["HOVER_BG"], theme["CARD_BG"])
            self.phases.show(None, theme["HOVER_BG"], theme)
            self.instruction.configure(text="")
            self._hide_amount()
            for key, button in (("primary", self.primary), ("secondary", self.secondary),
                                ("tertiary", self.tertiary)):
                self._set_button(key, button, None)
            self.view.render(None)
            return
        me = self.session.my_id
        current = state.current_player
        phase = state.phase
        self._render_players()
        self._render_cards()
        self._render_hover()
        self._render_log()

        if self.session.closed and phase != rules.PHASE_OVER and self.session.can_reconnect:
            self.banner.show([("Disconnected", theme["TITLE_FG"])], theme["HOVER_BG"],
                             theme["CARD_BG"])
            self.phases.show(None, theme["HOVER_BG"], theme)
            self._set_window_title("Disconnected")
            self.view.set_frame(None)
            self.instruction.configure(text=self.notice or "Lost the connection to the host.")
            self._hide_amount()
            self._set_button("primary", self.primary, "Reconnect", "reconnect")
            self._set_button("secondary", self.secondary, "Leave", "close")
            self._set_button("tertiary", self.tertiary, None)
            self.view.render(state)
            return

        if phase == rules.PHASE_OVER or self.session.closed:
            winner = state.winner
            if phase == rules.PHASE_OVER and winner == me:
                self.banner.show([("You win!", theme["TODAY_FG"])], theme["ACCENT"],
                                 theme["CARD_BG"])
            elif phase == rules.PHASE_OVER and winner is not None:
                color = self._player_color(winner)
                self.banner.show([(self._name(winner), color), (" wins!", theme["TITLE_FG"])],
                                 blend(color, theme["CARD_BG"], 0.86), theme["CARD_BG"])
            else:
                self.banner.show([("Game over" if phase == rules.PHASE_OVER else "Game ended",
                                   theme["TITLE_FG"])], theme["HOVER_BG"], theme["CARD_BG"])
            self.phases.show(None, theme["HOVER_BG"], theme)
            self._set_window_title("Game over")
            self.view.set_frame(None)
            self.instruction.configure(text=self.notice or "Close the window to leave.")
            self._hide_amount()
            self._set_button("primary", self.primary, "Close", "close")
            self._set_button("secondary", self.secondary, None)
            self._set_button("tertiary", self.tertiary, None)
            self.view.render(state)
            return

        if self.session.reconnecting is not None:
            self.banner.show([("Reconnecting\u2026", theme["TITLE_FG"])], theme["HOVER_BG"],
                             theme["CARD_BG"])
            self.phases.show(None, theme["HOVER_BG"], theme)
            self._set_window_title("Reconnecting")
            self.view.set_frame(None)
            self.effects.clear("aim")
            self.instruction.configure(text=self.notice)
            self._hide_amount()
            self._set_button("primary", self.primary, "Waiting\u2026", None, enabled=False)
            self._set_button("secondary", self.secondary, None)
            self._set_button("tertiary", self.tertiary, None)
            self.view.render(state)
            return

        if current["id"] != me:
            color = self._player_color(current["id"])
            self.banner.show([(f"{current['name']}\u2019s turn", text_on(color))],
                             color, theme["CARD_BG"])
            self.phases.show(phase, color, theme)
            self._set_window_title(f"{current['name']}\u2019s turn")
            self.view.set_frame(color, self.FRAME_PX)
            self.effects.clear("aim")
            if self._playing():
                waiting = f"Waiting for {current['name']}…"
            else:
                waiting = f"You are watching \u00b7 {current['name']} is playing."
            self.instruction.configure(text=self.notice or waiting)
            self._hide_amount()
            self._set_button("primary", self.primary, "Waiting…", None, enabled=False)
            self._set_button("secondary", self.secondary, None)
            self._set_button("tertiary", self.tertiary, None)
            self.view.render(state)
            return

        mine = self._player_color(me)
        self.banner.show([("Your turn", text_on(mine))], mine, theme["CARD_BG"])
        self.phases.show(phase, mine, theme)
        self._set_window_title(f"Your turn \u00b7 {PHASE_NAMES[phase]}")
        self.view.set_frame(mine, self.MY_FRAME_PX)
        selected, target = self.selected, self.target
        text = ""
        primary = secondary = tertiary = (None, None)
        show_amount = False
        if phase == rules.PHASE_REINFORCE:
            if rules.must_trade(state):
                text = "You hold five or more cards: trade a set first."
            elif selected:
                show_amount = True
                self._set_amount(1, state.reinforcements, state.reinforcements,
                                 f"of {state.reinforcements} armies")
                text = f"Place armies on {self._territory_name(selected)}, or pick another territory."
                # The instruction above names the territory in full; the
                # button column is too narrow for the longer names.
                primary = ("Place", {"type": "place", "t": selected, "n": self.amount})
                if state.reinforcements > 1:
                    secondary = (f"Place all ({state.reinforcements})",
                                 {"type": "place", "t": selected, "n": state.reinforcements})
            else:
                text = (f"Place {state.reinforcements} "
                        f"{'army' if state.reinforcements == 1 else 'armies'}: "
                        "click one of your territories.")
        elif phase == rules.PHASE_ATTACK:
            tertiary = ("End attack", {"type": "end_attack"})
            if selected:
                targets = rules.attack_targets(self.board, state, selected)
            if selected and target:
                top = rules.attack_dice_max(state, selected)
                show_amount = True
                self._set_amount(1, top, top, "attacking dice")
                text = (f"{self._territory_name(selected)} ({state.armies[selected]}) attacks "
                        f"{self._territory_name(target)} ({state.armies[target]}).")
                primary = ("Attack", {"type": "attack", "from": selected, "to": target,
                                      "dice": self.amount})
                secondary = ("Blitz", {"type": "blitz", "from": selected, "to": target})
                targets = {target}
            elif selected:
                text = (f"Attack from {self._territory_name(selected)}: click a "
                        "highlighted neighbour." if targets else
                        f"{self._territory_name(selected)} has no enemy neighbours.")
            else:
                text = "Click one of your territories with two or more armies to attack from."
        elif phase == rules.PHASE_OCCUPY:
            step = state.occupy
            show_amount = True
            self._set_amount(step["min"], step["max"], step["max"], "armies to move in")
            text = (f"You conquered {self._territory_name(step['to'])}! Choose how many "
                    "armies move in.")
            primary = ("Move in", {"type": "occupy", "n": self.amount})
            if step["max"] > step["min"]:
                secondary = (f"Move all ({step['max']})", {"type": "occupy", "n": step["max"]})
            selected, targets = step["from"], {step["to"]}
        elif phase == rules.PHASE_FORTIFY:
            tertiary = ("End turn", {"type": "end_turn"})
            if selected:
                targets = rules.reachable(self.board, state, selected)
            if selected and target:
                show_amount = True
                top = state.armies[selected] - 1
                self._set_amount(1, top, top, "armies to move")
                text = (f"Move armies from {self._territory_name(selected)} to "
                        f"{self._territory_name(target)}; that ends your turn.")
                primary = ("Fortify", {"type": "fortify", "from": selected, "to": target,
                                       "n": self.amount})
                if top > 1:
                    secondary = (f"Move all ({top})", {"type": "fortify", "from": selected,
                                                        "to": target, "n": top})
                targets = {target}
            elif selected:
                text = "Click a highlighted territory to move armies to."
            else:
                text = ("Optionally move armies once along your own territories, "
                        "then end your turn.")
        if not show_amount:
            self._hide_amount()
        self.instruction.configure(text=self.notice or text)
        self._set_button("primary", self.primary, primary[0] or "Choose a territory",
                         primary[1], enabled=primary[1] is not None)
        self._set_button("secondary", self.secondary, secondary[0], secondary[1])
        self._set_button("tertiary", self.tertiary, tertiary[0], tertiary[1])
        self.view.render(state, selected, targets)
        if selected and target and phase in (rules.PHASE_ATTACK, rules.PHASE_FORTIFY):
            self.effects.arrow(selected, target, self._player_color(me), key="aim")
        else:
            self.effects.clear("aim")

    def _card_label(self, card: dict) -> str:
        if card["s"] == rules.WILD:
            return "Wild"
        return self._territory_name(card["t"])

    def _set_window_title(self, status: str) -> None:
        title = f"{APP_NAME} \u2013 {self.TITLE} \u00b7 {status}"
        if self.top.title() != title:
            self.top.title(title)

    def _render_players(self) -> None:
        """One row per player; the current player's row is tinted in their
        colour. With chat on, clicking someone else's name mutes them."""
        state = self.state
        theme = self.theme
        session = self.session
        owned = Counter(state.owner.values())
        armies = Counter()
        for tid, owner in state.owner.items():
            armies[owner] += state.armies[tid]
        chat_on = session.chat_state == "secure"
        away = {p.get("id") for p in (session.lobby or {}).get("players", ())
                if p.get("away")}
        rows = []
        for player in state.players:
            pid = player["id"]
            current = pid == state.current_player["id"] and state.phase != rules.PHASE_OVER
            name = player["name"] + (" (you)" if pid == session.my_id else "")
            if player["left"]:
                name += " \u00b7 left"
            elif player.get("conceded"):
                name += " \u00b7 gave up"
            elif not player["alive"]:
                name += " \u00b7 out"
            elif pid in away and state.phase != rules.PHASE_OVER:
                name += " \u00b7 reconnecting\u2026"
            if pid in session.muted:
                name += " \u00b7 muted"
            stats = f"{owned[pid]} terr \u00b7 {armies[pid]} armies"
            if not player["neutral"]:
                stats += f" \u00b7 {player.get('card_count', len(player['cards']))} cards"
            mutable = chat_on and pid != session.my_id and not player["neutral"]
            rows.append((pid, player["color"], ("\u25b8 " if current else "") + name, current,
                         player["alive"] and not player["left"], stats, mutable))
        key = tuple(rows)
        if key == self._players_key:
            return
        self._players_key = key

        for child in self.players_frame.winfo_children():
            child.destroy()
        size = self._px(12)
        for pid, color, name, current, active, stats, mutable in rows:
            fill = player_color(theme, color)
            bg = blend(fill, theme["CARD_BG"], 0.72) if current else theme["CARD_BG"]
            row = tk.Frame(self.players_frame, bg=bg, padx=self._px(6), pady=self._px(2))
            row.pack(fill="x", pady=(0, self._px(1)))
            dot = tk.Canvas(row, width=size, height=size, bd=0, highlightthickness=0, bg=bg)
            dot.create_oval(1, 1, size - 1, size - 1, outline=theme["CARD_BORDER"], fill=fill)
            dot.pack(side="left", padx=(0, self._px(8)))
            label = tk.Label(row, text=name, font=self._font(11, bold=current), anchor="w",
                             bg=bg, fg=theme["TITLE_FG"] if active else theme["WEEK_NUM_FG"])
            label.pack(side="left")
            tk.Label(row, text=stats, font=self._font(10), anchor="e", bg=bg,
                     fg=theme["YEAR_FG"]).pack(side="right", padx=(self._px(12), 0))
            if mutable:
                label.configure(cursor="hand2")
                label.bind("<Button-1>", lambda e, p=pid: self._toggle_mute(p))

    def _toggle_mute(self, pid: int) -> None:
        self.session.toggle_mute(pid)
        self._players_key = None
        self._refresh()

    def _render_cards(self) -> None:
        state = self.state
        me = self.session.my_id
        try:
            mine = state.player(me)
        except rules.IllegalAction:
            mine = None
        cards = mine["cards"] if mine else []
        next_value = rules.trade_value(state.trade_count)
        self.cards_label.configure(
            text=f"Next set is worth {next_value} armies" if cards else
            f"No cards yet \u2014 conquer a territory to earn one\nNext set is worth {next_value} armies")
        trio = (rules.find_set(cards) if self._my_turn()
                and state.phase == rules.PHASE_REINFORCE else None)
        tradeable = trio is not None
        # The set a trade will use is shown lifted.
        lifted = set(trio or ())
        self.hand.show([(c["s"], self._card_label(c)) for c in cards], lifted)
        self.trade.set_text(f"Trade a set (+{next_value})")
        self.trade.set_enabled(tradeable)
        # A required trade is the next step: draw it as the main action, and
        # pulse it once a turn so it catches the eye.
        must = tradeable and rules.must_trade(state)
        self.trade.set_variant("primary" if must else "secondary")
        if must and self._trade_pulsed != state.turn and not self._reduce_motion:
            self._trade_pulsed = state.turn
            self.trade.pulse()

    # -- window ----------------------------------------------------------------------
    def _on_close(self) -> None:
        self.effects.cancel()
        sound.release()
        self.session.close()
        drop_map(self)
        network.deny()  # Timezone Dominator is closed: no connections until it is allowed again

    def close(self, confirm: bool = True) -> None:
        """Leave the game. While this player is still in it (or the host,
        whose leaving ends it for everyone), ask first."""
        state = self.state
        running = (state is not None and state.phase != rules.PHASE_OVER
                   and not self.session.closed)
        if confirm and running and (self._playing() or self.session.is_host):
            detail = ("Everyone's game ends when the host leaves."
                      if self.session.is_host else
                      "Your armies stay on the board for the others to conquer.")
            ConfirmDialog.ask(self.top, self.settings, parent=self.top, title="Timezone Dominator",
                              heading="Leave the game?", message=detail,
                              confirm="Leave", cancel="Stay",
                              on_result=lambda leave: leave and self.close(confirm=False))
            return
        super().close()
