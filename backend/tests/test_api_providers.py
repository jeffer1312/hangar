from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings


@pytest.fixture
def api_client(monkeypatch):
    settings.auth_token = "secret"
    import app.api as api_mod

    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    from app.api import app

    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}


def test_get_providers_retorna_4_providers(api_client, monkeypatch):
    fake = {
        "claude": {"disponivel": True, "motivo": None},
        "codex": {"disponivel": True, "motivo": None},
        "pi": {"disponivel": False, "motivo": "nao_encontrado"},
        "kimi": {"disponivel": False, "motivo": "sem_permissao"},
    }
    with patch("app.cli_probe.sondar_providers", return_value=fake) as mock:
        r = api_client.get("/api/providers", headers=_h())
    assert r.status_code == 200
    body = r.json()
    # os 4 providers presentes
    assert set(body.keys()) == {"claude", "codex", "pi", "kimi"}
    assert body["claude"]["disponivel"] is True
    assert body["pi"]["disponivel"] is False
    assert body["kimi"]["motivo"] == "sem_permissao"
    mock.assert_called_once()


def test_get_providers_requer_auth(api_client):
    r = api_client.get("/api/providers")
    assert r.status_code == 401
