import random

import pytest

import weeknumber_tzd_rules as rules


@pytest.fixture(scope="module")
def board():
    loaded = rules.load_board()
    assert loaded is not None
    return loaded


def make_state(board, owners, armies, players=2, phase=rules.PHASE_ATTACK):
    """A hand-built mid-game position: `owners` maps territory -> player id
    for the territories that matter, everything else belongs to player 9."""
    state = rules.GameState()
    for pid in range(players):
        state.players.append({"id": pid, "name": f"P{pid}", "color": pid,
                              "alive": True, "neutral": False, "left": False,
                              "cards": []})
    state.players.append({"id": 9, "name": "Rest", "color": 5, "alive": True,
                          "neutral": False, "left": False, "cards": []})
    for tid in board.order:
        state.owner[tid] = owners.get(tid, 9)
        state.armies[tid] = armies.get(tid, 1)
    state.phase = phase
    state.deck = rules.build_deck(board, random.Random(1))
    return state


class FixedDice(random.Random):
    """Returns queued die values in order."""

    def __init__(self, values):
        super().__init__(0)
        self.values = list(values)

    def randint(self, a, b):
        return self.values.pop(0)


def test_new_game_deals_every_territory_and_allotment(board):
    players = [(0, "Ann", 0), (1, "Bob", 1), (2, "Cy", 2)]
    state = rules.new_game(board, players, random.Random(7))
    assert set(state.owner) == set(board.order)
    for pid in (0, 1, 2):
        own = state.territories_of(pid)
        assert own
        assert sum(state.armies[t] for t in own) == rules.STARTING_ARMIES[3]
    assert state.phase == rules.PHASE_REINFORCE
    assert state.reinforcements == rules.reinforcement_count(
        board, state, state.current_player["id"])


def test_two_player_game_adds_a_passive_neutral(board):
    state = rules.new_game(board, [(0, "Ann", 0), (1, "Bob", 1)], random.Random(3))
    assert len(state.players) == 3
    neutral = state.players[-1]
    assert neutral["neutral"] and neutral["color"] == -1
    assert state.territories_of(neutral["id"])
    assert state.current_player["id"] in (0, 1)


@pytest.mark.parametrize(("territories", "expected"), [(3, 3), (11, 3), (12, 4), (20, 6)])
def test_reinforcement_minimum_and_division(board, territories, expected):
    # Spread across continents so no bonus applies: every 7th territory.
    picked = [t for i, t in enumerate(board.order) if i % 2 == 0][:territories]
    state = make_state(board, dict.fromkeys(picked, 0), {})
    owned_continents = [c for c in board.continents.values()
                        if set(picked).issuperset(c.territories)]
    bonus = sum(c.bonus for c in owned_continents)
    assert rules.reinforcement_count(board, state, 0) == expected + bonus


def test_continent_bonus(board):
    australia = board.continents["australia"].territories
    state = make_state(board, dict.fromkeys(australia, 0), {})
    assert rules.reinforcement_count(board, state, 0) == 3 + 2


def test_dice_ties_go_to_the_defender(board):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 4, "kamchatka": 2})
    state.current = 0
    rng = FixedDice([6, 3, 1, 6, 2])  # attacker 6,3,1 vs defender 6,2
    events = rules.apply(board, state, 0, {"type": "attack", "from": "alaska",
                                           "to": "kamchatka", "dice": 3}, rng)
    assert events[0]["att"] == [6, 3, 1] and events[0]["def"] == [6, 2]
    assert events[0]["att_loss"] == 1 and events[0]["def_loss"] == 1
    assert state.armies["alaska"] == 3 and state.armies["kamchatka"] == 1


def test_capture_requires_occupation_with_dice_count(board):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 5, "kamchatka": 1})
    state.current = 0
    rng = FixedDice([6, 5, 4, 1])
    events = rules.apply(board, state, 0, {"type": "attack", "from": "alaska",
                                           "to": "kamchatka", "dice": 3}, rng)
    assert any(e["type"] == "capture" for e in events)
    assert state.phase == rules.PHASE_OCCUPY
    assert state.occupy == {"from": "alaska", "to": "kamchatka", "min": 3, "max": 4}
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 0, {"type": "occupy", "n": 2}, rng)
    rules.apply(board, state, 0, {"type": "occupy", "n": 4}, rng)
    assert state.owner["kamchatka"] == 0
    assert state.armies["kamchatka"] == 4 and state.armies["alaska"] == 1
    assert state.phase == rules.PHASE_ATTACK


