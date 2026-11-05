"""Convidado decidido pelo Rust chega ao Python num cabeçalho assinado (`x-hangar-guest`)."""
import base64
import hashlib
import hmac
import json
from types import SimpleNamespace

import pytest
from fastapi import Depends, FastAPI, Request, WebSocket
from fastapi.testclient import TestClient

from app import guest_header, internal_api
from app.auth import require_auth
from app.config import settings

SECRET = "segredo-de-teste"
NOW = 1000
# Mesmo vetor do teste do Rust (`tests/it/share_forward.rs`): os dois lados assinam igual.
RUST_VECTOR = ("eyJzIjp7ImEiOiJzaGFyZSIsImIiOiJwYWlyIn0sImV4cCI6MTAzMH0."
               "96ab57f26c382e25a8c37a7f0956a26b80f43f723322cd992795c0968675a265")


def _sign(method: str, raw_path: str, sessions: dict, exp: int, secret: str = SECRET) -> str:
    payload = base64.urlsafe_b64encode(
        json.dumps({"s": sessions, "exp": exp}, separators=(",", ":")).encode()).rstrip(b"=").decode()
    mac = hmac.new(secret.encode(), f"{method}\n{raw_path}\n{payload}".encode(), hashlib.sha256).hexdigest()
    return f"{payload}.{mac}"


def _scope(value: str, *, method="GET", raw_path=b"/api/sessions/a/history", client="127.0.0.1",
           kind="http") -> dict:
    scope = {"type": kind, "raw_path": raw_path, "path": raw_path.decode(), "client": (client, 5),
             "headers": [(b"x-hangar-guest", value.encode())]}
    if kind == "http":
        scope["method"] = method
    return scope


@pytest.fixture(autouse=True)
def env(monkeypatch):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(guest_header, "time", SimpleNamespace(time=lambda: NOW))
    yield
    internal_api.set_secret(None)


def test_valid_from_loopback_gives_view():
    v = _sign("GET", "/api/sessions/a/history", {"a": "share", "b": "pair"}, NOW + 30)
    view = guest_header.guest_of(_scope(v))
    assert view is not None
    assert view.sessions == {"a": "share", "b": "pair"}
    assert view.sees("a") and not view.sees("c")
    assert view.kind_of("b") == "pair" and view.kind_of("c") is None


def test_guest_row_drops_every_field_that_names_another_session():
    """BRG-30: grupo e encadeamento citam outras sessões do dono; o convidado não vê nenhum."""
    from app.models import SessionInfo
    named = {"pair_peers": ["b", "ana::y"], "pair_gid": "ab12cd34", "pair_task": "T-1",
             "pair_external": {"alias": "ana", "owner": "Ana", "session": "y"}, "then_target": "c"}
    row = guest_header.guest_safe(SessionInfo(name="a", cwd="/r", **named))
    assert all(getattr(row, k) is None for k in named) and (row.name, row.cwd) == ("a", "/r")
    as_dict = guest_header.guest_safe({"name": "a", "cwd": "/r", **named})
    assert all(as_dict[k] is None for k in named) and as_dict["cwd"] == "/r"


def test_rust_vector_verifies():
    view = guest_header.guest_of(_scope(RUST_VECTOR))
    assert view is not None and view.sessions == {"a": "share", "b": "pair"}


def test_exp_equal_to_now_is_still_valid():
    v = _sign("GET", "/api/sessions/a/history", {"a": "share"}, NOW)
    assert guest_header.guest_of(_scope(v)) is not None


@pytest.mark.parametrize("case", ["no_secret", "expired", "other_path", "other_method", "outside",
                                  "tampered", "bad_mac", "garbage", "missing"])
def test_invalid_header_gives_none(case):
    v = _sign("GET", "/api/sessions/a/history", {"a": "share"}, NOW + 30)
    scope = _scope(v)
    if case == "no_secret":
        internal_api.set_secret(None)
    elif case == "expired":
        scope = _scope(_sign("GET", "/api/sessions/a/history", {"a": "share"}, NOW - 1))
    elif case == "other_path":
        scope = _scope(v, raw_path=b"/api/sessions/b/history")
    elif case == "other_method":
        scope = _scope(v, method="POST")
    elif case == "outside":
        scope = _scope(v, client="203.0.113.9")
    elif case == "tampered":
        payload, mac = v.split(".")
        other = base64.urlsafe_b64encode(b'{"s":{"z":"share"},"exp":1030}').rstrip(b"=").decode()
        scope = _scope(f"{other}.{mac}")
    elif case == "bad_mac":
        scope = _scope(v[:-1] + ("0" if v[-1] != "0" else "1"))
    elif case == "garbage":
        scope = _scope("nada")
    elif case == "missing":
        scope = {**scope, "headers": []}
    assert guest_header.guest_of(scope) is None


def test_signed_over_raw_path_not_decoded():
    v = _sign("GET", "/api/sessions/a%20b/history", {"a b": "share"}, NOW + 30)
    assert guest_header.guest_of(_scope(v, raw_path=b"/api/sessions/a%20b/history")) is not None
    assert guest_header.guest_of(_scope(v, raw_path=b"/api/sessions/a b/history")) is None


def test_websocket_scope_uses_get_and_raw_path():
    v = _sign("GET", "/api/sessions/a/nav-remoto", {"a": "share"}, NOW + 30)
    assert guest_header.guest_of(_scope(v, raw_path=b"/api/sessions/a/nav-remoto", kind="websocket")) is not None


def test_websocket_route_sees_guest():
    app = FastAPI()

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def nav(ws: WebSocket, name: str):
        await ws.accept()
        await ws.send_json({"guest": guest_header.guest_of(ws) is not None})
        await ws.close()

    c = TestClient(app, client=("127.0.0.1", 5))
    v = _sign("GET", "/api/sessions/a/nav-remoto", {"a": "share"}, NOW + 30)
    with c.websocket_connect("/api/sessions/a/nav-remoto?x=1", headers={"x-hangar-guest": v}) as ws:
        assert ws.receive_json() == {"guest": True}


@pytest.fixture
def gated(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "dono")
    app = FastAPI()

    @app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)])
    def history(name: str, request: Request):
        view = guest_header.guest_of(request)
        return {"sessions": view.sessions if view else None}

    return TestClient(app, client=("127.0.0.1", 5)), None


def test_require_auth_accepts_valid_header_without_authorization(gated):
    c, _ = gated
    v = _sign("GET", "/api/sessions/a/history", {"a": "share"}, NOW + 30)
    r = c.get("/api/sessions/a/history?limit=5", headers={"x-hangar-guest": v})
    assert r.status_code == 200
    assert r.json() == {"sessions": {"a": "share"}}
    assert c.get("/api/sessions/a/history").status_code == 401




def test_verified_once_per_request(gated, monkeypatch):
    c, _ = gated
    calls = []
    real = guest_header._verify
    monkeypatch.setattr(guest_header, "_verify", lambda scope: calls.append(1) or real(scope))
    v = _sign("GET", "/api/sessions/a/history", {"a": "share"}, NOW + 30)
    r = c.get("/api/sessions/a/history", headers={"x-hangar-guest": v})
    # Porteiro, require_auth e a rota perguntam; só a primeira confere a assinatura.
    assert r.status_code == 200 and r.json()["sessions"] == {"a": "share"}
    assert calls == [1]
