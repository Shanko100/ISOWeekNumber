"""Timezone Dominator rules engine for WeekNumber: the board, the game state and every rule.

No Tk and no sockets: the lobby host runs the only authoritative copy of the
game through apply(), and clients use the same module read-only to decide
which buttons and territories are live. Actions and states are plain dicts so
they travel over the wire unchanged.

Rules are the classic core:

* 2-6 players. Territories are dealt at random, one army each, and the rest of
  each starting allotment (40/35/30/25/20) is spread over the player's own
  territories automatically. Two players share the board with a passive
  neutral army, as in the classic two-player variant.
* Each turn: reinforce (max(3, territories // 3) plus continent bonuses, plus
  card trades), attack as often as wanted, then one fortifying move along a
  chain of the player's own territories.
* Dice: the attacker rolls up to three (always leaving one army behind), the
  defender as many as possible up to two; highest pairs are compared and ties
  go to the defender. A conquered territory must be occupied by at least as
  many armies as dice were rolled.
* One card per turn in which at least one territory was conquered. Sets are
  three of a kind or one of each, wilds substitute; sets trade for 4, 6, 8,
  10, 12, 15 and then 5 more each time, plus 2 armies on a pictured territory
  the player owns. Five or more cards force a trade. Eliminating a player
  takes their cards; reaching six forces an immediate trade.
* A player who leaves mid-game stops taking turns; their armies stay on the
  board to be conquered.
* A player may give up (concede) at any time, on their turn or not: they are
  out of the game but keep watching, and their armies stay on the board to be
  conquered, like a player who left.
"""

from __future__ import annotations

import hashlib
import random
import re
import weakref
from collections import deque
from pathlib import Path

from weeknumber_core import bundled_file, load_bounded_json, log
from weeknumber_timezone import _PointBudget, _parse_rings

DATA_FILE = "tzd_data.json"
DATA_FORMAT = "weeknumber-tzd-1"
MAX_DATA_BYTES = 2 * 1024 * 1024
MAX_TERRITORIES = 64
MAX_CONTINENTS = 12
MAX_TOTAL_POINTS = 80_000
_ID_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}$")

MIN_PLAYERS = 2
MAX_PLAYERS = 6
STARTING_ARMIES = {2: 40, 3: 35, 4: 30, 5: 25, 6: 20}
TRADE_VALUES = (4, 6, 8, 10, 12, 15)
TRADE_TERRITORY_BONUS = 2
MAX_HAND = 5           # holding this many at turn start forces a trade
MAX_HAND_IN_TURN = 6   # reaching this after an elimination forces one now
SYMBOLS = ("infantry", "cavalry", "artillery")
WILD = "wild"
NEUTRAL_NAME = "Neutral"
MAX_NAME_CHARS = 24
#: Sanity cap for armies on one territory in a received state; escalating
#: trade-ins make long games grow large stacks.
MAX_ARMIES = 1_000_000

PHASE_REINFORCE = "reinforce"
PHASE_ATTACK = "attack"
PHASE_OCCUPY = "occupy"
PHASE_FORTIFY = "fortify"
PHASE_OVER = "over"
PHASES = (PHASE_REINFORCE, PHASE_ATTACK, PHASE_OCCUPY, PHASE_FORTIFY, PHASE_OVER)


class BoardError(ValueError):
    pass


class IllegalAction(ValueError):
    """An action the current state does not allow. The message is safe to
    show to the player who sent it."""


# ---------------------------------------------------------------------------
# Board
# ---------------------------------------------------------------------------
class Territory:
    __slots__ = ("id", "name", "continent", "rings", "label")

    def __init__(self, tid: str, name: str, continent: str, rings: list,
                 label: tuple[float, float]) -> None:
        self.id = tid
        self.name = name
        self.continent = continent
        self.rings = rings
        self.label = label


class Continent:
    __slots__ = ("id", "name", "bonus", "territories")

    def __init__(self, cid: str, name: str, bonus: int) -> None:
        self.id = cid
        self.name = name
        self.bonus = bonus
        self.territories: list[str] = []


