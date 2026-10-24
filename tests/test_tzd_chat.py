"""The chat cryptography (weeknumber_tzd_chat): keys, boxes, replay and
tamper protection, the safety code and the text cleaner."""

import base64

import pytest

import weeknumber_tzd_chat as chat
import weeknumber_tzd_net as net
from weeknumber_platform import _common

pytestmark = pytest.mark.skipif(not chat.available(), reason="PyNaCl is not installed")

HOST_PRIVATE = bytes(range(32))
GUEST_PRIVATE = bytes(range(32, 64))


def pair(seat=3):
    host = chat.KeyPair.from_private_bytes(HOST_PRIVATE)
    guest = chat.KeyPair.from_private_bytes(GUEST_PRIVATE)
    return (chat.ChatLink(host, guest.public, seat, "host"),
            chat.ChatLink(guest, host.public, seat, "guest"))


def test_x25519_matches_rfc_7748():
    alice = chat.KeyPair.from_private_bytes(bytes.fromhex(
        "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"))
    bob = chat.KeyPair.from_private_bytes(bytes.fromhex(
        "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb"))
    assert alice.public.hex() == "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
    assert bob.public.hex() == "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"
    shared = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
    assert alice.exchange(bob.public).hex() == shared == bob.exchange(alice.public).hex()


@pytest.mark.parametrize(("salt", "info", "length", "okm"), [
    # RFC 5869, appendix A.1 and A.3.
    (bytes(range(13)), bytes(range(0xF0, 0xFA)), 42,
     "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
     "34007208d5b887185865"),
    (b"", b"", 42,
     "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d"
     "9d201395faa4b61a96c8"),
])
def test_hkdf_matches_rfc_5869(salt, info, length, okm):
    assert chat.hkdf_sha256(b"\x0b" * 22, salt, info, length).hex() == okm


def test_chacha20_poly1305_matches_rfc_8439():
    # RFC 8439, section 2.8.2.
    key = bytes(range(0x80, 0xA0))
    nonce = bytes.fromhex("070000004041424344454647")
    aad = bytes.fromhex("50515253c0c1c2c3c4c5c6c7")
    plain = (b"Ladies and Gentlemen of the class of '99: If I could offer you "
             b"only one tip for the future, sunscreen would be it.")
    box = chat._Aead(key).encrypt(nonce, plain, aad)
    assert box[-16:].hex() == "1ae10b594f09e26a7e902ecbd0600691"
    assert box[:16].hex() == "d31a8d34648e60db7b86afbc53ef7ec2"
    assert chat._Aead(key).decrypt(nonce, box, aad) == plain


def test_a_short_or_low_order_input_is_refused():
    host, guest = pair()
    assert guest.open("chat", 0, base64.b64encode(b"x" * 15).decode()) is None
    own = chat.KeyPair.from_private_bytes(HOST_PRIVATE)
    assert own.exchange(bytes(32)) is None          # low-order point: no secret
    assert own.exchange(b"short") is None
    with pytest.raises(ValueError):
        chat.KeyPair.from_private_bytes(b"short")


def test_the_wire_format_is_pinned():
    # Changing derivation, nonces, padding or the code breaks this on purpose:
    # players with the same PROTOCOL must agree byte for byte. (Pinned anew
    # for PROTOCOL 2, when the labels were renamed.)
    assert chat.PROTOCOL == 2
    host = chat.KeyPair.from_private_bytes(HOST_PRIVATE)
    assert host.public_b64 == "j0DFrbaPJWJK5bIU6nZ6bslNgp09e14a0bpvPiE4KF8="
    assert chat.safety_code(host.public) == "4486 4059 6029 0508"
    to_guest, _ = pair()
    assert to_guest.seal("welcome", {"k": "ready"}) == {"n": 0, "box": (
        "MON0Ibo1zUIXhT65F5+pqJyu0EmLkN1pscgDLXrXtnXDelk2c5H58lPRS5ZaMt6I"
        "E6EsmMJaWI1EgbysjjtMmKiX5KfDGKwno5FQrE33QD8=")}


def test_boxes_open_in_both_directions_only_for_their_kind():
    host, guest = pair()
    sealed = host.seal("chat", {"k": "say", "text": "hi"})
    assert guest.open("chat", sealed["n"], sealed["box"]) == {"k": "say", "text": "hi"}
    back = guest.seal("chat", {"k": "say", "text": "yo"})
    assert host.open("welcome", back["n"], back["box"]) is None  # kind is authenticated
    later = guest.seal("chat", {"k": "say", "text": "again"})
    assert host.open("chat", later["n"], later["box"])["text"] == "again"


