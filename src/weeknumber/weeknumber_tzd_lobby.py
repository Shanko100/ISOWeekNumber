"""Timezone Dominator lobby for WeekNumber: play online, or host or join on the local network.

TzdSession is the one object the UI talks to, whether this copy hosts or
joins. Network callbacks arrive on socket threads and are marshalled onto
Tk's thread through the app's run_on_ui -- the same pattern the tray uses --
so every Tk object is only ever touched on the main thread.

The dialog follows the other WeekNumber dialogs: a titled, centred window in
the active palette with hand-drawn controls, so it looks the same on Windows
and macOS.

Nothing connects until the player has agreed to what playing with others
connects to, in a notice shown every time Timezone Dominator opens (network_notice; the
tray asks it before the lobby opens). Until then, and again once the Timezone Dominator
windows close, the process refuses every Internet connection outright
(weeknumber_core.NetworkGate). IT policies can switch off online or LAN
play, or the whole game (see docs/ADMINISTRATION.md).
"""

from __future__ import annotations

import secrets
import threading
from collections import deque
import tkinter as tk
from typing import TYPE_CHECKING

import weeknumber_tzd_chat as chat
import weeknumber_tzd_net as net
import weeknumber_tzd_relay as relay
import weeknumber_tzd_rules as rules
from weeknumber_core import (
    SettingsStore,
    log,
    network,
    policy,
    show_error,
)
from weeknumber_dialog import TitledDialog
from weeknumber_tzd_board import player_color
from weeknumber_tzd_chat_view import ChatPanel
from weeknumber_widgets import PillButton

if TYPE_CHECKING:
    from collections.abc import Callable

    from weeknumber_timezone_dialog import TimeZoneDialog


