"""Troca de conta Claude: é do Rust. Aqui ficam a recusa sem ele e a leitura do Fast do motor."""
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.adapters.claude_headless import sessions as S
from app.config import ConfigDirInfo, settings

SID = "22222222-2222-2222-2222-222222222222"
_H = {"Authorization": "Bearer secret"}


@pytest.fixture
def contas(tmp_path, monkeypatch):
    import app.api as api_mod
    import app.archive as archive_mod
    a, b = tmp_path / ".claude-a", tmp_path / ".claude-b"
    for c in (a, b):
        (c / "projects").mkdir(parents=True)
    lista = [ConfigDirInfo(path=str(a), label="a", active=True), ConfigDirInfo(path=str(b), label="b", active=False)]
    monkeypatch.setattr(api_mod, "list_config_dirs", lambda ordered=True: lista)
    monkeypatch.setattr(archive_mod, "list_config_dirs", lambda ordered=True: lista)
    monkeypatch.setattr("app.cotas.cotas_claude", lambda: [])
    monkeypatch.setattr(settings, "projects_dir", a / "projects")
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(S, "_trocando", {})
    settings.auth_token = "secret"
    return str(a), str(b)


def test_claude_account_routes_need_rust(contas):
    import app.api as api_mod
    client = TestClient(api_mod.app)
    for r in (client.get("/api/sessions/hl/conta", headers=_H),
              client.post("/api/sessions/hl/conta", headers=_H, json={"config_dir": contas[1]})):
        assert r.status_code == 503 and r.json()["detail"]["code"] == "session_open_needs_rust_server"
