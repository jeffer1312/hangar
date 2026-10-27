"""As portas Python autenticam e transportam as responsabilidades migradas."""
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient
from starlette.responses import JSONResponse

from app import account_bridge, api
from app.config import settings
from app.models import SessionInfo


@pytest.mark.parametrize("method,path,body", [
    ("POST", "/api/sessions/g-orq/input", {"text": "oi"}),
    ("POST", "/api/sessions/g-orq/interrupt", None),
    ("DELETE", "/api/sessions/g-orq", None),
])
def test_orchestration_writes_are_decided_by_rust(monkeypatch, method, path, body):
    monkeypatch.setattr(settings, "auth_token", "test-owner")
    monkeypatch.setattr(api, "_cached_info", AsyncMock(return_value=SessionInfo(name="g-orq", provider="orq")))
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
    monkeypatch.setattr(api, "_cached_info", AsyncMock(return_value=SessionInfo(name="g-orq", provider="orq")))
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "python")
    forward = AsyncMock(side_effect=AssertionError("sem Rust não há transporte nem implementação de reserva"))
    monkeypatch.setattr(account_bridge, "forward_public", forward)
    response = TestClient(api.app).delete("/api/sessions/g-orq", headers={"Authorization": "Bearer test-owner"})
    assert response.status_code == 503
    assert response.json() == {"detail": {"code": "providers_need_rust_server"}}
    forward.assert_not_called()
