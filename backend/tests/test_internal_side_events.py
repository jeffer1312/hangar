# backend/tests/test_internal_side_events.py
"""Conexão interna do hangar-server: as fontes compartilhadas de uma sessão, sem o transcript."""
import asyncio
import json

import pytest
from fastapi.testclient import TestClient

from app import internal_api, plugin_bridge, pqueue, sse
from app.adapters.preview_push import PushPreviewSource
from app.models import ChatEvent, SessionInfo, StateEvent

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


async def test_side_nao_manda_transcript_nem_info_e_ainda_drena(tmp_path, fila, monkeypatch):
    adapter = _Adapter()
    monkeypatch.setattr(sse, "get_adapter", lambda provider: adapter)
    jsonl = tmp_path / "abc123.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-a", str(jsonl), side=True)
    vistos = await _coleta(gen, lambda v: any(e["event"] == "state" for e in v))
    assert not any(e["event"] in ("message", "info") for e in vistos)
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


async def test_side_nao_procura_o_transcript_sozinho(tmp_path, fila, monkeypatch):
    # Quem escolhe e troca a conversa é o Rust, religando esta conexão: aqui ninguém relê a lista.
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    velho = tmp_path / "velho.jsonl"
    velho.write_text("")
    lidas = []

    async def lista():
        lidas.append(1)
        return [SessionInfo(name="lado-troca", jsonl=str(tmp_path / "novo.jsonl"), provider="codex")]

    monkeypatch.setattr(sse, "_cached_list", lista)
    gen = sse.merged_events("lado-troca", str(velho), side=True)
    vistos = []
    try:
        # O vigia relê a lista a cada 2 s: a janela cobre duas rodadas dele.
        async with asyncio.timeout(4.5):
            async for ev in gen:
                vistos.append(ev)
    except TimeoutError:
        pass
    finally:
        await gen.aclose()
    assert lidas == []
    assert not any(e["event"] in ("reset", "info") for e in vistos)


async def test_side_nao_repassa_a_fila(tmp_path, fila, monkeypatch):
    # A fila do chat é do hub do Rust, que lê a projeção: a conexão interna não a manda.
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    pqueue.PromptQueue("lado-fila").append("manda isso depois", delivered=False)
    jsonl = tmp_path / "f.jsonl"
    jsonl.write_text("")
    gen = sse.merged_events("lado-fila", str(jsonl), side=True)
    vistos = []
    try:
        async with asyncio.timeout(1.5):
            async for ev in gen:
                vistos.append(ev)
    except TimeoutError:
        pass
    finally:
        await gen.aclose()
    assert any(e["event"] == "state" for e in vistos), "o stream abriu"
    assert not any(e["event"] in ("message", "queue_confirmed") for e in vistos)


async def test_rota_side_events_conta_app_so_com_app_1_e_recusa_conversa_invalida(
        tmp_path, fila, monkeypatch):
    from fastapi import HTTPException
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    jsonl = str(tmp_path / "r.jsonl")
    (tmp_path / "r.jsonl").write_text("")
    for provider, path in (("pi", jsonl), ("claude", ""), ("claude", "relativo.jsonl")):
        with pytest.raises(HTTPException) as exc:
            await internal_api.side_events("lado-rota", provider=provider, jsonl=path, app=1)
        assert exc.value.status_code == 400

    antes = plugin_bridge._apps_abertos
    gen = (await internal_api.side_events("lado-rota", provider="claude", jsonl=jsonl, app=1)).body_iterator
    await anext(gen)
    assert plugin_bridge._apps_abertos == antes + 1
    await gen.aclose()
    assert plugin_bridge._apps_abertos == antes

    gen0 = (await internal_api.side_events("lado-rota", provider="claude", jsonl=jsonl, app=0)).body_iterator
    await anext(gen0)
    assert plugin_bridge._apps_abertos == antes
    await gen0.aclose()


def test_rota_side_events_so_atende_conexao_interna_com_segredo(tmp_path, monkeypatch):
    from app import api

    async def finito(name, jsonl, **kw):
        yield {"event": "ping", "data": "{}"}

    monkeypatch.setattr(internal_api, "merged_events", finito)
    rota = f"/internal/sessions/s1/side-events?provider=claude&jsonl={tmp_path / 'r.jsonl'}"
    mudo = '{"detail":"Not Found"}'
    internal_api.set_secret(_SECRET)
    try:
        loopback = TestClient(api.app, client=("127.0.0.1", 50000))
        ok = loopback.get(rota, headers={"X-Hangar-Internal": _SECRET})
        assert ok.status_code == 200
        assert "event: ping" in ok.text
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


@pytest.mark.parametrize("side", [True, False])
async def test_faixa_stats_e_do_rust_na_conexao_interna(tmp_path, fila, monkeypatch, side):
    # ST-R1: com o Rust de pé a faixa sai do hub dele; o `/events` do próprio Python (Connect, modo python) segue com ela.
    monkeypatch.setattr(sse, "get_adapter", lambda provider: _Adapter())
    jsonl = tmp_path / "faixa.jsonl"
    linhas = [{"type": "user", "timestamp": "2026-10-23T12:00:00Z", "message": {"content": "oi"}},
              {"type": "assistant", "timestamp": "2026-10-23T12:00:02Z",
               "message": {"id": "m1", "usage": {"input_tokens": 1, "output_tokens": 5}}}]
    jsonl.write_text("".join(json.dumps(linha) + "\n" for linha in linhas))
    vistos = await _eventos_por(sse.merged_events(f"faixa-{side}", str(jsonl), side=side), 1.5)
    assert ("stats" in vistos) is not side, vistos