class TzdSession:
    """A lobby and, once started, a game -- hosted here or joined remotely.

    Listeners get (kind, data) on the Tk thread: "lobby" (dict), "state"
    ({"state": GameState, "events": [...]}), "welcome" (player id), "error"
    (message), "closed" (reason), "secure" (chat_state changed), "chat"
    (a message for chat_log, the player's own ones included), and for a
    player whose connection dropped mid-game "reconnecting" ({"attempt",
    "seconds_left"}) and "resumed" (player id). After a "closed" that
    `can_reconnect`, `reconnect()` tries again.

    Chat history lives here, not in a window, so it carries over from the
    lobby into the game. It is bounded and never written anywhere."""

    CHAT_HISTORY = 200

    def __init__(self, board: rules.Board,
                 run_on_ui: "Callable[[Callable[[], None]], None]") -> None:
        self.board = board
        self._run_on_ui = run_on_ui
        self._listeners: list = []
        self.host: "net.LobbyHost | None" = None
        self.client: "net.LobbyClient | None" = None
        self.my_id: "int | None" = None
        self.lobby: "dict | None" = None
        self.state: "rules.GameState | None" = None
        self.closed = False
        #: "pending" until the host's welcome, then "secure", "failed" (a key
        #: did not check out) or "unavailable" (no chat in this lobby).
        self.chat_state = "pending"
        self.safety_code: "str | None" = None
        self.chat_log: "deque[dict]" = deque(maxlen=self.CHAT_HISTORY)
        #: Players whose messages this player chose not to see.
        self.muted: set[int] = set()
        #: Whether the chat shows the safety code instead of the messages;
        #: kept here so it survives the lobby's rebuilds.
        self.code_shown = False
        #: While the connection to the host is being restored, the latest
        #: {"attempt", "seconds_left"}; None while connected.
        self.reconnecting: "dict | None" = None

    @property
    def is_host(self) -> bool:
        return self.host is not None

    def subscribe(self, listener) -> None:
        if listener not in self._listeners:
            self._listeners.append(listener)

    def unsubscribe(self, listener) -> None:
        if listener in self._listeners:
            self._listeners.remove(listener)

    def _from_network(self, kind: str, data: object) -> None:
        self._run_on_ui(lambda: self._deliver(kind, data))

    def _deliver(self, kind: str, data: object) -> None:
        if kind == "state" and isinstance(data, dict) and isinstance(data.get("state"), dict):
            try:
                data = {"state": rules.GameState.from_dict(data["state"], self.board),
                        "events": data.get("events", [])}
            except ValueError:
                log.exception("Host produced an invalid state view")
                return
        if kind == "lobby":
            self.lobby = data
        elif kind == "state":
            self.state = data["state"]
        elif kind == "welcome":
            self.my_id = data
        elif kind == "secure":
            self.chat_state = data["state"]
            self.safety_code = data["code"]
        elif kind == "chat":
            if data["from"] in self.muted:
                return
            self.chat_log.append(data)
        elif kind == "reconnecting":
            self.reconnecting = data
        elif kind == "resumed":
            self.reconnecting = None
        elif kind == "closed":
            if self.closed:
                return
            self.closed = True
            self.reconnecting = None
        for listener in list(self._listeners):
            try:
                listener(kind, data)
            except Exception:
                log.exception("Timezone Dominator listener failed for %s", kind)

    # -- commands -------------------------------------------------------------
    def open_host(self, name: str, lobby_name: str, online: bool) -> net.LobbyHost:
        """Create and start a host. Safe off the Tk thread, and online it
        should be: listing the lobby is an HTTPS request plus a WebSocket
        handshake. Raises OSError (RelayError online) when it cannot start."""
        transport = relay.RelayHostTransport() if online else None
        host = net.LobbyHost(self.board, name, lobby_name, self._from_network,
                             transport=transport)
        host.start()
        return host

    def adopt_host(self, host: net.LobbyHost) -> None:
        """Make a started host this session's (Tk thread)."""
        self.host = host
        self.my_id = net.LobbyHost.HOST_ID
        self.chat_state = "secure" if host.chat_on else "unavailable"
        self.safety_code = host.safety_code
        self._deliver("lobby", host.lobby())

    def start_hosting(self, name: str, lobby_name: str) -> None:
        """LAN hosting; raises OSError when the listener cannot be opened."""
        self.adopt_host(self.open_host(name, lobby_name, online=False))

    def join(self, address: str, port: int, name: str, code: str) -> None:
        self.client = net.LobbyClient(self.board, address, port, name, code,
                                      self._from_network)
        self.client.start()

    def join_online(self, lobby_id: str, name: str) -> None:
        self.client = net.LobbyClient(self.board, "", 0, name, "", self._from_network,
                                      connector=lambda: relay.connect_guest(lobby_id))
        self.client.start()

    def set_color(self, color: int) -> None:
        if self.host is not None:
            if not self.host.set_color(self.my_id, color):
                self._deliver("error", "That colour is taken.")
        elif self.client is not None:
            self.client.set_color(color)

    def start_game(self) -> None:
        if self.host is None:
            return
        try:
            self.host.start_game()
        except rules.IllegalAction as exc:
            self._deliver("error", str(exc))

    def send(self, action: dict) -> None:
        if self.host is not None:
            try:
                self.host.submit(self.my_id, action)
            except rules.IllegalAction as exc:
                self._deliver("error", str(exc))
        elif self.client is not None:
            self.client.send_action(action)

    def player_name(self, pid: "int | None") -> str:
        players = (self.lobby or {}).get("players", [])
        return next((p["name"] for p in players if p["id"] == pid), "Player")

    def send_chat(self, text: str) -> "str | None":
        """Send a chat message; None once sent, otherwise why not."""
        if self.chat_state != "secure":
            return "Chat isn't available in this lobby."
        if self.host is not None:
            cleaned = chat.clean_text(text)
            if cleaned is None:
                return "There is nothing to send."
            self.host.say(cleaned)
        elif self.client is not None:
            problem = self.client.send_chat(text)
            if problem is not None:
                return problem
            cleaned = chat.clean_text(text)
        else:
            return "Not connected."
        self._deliver("chat", {"from": self.my_id, "name": self.player_name(self.my_id),
                               "text": cleaned, "mine": True})
        return None

    @property
    def can_reconnect(self) -> bool:
        """A player whose connection ended mid-game can try to return."""
        return self.client is not None and self.client.can_reconnect

    def reconnect(self) -> bool:
        """Try again to return to the game after the automatic tries gave up."""
        if not self.closed or self.client is None or not self.client.reconnect():
            return False
        self.closed = False
        return True

    def toggle_mute(self, pid: int) -> None:
        if pid in self.muted:
            self.muted.discard(pid)
        elif pid != self.my_id:
            self.muted.add(pid)

    def close(self) -> None:
        if self.host is not None:
            self.host.stop()
        if self.client is not None:
            self.client.close()
        self._listeners.clear()
        self.closed = True


def network_needed() -> bool:
    """Whether Timezone Dominator can connect at all under the IT policy; without that
    the lobby only says so, and there is nothing to agree to."""
    return relay.online_available() or net.lan_available()


def network_notice() -> dict:
    """The text of the notice shown before Timezone Dominator may use the network:
    keyword arguments for weeknumber_dialog.ConfirmDialog.ask."""
    details = []
    url = relay.relay_url()
    if url is not None:
        public = url.rstrip("/") == relay.RELAY_URL.rstrip("/")
        operator = ("WeekNumber's public relay on Cloudflare" if public
                    else f"your organisation's relay at {url.split('://', 1)[-1]}")
        details.append(("Online — an Internet connection",
                        f"Connects over HTTPS to {operator}. It sees lobby and player "
                        "names, your moves and your IP address (kept only as a "
                        "short-lived hash). Chat is end-to-end encrypted."))
    if net.lan_available():
        details.append(("Local network",
                        f"Hosting opens TCP {net.TCP_PORT} and UDP {net.DISCOVERY_PORT} "
                        "on this computer; searching sends one broadcast. Your firewall "
                        "may ask first."))
    return {
        "title": "Timezone Dominator",
        "heading": "Timezone Dominator wants to connect",
        "message": ("Playing with others needs a network connection. WeekNumber "
                    "connects only while Timezone Dominator is open and only if you allow it now; "
                    "otherwise it never connects. You will be asked again every time."),
        "details": details,
        "confirm": "Allow and play",
        "cancel": "Don’t connect",
    }


