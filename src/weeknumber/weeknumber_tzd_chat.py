"""End-to-end encrypted chat for Timezone Dominator lobbies and games.

Chat follows the game's star topology: every message goes between a player
and the host, and the host passes it on to the others. Each host-player pair
shares a key that only the two of them can derive, so the online relay,
Cloudflare and anyone on the network see who sent a message, when, and its
size in 64-byte steps, but never its text. The host takes part in the game
and can read everything; anyone who joins the lobby reads chat from then on.

Safety code. Every player sees a fingerprint of the host's key: the host
shows its own, a player the key its welcome message proved. When everyone's
four groups match the host's, nobody sits in between. Forging a matching
code means searching about 2^53 real key pairs, far beyond a relay's reach
while a lobby is open.

Wire format (inside weeknumber_tzd_net's JSON messages):

* hello and welcome carry "chat": PROTOCOL (2) and a "key" (base64 X25519 public key);
  welcome also carries a sealed {"k": "ready"} proving the host's key.
* chat messages are {"type": "chat", "n": counter, "box": base64}. Players
  seal {"k": "ready"} (counter 0) and then {"k": "say", "text"}; the host
  seals {"k": "say", "from", "name", "text"}, stamping the sender itself.
* reconnecting: the returning player's hello carries a "resume" box sealed
  {"k": "resume", "player"} on the same channel, and the host answers with a
  "welcome" box {"k": "resumed", "player"}. The counters keep rising across
  the reconnection, so neither box can be replayed, and neither the relay
  nor anyone on the network can produce one.

Keys: X25519 per lobby (host) and per join (player), from the OS's random
source, never saved. HKDF-SHA256 derives one ChaCha20-Poly1305 key per
direction; nonces are the per-direction message counter, which must rise,
so a replayed or reordered box is refused. Chat text is never written to
disk or the log.

This module is pure: no Tk, no sockets. `cryptography` is imported on first
use, so the app starts without it and a missing library only means "Chat
unavailable", never plain text.
"""

from __future__ import annotations

import base64
import binascii
import functools
import hashlib
import json
import threading
import time
import unicodedata
from types import SimpleNamespace

from weeknumber_core import log, policy

#: The "chat" marker in hello and welcome; a later scheme gets a new number.
#: 2 since the key-derivation labels below were renamed: a copy
#: with the other number gets "Chat unavailable" instead of a channel whose
#: keys cannot match (which would look like interference).
PROTOCOL = 2
KEY_BYTES = 32
KEY_B64_CHARS = 44
MAX_TEXT_CHARS = 280
#: Sealed content is padded to a multiple of this, so a box's size gives away
#: only roughly how long a message is.
PAD_BYTES = 64
MAX_PLAIN_BYTES = 1536
MAX_BOX_CHARS = 2100
#: Counters stay within what JSON and the relay's JavaScript count exactly.
MAX_COUNTER = 2 ** 53 - 1
#: After this many unopenable boxes the UI warns that chat may be tampered with.
MAX_FAILURES = 3
#: What a box may be sealed as; the kind is authenticated with the box. A
#: "resume" box proves a returning player holds the key their seat was
#: secured with (see weeknumber_tzd_net, reconnecting).
KINDS = ("welcome", "chat", "resume")

# Key-derivation labels: part of the wire protocol (see PROTOCOL). Changing
# them changes every key and safety code, so it needs a new PROTOCOL number.
_SALT = b"TZD chat v1"
_AAD = b"TZD chat v1 "
_SAFETY = b"TZD safety v1"


@functools.lru_cache(maxsize=1)
def _crypto() -> "SimpleNamespace | None":
    """The cryptography primitives, imported once on first use."""
    try:
        from cryptography.exceptions import InvalidTag
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric.x25519 import (
            X25519PrivateKey,
            X25519PublicKey,
        )
        from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
        from cryptography.hazmat.primitives.kdf.hkdf import HKDF
    except Exception:
        log.warning("The cryptography library is unavailable; chat is off")
        return None
    return SimpleNamespace(
        InvalidTag=InvalidTag, hashes=hashes, serialization=serialization,
        X25519PrivateKey=X25519PrivateKey, X25519PublicKey=X25519PublicKey,
        ChaCha20Poly1305=ChaCha20Poly1305, HKDF=HKDF)


def available() -> bool:
    """Whether chat can run: the library loads and IT policy allows it."""
    return not policy().disable_chat and _crypto() is not None


class KeyPair:
    """An X25519 key pair; the private half never leaves this object."""

    def __init__(self, private) -> None:
        crypto = _crypto()
        self._private = private
        self.public = private.public_key().public_bytes(
            crypto.serialization.Encoding.Raw, crypto.serialization.PublicFormat.Raw)

    @classmethod
    def generate(cls) -> "KeyPair":
        return cls(_crypto().X25519PrivateKey.generate())

    @classmethod
    def from_private_bytes(cls, data: bytes) -> "KeyPair":
        """For tests: a fixed key gives reproducible boxes."""
        return cls(_crypto().X25519PrivateKey.from_private_bytes(data))

    @property
    def public_b64(self) -> str:
        return base64.b64encode(self.public).decode("ascii")

    def exchange(self, peer_public: bytes) -> "bytes | None":
        """The shared secret, or None for a key that yields no secret (the
        low-order points X25519 maps to all zeros)."""
        crypto = _crypto()
        try:
            shared = self._private.exchange(crypto.X25519PublicKey.from_public_bytes(peer_public))
        except ValueError:
            return None
        return None if shared == bytes(KEY_BYTES) else shared


