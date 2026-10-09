import asyncio
import json

import pytest

from app import pqueue, sse
from app.models import ChatEvent


@pytest.mark.parametrize("reconnect", [False, True])
async def test_codex_confirms_rollout_before_replaying_queue(tmp_path, monkeypatch, reconnect):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("codex-queue")
    queue.append("[de: outra] orientação", delivered=True)
    queue.append("ainda aguardando", delivered=False)
    queue.append("RPC aceito, ainda sem escrita", delivered=True)
    path = tmp_path / "rollout.jsonl"
    path.write_text("")

    def commit():
        path.write_text(json.dumps({"type": "response_item", "payload": {
            "type": "message", "role": "user", "id": "real",
            "content": [{"type": "input_text", "text": "[de: outra] orientação"}],
        }}) + "\n")

    class Adapter:
        async def transcript_stream(self, path, start_offset=None):
            if not reconnect:
                commit()
            yield ChatEvent(kind="user_msg", id="real", text="[de: outra] orientação")

        async def state_monitor(self, name, sid_get):
            if False:
                yield

    if reconnect:
        commit()
    monkeypatch.setattr(sse, "get_adapter", lambda _: Adapter())

    async def follow(self, min_ts=0, emit_confirmed=False):
        if reconnect:
            assert self.load()[0]["confirmed"] is True
            yield ChatEvent(kind="user_msg", id="queued-old", queued_confirmed=True)
        await asyncio.Event().wait()

    monkeypatch.setattr(pqueue.PromptQueue, "follow", follow)
    stream = sse.merged_events("codex-queue", str(path), provider="codex", start_offset=0)
    received = set()
    try:
        async with asyncio.timeout(3):
            async for event in stream:
                if event["event"] == "queue_confirmed":
                    assert json.loads(event["data"])["id"] == "queued-old"
                    received.add("queue_confirmed")
                if event["event"] == "message":
                    assert json.loads(event["data"])["id"] == "real"
                    received.add("message")
                if received == ({"message", "queue_confirmed"} if reconnect else {"message"}):
                    break
    finally:
        await stream.aclose()
    rows = queue.load()
    assert rows[0]["confirmed"] is True
    assert rows[1]["delivered"] is False and not rows[1].get("confirmed")
    assert rows[2]["delivered"] is True and not rows[2].get("confirmed")
    assert not any(row.get("attempts") or row.get("desistiu") for row in rows)


def test_unreadable_rollout_preserves_queue(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("codex-queue")
    queue.append("entregue", delivered=True)
    before = queue.path.read_bytes()
    sse._confirm_codex_queue("codex-queue", str(tmp_path / "missing.jsonl"))
    assert queue.path.read_bytes() == before


async def test_api_reserves_uncertain_codex_send_before_rpc_and_never_replays(tmp_path, monkeypatch):
    from app import api
    from app.adapters.codex.adapter import CodexAdapter

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("uncertain-send")
    attempts = []

    class Client:
        async def request(self, method, params):
            attempts.append(method)
            assert queue.load()[0]["delivered"] is True
            raise ConnectionError("confirmar o envio ficou impossível")

        async def notifications(self):
            await asyncio.Event().wait()
            yield {}

    adapter = CodexAdapter()
    adapter.attach("uncertain-send", Client(), "thread")
    monkeypatch.setattr(api, "get_adapter", lambda _: adapter)
    try:
        result = await api._send_one_codex_locked("uncertain-send", "mensagem única", track_entry=True)
        assert result["ok"] and result["uncertain"] and not result["delivered"]
        assert result["entry_id"] == queue.load()[0]["id"]
        assert await adapter.drain("uncertain-send", "") == 0
        assert attempts == ["turn/start"]
    finally:
        adapter._sessions["uncertain-send"]["bomba"].cancel()
        await asyncio.gather(adapter._sessions["uncertain-send"]["bomba"], return_exceptions=True)


async def test_cancelled_codex_send_keeps_reserved_entry(tmp_path, monkeypatch):
    from app import api

    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    written = asyncio.Event()

    class Adapter:
        async def deliverable(self, name):
            return True

        async def send_prompt(self, name, text):
            written.set()
            await asyncio.Event().wait()

    monkeypatch.setattr(api, "get_adapter", lambda _: Adapter())
    task = asyncio.create_task(api._send_one_codex_locked("cancelled-send", "não repetir"))
    await written.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert pqueue.PromptQueue("cancelled-send").load()[0]["delivered"] is True


def test_idle_codex_confirmation_does_not_release_unproved_entry(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from app import api, runtime_coordinator

    monkeypatch.setattr(runtime_coordinator, "_current", None)
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("idle-codex")
    entry = queue.append("RPC sem confirmação", delivered=True, ts=100)
    path = tmp_path / "rollout.jsonl"
    path.write_text('{"type":"session_meta","payload":{"id":"thread"},"timestamp":"1970-01-01T00:00:00Z"}\n')
    monkeypatch.setattr(api, "_transfer_send_error", lambda _: None)
    monkeypatch.setattr(api, "_cached_info_sync", lambda _: SimpleNamespace(jsonl=str(path), provider="codex"))
    monkeypatch.setattr(api, "_headless", lambda _: False)
    monkeypatch.setattr(api.hook_state, "get_state", lambda _: ("idle", 100))
    monkeypatch.setattr(api, "_agendar_confirmacao", lambda *args: None)
    monkeypatch.setattr(api, "_drenar", lambda *args: pytest.fail("uma confirmação não pode reenviar"))
    api._confirm_and_drain("idle-codex")
    row = queue.load()[0]
    assert row["id"] == entry["id"] and row["delivered"] is True
    assert not row.get("confirmed") and not row.get("attempts") and not row.get("desistiu")
