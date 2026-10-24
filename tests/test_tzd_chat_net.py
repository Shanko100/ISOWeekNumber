"""Encrypted chat over real localhost connections: the handshake, the host
passing messages on, older players, interception and limits."""

import json
import random
import socket
import threading

import pytest

import weeknumber_tzd_chat as chat
import weeknumber_tzd_net as net
import weeknumber_tzd_rules as rules
from test_tzd_net import Inbox, free_udp_port

pytestmark = pytest.mark.skipif(not chat.available(), reason="PyNaCl is not installed")


@pytest.fixture(scope="module")
def board():
    return rules.load_board()


@pytest.fixture
def host(board):
    inbox = Inbox()
    lobby = net.LobbyHost(board, "Hostess", "Chat lobby", inbox, port=0,
                          discovery_port=free_udp_port(), rng=random.Random(5))
    lobby.start()
    lobby.inbox = inbox
    yield lobby
    lobby.stop()


def join(board, port, name, code):
    inbox = Inbox()
    client = net.LobbyClient(board, "127.0.0.1", port, name, code, inbox)
    client.inbox = inbox
    client.start()
    return client


def secured(client):
    return client.inbox.wait("secure", lambda d: d["state"] == "secure")


def all_chat_ready(host, count):
    return host.inbox.wait("lobby", lambda d: len(d["players"]) == count
                           and all(p["chat"] for p in d["players"]))


def test_everyone_sees_the_hosts_safety_code(board, host):
    ann = join(board, host.port, "Ann", host.code)
    bob = join(board, host.port, "Bob", host.code)
    assert secured(ann)["code"] == host.safety_code == secured(bob)["code"]
    all_chat_ready(host, 3)
    ann.close(); bob.close()


def test_messages_reach_everyone_else_with_the_hosts_stamp(board, host):
    ann = join(board, host.port, "Ann", host.code)
    bob = join(board, host.port, "Bob", host.code)
    secured(ann); secured(bob)
    all_chat_ready(host, 3)
    assert ann.send_chat("hello there") is None
    said = {"from": ann.player_id, "name": "Ann", "text": "hello there"}
    assert host.inbox.wait("chat") == said  # ids follow the join order, which races
    assert bob.inbox.wait("chat") == said
    with pytest.raises(AssertionError):
        ann.inbox.wait("chat", timeout=0.5)  # no echo to the sender
    assert host.say("welcome all")
    for guest in (ann, bob):
        assert guest.inbox.wait("chat") == {"from": 0, "name": "Hostess", "text": "welcome all"}
    ann.close(); bob.close()


def test_an_older_player_without_a_key_never_gets_chat(board, host):
    ann = join(board, host.port, "Ann", host.code)
    secured(ann)
    old = socket.create_connection(("127.0.0.1", host.port), timeout=3)
    old.sendall(net.encode({"type": "hello", "name": "Old", "code": host.code,
                            "board": board.fingerprint}))
    reader = old.makefile("rb")
    welcome = net.decode(reader.readline())
    assert (welcome["type"], welcome["player"]) == ("welcome", 2)
    assert "chat" not in welcome and "box" not in welcome
    assert isinstance(welcome["resume_token"], str)  # its way back after a drop
    host.inbox.wait("lobby", lambda d: len(d["players"]) == 3)
    assert ann.send_chat("can the old one read this?") is None
    host.inbox.wait("chat")
    old.settimeout(0.8)
    kinds = []
    try:
        while True:
            line = reader.readline()
            if not line:
                break
            kinds.append(net.decode(line)["type"])
    except (socket.timeout, OSError):
        pass
    assert "chat" not in kinds
    old.close(); ann.close()


class Proxy:
    """A man in the middle on the TCP path: `to_host` and `to_guest` may
    rewrite each message."""

    def __init__(self, port, to_host, to_guest):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        self.target = port
        self.to_host, self.to_guest = to_host, to_guest
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self):
        guest, _ = self.listener.accept()
        upstream = socket.create_connection(("127.0.0.1", self.target))
        threading.Thread(target=self._pump, args=(guest, upstream, self.to_host), daemon=True).start()
        threading.Thread(target=self._pump, args=(upstream, guest, self.to_guest), daemon=True).start()

    @staticmethod
    def _pump(source, sink, rewrite):
        reader = source.makefile("rb")
        try:
            for line in reader:
                sink.sendall(net.encode(rewrite(net.decode(line))))
        except OSError:
            pass

    def close(self):
        self.listener.close()


def stranger_key():
    return chat.KeyPair.generate().public_b64


@pytest.mark.parametrize("swap", ["hello", "welcome"])
def test_a_swapped_key_leaves_chat_unsecured(board, host, swap):
    def to_host(m):
        return {**m, "key": stranger_key()} if m["type"] == "hello" and swap == "hello" else m

    def to_guest(m):
        return {**m, "key": stranger_key()} if m["type"] == "welcome" and swap == "welcome" else m

    proxy = Proxy(host.port, to_host, to_guest)
    ann = join(board, proxy.port, "Ann", host.code)
    assert ann.inbox.wait("secure")["state"] == "failed"
    with pytest.raises(AssertionError):  # the host never confirms Ann's chat
        host.inbox.wait("lobby", lambda d: any(p["id"] == 1 and p["chat"] for p in d["players"]),
                        timeout=0.8)
    assert ann.send_chat("secret") is not None
    ann.close(); proxy.close()


