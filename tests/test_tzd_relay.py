"""Online Timezone Dominator lobbies.

The unit tests need no network. The integration tests run against a real
relay and are skipped unless WEEKNUMBER_TZD_RELAY points at one, e.g.:

    cd relay && npx wrangler dev --port 8787
    WEEKNUMBER_TZD_RELAY=http://127.0.0.1:8787 python -m pytest tests/test_tzd_relay.py
"""

import json
import os
import queue
import random
import socket
import time

import pytest

import weeknumber_tzd_net as net
import weeknumber_tzd_relay as relay
import weeknumber_tzd_rules as rules

TIMEOUT = 10.0
LIVE = pytest.mark.skipif(not os.environ.get(relay.RELAY_ENV),
                          reason=f"set {relay.RELAY_ENV} to a running relay")


@pytest.fixture(scope="module")
def board():
    loaded = rules.load_board()
    assert loaded is not None
    return loaded


class Inbox:
    def __init__(self):
        self.items = queue.Queue()

    def __call__(self, kind, data=None):
        self.items.put((kind, data))

    def wait(self, kind, predicate=lambda data: True, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                got_kind, data = self.items.get(timeout=0.1)
            except queue.Empty:
                continue
            if got_kind == kind and predicate(data):
                return data
        raise AssertionError(f"no {kind!r} event within {timeout}s")


# -- unit ---------------------------------------------------------------------
def test_relay_url_needs_http_scheme(monkeypatch):
    monkeypatch.setenv(relay.RELAY_ENV, "tzd.example")
    assert relay.relay_url() is None
    monkeypatch.setenv(relay.RELAY_ENV, "https://tzd.example/")
    assert relay.relay_url() == "https://tzd.example"
    assert relay._ws_url("https://tzd.example", "/v1/lobbies") == "wss://tzd.example/v1/lobbies"
    assert relay._ws_url("http://127.0.0.1:8787", "/x") == "ws://127.0.0.1:8787/x"


@pytest.mark.parametrize("url, usable", [
    ("https://tzd.example", True),
    ("https://tzd.example:8443/base", True),
    ("http://127.0.0.1:8787", True),
    ("http://localhost:8787", True),
    ("http://[::1]:8787", True),
    ("http://tzd.example", False),           # unencrypted beyond this machine
    ("http://127.evil.example", False),
    ("https://user:pw@tzd.example", False),
    ("https://tzd.example/?x=1", False),
    ("https://tzd.example/#x", False),
    ("https://tzd.example:99999", False),
    ("ftp://tzd.example", False),
    ("https://", False),
])
def test_relay_url_must_be_a_plain_address(monkeypatch, url, usable):
    monkeypatch.setenv(relay.RELAY_ENV, url)
    assert (relay.relay_url() is not None) is usable


@pytest.mark.parametrize("value, ok", [
    ("abcdefghij12", True),
    ("abcdefghij1", False),
    ("abcdefghij\u0661\u0662", False),     # Arabic-Indic digits pass isalnum()
    ("\u00e9bcdefghij12"[:12], False),
    (None, False),
])
def test_lobby_ids_are_ascii(value, ok):
    assert relay._is_id(value, 12, 12) is ok


def test_lobby_list_is_validated(board):
    raw = {"type": "lobbies", "lobbies": [
        {"id": "abcdefghij12", "name": "Fri\x00day", "host": "Ann", "players": 3,
         "started": False, "board": board.fingerprint},
        {"id": "../etc/passwd", "name": "bad id"},
        {"id": "abcdefghij34", "name": "x" * 99, "host": None, "players": 99,
         "started": True, "board": "0000000000000000"},
        "junk",
    ]}
    lobbies = relay._clean_lobbies(raw, board)
    assert [l["id"] for l in lobbies] == ["abcdefghij12", "abcdefghij34"]
    assert lobbies[0]["name"] == "Friday" and lobbies[0]["compatible"]
    assert lobbies[1]["players"] == rules.MAX_PLAYERS
    assert len(lobbies[1]["name"]) == rules.MAX_NAME_CHARS
    assert lobbies[1]["host"] == "Host" and not lobbies[1]["compatible"]
    assert relay._clean_lobbies({"type": "other"}, board) is None


class FakeTransport:
    def __init__(self):
        self.sent, self.closed = [], []

    def send_to(self, peer, data):
        self.sent.append((peer, data))
        return True

    def close_peer(self, peer):
        self.closed.append(peer)


def test_relay_peer_behaves_like_a_connection():
    transport = FakeTransport()
    peer = relay.RelayPeer(transport, 7)
    assert peer.send({"type": "welcome", "player": 1})
    assert transport.sent == [(7, '{"type":"welcome","player":1,"v":1}')]
    peer.feed('{"v":1,"type":"hello","name":"Ann"}')
    assert peer.receive() == {"v": 1, "type": "hello", "name": "Ann"}
    peer.set_timeout(0.05)
    with pytest.raises(socket.timeout):
        peer.receive()
    peer.feed('{"v":2,"type":"hello"}')
    with pytest.raises(net.ProtocolError):
        peer.receive()
    peer.close()
    assert transport.closed == [7]
    assert peer.receive() is None and not peer.send({"type": "x"})


def test_a_flooded_relay_peer_is_disconnected_with_bounded_memory():
    transport = FakeTransport()
    peer = relay.RelayPeer(transport, 3)
    for _ in range(relay.PEER_INBOX * 10):
        peer.feed('{"v":1,"type":"chat"}')
    assert peer._inbox.qsize() <= relay.PEER_INBOX + 1
    assert transport.closed == [3]
    assert peer.receive() is None  # the host's reader sees the end at once


class _FakeFrames:
    def __init__(self):
        self.length = None
        self.next_length = 0
        self.next_opcode = 0x1

    def recv_length(self):
        self.length = self.next_length

    def recv_frame(self):
        return type("Frame", (), {"opcode": self.next_opcode, "data": b""})()


class _FakeParts:
    def __init__(self):
        self.cont_data = None

    def add(self, frame):
        if self.cont_data:
            self.cont_data[1] += frame.data
        else:
            self.cont_data = [1, frame.data]


def test_websocket_messages_are_capped_before_they_are_read():
    import websocket
    ws = type("WS", (), {})()
    ws.frame_buffer, ws.cont_frame = _FakeFrames(), _FakeParts()
    watch = relay._Watch(ws)
    ws.frame_buffer.next_length = relay.MAX_FRAME_BYTES
    ws.frame_buffer.recv_length()
    ws.frame_buffer.next_length = relay.MAX_FRAME_BYTES + 1
    with pytest.raises(websocket.WebSocketProtocolException):
        ws.frame_buffer.recv_length()   # a huge length is refused from the header alone
    part = type("Frame", (), {"data": b"x" * (relay.MAX_FRAME_BYTES // 2 + 1)})()
    ws.cont_frame.add(part)
    with pytest.raises(websocket.WebSocketProtocolException):
        ws.cont_frame.add(part)         # and so are fragments adding up to too much
    assert not watch.dead()


def test_a_silent_connection_counts_as_dead_only_after_it_answered_pings(monkeypatch):
    ws = type("WS", (), {})()
    ws.frame_buffer, ws.cont_frame = _FakeFrames(), _FakeParts()
    watch = relay._Watch(ws)
    clock = [1000.0]
    monkeypatch.setattr(relay.time, "monotonic", lambda: clock[0])
    watch.last = clock[0]
    clock[0] += relay.DEAD_AFTER_S + 1
    assert not watch.dead()             # never answered a ping: no verdict
    ws.frame_buffer.next_opcode = 0xA   # a pong
    ws.frame_buffer.recv_frame()
    assert not watch.dead()
    clock[0] += relay.DEAD_AFTER_S + 1
    assert watch.dead()


def test_an_unknown_websocket_client_is_refused_not_run_unbounded():
    with pytest.raises(relay.RelayError):
        relay._Watch(type("WS", (), {})())


def test_the_installed_websocket_client_has_the_hooks_the_limit_needs():
    """Fails when an upgrade of websocket-client moves them: online play
    would then refuse to start (see _Watch)."""
    import websocket
    ws = websocket.WebSocket()
    relay._Watch(ws)
    assert ws.frame_buffer.recv_length.__name__ == "limited_length"
    assert ws.cont_frame.add.__name__ == "limited_add"


def test_unconfigured_build_reports_online_off(monkeypatch):
    monkeypatch.delenv(relay.RELAY_ENV, raising=False)
    monkeypatch.setattr(relay, "RELAY_URL", "")
    assert not relay.online_available()
    with pytest.raises(relay.RelayError, match="not set up"):
        relay.connect_guest("abcdefghij12")


# -- integration --------------------------------------------------------------
def open_browser(board):
    lists = Inbox()
    browser = relay.LobbyBrowser(board, lambda l: lists("list", l), lambda s: lists("status", s))
    browser.start()
    return browser, lists


@LIVE
def test_online_game_end_to_end(board):
    browser, lists = open_browser(board)
    lists.wait("list")
    host_inbox = Inbox()
    host = net.LobbyHost(board, "Hostess", f"Relay test {random.randrange(10**6)}",
                         host_inbox, transport=relay.RelayHostTransport(),
                         rng=random.Random(4))
    host.start()
    lobby_id = host._transport.lobby_id
    listed = lists.wait("list", lambda l: any(x["id"] == lobby_id for x in l))
    entry = next(x for x in listed if x["id"] == lobby_id)
    assert entry["host"] == "Hostess" and entry["players"] == 1 and entry["compatible"]

    guest_inbox = Inbox()
    guest = net.LobbyClient(board, "", 0, "Guest", "", guest_inbox,
                            connector=lambda: relay.connect_guest(lobby_id))
    guest.start()
    guest_id = guest_inbox.wait("welcome")
    lists.wait("list", lambda l: any(x["id"] == lobby_id and x["players"] == 2 for x in l))

    host.start_game()
    state = guest_inbox.wait("state")["state"]
    assert state.owner == host.state.owner
    lists.wait("list", lambda l: any(x["id"] == lobby_id and x["started"] for x in l))

    # Play a few moves from both sides through the relay.
    for _ in range(6):
        current = host.state.current_player["id"]
        action = {"type": "end_turn"} if host.state.phase in (
            rules.PHASE_ATTACK, rules.PHASE_FORTIFY) else {
            "type": "place", "t": host.state.territories_of(current)[0],
            "n": host.state.reinforcements}
        if current == net.LobbyHost.HOST_ID:
            host.submit(current, action)
        else:
            guest.send_action(action)
        expected = "turn" if action["type"] == "end_turn" else action["type"]
        guest_inbox.wait("state", lambda d: d["events"] and
                         d["events"][0]["type"] == expected)

    host.stop()
    assert "closed the game" in guest_inbox.wait("closed")
    lists.wait("list", lambda l: all(x["id"] != lobby_id for x in l))
    browser.stop()
    assert guest_id == 1


@LIVE
def test_joining_a_closed_lobby_fails_cleanly(board):
    inbox = Inbox()
    guest = net.LobbyClient(board, "", 0, "Late", "", inbox,
                            connector=lambda: relay.connect_guest("zzzzzzzzzzzz"))
    guest.start()
    assert inbox.wait("closed") == "That lobby no longer exists."


@LIVE
def test_guest_leaving_mid_game_is_reported(board):
    host_inbox = Inbox()
    host = net.LobbyHost(board, "Hostess", "Leave test", host_inbox,
                         transport=relay.RelayHostTransport())
    host.start()
    lobby_id = host._transport.lobby_id
    guests = []
    for name in ("Ann", "Bob"):
        inbox = Inbox()
        client = net.LobbyClient(board, "", 0, name, "", inbox,
                                 connector=lambda: relay.connect_guest(lobby_id))
        client.start()
        inbox.wait("welcome")
        guests.append((client, inbox))
    host.start_game()
    guests[1][1].wait("state")
    leaving = guests[0][0].player_id
    guests[0][0].close()
    state = guests[1][1].wait("state", lambda d: any(
        e.get("type") == "left" for e in d["events"]))["state"]
    assert state.player(leaving)["left"]
    host.stop()


@LIVE
def test_a_dropped_guest_gets_back_into_the_online_game(board, monkeypatch):
    """Through the real relay: the old connection drops without a goodbye,
    the app joins the lobby again and proves it is the same player."""
    monkeypatch.setattr(net, "RECONNECT_BACKOFF_S", (0.3, 0.5))
    host_inbox = Inbox()
    host = net.LobbyHost(board, "Hostess", "Reconnect test", host_inbox,
                         transport=relay.RelayHostTransport())
    host.start()
    lobby_id = host._transport.lobby_id
    guests = []
    for name in ("Ann", "Bob"):
        inbox = Inbox()
        client = net.LobbyClient(board, "", 0, name, "", inbox,
                                 connector=lambda: relay.connect_guest(lobby_id))
        client.start()
        inbox.wait("welcome")
        guests.append((client, inbox))
    host.start_game()
    (ann, ann_inbox), (bob, bob_inbox) = guests
    ann_inbox.wait("state")
    pid = ann.player_id
    ann._conn.close()  # the network goes away under Ann
    host_inbox.wait("lobby", lambda d: any(p["id"] == pid and p["away"] for p in d["players"]))
    assert ann_inbox.wait("resumed") == pid
    state = ann_inbox.wait("state")["state"]
    assert not state.player(pid)["left"]
    host_inbox.wait("lobby", lambda d: all(not p["away"] for p in d["players"]))
    if host.chat_on:
        assert ann.send_chat("still here") is None
        assert bob_inbox.wait("chat")["text"] == "still here"
    ann.close(); bob.close()
    host.stop()


@LIVE
def test_relay_forwards_only_game_messages(board):
    """The relay refuses traffic that isn't Timezone Dominator's protocol, so it can't be
    used as a general-purpose message relay."""
    lobby_id, token, _features = relay.create_lobby("Protocol test", "Host", board)
    host, _ = relay._websocket(f"/v1/lobbies/{lobby_id}/host",
                            headers=[f"Authorization: Bearer {token}"])
    try:
        guest, _ = relay._websocket(f"/v1/lobbies/{lobby_id}/join")
        assert json.loads(host.recv()) == {"relay": "open", "peer": 1}
        guest.send(net.encode({"type": "hello", "name": "Ann"}).decode().strip())
        frame = json.loads(host.recv())
        assert frame["relay"] == "data" and json.loads(frame["data"])["type"] == "hello"
        # Host-to-player data that isn't a game message is dropped...
        host.send(json.dumps({"relay": "send", "peer": 1, "data": "not a game message"}))
        host.send(json.dumps({"relay": "send", "peer": 1,
                              "data": net.encode({"type": "welcome", "player": 1}).decode().strip()}))
        assert json.loads(guest.recv())["type"] == "welcome"
        # ...and a player sending anything else is disconnected.
        guest.send(json.dumps({"hello": "world"}))
        assert guest.recv() == ""
        assert json.loads(host.recv()) == {"relay": "close", "peer": 1}
    finally:
        host.close()


@LIVE
def test_the_relay_advertises_chat(board):
    lobby_id, token, features = relay.create_lobby("Feature test", "Host", board)
    host, _ = relay._websocket(f"/v1/lobbies/{lobby_id}/host",
                            headers=[f"Authorization: Bearer {token}"])
    host.close()
    assert "chat" in features


@LIVE
def test_encrypted_chat_works_online(board):
    import weeknumber_tzd_chat as chat
    if not chat.available():
        pytest.skip("PyNaCl is not installed")
    host_inbox = Inbox()
    host = net.LobbyHost(board, "Hostess", "Chat test", host_inbox,
                         transport=relay.RelayHostTransport())
    host.start()
    assert host.chat_on
    lobby_id = host._transport.lobby_id
    inbox = Inbox()
    guest = net.LobbyClient(board, "", 0, "Ann", "", inbox,
                            connector=lambda: relay.connect_guest(lobby_id))
    guest.start()
    assert inbox.wait("secure", lambda d: d["state"] == "secure")["code"] == host.safety_code
    host_inbox.wait("lobby", lambda d: all(p["chat"] for p in d["players"]) and len(d["players"]) == 2)
    assert guest.send_chat("hello via the relay") is None
    assert host_inbox.wait("chat")["text"] == "hello via the relay"
    assert host.say("hello back")
    assert inbox.wait("chat") == {"from": 0, "name": "Hostess", "text": "hello back"}
    host.stop()


@LIVE
def test_relay_chat_limits_drop_before_they_kick(board):
    lobby_id, token, _features = relay.create_lobby("Chat limits", "Host", board)
    host, _ = relay._websocket(f"/v1/lobbies/{lobby_id}/host",
                            headers=[f"Authorization: Bearer {token}"])
    try:
        guest, _ = relay._websocket(f"/v1/lobbies/{lobby_id}/join")
        assert json.loads(host.recv()) == {"relay": "open", "peer": 1}
        box = "A" * 200
        for n in range(25):  # over the soft limit of 20 in 10 s: the rest is dropped
            guest.send(net.encode({"type": "chat", "n": n, "box": box}).decode().strip())
        guest.send(net.encode({"type": "action", "action": {"type": "end_turn"}}).decode().strip())
        forwarded = []
        while True:
            frame = json.loads(host.recv())
            kind = json.loads(frame["data"])["type"]
            forwarded.append(kind)
            if kind == "action":
                break
        assert forwarded.count("chat") == 20  # dropped quietly, and the game still flows
        guest.send(net.encode({"type": "chat", "n": 99, "box": "A" * 4000}).decode().strip())
        assert guest.recv() == ""  # an oversized chat box disconnects
        assert json.loads(host.recv()) == {"relay": "close", "peer": 1}
    finally:
        host.close()
