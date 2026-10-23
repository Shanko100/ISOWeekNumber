"""Getting back into a game after the connection dropped, over real
localhost connections: with a secured channel and with the one-time token,
the host keeping the seat, and everyone who must not get a seat back."""

import random
import socket

import pytest

import weeknumber_tzd_chat as chat
import weeknumber_tzd_net as net
import weeknumber_tzd_rules as rules
from test_tzd_net import TIMEOUT, Inbox, free_udp_port


@pytest.fixture(scope="module")
def board():
    return rules.load_board()


@pytest.fixture(params=["secured", "token"])
def mode(request, monkeypatch):
    if request.param == "secured" and not chat.available():
        pytest.skip("cryptography is not installed")
    if request.param == "token":
        monkeypatch.setattr(chat, "available", lambda: False)
    monkeypatch.setattr(net, "RECONNECT_BACKOFF_S", (0.2, 0.3))
    return request.param


def make_host(board):
    inbox = Inbox()
    host = net.LobbyHost(board, "Hostess", "Comeback", inbox, port=0,
                         discovery_port=free_udp_port(), rng=random.Random(3))
    host.start()
    host.inbox = inbox
    return host


@pytest.fixture
def host(board, mode):
    lobby = make_host(board)
    yield lobby
    lobby.stop()


def join(board, host, name):
    inbox = Inbox()
    client = net.LobbyClient(board, "127.0.0.1", host.port, name, host.code, inbox)
    client.inbox = inbox
    client.start()
    return client


def started(board, host, *names):
    clients = [join(board, host, name) for name in names]
    for client in clients:
        client.inbox.wait("welcome")
    host.inbox.wait("lobby", lambda d: len(d["players"]) == len(names) + 1
                    and all(p["chat"] for p in d["players"]) or not host.chat_on)
    host.start_game()
    for client in clients:
        client.inbox.wait("state")
    return clients


def drop(client):
    """The network goes away under the player: no goodbye is sent."""
    client._conn.close()


def away(host, pid, value):
    return host.inbox.wait("lobby", lambda d: any(
        p["id"] == pid and p["away"] is value for p in d["players"]))


def test_a_dropped_player_gets_their_seat_back(board, host, mode):
    ann, bob = started(board, host, "Ann", "Bob")
    pid = ann.player_id
    drop(ann)
    away(host, pid, True)
    assert ann.inbox.wait("reconnecting")["attempt"] == 1
    assert ann.inbox.wait("resumed") == pid
    away(host, pid, False)
    state = ann.inbox.wait("state")["state"]
    assert not state.player(pid)["left"] and state.player(pid)["alive"]
    assert ann.player_id == pid
    # Still that player: their moves count, and with chat on it still works.
    if host.chat_on:
        assert ann.send_chat("back again") is None
        assert bob.inbox.wait("chat")["text"] == "back again"
    ann.send_action({"type": "concede"})
    state = bob.inbox.wait("state", lambda d: any(
        e.get("type") == "conceded" for e in d["events"]))["state"]
    assert state.player(pid)["conceded"]
    ann.close(); bob.close()


def test_a_second_drop_is_survived_too(board, host, mode):
    ann, bob = started(board, host, "Ann", "Bob")
    for _ in range(2):
        drop(ann)
        assert ann.inbox.wait("resumed") == ann.player_id
    ann.close(); bob.close()


def test_the_seat_is_given_up_when_the_player_does_not_return(board, host, mode, monkeypatch):
    monkeypatch.setattr(net, "RECONNECT_GRACE_S", 0.4)
    raw = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
    raw.sendall(net.encode({"type": "hello", "name": "Gone", "code": host.code,
                            "board": board.fingerprint}))
    welcome = net.decode(raw.makefile("rb").readline())
    bob = join(board, host, "Bob")
    bob.inbox.wait("welcome")
    host.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    host.start_game()
    bob.inbox.wait("state")
    raw.close()
    away(host, welcome["player"], True)
    state = bob.inbox.wait("state", lambda d: any(
        e.get("type") == "left" for e in d["events"]))["state"]
    assert state.player(welcome["player"])["left"]
    bob.close()


