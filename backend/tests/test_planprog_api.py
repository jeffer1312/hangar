from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings
import app.api as api_mod
from app.models import SessionInfo

_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def api_client(monkeypatch):
    """Espelha tests/test_api.py:57 — mesma fixture real (client generico + monkeypatch de
    _session_exists), nao a `client` descartavel do topo do arquivo (so tem /ping)."""
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    from app.api import app
    return TestClient(app)


PLAN = "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: verificação manual**\n"


def test_plan_404_sem_plano(api_client):
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd="/tmp")]), \
         patch("app.api.plan_progress", return_value=None):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404


def test_plan_404_sem_sessao(api_client):
    with patch("app.api.registry.list", return_value=[]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 404


def test_plan_devolve_detalhe_e_markdown(api_client, tmp_path):
    d = tmp_path / "docs" / "superpowers" / "plans"
    d.mkdir(parents=True)
    (tmp_path / ".git").mkdir()
    (d / "2026-07-29-plano.md").write_text(PLAN, encoding="utf-8")
    with patch("app.api.registry.list", return_value=[SessionInfo(name="s", cwd=str(tmp_path))]):
        r = api_client.get("/api/sessions/s/plan", headers=_H)
    assert r.status_code == 200
    j = r.json()
    assert j["name"] == "plano"
    assert (j["done"], j["total"]) == (1, 2)
    assert j["complete"] is False
    assert j["tasks"][0]["steps"][0]["done"] is True
    assert j["tasks"][0]["steps"][1]["manual"] is True
    # markdown cru viaja na resposta: o GET /file so serve path citado no transcript (api.py:2196),
    # e um plano descoberto por glob nunca aparece la.
    assert "### Task 1" in j["markdown"]
