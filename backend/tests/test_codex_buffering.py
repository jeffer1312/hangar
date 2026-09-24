import asyncio

import pytest

from app.adapters.codex.adapter import CodexAdapter, map_state


def notice(show=True, *, thread="thread", turn="turn"):
    return {"method": "model/safetyBuffering/updated", "params": {
        "threadId": thread, "turnId": turn, "model": "gpt-6-astra",
        "useCases": [], "reasons": [], "showBufferingUi": show, "fasterModel": None,
    }}


@pytest.fixture
async def watching(monkeypatch, tmp_path):
    from app import pqueue
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = asyncio.Queue()

    class Client:
        closed = False
        async def notifications(self):
            while (event := await queue.get()) is not None:
                yield event
        async def request(self, *args, **kwargs):
            return {}

    adapter = CodexAdapter()
    adapter.attach("cx", Client(), "thread", subscribed=True)
    adapter._sessions["cx"].update(state="working", in_progress=True, turn_id="turn")
    stream = adapter.state_monitor("cx", lambda: "thread")
    assert not (await anext(stream)).codex_buffering
    try:
        yield adapter, queue, stream
    finally:
        await stream.aclose()
        await queue.put(None)
        await adapter._sessions["cx"]["bomba"]


async def test_aviso_reabre_sem_bloquear_turno_e_ignora_outro_turno(watching):
    adapter, queue, stream = watching
    await queue.put(notice())
    state = await asyncio.wait_for(anext(stream), 1)
    assert state.state == "working" and state.codex_buffering
    assert state.codex_question is None
    reopened = adapter.state_monitor("cx", lambda: "thread")
    assert (await anext(reopened)).codex_buffering
    await reopened.aclose()
    await queue.put(notice(False, thread="other"))
    await queue.put(notice(False, turn="previous"))
    await queue.put({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread", "tokenUsage": {"total": {"totalTokens": 1}}}})
    assert (await asyncio.wait_for(anext(stream), 1)).codex_buffering
    await queue.put(notice(False))
    assert not (await asyncio.wait_for(anext(stream), 1)).codex_buffering


@pytest.mark.parametrize("event", [
    {"method": "item/agentMessage/delta", "params": {"threadId": "thread", "delta": "Resposta"}},
    {"method": "item/completed", "params": {"threadId": "thread", "item": {"type": "agentMessage", "text": "Resposta"}}},
    {"method": "turn/completed", "params": {"threadId": "thread", "turn": {"id": "turn"}}},
    {"method": "thread/status/changed", "params": {"threadId": "thread", "status": {"type": "idle"}}},
    {"method": "turn/started", "params": {"threadId": "thread", "turn": {"id": "next"}}},
])
async def test_aviso_some_quando_resposta_chega_ou_turno_muda(watching, event):
    _, queue, stream = watching
    await queue.put(notice())
    assert (await anext(stream)).codex_buffering
    await queue.put(event)
    assert not (await asyncio.wait_for(anext(stream), 1)).codex_buffering


async def test_delta_vazio_nao_finge_que_resposta_chegou(watching):
    _, queue, stream = watching
    await queue.put(notice())
    await anext(stream)
    await queue.put({"method": "item/agentMessage/delta", "params": {"threadId": "thread", "delta": ""}})
    await queue.put({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread", "tokenUsage": {"total": {"totalTokens": 1}}}})
    assert (await asyncio.wait_for(anext(stream), 1)).codex_buffering


def test_mapper_aceita_so_o_sinal_booleano_nativo():
    assert map_state(notice()).buffering is True
    assert map_state(notice(False)).buffering is False
    assert map_state(notice("true")).buffering is None


async def test_aviso_repetido_nao_reabre_apos_primeira_resposta(watching):
    _, queue, stream = watching
    await queue.put(notice())
    await anext(stream)
    await queue.put({"method": "item/agentMessage/delta", "params": {"threadId": "thread", "turnId": "turn", "delta": "Resposta"}})
    assert not (await anext(stream)).codex_buffering
    await queue.put(notice())
    await queue.put({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread", "tokenUsage": {"total": {"totalTokens": 1}}}})
    assert not (await asyncio.wait_for(anext(stream), 1)).codex_buffering
    await queue.put({"method": "turn/started", "params": {"threadId": "thread", "turn": {"id": "next"}}})
    await anext(stream)
    await queue.put(notice(turn="next"))
    assert (await asyncio.wait_for(anext(stream), 1)).codex_buffering


@pytest.mark.parametrize("event", [
    {"method": "item/agentMessage/delta", "params": {"threadId": "thread", "turnId": "previous", "delta": "Atrasada"}},
    {"method": "item/completed", "params": {"threadId": "thread", "turnId": "previous", "item": {"type": "agentMessage", "text": "Atrasada"}}},
])
async def test_resposta_de_outro_turno_nao_apaga_aviso_atual(watching, event):
    _, queue, stream = watching
    await queue.put(notice())
    await anext(stream)
    await queue.put(event)
    await queue.put({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread", "tokenUsage": {"total": {"totalTokens": 1}}}})
    assert (await asyncio.wait_for(anext(stream), 1)).codex_buffering
    await queue.put(notice())
    await queue.put({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread", "tokenUsage": {"total": {"totalTokens": 2}}}})
    assert (await asyncio.wait_for(anext(stream), 1)).codex_buffering


def retrying(details="Connection failed: error sending request"):
    return {"method": "error", "params": {"threadId": "thread", "turnId": "turn", "willRetry": True, "error": {
        "message": "Reconnecting... waiting for network", "additionalDetails": details}}}


async def test_provedor_fora_do_ar_aparece_e_some_quando_a_resposta_chega(watching):
    adapter, queue, stream = watching
    await queue.put(retrying())
    state = await asyncio.wait_for(anext(stream), 1)
    assert (state.state, state.problema) == ("working", "codex_sem_conexao")
    assert state.problema_detalhe == "Connection failed: error sending request"
    assert adapter.problema_de("cx") == "codex_sem_conexao"
    await queue.put(retrying())   # a mesma tentativa de novo não reemite
    await queue.put({"method": "item/agentMessage/delta", "params": {"threadId": "thread", "delta": "Oi"}})
    assert (await asyncio.wait_for(anext(stream), 1)).problema is None


async def test_reconectar_so_com_ferramenta_tambem_tira_o_aviso(watching):
    _, queue, stream = watching
    await queue.put(retrying())
    assert (await asyncio.wait_for(anext(stream), 1)).problema == "codex_sem_conexao"
    await queue.put({"method": "item/started", "params": {"threadId": "thread", "turnId": "turn",
                                                          "item": {"type": "commandExecution", "id": "c1"}}})
    assert (await asyncio.wait_for(anext(stream), 1)).problema is None


async def test_turno_que_falha_fica_marcado_ate_o_proximo_turno(watching):
    _, queue, stream = watching
    await queue.put(retrying("unexpected status 501\n<html>"))
    assert (await asyncio.wait_for(anext(stream), 1)).problema_detalhe == "unexpected status 501"
    await queue.put({"method": "turn/completed", "params": {"threadId": "thread", "turn": {
        "id": "turn", "status": "failed", "error": {"message": "unexpected status 501"}}}})
    state = await asyncio.wait_for(anext(stream), 1)
    assert (state.state, state.problema) == ("idle", "headless_turno_erro")
    await queue.put({"method": "turn/started", "params": {"threadId": "thread", "turn": {"id": "next"}}})
    assert (await asyncio.wait_for(anext(stream), 1)).problema is None


async def test_hook_que_barra_o_prompt_fica_visivel_depois_do_turno_vazio(watching):
    _, queue, stream = watching
    await queue.put({"method": "hook/completed", "params": {"threadId": "thread", "turnId": "turn", "run": {
        "eventName": "userPromptSubmit", "status": "blocked", "source": "plugin",
        "sourcePath": "/home/u/.codex/plugins/cache/mkt/exemplo/local/hooks/hooks.json",
        "entries": [{"kind": "stop", "text": "prompt recusado"}]}}})
    state = await asyncio.wait_for(anext(stream), 1)
    assert (state.problema, state.problema_detalhe) == ("codex_prompt_bloqueado", "exemplo: prompt recusado")
    await queue.put({"method": "turn/completed", "params": {"threadId": "thread", "turn": {"id": "turn", "status": "completed"}}})
    state = await asyncio.wait_for(anext(stream), 1)
    assert (state.state, state.problema) == ("idle", "codex_prompt_bloqueado")
