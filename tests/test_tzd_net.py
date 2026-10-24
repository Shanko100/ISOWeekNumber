import queue
import random
import socket
import time

import pytest

import weeknumber_tzd_net as net
import weeknumber_tzd_rules as rules

TIMEOUT = 5.0


@pytest.fixture(scope="module")
def board():
    loaded = rules.load_board()
    assert loaded is not None
    return loaded


class Inbox:
    """Collects (kind, data) callbacks from network threads."""

    def __init__(self):
        self.items = queue.Queue()

    def __call__(self, kind, data):
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


def free_udp_port():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@pytest.fixture
def host(board):
    inbox = Inbox()
    lobby = net.LobbyHost(board, "Hostess", "Test lobby", inbox, port=0,
                          discovery_port=free_udp_port(), rng=random.Random(11))
    lobby.start()
    lobby.inbox = inbox
    yield lobby
    lobby.stop()


def join(board, host, name, code=None):
    inbox = Inbox()
    client = net.LobbyClient(board, "127.0.0.1", host.port, name,
                             host.code if code is None else code, inbox)
    client.inbox = inbox
    client.start()
    return client


def test_clients_join_and_see_the_lobby(board, host):
    ann = join(board, host, "Ann")
    assert ann.inbox.wait("welcome") == 1
    bob = join(board, host, "Bob\x00‮")
    lobby = bob.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    assert [p["name"] for p in lobby["players"]] == ["Hostess", "Ann", "Bob"]
    assert len({p["color"] for p in lobby["players"]}) == 3
    assert "code" not in lobby
    host_view = host.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    assert host_view["code"] == host.code
    ann.close()
    bob.close()


def test_wrong_code_is_refused(board, host):
    wrong = "0000" if host.code != "0000" else "1111"
    intruder = join(board, host, "Mallory", code=wrong)
    assert intruder.inbox.wait("closed") == "Wrong lobby code."


def test_a_code_with_other_characters_is_refused_and_counted(board, host):
    # hmac.compare_digest raises TypeError for non-ASCII text; the attempt
    # must still be answered and counted like any wrong code.
    intruder = join(board, host, "Mallory", code="\u0661\u0662\u0663\u0664")
    assert intruder.inbox.wait("closed") == "Wrong lobby code."
    assert host._failure_total == 1


@pytest.mark.parametrize("given, expected, same", [
    ("1234", "1234", True),
    ("1235", "1234", False),
    ("\u00e9", "1234", False),
    ("\ud800", "1234", False),   # a lone surrogate, which JSON can carry
    (None, "1234", False),
    (["1234"], "1234", False),
    ("1234", None, False),
])
def test_same_secret(given, expected, same):
    assert net._same_secret(given, expected) is same


def test_different_board_is_refused(board, host):
    inbox = Inbox()
    other = rules.Board.from_document(_modified_board_document())
    client = net.LobbyClient(other, "127.0.0.1", host.port, "Eve", host.code, inbox)
    client.start()
    assert "different WeekNumber version" in inbox.wait("closed")


def _modified_board_document():
    import json
    from pathlib import Path
    path = Path(rules.__file__).resolve().parent / "tzd_data.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["continents"][0]["bonus"] += 1
    return document


def test_a_game_runs_over_the_wire(board, host):
    ann = join(board, host, "Ann")
    ann.inbox.wait("welcome")
    host.inbox.wait("lobby", lambda d: len(d["players"]) == 2)
    host.start_game()
    first = ann.inbox.wait("state")["state"]
    assert first.owner == host.state.owner
    assert "deck" not in host.state.to_dict(viewer=1)

    # Whoever is first places all reinforcements through their own channel.
    current = host.state.current_player["id"]
    target = host.state.territories_of(current)[0]
    action = {"type": "place", "t": target, "n": host.state.reinforcements}
    expected = host.state.armies[target] + host.state.reinforcements
    if current == net.LobbyHost.HOST_ID:
        host.submit(current, action)
    else:
        ann.send_action(action)
    after = ann.inbox.wait("state", lambda d: d["state"].phase == rules.PHASE_ATTACK)
    assert after["state"].armies[target] == expected
    assert after["events"][0]["type"] == "place"

    # Acting out of turn is answered with an error, not a state change.
    other = 1 if current == net.LobbyHost.HOST_ID else net.LobbyHost.HOST_ID
    if other == 1:
        ann.send_action({"type": "end_turn"})
        assert "not your turn" in ann.inbox.wait("error")
    else:
        with pytest.raises(rules.IllegalAction):
            host.submit(other, {"type": "end_turn"})
    ann.close()


def test_malformed_and_oversized_messages_disconnect(board, host):
    for payload in (b"{not json}\n", b"x" * (net.MAX_MESSAGE_BYTES + 10)):
        sock = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
        sock.sendall(net.encode({"type": "hello", "name": "Raw", "code": host.code,
                                 "board": board.fingerprint}))
        host.inbox.wait("lobby", lambda d: any(p["name"] == "Raw" for p in d["players"]))
        sock.sendall(payload)
        host.inbox.wait("lobby", lambda d: all(p["name"] != "Raw" for p in d["players"]))
        sock.close()