def default_player_name() -> str:
    """A neutral starting name. Online lobbies list player names publicly,
    so the default must not reveal the OS account name."""
    return f"Player {secrets.randbelow(9000) + 1000}"


def default_lobby_name(player_name: str) -> str:
    return f"{net.clean_name(player_name)}'s game"


def follow_lobby_name(lobby_name: str, old_player: str, new_player: str) -> str:
    """The lobby name after the player renames themselves: while it is still
    the default for the old name (or empty) it follows the new one; a name
    the player typed stays."""
    if lobby_name.strip() in ("", default_lobby_name(old_player)):
        return default_lobby_name(new_player)
    return lobby_name


def parse_address(text: str) -> "tuple[str, int] | None":
    """'host', 'host:port' or '[v6]:port' -> (host, port)."""
    text = text.strip()
    if not text or len(text) > 255:
        return None
    host, port = text, net.TCP_PORT
    if text.startswith("["):
        end = text.find("]")
        if end < 0:
            return None
        host, rest = text[1:end], text[end + 1:]
        if rest.startswith(":"):
            port_text = rest[1:]
        elif rest:
            return None
        else:
            port_text = ""
    elif text.count(":") == 1:
        host, port_text = text.split(":")
    else:
        port_text = ""
    if port_text:
        if not port_text.isdigit() or not 0 < int(port_text) < 65536:
            return None
        port = int(port_text)
    return (host, port) if host else None


