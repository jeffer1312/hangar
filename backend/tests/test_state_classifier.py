from pathlib import Path
from app.state import classify


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