def test_disconnect_mid_game_marks_the_player_as_left(board, host):
    ann = join(board, host, "Ann")
    ann_id = ann.inbox.wait("welcome")
    bob = join(board, host, "Bob")
    host.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    host.start_game()
    bob.inbox.wait("state")
    ann.close()
    state = bob.inbox.wait("state", lambda d: any(
        e.get("type") == "left" for e in d["events"]))["state"]
    assert state.player(ann_id)["left"]
    assert state.current_player["id"] != ann_id
    bob.close()


def test_discovery_finds_the_lobby(board, host):
    found = net.discover_lobbies(board, timeout=1.0, port=host._discovery_port)
    match = [f for f in found if f["port"] == host.port]
    assert match
    assert match[0]["name"] == "Test lobby"
    assert match[0]["compatible"] is True
    assert "code" not in match[0]


@pytest.mark.parametrize(("raw", "expected"), [
    ("  Ann  ", "Ann"), ("A\nB\tC", "ABC"), ("", "Player"), (None, "Player"),
    ("x" * 100, "x" * rules.MAX_NAME_CHARS),
])
def test_clean_name(raw, expected):
    assert net.clean_name(raw) == expected


def test_decode_rejects_wrong_version_and_shapes():
    with pytest.raises(net.ProtocolError):
        net.decode(b'{"v":2,"type":"hello"}')
    with pytest.raises(net.ProtocolError):
        net.decode(b'[1,2,3]')
    with pytest.raises(net.ProtocolError):
        net.decode(b'{"v":1}')
    assert net.decode(net.encode({"type": "x"}).strip()) == {"type": "x", "v": 1}


def test_a_local_close_ends_a_waiting_read_promptly():
    # macOS may not wake a thread blocked on a socket that another thread
    # shuts down; the read must notice `closed` itself, well before the
    # socket's 10-second send timeout.
    import threading
    with socket.create_server(("127.0.0.1", 0)) as server:
        client = socket.create_connection(server.getsockname(), timeout=TIMEOUT)
        peer, _ = server.accept()
        conn = net._Connection(client)
        result = []
        reader = threading.Thread(target=lambda: result.append(conn.receive()), daemon=True)
        reader.start()
        time.sleep(0.2)
        conn.closed = True  # as if close() had not woken the reader
        reader.join(timeout=2)
        try:
            assert not reader.is_alive() and result == [None]
        finally:
            client.close()
            peer.close()


def _pair(server):
    client = socket.create_connection(server.getsockname(), timeout=TIMEOUT)
    peer, _ = server.accept()
    peer.settimeout(TIMEOUT)
    return net._Connection(client), peer


def _start_reader(conn):
    import threading
    got = []

    def read():
        while (message := conn.receive()) is not None:
            got.append(message)
        conn.close()  # what LobbyHost._serve and LobbyClient._session do
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    return reader, got


def test_a_goodbye_arrives_although_messages_are_still_coming_in():
    # Windows resets a socket shut down or closed while data is still queued
    # or arriving, and a reset can throw the goodbye away on the other side.
    # finish() shuts down only the sending side and lets the reader close
    # once the peer has closed too.
    with socket.create_server(("127.0.0.1", 0)) as server:
        conn, peer = _pair(server)
        reader, got = _start_reader(conn)
        try:
            peer.sendall(net.encode({"type": "lobby"}) * 50)
            conn.finish({"type": "bye"})
            assert not conn.closed  # the reader closes it, not finish()
            lines = peer.makefile("rb")
            assert net.decode(lines.readline())["type"] == "bye"
            assert lines.readline() == b""  # then the end of the stream
            peer.sendall(net.encode({"type": "state"}))  # still arriving: discarded
            lines.close()  # the file holds the socket open
            peer.close()
            reader.join(timeout=2)
            assert not reader.is_alive() and conn.closed
            assert {"type": "state"} not in [{"type": m["type"]} for m in got]
        finally:
            peer.close()
            conn.close()


def test_after_a_goodbye_nothing_more_is_sent():
    with socket.create_server(("127.0.0.1", 0)) as server:
        conn, peer = _pair(server)
        reader, _ = _start_reader(conn)
        try:
            conn.finish({"type": "bye"})
            conn.finish({"type": "bye"})  # a second goodbye is not sent
            assert conn.send({"type": "action", "action": {}}) is False
            with peer.makefile("rb") as lines:
                assert net.decode(lines.readline())["type"] == "bye"
                assert lines.readline() == b""
        finally:
            peer.close()
            reader.join(timeout=2)
            conn.close()


def test_a_peer_that_never_closes_is_left_after_the_linger(monkeypatch):
    monkeypatch.setattr(net, "GOODBYE_LINGER_S", 0.3)
    with socket.create_server(("127.0.0.1", 0)) as server:
        conn, peer = _pair(server)
        reader, _ = _start_reader(conn)
        try:
            started = time.monotonic()
            conn.finish({"type": "bye"})
            reader.join(timeout=3)
            assert not reader.is_alive() and conn.closed
            assert time.monotonic() - started < 2
        finally:
            peer.close()
            conn.close()


def test_a_goodbye_on_a_closed_connection_does_nothing():
    with socket.create_server(("127.0.0.1", 0)) as server:
        conn, peer = _pair(server)
        try:
            conn.close()
            conn.finish({"type": "bye"})
            assert peer.recv(100) == b""
        finally:
            peer.close()
