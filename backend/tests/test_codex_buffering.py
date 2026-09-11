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
