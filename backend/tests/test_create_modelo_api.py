"""A borda HTTP da escolha de modelo."""
import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings

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
    """Regressão: a validação nova não pode transformar criação de Codex em 400. Aceita qualquer
    status que NÃO seja 400 — o resto do caminho (app-server ausente no teste) é outro assunto."""
    r = TestClient(app).post("/api/sessions", headers=AUTH, json={
        "name": "x", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code != 400
