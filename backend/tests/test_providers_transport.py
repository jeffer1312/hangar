"""As portas Python autenticam e transportam as responsabilidades migradas."""
import asyncio
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient
from starlette.responses import JSONResponse
from starlette.requests import Request

from app import account_bridge, api
from app.config import settings


@pytest.mark.parametrize("method,path,body", [
    ("POST", "/api/sessions/g-orq/input", {"text": "oi"}),
    ("POST", "/api/sessions/g-orq/interrupt", None),
    ("DELETE", "/api/sessions/g-orq", None),
])
def test_orchestration_writes_are_decided_by_rust(monkeypatch, method, path, body):
    monkeypatch.setattr(settings, "auth_token", "test-owner")
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "rust")
    received = []

    async def forward(request, target, **kwargs):
        received.append((request.method, request.url.path, await request.body(), target))
        return JSONResponse({"detail": {"code": "erro_sessao_orq"}}, status_code=409)

    monkeypatch.setattr(account_bridge, "forward_public", forward)
    response = TestClient(api.app).request(method, path, json=body, headers={"Authorization": "Bearer test-owner"})
    assert response.status_code == 409
    assert response.json() == {"detail": {"code": "erro_sessao_orq"}}
    assert received == [(method, path, b'{"text":"oi"}' if body else b"", "/__hangar_server/forwarded")]


def test_orchestration_authentication_precedes_session_lookup(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "test-owner")
    lookup = AsyncMock(side_effect=AssertionError("a sessão não deve ser consultada sem autenticação"))
    monkeypatch.setattr(api, "_cached_info", lookup)
    for method, path in [("POST", "/api/sessions/g-orq/input"), ("GET", "/api/sessions/g-orq/history")]:
        assert TestClient(api.app).request(method, path).status_code == 401
    lookup.assert_not_called()


def test_orchestration_write_without_rust_is_visible(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "test-owner")
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "python")
    forward = AsyncMock(side_effect=AssertionError("sem Rust não há transporte nem implementação de reserva"))
    monkeypatch.setattr(account_bridge, "forward_public", forward)
    response = TestClient(api.app).delete("/api/sessions/g-orq", headers={"Authorization": "Bearer test-owner"})
    assert response.status_code == 503
    assert response.json() == {"detail": {"code": "providers_need_rust_server"}}
    forward.assert_not_called()


def test_unclaimed_orq_suffix_continues_the_normal_session_route(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "test-owner")
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "rust")
    monkeypatch.setattr(account_bridge, "forward_public", AsyncMock(return_value=JSONResponse({}, status_code=421)))
    monkeypatch.setattr(api, "_session_exists", lambda name: False)
    response = TestClient(api.app).post("/api/sessions/ordinary-orq/input", json={"text": "oi"}, headers={"Authorization": "Bearer test-owner"})
    assert response.status_code == 404
    assert response.json()["detail"]["code"] == "erro_sessao_recado_nao_enfileirado"


def test_unclaimed_stream_closes_before_the_python_route_continues(monkeypatch):
    async def scenario():
        closed = asyncio.Event()

        async def upstream(reader, writer):
            try:
                await reader.readuntil(b"\r\n\r\n")
                writer.write(b"HTTP/1.1 421 Misdirected Request\r\nContent-Length: 0\r\n\r\n")
                await writer.drain()
                await reader.read()
                closed.set()
            finally:
                writer.close()

        server = await asyncio.start_server(upstream, "127.0.0.1", 0)
        async with server:
            port = server.sockets[0].getsockname()[1]
            monkeypatch.setattr(account_bridge, "_preparation_transport", (f"127.0.0.1:{port}", "synthetic"))
            request = Request({"type": "http", "method": "GET", "path": "/api/sessions/ordinary-orq/history", "query_string": b"", "headers": []})
            response = await asyncio.wait_for(account_bridge.forward_stream(request, guest=False), 3)
            assert response.status_code == 421
            waiting = asyncio.create_task(closed.wait())
            done, _ = await asyncio.wait({waiting}, timeout=0.5)
            waiting.cancel()
            server.close_clients()
            assert done, "a conexão deve fechar mesmo sem consumir um corpo de SSE"

    asyncio.run(scenario())
