"""Short sound effects for the Timezone Dominator game window.

The sounds are synthesised here, in plain Python, so there are no audio files
to ship or license. Each is rendered once (about 40 KB of 16-bit mono WAV;
prepare() does it in the background when a game opens) and played by the
platform layer without blocking the window. The
player mutes them with the game window's Sound switch
(SettingsStore.sound_enabled).

The palette is a soft wooden mallet rather than pure sine bells: a marimba
voice (a fundamental with the bar's 4x and 10x overtones, which die away
much faster than the note), a felt-mallet thump at the start of each note, a
low-pass that takes the digital edge off, and a little room around it all.
Notes sit in the middle register, the intervals are consonant (fourths,
fifths, a major chord), and every sound is short and quiet.
"""

from __future__ import annotations

import io
import math
import random
import sys
import threading
import wave
from array import array

from weeknumber_core import log, play_sound

RATE = 22050
#: Peak level as a fraction of full scale: game sounds stay well below
#: everything else on the computer.
VOLUME = 0.28
#: Longest sound, tail included, in seconds.
MAX_SECONDS = 1.4

_cache: dict[str, bytes] = {}

# Note frequencies (equal temperament, A4 = 440 Hz).
D4, F4, G4, A4 = 293.66, 349.23, 392.00, 440.00
C5, D5, E5, G5 = 523.25, 587.33, 659.26, 783.99
C4, E4 = 261.63, 329.63


def _samples(ms: float) -> int:
    return int(RATE * ms / 1000)


def _mallet(freq: float, ms: float, decay: float = 7.0, brightness: float = 1.0,
            rng: "random.Random | None" = None) -> list[float]:
    """One marimba-like note: the bar's fundamental plus its 4x and 10x
    partials, each with its own exponential decay, and a few milliseconds of
    soft noise for the felt mallet touching the bar."""
    rng = rng or random.Random(int(freq))
    count = _samples(ms)
    attack = max(1, _samples(3))
    thump = _samples(6)
    tau = math.tau
    partials = ((1.0, 1.0, decay), (4.0, 0.22 * brightness, decay * 3.2),
                (10.0, 0.05 * brightness, decay * 7.0))
    out = []
    noise = 0.0
    for i in range(count):
        t = i / RATE
        rise = min(1.0, i / attack)
        value = 0.0
        for ratio, level, rate in partials:
            if freq * ratio < RATE / 2.2:  # nothing near or above Nyquist
                value += level * math.exp(-rate * t) * math.sin(tau * freq * ratio * t)
        if i < thump:
            noise = 0.7 * noise + 0.3 * rng.uniform(-1.0, 1.0)
            value += 0.35 * noise * (1.0 - i / thump)
        out.append(rise * value)
    return out


def _knock(ms: float, rng: random.Random, tone: float, level: float = 1.0) -> list[float]:
    """A die landing on a wooden table: a dull knock with a short rattle."""
    count = _samples(ms)
    out, low = [], 0.0
    for i in range(count):
        t = i / RATE
        low = 0.8 * low + 0.2 * rng.uniform(-1.0, 1.0)  # dark, felt-like noise
        body = math.sin(math.tau * tone * t) + 0.4 * math.sin(math.tau * tone * 2.7 * t)
        out.append(level * (math.exp(-70 * t) * 0.8 * body + math.exp(-120 * t) * 0.9 * low))
    return out


def _mix(*tracks: "tuple[float, list[float], float]") -> list[float]:
    """Overlay (start ms, samples, gain) tracks."""
    length = max(_samples(start) + len(track) for start, track, _gain in tracks)
    out = [0.0] * length
    for start, track, gain in tracks:
        offset = _samples(start)
        for i, value in enumerate(track):
            out[offset + i] += gain * value
    return out


def _lowpass(samples: list[float], cutoff: float) -> list[float]:
    """One-pole low-pass: rounds off the top end the way air and a small
    speaker would."""
    k = 1.0 - math.exp(-math.tau * cutoff / RATE)
    out, state = [], 0.0
    for value in samples:
        state += k * (value - state)
        out.append(state)
    return out


def _room(samples: list[float], wet: float = 0.16) -> list[float]:
    """A little space around the sound: three short feedback echoes at
    mutually prime delays, low enough to read as a room, not an echo."""
    tail = _samples(240)
    dry = samples + [0.0] * tail
    out = list(dry)
    for delay_ms, feedback in ((29.7, 0.42), (37.1, 0.38), (43.9, 0.34)):
        delay = _samples(delay_ms)
        line = [0.0] * len(dry)
        for i in range(len(dry)):
            echo = line[i - delay] if i >= delay else 0.0
            line[i] = dry[i] + feedback * echo
            out[i] += wet / 3.0 * echo
    return out


