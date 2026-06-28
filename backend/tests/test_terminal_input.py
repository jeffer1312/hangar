from unittest.mock import patch, call

import pytest

from app import pqueue
from app import terminal_input
from app.pqueue import PromptQueue
from app.terminal_input import TerminalInput


@pytest.fixture
def tmp_queue(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_send_prompt_literal_then_enter():
    # Gate: pane entregavel (sessao viva, sem overlay) + marcador de ready -> envia e devolve "sent".
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value="? for shortcuts\n"), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "corrige o bug") == "sent"
    assert sk.call_args_list == [
        call("cc", "corrige o bug", literal=True),
        call("cc", "Enter"),
    ]


def test_send_prompt_defers_on_overlay():
    # Overlay aberto (rodape de navegacao) -> NAO digita as cegas; devolve "deferred", zero teclas.
    pane = "● plano\n────────\n  Esc to cancel · Enter to select\n"
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value=pane), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("cc", "oi") == "deferred"
    sk.assert_not_called()


def test_send_prompt_rejects_control_chars():
    with pytest.raises(ValueError):
        TerminalInput().send_prompt("cc", "bad\x00null")


def test_deliverable_false_when_no_session(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: False)
    assert terminal_input.deliverable("cc") is False


def test_deliverable_true_on_capture_error(monkeypatch):
    monkeypatch.setattr(terminal_input.tmux, "has_session", lambda name: True)
    def boom(name, lines=200):
        raise OSError("capture falhou")
    monkeypatch.setattr(terminal_input.tmux, "capture_pane", boom)
    assert terminal_input.deliverable("cc") is True   # degrada pro envio de hoje, sem regressao


def test_drain_sends_pending_and_marks_delivered(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    PromptQueue("cc").append("dois", delivered=False)
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: sent.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 2
    assert sent == ["um", "dois"]
    assert all(e["delivered"] for e in PromptQueue("cc").load())


def test_drain_noop_and_reverts_when_overlay(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: "deferred")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert PromptQueue("cc").load()[0]["delivered"] is False   # revertida (nao perdida)


def test_drain_does_not_revert_on_send_failure(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    def boom(self, name, text):
        raise RuntimeError("tty caiu no meio")
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt", boom)
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    # at-most-once: permanece True -> NAO re-enfileira -> nao digita 2x um prompt nao-idempotente.
    assert PromptQueue("cc").load()[0]["delivered"] is True


def test_drain_cheap_check_skips_capture_when_nothing_pending(tmp_queue, monkeypatch):
    PromptQueue("cc").append("ja entregue", delivered=True)
    called = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: called.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert called == []   # nem chamou send_prompt (e nem capture_pane)


def test_drain_skips_entries_before_start_ts(tmp_queue, tmp_path, monkeypatch):
    PromptQueue("cc").append("velha", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2999-01-01T00:00:00Z"}\n', encoding="utf-8")  # start_ts > ts da entrada
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text: sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 0 and sent == []


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
