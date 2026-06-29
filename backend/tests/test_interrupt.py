from unittest.mock import patch, call

from app.terminal_input import TerminalInput


def test_interrupt_no_clear_sends_single_escape():
    ti = TerminalInput()
    with patch("app.terminal_input.send_keys") as sk:
        ti.interrupt("sess", clear=False)
    assert sk.call_args_list == [call("sess", "Escape")]


def test_interrupt_clear_sends_double_escape():
    # clear=True -> 2o Esc limpa o input (msg que o Claude recolocou apos interromper).
    ti = TerminalInput()
    with patch("app.terminal_input.send_keys") as sk, patch("app.terminal_input.time.sleep"):
        ti.interrupt("sess", clear=True)
    assert sk.call_args_list == [call("sess", "Escape"), call("sess", "Escape")]


def test_interrupt_default_is_no_clear():
    ti = TerminalInput()
    with patch("app.terminal_input.send_keys") as sk:
        ti.interrupt("sess")
    assert sk.call_args_list == [call("sess", "Escape")]
