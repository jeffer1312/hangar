from app.adapters.codex.async_questions import AsyncQuestions


def question(item_id="call-1"):
    return {"id": item_id, "type": "agentMessage", "delivery": "async", "questions": [
        {"title": "Qual cor?", "options": ["Azul", "Verde"]},
        {"title": "Qual tamanho?", "options": None},
    ]}


def answer(item_id="user-1", text="> Qual cor?\n\nAzul"):
    return {"id": item_id, "type": "userMessage", "content": [{"type": "text", "text": text}]}


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


def test_contagem_invalida_cache_da_lista_mesmo_com_mesma_pergunta():
    from app.models import SessionInfo
    from app.sse import _list_sig
    session = SessionInfo(name="cx", state="working", pending_questions=1, question="Qual cor?")
    before = _list_sig([session])
    session.pending_questions = 2
    assert _list_sig([session]) != before
