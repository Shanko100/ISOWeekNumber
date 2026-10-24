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

The primitives come from libsodium through PyNaCl (X25519 and the IETF
ChaCha20-Poly1305 of RFC 8439); HKDF is RFC 5869 over the standard library's
HMAC-SHA256. The result is byte for byte what the earlier `cryptography`
build produced (see test_the_wire_format_is_pinned), so copies built either
way talk to each other. PyNaCl ships for Windows x64 and ARM64 and for macOS.

This module is pure: no Tk, no sockets. PyNaCl is imported on first use, so
the app starts without it and a missing library only means "Chat
unavailable", never plain text.
"""

from __future__ import annotations

import base64
import binascii
import functools
import hashlib
import hmac
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
NONCE_BYTES = 12
TAG_BYTES = 16
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
    """libsodium's primitives through PyNaCl, imported once on first use."""
    try:
        from nacl import bindings, exceptions
        if (bindings.crypto_scalarmult_BYTES != KEY_BYTES
                or bindings.crypto_aead_chacha20poly1305_ietf_KEYBYTES != KEY_BYTES
                or bindings.crypto_aead_chacha20poly1305_ietf_NPUBBYTES != NONCE_BYTES):
            raise ImportError("unexpected libsodium parameters")
    except Exception:
        log.warning("The PyNaCl library is unavailable; chat is off")
        return None
    return SimpleNamespace(
        scalarmult=bindings.crypto_scalarmult,
        scalarmult_base=bindings.crypto_scalarmult_base,
        seal=bindings.crypto_aead_chacha20poly1305_ietf_encrypt,
        unseal=bindings.crypto_aead_chacha20poly1305_ietf_decrypt,
        random=bindings.randombytes,
        Error=exceptions.CryptoError)


def available() -> bool:
    """Whether chat can run: the library loads and IT policy allows it."""
    return not policy().disable_chat and _crypto() is not None


def hkdf_sha256(secret: bytes, salt: bytes, info: bytes, length: int) -> bytes:
    """HKDF (RFC 5869) with HMAC-SHA256: extract, then expand to `length`
    bytes. Checked against the RFC's test vectors in test_tzd_chat."""
    if not 0 < length <= 255 * hashlib.sha256().digest_size:
        raise ValueError("invalid HKDF length")
    key = hmac.digest(salt or bytes(hashlib.sha256().digest_size), secret, "sha256")
    out, block, counter = b"", b"", 1
    while len(out) < length:
        block = hmac.digest(key, block + info + bytes((counter,)), "sha256")
        out += block
        counter += 1
    return out[:length]


class KeyPair:
    """An X25519 key pair; the private half never leaves this object."""

    def __init__(self, private: bytes) -> None:
        if not isinstance(private, bytes) or len(private) != KEY_BYTES:
            raise ValueError("an X25519 private key is 32 bytes")
        self._private = private
        self.public = _crypto().scalarmult_base(private)

    @classmethod
    def generate(cls) -> "KeyPair":
        """A fresh key from the OS's random source (libsodium's randombytes)."""
        return cls(_crypto().random(KEY_BYTES))

    @classmethod
    def from_private_bytes(cls, data: bytes) -> "KeyPair":
        """For tests: a fixed key gives reproducible boxes."""
        return cls(bytes(data))

    @property
    def public_b64(self) -> str:
        return base64.b64encode(self.public).decode("ascii")

    def exchange(self, peer_public: bytes) -> "bytes | None":
        """The shared secret, or None for a key that yields no secret (the
        low-order points X25519 maps to all zeros, which libsodium refuses)."""
        if not isinstance(peer_public, bytes) or len(peer_public) != KEY_BYTES:
            return None
        try:
            shared = _crypto().scalarmult(self._private, peer_public)
        except Exception:
            return None
        return None if hmac.compare_digest(shared, bytes(KEY_BYTES)) else shared


class _Aead:
    """ChaCha20-Poly1305 (IETF) with one fixed key: box = ciphertext + tag."""

    __slots__ = ("_key", "_seal", "_unseal")

    def __init__(self, key: bytes) -> None:
        if len(key) != KEY_BYTES:
            raise ValueError("a ChaCha20-Poly1305 key is 32 bytes")
        crypto = _crypto()
        self._key, self._seal, self._unseal = key, crypto.seal, crypto.unseal

    def encrypt(self, nonce: bytes, plain: bytes, aad: bytes) -> bytes:
        return self._seal(plain, aad, nonce, self._key)

    def decrypt(self, nonce: bytes, box: bytes, aad: bytes) -> bytes:
        if len(box) < TAG_BYTES:
            raise ValueError("box shorter than its tag")
        return self._unseal(box, aad, nonce, self._key)


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
        keys = hkdf_sha256(
            shared, _SALT,
            b"keys" + host_public + guest_public + seat_id.to_bytes(4, "big"),
            2 * KEY_BYTES)
        to_guest, to_host = keys[:KEY_BYTES], keys[KEY_BYTES:]
        send, receive = (to_guest, to_host) if role == "host" else (to_host, to_guest)
        self._sender = _Aead(send)
        self._receiver = _Aead(receive)
        self._invalid = crypto.Error
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
        box = self._sender.encrypt(n.to_bytes(NONCE_BYTES, "big"), plain,
                                   _AAD + kind.encode("ascii"))
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
            plain = self._receiver.decrypt(n.to_bytes(NONCE_BYTES, "big"), raw,
                                           _AAD + kind.encode("ascii"))
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
