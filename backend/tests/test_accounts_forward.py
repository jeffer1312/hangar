"""Contas pelas portas do Python (Connect, convidado): o Rust é o único escritor.

Com o Rust de pé, o pedido de conta que chega ao Python, já autenticado, vai pela ponte privada.
A resposta do Rust volta como veio; sem o Rust (modo python), indisponível.
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, contas, runtime_coordinator
from app.api import app
from app.config import settings

TOKEN = "t-contas-ponte"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


class _Rust(BaseHTTPRequestHandler):
    seen: list = []
    reply: tuple[int, dict] = (201, {"id": "via-connect"})

    def _handle(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        _Rust.seen.append({"method": self.command, "path": self.path, "body": body,
                           "account_path": self.headers.get("x-hangar-path"),
                           "secret": self.headers.get("x-hangar-internal")})
        status, payload = _Rust.reply
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_DELETE = _handle

    def log_message(self, format, *args):
        pass


@pytest.fixture
def rust(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Rust)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Rust.seen = []
    _Rust.reply = (201, {"id": "via-connect"})
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    monkeypatch.setattr(account_bridge, "_preparation_transport",
                        (f"127.0.0.1:{server.server_address[1]}", "segredo-sintetico"))
    yield
    server.shutdown()


def test_account_write_goes_to_the_rust_owner(rust, monkeypatch):
    monkeypatch.setattr(contas, "criar", lambda *a, **k: pytest.fail("escritor Python com o Rust de pé"))
    response = TestClient(app).post("/api/codex-contas?x=1", headers=AUTH, json={"name": "via-connect"})
    assert (response.status_code, response.json()) == (201, {"id": "via-connect"})
    [seen] = _Rust.seen
    assert seen["method"] == "POST"
    assert seen["path"] == "/__hangar_server/accounts/public?x=1"
    assert seen["account_path"] == "/api/codex-contas"
    assert seen["secret"] == "segredo-sintetico"
    assert json.loads(seen["body"]) == {"name": "via-connect"}


def test_claude_account_creation_goes_to_the_rust_owner(rust, monkeypatch):
    """T66: o cadastro Claude pelas portas do Python também vai ao Rust; o `contas.criar` não roda."""
    monkeypatch.setattr(contas, "criar", lambda *a, **k: pytest.fail("escritor Python com o Rust de pé"))
    _Rust.reply = (200, {"path": "/x/.claude-nova", "label": "nova", "active": False})
    response = TestClient(app).post("/api/claude-configs", headers=AUTH, json={"nome": "nova"})
    assert (response.status_code, response.json()) == (200, {"path": "/x/.claude-nova", "label": "nova", "active": False})
    [seen] = _Rust.seen
    assert (seen["method"], seen["path"], seen["account_path"]) == ("POST", "/__hangar_server/accounts/public", "/api/claude-configs")
    assert json.loads(seen["body"]) == {"nome": "nova"}


def test_owner_refusal_reaches_the_caller_unchanged(rust):
    _Rust.reply = (409, {"detail": {"code": "erro_processos_usam_conta", "params": {"pids": [7]}}})
    response = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "erro_processos_usam_conta"


def test_rust_answer_reaches_the_caller_as_it_came(rust):
    _Rust.reply = (404, {"code": "account_route_not_owned"})
    response = TestClient(app).get("/api/claude-configs/sem-rota", headers=AUTH)
    assert (response.status_code, response.json()) == (404, {"code": "account_route_not_owned"})
    assert len(_Rust.seen) == 1


def test_unauthenticated_request_never_reaches_the_owner(rust):
    response = TestClient(app).post("/api/codex-contas", json={"name": "x"})
    assert response.status_code == 401
    assert _Rust.seen == []


def test_python_mode_answers_unavailable_without_the_owner(rust, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).get("/api/claude-configs", headers=AUTH)
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "accounts_need_rust_server"
    assert _Rust.seen == []


def test_jev_aux_goes_to_the_rust_owner_and_needs_it(rust, monkeypatch):
    response = TestClient(app).post("/api/jev/aux/models", headers=AUTH, json={"base_url": "http://x/v1"})
    assert response.status_code == _Rust.reply[0]
    [seen] = _Rust.seen
    assert (seen["path"], seen["account_path"]) == ("/__hangar_server/jev/aux", "/api/jev/aux/models")
    assert TestClient(app).get("/api/jev/aux/cliproxy").status_code == 401
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).get("/api/jev/aux/cliproxy", headers=AUTH)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_jev_aux_precisa_rust")
    assert len(_Rust.seen) == 1


def test_jev_delivery_availability_goes_to_rust_without_exposing_configuration(rust):
    _Rust.reply = (200, {"available": True})
    response = TestClient(app).get("/api/jev/delivery", headers=AUTH)
    assert (response.status_code, response.json()) == (200, {"available": True})
    [seen] = _Rust.seen
    assert (seen["path"], seen["account_path"]) == ("/__hangar_server/forwarded", "/api/jev/delivery")
    assert TestClient(app).get("/api/jev/delivery").status_code == 401
    assert len(_Rust.seen) == 1


def test_plan_routes_and_catalog_go_to_the_rust_owner(rust):
    _Rust.reply = (200, {"plans": [], "pinned": None})
    response = TestClient(app).get("/api/sessions/s1/plans?x=1", headers=AUTH)
    assert (response.status_code, response.json()) == (200, {"plans": [], "pinned": None})
    response = TestClient(app).post("/api/sessions/s1/plan-step", headers=AUTH,
                                    json={"stem": "p", "idx": 0, "done": True})
    assert response.status_code == 200
    TestClient(app).get("/api/claude/customizations?cwd=/tmp", headers=AUTH)
    seen = [(s["method"], s["path"], s["account_path"]) for s in _Rust.seen]
    assert seen == [("GET", "/__hangar_server/forwarded?x=1", "/api/sessions/s1/plans"),
                    ("POST", "/__hangar_server/forwarded", "/api/sessions/s1/plan-step"),
                    ("GET", "/__hangar_server/forwarded?cwd=/tmp", "/api/claude/customizations")]
    assert json.loads(_Rust.seen[1]["body"]) == {"stem": "p", "idx": 0, "done": True}


def test_plan_routes_need_rust_and_never_reach_it_unauthenticated(rust, monkeypatch):
    assert TestClient(app).get("/api/sessions/s1/plan").status_code == 401
    assert _Rust.seen == []
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).get("/api/sessions/s1/plan-preview", headers=AUTH)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_planos_precisa_rust")
    response = TestClient(app).get("/api/claude/customizations?cwd=/tmp", headers=AUTH)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "claude_customizations_unavailable")
    assert _Rust.seen == []


def test_guest_never_gets_the_owners_catalog(rust, monkeypatch):
    from app import guest_users
    token = guest_users.current.set(object())
    try:
        response = TestClient(app).get("/api/claude/customizations?cwd=/tmp", headers=AUTH)
    finally:
        guest_users.current.reset(token)
    assert (response.status_code, response.json()["detail"]["code"]) == (403, "erro_fora_da_pasta")
    assert _Rust.seen == []
