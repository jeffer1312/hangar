import json
from unittest.mock import patch

from fastapi.testclient import TestClient

from app.config import settings
from app.models import SessionInfo


def _client():
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app)


def test_runners_route_returns_detected(tmp_path, monkeypatch):
    (tmp_path / "package.json").write_text(json.dumps({"scripts": {"dev": "vite"}}), encoding="utf-8")
    monkeypatch.setattr(settings, "projects_dir", str(tmp_path / "projects"))
    client = _client()
    with patch("app.api.registry.list",
               return_value=[SessionInfo(name="cc", cwd=str(tmp_path))]), \
         patch("app.runner.run_status", return_value=None):
        r = client.get("/api/sessions/cc/runners", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    data = r.json()
    assert any(d["command"] == "npm run dev" for d in data["detected"])
    assert data["running"] is None


def test_run_route_requires_auth():
    client = _client()
    assert client.post("/api/sessions/cc/run", json={"command": "x"}).status_code == 401
