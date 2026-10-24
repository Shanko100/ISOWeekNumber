"""Online Timezone Dominator lobbies through the Timezone Dominator relay (see relay/src/index.ts).

The relay is a Cloudflare Worker that keeps the public lobby list and passes
messages between a host and its players. Everything else is the LAN code
path: the host's LobbyHost stays authoritative and the same handshake,
validation and rules apply. This module only supplies connections:

* RelayHostTransport creates a lobby, holds the host's WebSocket, and hands
  LobbyHost one RelayPeer per player -- objects with the same send/receive
  interface as a LAN socket connection.
* connect_guest() opens a player's WebSocket, wrapped the same way.
* LobbyBrowser keeps a live copy of the public lobby list.

Online play is opt-in: nothing here runs until the player has allowed the
connection for this session of Timezone Dominator (asked every time it opens; until then
weeknumber_core.NetworkGate blocks every connection) and opens the Online tab, and an IT policy can switch it off or
point it at the organisation's own relay (see docs/ADMINISTRATION.md).
Certificates are checked against the operating system's trust store
(truststore), so no CA bundle has to ship with the app.

Bounds. Nothing the relay (or anyone pretending to be it) sends can make the
app hold more than a fixed amount: one WebSocket message is at most
MAX_FRAME_BYTES before it is read in full, each player's queue on the host
holds PEER_INBOX messages, and a host serves at most MAX_PEERS connections.
Sends and reads time out (SEND_TIMEOUT_S), and a connection that stops
answering pings is treated as lost, so a dropped player's app notices and
reconnects (weeknumber_tzd_net, reconnecting).
"""

from __future__ import annotations

import json
import os
import queue
import socket
import ssl
import threading
import time
import urllib.error
import urllib.request
from typing import TYPE_CHECKING

import weeknumber_tzd_net as net
import weeknumber_tzd_rules as rules
from weeknumber_core import APP_VERSION, log, policy

if TYPE_CHECKING:
    from collections.abc import Callable

#: Base URL of the deployed relay, e.g. "https://tzd-relay.<account>.workers.dev".
#: Empty disables online play. WEEKNUMBER_TZD_RELAY overrides it (testing,
#: a self-hosted relay); the RelayURL and DisableOnline policies override both.
RELAY_URL = "https://tzd-relay.weeknumber.workers.dev"
RELAY_ENV = "WEEKNUMBER_TZD_RELAY"
CONNECT_TIMEOUT_S = 10.0
KEEPALIVE_S = 30.0
MAX_LOBBIES = 200
MAX_LIST_BYTES = 256 * 1024
#: Largest WebSocket message accepted: the lobby list, or a game message as
#: the relay wraps it (JSON escaping can grow it up to about four times).
MAX_FRAME_BYTES = max(MAX_LIST_BYTES, 4 * net.MAX_MESSAGE_BYTES) + 4096
#: A connection that answered pings before and has now been silent this long
#: is dead (Wi-Fi gone, laptop asleep) even though no error arrived.
DEAD_AFTER_S = 2 * KEEPALIVE_S + 10
#: Player connections one host serves at once: the relay's five, plus room
#: for players coming back before the relay noticed their old connection end.
MAX_PEERS = 2 * rules.MAX_PLAYERS
#: Messages waiting per player on the host; more means the player is
#: sending faster than any game needs, and is disconnected.
PEER_INBOX = 64


class RelayError(OSError):
    """A relay problem with a message fit to show the player."""

    def __str__(self) -> str:
        return self.args[0] if self.args else "The online lobby service failed."


def relay_url() -> "str | None":
    org = policy()
    if org.disable_online:
        return None
    url = (org.relay_url or os.environ.get(RELAY_ENV) or RELAY_URL).strip().rstrip("/")
    if not url.startswith(("https://", "http://")):
        return None
    return url


def online_available() -> bool:
    return relay_url() is not None


def _ssl_context() -> ssl.SSLContext:
    try:
        import truststore
        return truststore.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    except Exception:
        return ssl.create_default_context()


def _ws_url(base: str, path: str) -> str:
    if base.startswith("https://"):
        return "wss://" + base[len("https://"):] + path
    return "ws://" + base[len("http://"):] + path


_STATUS_MESSAGES = {
    403: "The relay did not accept this host.",
    404: "That lobby no longer exists.",
    409: "That lobby is full.",
    410: "That game has ended.",
    429: "Too many requests from your network; wait a few minutes.",
    503: "The online lobby service is busy; try again shortly.",
}