@pytest.mark.parametrize("action", [
    {"type": "attack", "from": "alaska", "to": "brazil", "dice": 1},       # not adjacent
    {"type": "attack", "from": "kamchatka", "to": "alaska", "dice": 1},    # not owner
    {"type": "attack", "from": "alaska", "to": "kamchatka", "dice": 3},    # too many dice
    {"type": "attack", "from": "alaska", "to": "kamchatka", "dice": True},
    {"type": "place", "t": "alaska", "n": 1},                              # wrong phase
    {"type": "teleport"},
    "attack",
])
def test_illegal_actions_are_rejected_without_side_effects(board, action):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 3, "kamchatka": 2})
    state.current = 0
    before = state.to_dict(full=True)
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 0, action, random.Random(0))
    assert state.to_dict(full=True) == before


def test_only_the_current_player_may_act(board):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 3, "kamchatka": 3})
    state.current = 0
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 1, {"type": "end_turn"}, random.Random(0))


def test_fortify_needs_a_connected_chain(board):
    chain = {"alaska": 0, "alberta": 0, "western_us": 0, "brazil": 0}
    state = make_state(board, chain, {"alaska": 6})
    state.current = 0
    assert rules.reachable(board, state, "alaska") == {"alberta", "western_us"}
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 0, {"type": "fortify", "from": "alaska",
                                      "to": "brazil", "n": 2}, random.Random(0))
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 0, {"type": "fortify", "from": "alaska",
                                      "to": "western_us", "n": 6}, random.Random(0))
    rules.apply(board, state, 0, {"type": "fortify", "from": "alaska",
                                  "to": "western_us", "n": 5}, random.Random(0))
    assert state.armies["western_us"] == 6
    assert state.current_player["id"] != 0  # fortifying ends the turn


@pytest.mark.parametrize(("symbols", "valid"), [
    (["infantry", "infantry", "infantry"], True),
    (["infantry", "cavalry", "artillery"], True),
    (["infantry", "infantry", "cavalry"], False),
    (["infantry", "infantry", "wild"], True),
    (["wild", "cavalry", "artillery"], True),
])
def test_card_sets(symbols, valid):
    cards = [{"t": None if s == "wild" else "alaska", "s": s} for s in symbols]
    assert rules.is_set(cards) is valid


def test_trade_values_escalate():
    assert [rules.trade_value(i) for i in range(9)] == [4, 6, 8, 10, 12, 15, 20, 25, 30]


def test_trade_adds_armies_and_territory_bonus(board):
    state = make_state(board, {"alaska": 0}, {}, phase=rules.PHASE_REINFORCE)
    state.current = 0
    state.reinforcements = 3
    state.players[0]["cards"] = [{"t": "alaska", "s": "infantry"},
                                 {"t": "brazil", "s": "infantry"},
                                 {"t": "peru", "s": "infantry"}]
    events = rules.apply(board, state, 0, {"type": "trade", "cards": [0, 1, 2]},
                         random.Random(0))
    assert events[0]["armies"] == 4
    assert state.reinforcements == 7
    assert state.armies["alaska"] == 1 + rules.TRADE_TERRITORY_BONUS
    assert state.players[0]["cards"] == []
    assert state.trade_count == 1


def test_five_cards_force_a_trade_before_placing(board):
    state = make_state(board, {"alaska": 0}, {}, phase=rules.PHASE_REINFORCE)
    state.current = 0
    state.reinforcements = 3
    state.players[0]["cards"] = [{"t": "brazil", "s": "infantry"}] * 2 + \
        [{"t": "peru", "s": "cavalry"}] * 2 + [{"t": "iceland", "s": "artillery"}]
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, 0, {"type": "place", "t": "alaska", "n": 1},
                    random.Random(0))
    trio = rules.find_set(state.players[0]["cards"])
    rules.apply(board, state, 0, {"type": "trade", "cards": trio}, random.Random(0))
    rules.apply(board, state, 0, {"type": "place", "t": "alaska", "n": 7},
                random.Random(0))
    assert state.phase == rules.PHASE_ATTACK


