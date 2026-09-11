"""Pedidos bidirecionais e perguntas compartilhadas entre desktop, celular e TUI."""
import asyncio
import json

import pytest
import websockets

from app.adapters.codex.adapter import CodexAdapter
from app.adapters.codex.appserver import AppServerClient
from app.adapters.codex.questions import response


def pedido(request_id=1):
    return {"id": request_id, "method": "item/tool/requestUserInput", "params": {
        "threadId": "thread", "turnId": "turn", "itemId": "item",
        "questions": [
            {"id": "destino", "header": "Destino", "question": "Qual destino?",
             "isOther": True, "isSecret": False,
             "options": [{"label": "Local", "description": "Nesta máquina"}]},
            {"id": "detalhes", "header": "Detalhes", "question": "Quais detalhes?",
             "isOther": False, "isSecret": True, "options": None},
        ],
    }}


@pytest.fixture
async def conectado():
    sockets = asyncio.Queue()
    async def receber(ws):
        await sockets.put(ws)
        await ws.wait_closed()
    async with websockets.serve(receber, "127.0.0.1", 0) as server:
        client = AppServerClient()
        await client.connect(f"ws://127.0.0.1:{server.sockets[0].getsockname()[1]}")
        ws = await sockets.get()
        try:
            yield client, ws
        finally:
            await client.close()


async def test_request_do_servidor_nao_resolve_future_com_mesmo_id(conectado):
    client, ws = conectado
    task = asyncio.create_task(client.request("thread/read", {}))
    outgoing = json.loads(await ws.recv())
    await ws.send(json.dumps(pedido(outgoing["id"])))
    notifications = client.notifications()
    assert (await anext(notifications))["method"] == "item/tool/requestUserInput"
    assert not task.done()
    await ws.send(json.dumps({"id": outgoing["id"], "result": {"thread": {"id": "thread"}}}))
    assert await task == {"thread": {"id": "thread"}}
    assert outgoing["id"] in client.server_requests
    await notifications.aclose()


async def test_dois_sse_replay_resposta_nativa_e_resolucao_na_tui(conectado, monkeypatch):
    client, ws = conectado
    ad = CodexAdapter()
    ad.attach("cx", client, "thread", subscribed=True)
    async def running(name):
        return client
    async def settings(name):
        return {}
    monkeypatch.setattr(ad, "ensure_running", running)
    monkeypatch.setattr(ad, "read_settings", settings)
    monkeypatch.setattr(ad, "read_rate_limits", settings)
    streams = [ad.state_monitor("cx", lambda: "thread") for _ in range(2)]
    try:
        for stream in streams:
            assert (await anext(stream)).codex_question is None
        await ws.send(json.dumps(pedido()))
        for stream in streams:
            event = await asyncio.wait_for(anext(stream), 2)
            assert event.state == "awaiting_input"
            assert event.codex_question["request_id"] == 1
            assert event.codex_question["questions"][1]["isSecret"]
        # Reabrir uma conexão recupera o mesmo pedido, sem consumi-lo da outra.
        await streams[1].aclose()
        streams[1] = ad.state_monitor("cx", lambda: "thread")
        assert (await anext(streams[1])).codex_question["request_id"] == 1
        await ad.answer_questions("cx", 1, [
            {"question_id": "destino", "kind": "option", "indices": [0]},
            {"question_id": "detalhes", "kind": "text", "value": "Preservar acentos"},
        ])
        assert json.loads(await ws.recv()) == {"jsonrpc": "2.0", "id": 1, "result": {
            "answers": {"destino": {"answers": ["Local"]},
                        "detalhes": {"answers": ["Preservar acentos"]}}}}
        with pytest.raises(ValueError, match="já está sendo"):
            await client.respond(1, {})
        # Este aviso é idêntico quando outro cliente (a TUI) responde.
        await ws.send(json.dumps({"method": "serverRequest/resolved", "params": {
            "threadId": "thread", "requestId": 1}}))
        for stream in streams:
            assert (await asyncio.wait_for(anext(stream), 2)).codex_question is None
        with pytest.raises(ValueError, match="respondida ou cancelada"):
            await ad.answer_questions("cx", 1, [])
        await streams[1].aclose()
        streams[1] = ad.state_monitor("cx", lambda: "thread")
        assert (await anext(streams[1])).codex_question is None
        # A pergunta seguinte tem identidade própria, mesmo com texto idêntico.
        await ws.send(json.dumps(pedido("segunda")))
        for stream in streams:
            event = await asyncio.wait_for(anext(stream), 2)
            assert event.codex_question["request_id"] == "segunda"
        assert await ad.deliverable("cx") is False
        # A TUI resolve esta sem nenhuma resposta enviada pelo Hangar.
        await ws.send(json.dumps({"method": "serverRequest/resolved", "params": {
            "threadId": "thread", "requestId": "segunda"}}))
        for stream in streams:
            assert (await asyncio.wait_for(anext(stream), 2)).codex_question is None
    finally:
        for stream in streams:
            await stream.aclose()


