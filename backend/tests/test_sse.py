import pytest
import asyncio
import json
from app.sse import merged_events
from app.models import ChatEvent, StateEvent


class _StubModel:
    def model_dump(self):
        return {}


async def _empty_agen():
    return
    yield  # make it an async generator


async def _raising_agen():
    raise FileNotFoundError("simulated missing dir")
    yield  # make it an async generator


class _StubTailerRaises:
    def __init__(self, path):
        pass

    def follow(self):
        return _raising_agen()


class _StubMonitorIdle:
    def __init__(self, name, **kw):
        pass

    def stream(self):
        return _empty_agen()


@pytest.mark.asyncio
async def test_pump_error_propagates(monkeypatch):
    """If a pump raises, merged_events must re-raise instead of hanging."""
    monkeypatch.setattr("app.sse.TranscriptTailer", _StubTailerRaises)
    monkeypatch.setattr("app.sse.StateMonitor", _StubMonitorIdle)

    with pytest.raises(FileNotFoundError):
        async for _ in merged_events("x", "y"):
            pass


async def _one_chat_event():
    yield ChatEvent(kind="user_msg", id="1", text="hi")  # tool_name etc. stay None


class _StubTailerOne:
    def __init__(self, path):
        pass

    def follow(self):
        return _one_chat_event()


@pytest.mark.asyncio
async def test_sse_data_is_json_string(monkeypatch):
    """SSE `data` must be a JSON string (browser does JSON.parse(e.data)); a raw dict
    gets str()'d into Python repr (None / single quotes) = invalid JSON."""
    monkeypatch.setattr("app.sse.TranscriptTailer", _StubTailerOne)
    monkeypatch.setattr("app.sse.StateMonitor", _StubMonitorIdle)

    async for ev in merged_events("cc", "j"):
        assert ev["event"] == "message"
        assert isinstance(ev["data"], str)
        parsed = json.loads(ev["data"])  # must not raise
        assert parsed["kind"] == "user_msg"
        assert parsed["tool_name"] is None      # serialized as JSON null
        assert "null" in ev["data"] and "None" not in ev["data"]
        break


async def _seq_states():
    # overlay aberto (nao-entregavel) -> idle (entregavel): a transicao dispara o drain UMA vez.
    yield StateEvent(session="cc", state="awaiting_input", overlay=True)
    yield StateEvent(session="cc", state="idle", overlay=False)
    yield StateEvent(session="cc", state="idle", overlay=False)   # repetido NAO redispara


class _StubMonitorSeq:
    def __init__(self, name, **kw):
        pass

    def stream(self):
        return _seq_states()


@pytest.mark.asyncio
async def test_drain_fires_once_on_overlay_to_idle(monkeypatch):
    calls = []
    monkeypatch.setattr("app.sse.TranscriptTailer", _StubTailerOne)
    monkeypatch.setattr("app.sse.StateMonitor", _StubMonitorSeq)
    monkeypatch.setattr("app.sse.drain", lambda name, jsonl: calls.append((name, jsonl)) or 0)
    seen_idle = 0
    async for ev in merged_events("cc", "j"):
        if ev["event"] == "state" and json.loads(ev["data"])["state"] == "idle":
            seen_idle += 1
            if seen_idle >= 2:
                await asyncio.sleep(0.05)   # deixa o to_thread(drain) rodar
                break
    assert calls == [("cc", "j")]            # exatamente 1 drain, no jsonl corrente
