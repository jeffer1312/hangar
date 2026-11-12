import json

import pytest

from app.adapters.codex.async_questions import AsyncQuestions


def question(item_id="call-1"):
    return {"id": item_id, "type": "agentMessage", "delivery": "async", "questions": [
        {"title": "Qual cor?", "options": ["Azul", "Verde"]},
        {"title": "Qual tamanho?", "options": None},
    ]}


def answer(item_id="user-1", text="> Qual cor?\n\nAzul"):
    return {"id": item_id, "type": "userMessage", "content": [{"type": "text", "text": text}]}


def terminal_answer(item_id="user-1", question_item_id="call-1", index=0, title="Qual cor?", value="Azul"):
    reply = [{"answer": value, "question": title,
              "questionItemId": json.dumps(["request_user_input_async", question_item_id, index])}]
    return answer(item_id, "<send_user_message_question_reply>\n" + json.dumps(reply, ensure_ascii=False)
                  + "\n</send_user_message_question_reply>")


def test_terminal_reply_resolves_by_item_id_instead_of_repeated_title():
    state = AsyncQuestions("thread")
    state.observe(question())
    state.observe(question("call-2"))
    reply = terminal_answer(question_item_id="call-2")
    assert state.observe(reply)
    assert state.count == 3
    assert state.pending()["request_id"] == "async:thread:call-1:0"
    assert not state.observe(reply)
    state.observe(terminal_answer("reply-2", "call-2", 1, "Qual tamanho?", "Grande"))
    assert state.count == 2
    state.observe(question("call-2"))
    assert state.count == 2


def test_terminal_replies_survive_history_reload_and_live_reply_during_load():
    reply = terminal_answer()
    state = AsyncQuestions("thread")
    state.observe(reply)
    state.hydrate({"turns": [{"items": [question()]}]})
    assert state.count == 1
    assert state.pending()["questions"][0]["question"] == "Qual tamanho?"
    restored = AsyncQuestions("thread")
    restored.hydrate({"turns": [{"items": [question(), reply]}]})
    assert restored.pending() == state.pending()


@pytest.mark.parametrize("reply", [
    "não é JSON", {}, [None],
    [{"answer": "Azul", "questionItemId": "não é JSON"}],
    [{"answer": "Azul", "questionItemId": '["outra_tool", "call-1", 0]'}],
    [{"answer": "Azul", "questionItemId": '["request_user_input_async", "call-1", true]'}],
    [{"answer": "Azul", "questionItemId": '["request_user_input_async", "call-1", -1]'}],
    [{"answer": "", "questionItemId": '["request_user_input_async", "call-1", 0]'}],
    [{"answer": "Azul", "questionItemId": '["request_user_input_async", "outra", 0]'}],
])
def test_invalid_or_unrelated_terminal_reply_keeps_pending_questions(reply):
    state = AsyncQuestions("thread")
    state.observe(question())
    text = "<send_user_message_question_reply>\n" + json.dumps(reply) + "\n</send_user_message_question_reply>"
    assert not state.observe(answer(text=text))
    assert state.count == 2


def test_terminal_reply_batch_resolves_each_question():
    state = AsyncQuestions("thread")
    state.observe(question())
    replies = [
        {"answer": "Azul", "questionItemId": '["request_user_input_async", "call-1", 0]'},
        {"answer": "Grande", "questionItemId": '["request_user_input_async", "call-1", 1]'},
    ]
    text = "<send_user_message_question_reply>\n" + json.dumps(replies) + "\n</send_user_message_question_reply>"
    assert state.observe(answer(text=text))
    assert state.pending() is None


def test_perguntas_individuais_sobrevivem_ao_turno_e_reabertura():
    state = AsyncQuestions("thread")
    state.observe(question())
    first = state.pending()
    assert state.count == 2
    assert first["questions"][0]["question"] == "Qual cor?"
    state.observe(answer())
    assert state.count == 1
    assert state.pending()["request_id"] != first["request_id"]
    assert state.pending()["questions"][0]["question"] == "Qual tamanho?"
    restored = AsyncQuestions("thread")
    restored.hydrate({"turns": [{"items": [question(), answer()]}]})
    assert restored.pending() == state.pending()


def test_historico_atrasado_preserva_resposta_recebida_ao_vivo():
    state = AsyncQuestions("thread")
    state.observe(answer())
    state.hydrate({"turns": [{"items": [question()]}]})
    assert state.count == 1
    state.observe(question())
    assert state.count == 1


def test_resposta_nativa_e_pergunta_de_outra_thread():
    state = AsyncQuestions("thread")
    state.observe(question())
    payload = state.pending()
    text = state.response(payload["request_id"], [{"question_id": "answer", "kind": "option", "indices": [0]}])
    assert text == "> Qual cor?\n\nAzul"
    other = AsyncQuestions("other")
    other.observe(question())
    assert other.pending()["request_id"] != payload["request_id"]


def test_mesmo_titulo_consumido_uma_vez_e_texto_comum_nao_responde():
    state = AsyncQuestions("thread")
    state.observe(question())
    state.observe(question("call-2"))
    state.observe(answer(text="Qual cor? Azul"))
    assert state.count == 4
    state.observe(answer("reply"))
    state.observe(answer("reply"))
    assert state.count == 3


def test_eco_da_resposta_local_nao_responde_pergunta_repetida():
    state = AsyncQuestions("thread")
    state.observe(question())
    state.observe(question("call-2"))
    request_id = state.pending()["request_id"]
    state.record_answer(request_id, "> Qual cor?\n\nAzul")
    assert state.count == 3
    state.observe(answer())
    assert state.count == 3
    restored = AsyncQuestions("thread")
    restored.hydrate({"turns": [{"items": [question(), question("call-2"), answer()]}]})
    restored.record_answer(request_id, "> Qual cor?\n\nAzul")
    assert restored.count == 3


def test_inicio_vazio_nao_esconde_perguntas_do_item_completo():
    state = AsyncQuestions("thread")
    state.observe({**question(), "questions": None})
    state.observe(question())
    assert state.count == 2


def test_pergunta_pulada_nao_volta_pelo_historico_nem_na_reabertura():
    state = AsyncQuestions("thread")
    state.observe(question())
    skipped = state.pending()["request_id"]
    state.skip(skipped)
    assert state.pending()["questions"][0]["question"] == "Qual tamanho?"
    state.hydrate({"turns": [{"items": [question()]}]})
    assert state.count == 1
    restored = AsyncQuestions("thread", state.skipped)
    restored.hydrate({"turns": [{"items": [question()]}]})
    assert restored.pending() == state.pending()
    try:
        state.skip(skipped)
    except ValueError:
        pass
    else:
        raise AssertionError("pular de novo deveria recusar")