def test_conquest_earns_one_card_at_turn_end(board):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 3, "kamchatka": 1})
    state.current = 0
    rules.apply(board, state, 0, {"type": "attack", "from": "alaska", "to": "kamchatka",
                                  "dice": 1}, FixedDice([6, 1]))
    rules.apply(board, state, 0, {"type": "occupy", "n": 1}, random.Random(0))
    rules.apply(board, state, 0, {"type": "end_turn"}, random.Random(0))
    assert len(state.player(0)["cards"]) == 1


def test_elimination_transfers_cards_and_wins(board):
    state = make_state(board, {"alaska": 0, "kamchatka": 1}, {"alaska": 3, "kamchatka": 1},
                       players=2)
    # Player 9 ("Rest") would still be in the game; remove it from play.
    state.player(9)["left"] = True
    for tid in board.order:
        if state.owner[tid] == 9:
            state.owner[tid] = 0
    state.current = 0
    state.player(1)["cards"] = [{"t": "peru", "s": "cavalry"}]
    events = rules.apply(board, state, 0, {"type": "attack", "from": "alaska",
                                           "to": "kamchatka", "dice": 1}, FixedDice([6, 1]))
    kinds = [e["type"] for e in events]
    assert "eliminated" in kinds and "win" in kinds
    assert state.player(0)["cards"] == [{"t": "peru", "s": "cavalry"}]
    assert state.winner == 0
    rules.apply(board, state, 0, {"type": "occupy", "n": 1}, random.Random(0))
    assert state.phase == rules.PHASE_OVER


def test_leaving_player_passes_the_turn(board):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1), (2, "C", 2)], random.Random(5))
    leaving = state.current_player["id"]
    events = rules.remove_player(board, state, leaving)
    assert events[0] == {"type": "left", "player": leaving}
    assert state.current_player["id"] != leaving
    assert state.phase == rules.PHASE_REINFORCE
    others = [p["id"] for p in state.players if p["id"] != leaving]
    rules.remove_player(board, state, others[0])
    assert state.phase == rules.PHASE_OVER
    assert state.winner == others[1]


def test_client_view_is_redacted_and_round_trips(board):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1)], random.Random(2))
    state.player(0)["cards"] = [{"t": "alaska", "s": "infantry"}]
    view = state.to_dict(viewer=1)
    assert "deck" not in view and view["deck_count"] == len(state.deck)
    mine = next(p for p in view["players"] if p["id"] == 1)
    assert mine["cards"] == []
    hidden = next(p for p in view["players"] if p["id"] == 0)
    assert "cards" not in hidden and hidden["card_count"] == 1
    restored = rules.GameState.from_dict(view, board)
    assert restored.owner == state.owner and restored.armies == state.armies


@pytest.mark.parametrize("mutate", [
    lambda d: d.update(phase="party"),
    lambda d: d["armies"].update(alaska=0),
    lambda d: d["owner"].update(alaska=77),
    lambda d: d.pop("owner"),
    lambda d: d["players"][0].update(name="x" * 200),
    lambda d: d.update(current=True),
])
def test_from_dict_rejects_malformed_state(board, mutate):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1)], random.Random(2))
    data = state.to_dict(viewer=0)
    mutate(data)
    with pytest.raises(ValueError):
        rules.GameState.from_dict(data, board)


def _random_legal_action(board, state, rng):
    """A greedy bot with some noise: masses on its best front, blitzes weaker
    neighbours, and sometimes rolls single attacks or fortifies at random so
    every rule path gets exercised. Greedy enough that games finish."""
    me = state.current_player["id"]
    own = state.territories_of(me)
    if state.phase == rules.PHASE_REINFORCE:
        if rules.must_trade(state) or (state.current_player["cards"] and rng.random() < 0.5):
            trio = rules.find_set(state.current_player["cards"])
            if trio is not None:
                return {"type": "trade", "cards": trio}
        fronts = [t for t in own if rules.attack_targets(board, state, t)
                  or any(state.owner[n] != me for n in board.neighbours[t])]
        best = max(fronts or own, key=lambda t: state.armies[t] - min(
            (state.armies[n] for n in board.neighbours[t] if state.owner[n] != me),
            default=0))
        return {"type": "place", "t": best, "n": state.reinforcements}
    if state.phase == rules.PHASE_OCCUPY:
        step = state.occupy
        n = step["max"] if rng.random() < 0.8 else rng.randint(step["min"], step["max"])
        return {"type": "occupy", "n": n}
    if state.phase == rules.PHASE_ATTACK:
        options = [(s, t) for s in own for t in rules.attack_targets(board, state, s)
                   if state.armies[s] > state.armies[t] + 1]
        if options:
            source, target = max(options, key=lambda o: state.armies[o[0]] - state.armies[o[1]])
            if rng.random() < 0.2:
                return {"type": "attack", "from": source, "to": target,
                        "dice": rng.randint(1, rules.attack_dice_max(state, source))}
            return {"type": "blitz", "from": source, "to": target}
    movers = [s for s in own if state.armies[s] > 1 and rules.reachable(board, state, s)]
    if movers and rng.random() < 0.3:
        source = rng.choice(movers)
        target = rng.choice(sorted(rules.reachable(board, state, source)))
        return {"type": "fortify", "from": source, "to": target,
                "n": rng.randint(1, state.armies[source] - 1)}
    return {"type": "end_turn"}


