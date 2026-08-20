"""Modo de permissão na criação — borda HTTP."""
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings
from app.models import SessionInfo

TOKEN = "t-permissao"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _auth(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)


def _client():
    from app.api import app

    return TestClient(app)


def test_create_com_permissao_claude_ok():
    with patch("app.api.registry.create", return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "claude", "permission_mode": "plan"})
    assert r.status_code == 200
    assert cr.call_args.kwargs.get("permission_mode") == "plan"


def test_create_com_permissao_kimi_409():
    with patch("app.api.registry.create") as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "kimi", "permission_mode": "plan"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_so_claude"
    cr.assert_not_called()


def test_create_com_permissao_invalida_409():
    with patch("app.api.registry.create") as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "claude", "permission_mode": "invalido"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_invalida"
    cr.assert_not_called()


def test_create_sem_permissao_recebe_none():
    with patch("app.api.registry.create", return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={"name": "x", "cwd": "/tmp", "provider": "claude"})
    assert r.status_code == 200
    assert cr.call_args.kwargs.get("permission_mode") is None
