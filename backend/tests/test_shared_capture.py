"""Quadro de pane compartilhado entre estado e prévia dos consumidores Python."""
import asyncio
import threading
import time


from app import state


def _fake_capture(calls, text="● oi\n", delay=0.05):
    def capture(name):
        calls.append(name)
        time.sleep(delay)
        return text
    return capture


def test_concurrent_consumers_share_one_capture(monkeypatch):
    calls = []
    monkeypatch.setattr(state.tmux, "capture_pane", _fake_capture(calls))

    async def run():
        return await asyncio.gather(state.shared_capture("s", 0.5), state.shared_capture("s", 0.1))

    assert asyncio.run(run()) == ["● oi\n", "● oi\n"]
    assert calls == ["s"]


def test_fresh_frame_reused_stale_recaptured(monkeypatch):
    calls = []
    monkeypatch.setattr(state.tmux, "capture_pane", _fake_capture(calls, delay=0))

    async def run():
        await state.shared_capture("s", 0.5)
        await state.shared_capture("s", 0.5)      # dentro da idade: reusa
        assert len(calls) == 1
        await asyncio.sleep(0.05)
        await state.shared_capture("s", 0.01)     # quem pede quadro mais novo captura
        assert len(calls) == 2

    asyncio.run(run())


def test_max_age_zero_does_not_join_older_inflight(monkeypatch):
    # Depois de um wake do plugin o pane de antes do evento não serve: nova captura.
    calls = []
    monkeypatch.setattr(state.tmux, "capture_pane", _fake_capture(calls, delay=0.05))

    async def run():
        primeira = asyncio.ensure_future(state.shared_capture("s", 0.5))
        await asyncio.sleep(0.01)
        await asyncio.gather(primeira, state.shared_capture("s", 0))

    asyncio.run(run())
    assert calls == ["s", "s"]


def test_old_frames_are_evicted_on_store(monkeypatch):
    monkeypatch.setattr(state.tmux, "capture_pane", _fake_capture([], delay=0))
    state._frames["morta"] = (time.monotonic() - state._FRAME_EVICT_AGE - 1, "x")
    asyncio.run(state.shared_capture("viva", 0))
    assert "morta" not in state._frames and "viva" in state._frames


def test_empty_frame_is_not_shared(monkeypatch):
    # Pane vazio leva o monitor ao has-session; reaproveitar "" esconderia a sessão voltando.
    calls = []
    monkeypatch.setattr(state.tmux, "capture_pane", _fake_capture(calls, text="", delay=0))

    async def run():
        await state.shared_capture("s", 0.5)
        await state.shared_capture("s", 0.5)

    asyncio.run(run())
    assert len(calls) == 2


def test_capture_runs_on_tmux_pool(monkeypatch):
    threads = []

    def capture(name):
        threads.append(threading.current_thread().name)
        return "x"

    monkeypatch.setattr(state.tmux, "capture_pane", capture)
    asyncio.run(state.shared_capture("s", 0))
    assert threads[0].startswith("hangar-tmux")

