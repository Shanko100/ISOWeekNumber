"""Lobbies for WeekNumber's Timezone Dominator game: the host, the client and the protocol.

Together with weeknumber_tzd_relay (online lobbies through the Timezone Dominator relay)
this is the only part of WeekNumber that touches the network, and only while
the player has a lobby open. On the local network:

* Hosting binds a TCP listener (port 47842, or any free port when that one is
  taken) and a UDP discovery responder (port 47843). Both close when the
  lobby or the game window closes. Nothing listens at any other time.
* Joining sends one UDP broadcast probe to list lobbies on the local network,
  then opens one TCP connection to the chosen host.

The host is authoritative. Clients send actions; the host runs them through
weeknumber_tzd_rules, then sends every client its own redacted view of the
new state (its own cards, everyone else's card counts). Dice are rolled on
the host only.

Wire format: one UTF-8 JSON object per line, {"v": 1, "type": ...}. Lines
are capped at 64 KiB and every field is type-checked before use, with the
same distrust the app applies to its data files. A LAN joiner must present
the host's 4-digit lobby code; wrong codes are answered slowly and an address
that keeps guessing is refused for the rest of the lobby. Online lobbies are
public and open, so they have no code. Over the relay each message is one
WebSocket frame instead of one line.

The DisableLAN policy (see docs/ADMINISTRATION.md) refuses all of the above.

Chat rides on the same connections, end-to-end encrypted between each player
and the host (see weeknumber_tzd_chat); the host passes messages on.

Hardening. Nothing a peer sends can make this side's memory grow without
bound or stall the game: every message is capped at 64 KiB, every
connection's incoming messages are rate-limited (a flood disconnects the
sender), sends time out (a peer that stops reading is dropped instead of
blocking the host), pending handshakes are capped, and a LAN lobby stops
accepting codes altogether after MAX_TOTAL_FAILURES wrong ones. TCP
keep-alives notice a vanished peer within about half a minute.

Reconnecting. A player whose connection drops mid-game without saying goodbye
keeps their seat for RECONNECT_GRACE_S; their app reconnects on its own and
proves it is the same player -- with a box sealed on its end-to-end channel
when chat is secured, which the relay and the network can neither forge nor
replay, or else with a one-time token from the host -- and the host hands the
seat back. Leaving on purpose, or running out of time, removes the player as
before. Every message keeps to the relay's existing types (hello, welcome).

Threads: one accept thread and one reader thread per connection. Callbacks
(`on_event`) fire on those threads; the UI marshals them onto Tk's thread.
"""

from __future__ import annotations

import hmac
import json
import random
import secrets
import select
import socket
import threading
import time
import unicodedata
from typing import TYPE_CHECKING

import weeknumber_tzd_chat as chat
import weeknumber_tzd_rules as rules
from weeknumber_core import log, policy

if TYPE_CHECKING:
    from collections.abc import Callable

PROTOCOL_VERSION = 1
TCP_PORT = 47842
DISCOVERY_PORT = 47843
MAX_MESSAGE_BYTES = 64 * 1024
HELLO_TIMEOUT_S = 5.0
WRONG_CODE_DELAY_S = 0.6
MAX_CODE_FAILURES = 5
#: Wrong codes or failed reconnections from all addresses together, after
#: which a LAN lobby accepts no more attempts: guessing from many machines
#: gets no further than from one.
MAX_TOTAL_FAILURES = 30
MAX_PENDING = 8
DISCOVERY_PROBE = b'{"v":1,"type":"discover"}'
CODE_DIGITS = 4
#: A send that cannot finish in this time drops the connection: a peer that
#: stops reading must not hold up the game for everyone else.
SEND_TIMEOUT_S = 10.0
#: How often a waiting read checks whether its own side closed the
#: connection (see _Connection.receive).
READ_POLL_S = 0.25
#: How long a connection that said goodbye waits for the peer to close too
#: (see _Connection.finish).
GOODBYE_LINGER_S = 2.0
#: Messages a connection may send at once, and how fast that refills. Far
#: above human play and chat; a flood beyond it disconnects the sender.
PEER_BURST = 60
PEER_REFILL_S = 0.1
#: The same for what a host may send a player: state for every move of up to
#: six players, chat passed on, lobby updates.
HOST_BURST = 400
HOST_REFILL_S = 0.01
#: How long a dropped player's seat waits for them mid-game.
RECONNECT_GRACE_S = 120.0
#: How long a dropped player's app keeps trying, inside that grace.
RECONNECT_WINDOW_S = 100.0
RECONNECT_BACKOFF_S = (1, 2, 3, 5, 8, 10, 15)
#: Discovery answers per second, and lobbies one search keeps.
DISCOVERY_REPLIES_PER_S = 20
MAX_DISCOVERED = 64


class PolicyBlocked(OSError):
    """Refused by the organisation's IT policy; the message is for the player."""

    def __str__(self) -> str:
        return self.args[0] if self.args else LAN_OFF


LAN_OFF = "Your organisation has turned off playing on the local network."


def lan_available() -> bool:
    """Whether the DisableLAN policy allows local-network play."""
    return not policy().disable_lan


class ProtocolError(ValueError):
    pass


def clean_name(value: object, fallback: str = "Player") -> str:
    """Printable, single-line, bounded display name."""
    if not isinstance(value, str):
        return fallback
    text = "".join(ch for ch in value
                   if unicodedata.category(ch)[0] not in ("C", "Z") or ch == " ")
    text = " ".join(text.split())[:rules.MAX_NAME_CHARS]
    return text or fallback


def encode(message: dict) -> bytes:
    payload = dict(message)
    payload["v"] = PROTOCOL_VERSION
    data = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(data) >= MAX_MESSAGE_BYTES:
        raise ProtocolError("message too large")
    return data + b"\n"


