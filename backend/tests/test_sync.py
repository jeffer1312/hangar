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
    fresh = config.Settings()
    # Preserva o objeto compartilhado: reload deixava os outros módulos com configurações diferentes.
    for name in ("sync", "sync_bootstrap", "sync_data", "sync_session_secret", "sync_rate_max", "sync_rate_window", "lan_bind_ip", "auth_token"):
        monkeypatch.setattr(config.settings, name, getattr(fresh, name))
    from app import auth, runtime_config
    monkeypatch.setattr(auth, "settings", config.settings)
    monkeypatch.setattr(runtime_config, "settings", config.settings)
    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: tmp_path)
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


def _setup_payload():
    return {"user": "jefferson", "salt": SALT,
            "auth_hash": base64.b64encode(b"a" * 32).decode(),
            "enc_blob": {"iv": base64.b64encode(b"i" * 12).decode(),
                         "data": base64.b64encode(b"d" * 16).decode()}}


def _admin_client(tmp_path, monkeypatch):
    c = _make_client(tmp_path, monkeypatch, CP_SYNC=0, CP_AUTH_TOKEN="setup-token")
    c.headers["Authorization"] = "Bearer setup-token"
    return c


def test_setup_exige_token_normal_mesmo_desligado(tmp_path, monkeypatch):
    c = _admin_client(tmp_path, monkeypatch)
    c.headers.clear()
    assert c.get("/api/sync/setup").status_code == 401
    assert c.post("/api/sync/setup", json=_setup_payload()).status_code == 401
    assert c.post("/api/sync/setup/disable").status_code == 401
    assert not (tmp_path / "vault.json").exists()


def test_setup_ativa_imediatamente_e_persiste_conta_cifrada(tmp_path, monkeypatch):
    c = _admin_client(tmp_path, monkeypatch)
    assert c.get("/api/sync/status").status_code == 404
    assert c.get("/api/sync/setup").json() == {
        "enabled": False, "registered": False, "user": None}
    payload = _setup_payload()
    response = c.post("/api/sync/setup", json=payload)
    assert response.status_code == 200
    assert response.json() == {"enabled": True, "registered": True, "user": "jefferson"}
    assert "set-cookie" not in response.headers
    c.headers.clear()
    assert c.get("/api/sync/status").json() == {"enabled": True, "registered": True}
    assert c.post("/api/sync/login", json={key: payload[key] for key in ("user", "auth_hash")}).status_code == 200
    assert c.get("/api/sync/vault").json() == {"enc_blob": payload["enc_blob"], "rev": 1}
    restarted = _admin_client(tmp_path, monkeypatch)
    assert restarted.get("/api/sync/status").json() == {"enabled": True, "registered": True}


def test_setup_existente_so_reativa_sem_sobrescrever(tmp_path, monkeypatch):
    c = _admin_client(tmp_path, monkeypatch)
    assert c.post("/api/sync/setup", json=_setup_payload()).status_code == 200
    before = (tmp_path / "vault.json").read_bytes()
    payload = _setup_payload()
    assert c.post("/api/sync/login", json={key: payload[key] for key in ("user", "auth_hash")}).status_code == 200
    cookie = c.cookies.get("cp_sync")
    assert c.post("/api/sync/setup/disable").json() == {
        "enabled": False, "registered": True, "user": "jefferson"}
    assert c.get("/api/sync/vault").status_code == 404
    assert c.cookies.get("cp_sync") == cookie
    assert c.post("/api/sync/setup", json=_setup_payload()).status_code == 409
    assert c.get("/api/sync/setup").json()["enabled"] is False
    assert c.post("/api/sync/setup", json={}).status_code == 200
    assert (tmp_path / "vault.json").read_bytes() == before
    assert c.get("/api/sync/vault").json() == {"enc_blob": payload["enc_blob"], "rev": 1}


@pytest.mark.parametrize("bad", [None, {}, {"user": None}, {"user": " "}, {"user": "x" * 101},
                                  {"salt": "!" * 24}, {"auth_hash": SALT},
                                  {"enc_blob": {"iv": SALT, "data": AUTH}},
                                  {"enc_blob": {"iv": "a" * 16, "data": "!" * 24}}])
def test_setup_invalido_nao_cria_nem_ativa(tmp_path, monkeypatch, bad):
    c = _admin_client(tmp_path, monkeypatch)
    payload = _setup_payload() | bad if bad else bad
    response = c.post("/api/sync/setup", json=payload)
    assert response.status_code == 422
    assert not (tmp_path / "vault.json").exists()
    assert c.get("/api/sync/status").status_code == 404


@pytest.mark.parametrize("content", ["null", "[]", "{}", "{broken"])
def test_setup_cofre_invalido_falha_sem_sobrescrever(tmp_path, monkeypatch, content):
    c = _admin_client(tmp_path, monkeypatch)
    c = TestClient(c.app, raise_server_exceptions=False)
    c.headers["Authorization"] = "Bearer setup-token"
    path = tmp_path / "vault.json"
    path.write_text(content)
    assert c.get("/api/sync/setup").status_code == 500
    assert c.post("/api/sync/setup", json=_setup_payload()).status_code == 500
    assert path.read_text() == content
    assert c.get("/api/sync/status").status_code == 404


