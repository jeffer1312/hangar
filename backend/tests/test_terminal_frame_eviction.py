"""Expiração de quadro sem I/O real nem troca de identidade da sessão."""
import asyncio
import time

import pytest

from app import state, terminal_observer


@pytest.fixture(autouse=True)
def isolated_terminal(monkeypatch):
    forbidden = []
    def deny(*args, **kwargs):
        forbidden.append(args)
        raise AssertionError("real tmux forbidden in frame eviction test")
    monkeypatch.setattr(state.tmux, "_run", deny)
    monkeypatch.setattr(state.tmux, "RUN", deny)
    monkeypatch.setattr(state.tmux, "capture_pane", lambda name: "fresh frame")
    state._frames.clear()
    state._frame_tags.clear()
    state._frames_inflight.clear()
    terminal_observer.configure(None, None)
    yield
    terminal_observer.configure(None, None)
    state._frames.clear()
    state._frame_tags.clear()
    assert not forbidden


def test_expiring_another_sessions_frame_preserves_lease_epoch():
    async def run():
        old = terminal_observer.lease("frame-expired-other", "claude", lambda: "thread")
        await old.start()
        try:
            with terminal_observer.use(old):
                before = terminal_observer.stamp(old.name)
                started = time.monotonic() - 61
                state._frames[old.name] = (started, "expired frame")
                state._frame_tags[old.name] = before
                terminal_observer._analysis[old.name] = (before, started, "expired frame", {})
            assert await state.shared_capture("frame-fresh", 0) == "fresh frame"
            with terminal_observer.use(old):
                assert terminal_observer.stamp(old.name) == before
                assert not terminal_observer.retired(old.name)
            assert old.name not in state._frames
            assert old.name not in state._frame_tags
            assert old.name not in terminal_observer._analysis
        finally:
            await old.close()
    asyncio.run(run())