class _Watch:
    """Limits what one WebSocket may make the app hold, and notices when it
    went silent. websocket-client has no size limit of its own, so the
    length of every frame is checked as soon as its header arrives -- before
    any of it is read -- and so is a message assembled from fragments. These
    are hooks into that library's frame reader; a version without them is
    refused (no online play) rather than run without the limit."""

    def __init__(self, ws) -> None:
        import websocket
        frames, parts = getattr(ws, "frame_buffer", None), getattr(ws, "cont_frame", None)
        recv_length = getattr(frames, "recv_length", None)
        recv_frame = getattr(frames, "recv_frame", None)
        add = getattr(parts, "add", None)
        if not (callable(recv_length) and callable(recv_frame) and callable(add)
                and hasattr(frames, "length") and hasattr(parts, "cont_data")):
            raise RelayError("This version of websocket-client is not supported for online play.")
        too_large = websocket.WebSocketProtocolException
        pong = websocket.ABNF.OPCODE_PONG

        def limited_length() -> None:
            recv_length()
            if not isinstance(frames.length, int) or frames.length > MAX_FRAME_BYTES:
                raise too_large("message too large")

        def limited_add(frame) -> None:
            held = parts.cont_data[1] if parts.cont_data else b""
            if len(held) + len(frame.data) > MAX_FRAME_BYTES:
                raise too_large("message too large")
            add(frame)

        def watched_frame():
            frame = recv_frame()
            self.last = time.monotonic()
            if frame.opcode == pong:
                self.answers = True
            return frame

        frames.recv_length = limited_length
        frames.recv_frame = watched_frame
        parts.add = limited_add
        self.last = time.monotonic()
        #: Only a connection that answered a ping before can be called silent.
        self.answers = False

    def dead(self) -> bool:
        return self.answers and time.monotonic() - self.last > DEAD_AFTER_S


def _recv(ws, watch: _Watch, stop: threading.Event):
    """The next message, or None once the connection closed, went silent or
    `stop` was set. Reads wake every SEND_TIMEOUT_S to check."""
    import websocket
    while not stop.is_set():
        try:
            return ws.recv()
        except websocket.WebSocketTimeoutException:
            if watch.dead():
                log.warning("The Timezone Dominator relay connection went silent")
                return None
    return None


def _websocket(path: str, headers: "list[str] | None" = None):
    """Open a WebSocket to the relay, mapping failures to RelayError.
    Returns the socket and its _Watch."""
    base = relay_url()
    if base is None:
        raise RelayError("Online play is not set up in this build.")
    try:
        import websocket
    except ImportError as exc:
        raise RelayError("Online play needs the websocket-client package.") from exc
    url = _ws_url(base, path)
    options = {"context": _ssl_context()} if url.startswith("wss://") else {}
    try:
        ws = websocket.create_connection(
            url, timeout=CONNECT_TIMEOUT_S, header=headers or [],
            sslopt=options, enable_multithread=True, suppress_origin=True)
    except websocket.WebSocketBadStatusException as exc:
        status = getattr(exc, "status_code", 0)
        raise RelayError(_STATUS_MESSAGES.get(
            status, f"The online lobby service refused the connection ({status}).")) from exc
    except (OSError, websocket.WebSocketException) as exc:
        raise RelayError("Could not reach the online lobby service.") from exc
    try:
        watch = _Watch(ws)
    except RelayError:
        ws.close()
        raise
    # Sends and reads end after this; readers just try again (see _recv).
    ws.settimeout(net.SEND_TIMEOUT_S)
    if getattr(ws, "sock", None) is not None:
        net.enable_keepalive(ws.sock)
    return ws, watch


def _keepalive(ws, stop: threading.Event) -> None:
    """Protocol pings keep idle connections open through proxies and NAT.
    The relay answers them without waking a lobby, and they are not billed."""
    while not stop.wait(KEEPALIVE_S):
        try:
            ws.ping()
        except Exception:
            return


def _start_keepalive(ws, stop: threading.Event) -> None:
    threading.Thread(target=_keepalive, args=(ws, stop), name="tzd-ping",
                     daemon=True).start()


