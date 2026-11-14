"""O Rust é o único que grava `.hangar-pair`: o que chega às portas do Python segue ao Rust, num
sentido."""
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import api, pair, runtime_coordinator
from app.config import settings

H = {"Authorization": "Bearer secret"}


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "arquivo")
    return tmp_path


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "secret")
    return TestClient(api.app)


def test_associate_in_rust_mode_forwards_without_rebuilding_orq_facts(monkeypatch, client):
    from app import account_bridge
    from fastapi.responses import JSONResponse
    calls = []
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))

    async def forward(request, destination, **options):
        calls.append((destination, request.method, request.url.path, await request.json(), options))
        return JSONResponse({"ok": True, "gid": "g1", "grouped": True})
    monkeypatch.setattr(account_bridge, "forward_public", forward)
    r = client.post("/api/sessions/arb/orq/grupo", headers=H, json={"gid": "g1", "mtime": 1.5})
    assert r.status_code == 200, r.text
    assert r.json() == {"ok": True, "gid": "g1", "grouped": True}
    assert calls == [("/__hangar_server/forwarded", "POST", "/api/sessions/arb/orq/grupo", {"gid": "g1", "mtime": 1.5}, {"limit": None})]


def test_child_gets_group_env(monkeypatch, tmp_path):
    from app import rust_server
    monkeypatch.delenv("HANGAR_SERVER_ID", raising=False)
    monkeypatch.delenv("HANGAR_PEERS_FILE", raising=False)
    monkeypatch.setattr(settings, "server_id", "casa")
    sup = rust_server.Supervisor(tmp_path / "bin", "127.0.0.1", 1, 2, "t", "127.0.0.1", lambda: False)
    try:
        env = sup._env()
    finally:
        from app import internal_api
        internal_api.set_secret(None)
    assert "HANGAR_SERVER_ID" not in env
    assert "HANGAR_PEERS_FILE" not in env
    assert env["HANGAR_PAIR_ARCHIVE"] == str(pair._arquivo_dir())
