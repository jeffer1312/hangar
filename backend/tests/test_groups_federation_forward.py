"""Grupo entre máquinas pelas portas do Python. Na 8768 as rotas `/api/groups/*` e `GET …/group` só
repassam ao Rust (nunca um 404 sem código, que a outra máquina leria como versão antiga). E a rota de
grupo que vai pela ponte devolve ao Python somente as entregas que ele ainda atende."""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, api, groups_bridge, runtime_coordinator
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


def test_groups_routes_refuse_guest_and_python_mode_with_code(rust, monkeypatch, forwarded_guest):
    client = TestClient(api.app)
    assert client.get("/api/groups/g1").status_code == 401
    with forwarded_guest():
        r = client.get("/api/groups/g1", headers=AUTH)
        assert (r.status_code, r.json()["detail"]["code"]) == (403, "erro_fora_do_convite")
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    r = client.get("/api/sessions/a/group", headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (503, "groups_need_rust_server")
    assert _Rust.seen == []


@pytest.fixture
def bridge(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    state = SimpleNamespace(calls=[], reply=None)

    def call(op, **args):
        state.calls.append((op, args))
        return state.reply if op == "group.route" else {}
    monkeypatch.setattr(groups_bridge, "call", call)
    return state


def test_bridged_route_reads_no_orq_facts(bridge):
    bridge.reply = {"status": 200, "body": {"ok": True, "warning": None}, "post": []}
    r = TestClient(api.app).post("/api/sessions/a/pair", json={"peer": "b"}, headers=AUTH)
    assert r.status_code == 200
    [(op, args)] = bridge.calls
    assert op == "group.route" and args["facts"] == {}


def test_promote_conflict_comes_from_rust_without_a_second_request(bridge):
    bridge.reply = {"status": 409, "body": {"detail": {
        "code": "erro_orq_arquivo_mudou", "params": {}, "msg": "o grupo já possui outro contrato"}},
        "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_promote_os_error_preserves_who_did_not_leave(bridge):
    """A restauração parcial e seu diário são do Rust; a porta entrega o envelope inteiro."""
    bridge.reply = {"status": 409, "body": {"detail": {
        "code": "erro_orq_arquivo_mudou", "params": {"nao_sairam": ["b"]}, "msg": "disco cheio"}},
        "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert (r.status_code, r.json()["detail"]["code"]) == (409, "erro_orq_arquivo_mudou")
    assert r.json()["detail"]["params"]["nao_sairam"] == ["b"]
    assert [op for op, _ in bridge.calls] == ["group.route"]


def test_promote_without_identity_keeps_the_group_and_warns(bridge):
    bridge.reply = {"status": 200, "body": {"ok": True, "gid": "g9", "warning": {
        "code": "erro_orq_promocao_sem_identidade", "params": {"sessao": "arb", "grupo": "g9"},
        "msg": "o grupo ficou, mas o time não foi promovido"}}, "post": []}
    r = TestClient(api.app).post("/api/sessions/arb/pair", json={"orq": True}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["warning"]["code"] == "erro_orq_promocao_sem_identidade"
    assert r.json()["gid"] == "g9"
    assert [op for op, _ in bridge.calls] == ["group.route"]