# ---------------------------------------------------------------------------
# Lobby creation
# ---------------------------------------------------------------------------
def create_lobby(name: str, host: str, board: rules.Board) -> "tuple[str, str, frozenset[str]]":
    """Register a public lobby; returns (lobby id, host token, features). The
    features say what the relay passes beyond the game, e.g. "chat"; an
    older relay sends none and would disconnect players who send chat."""
    base = relay_url()
    if base is None:
        raise RelayError("Online play is not set up in this build.")
    body = json.dumps({"name": name, "host": host, "board": board.fingerprint}).encode()
    request = urllib.request.Request(
        base + "/v1/lobbies", data=body, method="POST",
        headers={"Content-Type": "application/json",
                 "User-Agent": f"WeekNumber/{APP_VERSION}"})
    context = _ssl_context() if base.startswith("https://") else None
    try:
        with urllib.request.urlopen(request, timeout=CONNECT_TIMEOUT_S,
                                    context=context) as response:
            payload = json.loads(response.read(4096).decode("utf-8"))
    except urllib.error.HTTPError as exc:
        message = None
        try:
            message = json.loads(exc.read(4096).decode("utf-8")).get("error")
        except Exception:
            pass
        raise RelayError(net._clean_text(message, _STATUS_MESSAGES.get(
            exc.code, "The online lobby service refused the lobby."))) from exc
    except (OSError, ValueError) as exc:
        raise RelayError("Could not reach the online lobby service.") from exc
    lobby_id, token = payload.get("id"), payload.get("token")
    if not (isinstance(lobby_id, str) and lobby_id.isalnum() and len(lobby_id) == 12
            and isinstance(token, str) and token.isalnum() and 16 <= len(token) <= 128):
        raise RelayError("The online lobby service sent an unexpected reply.")
    features = payload.get("features")
    features = frozenset(f for f in features if isinstance(f, str) and len(f) <= 32) \
        if isinstance(features, list) else frozenset()
    return lobby_id, token, features


# ---------------------------------------------------------------------------
# Host side
# ---------------------------------------------------------------------------
class RelayPeer:
    """One player as LobbyHost sees them: the interface of a LAN connection,
    carried over the host's single relay WebSocket."""

    def __init__(self, transport: "RelayHostTransport", peer: int) -> None:
        self._transport = transport
        self.peer = peer
        self._inbox: "queue.Queue[str | None]" = queue.Queue(maxsize=PEER_INBOX + 1)
        self._timeout: "float | None" = None
        self._overflow = False
        self.closed = False

    def feed(self, text: "str | None") -> None:
        """Queue a message from the relay's reader thread. One slot is kept
        for the end of the connection; a full queue disconnects the player."""
        if text is None:
            self.closed = True
            try:
                self._inbox.put_nowait(None)
            except queue.Full:
                self._overflow = True
            return
        if self._overflow:
            return
        if self._inbox.qsize() >= PEER_INBOX:
            log.warning("Relay player %s sent too much at once; disconnecting", self.peer)
            self._overflow = True
            self.close()
            return
        try:
            self._inbox.put_nowait(text)
        except queue.Full:
            self._overflow = True

    def set_timeout(self, seconds: "float | None") -> None:
        self._timeout = seconds

    def send(self, message: dict) -> bool:
        if self.closed:
            return False
        try:
            data = net.encode(message).decode("utf-8").rstrip("\n")
        except net.ProtocolError:
            self.close()
            return False
        if not self._transport.send_to(self.peer, data):
            self.closed = True
            return False
        return True

    def receive(self) -> "dict | None":
        if self._overflow:
            self.closed = True
            return None
        try:
            text = self._inbox.get(timeout=self._timeout)
        except queue.Empty:
            raise socket.timeout("no message from the player in time") from None
        if text is None or self._overflow:
            self.closed = True
            return None
        return net.decode(text.encode("utf-8"))

    def finish(self, message: dict) -> None:
        """A last message, then close. The relay WebSocket delivers frames
        in order, so the goodbye cannot be overtaken by the close."""
        self.send(message)
        self.close()

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        self._transport.close_peer(self.peer)
        try:
            self._inbox.put_nowait(None)
        except queue.Full:
            self._overflow = True


