from pathlib import Path
from unittest.mock import patch

import pytest

from app import state as state_mod
from app.state import classify, StateMonitor


def test_working_with_spinner_label():
    state, label, q, opts = classify("● PONG\n\n✽ Elucidating…\n\n❯ \n  ← for agents\n")
    assert state == "working"
    assert label == "Elucidating…"


def test_working_elapsed_form():
    state, label, q, opts = classify("✻ Crunched for 8s\n❯ \n")
    assert state == "working" and label == "Crunched for 8s"


def test_assistant_bullet_is_not_spinner():
    # ● is the message bullet, not a spinner glyph
    state, label, q, opts = classify("● PONG\n❯ \n")
    assert state == "idle"


def test_awaiting_input_parses_question_and_options():
    pane = (
        "   Claude has written up a plan. Would you like to proceed?\n"
        "\n"
        "   ❯ 1. Yes, and bypass permissions\n"
        "     2. Yes, manually approve edits\n"
        "     3. No, keep planning\n"
    )
    state, label, question, options = classify(pane)
    assert state == "awaiting_input"
    assert question == "Claude has written up a plan. Would you like to proceed?"
    assert options == ["Yes, and bypass permissions", "Yes, manually approve edits", "No, keep planning"]


def test_numbered_list_without_cursor_stays_idle():
    # a plain numbered list (no ❯ cursor on an option) is NOT a widget
    state, *_ = classify("Steps:\n  1. do this\n  2. do that\n❯ \n")
    assert state == "idle"


def test_idle_when_no_spinner_or_widget():
    state, label, q, opts = classify("❯ \n  ← for agents\n")
    assert state == "idle"


def test_real_fixtures():
    fx = Path(__file__).parent / "fixtures"
    assert classify((fx / "pane_idle.txt").read_text())[0] == "idle"
    s, lbl, *_ = classify((fx / "pane_thinking.txt").read_text())
    assert s == "working" and lbl == "Elucidating…"
    s2, _, q2, opts2 = classify((fx / "pane_awaiting_input.txt").read_text())
    assert s2 == "awaiting_input" and opts2 and "proceed?" in (q2 or "")


def test_quoted_menu_in_scrollback_is_idle():
    """O assistente citou o menu nativo na propria mensagem ("❯ 1. Yes, switch to xhigh / 2. No,
    go back"). Esse "❯ N." vive no scrollback com o composer vivo (input box) renderizado ABAIXO,
    entao NAO e um widget selecionavel -> idle, nao awaiting_input (senao o app trava num menu
    fantasma). Captura real do pane que travou o claude-pocket."""
    fx = Path(__file__).parent / "fixtures" / "pane_quoted_menu_scrollback.txt"
    state, _, _, options = classify(fx.read_text())
    assert state == "idle", f"menu citado virou {state} com opcoes {options}"


def test_askuserquestion_real_fixture():
    """A AskUserQuestion (widget do assistente) capturada de verdade do pane: o classificador
    tem que extrair a pergunta e as opcoes reais, escopadas ao box do picker."""
    fx = Path(__file__).parent / "fixtures" / "pane_askuserquestion.txt"
    state, _, question, options = classify(fx.read_text())
    assert state == "awaiting_input"
    assert "Captura de formato" in (question or "")
    assert "Opção Alpha" in options
    assert "Opção Bravo" in options


def test_picker_options_exclude_scrollback_numbered_lines():
    """Bug real: o classificador coletava TODA linha numerada do pane. Uma lista numerada no
    scrollback (acima de um bullet) NAO pode vazar pras opcoes do picker."""
    pane = (
        "● Earlier I listed steps:\n"
        "  1. first scrollback item\n"
        "  2. second scrollback item\n"
        "\n"
        "● Now pick one:\n"
        "   ❯ 1. Real Alpha\n"
        "     2. Real Bravo\n"
        "Enter to select · ↑/↓ to navigate · Esc to cancel\n"
    )
    state, _, _, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Real Alpha", "Real Bravo"]
    assert all("scrollback" not in o for o in options)