class Board:
    """Immutable after construction: territories, continents, adjacency, and
    the sea routes the map draws."""

    def __init__(self, continents: list[Continent], territories: list[Territory],
                 links: list[tuple[str, str, "list[float] | None"]]) -> None:
        self.continents = {c.id: c for c in continents}
        self.continent_order = [c.id for c in continents]
        self.territories = {t.id: t for t in territories}
        self.order = [t.id for t in territories]
        self.neighbours: dict[str, set[str]] = {t.id: set() for t in territories}
        self.routes: list[tuple[str, str, list[float]]] = []
        for a, b, line in links:
            self.neighbours[a].add(b)
            self.neighbours[b].add(a)
            if line is not None:
                self.routes.append((a, b, line))
        for territory in territories:
            self.continents[territory.continent].territories.append(territory.id)
        self.fingerprint = self._fingerprint()

    def _fingerprint(self) -> str:
        """Identifies the rules-relevant board, so a host and a client with
        different data files refuse to play together."""
        parts = [f"{c.id}:{c.bonus}:{','.join(sorted(c.territories))}"
                 for c in self.continents.values()]
        parts += [f"{t}>{','.join(sorted(self.neighbours[t]))}" for t in self.order]
        return hashlib.sha256("|".join(parts).encode("utf-8")).hexdigest()[:16]

    def adjacent(self, a: str, b: str) -> bool:
        return b in self.neighbours.get(a, ())

    @classmethod
    def from_document(cls, raw: object) -> "Board":
        """Validate the bundled JSON with the same distrust as the other data
        files: it can sit in a user-writable directory beside the app."""
        if not isinstance(raw, dict) or raw.get("format") != DATA_FORMAT:
            raise BoardError("unknown board format")
        continents_raw = raw.get("continents")
        territories_raw = raw.get("territories")
        links_raw = raw.get("links")
        if not isinstance(continents_raw, list) or not 1 <= len(continents_raw) <= MAX_CONTINENTS:
            raise BoardError("invalid continent list")
        if not isinstance(territories_raw, list) or not 2 <= len(territories_raw) <= MAX_TERRITORIES:
            raise BoardError("invalid territory list")
        if not isinstance(links_raw, list) or len(links_raw) > MAX_TERRITORIES * 8:
            raise BoardError("invalid link list")

        continents: list[Continent] = []
        for item in continents_raw:
            if not isinstance(item, dict):
                raise BoardError("continent entry is not an object")
            cid, name, bonus = item.get("id"), item.get("name"), item.get("bonus")
            if not _valid_id(cid) or not _valid_name(name):
                raise BoardError("invalid continent id or name")
            if not isinstance(bonus, int) or isinstance(bonus, bool) or not 0 <= bonus <= 20:
                raise BoardError(f"invalid bonus for {cid}")
            if any(c.id == cid for c in continents):
                raise BoardError(f"duplicate continent {cid}")
            continents.append(Continent(cid, name.strip(), bonus))
        continent_ids = {c.id for c in continents}

        budget = _PointBudget(MAX_TOTAL_POINTS)
        territories: list[Territory] = []
        seen: set[str] = set()
        for item in territories_raw:
            if not isinstance(item, dict):
                raise BoardError("territory entry is not an object")
            tid, name, continent = item.get("id"), item.get("name"), item.get("continent")
            if not _valid_id(tid) or tid in seen or not _valid_name(name):
                raise BoardError("invalid or duplicate territory")
            if continent not in continent_ids:
                raise BoardError(f"{tid} names an unknown continent")
            label = item.get("label")
            if (not isinstance(label, list) or len(label) != 2
                    or not all(_number(v) for v in label)
                    or abs(label[0]) > 180 or abs(label[1]) > 90):
                raise BoardError(f"invalid label point for {tid}")
            rings = _parse_rings(item.get("rings"), budget)
            if not rings:
                raise BoardError(f"{tid} has no usable shape")
            seen.add(tid)
            territories.append(Territory(tid, name.strip(), continent, rings,
                                         (float(label[0]), float(label[1]))))

        links: list[tuple[str, str, "list[float] | None"]] = []
        pairs: set[frozenset] = set()
        for item in links_raw:
            if not isinstance(item, dict):
                raise BoardError("link entry is not an object")
            a, b = item.get("a"), item.get("b")
            if a not in seen or b not in seen or a == b:
                raise BoardError("link names an unknown territory")
            pair = frozenset((a, b))
            if pair in pairs:
                raise BoardError(f"duplicate link {a}-{b}")
            pairs.add(pair)
            line = item.get("line")
            if line is not None:
                if (not isinstance(line, list) or len(line) != 4
                        or not all(_number(v) for v in line)
                        or abs(line[0]) > 180 or abs(line[2]) > 180
                        or abs(line[1]) > 90 or abs(line[3]) > 90):
                    raise BoardError(f"invalid route for {a}-{b}")
                line = [float(v) for v in line]
            links.append((a, b, line))

        board = cls(continents, territories, links)
        if any(not c.territories for c in continents):
            raise BoardError("a continent has no territories")
        if any(not n for n in board.neighbours.values()):
            raise BoardError("a territory has no neighbours")
        if not _connected(board):
            raise BoardError("the board is not connected")
        return board


