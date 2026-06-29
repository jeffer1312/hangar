from unittest.mock import patch
from app import terminal_input as ti


def test_send_prompt_waits_for_ready_before_sending():
    # Core bug: msg mandada com claude bootando era engolida. Com o gate de entregabilidade, o pane
    # precisa estar VIVO (has_session) + entregavel (sem overlay) + READY (rodape 'bypass permissions')
    # pra send_prompt enviar e devolver "sent". O wait-for-boot em si (rodape ausente -> espera/timeout)
    # fica coberto por test_wait_input_ready_times_out_then_false.
    ready = "❯ \n⏵⏵ bypass permissions on (shift+tab to cycle)"
    keys = []
    with patch.object(ti.tmux, "has_session", return_value=True), \
         patch.object(ti, "_capture", lambda name: ready), \
         patch.object(ti.time, "sleep", lambda *_: None), \
         patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)):
        assert ti.TerminalInput().send_prompt("s", "oi") == "sent"
    assert keys == ["oi", "Enter"]  # enviou (pane entregavel + ready)


def test_wait_input_ready_times_out_then_false():
    with patch.object(ti, "_capture", lambda name: "bootando sem rodape"), \
         patch.object(ti.time, "sleep", lambda *_: None):
        assert ti._wait_input_ready("s", timeout=0.0) is False


def test_single_question_no_review_submits_without_escape():
    # Pergunta UNICA: o Enter da selecao ja submete; NAO ha tela de "Submit answers". O passo final
    # nao pode mandar Escape (interromperia o Claude que ja recebeu a resposta -> bug "aceitou mas
    # chegou errado"). Cursor abre na linha 1; Down x2 -> linha 3 (= indice 2 + 1) -> guard passa.
    keys = []
    submitted = {"v": False}

    def cap(name):
        if submitted["v"]:
            return "❯ \n⏵⏵ bypass permissions on"          # picker fechou: ja submeteu
        return "Pick one\n❯ 3. OPT-TWO\n  4. OPT-THREE\nEnter to select · Esc to cancel"

    def send(name, k, **kw):
        keys.append(k)
        if k == "Enter":
            submitted["v"] = True

    with patch.object(ti, "send_keys", send), patch.object(ti, "_capture", cap):
        ti.answer_questions("s", [{"kind": "option", "indices": [2], "multi": False, "labels": ["OPT-TWO"]}])
    assert keys == ["Down", "Down", "Enter"]   # navegou e submeteu
    assert "Escape" not in keys                 # SEM Escape espurio (era o interrupt do bug)


def test_single_question_nav_drift_aborts_before_submit():
    # Um Down engolido no redraw: cursor fica na linha 2 quando esperavamos a 3. Guard pega ANTES do
    # Enter -> Escape, levanta, NUNCA submete (era assim que a opcao errada chegava calada no terminal).
    import pytest
    keys = []
    drift = "Pick one\n❯ 2. OPT-ONE\n  3. OPT-TWO\nEnter to select · Esc to cancel"
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: drift):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "option", "indices": [2], "multi": False, "labels": ["OPT-TWO"]}])
    assert "Escape" in keys and "Enter" not in keys  # abortou pre-submit


def test_multi_question_review_submits():
    # Multiplas perguntas: ai SIM existe a tela "Submit answers". Guard por pergunta (linha do cursor) +
    # review final que bate os labels -> Enter submete. Sequencia de capturas: guard Q1, guard Q2, review.
    keys = []
    caps = iter([
        "First q\n❯ 2. A-ONE\n  3. A-TWO\nEsc to cancel",   # Q1: Down x1 -> linha 2 (indice 1)
        "Second q\n❯ 1. B-ZERO\n  2. B-ONE\nEsc to cancel",  # Q2: Down x0 -> linha 1 (indice 0)
        "Review your answers\n ● First q\n   → A-ONE\n ● Second q\n   → B-ZERO\n❯ 1. Submit answers\n  2. Cancel\n",
    ])
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: next(caps)):
        ti.answer_questions("s", [
            {"kind": "option", "indices": [1], "multi": False, "labels": ["A-ONE"]},
            {"kind": "option", "indices": [0], "multi": False, "labels": ["B-ZERO"]},
        ])
    assert keys == ["Down", "Enter", "Enter", "Enter"]  # Q1 Down+Enter, Q2 Enter, submit Enter
    assert "Escape" not in keys


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


def test_text_without_value_raises_before_any_key():
    keys = []
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: ""):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "text", "type_index": 3}])  # value ausente
    assert keys == []  # NENHUMA tecla enviada — TUI intocado


def test_empty_indices_raises_before_any_key():
    keys = []
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: ""):
        with pytest.raises(ValueError):
            ti.answer_questions("s", [{"kind": "option", "indices": [], "multi": False, "labels": ["X"]}])
    assert keys == []


def test_review_substring_not_false_positive():
    # label "A" NAO deve casar dentro de "Apply" no review
    keys = []
    review = "Review your answers\n ● Q1\n   → Apply\n❯ 1. Submit answers\n  2. Cancel\n"
    import pytest
    with patch.object(ti, "send_keys", lambda name, k, **kw: keys.append(k)), \
         patch.object(ti, "_capture", lambda name: review):
        with pytest.raises(ValueError):  # "A" nao e token exato de "Apply" -> mismatch -> Escape
            ti.answer_questions("s", [{"kind": "option", "indices": [0], "multi": False, "labels": ["A"]}])
    assert "Escape" in keys