def test_chip_header_excludes_prose_numbered_list_above():
    """Bug real (web mostrava 10 opcoes): uma AskUserQuestion logo abaixo de uma LISTA NUMERADA
    EM PROSA, sem bullet ● entre elas. O chip ☐ do widget e o topo do bloco; os '1. ... 2. ...'
    da prosa ficam acima do chip e NAO podem virar opcoes falsas."""
    pane = (
        "● Caminho único e limpo:\n"
        "  1. Aqui digita /exit\n"
        "  2. No shell: tmux kill-server\n"
        "  3. Abre tmux limpo\n"
        "  4. Retoma esta conversa\n"
        "\n"
        "☐ Status bar\n"
        "Status bar do tmux: religar pra ver sessão/janelas?\n"
        "\n"
        "❯ 1. Religar minimal\n"
        "  2. Deixar off\n"
        "Enter to select · ↑/↓ to navigate · Esc to cancel\n"
    )
    state, _, question, options = classify(pane)
    assert state == "awaiting_input"
    assert options == ["Religar minimal", "Deixar off"]
    assert "religar" in (question or "").lower()


@pytest.mark.asyncio
async def test_monitor_emits_only_on_change():
    # Dedup: o spinner byte-identico no 2o poll NAO re-emite. Vai do spinner pro MENU (awaiting_input
    # e autoritativo/sem debounce) em vez de voltar pra idle: idle->working tem IDLE_DEBOUNCE e o
    # status_line flipa ao perder o spinner (re-emite working), o que polui um teste de "so na mudanca".
    panes = iter([
        "❯ \n",                  # idle
        "✽ Elucidating…\n",      # working
        "✽ Elucidating…\n",      # byte-identico -> NAO emite de novo
        "❯ 1. Religar\n  2. Deixar off\nEnter to select · ↑/↓ to navigate · Esc to cancel\n",  # menu
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if len(seen) == 3:
                break
    # 3 emits, nao 4: o 2o spinner identico foi suprimido (senao haveria um working extra no meio).
    assert seen == [("idle", None), ("working", "Elucidating…"), ("awaiting_input", None)]


@pytest.mark.asyncio
async def test_monitor_frozen_completed_marker_reads_idle():
    """Regression (bug #4): a completed-turn marker ("✻ Worked for 13s") lingers in the pane
    while Claude is idle. It is shaped exactly like a live spinner; a single frame can't tell
    them apart (classify reports 'working' for any spinner). The proof it's frozen is that it
    NEVER changes — so the monitor reports 'working' briefly, then DOWNGRADES to 'idle' after
    STALE_LIMIT identical polls. Net result the UI cares about: it does NOT stay stuck working."""
    frozen = "● the answer\n✻ Worked for 13s\n────\n❯ \n────\n  ⏵⏵ bypass permissions on\n"
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", return_value=frozen):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if ev.state == "idle":
                break
    assert seen[-1] == ("idle", None)                 # converge pra idle (composer nao trava working)
    assert ("working", "Worked for 13s") in seen      # flash breve antes do downgrade (tradeoff anti-flicker)


@pytest.mark.asyncio
async def test_monitor_animating_spinner_reads_working():
    """A live spinner animates (glyph cycles) while its label holds — that change is the
    proof of life that distinguishes it from a frozen marker."""
    panes = iter([
        "❯ \n",                       # idle baseline
        "✽ Pondering…\n❯ \n",         # spinner appears
        "✶ Pondering…\n❯ \n",         # glyph cycled -> alive (same label)
        "❯ \n",                       # idle again
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if ev.state == "working":
                break
    assert seen[0] == ("idle", None)
    assert seen[-1] == ("working", "Pondering…")


def test_is_overlay_true_with_nav_footer():
    pane = "alguma conversa\n● resposta\n────────\n  Esc to cancel · Enter to select\n"
    assert state_mod.is_overlay(pane) is True


def test_is_overlay_false_without_footer():
    assert state_mod.is_overlay("● PONG\n❯ \n") is False


def test_status_line_is_the_chrome_below_the_input_box():
    pane = (
        "● the answer\n"
        "✻ Worked for 1s\n"
        "──────────────────────────────\n"
        "❯ \n"
        "──────────────────────────────\n"
        "  📂 proj │ 💬 43k/606 40k/1M │ 💵 $0.47 │ 🕐 14:00\n"
        "  ⏵⏵ bypass permissions on · ← for agents\n"
    )
    sl = state_mod.status_line(pane)
    assert "💬 43k/606" in sl and "💵 $0.47" in sl
    assert "the answer" not in sl   # conversation excluded
    assert "✻ Worked" not in sl     # spinner / completed marker excluded