def _valid_id(value: object) -> bool:
    return isinstance(value, str) and _ID_RE.fullmatch(value) is not None


def _valid_name(value: object) -> bool:
    return isinstance(value, str) and 1 <= len(value.strip()) <= 40


def _number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _connected(board: Board) -> bool:
    start = board.order[0]
    seen = {start}
    queue = deque([start])
    while queue:
        for nxt in board.neighbours[queue.popleft()]:
            if nxt not in seen:
                seen.add(nxt)
                queue.append(nxt)
    return len(seen) == len(board.order)


#: The bundled board while anything holds it -- a window, a session -- so it
#: is loaded once per game and leaves memory with the last window.
_BOARD: "weakref.ref[Board] | None" = None


def load_board(path: "Path | None" = None) -> "Board | None":
    """The bundled board, loaded once while it is in use. None (and a log
    entry) when the file is missing or invalid; the game menu then reports
    that Timezone Dominator is unavailable."""
    global _BOARD
    board = _BOARD() if path is None and _BOARD is not None else None
    if board is not None:
        return board
    try:
        source = path or bundled_file(DATA_FILE)
        if source is None:
            log.info("%s not found; Timezone Dominator disabled", DATA_FILE)
            return None
        board = Board.from_document(load_bounded_json(source, MAX_DATA_BYTES))
    except Exception:
        log.exception("Failed to load %s; Timezone Dominator disabled", DATA_FILE)
        return None
    if path is None:
        _BOARD = weakref.ref(board)
    return board


# ---------------------------------------------------------------------------
# Cards
# ---------------------------------------------------------------------------
def build_deck(board: Board, rng: random.Random) -> list[dict]:
    """One card per territory, symbols cycling, plus two wilds."""
    deck = [{"t": tid, "s": SYMBOLS[i % len(SYMBOLS)]}
            for i, tid in enumerate(board.order)]
    deck += [{"t": None, "s": WILD}, {"t": None, "s": WILD}]
    rng.shuffle(deck)
    return deck


def is_set(cards: list[dict]) -> bool:
    if len(cards) != 3:
        return False
    symbols = [c["s"] for c in cards if c["s"] != WILD]
    wilds = 3 - len(symbols)
    if wilds >= 1:
        # Two cards plus a wild always complete either kind of set.
        return True
    return len(set(symbols)) in (1, 3)


def trade_value(trade_count: int) -> int:
    """Armies for the (trade_count + 1)-th set traded in this game."""
    if trade_count < len(TRADE_VALUES):
        return TRADE_VALUES[trade_count]
    return TRADE_VALUES[-1] + 5 * (trade_count - len(TRADE_VALUES) + 1)


def find_set(cards: list[dict]) -> "list[int] | None":
    """Indices of a tradeable set in a hand, preferring ones without wilds."""
    n = len(cards)
    best = None
    for i in range(n):
        for j in range(i + 1, n):
            for k in range(j + 1, n):
                trio = [cards[i], cards[j], cards[k]]
                if is_set(trio):
                    wilds = sum(1 for c in trio if c["s"] == WILD)
                    if best is None or wilds < best[0]:
                        best = (wilds, [i, j, k])
    return best[1] if best else None


