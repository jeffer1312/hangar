import pytest
import asyncio
import json
from app.sse import merged_events
from app.models import ChatEvent


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
    def __init__(self, name):
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
