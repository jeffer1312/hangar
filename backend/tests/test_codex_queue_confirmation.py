import asyncio
import json

import pytest

from app import pqueue


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


def test_confirm_and_drain_never_writes_the_projection_of_a_rust_codex_terminal(tmp_path, monkeypatch):
    # O hook ocioso agenda `_confirm_and_drain`: com o Rust dono do Codex com terminal, a projeção da fila é
    # dele; o Python gravar `confirmed` nela era desfeito pelo Rust e a bolha sumia e voltava.
    from types import SimpleNamespace
    from app import api, runtime_coordinator
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    queue = pqueue.PromptQueue("codex-term")
    queue.append("[de: outra] orientação", delivered=True)
    path = tmp_path / "rollout.jsonl"
    path.write_text(json.dumps({"type": "response_item", "payload": {"type": "message", "role": "user", "id": "real",
        "content": [{"type": "input_text", "text": "[de: outra] orientação"}]}}) + "\n")
    before = queue.path.read_bytes()
    owner = SimpleNamespace(mode="rust", managed_runtime=lambda name: False, rust_owns=lambda provider, headless: True)
    monkeypatch.setattr(runtime_coordinator, "_current", owner)
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: SimpleNamespace(name=name, jsonl=str(path), provider="codex", headless=False))
    monkeypatch.setattr(api, "_headless", lambda name: False)
    monkeypatch.setattr(api, "_CONFIRM_GRACE", -10.0)     # a entrada já passou do prazo de conferência
    api._confirm_and_drain("codex-term")
    assert queue.path.read_bytes() == before