# ---------------------------------------------------------------------------
# State
# ---------------------------------------------------------------------------
class GameState:
    """Everything about one game. `players` is in turn order; the neutral
    player, when present, is last and never takes a turn."""

    def __init__(self) -> None:
        self.players: list[dict] = []
        self.owner: dict[str, int] = {}
        self.armies: dict[str, int] = {}
        self.current = 0
        self.phase = PHASE_REINFORCE
        self.reinforcements = 0
        self.conquered = False
        self.fortified = False
        self.occupy: "dict | None" = None
        self.resume_attack = False
        self.trade_count = 0
        self.deck: list[dict] = []
        self.deck_count = 0
        self.turn = 1
        self.winner: "int | None" = None

    # -- lookups ------------------------------------------------------------
    def player(self, pid: int) -> dict:
        for player in self.players:
            if player["id"] == pid:
                return player
        raise IllegalAction("Unknown player.")

    @property
    def current_player(self) -> dict:
        return self.players[self.current]

    def territories_of(self, pid: int) -> list[str]:
        return [t for t, owner in self.owner.items() if owner == pid]

    def active_players(self) -> list[dict]:
        return [p for p in self.players
                if p["alive"] and not p["neutral"] and not p["left"]]

    # -- wire format ----------------------------------------------------------
    def to_dict(self, viewer: "int | None" = None, full: bool = False) -> dict:
        """Serialise. Clients get a redacted view: their own cards in full,
        everyone else's as a count, and the deck as a count only."""
        players = []
        for p in self.players:
            entry = {k: p[k] for k in ("id", "name", "color", "alive", "neutral", "left")}
            entry["conceded"] = bool(p.get("conceded", False))
            if full or p["id"] == viewer:
                entry["cards"] = [dict(c) for c in p["cards"]]
            entry["card_count"] = len(p["cards"])
            players.append(entry)
        data = {
            "players": players,
            "owner": dict(self.owner),
            "armies": dict(self.armies),
            "current": self.current,
            "phase": self.phase,
            "reinforcements": self.reinforcements,
            "conquered": self.conquered,
            "fortified": self.fortified,
            "occupy": dict(self.occupy) if self.occupy else None,
            "resume_attack": self.resume_attack,
            "trade_count": self.trade_count,
            "deck_count": len(self.deck),
            "turn": self.turn,
            "winner": self.winner,
        }
        if full:
            data["deck"] = [dict(c) for c in self.deck]
        return data

    @classmethod
    def from_dict(cls, data: object, board: Board) -> "GameState":
        """Rebuild a state received from the host, validating every field
        against the board: a malformed message must never crash the UI."""
        if not isinstance(data, dict):
            raise ValueError("state is not an object")
        state = cls()
        players = data.get("players")
        if not isinstance(players, list) or not MIN_PLAYERS <= len(players) <= MAX_PLAYERS + 1:
            raise ValueError("invalid player list")
        ids = set()
        for p in players:
            if not isinstance(p, dict):
                raise ValueError("invalid player")
            pid, name, color = p.get("id"), p.get("name"), p.get("color")
            if not _small_int(pid, 0, 99) or pid in ids:
                raise ValueError("invalid player id")
            if not isinstance(name, str) or not 1 <= len(name) <= MAX_NAME_CHARS:
                raise ValueError("invalid player name")
            if not _small_int(color, -1, MAX_PLAYERS - 1):
                raise ValueError("invalid player colour")
            cards = p.get("cards", [])
            if not isinstance(cards, list) or len(cards) > len(board.order) + 2:
                raise ValueError("invalid hand")
            clean_cards = [_clean_card(c, board) for c in cards]
            card_count = p.get("card_count", len(clean_cards))
            if not _small_int(card_count, 0, len(board.order) + 2):
                raise ValueError("invalid card count")
            ids.add(pid)
            state.players.append({
                "id": pid, "name": name, "color": color,
                "alive": bool(p.get("alive", True)),
                "neutral": bool(p.get("neutral", False)),
                "left": bool(p.get("left", False)),
                "conceded": p.get("conceded") is True,
                "cards": clean_cards, "card_count": card_count,
            })
        owner, armies = data.get("owner"), data.get("armies")
        if not isinstance(owner, dict) or not isinstance(armies, dict):
            raise ValueError("invalid board state")
        if set(owner) != set(board.order) or set(armies) != set(board.order):
            raise ValueError("board state does not match the board")
        for tid in board.order:
            # 0 is legal only on a conquest awaiting occupation (checked below).
            if owner[tid] not in ids or not _small_int(armies[tid], 0, MAX_ARMIES):
                raise ValueError(f"invalid state for {tid}")
        state.owner = dict(owner)
        state.armies = dict(armies)
        current = data.get("current")
        if not _small_int(current, 0, len(players) - 1):
            raise ValueError("invalid current player")
        state.current = current
        if data.get("phase") not in PHASES:
            raise ValueError("invalid phase")
        state.phase = data["phase"]
        for key in ("reinforcements", "trade_count", "deck_count", "turn"):
            if not _small_int(data.get(key), 0, 99999):
                raise ValueError(f"invalid {key}")
        state.reinforcements = data["reinforcements"]
        state.trade_count = data["trade_count"]
        state.deck_count = data["deck_count"]
        state.turn = data["turn"]
        state.conquered = bool(data.get("conquered"))
        state.fortified = bool(data.get("fortified"))
        state.resume_attack = bool(data.get("resume_attack"))
        occupy = data.get("occupy")
        if occupy is not None:
            if (not isinstance(occupy, dict) or occupy.get("from") not in board.territories
                    or occupy.get("to") not in board.territories
                    or not _small_int(occupy.get("min"), 1, MAX_ARMIES)
                    or not _small_int(occupy.get("max"), 1, MAX_ARMIES)):
                raise ValueError("invalid occupy step")
            occupy = {k: occupy[k] for k in ("from", "to", "min", "max")}
        empty = {t for t in board.order if armies[t] == 0}
        if empty - ({occupy["to"]} if occupy else set()):
            raise ValueError("a territory has no armies")
        state.occupy = occupy
        winner = data.get("winner")
        if winner is not None and winner not in ids:
            raise ValueError("invalid winner")
        state.winner = winner
        deck = data.get("deck")
        if isinstance(deck, list):
            state.deck = [_clean_card(c, board) for c in deck[:len(board.order) + 2]]
        return state