def _finish(samples: list[float]) -> list[float]:
    """Cut to MAX_SECONDS and fade the last 60 ms, so no sound ends in a
    click or runs long."""
    samples = samples[:int(RATE * MAX_SECONDS)]
    fade = min(len(samples), _samples(60))
    for i in range(fade):
        samples[len(samples) - fade + i] *= 1.0 - (i + 1) / fade
    return samples


def _wav(samples: list[float]) -> bytes:
    peak = max((abs(s) for s in samples), default=0.0) or 1.0
    scale = VOLUME * 32767 / peak
    pcm = array("h", (int(s * scale) for s in samples))
    if sys.byteorder == "big":  # WAV samples are little-endian
        pcm.byteswap()
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(pcm.tobytes())
    return buffer.getvalue()


def _dice() -> list[float]:
    """Two or three dice tumbling to a stop: knocks that come closer
    together and quieter, low in pitch."""
    rng = random.Random(6)
    tracks, at, level = [], 0.0, 1.0
    for n in range(7):
        tracks.append((at, _knock(70, rng, rng.uniform(190, 310), level), 1.0))
        at += rng.uniform(38, 62) * (1 - n * 0.06)
        level *= 0.82
    return _mix(*tracks)


def _render(name: str) -> list[float]:
    if name == "dice":
        return _finish(_room(_lowpass(_dice(), 2600), wet=0.1))
    if name == "turn":       # a friendly rising fourth: your move
        dry = _mix((0, _mallet(D5, 600), 0.9), (120, _mallet(G5, 700), 1.0))
    elif name == "capture":  # a low open fifth, landing firmly
        dry = _mix((0, _mallet(G4, 650, decay=6.0), 1.0),
                   (0, _mallet(D5, 550, decay=7.5), 0.55),
                   (70, _mallet(G5, 450, decay=9.0, brightness=0.6), 0.35))
    elif name == "trade":    # a quick, light major arpeggio
        dry = _mix((0, _mallet(C5, 420, decay=8.0), 0.8),
                   (70, _mallet(E5, 420, decay=8.0), 0.8),
                   (140, _mallet(G5, 600, decay=6.5), 0.9))
    elif name == "win":      # a rolled C major chord that rings out
        dry = _mix((0, _mallet(C4, 1100, decay=3.2), 0.8),
                   (60, _mallet(E4, 1050, decay=3.4), 0.7),
                   (120, _mallet(G4, 1000, decay=3.6), 0.7),
                   (180, _mallet(C5, 950, decay=3.8), 0.75),
                   (260, _mallet(E5, 900, decay=4.0, brightness=0.8), 0.6))
    elif name == "lose":     # a gentle falling minor line, no alarm
        dry = _mix((0, _mallet(A4, 500, decay=6.5, brightness=0.6), 0.8),
                   (200, _mallet(F4, 500, decay=6.5, brightness=0.6), 0.75),
                   (400, _mallet(D4, 700, decay=5.0, brightness=0.5), 0.8))
    else:
        raise KeyError(name)
    return _finish(_room(_lowpass(dry, 4200)))


NAMES = ("dice", "capture", "turn", "trade", "win", "lose")


def sound(name: str) -> bytes:
    """The named sound as WAV bytes, rendered on first use."""
    data = _cache.get(name)
    if data is None:
        data = _cache[name] = _wav(_render(name))
    return data


def prepare() -> None:
    """Render every sound on a background thread, so the first capture or
    win does not hold the window up for the 50-150 ms the synthesis takes.
    Each finished sound lands in the cache in one assignment; play() renders
    anything still missing itself."""
    missing = [name for name in NAMES if name not in _cache]
    if not missing:
        return

    def work() -> None:
        for name in missing:
            try:
                sound(name)
            except Exception:
                log.debug("Could not render the %s sound", name, exc_info=True)

    threading.Thread(target=work, name="tzd-sounds", daemon=True).start()


def play(name: str) -> None:
    try:
        play_sound(sound(name))
    except Exception:
        log.debug("Could not play the %s sound", name, exc_info=True)


def release() -> None:
    """Forget the rendered sounds (the game window closed)."""
    _cache.clear()
