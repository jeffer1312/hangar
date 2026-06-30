from pathlib import Path
from unittest.mock import call, patch

import pytest

from app import model_picker as mp
from app import terminal_input
from app.terminal_input import TerminalInput

FIX = Path(__file__).parent / "fixtures"
# Panes reais capturados do picker ao vivo (tmux): Opus ativo (6 niveis de esforco, cursor
# na linha 2, esforco xHigh) e cursor sobre Haiku (esforco nao suportado).
PANE_OPUS = (FIX / "pane_model_picker_opus.txt").read_text()
PANE_HAIKU = (FIX / "pane_model_picker_haiku.txt").read_text()


# ── parse de linhas de modelo ────────────────────────────────────────────────
def test_parse_model_rows_opus_fixture():
    rows = mp.parse_model_rows(PANE_OPUS)
    assert [r["number"] for r in rows] == [1, 2, 3, 4]
    assert [r["keyword"] for r in rows] == ["default", "opus", "sonnet", "haiku"]
    # cursor (❯) e ativo (✔) ambos na linha 2 (Opus) ao abrir
    assert [r["cursor"] for r in rows] == [False, True, False, False]
    assert [r["active"] for r in rows] == [False, True, False, False]


def test_parse_model_rows_haiku_cursor_distinct_from_active():
    rows = mp.parse_model_rows(PANE_HAIKU)
    cur = mp.cursor_row(rows)
    assert cur["keyword"] == "haiku" and cur["cursor"] is True
    # ativo (✔) continua no Opus (linha 2), separado do cursor
    assert next(r for r in rows if r["active"])["keyword"] == "opus"


def test_picker_open_detection():
    assert mp.picker_open(PANE_OPUS) is True
    assert mp.picker_open("apenas chat sem picker\n❯ \n") is False


def test_parse_model_rows_ignores_chat_scrollback_numbered_list():
    # Lista numerada no historico do chat NAO deve virar linha de modelo (sem regiao do picker).
    noise = "assistant: passos:\n  1. abrir\n  2. fechar\n❯ \n"
    assert mp.parse_model_rows(noise) == []


# ── parse do esforco atual ───────────────────────────────────────────────────
def test_parse_current_effort_opus_is_xhigh():
    assert mp.parse_current_effort(PANE_OPUS) == "xhigh"


def test_parse_current_effort_haiku_not_supported_is_none():
    assert mp.parse_current_effort(PANE_HAIKU) is None


def test_parse_current_effort_handles_default_suffix():
    pane = PANE_OPUS.replace("◉ xHigh effort ←/→ to adjust", "● High effort (default) ←/→ to adjust")
    assert mp.parse_current_effort(pane) == "high"


# ── contagem de passos (modelo) ──────────────────────────────────────────────
def test_model_nav_steps_from_opus_cursor():
    rows = mp.parse_model_rows(PANE_OPUS)  # cursor na linha 2 (opus)
    assert mp.model_nav_steps(rows, "haiku") == 2  # Down 2
    assert mp.model_nav_steps(rows, "sonnet") == 1  # Down 1
    assert mp.model_nav_steps(rows, "default") == -1  # Up 1
    assert mp.model_nav_steps(rows, "opus") == 0  # ja esta


def test_model_nav_steps_unknown_target_raises():
    rows = mp.parse_model_rows(PANE_OPUS)
    with pytest.raises(ValueError):
        mp.model_nav_steps(rows, "gpt")


def test_model_nav_steps_offscreen_target_uses_fallback_number():
    # Cursor numa linha alta (5) e o alvo "default" (linha 1) fora da viewport -> fallback.
    rows = [
        {"number": 5, "keyword": "opus", "label": "Opus ✔", "cursor": True, "active": True},
        {"number": 4, "keyword": "haiku", "label": "Haiku", "cursor": False, "active": False},
    ]
    assert mp.model_nav_steps(rows, "default") == -4  # 1 - 5


# ── parse da linha de resultado ──────────────────────────────────────────────
def test_parse_result_line_session_only():
    pane = "  ⎿  Set model to Sonnet 4.6 for this session only with high effort\n❯ \n"
    assert mp.parse_result_line(pane) == "Set model to Sonnet 4.6 for this session only with high effort"


def test_parse_result_line_default():
    pane = "  ⎿  Set model to Opus 4.8 and saved as your default for new sessions\n❯ \n"
    assert "saved as your default" in mp.parse_result_line(pane)


def test_parse_result_line_absent():
    assert mp.parse_result_line("❯ \nsem resultado\n") is None


