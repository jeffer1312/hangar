# backend/tests/test_internal_side_events.py
"""Conexão interna do hangar-server: as fontes compartilhadas de uma sessão, sem o transcript."""
import asyncio
import json

import pytest
from fastapi.testclient import TestClient

from app import internal_api, plugin_bridge, pqueue, sse
from app.adapters.preview_push import PushPreviewSource
from app.models import ChatEvent, SessionInfo, StateEvent, session_key

_TEXTO = "resposta já gravada no arquivo"
_SECRET = "cd" * 32


class _Adapter:
    provider = "claude"

    def __init__(self):
        self.drains = []

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
        self.drains.append((name, path))
        return 0


@pytest.fixture
def fila(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    return tmp_path


async def _coleta(gen, ate, limite=5.0):
    vistos = []
    try:
        async with asyncio.timeout(limite):
            async for ev in gen:
                vistos.append(ev)
                if ate(vistos):
                    return vistos
    finally:
        await gen.aclose()
    return vistos


async def test_side_comeca_por_info_nao_manda_transcript_e_ainda_drena(tmp_path, fila, monkeypatch):
    adapter = _Adapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    jsonl = tmp_path / "abc123.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-a", str(jsonl), side=True)
    vistos = await _coleta(gen, lambda v: any(e["event"] == "state" for e in v))
    assert vistos[0]["event"] == "info"
    info = json.loads(vistos[0]["data"])
    assert info["provider"] == "claude"
    assert info["jsonl"] == str(jsonl)
    assert info["session_key"] == session_key(str(jsonl))
    assert not any(e["event"] == "message" for e in vistos)
    await asyncio.sleep(0.05)   # o drain é fire-and-forget
    assert adapter.drains == [("lado-a", str(jsonl))]


async def test_side_ainda_suprime_previa_ja_gravada(tmp_path, fila, monkeypatch):
    # Sem o tail_pump no modo lateral, a prévia ficaria com o texto que já está no arquivo.
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    nome = "lado-previa"
    await PushPreviewSource.get(nome).push(_TEXTO)
    jsonl = tmp_path / "p.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events(nome, str(jsonl), provider="codex", side=True)
    vistos = await _coleta(gen, lambda v: any(
        e["event"] == "preview" and json.loads(e["data"])["text"] == "" for e in v))
    assert any(e["event"] == "preview" and json.loads(e["data"])["text"] == "" for e in vistos)


async def test_side_troca_de_provider_emite_info_e_nunca_reset(tmp_path, fila, monkeypatch):
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    velho = tmp_path / "velho.jsonl"
    velho.write_text("")
    novo = tmp_path / "rollout-2026-10-02T10-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000.jsonl"
    novo.write_text("")

    async def lista():
        return [SessionInfo(name="lado-troca", jsonl=str(novo), provider="codex")]

    monkeypatch.setattr(sse, "_cached_list", lista)
    gen = sse.merged_events("lado-troca", str(velho), side=True)
    vistos = await _coleta(gen, lambda v: sum(e["event"] == "info" for e in v) >= 2)
    infos = [json.loads(e["data"]) for e in vistos if e["event"] == "info"]
    assert infos[1]["provider"] == "codex"
    assert infos[1]["jsonl"] == str(novo)
    assert not any(e["event"] == "reset" for e in vistos)


async def test_side_repassa_a_fila(tmp_path, fila, monkeypatch):
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    entry = pqueue.PromptQueue("lado-fila").append("manda isso depois", delivered=False)
    jsonl = tmp_path / "f.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-fila", str(jsonl), side=True)
    vistos = await _coleta(gen, lambda v: any(e["event"] == "message" for e in v))
    msg = next(json.loads(e["data"]) for e in vistos if e["event"] == "message")
    assert msg["id"] == f"queued-{entry['id']}"


async def test_rota_side_events_conta_app_so_com_app_1_e_recusa_sessao_inexistente(
        tmp_path, fila, monkeypatch):
    from fastapi import HTTPException
    from app import api
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    jsonl = tmp_path / "r.jsonl"
    jsonl.write_text("")
    monkeypatch.setattr(api.registry, "list",
                        lambda: [SessionInfo(name="lado-rota", jsonl=str(jsonl), provider="claude")])
    with pytest.raises(HTTPException) as exc:
        await internal_api.side_events("outra", app=1)
    assert exc.value.status_code == 404

    antes = plugin_bridge._apps_abertos
    gen = (await internal_api.side_events("lado-rota", app=1)).body_iterator
    assert (await anext(gen))["event"] == "info"
    assert plugin_bridge._apps_abertos == antes + 1
    await gen.aclose()
    assert plugin_bridge._apps_abertos == antes

    gen0 = (await internal_api.side_events("lado-rota", app=0)).body_iterator
    assert (await anext(gen0))["event"] == "info"
    assert plugin_bridge._apps_abertos == antes
    await gen0.aclose()


def test_rota_side_events_so_atende_conexao_interna_com_segredo(tmp_path, monkeypatch):
    from app import api
    jsonl = tmp_path / "r.jsonl"
    monkeypatch.setattr(api.registry, "list",
                        lambda: [SessionInfo(name="s1", jsonl=str(jsonl), provider="claude")])

    async def finito(name, jsonl, **kw):
        yield {"event": "info", "data": "{}"}

    monkeypatch.setattr(internal_api, "merged_events", finito)
    rota = "/internal/sessions/s1/side-events"
    mudo = '{"detail":"Not Found"}'
    internal_api.set_secret(_SECRET)
    try:
        loopback = TestClient(api.app, client=("127.0.0.1", 50000))
        ok = loopback.get(rota, headers={"X-Hangar-Internal": _SECRET})
        assert ok.status_code == 200
        assert "event: info" in ok.text
        # Recusa é sempre o 404 mudo: sem cabeçalho, cabeçalho errado ou fora do loopback.
        for client, headers in (
            (loopback, {}),
            (loopback, {"X-Hangar-Internal": "x"}),
            (TestClient(api.app, client=("203.0.113.9", 50000)), {"X-Hangar-Internal": _SECRET}),
        ):
            r = client.get(rota, headers=headers)
            assert (r.status_code, r.text) == (404, mudo)
        # Sem segredo definido no processo, nem o cabeçalho certo passa.
        internal_api.set_secret(None)
        assert loopback.get(rota, headers={"X-Hangar-Internal": _SECRET}).status_code == 404
    finally:
        internal_api.set_secret(None)