def test_setup_falha_na_ativacao_preserva_conta_para_repetir(tmp_path, monkeypatch):
    from app import runtime_config
    c = _admin_client(tmp_path, monkeypatch)
    original = runtime_config.aplicar

    def fail(*args, **kwargs):
        raise OSError("disco indisponível")

    monkeypatch.setattr(runtime_config, "aplicar", fail)
    with pytest.raises(OSError):
        c.post("/api/sync/setup", json=_setup_payload())
    before = (tmp_path / "vault.json").read_bytes()
    assert c.get("/api/sync/setup").json()["enabled"] is False
    monkeypatch.setattr(runtime_config, "aplicar", original)
    assert c.post("/api/sync/setup", json={}).status_code == 200
    assert (tmp_path / "vault.json").read_bytes() == before


def test_setup_sem_blob_cria_cofre_vazio(tmp_path, monkeypatch):
    from app import sync
    c = _admin_client(tmp_path, monkeypatch)
    payload = _setup_payload()
    del payload["enc_blob"]
    assert c.post("/api/sync/setup", json=payload).status_code == 200
    assert sync.load_vault()["enc_blob"] is None
    assert sync.load_vault()["rev"] == 0


BLOB = {"iv": base64.b64encode(b"123456789012").decode(),
        "data": base64.b64encode(b"x" * 32).decode()}
GUEST_AUTH = base64.b64encode(b"guest-hash-32-bytes-padding-her!").decode()


def _owner_logged(client):
    client.post("/api/sync/register",
                json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})
    assert client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH}).status_code == 200


def _add_guest(client, user="ana"):
    return client.post("/api/sync/guests", json={
        "user": user, "salt": SALT, "auth_hash": GUEST_AUTH, "enc_blob": BLOB, "admin_blob": BLOB})


def test_owner_adds_guest_and_guest_logs_in(client):
    _owner_logged(client)
    assert _add_guest(client).status_code == 200
    assert client.get("/api/sync/guests").json() == [{"user": "ana", "admin_blob": BLOB}]
    client.post("/api/sync/logout")
    assert client.get("/api/sync/prelogin", params={"user": "ana"}).json()["salt"] == SALT
    assert client.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH}).status_code == 200
    v = client.get("/api/sync/vault").json()
    assert v["enc_blob"] == BLOB and v["rev"] == 1


def test_guest_cannot_manage_guests(client):
    _owner_logged(client)
    _add_guest(client)
    client.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH})
    assert client.get("/api/sync/guests").status_code == 403
    assert _add_guest(client, "bia").status_code == 403


def test_guest_vault_is_separate_from_owner(client):
    _owner_logged(client)
    _add_guest(client)
    owner = client.get("/api/sync/vault").json()
    assert owner["enc_blob"] is None                     # o do dono não mudou
    client.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH})
    assert client.put("/api/sync/vault", json={"enc_blob": BLOB, "base_rev": 1}).json() == {"rev": 2}


def test_guest_cannot_take_owner_name(client):
    _owner_logged(client)
    r = _add_guest(client, "j")
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_usuario_em_uso"


@pytest.mark.parametrize("user", ["   ", "a/b"])
def test_guest_invalid_username_is_422(client, user):
    _owner_logged(client)
    assert _add_guest(client, user).status_code == 422
    assert client.get("/api/sync/guests").json() == []


def test_deleted_guest_vault_is_401(client):
    _owner_logged(client)
    _add_guest(client)
    guest = TestClient(client.app)
    guest.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH})
    assert client.post("/api/sync/guests/ana/delete").json() == {"ok": True}
    assert guest.get("/api/sync/vault").status_code == 401
    assert guest.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH}).status_code == 401


def test_prelogin_unknown_names_get_stable_fake_salts(client):
    _owner_logged(client)
    _add_guest(client)
    salt = lambda u: client.get("/api/sync/prelogin", params={"user": u}).json()["salt"]
    assert salt("x") != salt("y")
    assert salt("x") == salt("x")
    assert salt("x") != SALT and salt("y") != SALT


def test_login_unknown_user_still_pays_the_verifier(client, monkeypatch):
    import app.sync as sync
    _owner_logged(client)
    calls = []
    real = sync.make_verifier
    monkeypatch.setattr(sync, "make_verifier", lambda *a: calls.append(a) or real(*a))
    assert client.post("/api/sync/login", json={"user": "ghost", "auth_hash": GUEST_AUTH}).status_code == 401
    assert len(calls) == 1


def test_recreated_guest_rejects_old_cookie(client):
    _owner_logged(client)
    _add_guest(client)
    guest = TestClient(client.app)
    guest.post("/api/sync/login", json={"user": "ana", "auth_hash": GUEST_AUTH})
    assert guest.get("/api/sync/vault").status_code == 200
    client.post("/api/sync/guests/ana/delete")
    _add_guest(client)                                   # outra conta "ana"
    assert guest.get("/api/sync/vault").status_code == 401


def test_owner_old_format_cookie_still_works(client):
    import hashlib, hmac, time
    import app.sync as sync
    _owner_logged(client)
    msg = f"j.{int(time.time()) + 3600}"
    sig = hmac.new(sync._SESSION_SECRET, msg.encode(), hashlib.sha256).hexdigest()
    old = TestClient(client.app)
    old.cookies.set("cp_sync", f"{msg}.{sig}")
    assert old.get("/api/sync/vault").status_code == 200
    assert old.get("/api/sync/guests").status_code == 200   # e segue sendo o dono
    # o mesmo formato antigo não vale para convidado
    _add_guest(client)
    msg = f"ana.{int(time.time()) + 3600}"
    sig = hmac.new(sync._SESSION_SECRET, msg.encode(), hashlib.sha256).hexdigest()
    g = TestClient(client.app)
    g.cookies.set("cp_sync", f"{msg}.{sig}")
    assert g.get("/api/sync/vault").status_code == 401
