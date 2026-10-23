"""IT policy (docs/ADMINISTRATION.md) and the network gate. The parsing is
portable; the registry and managed-preference readers only supply raw
values."""

import os
import subprocess
import sys
import textwrap

import pytest

import weeknumber_core as core
import weeknumber_tzd_net as net
import weeknumber_tzd_relay as relay
from weeknumber_platform import _common as c


class FakePolicy(c.PolicyStore):
    def __init__(self, flags=None, texts=None):
        self.flags = flags or {}
        self.texts = texts or {}

    def flag(self, name):
        return self.flags.get(name)

    def text(self, name):
        return self.texts.get(name)


def test_no_policy_allows_everything():
    assert c.parse_policy(FakePolicy()) == c.Policy()


@pytest.mark.parametrize(("raw", "on"), [
    (1, True), (True, True), (7, True),
    (0, False), (False, False), ("1", False), (None, False), (1.0, False),
])
def test_switches_take_numbers_and_booleans_only(raw, on):
    policy = c.parse_policy(FakePolicy(flags={"DisableOnline": raw}))
    assert policy.disable_online is on


def test_each_switch_maps_to_its_field():
    policy = c.parse_policy(FakePolicy(flags={name: 1 for name in c.POLICY_FLAGS}))
    assert policy == c.Policy(True, True, True, True, None)


@pytest.mark.parametrize(("raw", "url"), [
    ("https://relay.example.com", "https://relay.example.com"),
    ("  https://relay.example.com/  ", "https://relay.example.com"),
    ("http://relay.example.com", None),             # never unencrypted
    ("https://user:pw@relay.example.com", None),
    ("https://relay.example.com/?x=1", None),
    ("https://", None),
    ("relay.example.com", None),
    ("https://" + "a" * 300 + ".com", None),
])
def test_relay_url_must_be_a_plain_https_address(raw, url):
    assert c.parse_policy(FakePolicy(texts={"RelayURL": raw})).relay_url == url


def test_a_broken_store_falls_back_to_the_defaults():
    class Broken(c.PolicyStore):
        def flag(self, name):
            raise OSError("registry unavailable")
    assert c.parse_policy(Broken()) == c.Policy()


def test_disable_online_and_relay_url_reach_relay_url(monkeypatch):
    monkeypatch.delenv(relay.RELAY_ENV, raising=False)
    monkeypatch.setattr(relay, "policy", lambda: c.Policy(relay_url="https://corp.example"))
    assert relay.relay_url() == "https://corp.example"
    monkeypatch.setattr(relay, "policy", lambda: c.Policy(disable_online=True,
                                                          relay_url="https://corp.example"))
    assert relay.relay_url() is None and not relay.online_available()


def test_disable_lan_refuses_hosting_and_discovery(monkeypatch):
    import weeknumber_tzd_rules as rules
    monkeypatch.setattr(net, "policy", lambda: c.Policy(disable_lan=True))
    assert not net.lan_available()
    board = rules.load_board()
    assert net.discover_lobbies(board, timeout=0.1) == []
    host = net.LobbyHost(board, "Host", "Lobby", lambda kind, data: None, port=0)
    with pytest.raises(net.PolicyBlocked):
        host.start()


_GATE_SCRIPT = textwrap.dedent("""
    import os, socket, sys, tempfile
    sys.path.insert(0, sys.argv[1])
    from weeknumber_core import network
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.bind(("127.0.0.1", 0))      # before the gate exists
    server.listen(1)
    port = server.getsockname()[1]
    network.install()
    results = []
    def attempt(name, action):
        try:
            action()
            results.append(name + "=ok")
        except PermissionError:
            results.append(name + "=blocked")
    def connect():
        with socket.create_connection(("127.0.0.1", port), timeout=2):
            pass
    def lookup():
        socket.getaddrinfo("localhost", 80)
    def bind():
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", 0))
    def udp():
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.sendto(b"x", ("127.0.0.1", 9))
    def local():
        if not hasattr(socket, "AF_UNIX"):
            return
        path = os.path.join(tempfile.mkdtemp(), "s")
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.bind(path)
    for name, action in (("connect", connect), ("lookup", lookup), ("bind", bind),
                         ("udp", udp), ("local", local)):
        attempt(name, action)
    network.allow()
    attempt("allowed", connect)
    network.deny()
    attempt("again", connect)
    print(" ".join(results))
""")


