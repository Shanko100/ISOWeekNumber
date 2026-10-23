"""When the die's board turns back into the map (TrayIcon._open_tzd).
Tk-free: the windows are stand-ins."""

from types import SimpleNamespace

import pytest

import weeknumber_tzd_dialog
import weeknumber_tzd_lobby
from weeknumber_core import network
from weeknumber_tray import TrayIcon


class _Backdrop:
    def __init__(self):
        self.ended = 0

    def end_tzd(self, game_started=False):
        self.ended += 1


class _Lobby:
    def __init__(self, backdrop):
        self.backdrop = backdrop

    def is_over(self, backdrop):
        return self.backdrop is backdrop


@pytest.fixture
def tzd(monkeypatch):
    state = {"game": False, "lobby": None, "opens": True, "shown": 0,
             "answer": True, "asked": 0}
    game = SimpleNamespace(raise_open=lambda: state["game"])

    def show(*args, backdrop=None, **kwargs):
        state["shown"] += 1
        if state["opens"]:
            state["lobby"] = _Lobby(backdrop)

    lobby = SimpleNamespace(_live=lambda: state["lobby"],
                            raise_open=lambda: state["lobby"] is not None,
                            show=show)
    # _open_tzd imports the windows when it runs, from their own modules.
    monkeypatch.setattr(weeknumber_tzd_dialog, "TzdGameWindow", game)
    monkeypatch.setattr(weeknumber_tzd_lobby, "TzdLobbyDialog", lobby)
    monkeypatch.setattr(weeknumber_tzd_lobby, "network_needed", lambda: True)

    def confirm(backdrop, on_result):
        state["asked"] += 1
        on_result(state["answer"])

    tray = SimpleNamespace(app=SimpleNamespace(root=None, settings=None, run_on_ui=None),
                           _confirm_network=confirm)
    state["open"] = lambda backdrop: TrayIcon._open_tzd(tray, backdrop)
    network.deny()
    yield state
    network.deny()


def test_die_opens_the_lobby_over_the_board(tzd):
    backdrop = _Backdrop()
    tzd["open"](backdrop)
    assert tzd["shown"] == 1 and tzd["lobby"].is_over(backdrop)
    assert backdrop.ended == 0


def test_die_again_keeps_the_board_under_its_own_lobby(tzd):
    backdrop = _Backdrop()
    tzd["lobby"] = _Lobby(backdrop)
    tzd["open"](backdrop)
    assert tzd["shown"] == 0 and backdrop.ended == 0


@pytest.mark.parametrize("elsewhere", ["game", "lobby"])
def test_board_turns_back_when_tzd_is_already_open(tzd, elsewhere):
    if elsewhere == "game":
        tzd["game"] = True
    else:
        tzd["lobby"] = _Lobby(_Backdrop())
    backdrop = _Backdrop()
    tzd["open"](backdrop)
    assert tzd["shown"] == 0 and backdrop.ended == 1


def test_board_turns_back_when_the_lobby_cannot_open(tzd):
    tzd["opens"] = False
    backdrop = _Backdrop()
    tzd["open"](backdrop)
    assert tzd["shown"] == 1 and backdrop.ended == 1


def test_the_network_question_is_asked_every_time(tzd):
    for _ in range(2):
        tzd["lobby"] = None
        tzd["open"](_Backdrop())
    assert tzd["asked"] == 2 and tzd["shown"] == 2
    assert network.is_open  # allowed for the lobby that is open now


def test_declining_keeps_the_network_closed_and_opens_nothing(tzd):
    tzd["answer"] = False
    backdrop = _Backdrop()
    tzd["open"](backdrop)
    assert tzd["asked"] == 1 and tzd["shown"] == 0 and backdrop.ended == 1
    assert not network.is_open


def test_a_lobby_that_cannot_open_closes_the_network_again(tzd):
    tzd["opens"] = False
    tzd["open"](_Backdrop())
    assert not network.is_open
