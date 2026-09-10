"""Voz usa a thread atual, preserva o consumidor do chat e encerra com o navegador."""
import asyncio
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from app import codex_voice
from app.adapters.codex.adapter import CodexAdapter


class Socket:
    headers = {"origin": "http://localhost", "host": "localhost"}

    def __init__(self, voice="coral", control="stop"):
        self.sent = []
        self.input = asyncio.Queue()
        self.input.put_nowait(json.dumps({"type": "start", "sdp": "v=0\r\n", "voice": voice}))
        self.control = control

    async def accept(self):
        pass

    async def send_json(self, value):
        self.sent.append(value)
        if value.get("method") == "thread/realtime/sdp":
            self.input.put_nowait(self.control)

    async def receive_text(self):
        return await self.input.get()

    async def close(self, **kwargs):
        pass


@pytest.fixture
def setup(monkeypatch):
    monkeypatch.setattr(codex_voice, "require_auth", lambda _: None)
    monkeypatch.setattr(codex_voice, "_origem_aceita", lambda *_: True)
    monkeypatch.setattr(codex_voice.runtime_config, "get", lambda key: key == "codex_voice_beta")
    client = SimpleNamespace(request=AsyncMock())
    sess = {"client": client, "thread_id": "existing-thread"}
    adapter = SimpleNamespace(_sessions={"sess": sess}, ensure_running=AsyncMock(return_value=client))
    broker = SimpleNamespace(start=AsyncMock(), close=AsyncMock(), target_event=AsyncMock())

    def factory(_adapter, _name, _sess, send):
        async def start(sdp, voice):
            broker.reader = asyncio.create_task(asyncio.Event().wait())
            await send({"method": "thread/realtime/sdp", "params": {"sdp": "answer"}})
        broker.start.side_effect = start
        return broker
    monkeypatch.setattr(codex_voice, "VoiceBroker", factory)

    async def monitor(*_):
        yield SimpleNamespace(state="idle")
        await asyncio.Future()

    adapter.state_monitor = monitor
    client.broker = broker
    return adapter, client, sess


async def test_recurso_desligado_recusa_antes_de_ligar_sessao(setup, monkeypatch):
    adapter, _, _ = setup
    monkeypatch.setattr(codex_voice.runtime_config, "get", lambda _: False)
    ws = Socket()
    await codex_voice.voice_ws(ws, "sess", adapter)
    adapter.ensure_running.assert_not_called()


async def test_desligar_em_outro_aparelho_encerra_chamada_no_heartbeat(setup, monkeypatch):
    adapter, _, _ = setup
    valores = iter([True, False])
    monkeypatch.setattr(codex_voice.runtime_config, "get", lambda _: next(valores))
    ws = Socket(control="ping")
    await codex_voice.voice_ws(ws, "sess", adapter)
    assert {"type": "error", "code": "disabled"} in ws.sent


async def test_conecta_na_thread_atual_e_para_sem_fechar_cliente(setup):
    adapter, client, sess = setup
    ws = Socket()
    await asyncio.wait_for(codex_voice.voice_ws(ws, "sess", adapter), 2)
    client.broker.start.assert_awaited_once_with("v=0\r\n", "coral")
    client.broker.close.assert_awaited_once()
    client.request.assert_not_called()
    assert "voice_events" not in sess
    assert adapter._sessions["sess"] is sess


async def test_segunda_chamada_nao_encerra_a_primeira(setup):
    adapter, client, sess = setup
    owner = sess["voice_events"] = asyncio.Queue()
    ws = Socket()
    await codex_voice.voice_ws(ws, "sess", adapter)
    assert ws.sent == [{"type": "error", "code": "busy"}]
    assert sess["voice_events"] is owner
    client.request.assert_not_called()


async def test_voz_invalida_nao_chega_ao_codex(setup):
    adapter, client, _ = setup
    ws = Socket(voice="viola")
    await codex_voice.voice_ws(ws, "sess", adapter)
    client.request.assert_not_called()
    assert ws.sent[-1]["code"] == "failed"


async def test_origem_recusada_nao_conecta(setup, monkeypatch):
    adapter, client, _ = setup
    monkeypatch.setattr(codex_voice, "_origem_aceita", lambda *_: False)
    await codex_voice.voice_ws(Socket(), "sess", adapter)
    adapter.ensure_running.assert_not_called()


async def test_eventos_de_voz_nao_roubam_turn_completed(monkeypatch, tmp_path):
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    adapter = CodexAdapter()
    client = SimpleNamespace(server_requests={})
    notifications = [
        {"method": "thread/realtime/sdp", "params": {"threadId": "other", "sdp": "wrong"}},
        {"method": "item/completed", "params": {"threadId": "other", "item": {"type": "agentMessage", "text": "wrong"}}},
        {"method": "item/completed", "params": {"threadId": "existing-thread", "item": {"type": "agentMessage", "text": "answer"}}},
        {"method": "turn/completed", "params": {"threadId": "existing-thread", "turn": {"id": "turn-1"}}},
    ]

    async def stream():
        for event in notifications:
            yield event

    client.notifications = stream
    adapter.attach("voice-test", client, "existing-thread", subscribed=True)
    sess = adapter._sessions["voice-test"]
    sess["voice_events"] = asyncio.Queue()
    monkeypatch.setattr(adapter, "drain", AsyncMock())
    states = []
    try:
        await adapter._consumir("voice-test", client, sess, states.append)
        assert sess["voice_events"].qsize() == 2
        assert sess["voice_events"].get_nowait()["params"]["item"]["text"] == "answer"
        adapter.drain.assert_awaited_once()
        assert sess["in_progress"] is False
        assert states[-1].state == "idle"
    finally:
        adapter._sessions.pop("voice-test", None)
