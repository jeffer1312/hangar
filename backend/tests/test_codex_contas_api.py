"""Rotas autenticadas de contas Codex."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app import codex_contas_api
from app import codex_contas as accounts
from app.auth import reset_backoff
from app.config import settings


TOKEN = "token-codex-contas"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture
def painel(monkeypatch):
    service = SimpleNamespace(
        accounts_snapshot=AsyncMock(return_value=[]),
        create_account=AsyncMock(return_value={"id": "work"}),
        prepare=AsyncMock(return_value={"status": "running"}),
        preparation_status=Mock(return_value={"status": "ready"}),
        start_login=AsyncMock(return_value={"attempt_id": "a1", "status": "waiting"}),
        login_status=Mock(return_value=None),
        cancel_login=AsyncMock(return_value={"attempt_id": "a1", "status": "cancelled"}),
        delete_account=AsyncMock(return_value=None),
    )
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(codex_contas_api.accounts, "resolve_account",
                        lambda account_id: accounts.Account(account_id, f"/tmp/.codex-{account_id}", account_id == "default"))
    app = FastAPI()
    app.state.codex_contas_login = service
    app.include_router(codex_contas_api.codex_contas_router)
    reset_backoff()
    with TestClient(app) as client:
        yield client, service
    reset_backoff()


def test_rotas_exigem_auth(painel):
    client, service = painel

    assert client.get("/api/codex-contas").status_code == 401
    assert client.post("/api/codex-contas", json={"name": "work"}).status_code == 401
    service.accounts_snapshot.assert_not_awaited()


def test_delete_apaga_pelo_servico_e_traduz_recusa(painel):
    client, service = painel

    assert client.delete("/api/codex-contas/work").status_code == 401
    assert client.delete("/api/codex-contas/work", headers=AUTH).json() == {"ok": True}
    service.delete_account.assert_awaited_once()
    assert service.delete_account.await_args.args[0].id == "work"

    service.delete_account.side_effect = accounts.AccountError(
        409, "codex_account_default_protected", {"account_id": "default"})
    resp = client.delete("/api/codex-contas/default", headers=AUTH)
    assert resp.status_code == 409
    assert resp.json()["detail"]["code"] == "codex_account_default_protected"


def test_get_nao_cria_nem_prepara(painel):
    client, service = painel

    assert client.get("/api/codex-contas", headers=AUTH).status_code == 200

    service.accounts_snapshot.assert_awaited_once()
    service.create_account.assert_not_awaited()
    service.prepare.assert_not_awaited()


def test_post_prepare_e_login_usam_id_da_rota(painel):
    client, service = painel

    assert client.post("/api/codex-contas/work/prepare", headers=AUTH).status_code == 202
    assert client.get("/api/codex-contas/work/prepare", headers=AUTH).status_code == 200
    assert client.post("/api/codex-contas/work/login", headers=AUTH).status_code == 200
    assert client.get("/api/codex-contas/work/login", headers=AUTH).status_code == 200
    assert client.delete("/api/codex-contas/work/login?attempt_id=a1", headers=AUTH).status_code == 200
    assert service.prepare.await_args.args[0].id == "work"
    assert service.preparation_status.call_args.args[0].id == "work"
    assert service.start_login.await_args.args[0].id == "work"
    assert service.login_status.call_args.args[0].id == "work"
    assert service.cancel_login.await_args.args[0].id == "work"


def test_post_prepare_manual_forca_a_cadeia_completa(painel):
    client, service = painel

    assert client.post("/api/codex-contas/work/prepare?forcar=true", headers=AUTH).status_code == 202
    service.prepare.assert_awaited_once()
    assert service.prepare.await_args.kwargs == {"forcar": True}


def test_nome_invalido_e_conta_ausente_falham(painel):
    client, service = painel
    async def create(name):
        if name == "UPPER":
            raise accounts.AccountError(400, "codex_account_invalid_name")
        raise accounts.AccountError(404, "codex_account_not_found")
    service.create_account.side_effect = create

    assert client.post("/api/codex-contas", json={"name": "UPPER"}, headers=AUTH).status_code == 400
    assert client.post("/api/codex-contas", json={"name": "work"}, headers=AUTH).status_code == 404


def test_detector_da_api_nao_confunde_sidecar_com_pane_de_outra_cli(monkeypatch, tmp_path):
    from app import api

    account = accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(api.registry, "list", lambda: [SimpleNamespace(name="same", provider="codex")])
    monkeypatch.setattr(api, "codex_session_alive", lambda name, meta: False)

    assert api._codex_account_in_use(account) is False


def test_post_prepare_pendente_devolve_sync_completo(monkeypatch, tmp_path):
    from app.codex_contas_login import CodexContasLogin

    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    account = accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(accounts, "resolve_account", lambda account_id: account)
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    release = asyncio.Event()
    started = asyncio.Event()

    async def prepare(account):
        started.set()
        await release.wait()
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", prepare)
    service = CodexContasLogin(account_in_use=lambda account: False)
    app = FastAPI()
    app.state.codex_contas_login = service
    app.include_router(codex_contas_api.codex_contas_router)
    reset_backoff()
    with TestClient(app) as client:
        try:
            response = client.post("/api/codex-contas/work/prepare", headers=AUTH)
            client.portal.call(asyncio.wait_for, started.wait(), 1)
            assert not release.is_set()
            assert response.status_code == 202
            assert response.json() == {
                "status": "running", "trust_pending": False, "issues": [], "etapa": None,
            }
        finally:
            client.portal.call(release.set)
            client.portal.call(service.close)
            reset_backoff()