def test_leaving_on_purpose_frees_the_seat_at_once(board, host, mode, monkeypatch):
    monkeypatch.setattr(net, "RECONNECT_GRACE_S", 60)
    ann, bob = started(board, host, "Ann", "Bob")
    ann.close()  # says goodbye
    state = bob.inbox.wait("state", lambda d: any(
        e.get("type") == "left" for e in d["events"]))["state"]
    assert state.player(ann.player_id)["left"]
    bob.close()


def _resume_hello(board, host, resume, key=None):
    hello = {"type": "hello", "name": "Mallory", "code": host.code,
             "board": board.fingerprint, "resume": resume}
    if key is not None:
        hello.update({"chat": chat.PROTOCOL, "key": key})
    sock = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
    sock.sendall(net.encode(hello))
    reply = net.decode(sock.makefile("rb").readline())
    sock.close()
    return reply


def test_nobody_else_can_take_a_dropped_seat(board, host, mode, monkeypatch):
    monkeypatch.setattr(net, "WRONG_CODE_DELAY_S", 0)
    monkeypatch.setattr(net, "RECONNECT_BACKOFF_S", (30,))  # Ann stays away
    monkeypatch.setattr(net, "MAX_CODE_FAILURES", 100)       # counted, but not capped here
    monkeypatch.setattr(net, "MAX_TOTAL_FAILURES", 100)
    ann, bob = started(board, host, "Ann", "Bob")
    pid = ann.player_id
    drop(ann)
    away(host, pid, True)
    attempts = [{"player": pid, "token": "x" * 32},
                {"player": pid, "n": 99, "box": "AAAA"},
                {"player": pid},
                {"player": 0, "token": "x" * 32},        # the host's own seat
                {"player": "1"}, "not a dict"]
    if host.chat_on:
        seat = next(s for s in host._seats if s["id"] == pid)
        stranger = chat.KeyPair.generate()
        link = chat.ChatLink(stranger, host._chat_keys.public, pid, "guest")
        forged = {"player": pid, **link.seal("resume", {"k": "resume", "player": pid})}
        replies = [_resume_hello(board, host, forged, key=stranger.public_b64),
                   _resume_hello(board, host, forged, key=seat["key"])]
    else:
        replies = []
    replies += [_resume_hello(board, host, attempt) for attempt in attempts]
    assert all(r["type"] == "bye" for r in replies), replies
    with host._lock:
        seat = next(s for s in host._seats if s["id"] == pid)
        assert seat["conn"] is None and seat["away"]
    assert not host.state.player(pid)["left"]
    assert host._failure_total >= 3  # every refused proof counts like a wrong code
    ann.close(); bob.close()


def test_no_seat_comes_back_before_the_game_or_after_it(board, host, mode):
    reply = _resume_hello(board, host, {"player": 1, "token": "x" * 32})
    assert reply["type"] == "bye"  # no game yet


def test_the_app_gives_up_reconnecting_in_time_and_can_try_again(board, mode, monkeypatch):
    monkeypatch.setattr(net, "RECONNECT_WINDOW_S", 1.2)
    lobby = make_host(board)
    ann, bob = started(board, lobby, "Ann", "Bob")
    drop(ann)
    away(lobby, ann.player_id, True)
    lobby.stop()  # the host is gone for good while Ann is away
    reason = ann.inbox.wait("closed")
    assert "did not work in time" in reason
    assert ann.can_reconnect
    assert ann.reconnect()
    assert "did not work in time" in ann.inbox.wait("closed")
    ann.close(); bob.close()


def test_a_flooding_player_is_disconnected_and_loses_the_seat(board, host, mode, monkeypatch):
    monkeypatch.setattr(net, "RECONNECT_GRACE_S", 60)
    raw = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
    raw.sendall(net.encode({"type": "hello", "name": "Flood", "code": host.code,
                            "board": board.fingerprint}))
    welcome = net.decode(raw.makefile("rb").readline())
    bob = join(board, host, "Bob")
    bob.inbox.wait("welcome")
    host.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    host.start_game()
    bob.inbox.wait("state")
    try:
        raw.sendall(net.encode({"type": "chat"}) * (net.PEER_BURST * 3))
    except OSError:
        pass
    state = bob.inbox.wait("state", lambda d: any(
        e.get("type") == "left" for e in d["events"]))["state"]
    assert state.player(welcome["player"])["left"]  # no grace for a flood
    raw.close(); bob.close()