def test_a_full_interception_shows_a_different_safety_code(board, host):
    """The proxy holds its own keys and re-seals everything: chat would work,
    but the player's code no longer matches the host's."""
    middle = chat.KeyPair.generate()
    links = {}

    def to_host(m):
        if m["type"] == "hello":
            links["guest_pub"] = chat.parse_public_key(m["key"])
            return {**m, "key": middle.public_b64}
        if m["type"] == "chat":
            payload = links["guest"].open("chat", m["n"], m["box"])
            return {"v": 1, "type": "chat", **links["host"].seal("chat", payload)}
        return m

    def to_guest(m):
        if m["type"] == "welcome":
            host_pub = chat.parse_public_key(m["key"])
            links["host"] = chat.ChatLink(middle, host_pub, m["player"], "guest")
            links["guest"] = chat.ChatLink(middle, links["guest_pub"], m["player"], "host")
            payload = links["host"].open("welcome", m["n"], m["box"])
            return {**m, "key": middle.public_b64, **links["guest"].seal("welcome", payload)}
        return m

    proxy = Proxy(host.port, to_host, to_guest)
    ann = join(board, proxy.port, "Ann", host.code)
    code = secured(ann)["code"]
    assert code != host.safety_code
    ann.close(); proxy.close()


def test_concurrent_senders_arrive_complete_and_in_order(board, host, monkeypatch):
    monkeypatch.setattr(net.LobbyHost, "CHAT_BURST", 1000)
    guests = [join(board, host.port, name, host.code) for name in ("Ann", "Bob", "Cy")]
    for guest in guests:
        secured(guest)
    all_chat_ready(host, 4)
    per_sender = 30

    def chatter(guest):
        for i in range(per_sender):
            assert guest.send_chat(f"{guest.name} {i}") is None

    threads = [threading.Thread(target=chatter, args=(g,)) for g in guests]
    threads.append(threading.Thread(target=lambda: [host.say(f"Hostess {i}") for i in range(per_sender)]))
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    for guest in guests:
        seen = {}
        for _ in range(3 * per_sender):  # everyone else: two guests and the host
            message = guest.inbox.wait("chat", timeout=10)
            seen.setdefault(message["name"], []).append(message["text"])
        for name, texts in seen.items():
            assert texts == [f"{name} {i}" for i in range(per_sender)], name
        assert guest.name not in seen
    for guest in guests:
        guest.close()


def test_flooding_is_limited_without_disconnecting(board, host):
    ann = join(board, host.port, "Ann", host.code)
    secured(ann)
    all_chat_ready(host, 2)
    ann._chat_bucket = chat.TokenBucket(1000, 1.0)  # get past the client's own limit
    for i in range(12):
        ann.send_chat(f"spam {i}")
    assert "too quickly" in ann.inbox.wait("error")
    received = []
    while True:
        try:
            received.append(host.inbox.wait("chat", timeout=0.5)["text"])
        except AssertionError:
            break
    assert received == [f"spam {i}" for i in range(net.LobbyHost.CHAT_BURST)]
    ann.set_color(3)  # still connected
    host.inbox.wait("lobby", lambda d: any(p["id"] == 1 and p["color"] == 3 for p in d["players"]))
    ann.close()


def test_junk_chat_frames_are_ignored_not_fatal(board, host):
    ann = join(board, host.port, "Ann", host.code)
    secured(ann)
    all_chat_ready(host, 2)
    for junk in ({"type": "chat"}, {"type": "chat", "n": "x", "box": 5},
                 {"type": "chat", "n": 99, "box": "AAAA"}):
        ann._conn.send(junk)
    assert ann.send_chat("still fine") is None
    assert host.inbox.wait("chat")["text"] == "still fine"
    ann.close()


def test_chat_keeps_working_after_the_game_starts(board, host):
    ann = join(board, host.port, "Ann", host.code)
    secured(ann)
    all_chat_ready(host, 2)
    host.start_game()
    ann.inbox.wait("state")
    assert ann.send_chat("good luck") is None
    assert host.inbox.wait("chat")["text"] == "good luck"
    assert host.say("you too")
    assert ann.inbox.wait("chat")["text"] == "you too"
    ann.close()


def test_frames_never_carry_the_text(board, host):
    seen = []

    def record(m):
        seen.append(json.dumps(m))
        return m

    proxy = Proxy(host.port, record, record)
    ann = join(board, proxy.port, "Ann", host.code)
    secured(ann)
    all_chat_ready(host, 2)
    ann.send_chat("the eagle lands at noon")
    host.inbox.wait("chat")
    host.say("roger that")
    ann.inbox.wait("chat")
    assert not any("eagle" in frame or "roger" in frame for frame in seen)
    ann.close(); proxy.close()


def test_a_copy_with_the_older_chat_protocol_gets_no_chat_not_a_broken_one(board, host):
    old = socket.create_connection(("127.0.0.1", host.port), timeout=3)
    old.sendall(net.encode({"type": "hello", "name": "Old", "code": host.code,
                            "board": board.fingerprint, "chat": 1,
                            "key": chat.KeyPair.generate().public_b64}))
    welcome = net.decode(old.makefile("rb").readline())
    assert welcome["type"] == "welcome" and "chat" not in welcome and "box" not in welcome
    old.close()