# ── driver (IO mockado): replay da sequencia de teclas ───────────────────────
def _pane_with_effort(level_word: str) -> str:
    # Troca o marcador de esforco do fixture Opus por outro nivel (pra simular o Right).
    return PANE_OPUS.replace("◉ xHigh effort", f"◉ {level_word} effort")


def test_set_model_effort_session_navigates_and_presses_s():
    # Alvo: Sonnet (linha 3) a partir do cursor em Opus (linha 2) => Down 1, depois `s`.
    result_pane = "❯ \n  ⎿  Set model to Sonnet 4.6 for this session only with xhigh effort\n"
    panes = [PANE_OPUS, _pane_with_effort("xHigh"), result_pane]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_model_effort("cc", model="sonnet", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys[:2] == ["/model", "Enter"]  # abre o picker
    assert "Down" in keys and keys.count("Down") == 1  # navega 1 linha (sem teclas de numero)
    assert keys[-1] == "s"  # confirma SO na sessao
    assert "session only" in out["result"]


def test_set_model_effort_default_presses_enter():
    result_pane = "❯ \n  ⎿  Set model to Opus 4.8 and saved as your default for new sessions\n"
    # capturas: abrir, pos-navegacao (opus = 0 passos, mas captura assim mesmo), pos-confirmar
    panes = [PANE_OPUS, PANE_OPUS, result_pane]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(terminal_input.time, "sleep"):
        TerminalInput().set_model_effort("cc", model="opus", scope="default")
    keys = [c.args[1] for c in sk.call_args_list]
    # opus ja e o modelo atual -> sem Down/Up; confirma com Enter (default). 2 Enters: abrir + confirmar.
    assert "Down" not in keys and "Up" not in keys
    assert keys[-1] == "Enter"


def test_set_model_effort_adjusts_effort_with_right():
    # Esforco-so: xHigh -> max (1 Right). Sem modelo: cursor fica na linha atual.
    panes = [
        PANE_OPUS,  # abre: esforco xHigh
        _pane_with_effort("Max"),  # apos 1 Right
        "❯ \n  ⎿  Set model to Opus 4.8 for this session only with max effort\n",
    ]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(terminal_input.time, "sleep"):
        TerminalInput().set_model_effort("cc", effort="max", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys.count("Right") == 1
    assert keys[-1] == "s"


# Pane real do follow-up condicional disparado ao confirmar uma troca de effort (cache re-read).
EFFORT_CONFIRM_PANE = (
    "   Change effort level?\n"
    "   Your next response will be slower and use more tokens\n"
    "\n"
    "   This conversation is cached for the current effort level. Switching to max means the\n"
    "   full history gets re-read on your next message.\n"
    "\n"
    "   ❯ 1. Yes, switch to max\n"
    "     2. No, go back\n"
)


def test_effort_confirm_open_detects_dialog():
    assert mp.effort_confirm_open(EFFORT_CONFIRM_PANE) is True
    assert mp.effort_confirm_open(PANE_OPUS) is False  # picker normal != dialog de confirmacao


def test_set_model_effort_reports_pending_confirm_on_dialog():
    # xHigh -> max (1 Right); apos `s`, o follow-up "Change effort level?" aparece -> NAO auto-
    # confirma: retorna pending_confirm pro usuario decidir via OptionButtons. Ultimo toque = `s`.
    panes = [PANE_OPUS, _pane_with_effort("Max"), EFFORT_CONFIRM_PANE]
    with patch.object(terminal_input.tmux, "capture_pane", side_effect=panes), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(terminal_input.time, "sleep"):
        out = TerminalInput().set_model_effort("cc", effort="max", scope="session")
    keys = [c.args[1] for c in sk.call_args_list]
    assert keys[-1] == "s"  # confirmou a sessao; nao tocou no menu de follow-up
    assert "Enter" not in keys[-2:]  # nao auto-confirmou o "Yes"
    assert out["pending_confirm"] == "max"
    assert out["result"] is None


def test_set_model_effort_aborts_when_picker_never_opens():
    not_open = "❯ \nsem picker aqui\n"
    with patch.object(terminal_input.tmux, "capture_pane", return_value=not_open), patch.object(
        terminal_input, "send_keys"
    ) as sk, patch.object(terminal_input.time, "sleep"):
        with pytest.raises(mp.PickerError) as ei:
            TerminalInput().set_model_effort("cc", model="sonnet")
    assert ei.value.status == 409
    assert sk.call_args_list[-1] == call("cc", "Escape")  # Esc pra nao deixar preso


def test_set_model_effort_rejects_unknown_model():
    with pytest.raises(ValueError):
        TerminalInput().set_model_effort("cc", model="gpt")


def test_set_model_effort_requires_model_or_effort():
    with pytest.raises(ValueError):
        TerminalInput().set_model_effort("cc")