def test_guessing_the_code_from_many_places_is_capped(board, host, mode, monkeypatch):
    monkeypatch.setattr(net, "WRONG_CODE_DELAY_S", 0)
    monkeypatch.setattr(net, "MAX_CODE_FAILURES", 1000)  # as if from many addresses
    monkeypatch.setattr(net, "MAX_TOTAL_FAILURES", 3)
    wrong = "0000" if host.code != "0000" else "1111"
    for _ in range(3):
        sock = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
        sock.sendall(net.encode({"type": "hello", "name": "Guess", "code": wrong,
                                 "board": board.fingerprint}))
        assert net.decode(sock.makefile("rb").readline())["type"] == "bye"
        sock.close()
    sock = socket.create_connection(("127.0.0.1", host.port), timeout=TIMEOUT)
    sock.sendall(net.encode({"type": "hello", "name": "Late", "code": host.code,
                             "board": board.fingerprint}))
    # The lobby takes no more tries: it closes the connection unanswered.
    # Closing with the hello still unread makes the OS reset the connection,
    # which Windows reports as an error and Linux and macOS often as EOF.
    try:
        reply = sock.makefile("rb").readline()
    except ConnectionError:
        reply = b""
    assert reply == b""
    sock.close()


@pytest.mark.skipif(not chat.available(), reason="cryptography is not installed")
def test_the_player_checks_that_the_same_host_answered():
    host_keys, guest_keys = chat.KeyPair.generate(), chat.KeyPair.generate()
    host_link = chat.ChatLink(host_keys, guest_keys.public, 1, "host")
    client = net.LobbyClient(rules.load_board(), "127.0.0.1", 1, "Ann", "1234", Inbox())
    client.player_id = 1
    client._link = chat.ChatLink(guest_keys, host_keys.public, 1, "guest")

    def welcome(payload):
        return {"type": "welcome", "player": 1, "resumed": True,
                **host_link.seal("welcome", payload)}

    good = welcome({"k": "resumed", "player": 1})
    assert client._accept_resumed(good)
    assert not client._accept_resumed(good)                       # replayed
    assert not client._accept_resumed(welcome({"k": "resumed", "player": 2}))
    assert not client._accept_resumed(welcome({"k": "ready"}))
    assert not client._accept_resumed({"type": "welcome", "player": 1, "resumed": True})
    stranger = chat.ChatLink(chat.KeyPair.generate(), guest_keys.public, 1, "host")
    assert not client._accept_resumed({"type": "welcome", "player": 1, "resumed": True,
                                       **stranger.seal("welcome", {"k": "resumed", "player": 1})})
    later = welcome({"k": "resumed", "player": 1})
    assert client._accept_resumed(later)


def test_the_token_is_replaced_on_every_return():
    client = net.LobbyClient(rules.load_board(), "127.0.0.1", 1, "Ann", "1234", Inbox())
    client.player_id = 1
    assert not client._accept_resumed({"type": "welcome", "player": 1, "resumed": True})
    assert not client._accept_resumed({"type": "welcome", "player": 1, "resumed": True,
                                       "resume_token": "short"})
    token = "t" * 32
    assert client._accept_resumed({"type": "welcome", "player": 1, "resumed": True,
                                   "resume_token": token})
    assert client._resume_token == token


def test_a_game_the_host_ended_offers_no_reconnection(board, mode):
    lobby = make_host(board)
    ann, bob = started(board, lobby, "Ann", "Bob")
    lobby.stop()  # says goodbye to everyone
    assert "closed the game" in ann.inbox.wait("closed")
    assert not ann.can_reconnect and not ann.reconnect()
    ann.close(); bob.close()
