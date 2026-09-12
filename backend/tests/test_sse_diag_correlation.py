import asyncio
from types import SimpleNamespace

import pytest
from fastapi import Request, Response

from app import diag, sse
from app.api import _correlaciona_diag
from app.difusor import Difusor


@pytest.mark.asyncio
@pytest.mark.parametrize("path,query,headers,expected", [
    ("/api/sessions/events", b"diag_req=lista-1", [], "lista-1"),
    ("/api/sessions/demo/events", b"diag_req=chat-1", [], "chat-1"),
    ("/api/sessions/demo/events", b"diag_req=chat-1", [(b"x-hangar-req", b"header-1")], "header-1"),
    ("/api/sessions/events", b"diag_req=" + b"x" * 33, [], ""),
    ("/api/sessions/events", b"diag_req=nao%20vale", [], ""),
    ("/api/sessions", b"diag_req=fora-do-sse", [], ""),
])
async def test_sse_req_aceito_so_na_rota_certa_e_contexto_restaurado(path, query, headers, expected):
    request = Request({"type": "http", "method": "GET", "scheme": "http", "path": path,
                       "headers": headers, "query_string": query, "server": ("localhost", 80)})
    previous = diag.req_atual.set("anterior")
    try:
        async def respond(_):
            assert diag.req_atual.get() == expected
            return Response(status_code=200)
        await _correlaciona_diag(request, respond)
        assert diag.req_atual.get() == "anterior"
    finally:
        diag.req_atual.reset(previous)


@pytest.mark.asyncio
async def test_produtores_compartilhados_nao_herdam_req_do_primeiro_ouvinte(monkeypatch):
    seen = []
    refresher = sse._ListRefresher()
    async def run():
        seen.append(diag.req_atual.get())
    monkeypatch.setattr(refresher, "_run", run)
    previous = diag.req_atual.set("primeira-conexao")
    try:
        refresher._ensure()
        await refresher._task
        async def source():
            seen.append(diag.req_atual.get())
            yield "quadro"
        stream = Difusor().ouvir("sessao", source)
        assert await anext(stream) == "quadro"
        await stream.aclose()
        assert seen == ["", ""]
        assert diag.req_atual.get() == "primeira-conexao"
    finally:
        diag.req_atual.reset(previous)


@pytest.mark.asyncio
async def test_lista_registra_abertura_e_fechamento_por_conexao(monkeypatch):
    records = []
    monkeypatch.setattr(diag, "registrar", lambda event, **fields: records.append((event, diag.req_atual.get())))
    monkeypatch.setattr(sse, "_list_refresher", SimpleNamespace(
        acquire=lambda: asyncio.Condition(), release=lambda: None,
        version=1, errored=False, data="[]"))
    previous = diag.req_atual.set("lista-1")
    try:
        stream = sse.list_events()
        assert (await anext(stream))["event"] == "sessions"
        await stream.aclose()
        assert records == [("sse.lista_abriu", "lista-1"), ("sse.lista_fechou", "lista-1")]
    finally:
        diag.req_atual.reset(previous)


@pytest.mark.asyncio
async def test_duas_conexoes_mantem_ids_distintos_ate_o_fechamento(monkeypatch):
    records = []
    monkeypatch.setattr(diag, "registrar", lambda event, **fields: records.append((event, diag.req_atual.get())))
    monkeypatch.setattr(sse, "_list_refresher", SimpleNamespace(
        acquire=lambda: asyncio.Condition(), release=lambda: None,
        version=1, errored=False, data="[]"))
    barrier = asyncio.Barrier(2)

    async def connect(req):
        request = Request({"type": "http", "method": "GET", "scheme": "http",
                           "path": "/api/sessions/events", "headers": [],
                           "query_string": f"diag_req={req}".encode(), "server": ("localhost", 80)})
        async def respond(_):
            stream = sse.list_events()
            await anext(stream)
            await barrier.wait()
            await stream.aclose()
            return Response(status_code=200)
        await _correlaciona_diag(request, respond)

    async with asyncio.timeout(2):
        await asyncio.gather(connect("desktop-1"), connect("mobile-1"))
    for req in ("desktop-1", "mobile-1"):
        assert [event for event, owner in records if owner == req] == ["sse.lista_abriu", "sse.lista_fechou"]
