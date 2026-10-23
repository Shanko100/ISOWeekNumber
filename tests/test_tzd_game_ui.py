"""Game-window pieces that need no window: the sounds and the move log."""

import io
import wave
from types import SimpleNamespace

import pytest

import weeknumber_tzd_rules as rules
import weeknumber_tzd_sound as sound
from weeknumber_tzd_dialog import TzdGameWindow


@pytest.mark.parametrize("name", sound.NAMES)
def test_every_sound_is_a_short_quiet_wav(name):
    data = sound.sound(name)
    with wave.open(io.BytesIO(data)) as clip:
        assert (clip.getnchannels(), clip.getsampwidth(), clip.getframerate()) == (1, 2, sound.RATE)
        seconds = clip.getnframes() / clip.getframerate()
        frames = clip.readframes(clip.getnframes())
    assert 0.1 < seconds < 1.5
    peak = max(abs(int.from_bytes(frames[i:i + 2], "little", signed=True))
               for i in range(0, len(frames), 2))
    assert peak <= sound.VOLUME * 32767 + 1


def test_play_hands_the_wav_to_the_platform(monkeypatch):
    played = []
    monkeypatch.setattr(sound, "play_sound", played.append)
    sound.play("capture")
    assert played == [sound.sound("capture")]
    sound.release()
    assert sound._cache == {}


@pytest.fixture
def window():
    """Just enough of a game window for its log methods."""
    board = rules.load_board()
    names = {0: "Dana", 1: "Bob", 2: "Carla"}
    stub = SimpleNamespace(board=board, session=SimpleNamespace(my_id=0), _after_blitz=False)
    stub._name = lambda pid: "You" if pid == 0 else names.get(pid, "?")
    stub._territory_name = lambda tid: board.territories[tid].name if tid in board.territories else "?"
    stub._describe = lambda event: TzdGameWindow._describe(stub, event)
    stub.log_lines = []
    return stub


EVENTS = [
    {"type": "start"},
    {"type": "turn", "player": 1, "reinforcements": 3},
    {"type": "place", "player": 1, "t": "egypt", "n": 3},
    {"type": "attack", "player": 1, "defender": 0, "from": "egypt", "to": "east_africa",
     "att": [6, 3, 1], "def": [6, 2], "att_loss": 1, "def_loss": 1},
    {"type": "blitz", "player": 1, "from": "egypt", "to": "east_africa", "rolls": 2,
     "att_loss": 1, "def_loss": 2},
    {"type": "capture", "player": 1, "from": "egypt", "to": "east_africa", "defender": 0},
    {"type": "occupy", "player": 1, "from": "egypt", "to": "east_africa", "n": 3},
    {"type": "eliminated", "player": 2, "by": 1, "cards": 0},
    {"type": "trade", "player": 0, "armies": 4, "bonus_at": None},
    {"type": "fortify", "player": 0, "from": "brazil", "to": "peru", "n": 2},
    {"type": "card", "player": 0},
    {"type": "left", "player": 2},
    {"type": "win", "player": 1},
]


@pytest.mark.parametrize("event", EVENTS, ids=lambda e: e["type"])
def test_every_move_gets_a_log_line_with_the_player_in_colour(window, event):
    line = window._describe(event)
    assert line, event["type"]
    if "player" in event:
        assert line[0] == (window._name(event["player"]), event["player"])


def test_a_blitz_logs_once_although_its_final_roll_follows(window):
    blitz, final = EVENTS[4], EVENTS[3]
    TzdGameWindow._record(window, [blitz, final])
    assert len(window.log_lines) == 1
    assert "blitzed" in window.log_lines[0][1][0]


def test_you_are_addressed_as_you(window):
    line = window._describe({"type": "turn", "player": 0, "reinforcements": 1})
    assert line[0] == ("You", 0) and line[1][0].startswith(" are up")
    assert "1 army" in line[1][0]


def test_sounds_render_in_the_background():
    sound.release()
    import threading
    before = set(threading.enumerate())
    sound.prepare()
    for thread in set(threading.enumerate()) - before:
        thread.join(timeout=30)
    assert set(sound._cache) == set(sound.NAMES)
    sound.release()


def _roughness(name: str) -> float:
    """Mean sample-to-sample change over the mean level: about 2 pi f / RATE
    for a pure tone of f Hz, so it grows with how bright a sound is."""
    with wave.open(io.BytesIO(sound.sound(name))) as clip:
        frames = clip.readframes(clip.getnframes())
    samples = [int.from_bytes(frames[i:i + 2], "little", signed=True)
               for i in range(0, len(frames), 2)]
    level = sum(abs(s) for s in samples) / len(samples)
    change = sum(abs(samples[i] - samples[i - 1]) for i in range(1, len(samples)))
    return change / len(samples) / level


@pytest.mark.parametrize("name", sound.NAMES)
def test_sounds_are_soft_not_shrill(name):
    # No brighter than a pure 800 Hz tone: warm mallet notes and dull
    # knocks. The old sine bells and noise clicks measured up to twice that.
    import math
    assert _roughness(name) < 2 * math.pi * 800 / sound.RATE