def parse_public_key(value: object, own: "bytes | None" = None) -> "bytes | None":
    """A peer's key from the wire: exactly 44 characters of base64 giving 32
    bytes, and not our own key reflected back."""
    if not isinstance(value, str) or len(value) != KEY_B64_CHARS:
        return None
    try:
        key = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError):
        return None
    if len(key) != KEY_BYTES or key == own:
        return None
    return key


def safety_code(host_public: bytes) -> str:
    """The host's key as four groups of four digits (about 53 bits)."""
    digest = hashlib.sha256(_SAFETY + host_public).digest()
    digits = f"{int.from_bytes(digest[:8], 'big') % 10 ** 16:016d}"
    return " ".join(digits[i:i + 4] for i in range(0, 16, 4))


class ChatLink:
    """One end of a host-player channel.

    Sending is shared between threads, so callers hold `send_lock` across
    `seal` and the socket send: a box sealed later must never overtake an
    earlier one, or the receiver refuses the earlier as a replay. Receiving
    belongs to the one reader thread of that connection.
    """

    def __init__(self, own: KeyPair, peer_public: bytes, seat_id: int, role: str) -> None:
        crypto = _crypto()
        shared = own.exchange(peer_public)
        if shared is None:
            raise ValueError("unusable peer key")
        if role == "host":
            host_public, guest_public = own.public, peer_public
        else:
            host_public, guest_public = peer_public, own.public
        keys = crypto.HKDF(
            algorithm=crypto.hashes.SHA256(), length=2 * KEY_BYTES, salt=_SALT,
            info=b"keys" + host_public + guest_public + seat_id.to_bytes(4, "big"),
        ).derive(shared)
        to_guest, to_host = keys[:KEY_BYTES], keys[KEY_BYTES:]
        send, receive = (to_guest, to_host) if role == "host" else (to_host, to_guest)
        self._sender = crypto.ChaCha20Poly1305(send)
        self._receiver = crypto.ChaCha20Poly1305(receive)
        self._invalid = crypto.InvalidTag
        self.host_public = host_public
        self.send_lock = threading.Lock()
        self._next = 0
        self._last = -1
        #: Host side: the welcome went out, and the player proved its key.
        self.ready = False
        self.confirmed = False
        #: Boxes refused, and counters skipped (messages that never arrived).
        self.failures = 0
        self.gaps = 0

    def seal(self, kind: str, payload: dict) -> dict:
        """{"n", "box"} for `payload`. Hold `send_lock` until it is sent."""
        plain = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if len(plain) > MAX_PLAIN_BYTES:
            raise ValueError("chat message too large")
        plain += b" " * (-len(plain) % PAD_BYTES)
        n = self._next
        self._next += 1
        box = self._sender.encrypt(n.to_bytes(12, "big"), plain, _AAD + kind.encode("ascii"))
        return {"n": n, "box": base64.b64encode(box).decode("ascii")}

    def open(self, kind: str, n: object, box: object) -> "dict | None":
        """The payload of a received box, or None (counted) when it is
        malformed, forged, replayed or out of order."""
        payload = self._open(kind, n, box)
        if payload is None:
            self.failures += 1
        return payload

    def _open(self, kind: str, n: object, box: object) -> "dict | None":
        if kind not in KINDS or not isinstance(n, int) or isinstance(n, bool):
            return None
        if not self._last < n <= MAX_COUNTER:
            return None
        if not isinstance(box, str) or not 0 < len(box) <= MAX_BOX_CHARS:
            return None
        try:
            raw = base64.b64decode(box, validate=True)
            plain = self._receiver.decrypt(n.to_bytes(12, "big"), raw, _AAD + kind.encode("ascii"))
            payload = json.loads(plain.decode("utf-8"))
        except (binascii.Error, ValueError, UnicodeDecodeError, self._invalid):
            return None
        if not isinstance(payload, dict):
            return None
        # Advance only after the tag checked out, so a forged high counter
        # cannot block the real messages that follow.
        if n > self._last + 1:
            self.gaps += 1
        self._last = n
        return payload


def clean_text(value: object) -> "str | None":
    """Chat text fit to show: control and formatting characters (bidi
    overrides included) dropped, line breaks as spaces, at most 280
    characters. None when nothing is left."""
    if not isinstance(value, str):
        return None
    text = value.replace("\r", " ").replace("\n", " ").replace("\t", " ")
    text = "".join(ch for ch in text if not unicodedata.category(ch).startswith("C"))
    text = " ".join(text.split())[:MAX_TEXT_CHARS]
    return text or None


class TokenBucket:
    """`capacity` messages at once, refilled by one every `refill_s` seconds."""

    def __init__(self, capacity: int, refill_s: float, clock=time.monotonic) -> None:
        self.capacity = capacity
        self.refill_s = refill_s
        self._clock = clock
        self._tokens = float(capacity)
        self._at = clock()

    def allow(self) -> bool:
        now = self._clock()
        self._tokens = min(self.capacity, self._tokens + (now - self._at) / self.refill_s)
        self._at = now
        if self._tokens < 1:
            return False
        self._tokens -= 1
        return True
