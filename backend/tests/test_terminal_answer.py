from unittest.mock import patch
from app import terminal_input as ti


def test_single_select_macro():
    keys = []
    review = "Review your answers\n ● Q1\n   → B\nReady to submit\n❯ 1. Submit answers\n  2. Cancel\n"
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: review):
        ti.answer_questions("s", [{"kind": "option", "indices": [1], "multi": False, "labels": ["B"]}])
    assert keys[:2] == ["Down", "Enter"]   # Down x1 (index 1) + Enter (auto-avanca)
    assert keys[-1] == "Enter"             # submit confirmado (review bate "B")


def test_multi_select_macro_and_mismatch_aborts():
    keys = []
    bad_review = "Review your answers\n ● Q1\n   → Z\n❯ 1. Submit answers\n  2. Cancel\n"
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: bad_review):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "option", "indices": [0, 1], "multi": True, "labels": ["X", "Y"]}])
    # multi: Space, Down, Space, Right ; depois verify falha (review tem Z, nao X/Y) -> Escape
    assert keys[:4] == ["Space", "Down", "Space", "Right"]
    assert "Escape" in keys and "Enter" not in keys[4:]  # nunca submeteu
