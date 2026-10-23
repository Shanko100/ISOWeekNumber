"""The lobby name follows the player's name until the player types their own."""

from weeknumber_tzd_lobby import default_lobby_name, follow_lobby_name


def test_the_default_lobby_name_follows_a_rename():
    assert follow_lobby_name("Player 4821's game", "Player 4821", "Dana") == "Dana's game"


def test_it_keeps_following_while_the_name_is_typed():
    lobby, name = default_lobby_name("Player 4821"), "Player 4821"
    for typed in ("", "D", "Da", "Dan", "Dana ", "Dana B"):
        lobby, name = follow_lobby_name(lobby, name, typed), typed
    assert lobby == "Dana B's game"


def test_an_emptied_lobby_name_follows_too():
    assert follow_lobby_name("", "Dana", "Bob") == "Bob's game"


def test_a_lobby_name_the_player_typed_stays():
    assert follow_lobby_name("Office war", "Dana", "Bob") == "Office war"
    assert follow_lobby_name("Dana's game night", "Dana", "Bob") == "Dana's game night"