def test_the_gate_blocks_every_internet_socket_until_allowed():
    """In a fresh process, as the app runs: install() is irreversible."""
    result = subprocess.run([sys.executable, "-c", _GATE_SCRIPT, os.path.dirname(core.__file__)],
                            capture_output=True, text=True, timeout=60, env=dict(os.environ))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split() == [
        "connect=blocked", "lookup=blocked", "bind=blocked", "udp=blocked", "local=ok",
        "allowed=ok", "again=blocked"]


def test_the_gate_starts_closed_and_the_lobby_closes_it():
    gate = core.NetworkGate()
    assert not gate.is_open
    gate.allow()
    assert gate.is_open
    gate.deny()
    assert not gate.is_open
    with pytest.raises(PermissionError):
        gate._audit("socket.getaddrinfo", ("example.com", 443, 0, 0, 0, 0))
    gate._audit("socket.__new__", (None, 2, 1, 0))  # making a socket is not traffic
    gate._audit("open", ("file", "r", 0))


def test_no_network_setting_is_remembered(tmp_path, monkeypatch):
    """The question is asked every time Timezone Dominator opens: nothing about it is saved."""
    monkeypatch.setattr(core, "SETTINGS_DIR", tmp_path)
    monkeypatch.setattr(core, "SETTINGS_FILE", tmp_path / "settings.json")
    settings = core.SettingsStore()
    assert not hasattr(settings, "has_network_consent")
    assert not hasattr(core, "network_consent_token")


def test_sound_setting_defaults_on_and_persists(tmp_path, monkeypatch):
    monkeypatch.setattr(core, "SETTINGS_DIR", tmp_path)
    monkeypatch.setattr(core, "SETTINGS_FILE", tmp_path / "settings.json")
    settings = core.SettingsStore()
    assert settings.sound_enabled
    settings.sound_enabled = False
    settings.save()
    assert core.SettingsStore().sound_enabled is False


def test_read_policy_works_on_this_machine():
    # Whatever this machine's policy is, reading it must not raise.
    assert isinstance(core.read_policy(), c.Policy)


_GAME_SCRIPT = textwrap.dedent("""
    import queue, random, sys
    sys.path.insert(0, sys.argv[1])
    from weeknumber_core import network
    import weeknumber_tzd_net as net
    import weeknumber_tzd_rules as rules
    network.install()
    board = rules.load_board()
    events = queue.Queue()
    def wait(kind):
        while True:
            got, data = events.get(timeout=10)
            if got == kind:
                return data
    try:
        net.LobbyHost(board, "H", "L", lambda k, d: None, port=0, discovery_port=0).start()
        print("host=started")
    except PermissionError:
        print("host=blocked")
    network.allow()
    host = net.LobbyHost(board, "H", "L", lambda k, d: None, port=0, discovery_port=0,
                         rng=random.Random(1))
    host.start()
    client = net.LobbyClient(board, "127.0.0.1", host.port, "Ann", host.code,
                             lambda k, d: events.put((k, d)))
    client.start()
    print("welcome=%s" % wait("welcome"))
    network.deny()
    late = net.LobbyClient(board, "127.0.0.1", host.port, "Bob", host.code,
                           lambda k, d: events.put(("late-" + k, d)))
    late.start()
    print("late=" + ("blocked" if "Network access is off" in wait("late-closed") else "?"))
    client.close(); host.stop()
""")


def test_tzd_connects_only_while_the_gate_is_open():
    result = subprocess.run([sys.executable, "-c", _GAME_SCRIPT, os.path.dirname(core.__file__)],
                            capture_output=True, text=True, timeout=60, env=dict(os.environ))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split() == ["host=blocked", "welcome=1", "late=blocked"], result.stdout
