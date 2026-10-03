"""Encerramento da prévia com heartbeat sintético, sem terminal ou HTTP."""
import asyncio

import pytest

from app import terminal_observer, tmux
from app.preview import PreviewBroker


@pytest.mark.parametrize("external_cancel", [False, True])
async def test_preview_cleanup_preserves_external_cancellation(monkeypatch, external_cancel):
    blocked = []
    def deny(*args, **kwargs):
        blocked.append(args)
        raise AssertionError("real tmux forbidden")
    monkeypatch.setattr(tmux, "_run", deny)
    monkeypatch.setattr(tmux, "RUN", deny)
    started, cleaning, finish = asyncio.Event(), asyncio.Event(), asyncio.Event()
    class Source:
        async def __aenter__(self):
            return self
        async def __aexit__(self, *exc):
            return None
        async def watch(self):
            started.set()
            try:
                await asyncio.Future()
            except asyncio.CancelledError:
                cleaning.set()
                await finish.wait()
                raise
    monkeypatch.setattr(terminal_observer, "lease", lambda *args: Source())
    broker = PreviewBroker("cleanup-fixture", "claude", lambda: "binding")
    async def observe():
        await started.wait()
    monkeypatch.setattr(broker, "_observe_loop", observe)
    task = asyncio.create_task(broker._loop())
    try:
        await asyncio.wait_for(cleaning.wait(), 1)
        if external_cancel:
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            finish.set()
            await task
            assert not task.cancelled()
    finally:
        finish.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    assert blocked == []