def test_replays_tampering_and_other_links_are_refused():
    host, guest = pair()
    first = host.seal("chat", {"k": "say", "text": "one"})
    assert guest.open("chat", first["n"], first["box"]) is not None
    assert guest.open("chat", first["n"], first["box"]) is None          # replay
    second = host.seal("chat", {"k": "say", "text": "two"})
    raw = bytearray(base64.b64decode(second["box"]))
    raw[5] ^= 1
    assert guest.open("chat", second["n"], base64.b64encode(raw).decode()) is None
    other_host, other_guest = pair(seat=4)                              # another seat's key
    stranger = other_host.seal("chat", {"k": "say", "text": "x"})
    assert guest.open("chat", second["n"] + 1, stranger["box"]) is None
    assert guest.failures == 3


def test_a_forged_high_counter_does_not_block_real_messages():
    host, guest = pair()
    assert guest.open("chat", chat.MAX_COUNTER, "AAAA") is None
    real = host.seal("chat", {"k": "say", "text": "still here"})
    assert guest.open("chat", real["n"], real["box"])["text"] == "still here"


def test_skipped_counters_are_reported_as_gaps():
    host, guest = pair()
    host.seal("chat", {"k": "say", "text": "lost"})                      # dropped on the way
    kept = host.seal("chat", {"k": "say", "text": "kept"})
    assert guest.open("chat", kept["n"], kept["box"]) is not None
    assert guest.gaps == 1


@pytest.mark.parametrize("n", [True, -1, 1.0, "1", None, chat.MAX_COUNTER + 1])
def test_bad_counters_are_refused(n):
    host, guest = pair()
    box = host.seal("chat", {"k": "say", "text": "x"})["box"]
    assert guest.open("chat", n, box) is None


def test_public_keys_are_parsed_strictly():
    host = chat.KeyPair.from_private_bytes(HOST_PRIVATE)
    good = host.public_b64
    assert chat.parse_public_key(good) == host.public
    assert chat.parse_public_key(good, own=host.public) is None          # reflected back
    for bad in (None, 42, True, good[:-1], good + "=", "!" * 44,
                base64.b64encode(bytes(31)).decode()):
        assert chat.parse_public_key(bad) is None


def test_a_low_order_key_gives_no_link():
    host = chat.KeyPair.from_private_bytes(HOST_PRIVATE)
    assert host.exchange(bytes(32)) is None
    with pytest.raises(ValueError):
        chat.ChatLink(host, bytes(32), 1, "host")


@pytest.mark.parametrize(("raw", "clean"), [
    ("hello", "hello"),
    ("  two\nlines\r\n  ", "two lines"),
    ("tab\there", "tab here"),
    ("evil‮right-to-left", "evilright-to-left"),
    ("bell\x07", "bell"),
    ("​", None),
    ("", None),
    (42, None),
    ("🙂" * 300, "🙂" * chat.MAX_TEXT_CHARS),
])
def test_clean_text(raw, clean):
    assert chat.clean_text(raw) == clean


def test_the_largest_message_fits_well_under_the_relay_cap():
    host, _ = pair()
    sealed = host.seal("chat", {"k": "say", "from": 5, "name": "🙂" * 24,
                                "text": "🙂" * chat.MAX_TEXT_CHARS})
    frame = net.encode({"type": "chat", "n": chat.MAX_COUNTER, "box": sealed["box"]})
    assert len(sealed["box"]) <= chat.MAX_BOX_CHARS
    assert len(frame) < 3 * 1024
    assert len(base64.b64decode(sealed["box"])) % chat.PAD_BYTES == 16  # padding + tag


def test_token_bucket_allows_a_burst_then_one_per_interval():
    now = [100.0]
    bucket = chat.TokenBucket(5, 2.0, clock=lambda: now[0])
    assert [bucket.allow() for _ in range(6)] == [True] * 5 + [False]
    now[0] += 2.0
    assert bucket.allow() and not bucket.allow()


def test_policy_can_switch_chat_off(monkeypatch):
    monkeypatch.setattr(chat, "policy", lambda: _common.Policy(disable_chat=True))
    assert not chat.available()