class RelayHostTransport:
    """Creates a public lobby and relays its players to LobbyHost."""

    def __init__(self) -> None:
        self._ws = None
        self._watch: "_Watch | None" = None
        self._send_lock = threading.Lock()
        self._peers: dict[int, RelayPeer] = {}
        self._stop = threading.Event()
        self._last_info: "tuple[int, bool] | None" = None
        self.lobby_id: "str | None" = None
        #: Whether this relay passes chat messages (see create_lobby).
        self.supports_chat = False

    def start(self, lobby_name: str, host_name: str, board: rules.Board,
              on_peer: "Callable[[RelayPeer, str], None]",
              on_lost: "Callable[[str], None]") -> None:
        """Blocking: one HTTPS request and one WebSocket handshake. Raises
        RelayError when the relay cannot be reached or refuses."""
        lobby_id, token, features = create_lobby(lobby_name, host_name, board)
        self.supports_chat = "chat" in features
        self._ws, self._watch = _websocket(f"/v1/lobbies/{lobby_id}/host",
                                           headers=[f"Authorization: Bearer {token}"])
        self.lobby_id = lobby_id
        _start_keepalive(self._ws, self._stop)
        threading.Thread(target=self._read, args=(on_peer, on_lost),
                         name="tzd-relay-host", daemon=True).start()
        log.info("Timezone Dominator online lobby %s is listed", lobby_id)

    def _read(self, on_peer, on_lost) -> None:
        ws, watch = self._ws, self._watch
        try:
            while not self._stop.is_set():
                text = _recv(ws, watch, self._stop)
                if not text:
                    break
                if not isinstance(text, str) or len(text) > MAX_FRAME_BYTES:
                    continue
                try:
                    frame = json.loads(text)
                except ValueError:
                    continue
                if not isinstance(frame, dict):
                    continue
                peer = frame.get("peer")
                if not isinstance(peer, int) or isinstance(peer, bool):
                    continue
                kind = frame.get("relay")
                if kind == "open" and peer not in self._peers:
                    if len(self._peers) >= MAX_PEERS:
                        self._send_frame({"relay": "close", "peer": peer})
                        continue
                    connection = RelayPeer(self, peer)
                    self._peers[peer] = connection
                    # The relay tells the host nothing about a player's network.
                    on_peer(connection, f"peer-{peer}")
                elif kind == "data" and isinstance(frame.get("data"), str) \
                        and len(frame["data"]) <= 2 * net.MAX_MESSAGE_BYTES:
                    connection = self._peers.get(peer)
                    if connection is not None:
                        connection.feed(frame["data"])
                elif kind == "close":
                    connection = self._peers.pop(peer, None)
                    if connection is not None:
                        connection.closed = True
                        connection.feed(None)
        except Exception:
            if not self._stop.is_set():
                log.warning("Lost the Timezone Dominator relay connection")
        finally:
            for connection in list(self._peers.values()):
                connection.closed = True
                connection.feed(None)
            self._peers.clear()
            if not self._stop.is_set():
                on_lost("Lost the connection to the online lobby service.")

    def _send_frame(self, frame: dict) -> bool:
        ws = self._ws
        if ws is None:
            return False
        try:
            with self._send_lock:
                ws.send(json.dumps(frame, separators=(",", ":"), ensure_ascii=False))
            return True
        except Exception:
            return False

    def send_to(self, peer: int, data: str) -> bool:
        return self._send_frame({"relay": "send", "peer": peer, "data": data})

    def close_peer(self, peer: int) -> None:
        if self._peers.pop(peer, None) is not None:
            self._send_frame({"relay": "close", "peer": peer})

    def update(self, players: int, started: bool) -> None:
        """Seat count and status for the public list; sent only on change."""
        info = (players, started)
        if info != self._last_info:
            self._last_info = info
            self._send_frame({"relay": "info", "players": players, "started": started})

    def stop(self) -> None:
        self._stop.set()
        ws, self._ws = self._ws, None
        if ws is not None:
            try:
                ws.close()  # the relay then delists the lobby and closes the players
            except Exception:
                pass


# ---------------------------------------------------------------------------
# Player side
# ---------------------------------------------------------------------------
class GuestConnection:
    """A player's relay WebSocket with the LAN connection's interface."""

    def __init__(self, ws, watch: _Watch) -> None:
        self._ws = ws
        self._watch = watch
        self._send_lock = threading.Lock()
        self._stop = threading.Event()
        self._read_timeout: "float | None" = None
        self.closed = False
        _start_keepalive(ws, self._stop)

    def set_timeout(self, seconds: "float | None") -> None:
        """How long `receive` may wait (None: as long as the connection
        lives). Sends stay bounded either way."""
        self._read_timeout = seconds
        try:
            self._ws.settimeout(net.SEND_TIMEOUT_S if seconds is None
                                else min(seconds, net.SEND_TIMEOUT_S))
        except Exception:
            pass

    def send(self, message: dict) -> bool:
        if self.closed:
            return False
        try:
            data = net.encode(message).decode("utf-8").rstrip("\n")
            with self._send_lock:
                self._ws.send(data)
            return True
        except Exception:
            self.close()
            return False

    def receive(self) -> "dict | None":
        """Next message, or None when the connection closed or went silent.
        Raises socket.timeout when a read timeout is set and passes."""
        import websocket
        deadline = (None if self._read_timeout is None
                    else time.monotonic() + self._read_timeout)
        while True:
            try:
                text = self._ws.recv()
                break
            except websocket.WebSocketTimeoutException:
                if self.closed or self._watch.dead():
                    return None
                if deadline is not None and time.monotonic() >= deadline:
                    raise socket.timeout("no message from the host in time") from None
            except Exception:
                return None
        if not text:
            return None
        if not isinstance(text, str):
            raise net.ProtocolError("unexpected binary message")
        return net.decode(text.encode("utf-8"))

    def finish(self, message: dict) -> None:
        """A last message, then close: the WebSocket closing handshake comes
        after the goodbye frame, so the goodbye is not lost."""
        self.send(message)
        self.close()

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        self._stop.set()
        try:
            self._ws.close()
        except Exception:
            pass


