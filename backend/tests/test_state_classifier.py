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


@pytest.mark.asyncio
async def test_monitor_emits_only_on_change():
    panes = iter([
        "❯ \n",                  # idle
        "✽ Elucidating…\n",      # working
        "✽ Elucidating…\n",      # still working, same label (no emit)
        "❯ \n",                  # idle again
    ])
    with patch.object(state_mod.tmux, "has_session", return_value=True), \
         patch.object(state_mod.tmux, "capture_pane", side_effect=lambda *a, **k: next(panes)):
        mon = StateMonitor("cc", poll=0.001)
        seen = []
        async for ev in mon.stream():
            seen.append((ev.state, ev.label))
            if len(seen) == 3:
                break
    assert seen == [("idle", None), ("working", "Elucidating…"), ("idle", None)]