def decode(line: bytes) -> dict:
    if len(line) > MAX_MESSAGE_BYTES:
        raise ProtocolError("message too large")
    try:
        message = json.loads(line.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProtocolError("malformed message") from exc
    if not isinstance(message, dict) or message.get("v") != PROTOCOL_VERSION:
        raise ProtocolError("unsupported protocol version")
    if not isinstance(message.get("type"), str):
        raise ProtocolError("message has no type")
    return message


def enable_keepalive(sock: socket.socket, idle_s: int = 10, interval_s: int = 3,
                     count: int = 5) -> None:
    """TCP keep-alives, so a peer that vanishes (Wi-Fi gone, laptop shut) is
    noticed within about half a minute instead of never. Best effort: an
    option the system lacks is skipped."""
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
    except OSError:
        return
    if hasattr(socket, "SIO_KEEPALIVE_VALS"):  # Windows
        try:
            sock.ioctl(socket.SIO_KEEPALIVE_VALS, (1, idle_s * 1000, interval_s * 1000))
        except (OSError, ValueError):
            pass
        return
    for name, value in (("TCP_KEEPIDLE", idle_s), ("TCP_KEEPALIVE", idle_s),
                        ("TCP_KEEPINTVL", interval_s), ("TCP_KEEPCNT", count)):
        option = getattr(socket, name, None)  # TCP_KEEPALIVE is macOS's idle time
        if option is not None:
            try:
                sock.setsockopt(socket.IPPROTO_TCP, option, value)
            except OSError:
                pass


class _Connection:
    """A framed, thread-safe JSON-lines socket. The relay's RelayPeer and
    GuestConnection (weeknumber_tzd_relay) offer the same interface.

    The socket always has SEND_TIMEOUT_S as its timeout, so a send can never
    block for good; `set_timeout` sets how long `receive` may wait for a
    message (None: as long as the connection lives)."""

    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self._buffer = b""
        self._send_lock = threading.Lock()
        self._read_timeout: "float | None" = None
        #: Set by finish(): until then the reader discards what arrives.
        self._drain_until: "float | None" = None
        self.closed = False
        enable_keepalive(sock)
        sock.settimeout(SEND_TIMEOUT_S)

    def set_timeout(self, seconds: "float | None") -> None:
        self._read_timeout = seconds

    def send(self, message: dict) -> bool:
        if self._drain_until is not None:
            return False  # said goodbye: the sending side is shut
        try:
            data = encode(message)
            with self._send_lock:
                self.sock.sendall(data)
            return True
        except (OSError, ProtocolError):
            self.close()
            return False

    def receive(self) -> "dict | None":
        """Next message, or None when the peer closed the connection or
        close() was called. Raises socket.timeout when a read timeout is set
        and passes.

        The wait is sliced into READ_POLL_S steps that check `closed`:
        macOS does not reliably wake a thread blocked on a socket when
        another thread shuts it down and closes it, so without the check a
        local close() went unnoticed until the socket's own timeout.

        After finish() nothing more is delivered: what arrives is discarded
        until the peer closes or GOODBYE_LINGER_S passes, then None."""
        deadline = (None if self._read_timeout is None
                    else time.monotonic() + self._read_timeout)
        while True:
            if self._drain_until is not None:
                self._buffer = b""
                if time.monotonic() >= self._drain_until:
                    return None
            elif b"\n" in self._buffer:
                break
            if len(self._buffer) > MAX_MESSAGE_BYTES:
                raise ProtocolError("message too large")
            if self.closed:
                return None
            try:
                wait = READ_POLL_S
                if deadline is not None:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise socket.timeout("no message in time")
                    wait = min(wait, remaining)
                if not select.select([self.sock], [], [], wait)[0]:
                    continue  # nothing yet: check `closed` and the deadline again
                chunk = self.sock.recv(16384)
            except socket.timeout:
                if self.closed:
                    return None
                if deadline is not None:
                    raise
                continue  # idle: the base timeout only bounds sends
            except (OSError, ValueError):
                return None
            if not chunk:
                return None
            self._buffer += chunk
        line, self._buffer = self._buffer.split(b"\n", 1)
        return decode(line)

    @property
    def finishing(self) -> bool:
        """Whether finish() was called and the reader is draining."""
        return self._drain_until is not None and not self.closed

    def finish(self, message: dict) -> None:
        """Send a last message -- a goodbye -- so that it arrives, and leave
        closing to the thread reading this connection.

        Closing straight after the send can lose it on Windows: a socket shut
        down or closed with unread data queued, or with data still arriving,
        is reset, and the peer's system can throw away what it has not read
        yet, the goodbye included. So only the sending side shuts down (the
        peer reads the message, then the end of the stream); this side's
        reader discards whatever still comes until the peer closes too, or
        GOODBYE_LINGER_S passes, and then closes as usual. Use it only where
        a reader thread exists; elsewhere send() and close()."""
        if self.closed or self._drain_until is not None:
            return
        if not self.send(message):
            return
        self._drain_until = time.monotonic() + GOODBYE_LINGER_S
        try:
            self.sock.shutdown(socket.SHUT_WR)
        except OSError:
            self.close()

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            self.sock.close()
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Host
# ---------------------------------------------------------------------------
class LobbyHost:
    """Owns the lobby, the seats and the one authoritative game state.

    Players arrive over the LAN listener, or -- with a `transport` -- through
    the online relay; both deliver connections with the same interface.

    Events delivered to `on_event(kind, data)`:
      "lobby"  -- seats changed (data: lobby dict)
      "state"  -- game advanced (data: {"state": GameState view, "events": [...]})
      "error"  -- something the local player should see (data: message)
      "closed" -- an online lobby lost the relay (data: reason)
      "chat"   -- a player said something (data: {"from", "name", "text"})
    """

    HOST_ID = 0
    #: Chat messages a player may send at once, and the refill interval.
    CHAT_BURST = 5
    CHAT_REFILL_S = 2.0

    def __init__(self, board: rules.Board, host_name: str, lobby_name: str,
                 on_event: "Callable[[str, object], None]",
                 port: int = TCP_PORT, discovery_port: int = DISCOVERY_PORT,
                 rng: "random.Random | None" = None, transport=None,
                 chat_keys: "chat.KeyPair | None" = None) -> None:
        self.board = board
        self.lobby_name = clean_name(lobby_name, "Timezone Dominator")
        self.on_event = on_event
        self._transport = transport
        # Online lobbies are public and open to everyone; LAN lobbies need
        # the code, since anyone on the same network could otherwise join.
        self.code = None if transport is not None else \
            "".join(secrets.choice("0123456789") for _ in range(CODE_DIGITS))
        self._port = port
        self._discovery_port = discovery_port
        self._rng = rng or random.Random(secrets.randbits(64))
        self._lock = threading.RLock()
        self._seats: list[dict] = [{"id": self.HOST_ID, "name": clean_name(host_name),
                                    "color": 0, "conn": None}]
        self._next_id = 1
        self._failures: dict[str, int] = {}
        self._failure_total = 0
        self._pending = 0
        self._discovery_bucket = chat.TokenBucket(DISCOVERY_REPLIES_PER_S,
                                                  1.0 / DISCOVERY_REPLIES_PER_S)
        self._tcp: "socket.socket | None" = None
        self._udp: "socket.socket | None" = None
        self._stopping = threading.Event()
        self.state: "rules.GameState | None" = None
        self.port = 0
        #: Chat: decided in start(), once it is known whether the relay passes it.
        self._chat_keys = chat_keys
        self.chat_on = False
        self.safety_code: "str | None" = None

    # -- lifecycle ------------------------------------------------------------
    def start(self) -> None:
        """Start accepting players. LAN: binds the listener, raising OSError
        if no TCP port is free. Online: lists the lobby on the relay,
        raising RelayError (an OSError) when it cannot."""
        if self._transport is not None:
            self._transport.start(self.lobby_name, self._seats[0]["name"], self.board,
                                  self._admit, self._transport_lost)
            self._start_chat(getattr(self._transport, "supports_chat", False))
            return
        if not lan_available():
            raise PolicyBlocked(LAN_OFF)
        self._start_chat(True)
        self._tcp = self._bind_tcp()
        self.port = self._tcp.getsockname()[1]
        threading.Thread(target=self._accept_loop, name="tzd-accept",
                         daemon=True).start()
        self._udp = self._bind_udp()
        if self._udp is not None:
            threading.Thread(target=self._discovery_loop, name="tzd-discovery",
                             daemon=True).start()
        log.info("Timezone Dominator lobby hosting on TCP %d (discovery %s)", self.port,
                 "on" if self._udp else "off")

    def _start_chat(self, carried: bool) -> None:
        """Chat needs the library, the policy's consent and a relay that
        passes chat messages (an older one would disconnect players)."""
        if not carried or not chat.available():
            return
        if self._chat_keys is None:
            self._chat_keys = chat.KeyPair.generate()
        self.safety_code = chat.safety_code(self._chat_keys.public)
        self.chat_on = True

    def _bind_tcp(self) -> socket.socket:
        for port in (self._port, 0):
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            try:
                sock.bind(("", port))
                sock.listen(MAX_PENDING)
                sock.settimeout(0.5)
                return sock
            except OSError:
                sock.close()
                if port == 0:
                    raise
        raise OSError("no port available")

    def _bind_udp(self) -> "socket.socket | None":
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            if hasattr(socket, "SO_REUSEPORT"):
                try:
                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
                except OSError:
                    pass
            sock.bind(("", self._discovery_port))
            sock.settimeout(0.5)
            return sock
        except OSError:
            log.warning("Timezone Dominator discovery port %d unavailable; manual join only",
                        self._discovery_port)
            sock.close()
            return None

    def stop(self) -> None:
        if self._stopping.is_set():
            return
        self._stopping.set()
        # The connections themselves, not the seats: a goodbye can make that
        # player's reader thread clear its seat's connection meanwhile.
        with self._lock:
            conns = [s["conn"] for s in self._seats if s["conn"] is not None]
            timers = [s.pop("timer") for s in self._seats if s.get("timer") is not None]
        for timer in timers:
            timer.cancel()
        for conn in conns:  # each player's reader closes its connection after
            conn.finish({"type": "bye", "reason": "The host closed the game."})
        if self._transport is not None:
            self._transport.stop()
        for sock in (self._tcp, self._udp):
            if sock is not None:
                try:
                    sock.close()
                except OSError:
                    pass
        log.info("Timezone Dominator lobby closed")

    # -- lobby ------------------------------------------------------------------
    def lobby(self) -> dict:
        with self._lock:
            return {
                "name": self.lobby_name,
                "code": self.code,
                "port": self.port,
                "online": self._transport is not None,
                "started": self.state is not None,
                "players": [{"id": s["id"], "name": s["name"], "color": s["color"],
                             "chat": self._chat_ready(s), "away": bool(s.get("away"))}
                            for s in self._seats],
            }

    def _broadcast_lobby(self) -> None:
        info = self.lobby()
        public = dict(info)
        public.pop("code")
        with self._lock:
            conns = [s["conn"] for s in self._seats if s["conn"] is not None]
        for conn in conns:
            conn.send({"type": "lobby", "lobby": public})
        if self._transport is not None:
            self._transport.update(len(info["players"]), info["started"])
        self.on_event("lobby", info)

    def _chat_ready(self, seat: dict) -> bool:
        if seat["id"] == self.HOST_ID:
            return self.chat_on
        link = seat.get("link")
        return link is not None and link.confirmed

    def _free_color(self) -> "int | None":
        used = {s["color"] for s in self._seats}
        return next((c for c in range(rules.MAX_PLAYERS) if c not in used), None)

    def set_color(self, pid: int, color: object) -> bool:
        with self._lock:
            if self.state is not None:
                return False
            if not isinstance(color, int) or isinstance(color, bool) \
                    or not 0 <= color < rules.MAX_PLAYERS:
                return False
            if any(s["color"] == color and s["id"] != pid for s in self._seats):
                return False
            seat = next((s for s in self._seats if s["id"] == pid), None)
            if seat is None:
                return False
            seat["color"] = color
        self._broadcast_lobby()
        return True

    # -- game -------------------------------------------------------------------
    def start_game(self) -> None:
        with self._lock:
            if self.state is not None:
                raise rules.IllegalAction("The game has already started.")
            if not rules.MIN_PLAYERS <= len(self._seats) <= rules.MAX_PLAYERS:
                raise rules.IllegalAction(
                    f"Timezone Dominator needs {rules.MIN_PLAYERS} to "
                    f"{rules.MAX_PLAYERS} players.")
            players = [(s["id"], s["name"], s["color"]) for s in self._seats]
            self.state = rules.new_game(self.board, players, self._rng)
        self._broadcast_lobby()
        self._broadcast_state([{"type": "start"},
                               {"type": "turn", "player": self.state.current_player["id"],
                                "reinforcements": self.state.reinforcements}])

    def submit(self, pid: int, action: object) -> None:
        """Apply an action for `pid`. Raises IllegalAction for the host's own
        actions so its UI can report them; clients get an error message."""
        with self._lock:
            if self.state is None:
                raise rules.IllegalAction("The game has not started.")
            events = rules.apply(self.board, self.state, pid, action, self._rng)
        self._broadcast_state(events)

    def _broadcast_state(self, events: list[dict]) -> None:
        with self._lock:
            state = self.state
            if state is None:
                return
            views = {s["id"]: state.to_dict(viewer=s["id"]) for s in self._seats}
            conns = [(s["id"], s["conn"]) for s in self._seats if s["conn"] is not None]
        for pid, conn in conns:
            conn.send({"type": "state", "state": views[pid], "events": events})
        self.on_event("state", {"state": views[self.HOST_ID], "events": events})

    # -- networking -------------------------------------------------------------
    def _accept_loop(self) -> None:
        while not self._stopping.is_set():
            try:
                sock, address = self._tcp.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self._admit(_Connection(sock), address[0])

    def _admit(self, conn, address: str) -> None:
        """Serve one arriving player -- a LAN socket or a relay peer."""
        with self._lock:
            if (self._pending >= MAX_PENDING
                    or self._failures.get(address, 0) >= MAX_CODE_FAILURES
                    or self._failure_total >= MAX_TOTAL_FAILURES):
                conn.close()
                return
            self._pending += 1
        threading.Thread(target=self._serve, args=(conn, address),
                         name="tzd-peer", daemon=True).start()

    def _transport_lost(self, reason: str) -> None:
        if not self._stopping.is_set():
            self.on_event("closed", reason)

    def _serve(self, conn, address: str) -> None:
        seat = None
        try:
            conn.set_timeout(HELLO_TIMEOUT_S)
            seat = self._handshake(conn, address)
        except (ProtocolError, socket.timeout, OSError):
            seat = None
        finally:
            with self._lock:
                self._pending -= 1
        if seat is None:
            conn.close()
            return
        conn.set_timeout(None)
        self._broadcast_lobby()
        if self.state is not None:
            self._broadcast_state([])
        inbound = chat.TokenBucket(PEER_BURST, PEER_REFILL_S)
        try:
            # Once stopping, a connection the host said goodbye on is drained
            # to its end (see _Connection.finish); any other one ends now.
            while not self._stopping.is_set() or getattr(conn, "finishing", False):
                message = conn.receive()
                if message is None:
                    break
                if not inbound.allow():
                    log.warning("Game peer %s sent too many messages; disconnecting", address)
                    seat["kicked"] = True
                    conn.send({"type": "bye", "reason": "Too many messages."})
                    break
                self._handle(seat, message)
        except ProtocolError:
            log.warning("Game peer %s sent an invalid message; disconnecting", address)
            seat["kicked"] = True
        except OSError:
            pass
        finally:
            conn.close()
            self._drop(seat, conn)

    def _handshake(self, conn: _Connection, address: str) -> "dict | None":
        hello = conn.receive()
        if hello is None or hello.get("type") != "hello":
            return None
        if hello.get("board") != self.board.fingerprint:
            conn.send({"type": "bye", "reason": "That host runs a different WeekNumber version."})
            return None
        if self.code is not None and not (isinstance(hello.get("code"), str) and
                                          hmac.compare_digest(hello["code"], self.code)):
            self._failed(conn, address, "Wrong lobby code.")
            return None
        if "resume" in hello:
            return self._resume(conn, hello, address)
        with self._lock:
            if self.state is not None:
                conn.send({"type": "bye", "reason": "That game has already started."})
                return None
            if len(self._seats) >= rules.MAX_PLAYERS:
                conn.send({"type": "bye", "reason": "That lobby is full."})
                return None
            link = self._chat_link(hello, self._next_id)
            seat = {"id": self._next_id, "name": clean_name(hello.get("name")),
                    "color": self._free_color(), "conn": conn, "link": link,
                    "key": hello.get("key") if link is not None else None,
                    # Without a secured channel, a one-time token is what
                    # proves a returning player (see _resume).
                    "token": None if link is not None else secrets.token_urlsafe(24),
                    "bucket": chat.TokenBucket(self.CHAT_BURST, self.CHAT_REFILL_S),
                    "throttled": False}
            self._next_id += 1
            self._seats.append(seat)
        welcome = {"type": "welcome", "player": seat["id"]}
        link = seat["link"]
        if link is None:
            welcome["resume_token"] = seat["token"]
            conn.send(welcome)
            return seat
        # Under the link's lock, so no chat box can be sealed before it.
        with link.send_lock:
            welcome.update({"chat": chat.PROTOCOL, "key": self._chat_keys.public_b64},
                           **link.seal("welcome", {"k": "ready"}))
            conn.send(welcome)
            link.ready = True
        return seat

    def _failed(self, conn, address: str, reason: str) -> None:
        """A wrong code or a refused reconnection: answered slowly, counted
        per address and for the whole lobby (see MAX_TOTAL_FAILURES)."""
        time.sleep(WRONG_CODE_DELAY_S)
        with self._lock:
            if address in self._failures or len(self._failures) < 4 * MAX_TOTAL_FAILURES:
                self._failures[address] = self._failures.get(address, 0) + 1
            self._failure_total += 1
        conn.send({"type": "bye", "reason": reason})

    def _resume(self, conn, hello: dict, address: str) -> "dict | None":
        """A player whose connection dropped mid-game comes back to their seat.

        Proof that it is them: with a secured channel, a "resume" box sealed
        on it (only they and the host hold its key; the rising counter makes
        an old box useless), otherwise the one-time token the host gave
        them, replaced on every return. The host answers with a box of its
        own, so the player knows it is the same host."""
        resume = hello.get("resume")
        pid = resume.get("player") if isinstance(resume, dict) else None
        gone = "Your seat in that game is no longer available."
        with self._lock:
            state = self.state
            seat = next((s for s in self._seats if s["id"] == pid), None) \
                if isinstance(pid, int) and not isinstance(pid, bool) and pid != self.HOST_ID \
                else None
            usable = (state is not None and seat is not None and not seat.get("kicked")
                      and state.phase != rules.PHASE_OVER
                      and not state.player(pid)["left"])
            proven = False
            if usable:
                link = seat.get("link")
                if link is not None:
                    box = link.open("resume", resume.get("n"), resume.get("box")) \
                        if hello.get("key") == seat.get("key") else None
                    proven = (isinstance(box, dict) and box.get("k") == "resume"
                              and box.get("player") == pid)
                else:
                    token, expected = resume.get("token"), seat.get("token")
                    proven = (isinstance(token, str) and isinstance(expected, str)
                              and hmac.compare_digest(token, expected))
            if usable and proven:
                old, seat["conn"] = seat["conn"], conn
                seat["away"] = None
                timer = seat.pop("timer", None)
                if seat.get("link") is None:
                    seat["token"] = secrets.token_urlsafe(24)
        if not usable:
            conn.send({"type": "bye", "reason": gone})
            return None
        if not proven:
            self._failed(conn, address, gone)
            return None
        if timer is not None:
            timer.cancel()
        welcome = {"type": "welcome", "player": pid, "resumed": True}
        link = seat.get("link")
        if link is not None:
            with link.send_lock:
                welcome.update(link.seal("welcome", {"k": "resumed", "player": pid}))
                conn.send(welcome)
        else:
            welcome["resume_token"] = seat["token"]
            conn.send(welcome)
        if old is not None and old is not conn:
            old.close()  # the connection the host had not noticed was dead
        log.info("Timezone Dominator player %s reconnected", pid)
        return seat

    def _chat_link(self, hello: dict, seat_id: int) -> "chat.ChatLink | None":
        if not self.chat_on or hello.get("chat") != chat.PROTOCOL:
            return None
        peer = chat.parse_public_key(hello.get("key"), own=self._chat_keys.public)
        if peer is None:
            return None
        try:
            return chat.ChatLink(self._chat_keys, peer, seat_id, "host")
        except ValueError:
            return None

    def _handle(self, seat: dict, message: dict) -> None:
        kind = message.get("type")
        if kind == "color":
            if not self.set_color(seat["id"], message.get("color")):
                seat["conn"].send({"type": "error", "message": "That colour is taken."})
        elif kind == "action":
            try:
                self.submit(seat["id"], message.get("action"))
            except rules.IllegalAction as exc:
                seat["conn"].send({"type": "error", "message": str(exc)})
        elif kind == "chat":
            self._on_chat(seat, message)
        elif kind == "bye":
            seat["leaving"] = True  # left on purpose: no seat kept for them
            conn = seat["conn"]
            if conn is not None:
                conn.close()

    # -- chat ---------------------------------------------------------------------
    def _on_chat(self, seat: dict, message: dict) -> None:
        link = seat.get("link")
        if link is None or not link.ready:
            return
        payload = link.open("chat", message.get("n"), message.get("box"))
        if payload is None:
            return
        if payload.get("k") == "ready":
            if not link.confirmed:
                link.confirmed = True
                self._broadcast_lobby()
            return
        text = chat.clean_text(payload.get("text"))
        if payload.get("k") != "say" or not link.confirmed or text is None:
            return
        if not seat["bucket"].allow():
            if not seat["throttled"]:  # one notice per burst
                seat["throttled"] = True
                seat["conn"].send({"type": "error",
                                   "message": "You're sending messages too quickly."})
            return
        seat["throttled"] = False
        self._fan_out(seat["id"], seat["name"], text)
        self.on_event("chat", {"from": seat["id"], "name": seat["name"], "text": text})

    def say(self, text: object) -> bool:
        """The host's own message, to every player whose chat is secured.
        The session shows it locally; this only sends it."""
        text = chat.clean_text(text)
        if not self.chat_on or text is None:
            return False
        self._fan_out(self.HOST_ID, self._seats[0]["name"], text)
        return True

    def _fan_out(self, sender: int, name: str, text: str) -> None:
        """Seal the message once per recipient with that player's key. The
        host stamps sender and name inside the box, so nobody on the way can
        change who said it. Never holds _lock while sending."""
        payload = {"k": "say", "from": sender, "name": name, "text": text}
        with self._lock:
            targets = [(s["conn"], s["link"]) for s in self._seats
                       if s["id"] != sender and s["conn"] is not None
                       and s.get("link") is not None and s["link"].confirmed]
        for conn, link in targets:
            with link.send_lock:
                conn.send({"type": "chat", **link.seal("chat", payload)})

    def _drop(self, seat: dict, conn) -> None:
        """`conn`, `seat`'s connection, ended. Before the game the seat goes.
        Mid-game, a player who said goodbye, was disconnected for misbehaving
        or is out of the game anyway leaves now; anyone else keeps the seat
        for RECONNECT_GRACE_S (see _resume, _expire)."""
        with self._lock:
            if seat not in self._seats or seat["conn"] is not conn:
                return  # already replaced by the player's new connection
            seat["conn"] = None
            events = []
            if self.state is None:
                self._seats.remove(seat)
            else:
                player = self.state.player(seat["id"])
                if (seat.get("leaving") or seat.get("kicked") or self._stopping.is_set()
                        or self.state.phase == rules.PHASE_OVER or not player["alive"]):
                    events = rules.remove_player(self.board, self.state, seat["id"])
                else:
                    seat["away"] = time.monotonic()
                    timer = threading.Timer(RECONNECT_GRACE_S, self._expire, (seat,))
                    timer.daemon = True
                    seat["timer"] = timer
                    timer.start()
                    log.info("Game player %s lost the connection; seat kept", seat["id"])
        if self._stopping.is_set():
            return
        self._broadcast_lobby()
        if events:
            self._broadcast_state(events)

    def _expire(self, seat: dict) -> None:
        """A dropped player did not come back in time: they leave the game."""
        with self._lock:
            if self._stopping.is_set() or seat["conn"] is not None or not seat.get("away") \
                    or self.state is None:
                return
            seat["away"] = None
            seat.pop("timer", None)
            events = rules.remove_player(self.board, self.state, seat["id"])
        self._broadcast_lobby()
        if events:
            self._broadcast_state(events)

    def _discovery_loop(self) -> None:
        while not self._stopping.is_set():
            try:
                data, address = self._udp.recvfrom(512)
            except socket.timeout:
                continue
            except OSError:
                return
            if data.strip() != DISCOVERY_PROBE or not self._discovery_bucket.allow():
                continue
            info = self.lobby()
            reply = {"type": "lobby_info", "name": info["name"], "port": self.port,
                     "host": clean_name(self._seats[0]["name"]),
                     "players": len(info["players"]), "started": info["started"],
                     "board": self.board.fingerprint}
            try:
                self._udp.sendto(encode(reply), address)
            except (OSError, ProtocolError):
                pass


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------
class LobbyClient:
    """One connection to a host. Events delivered to `on_event(kind, data)`:
    "welcome" (player id), "lobby", "state", "error" (message), "closed"
    (reason), "secure" ({"state": "secure" | "failed" | "unavailable",
    "code"}), "chat" ({"from", "name", "text"}), and around a lost
    connection mid-game "reconnecting" ({"attempt", "seconds_left"}) and
    "resumed" (player id)."""

    CONNECT_TIMEOUT_S = 5.0

    def __init__(self, board: rules.Board, address: str, port: int, name: str,
                 code: str, on_event: "Callable[[str, object], None]",
                 connector: "Callable[[], object] | None" = None,
                 chat_keys: "chat.KeyPair | None" = None) -> None:
        self.board = board
        self.address = address
        self.port = port
        #: Opens the connection; the online relay passes its own, LAN uses TCP.
        self._connector = connector or self._connect_tcp
        self._online = connector is not None
        self.name = clean_name(name)
        self.code = code.strip()
        self.on_event = on_event
        self.player_id: "int | None" = None
        self._conn: "_Connection | None" = None
        self._closing = False
        self._wake = threading.Event()
        self._thread: "threading.Thread | None" = None
        # Chat: keys are made on the connection thread (the library loads on
        # first use); the link exists once the host's welcome proved its key.
        # Both outlive a lost connection: they are how a reconnection is
        # proven (see _hello).
        self._chat_keys = chat_keys
        self._link: "chat.ChatLink | None" = None
        self._chat_bucket = chat.TokenBucket(LobbyHost.CHAT_BURST, LobbyHost.CHAT_REFILL_S)
        self.safety_code: "str | None" = None
        #: Set by the host when the channel is not secured; replaced on return.
        self._resume_token: "str | None" = None
        #: Whether the game started: only then is a seat kept to come back to.
        self._in_game = False
        #: The host said goodbye or refused us: there is nothing to return to.
        self._ended = False

    def start(self) -> None:
        self._thread = threading.Thread(target=self._run, args=(False,),
                                        name="tzd-client", daemon=True)
        self._thread.start()

    @property
    def can_reconnect(self) -> bool:
        """Whether this player has a seat to try to return to."""
        return (self._in_game and self.player_id is not None and not self._closing
                and not self._ended
                and (self._link is not None or self._resume_token is not None))

    def reconnect(self) -> bool:
        """Try again to return to the seat after giving up (the player asked).
        False when there is nothing to return to or a try is running."""
        if not self.can_reconnect or (self._thread is not None and self._thread.is_alive()):
            return False
        self._thread = threading.Thread(target=self._run, args=(True,),
                                        name="tzd-client", daemon=True)
        self._thread.start()
        return True

    # -- connection lifecycle ------------------------------------------------------
    def _run(self, resuming: bool) -> None:
        if resuming:
            outcome, reason = "lost", "Disconnected from the host."
        else:
            outcome, reason = self._session(resume=False)
        while outcome == "lost" and self.can_reconnect:
            outcome, reason = self._reconnect_loop(reason)
        if outcome == "ended":
            self._ended = True
        if not self._closing:
            self.on_event("closed", reason)

    def _reconnect_loop(self, reason: str) -> "tuple[str, str]":
        """Keep trying to get back to the seat for RECONNECT_WINDOW_S, backing
        off between tries. Returns how the last try ended."""
        deadline = time.monotonic() + RECONNECT_WINDOW_S
        attempt = 0
        while not self._closing:
            left = deadline - time.monotonic()
            if left <= 0:
                return "gave_up", reason + " Reconnecting did not work in time."
            attempt += 1
            self.on_event("reconnecting", {"attempt": attempt, "seconds_left": int(left)})
            delay = RECONNECT_BACKOFF_S[min(attempt - 1, len(RECONNECT_BACKOFF_S) - 1)]
            if self._wake.wait(min(delay, max(0.0, left))):
                self._wake.clear()
                if self._closing:
                    break
            outcome, why = self._session(resume=True)
            if outcome == "lost":     # back in, and then lost again: start over
                return outcome, why
            if outcome == "ended":    # refused for good, or closed by us
                return outcome, why
            reason = why              # "failed": could not get through; try again
        return "ended", reason

    def _session(self, resume: bool) -> "tuple[str, str]":
        """One connection, until it ends. Returns (outcome, reason):
        "lost" -- it was up and dropped without a goodbye (may reconnect);
        "failed" -- it never came up (unreachable, or no answer);
        "ended" -- the host said goodbye, refused us, or broke the protocol."""
        reason = "Disconnected from the host."
        established = False
        try:
            self._conn = self._connector()
            if self._closing:
                return "ended", reason
            if resume:  # a host that takes the connection but never answers
                self._conn.set_timeout(HELLO_TIMEOUT_S)
            self._conn.send(self._hello(resume))
            inbound = chat.TokenBucket(HOST_BURST, HOST_REFILL_S)
            while True:
                message = self._conn.receive()
                if message is None:
                    break
                if not inbound.allow():
                    return "ended", "The host sent more than a game ever needs; disconnected."
                if message["type"] == "bye":
                    return "ended", _clean_text(message.get("reason"), reason)
                if message["type"] == "welcome":
                    if resume and not self._accept_resumed(message):
                        return "ended", "Could not confirm it was the same host; disconnected."
                    established = True
                    if resume:
                        self._conn.set_timeout(None)
                        continue
                self._dispatch(message)
        except ProtocolError:
            return "ended", "The host sent something this version does not understand."
        except PolicyBlocked as exc:
            return "ended", str(exc)
        except OSError as exc:
            # A relay failure carries its own player-facing message.
            reason = (str(exc) if self._online else
                      f"Could not reach {self.address}:{self.port} ({exc.strerror or exc}).")
        finally:
            if self._conn is not None:
                self._conn.close()
        if self._closing:
            return "ended", reason
        return ("lost" if established else "failed"), reason

    def _hello(self, resume: bool) -> dict:
        hello = {"type": "hello", "name": self.name, "code": self.code,
                 "board": self.board.fingerprint}
        if self._chat_keys is None and chat.available():
            self._chat_keys = chat.KeyPair.generate()
        if self._chat_keys is not None:
            hello.update({"chat": chat.PROTOCOL, "key": self._chat_keys.public_b64})
        if resume:
            link = self._link
            if link is not None:
                with link.send_lock:
                    box = link.seal("resume", {"k": "resume", "player": self.player_id})
                hello["resume"] = {"player": self.player_id, **box}
            else:
                hello["resume"] = {"player": self.player_id, "token": self._resume_token}
        return hello

    def _accept_resumed(self, welcome: dict) -> bool:
        """The host's answer to a reconnection: with a secured channel it must
        carry a box only the real host can seal."""
        if welcome.get("resumed") is not True or welcome.get("player") != self.player_id:
            return False
        link = self._link
        if link is not None:
            proof = link.open("welcome", welcome.get("n"), welcome.get("box"))
            if not (isinstance(proof, dict) and proof.get("k") == "resumed"
                    and proof.get("player") == self.player_id):
                return False
        else:
            token = welcome.get("resume_token")
            if not (isinstance(token, str) and 16 <= len(token) <= 64):
                return False
            self._resume_token = token
        log.info("Timezone Dominator reconnected to the host")
        self.on_event("resumed", self.player_id)
        return True

    def _connect_tcp(self) -> _Connection:
        if not lan_available():
            raise PolicyBlocked(LAN_OFF)
        sock = socket.create_connection((self.address, self.port),
                                        timeout=self.CONNECT_TIMEOUT_S)
        return _Connection(sock)

    def _dispatch(self, message: dict) -> None:
        kind = message["type"]
        if kind == "welcome":
            pid = message.get("player")
            if isinstance(pid, int) and not isinstance(pid, bool):
                self.player_id = pid
                token = message.get("resume_token")
                if isinstance(token, str) and 16 <= len(token) <= 64:
                    self._resume_token = token
                self.on_event("welcome", pid)
                self._secure(message, pid)
        elif kind == "chat":
            self._receive_chat(message)
        elif kind == "lobby":
            lobby = _clean_lobby(message.get("lobby"))
            if lobby is not None:
                self.on_event("lobby", lobby)
        elif kind == "state":
            try:
                state = rules.GameState.from_dict(message.get("state"), self.board)
            except ValueError:
                log.warning("Ignoring an invalid Timezone Dominator state from the host")
                return
            self._in_game = True
            events = message.get("events")
            events = [e for e in events if isinstance(e, dict)][:64] \
                if isinstance(events, list) else []
            self.on_event("state", {"state": state, "events": events})
        elif kind == "error":
            self.on_event("error", _clean_text(message.get("message"), "Not allowed."))

    # -- chat ---------------------------------------------------------------------
    def _secure(self, welcome: dict, player_id: int) -> None:
        """Set up chat from the host's welcome: its key must open the sealed
        "ready" box, which proves the key came from the host itself."""
        state = "unavailable"
        if self._chat_keys is not None and welcome.get("chat") == chat.PROTOCOL:
            state = "failed"
            host_key = chat.parse_public_key(welcome.get("key"), own=self._chat_keys.public)
            link = None
            if host_key is not None:
                try:
                    link = chat.ChatLink(self._chat_keys, host_key, player_id, "guest")
                except ValueError:
                    link = None
            proof = None if link is None else \
                link.open("welcome", welcome.get("n"), welcome.get("box"))
            if proof is not None and proof.get("k") == "ready":
                with link.send_lock:
                    self._conn.send({"type": "chat", **link.seal("chat", {"k": "ready"})})
                self._link = link
                self.safety_code = chat.safety_code(host_key)
                state = "secure"
        self.on_event("secure", {"state": state, "code": self.safety_code})

    def _receive_chat(self, message: dict) -> None:
        link = self._link
        if link is None:
            return
        payload = link.open("chat", message.get("n"), message.get("box"))
        if payload is None:
            if link.failures == chat.MAX_FAILURES:
                self.on_event("error", "Some chat messages could not be read.")
            return
        sender, text = payload.get("from"), chat.clean_text(payload.get("text"))
        if payload.get("k") != "say" or text is None \
                or not isinstance(sender, int) or isinstance(sender, bool):
            return
        self.on_event("chat", {"from": sender, "name": clean_name(payload.get("name")),
                               "text": text})

    def send_chat(self, text: object) -> "str | None":
        """None once sent, otherwise why not, for the player."""
        link, conn = self._link, self._conn
        if link is None or conn is None:
            return "Chat isn't available in this lobby."
        text = chat.clean_text(text)
        if text is None:
            return "There is nothing to send."
        if not self._chat_bucket.allow():
            return "You're sending messages too quickly."
        with link.send_lock:
            sent = conn.send({"type": "chat", **link.seal("chat", {"k": "say", "text": text})})
        return None if sent is not False else "Not connected to the host."

    def send_action(self, action: dict) -> None:
        if self._conn is not None:
            self._conn.send({"type": "action", "action": action})

    def set_color(self, color: int) -> None:
        if self._conn is not None:
            self._conn.send({"type": "color", "color": color})

    def close(self) -> None:
        self._closing = True
        self._wake.set()
        if self._conn is not None:  # _session's reader closes it after
            self._conn.finish({"type": "bye"})


def _clean_text(value: object, fallback: str) -> str:
    if not isinstance(value, str):
        return fallback
    text = "".join(ch for ch in value if unicodedata.category(ch)[0] != "C")
    return text.strip()[:160] or fallback


def _clean_lobby(raw: object) -> "dict | None":
    if not isinstance(raw, dict) or not isinstance(raw.get("players"), list):
        return None
    players = []
    for p in raw["players"][:rules.MAX_PLAYERS]:
        if not isinstance(p, dict):
            return None
        pid, color = p.get("id"), p.get("color")
        if not isinstance(pid, int) or isinstance(pid, bool):
            return None
        if not (color is None or (isinstance(color, int) and not isinstance(color, bool)
                                  and 0 <= color < rules.MAX_PLAYERS)):
            return None
        players.append({"id": pid, "name": clean_name(p.get("name")), "color": color,
                        "chat": p.get("chat") is True, "away": p.get("away") is True})
    return {"name": clean_name(raw.get("name"), "Timezone Dominator"), "players": players,
            "started": bool(raw.get("started"))}


def discover_lobbies(board: rules.Board, timeout: float = 1.2,
                     port: int = DISCOVERY_PORT) -> list[dict]:
    """Broadcast one probe and collect the lobbies that answer. Blocking;
    call it off the UI thread."""
    if not lan_available():
        return []
    found: dict[tuple[str, int], dict] = {}
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.settimeout(0.2)
        for target in ("255.255.255.255", "127.0.0.1"):
            try:
                sock.sendto(DISCOVERY_PROBE, (target, port))
            except OSError:
                pass
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                data, (address, _src_port) = sock.recvfrom(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                info = decode(data.strip())
            except ProtocolError:
                continue
            lobby_port = info.get("port")
            if info.get("type") != "lobby_info" or not isinstance(lobby_port, int) \
                    or isinstance(lobby_port, bool) or not 0 < lobby_port < 65536:
                continue
            players = info.get("players")
            if (address, lobby_port) not in found and len(found) >= MAX_DISCOVERED:
                continue
            found[(address, lobby_port)] = {
                "address": address, "port": lobby_port,
                "name": clean_name(info.get("name"), "Timezone Dominator"),
                "host": clean_name(info.get("host"), "Host"),
                "players": players if isinstance(players, int)
                and not isinstance(players, bool) else 0,
                "started": bool(info.get("started")),
                "compatible": info.get("board") == board.fingerprint,
            }
    finally:
        sock.close()
    # A host on this machine answers on loopback and on its LAN address; keep
    # the LAN entry, which is the one other players would see too.
    lan = {(i["name"], i["port"]) for (a, _p), i in found.items() if not a.startswith("127.")}
    return [info for (address, _p), info in found.items()
            if not (address.startswith("127.") and (info["name"], info["port"]) in lan)]
