from unittest.mock import patch, call
from app import terminal_input
from app.terminal_input import TerminalInput


def test_send_prompt_literal_then_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().send_prompt("cc", "corrige o bug")
    assert sk.call_args_list == [
        call("cc", "corrige o bug", literal=True),
        call("cc", "Enter"),
    ]


def test_send_prompt_rejects_control_chars():
    import pytest
    with pytest.raises(ValueError):
        TerminalInput().send_prompt("cc", "bad\x00null")


def test_select_option_three_navigates_then_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 3)
    assert sk.call_args_list == [call("cc", "Down"), call("cc", "Down"), call("cc", "Enter")]


def test_select_option_one_just_enter():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().select("cc", 1)
    assert sk.call_args_list == [call("cc", "Enter")]


def test_interrupt_sends_escape():
    with patch.object(terminal_input, "send_keys") as sk:
        TerminalInput().interrupt("cc")
    assert sk.call_args_list == [call("cc", "Escape")]