async def test_cancelamento_sem_sse_nao_reabre_pedido(conectado):
    client, ws = conectado
    notifications = client.notifications()
    await ws.send(json.dumps(pedido("req")))
    await anext(notifications)
    await ws.send(json.dumps({"method": "turn/completed", "params": {
        "threadId": "thread", "turn": {"id": "turn", "status": "interrupted"}}}))
    await anext(notifications)
    assert not client.server_requests
    await notifications.aclose()


async def test_async_visivel_trabalhando_reabre_e_responde_sem_terminal(conectado, monkeypatch, tmp_path):
    from unittest.mock import AsyncMock
    from app import pqueue
    from tests.test_codex_async_questions import question, answer
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    client, ws = conectado
    adapter = CodexAdapter()
    adapter.attach("cx", client, "thread", subscribed=True)
    adapter._sessions["cx"].update(state="working", in_progress=True)
    monkeypatch.setattr(adapter, "read_settings", AsyncMock(return_value={}))
    monkeypatch.setattr(adapter, "read_rate_limits", AsyncMock(return_value=None))
    stream = adapter.state_monitor("cx", lambda: "thread")
    try:
        assert (await anext(stream)).codex_question is None
        await ws.send(json.dumps({"method": "item/completed", "params": {"threadId": "thread", "item": question()}}))
        event = await asyncio.wait_for(anext(stream), 2)
        assert event.state == "working"
        assert event.codex_question["is_async"]
        assert adapter.async_question_status("cx") == (2, "Qual cor?")
        await stream.aclose()
        stream = adapter.state_monitor("cx", lambda: "thread")
        assert (await anext(stream)).codex_question == event.codex_question
        await ws.send(json.dumps({"method": "item/completed", "params": {"threadId": "thread", "item": answer()}}))
        event = await asyncio.wait_for(anext(stream), 2)
        assert event.codex_question["questions"][0]["question"] == "Qual tamanho?"
        responding = asyncio.create_task(adapter.answer_questions("cx", event.codex_question["request_id"], [
            {"question_id": "answer", "kind": "text", "value": "Grande"},
        ]))
        request = json.loads(await ws.recv())
        assert request["method"] == "turn/start"
        assert request["params"] == {"threadId": "thread", "input": [{"type": "text", "text": "> Qual tamanho?\n\nGrande"}]}
        await ws.send(json.dumps({"id": request["id"], "result": {"turn": {"id": "turn"}}}))
        await responding
        assert (await anext(stream)).codex_question is None
        assert pqueue.PromptQueue("cx").load() == []
        assert adapter.async_question_status("cx") == (0, None)
        with pytest.raises(ValueError):
            await adapter.answer_questions("cx", event.codex_question["request_id"], [])
    finally:
        await stream.aclose()


async def test_falha_na_resposta_async_mantem_pergunta(conectado, monkeypatch):
    from unittest.mock import AsyncMock
    from tests.test_codex_async_questions import question
    client, _ = conectado
    adapter = CodexAdapter()
    adapter.attach("cx", client, "thread", subscribed=True)
    state = adapter._sessions["cx"]["async_questions"]
    state.observe(question())
    request_id = state.pending()["request_id"]
    monkeypatch.setattr(client, "request", AsyncMock(side_effect=ConnectionError("sem conexão")))
    with pytest.raises(ConnectionError):
        await adapter.answer_questions("cx", request_id, [{"question_id": "answer", "kind": "text", "value": "Azul"}])
    assert state.count == 2
    assert state.pending()["request_id"] == request_id


@pytest.mark.parametrize("answers", [[], [{"kind": "text", "question_id": "outra", "value": "x"}],
    [{"kind": "option", "question_id": "destino", "indices": [999]}]])
def test_respostas_incompletas_ou_de_outra_pergunta_sao_recusadas(answers):
    from app.adapters.codex.questions import pending
    class Client:
        server_requests = {1: pedido()}
    with pytest.raises(ValueError):
        response(pending(Client(), "thread"), answers)


async def test_api_codex_responde_com_painel_aberto_sem_digitar_no_terminal(monkeypatch):
    from types import SimpleNamespace
    from app import api
    calls = []
    class Adapter:
        async def answer_questions(self, *args):
            calls.append(args)
    monkeypatch.setattr(api, "_loop_servidor", asyncio.get_running_loop())
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: SimpleNamespace(provider="codex"))
    monkeypatch.setattr(api, "get_adapter", lambda provider: Adapter())
    monkeypatch.setattr(api, "_recusa_se_painel_aberto", lambda name: pytest.fail("consultou o terminal"))
    body = api.AnswerBody(request_id="req", answers=[api.AnswerItem(
        question_id="pergunta", kind="text", value="Resposta")])
    assert await asyncio.to_thread(api.answer, "cx", body) == {"ok": True, "fallback": False}
    assert calls[0][0:2] == ("cx", "req")
    assert calls[0][2][0]["question_id"] == "pergunta"