def _small_int(value: object, low: int, high: int) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and low <= value <= high


def _clean_card(card: object, board: Board) -> dict:
    if not isinstance(card, dict):
        raise ValueError("invalid card")
    symbol, tid = card.get("s"), card.get("t")
    if symbol == WILD and tid is None:
        return {"t": None, "s": WILD}
    if symbol in SYMBOLS and tid in board.territories:
        return {"t": tid, "s": symbol}
    raise ValueError("invalid card")


# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------
def new_game(board: Board, players: list[tuple[int, str, int]],
             rng: random.Random) -> GameState:
    """Start a game. `players` is (id, name, colour slot) in lobby order;
    turn order is drawn at random."""
    if not MIN_PLAYERS <= len(players) <= MAX_PLAYERS:
        raise IllegalAction(f"Timezone Dominator needs {MIN_PLAYERS} to {MAX_PLAYERS} players.")
    state = GameState()
    order = list(players)
    rng.shuffle(order)
    for pid, name, color in order:
        state.players.append({"id": pid, "name": name[:MAX_NAME_CHARS], "color": color,
                              "alive": True, "neutral": False, "left": False,
                              "conceded": False, "cards": []})
    if len(players) == 2:
        neutral_id = max(p[0] for p in players) + 1
        state.players.append({"id": neutral_id, "name": NEUTRAL_NAME, "color": -1,
                              "alive": True, "neutral": True, "left": False,
                              "conceded": False, "cards": []})
    allotment = STARTING_ARMIES[len(players)]

    territories = list(board.order)
    rng.shuffle(territories)
    seats = state.players
    for index, tid in enumerate(territories):
        state.owner[tid] = seats[index % len(seats)]["id"]
        state.armies[tid] = 1
    for seat in seats:
        own = state.territories_of(seat["id"])
        remaining = max(0, allotment - len(own))
        rng.shuffle(own)
        for i in range(remaining):
            state.armies[own[i % len(own)]] += 1

    state.deck = build_deck(board, rng)
    state.current = 0
    _begin_turn(board, state)
    return state