def connect_guest(lobby_id: str) -> GuestConnection:
    if not (lobby_id.isalnum() and len(lobby_id) == 12):
        raise RelayError("That lobby no longer exists.")
    return GuestConnection(*_websocket(f"/v1/lobbies/{lobby_id}/join"))


# ---------------------------------------------------------------------------
# Public lobby list
# ---------------------------------------------------------------------------
def _clean_lobbies(raw: object, board: rules.Board) -> "list[dict] | None":
    if not isinstance(raw, dict) or raw.get("type") != "lobbies":
        return None
    items = raw.get("lobbies")
    if not isinstance(items, list):
        return None
    lobbies = []
    for item in items[:MAX_LOBBIES]:
        if not isinstance(item, dict):
            continue
        lobby_id, players = item.get("id"), item.get("players")
        if not (isinstance(lobby_id, str) and lobby_id.isalnum() and len(lobby_id) == 12):
            continue
        if not isinstance(players, int) or isinstance(players, bool):
            players = 1
        lobbies.append({
            "id": lobby_id,
            "name": net.clean_name(item.get("name"), "Timezone Dominator"),
            "host": net.clean_name(item.get("host"), "Host"),
            "players": max(1, min(rules.MAX_PLAYERS, players)),
            "started": bool(item.get("started")),
            "compatible": item.get("board") == board.fingerprint,
        })
    return lobbies


class LobbyBrowser:
    """Keeps a live copy of the public lobby list while the Online tab is
    open. Callbacks fire on the browser's thread: `on_update(lobbies)` with
    every new list, `on_status(message | None)` when connectivity changes."""

    BACKOFF_S = (2, 4, 8, 15, 30)

    def __init__(self, board: rules.Board, on_update, on_status) -> None:
        self.board = board
        self._on_update = on_update
        self._on_status = on_status
        self._stop = threading.Event()
        self._ws = None

    def start(self) -> None:
        threading.Thread(target=self._run, name="tzd-browser", daemon=True).start()

    def stop(self) -> None:
        self._stop.set()
        ws, self._ws = self._ws, None
        if ws is not None:
            try:
                ws.close()
            except Exception:
                pass

    def _run(self) -> None:
        attempt = 0
        while not self._stop.is_set():
            ping_stop = threading.Event()
            try:
                ws, watch = _websocket("/v1/lobbies")
                if self._stop.is_set():
                    ws.close()
                    return
                self._ws = ws
                _start_keepalive(ws, ping_stop)
                attempt = 0
                self._on_status(None)
                while not self._stop.is_set():
                    text = _recv(ws, watch, self._stop)
                    if not text:
                        break
                    if not isinstance(text, str) or len(text) > MAX_LIST_BYTES:
                        continue
                    try:
                        lobbies = _clean_lobbies(json.loads(text), self.board)
                    except ValueError:
                        continue
                    if lobbies is not None:
                        self._on_update(lobbies)
                if not self._stop.is_set():
                    self._on_status("Reconnecting to the online lobby service\u2026")
            except RelayError as exc:
                if not self._stop.is_set():
                    self._on_status(str(exc))
            except Exception:
                if not self._stop.is_set():
                    self._on_status("Lost the connection to the online lobby service.")
            finally:
                ping_stop.set()
            if self._stop.is_set():
                return
            delay = self.BACKOFF_S[min(attempt, len(self.BACKOFF_S) - 1)]
            attempt += 1
            if self._stop.wait(delay):
                return
