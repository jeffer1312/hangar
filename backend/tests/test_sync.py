import base64
import importlib

import pytest
from fastapi.testclient import TestClient


def _make_client(tmp_path, monkeypatch, bind_ip="127.0.0.1", **extra_env):
    monkeypatch.setenv("CP_SYNC", "1")
    monkeypatch.setenv("CP_SYNC_BOOTSTRAP", "boot-secret")
    monkeypatch.setenv("CP_SYNC_DATA", str(tmp_path / "vault.json"))
    monkeypatch.setenv("CP_SYNC_SESSION_SECRET", "test-session-secret")
    monkeypatch.setenv("CP_LAN_BIND_IP", bind_ip)
    for k, v in extra_env.items():
        monkeypatch.setenv(k, str(v))
    import app.config as config
    importlib.reload(config)
    import app.sync as sync
    importlib.reload(sync)
    import app.api as api
    importlib.reload(api)
    return TestClient(api.app)


@pytest.fixture
def client(tmp_path, monkeypatch):
    return _make_client(tmp_path, monkeypatch)


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


def _register(client):
    client.post("/api/sync/register",
                json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})


def test_prelogin_returns_salt(client):
    _register(client)
    r = client.get("/api/sync/prelogin", params={"user": "j"})
    assert r.status_code == 200
    assert r.json()["salt"] == SALT
    assert r.json()["iterations"] == 600000


def test_login_good_and_bad(client):
    _register(client)
    bad = client.post("/api/sync/login", json={"user": "j", "auth_hash": "deadbeef"})
    assert bad.status_code == 401
    ok = client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH})
    assert ok.status_code == 200
    assert "cp_sync" in ok.cookies


def test_vault_round_trip_and_stale_rev(client):
    _register(client)
    client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH})
    empty = client.get("/api/sync/vault")
    assert empty.status_code == 200
    assert empty.json() == {"enc_blob": None, "rev": 0}

    blob = {"iv": "aXY=", "data": "ZGF0YQ=="}
    put = client.put("/api/sync/vault", json={"enc_blob": blob, "base_rev": 0})
    assert put.status_code == 200
    assert put.json()["rev"] == 1

    got = client.get("/api/sync/vault")
    assert got.json() == {"enc_blob": blob, "rev": 1}

    stale = client.put("/api/sync/vault", json={"enc_blob": blob, "base_rev": 0})
    assert stale.status_code == 409
    assert stale.json()["detail"]["rev"] == 1


def test_vault_requires_session(client):
    _register(client)
    assert client.get("/api/sync/vault").status_code == 401


def test_login_cookie_secure_when_non_loopback(tmp_path, monkeypatch):
    c = _make_client(tmp_path, monkeypatch, "192.168.1.50")
    _register(c)
    resp = c.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH})
    assert resp.status_code == 200
    cookie = resp.headers.get("set-cookie", "")
    assert "Secure" in cookie
    assert "HttpOnly" in cookie
    assert "samesite=lax" in cookie.lower()


def test_login_rate_limited_at_configured_max(tmp_path, monkeypatch):
    # CP_SYNC_RATE_MAX is configurable; here 3 bad logins are allowed, the 4th trips 429.
    c = _make_client(tmp_path, monkeypatch, CP_SYNC_RATE_MAX=3)
    _register(c)
    for _ in range(3):
        assert c.post("/api/sync/login", json={"user": "j", "auth_hash": "wronghash"}).status_code == 401
    assert c.post("/api/sync/login", json={"user": "j", "auth_hash": "wronghash"}).status_code == 429


def test_session_slides_on_authed_request(client):
    # Sliding: cada request autenticado re-emite o cookie com prazo novo -> nao expira em uso.
    _register(client)
    assert "cp_sync" in client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH}).cookies
    r = client.get("/api/sync/vault")
    assert r.status_code == 200
    assert "cp_sync=" in r.headers.get("set-cookie", "")
