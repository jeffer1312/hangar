"""A borda HTTP da escolha de modelo."""
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings
from app.registry import SessionInfo

TOKEN = "t-modelo"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _auth(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)


def test_model_invalido_devolve_400():
    r = TestClient(app).post("/api/sessions", headers=AUTH, json={
        "name": "x", "cwd": "/tmp", "model": "k3; touch /tmp/x"})
    assert r.status_code == 400


def test_effort_fora_da_lista_devolve_400():
    r = TestClient(app).post("/api/sessions", headers=AUTH, json={
        "name": "x", "cwd": "/tmp", "provider": "claude", "effort": "turbo"})
    assert r.status_code == 400


def test_codex_sem_modelo_nao_e_barrado_pela_validacao():
    """Regressão: a validação nova não pode transformar criação de Codex em 400. O create_codex
    vai mocado (convenção de tests/test_api.py:790) — sem isso o teste sobe um `codex --remote`
    de verdade na máquina, e da 2ª rodada em diante passa por 409, sem exercitar nada."""
    fake = AsyncMock(return_value=SessionInfo(name="cx-modelo", cwd="/tmp", provider="codex"))
    with patch("app.api.registry.create_codex", fake):
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "cx-modelo", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 200
    fake.assert_awaited_once()


def test_kimi_sem_modelo_nao_e_barrado_pela_validacao():
    """Irmão do Codex: o Kimi também é provider fora de escopo, e o caminho dele é o
    registry.create (não o create_codex) — sem a guarda de mock, o teste subiria um pane tmux
    de verdade (o mesmo defeito que o teste do Codex tinha)."""
    with patch("app.api.registry.create",
               return_value=SessionInfo(name="km", cwd="/tmp", provider="kimi")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "km", "cwd": "/tmp", "provider": "kimi"})
    assert r.status_code == 200
    cr.assert_called_once_with("km", "/tmp", None, provider="kimi", engine=None,
                               model=None, effort=None, context_window=None)
