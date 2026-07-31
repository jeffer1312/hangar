from unittest.mock import patch, call

import pytest
import time

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
    # _entrou_no_composer stubado: o ponto DESTE teste e a ordem digita->Enter, nao a evidencia de
    # ingestao (essa tem teste proprio em test_terminal_answer.py). Sem o stub o pane falso nunca
    # mostraria o texto no composer e o envio viraria "partial".
    with patch("app.terminal_input.tmux.has_session", return_value=True), \
         patch("app.terminal_input.tmux.capture_pane", return_value="? for shortcuts\n"), \
         patch.object(terminal_input, "_entrou_no_composer", lambda *_a: True), \
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
                        lambda self, name, text, provider="claude": sent.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 2
    assert sent == ["um", "dois"]
    assert all(e["delivered"] for e in PromptQueue("cc").load())


def test_drain_noop_and_reverts_when_overlay(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude": "deferred")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert PromptQueue("cc").load()[0]["delivered"] is False   # revertida (nao perdida)


def test_drain_does_not_revert_on_send_failure(tmp_queue, monkeypatch):
    PromptQueue("cc").append("um", delivered=False)
    def boom(self, name, text, provider="claude"):
        raise RuntimeError("tty caiu no meio")
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt", boom)
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    # at-most-once: permanece True -> NAO re-enfileira -> nao digita 2x um prompt nao-idempotente.
    assert PromptQueue("cc").load()[0]["delivered"] is True


def test_drain_cheap_check_skips_capture_when_nothing_pending(tmp_queue, monkeypatch):
    PromptQueue("cc").append("ja entregue", delivered=True)
    called = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude": called.append(text) or "sent")
    assert terminal_input.drain("cc", "/no/such.jsonl") == 0
    assert called == []   # nem chamou send_prompt (e nem capture_pane)


def test_drain_skips_entries_before_start_ts(tmp_queue, tmp_path, monkeypatch):
    PromptQueue("cc").append("velha", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2999-01-01T00:00:00Z"}\n', encoding="utf-8")  # start_ts > ts da entrada
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude": sent.append(text) or "sent")
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


# --- _composer_ocupado_pi: guarda anti-colagem (caso real TICKET-000: aviso de grupo ficou no
# composer com Enter engolido e o prompt do cockpit foi digitado em cima, virando UMA mensagem) ---

_REGUA = "─" * 60


def _pane_pi(composer_lines):
    corpo = ["pi v0.83.0", "conversa...", ""]
    return "\n".join(corpo + [_REGUA] + composer_lines + [_REGUA, "🤖 k3 status"])


def test_composer_pi_com_residuo_adia():
    with patch.object(terminal_input, "_capture", return_value=_pane_pi(["texto parado no composer"])):
        assert terminal_input._composer_ocupado_pi("pi-x") is True


def test_composer_pi_vazio_envia():
    with patch.object(terminal_input, "_capture", return_value=_pane_pi([])):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_composer_pi_ilegivel_nao_bloqueia():
    # pane sem réguas (redraw/boot): na dúvida envia — mesma política do resto do arquivo
    with patch.object(terminal_input, "_capture", return_value="pi v0.83.0\nsem reguas aqui"):
        assert terminal_input._composer_ocupado_pi("pi-x") is False


def test_send_prompt_pi_adia_com_residuo(monkeypatch):
    monkeypatch.setattr(terminal_input, "deliverable", lambda name: True)
    monkeypatch.setattr(terminal_input, "_wait_input_ready", lambda name, provider="claude": True)
    with patch.object(terminal_input, "_capture", return_value=_pane_pi(["residuo de enter engolido"])), \
         patch.object(terminal_input, "send_keys") as sk:
        assert TerminalInput().send_prompt("pi-x", "mensagem nova", provider="pi") == "deferred"
    sk.assert_not_called()   # nada digitado por cima do residuo


def test_drain_poda_entradas_de_vida_anterior_da_sessao(tmp_queue, tmp_path, monkeypatch):
    # Sessao morreu devendo (entrada pendente), tmux recriado com o MESMO nome (mesma pasta),
    # transcript RETOMADO (`pi -c` -> start_ts velho). A entrada da vida anterior NAO entrega.
    PromptQueue("cc").append("recado pra sessao que morreu", delivered=False)
    j = tmp_path / "t.jsonl"
    j.write_text('{"timestamp":"2000-01-01T00:00:00Z"}\n', encoding="utf-8")  # transcript velho
    monkeypatch.setattr(terminal_input.tmux, "session_created", lambda name: time.time() + 60)
    sent = []
    monkeypatch.setattr(terminal_input.TerminalInput, "send_prompt",
                        lambda self, name, text, provider="claude": sent.append(text) or "sent")
    assert terminal_input.drain("cc", str(j)) == 0 and sent == []
    assert all(e.get("delivered") is not False for e in PromptQueue("cc").load())  # podada, nao pendente