# ---------------------------------------------------------------------------
# Derived values
# ---------------------------------------------------------------------------
def reinforcement_count(board: Board, state: GameState, pid: int) -> int:
    owned = set(state.territories_of(pid))
    total = max(3, len(owned) // 3)
    for continent in board.continents.values():
        if continent.territories and owned.issuperset(continent.territories):
            total += continent.bonus
    return total


def attack_dice_max(state: GameState, source: str) -> int:
    return max(0, min(3, state.armies[source] - 1))


def reachable(board: Board, state: GameState, source: str) -> set[str]:
    """Territories the owner of `source` can fortify into: connected through
    their own territories."""
    pid = state.owner[source]
    seen = {source}
    queue = deque([source])
    while queue:
        for nxt in board.neighbours[queue.popleft()]:
            if nxt not in seen and state.owner[nxt] == pid:
                seen.add(nxt)
                queue.append(nxt)
    seen.discard(source)
    return seen


def attack_targets(board: Board, state: GameState, source: str) -> set[str]:
    pid = state.owner[source]
    if state.armies[source] < 2:
        return set()
    return {t for t in board.neighbours[source] if state.owner[t] != pid}


def must_trade(state: GameState) -> bool:
    """Five or more cards while reinforcing must be traded down first. Five
    cards always contain a set, so this can never deadlock."""
    return (state.phase == PHASE_REINFORCE
            and len(state.current_player["cards"]) >= MAX_HAND)


# ---------------------------------------------------------------------------
# Actions
# ---------------------------------------------------------------------------
def apply(board: Board, state: GameState, pid: int, action: object,
          rng: random.Random) -> list[dict]:
    """Validate and perform one action for player `pid`; return the events it
    produced. Raises IllegalAction and leaves the state untouched otherwise."""
    if state.phase == PHASE_OVER:
        raise IllegalAction("The game is over.")
    if not isinstance(action, dict) or not isinstance(action.get("type"), str):
        raise IllegalAction("Malformed action.")
    if action["type"] == "concede":  # allowed at any time, not only on your turn
        return concede(board, state, pid)
    if state.current_player["id"] != pid:
        raise IllegalAction("It is not your turn.")
    kind = action["type"]
    handler = _HANDLERS.get(kind)
    if handler is None:
        raise IllegalAction("Unknown action.")
    return handler(board, state, pid, action, rng)


def _territory(board: Board, action: dict, key: str) -> str:
    value = action.get(key)
    if value not in board.territories:
        raise IllegalAction("Unknown territory.")
    return value


def _count(action: dict, key: str, low: int, high: int) -> int:
    value = action.get(key)
    if not _small_int(value, low, high):
        raise IllegalAction(f"Choose between {low} and {high}.")
    return value


def _trade(board, state, pid, action, rng):
    if state.phase != PHASE_REINFORCE:
        raise IllegalAction("Cards can only be traded while reinforcing.")
    player = state.player(pid)
    indices = action.get("cards")
    if (not isinstance(indices, list) or len(indices) != 3
            or len(set(indices)) != 3
            or not all(_small_int(i, 0, len(player["cards"]) - 1) for i in indices)):
        raise IllegalAction("Choose three of your cards.")
    cards = [player["cards"][i] for i in indices]
    if not is_set(cards):
        raise IllegalAction("Those cards are not a set.")
    value = trade_value(state.trade_count)
    state.trade_count += 1
    state.reinforcements += value
    bonus_at = next((c["t"] for c in cards
                     if c["t"] is not None and state.owner[c["t"]] == pid), None)
    if bonus_at is not None:
        state.armies[bonus_at] += TRADE_TERRITORY_BONUS
    for i in sorted(indices, reverse=True):
        player["cards"].pop(i)
    # Traded cards go to the bottom of the deck, as in the boxed game.
    state.deck.extend(cards)
    return [{"type": "trade", "player": pid, "armies": value, "bonus_at": bonus_at}]


def _place(board, state, pid, action, rng):
    if state.phase != PHASE_REINFORCE:
        raise IllegalAction("You have no armies to place.")
    if must_trade(state):
        raise IllegalAction("You hold too many cards: trade a set first.")
    tid = _territory(board, action, "t")
    if state.owner[tid] != pid:
        raise IllegalAction("Place armies on your own territories.")
    n = _count(action, "n", 1, max(1, state.reinforcements))
    if n > state.reinforcements:
        raise IllegalAction("You do not have that many armies.")
    state.armies[tid] += n
    state.reinforcements -= n
    events = [{"type": "place", "player": pid, "t": tid, "n": n}]
    if state.reinforcements == 0:
        state.phase = PHASE_ATTACK
        state.resume_attack = False
    return events


def _roll(rng: random.Random, n: int) -> list[int]:
    return sorted((rng.randint(1, 6) for _ in range(n)), reverse=True)


def _attack_once(board, state, pid, source, target, dice, rng) -> list[dict]:
    defender = state.owner[target]
    defend = min(2, state.armies[target])
    attack_roll = _roll(rng, dice)
    defend_roll = _roll(rng, defend)
    att_loss = def_loss = 0
    for a, d in zip(attack_roll, defend_roll):
        if a > d:
            def_loss += 1
        else:
            att_loss += 1
    state.armies[source] -= att_loss
    state.armies[target] -= def_loss
    events = [{"type": "attack", "player": pid, "defender": defender,
               "from": source, "to": target, "att": attack_roll, "def": defend_roll,
               "att_loss": att_loss, "def_loss": def_loss}]
    if state.armies[target] == 0:
        events += _capture(board, state, pid, defender, source, target, dice)
    return events


def _check_attack(board, state, pid, action) -> tuple[str, str]:
    if state.phase != PHASE_ATTACK:
        raise IllegalAction("You cannot attack now.")
    source = _territory(board, action, "from")
    target = _territory(board, action, "to")
    if state.owner[source] != pid:
        raise IllegalAction("Attack from one of your own territories.")
    if state.owner[target] == pid:
        raise IllegalAction("You cannot attack your own territory.")
    if not board.adjacent(source, target):
        raise IllegalAction("Those territories do not border each other.")
    if state.armies[source] < 2:
        raise IllegalAction("You need at least two armies to attack.")
    return source, target


def _attack(board, state, pid, action, rng):
    source, target = _check_attack(board, state, pid, action)
    dice = _count(action, "dice", 1, 3)
    if dice > attack_dice_max(state, source):
        raise IllegalAction("Too many dice for the armies there.")
    return _attack_once(board, state, pid, source, target, dice, rng)


def _blitz(board, state, pid, action, rng):
    """Attack with the most dice until the territory falls or only one army
    is left behind; reported as one aggregated event plus any capture."""
    source, target = _check_attack(board, state, pid, action)
    rolls = 0
    att_total = def_total = 0
    events: list[dict] = []
    while state.phase == PHASE_ATTACK and state.armies[source] >= 2 \
            and state.owner[target] != pid:
        result = _attack_once(board, state, pid, source, target,
                              attack_dice_max(state, source), rng)
        first = result[0]
        rolls += 1
        att_total += first["att_loss"]
        def_total += first["def_loss"]
        events = result  # keep the last roll and any capture events
    summary = {"type": "blitz", "player": pid, "from": source, "to": target,
               "rolls": rolls, "att_loss": att_total, "def_loss": def_total}
    return [summary] + events


def _capture(board, state, pid, defender, source, target, dice) -> list[dict]:
    state.owner[target] = pid
    state.conquered = True
    events = [{"type": "capture", "player": pid, "from": source, "to": target,
               "defender": defender}]
    loser = state.player(defender)
    if not state.territories_of(defender):
        loser["alive"] = False
        taken = len(loser["cards"])
        state.player(pid)["cards"].extend(loser["cards"])
        loser["cards"] = []
        events.append({"type": "eliminated", "player": defender, "by": pid,
                       "cards": taken})
    available = state.armies[source] - 1
    # Even a winning capture is occupied, so the final board reads correctly.
    state.phase = PHASE_OCCUPY
    state.occupy = {"from": source, "to": target,
                    "min": max(1, min(dice, available)), "max": max(1, available)}
    _check_winner(state, events)
    return events


def _check_winner(state: GameState, events: list[dict]) -> bool:
    """The game ends when one player is left who still plays: everyone else
    eliminated or gone. The neutral army never needs to be beaten."""
    active = state.active_players()
    if len(active) > 1:
        return False
    state.winner = active[0]["id"] if active else None
    if state.winner is not None:
        events.append({"type": "win", "player": state.winner})
    return True


def _occupy(board, state, pid, action, rng):
    if state.phase != PHASE_OCCUPY or state.occupy is None:
        raise IllegalAction("There is nothing to occupy.")
    step = state.occupy
    n = _count(action, "n", step["min"], step["max"])
    state.armies[step["from"]] -= n
    state.armies[step["to"]] += n
    state.occupy = None
    events = [{"type": "occupy", "player": pid, "from": step["from"],
               "to": step["to"], "n": n}]
    if state.winner is not None:
        state.phase = PHASE_OVER
        return events
    player = state.player(pid)
    if len(player["cards"]) >= MAX_HAND_IN_TURN:
        # Cards won by elimination: trade down now and place, then attack on.
        state.phase = PHASE_REINFORCE
        state.resume_attack = True
    else:
        state.phase = PHASE_ATTACK
    return events


def _end_attack(board, state, pid, action, rng):
    if state.phase != PHASE_ATTACK:
        raise IllegalAction("You are not attacking.")
    state.phase = PHASE_FORTIFY
    return [{"type": "phase", "player": pid, "phase": PHASE_FORTIFY}]


def _fortify(board, state, pid, action, rng):
    if state.phase not in (PHASE_ATTACK, PHASE_FORTIFY) or state.fortified:
        raise IllegalAction("You cannot fortify now.")
    source = _territory(board, action, "from")
    target = _territory(board, action, "to")
    if state.owner[source] != pid or state.owner[target] != pid:
        raise IllegalAction("Fortify between your own territories.")
    if target not in reachable(board, state, source):
        raise IllegalAction("There is no chain of your territories between them.")
    n = _count(action, "n", 1, max(1, state.armies[source] - 1))
    if n > state.armies[source] - 1:
        raise IllegalAction("One army must stay behind.")
    state.armies[source] -= n
    state.armies[target] += n
    state.fortified = True
    events = [{"type": "fortify", "player": pid, "from": source, "to": target, "n": n}]
    return events + _end_turn(board, state, pid, {}, rng)


def _end_turn(board, state, pid, action, rng):
    if state.phase not in (PHASE_ATTACK, PHASE_FORTIFY):
        raise IllegalAction("Finish placing your armies first.")
    events: list[dict] = []
    if state.conquered and state.deck:
        card = state.deck.pop(0)
        state.player(pid)["cards"].append(card)
        events.append({"type": "card", "player": pid})
    events += _advance(board, state)
    return events


def _advance(board: Board, state: GameState) -> list[dict]:
    """Pass the turn to the next player who still plays."""
    count = len(state.players)
    for step in range(1, count + 1):
        index = (state.current + step) % count
        candidate = state.players[index]
        if candidate["alive"] and not candidate["neutral"] and not candidate["left"]:
            if index <= state.current:
                state.turn += 1
            state.current = index
            break
    else:
        state.phase = PHASE_OVER
        return []
    _begin_turn(board, state)
    return [{"type": "turn", "player": state.current_player["id"],
             "reinforcements": state.reinforcements}]


def _begin_turn(board: Board, state: GameState) -> None:
    pid = state.current_player["id"]
    state.phase = PHASE_REINFORCE
    state.reinforcements = reinforcement_count(board, state, pid)
    state.conquered = False
    state.fortified = False
    state.occupy = None
    state.resume_attack = False


_HANDLERS = {
    "trade": _trade,
    "place": _place,
    "attack": _attack,
    "blitz": _blitz,
    "occupy": _occupy,
    "end_attack": _end_attack,
    "fortify": _fortify,
    "end_turn": _end_turn,
}


def _settle_occupation(state: GameState) -> None:
    """Finish a pending occupation with the minimum move, so the board never
    holds a conquered territory without armies."""
    step = state.occupy
    if step is not None:
        state.armies[step["from"]] -= step["min"]
        state.armies[step["to"]] += step["min"]
        state.occupy = None


def concede(board: Board, state: GameState, pid: int) -> list[dict]:
    """`pid` gives up. They are out of the game from now on and only watch;
    their armies stay on the board for the others to conquer, and their
    cards go with their last territory, as for any defeated player. If it
    was their turn, play passes on."""
    player = state.player(pid)
    if player["neutral"] or player["left"] or not player["alive"]:
        raise IllegalAction("You are no longer in this game.")
    player["alive"] = False
    player["conceded"] = True
    events = [{"type": "conceded", "player": pid}]
    was_current = state.current_player["id"] == pid
    if _check_winner(state, events):
        _settle_occupation(state)
        state.phase = PHASE_OVER
        return events
    if was_current:
        _settle_occupation(state)
        events += _advance(board, state)
    return events


def remove_player(board: Board, state: GameState, pid: int) -> list[dict]:
    """A player left the game. Their armies stay on the board; if it was their
    turn, play passes on."""
    player = state.player(pid)
    if player["left"] or state.phase == PHASE_OVER:
        return []
    player["left"] = True
    events = [{"type": "left", "player": pid}]
    was_current = state.current_player["id"] == pid
    if _check_winner(state, events):
        _settle_occupation(state)
        state.phase = PHASE_OVER
        return events
    if was_current:
        _settle_occupation(state)
        events += _advance(board, state)
    return events
