"""Grupo entre máquinas pelas portas do Python. Na 8768 as rotas `/api/groups/*` e `GET …/group` só
repassam ao Rust (nunca um 404 sem código, que a outra máquina leria como versão antiga)."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, api, runtime_coordinator
from app.config import settings

TOKEN = "t-grupos"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


class _Rust(BaseHTTPRequestHandler):
    seen: list = []
    extra: dict = {}

    def _answer(self):
        length = int(self.headers.get("content-length") or 0)
        _Rust.seen.append({"method": self.command, "path": self.path, "original": self.headers.get("x-hangar-path"),
                           "body": self.rfile.read(length).decode() if length else ""})
        # Rota que o Rust não atende: o 404 com código do `forwarded::private`.
        unknown = _Rust.seen[-1]["original"] == "/api/groups/x/y"
        data = json.dumps({"code": "forwarded_route_not_owned"} if unknown else {"server_id": "casa", "de": "rust", **_Rust.extra}).encode()
        self.send_response(404 if unknown else 200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_PUT = do_POST = _answer

    def log_message(self, format, *args):
        pass


@pytest.fixture
def rust(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Rust)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Rust.seen = []
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    monkeypatch.setattr(account_bridge, "_preparation_transport", (f"127.0.0.1:{server.server_address[1]}", "segredo"))
    yield
    server.shutdown()


ROUTES = [("get", "/api/groups/g1", None), ("put", "/api/groups/g1", {"protocol": 1, "owner": "lab", "version": 2}),
          ("post", "/api/groups/g1/ops", {"op": "join"}), ("get", "/api/sessions/a/group", None)]


def test_connect_forwards_groups_routes(rust):
    client = TestClient(api.app)
    for method, path, body in ROUTES:
        kw = {"json": body} if body is not None else {}
        r = getattr(client, method)(path, headers=AUTH, **kw)
        assert (r.status_code, r.json()["de"]) == (200, "rust"), path
    assert [(s["method"], s["path"], s["original"]) for s in _Rust.seen] == [
        (m.upper(), "/__hangar_server/forwarded", p) for m, p, _ in ROUTES]
    # O corpo do PUT e do POST chega inteiro ao Rust.
    assert json.loads(_Rust.seen[1]["body"]) == ROUTES[1][2] and json.loads(_Rust.seen[2]["body"]) == ROUTES[2][2]
    # Caminho que o Rust não conhece também segue a ele, e volta com código.
    r = client.post("/api/groups/x/y", headers=AUTH, json={})
    assert (r.status_code, r.json()) == (404, {"code": "forwarded_route_not_owned"})
    assert _Rust.seen[-1]["original"] == "/api/groups/x/y"


def test_groups_routes_refuse_guest_with_code(rust, forwarded_guest):
    client = TestClient(api.app)
    assert client.get("/api/groups/g1").status_code == 401
    with forwarded_guest():
        r = client.get("/api/groups/g1", headers=AUTH)
        assert (r.status_code, r.json()["detail"]["code"]) == (403, "erro_fora_do_convite")
    assert _Rust.seen == []