@pytest.mark.parametrize("seed", range(6))
def test_random_games_keep_invariants_and_finish(board, seed):
    rng = random.Random(seed)
    players = [(i, f"P{i}", i) for i in range(2 + seed % 5)]
    state = rules.new_game(board, players, rng)
    for _ in range(20000):
        if state.phase == rules.PHASE_OVER:
            break
        action = _random_legal_action(board, state, rng)
        rules.apply(board, state, state.current_player["id"], action, rng)
        empty = {t for t, n in state.armies.items() if n < 1}
        # Only a just-conquered territory awaiting occupation may be empty.
        assert empty <= ({state.occupy["to"]} if state.occupy else set())
        assert len(state.deck) + sum(len(p["cards"]) for p in state.players) == len(board.order) + 2
        if state.phase != rules.PHASE_OVER:
            assert not state.current_player["neutral"]
        # Every snapshot must survive the client round trip.
        rules.GameState.from_dict(state.to_dict(viewer=state.current_player["id"]), board)
    assert state.phase == rules.PHASE_OVER
    assert state.winner is not None


def test_board_is_shared_while_held_and_freed_after(board):
    import gc
    import weakref
    assert rules.load_board() is board  # the module fixture holds it
    rules._BOARD = None
    first = rules.load_board()
    assert first is not board and rules.load_board() is first
    gone = weakref.ref(first)
    del first
    gc.collect()
    assert gone() is None  # nothing but its holders keeps it in memory


def test_giving_up_out_of_turn_keeps_the_turn_and_leaves_the_armies(board):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1), (2, "C", 2)], random.Random(5))
    current = state.current_player["id"]
    quitter = next(p["id"] for p in state.players if p["id"] != current)
    territories = state.territories_of(quitter)
    events = rules.apply(board, state, quitter, {"type": "concede"}, random.Random(0))
    assert events == [{"type": "conceded", "player": quitter}]
    gone = state.player(quitter)
    assert gone["conceded"] and not gone["alive"] and not gone["left"]
    assert state.current_player["id"] == current
    assert state.territories_of(quitter) == territories  # still on the board
    with pytest.raises(rules.IllegalAction):
        rules.apply(board, state, quitter, {"type": "concede"}, random.Random(0))
    copy = rules.GameState.from_dict(state.to_dict(viewer=current), board)
    assert copy.player(quitter)["conceded"] is True


def test_giving_up_on_your_turn_passes_it_on(board):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1), (2, "C", 2)], random.Random(5))
    quitter = state.current_player["id"]
    events = rules.apply(board, state, quitter, {"type": "concede"}, random.Random(0))
    assert events[0] == {"type": "conceded", "player": quitter}
    assert any(e["type"] == "turn" for e in events)
    assert state.current_player["id"] != quitter
    assert state.phase == rules.PHASE_REINFORCE
    with pytest.raises(rules.IllegalAction):  # watching now: no more moves
        rules.apply(board, state, quitter, {"type": "end_turn"}, random.Random(0))


def test_the_last_one_still_playing_wins_when_the_other_gives_up(board):
    state = rules.new_game(board, [(0, "A", 0), (1, "B", 1)], random.Random(5))
    events = rules.apply(board, state, 1, {"type": "concede"}, random.Random(0))
    assert {"type": "win", "player": 0} in events
    assert state.phase == rules.PHASE_OVER and state.winner == 0
    neutral = next(p for p in state.players if p["neutral"])
    state2 = rules.new_game(board, [(0, "A", 0), (1, "B", 1)], random.Random(5))
    with pytest.raises(rules.IllegalAction):
        rules.concede(board, state2, neutral["id"])
