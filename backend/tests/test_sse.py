import pytest
import asyncio
from app.sse import merged_events


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