class TzdLobbyDialog(TitledDialog):
    """Single-instance lobby window. Opens the game window once the host
    starts the game, handing it the live session."""

    TITLE = "Timezone Dominator"
    WIDTH_PX = 470
    MARGIN_PX = 24
    #: The online status until the relay answers.
    CONNECTING = "Connecting\u2026"

    @classmethod
    def show(cls, master: tk.Misc, settings: SettingsStore, run_on_ui,
             on_game_started: "Callable[[TzdSession], None]",
             backdrop: "TimeZoneDialog | None" = None) -> None:
        if policy().disable_game or cls.raise_open():
            return
        board = rules.load_board()
        if board is None:
            show_error("The Timezone Dominator board data is missing or damaged; "
                       "see the log for details.")
            return
        super().show(master, settings, board, run_on_ui, on_game_started, backdrop)

    def __init__(self, master: tk.Misc, settings: SettingsStore, board: rules.Board,
                 run_on_ui, on_game_started,
                 backdrop: "TimeZoneDialog | None" = None) -> None:
        work = self._init_window(master, settings)
        #: The time-zone window whose map turned into the board; the lobby
        #: sits above it and tells it when Timezone Dominator ends (see _on_close).
        self._backdrop = backdrop
        self.board = board
        self._run_on_ui = run_on_ui
        self._on_game_started = on_game_started
        self.session: "TzdSession | None" = None
        # online | host | join, until a lobby is open; None when the IT
        # policy allows neither online nor LAN play.
        self.mode = ("online" if relay.online_available()
                     else "host" if net.lan_available() else None)
        #: The lobby only opens after the network notice was accepted.
        self._consented = network.is_open
        self._handed_over = False
        self._lobbies: list[dict] = []
        self._scanning = False
        self._browser: "relay.LobbyBrowser | None" = None
        self._online_lobbies: list[dict] = []
        self._online_status: "str | None" = self.CONNECTING
        self._selected_online: "str | None" = None
        self._creating = False
        self.name_var = tk.StringVar(master, value=default_player_name())
        self.lobby_var = tk.StringVar(master, value=default_lobby_name(self.name_var.get()))
        self._player_name = self.name_var.get()
        # Removed in _on_close: Tk holds the callback, which holds this window.
        self._name_trace = self.name_var.trace_add("write", lambda *_: self._follow_name())
        self.address_var = tk.StringVar(master)
        self.code_var = tk.StringVar(master)
        self.chat_var = tk.StringVar(master)
        self._chat_panel: "ChatPanel | None" = None

        margin = self._px(self.MARGIN_PX)
        self.card = tk.Frame(self.top, padx=margin, pady=self._px(22))
        self.card.pack(fill="both", expand=True)
        self.body: "tk.Frame | None" = None
        self._entries: dict[str, tk.Entry] = {}
        self.status = ""
        self._render()
        self._place(work)
        self.top.deiconify()
        self._raise()
        if backdrop is not None:
            backdrop.keep_above(self.top)
        if self._consented and self.mode == "online":
            self._start_browser()

    # -- widgets ------------------------------------------------------------------
    def _label(self, parent, text, size=11, bold=False, muted=False):
        label = tk.Label(parent, text=text, font=self._font(size, bold), anchor="w",
                         justify="left", bg=self.theme["CARD_BG"],
                         fg=self.theme["YEAR_FG" if muted else "TITLE_FG"],
                         wraplength=self._px(self.WIDTH_PX - 2 * self.MARGIN_PX))
        return label

    def _entry(self, parent, variable, width=1):
        """A text field; the default width=1 lets it take the space it is
        packed into, so a full-length name always shows."""
        theme = self.theme
        entry = tk.Entry(parent, textvariable=variable, width=width, font=self._font(11),
                        relief="flat", bd=self._px(4), bg=theme["INPUT_BG"],
                        fg=theme["TITLE_FG"], insertbackground=theme["TITLE_FG"],
                        disabledbackground=theme["CARD_BG"],
                        disabledforeground=theme["YEAR_FG"],
                        highlightthickness=1, highlightbackground=theme["INPUT_BORDER"],
                        highlightcolor=theme["ACCENT"])
        self._entries[str(variable)] = entry
        return entry

    def _button(self, parent, text, command, primary=True):
        button = PillButton(parent, text, self._font(11, bold=True), self.scale,
                            command, primary=primary)
        button.apply_theme(self.theme)
        return button

    def _row(self, parent, pady=(0, 0)):
        row = tk.Frame(parent, bg=self.theme["CARD_BG"])
        row.pack(fill="x", pady=pady)
        return row

    # -- rendering ------------------------------------------------------------------
    def _render(self) -> None:
        """Rebuild the window body for the current step. The body is small and
        steps change rarely, so rebuilding beats keeping five layouts in sync;
        entry text lives in StringVars and survives it."""
        theme = self.theme
        # Keep the caret where the user was typing across the rebuild.
        try:
            current = self.top.focus_get()
        except (KeyError, tk.TclError):  # Tk raises KeyError for some popdowns
            current = None
        focused = next((name for name, entry in self._entries.items()
                        if current is entry), None)
        self._entries = {}
        self._chat_panel = None
        self.top.configure(bg=theme["CARD_BG"])
        self.card.configure(bg=theme["CARD_BG"])
        if self.body is not None:
            self.body.destroy()
        self.body = tk.Frame(self.card, bg=theme["CARD_BG"])
        self.body.pack(fill="both", expand=True)
        body = self.body
        gap = self._px(14)
        # Fixes the width, so steps with less text do not shrink the window.
        tk.Frame(body, width=self._px(self.WIDTH_PX - 2 * self.MARGIN_PX), height=0,
                 bg=theme["CARD_BG"]).pack()

        self._label(body, "Play Timezone Dominator", 19, bold=True).pack(fill="x")
        session = self.session
        if session is None and self.mode is None:
            self._render_policy_off(body, gap)
        elif session is None and not self._consented:
            self._render_not_allowed(body, gap)
        elif session is None:
            self._label(body, "Conquer the world map with other WeekNumber users, "
                              "online or on your local network.",
                        muted=True).pack(fill="x", pady=(self._px(4), gap))
            name_row = self._row(body, (0, gap))
            self._label(name_row, "Your name", bold=True).pack(side="left")
            self._entry(name_row, self.name_var).pack(
                side="right", fill="x", expand=True, padx=(self._px(12), 0))
            tabs = self._row(body, (0, gap))
            choices = ([("host", "Host on LAN"), ("join", "Join on LAN")]
                       if net.lan_available() else [])
            if relay.online_available():
                choices.insert(0, ("online", "Online"))
            for index, (mode, label) in enumerate(choices):
                self._button(tabs, label, lambda m=mode: self._set_mode(m),
                             primary=self.mode == mode).pack(
                    side="left", padx=(0 if index == 0 else self._px(8), 0))
            if self.mode == "online":
                self._render_online_setup(body, gap)
            elif self.mode == "host":
                self._render_host_setup(body, gap)
            else:
                self._render_join_setup(body, gap)
        else:
            self._render_lobby(body, gap)

        if self.status:
            self._label(body, self.status, muted=True).pack(fill="x", pady=(gap, 0))
        footer = self._row(body, (self._px(18), 0))
        self._button(footer, "Close", self.close, primary=False).pack(side="right")
        if session is not None and session.is_host:
            ready = (session.lobby is not None and
                     rules.MIN_PLAYERS <= len(session.lobby["players"]) <= rules.MAX_PLAYERS)
            start = self._button(footer, "Start game", session.start_game)
            start.set_enabled(ready)
            start.pack(side="right", padx=(0, self._px(8)))
        if focused in self._entries:
            entry = self._entries[focused]
            entry.focus_set()
            entry.icursor("end")
        # Aqua Tk does not draw an Entry or Listbox created after the window
        # was mapped until an idle pass runs; force it so rebuilt steps show.
        self._sync_titlebar()  # its idle pass also draws the rebuilt widgets

    def _listbox(self, parent, height: int) -> tk.Listbox:
        theme = self.theme
        return tk.Listbox(
            parent, height=height, font=self._font(11), activestyle="none", relief="flat",
            bg=theme["INPUT_BG"], fg=theme["TITLE_FG"], highlightthickness=1,
            highlightbackground=theme["INPUT_BORDER"], highlightcolor=theme["ACCENT"],
            selectbackground=theme["MENU_ACTIVE_BG"], selectforeground=theme["MENU_ACTIVE_FG"],
            exportselection=False)

    @staticmethod
    def _joinable(lobby: dict) -> bool:
        return (lobby["compatible"] and not lobby["started"]
                and lobby["players"] < rules.MAX_PLAYERS)

    def _render_online_setup(self, body, gap) -> None:
        header = self._row(body, (0, self._px(6)))
        self._label(header, "Public lobbies", bold=True).pack(side="left")
        status = self._online_status
        # The header has room for a word or two; a relay message is shown in
        # full under the list instead.
        state = (f"{len(self._online_lobbies)} listed \u00b7 live" if status is None
                 else status if status == self.CONNECTING else "Offline")
        self._label(header, state, 10, muted=True).pack(side="right")
        self.online_list = self._listbox(body, 5)
        self.online_list.pack(fill="x", pady=(0, self._px(8)))
        if status is not None and status != self.CONNECTING:
            self._label(body, status, 10, muted=True).pack(
                fill="x", pady=(0, self._px(8)))
        for lobby in self._online_lobbies:
            note = ""
            if lobby["started"]:
                note = "  \u00b7 in progress"
            elif not lobby["compatible"]:
                note = "  \u00b7 other version"
            elif lobby["players"] >= rules.MAX_PLAYERS:
                note = "  \u00b7 full"
            self.online_list.insert(
                "end", f"{lobby['name']} \u2014 {lobby['host']} \u00b7 "
                       f"{lobby['players']}/{rules.MAX_PLAYERS}{note}")
        if not self._online_lobbies and status is None:
            self.online_list.insert("end", "No open lobbies yet \u2014 create one below.")
        ids = [lobby["id"] for lobby in self._online_lobbies]
        if self._selected_online in ids:
            index = ids.index(self._selected_online)
            self.online_list.selection_set(index)
            self.online_list.see(index)
        self.online_list.bind("<<ListboxSelect>>", self._pick_online)
        self.online_list.bind("<Double-Button-1>", lambda e: self._join_online())
        actions = self._row(body, (0, gap))
        selected = next((l for l in self._online_lobbies
                         if l["id"] == self._selected_online), None)
        self._online_join = self._button(actions, "Join lobby", self._join_online)
        self._online_join.set_enabled(selected is not None and self._joinable(selected))
        self._online_join.pack(side="left")

        row = self._row(body, (0, self._px(8)))
        self._label(row, "New lobby", bold=True).pack(side="left")
        self._entry(row, self.lobby_var).pack(
            side="right", fill="x", expand=True, padx=(self._px(12), 0))
        create = self._row(body, (0, self._px(6)))
        button = self._button(create, "Creating\u2026" if self._creating
                              else "Create public lobby", self._host_online, primary=False)
        button.set_enabled(not self._creating)
        button.pack(side="left")
        self._label(body, "Online lobbies are public: everyone using WeekNumber can "
                          "see and join them.", 10, muted=True).pack(fill="x")

    def _render_host_setup(self, body, gap) -> None:
        row = self._row(body, (0, gap))
        self._label(row, "Lobby name", bold=True).pack(side="left")
        self._entry(row, self.lobby_var).pack(
            side="right", fill="x", expand=True, padx=(self._px(12), 0))
        self._label(body, "Opening a lobby lets other WeekNumber users on this "
                          "network find it. Nothing listens once you close it.",
                    muted=True).pack(fill="x", pady=(0, gap))
        actions = self._row(body)
        self._button(actions, "Open lobby", self._host).pack(side="left")

    def _render_join_setup(self, body, gap) -> None:
        theme = self.theme
        header = self._row(body, (0, self._px(6)))
        self._label(header, "Lobbies on your network", bold=True).pack(side="left")
        refresh = self._button(header, "Searching…" if self._scanning else "Refresh",
                               self._scan, primary=False)
        refresh.set_enabled(not self._scanning)
        refresh.pack(side="right")
        self.listbox = self._listbox(body, 4)
        self.listbox.pack(fill="x", pady=(0, gap))
        for lobby in self._lobbies:
            note = "" if lobby["compatible"] else "  (other version)"
            if lobby["started"]:
                note = "  (in progress)"
            self.listbox.insert("end", f"{lobby['name']} — {lobby['host']}, "
                                       f"{lobby['players']} player(s){note}")
        if not self._lobbies:
            self.listbox.insert("end", "No lobbies found yet." if not self._scanning
                                else "Searching…")
        self.listbox.bind("<<ListboxSelect>>", self._pick_lobby)
        self.listbox.bind("<Double-Button-1>", lambda e: self._join())

        grid = tk.Frame(body, bg=theme["CARD_BG"])
        grid.pack(fill="x")
        self._label(grid, "Host address", bold=True).grid(row=0, column=0, sticky="w")
        self._entry(grid, self.address_var).grid(row=0, column=1, sticky="ew",
                                                 padx=(self._px(12), 0),
                                                 pady=(0, self._px(8)))
        self._label(grid, "Lobby code", bold=True).grid(row=1, column=0, sticky="w")
        code = self._entry(grid, self.code_var, 8)
        code.grid(row=1, column=1, sticky="e")
        code.bind("<Return>", lambda e: self._join())
        grid.grid_columnconfigure(1, weight=1)
        actions = self._row(body, (gap, 0))
        self._button(actions, "Join", self._join).pack(side="left")

    def _render_lobby(self, body, gap) -> None:
        session = self.session
        theme = self.theme
        lobby = session.lobby
        if session.is_host and session.host.code is None:
            self._label(body, "Your lobby is listed publicly:",
                        muted=True).pack(fill="x", pady=(self._px(4), self._px(2)))
            tk.Label(body, text=session.host.lobby_name, font=self._font(22, bold=True),
                     bg=theme["CARD_BG"], fg=theme["ACCENT"], anchor="w").pack(fill="x")
            self._label(body, "Anyone using WeekNumber can join it from the Online "
                              "tab. No code needed.",
                        muted=True).pack(fill="x", pady=(0, gap))
        elif session.is_host:
            self._label(body, "Share this code with the players who join:",
                        muted=True).pack(fill="x", pady=(self._px(4), self._px(2)))
            tk.Label(body, text=session.host.code, font=self._font(30, bold=True),
                     bg=theme["CARD_BG"], fg=theme["ACCENT"], anchor="w").pack(fill="x")
            self._label(body, f"Listening on port {session.host.port}. They open the "
                              "game the same way you did and choose Join on LAN.",
                        muted=True).pack(fill="x", pady=(0, gap))
        else:
            text = ("Connecting…" if lobby is None else
                    f"Joined “{lobby['name']}”. Waiting for the host to start…")
            self._label(body, text, muted=True).pack(fill="x", pady=(self._px(4), gap))
        if lobby is None:
            return
        self._label(body, f"Players ({len(lobby['players'])}/{rules.MAX_PLAYERS})",
                    bold=True).pack(fill="x", pady=(0, self._px(6)))
        for player in lobby["players"]:
            row = self._row(body, (0, self._px(4)))
            size = self._px(16)
            swatch = tk.Canvas(row, width=size, height=size, highlightthickness=0, bd=0,
                               bg=theme["CARD_BG"])
            swatch.create_oval(1, 1, size - 1, size - 1, outline=theme["CARD_BORDER"],
                               fill=player_color(theme, -1 if player["color"] is None
                                                 else player["color"]))
            swatch.pack(side="left", padx=(0, self._px(10)))
            mine = player["id"] == session.my_id
            muted = player["id"] in session.muted
            name = player["name"] + ("  (you)" if mine else "  \u00b7 muted" if muted else "")
            label = self._label(row, name, bold=mine)
            label.pack(side="left")
            if mine:
                swatch.configure(cursor="hand2")
                swatch.bind("<Button-1>", lambda e: self._next_color())
                self._label(row, "click the dot to change colour", 10,
                            muted=True).pack(side="right")
            elif session.chat_state == "secure":
                label.configure(cursor="hand2")
                label.bind("<Button-1>", lambda e, pid=player["id"]: self._toggle_mute(pid))
        self._render_chat(body, gap)

    def _render_chat(self, body, gap) -> None:
        panel = ChatPanel(body, self.session, self.theme, self.scale, self._family,
                          self.chat_var, lines=5, on_status=self._chat_status)
        panel.pack(fill="x", pady=(gap, 0))
        self._entries[str(self.chat_var)] = panel.entry  # keeps the caret across rebuilds
        self._chat_panel = panel

    def _follow_name(self) -> None:
        name = self.name_var.get()
        self.lobby_var.set(follow_lobby_name(self.lobby_var.get(), self._player_name, name))
        self._player_name = name

    def _toggle_mute(self, pid: int) -> None:
        self.session.toggle_mute(pid)
        self._render()

    def _chat_status(self, text: str) -> None:
        if text != self.status:
            self.status = text
            self._render()

    # -- consent and policy ------------------------------------------------------------
    def _render_not_allowed(self, body, gap) -> None:
        """Only when the gate closed under an open lobby: nothing connects."""
        self._label(body, "Network access is off, so Timezone Dominator cannot connect. "
                          "Close this window and open Timezone Dominator again to allow it.",
                    muted=True).pack(fill="x", pady=(self._px(4), 0))

    def _render_policy_off(self, body, gap) -> None:
        self._label(body, "Your organisation has turned off playing with others on "
                          "this computer.", muted=True).pack(fill="x", pady=(self._px(4), 0))

    def _set_mode(self, mode: str) -> None:
        if not self._consented:
            return
        entering_join = mode == "join" and self.mode != "join"
        self.mode = mode
        self.status = ""
        if mode == "online":
            self._start_browser()
        else:
            self._stop_browser()
        self._render()
        if entering_join:
            self._scan()  # one automatic search; Refresh repeats it

    # -- online ----------------------------------------------------------------------
    def _start_browser(self) -> None:
        if self._browser is not None or not self._consented or not relay.online_available():
            return
        post = self._run_on_ui
        self._online_status = self.CONNECTING
        self._browser = relay.LobbyBrowser(
            self.board,
            lambda lobbies: post(lambda: self._online_update(lobbies)),
            lambda status: post(lambda: self._online_status_changed(status)))
        self._browser.start()

    def _stop_browser(self) -> None:
        if self._browser is not None:
            self._browser.stop()
            self._browser = None

    def _online_visible(self) -> bool:
        try:
            return (self.top.winfo_exists() and self.session is None
                    and self.mode == "online")
        except tk.TclError:
            return False

    def _online_update(self, lobbies: list[dict]) -> None:
        # The browser re-sends the list on every directory push; an
        # unchanged list must not rebuild the window under the user's cursor.
        if lobbies == self._online_lobbies and self._online_status is None:
            return
        self._online_lobbies = lobbies
        self._online_status = None
        if self._online_visible():
            self._render()

    def _online_status_changed(self, status: "str | None") -> None:
        if status == self._online_status:
            return
        self._online_status = status
        if self._online_visible():
            self._render()

    def _pick_online(self, event=None) -> None:
        selection = self.online_list.curselection()
        if selection and selection[0] < len(self._online_lobbies):
            lobby = self._online_lobbies[selection[0]]
            self._selected_online = lobby["id"]
            self._online_join.set_enabled(self._joinable(lobby))

    def _join_online(self) -> None:
        lobby = next((l for l in self._online_lobbies
                      if l["id"] == self._selected_online), None)
        if lobby is None or not self._joinable(lobby):
            return
        session = TzdSession(self.board, self._run_on_ui)
        session.subscribe(self._on_session)
        session.join_online(lobby["id"], net.clean_name(self.name_var.get()))
        self._stop_browser()
        self.session = session
        self.status = ""
        self._render()

    def _host_online(self) -> None:
        """List a public lobby. The relay round trips happen off the Tk
        thread; the result comes back through run_on_ui."""
        if self._creating:
            return
        self._creating = True
        self.status = ""
        self._render()
        session = TzdSession(self.board, self._run_on_ui)
        session.subscribe(self._on_session)
        name = net.clean_name(self.name_var.get())
        lobby_name = self.lobby_var.get() or default_lobby_name(name)

        def work() -> None:
            try:
                host = session.open_host(name, lobby_name, online=True)
            except OSError as exc:
                message = str(exc)
                self._run_on_ui(lambda: self._online_failed(message))
                return
            self._run_on_ui(lambda: self._online_hosted(session, host))

        threading.Thread(target=work, name="tzd-create", daemon=True).start()

    def _online_hosted(self, session: TzdSession, host: net.LobbyHost) -> None:
        self._creating = False
        try:
            alive = self.top.winfo_exists()
        except tk.TclError:
            alive = False
        if not alive or self.session is not None:
            host.stop()  # the window closed while the lobby was being listed
            return
        self._stop_browser()
        self.session = session
        session.adopt_host(host)
        self._render()

    def _online_failed(self, message: str) -> None:
        self._creating = False
        self.status = message
        if self._online_visible():
            self._render()

    def _host(self) -> None:
        session = TzdSession(self.board, self._run_on_ui)
        session.subscribe(self._on_session)
        name = net.clean_name(self.name_var.get())
        try:
            session.start_hosting(name, self.lobby_var.get() or default_lobby_name(name))
        except OSError as exc:
            log.exception("Could not open a Timezone Dominator lobby")
            self.status = f"Could not open a lobby: {exc.strerror or exc}"
            self._render()
            return
        self.session = session
        self.status = ""
        self._render()

    def _scan(self) -> None:
        if self._scanning or not self._consented:
            return
        self._scanning = True
        self._render()
        board = self.board

        def work() -> None:
            try:
                found = net.discover_lobbies(board)
            except OSError:
                log.exception("Timezone Dominator lobby discovery failed")
                found = []
            self._run_on_ui(lambda: self._scan_done(found))

        threading.Thread(target=work, name="tzd-scan", daemon=True).start()

    def _scan_done(self, found: list[dict]) -> None:
        self._scanning = False
        self._lobbies = found
        try:
            if self.top.winfo_exists() and self.session is None and self.mode == "join":
                self._render()
        except tk.TclError:
            pass

    def _pick_lobby(self, event=None) -> None:
        selection = self.listbox.curselection()
        if not selection or selection[0] >= len(self._lobbies):
            return
        lobby = self._lobbies[selection[0]]
        address = lobby["address"]
        if ":" in address:
            address = f"[{address}]"
        self.address_var.set(f"{address}:{lobby['port']}")

    def _join(self) -> None:
        target = parse_address(self.address_var.get())
        if target is None:
            self.status = "Pick a lobby or type the host's address (host or host:port)."
            self._render()
            return
        code = self.code_var.get().strip()
        if not (len(code) == net.CODE_DIGITS and code.isdigit()):
            self.status = f"Enter the {net.CODE_DIGITS}-digit code the host sees."
            self._render()
            return
        session = TzdSession(self.board, self._run_on_ui)
        session.subscribe(self._on_session)
        session.join(target[0], target[1], net.clean_name(self.name_var.get()), code)
        self.session = session
        self.status = ""
        self._render()

    def _next_color(self) -> None:
        session = self.session
        if session is None or session.lobby is None:
            return
        players = session.lobby["players"]
        taken = {p["color"] for p in players if p["id"] != session.my_id}
        mine = next((p["color"] for p in players if p["id"] == session.my_id), 0) or 0
        for step in range(1, rules.MAX_PLAYERS + 1):
            candidate = (mine + step) % rules.MAX_PLAYERS
            if candidate not in taken:
                session.set_color(candidate)
                return

    def _on_session(self, kind: str, data: object) -> None:
        try:
            if not self.top.winfo_exists():
                return
        except tk.TclError:
            return
        if kind == "state" and not self._handed_over:
            self._handed_over = True
            session = self.session
            session.unsubscribe(self._on_session)
            self.session = None
            self.close(keep_session=True)
            self._on_game_started(session)
            return
        if kind == "chat":
            if self._chat_panel is not None:
                self._chat_panel.append(data)
            return
        if kind == "closed":
            self.status = str(data)
            self.session = None
            if self.mode == "online":
                self._start_browser()
        elif kind == "error":
            self.status = str(data)
        self._render()

    # -- window -------------------------------------------------------------------------
    def _apply_theme(self) -> None:
        super()._apply_theme()
        self._render()

    def _place(self, work) -> None:
        self.top.update_idletasks()
        width = max(self.top.winfo_reqwidth(), self._px(self.WIDTH_PX))
        height = self.top.winfo_reqheight()
        frame = self._backdrop_frame()
        if frame is not None:  # centred over the board it was opened from
            bx, by, bw, bh = frame
            x = max(work.left, min(bx + (bw - width) // 2, work.right - width))
            y = max(work.top, min(by + (bh - height) // 2, work.bottom - height))
        else:
            x = work.left + max(0, (work.right - work.left - width) // 2)
            y = work.top + max(0, (work.bottom - work.top - height) // 3)
        self.top.geometry(f"+{int(x)}+{int(y)}")

    def is_over(self, backdrop: "TimeZoneDialog") -> bool:
        """Whether this lobby was opened over `backdrop`'s board."""
        return self._backdrop is backdrop

    def _backdrop_frame(self) -> "tuple[int, int, int, int] | None":
        if self._backdrop is None:
            return None
        top = self._backdrop.top
        try:
            if top.winfo_exists():
                return top.winfo_x(), top.winfo_y(), top.winfo_width(), top.winfo_height()
        except tk.TclError:
            pass
        return None

    def _on_close(self) -> None:
        try:
            self.name_var.trace_remove("write", self._name_trace)
        except tk.TclError:
            pass
        if self._backdrop is not None:
            # A started game replaces the board window; otherwise the board
            # turns back into the time-zone map.
            self._backdrop.end_tzd(game_started=self._handed_over)
            self._backdrop = None
        self._stop_browser()
        if self.session is not None:  # e.g. the app quitting: close the lobby
            self.session.close()
            self.session = None
        if not self._handed_over:
            # No game follows: once its goodbyes are sent, back to no
            # network at all.
            network.deny()

    def close(self, keep_session: bool = False) -> None:
        if keep_session:
            self.session = None  # handed to the game window; don't close it
        super().close()
