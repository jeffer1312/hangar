import base64
import importlib

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client(tmp_path, monkeypatch):
    # Point the vault at a temp file and enable sync BEFORE importing the app, so the router mounts.
    monkeypatch.setenv("CP_SYNC", "1")
    monkeypatch.setenv("CP_SYNC_BOOTSTRAP", "boot-secret")
    monkeypatch.setenv("CP_SYNC_DATA", str(tmp_path / "vault.json"))
    monkeypatch.setenv("CP_SYNC_SESSION_SECRET", "test-session-secret")
    import app.config as config
    importlib.reload(config)
    import app.sync as sync
    importlib.reload(sync)
    import app.api as api
    importlib.reload(api)
    return TestClient(api.app)


# A fake client-derived pair. The server never derives these; it only stores/compares.
SALT = base64.b64encode(b"0123456789abcdef").decode()
AUTH = base64.b64encode(b"auth-hash-32-bytes-padding-here!").decode()


def test_status_unregistered(client):
    r = client.get("/api/sync/status")
    assert r.status_code == 200
    assert r.json() == {"enabled": True, "registered": False}


def test_register_requires_bootstrap(client):
    r = client.post("/api/sync/register",
                    json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "wrong"})
    assert r.status_code == 403


def test_register_once_then_locked(client):
    ok = client.post("/api/sync/register",
                     json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})
    assert ok.status_code == 200
    again = client.post("/api/sync/register",
                        json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})
    assert again.status_code == 403
    assert client.get("/api/sync/status").json()["registered"] is True
