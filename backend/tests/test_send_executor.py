import asyncio
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from app import api, pqueue
from app.adapters.codex.adapter import CodexAdapter
from app.adapters.codex.appserver import RequestRejected
from app.adapters.codex import sessions


@pytest.mark.parametrize("operation", ["input", "steer", "drain"])
@pytest.mark.parametrize("reject", [False, True])
def test_codex_delivery_with_default_executor_occupied(tmp_path, monkeypatch, operation, reject):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "sessions")
    monkeypatch.setattr(api, "_session_exists", lambda name: True)
    monkeypatch.setattr(api, "_provider_of", lambda name: "codex")
    adapter = CodexAdapter()
    rpc = SimpleNamespace(request=AsyncMock(
        side_effect=RequestRejected("RPC recusado") if reject else None,
        return_value={"turn": {"id": "turn-1"}},
    ))
    adapter.attach("dest", rpc, "thread-1", subscribed=True)
    if operation == "steer":
        adapter._sessions["dest"].update(in_progress=True, turn_id="turn-1")
    monkeypatch.setattr(api, "get_adapter", lambda provider: adapter)
    queue = pqueue.PromptQueue("dest")
    if operation == "drain":
        queue.append("pedido")

    async def run():
        loop = asyncio.get_running_loop()
        loop.set_default_executor(ThreadPoolExecutor(max_workers=1))
        started, release = asyncio.Event(), threading.Event()

        def occupy():
            loop.call_soon_threadsafe(started.set)
            release.wait()

        blocker = loop.run_in_executor(None, occupy)
        await started.wait()
        try:
            if operation == "drain":
                assert await asyncio.wait_for(adapter.drain("dest", ""), 2) == int(not reject)
            else:
                result = await asyncio.wait_for(api.input_prompt(
                    "dest", api.InputBody(text="pedido", steer=operation == "steer")), 2)
                assert result["delivered"] is (not reject)
            rows = queue.load()
            assert len(rows) == 1
            assert rows[0]["text"] == "pedido"
            assert rows[0]["delivered"] is (not reject)
            assert rpc.request.await_count == 1
            assert rpc.request.call_args.args[0] == (
                "turn/steer" if operation == "steer" else "turn/start")
        finally:
            release.set()
            await blocker

    asyncio.run(run())
