# backend/tests/test_sse_stats_band.py
"""Faixa de estatísticas do `/events` servido pelo próprio Python (modo python, convidado com login)."""
import asyncio
import json

import pytest

from app import pqueue, sse
from app.models import ChatEvent, StateEvent

_TEXTO = "resposta já gravada no arquivo"


class _Adapter:
    provider = "claude"

    async def _transcript(self, path, start_offset=None):
        yield ChatEvent(kind="assistant_msg", id="a1", text=_TEXTO)
        await asyncio.Event().wait()

    async def _estados(self):
        yield StateEvent(session="s", state="idle")
        await asyncio.Event().wait()

    def transcript_stream(self, path, start_offset=None):
        return self._transcript(path, start_offset)

    def state_monitor(self, name, sid_get, **kw):
        return self._estados()

    async def drain(self, name, path):
        return 0


@pytest.fixture
def fila(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    return tmp_path


async def _eventos_por(gen, segundos):
    vistos = []
    try:
        async with asyncio.timeout(segundos):
            async for ev in gen:
                vistos.append(ev["event"])
    except TimeoutError:
        pass
    finally:
        await gen.aclose()
    return vistos


async def test_faixa_stats_sai_no_events_do_python(tmp_path, fila, monkeypatch):
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    jsonl = tmp_path / "faixa.jsonl"
    linhas = [{"type": "user", "timestamp": "2026-10-23T12:00:00Z", "message": {"content": "oi"}},
              {"type": "assistant", "timestamp": "2026-10-23T12:00:02Z",
               "message": {"id": "m1", "usage": {"input_tokens": 1, "output_tokens": 5}}}]
    jsonl.write_text("".join(json.dumps(linha) + "\n" for linha in linhas))
    vistos = await _eventos_por(sse.merged_events("faixa", str(jsonl)), 1.5)
    assert "stats" in vistos, vistos
