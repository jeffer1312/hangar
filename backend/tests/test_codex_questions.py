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
