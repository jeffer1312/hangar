"""Contas pelas portas do Python (Connect, convidado): o Rust é o único escritor.

Com o Rust de pé, o pedido de conta que chega ao Python, já autenticado, vai pela ponte privada.
O Python só atende o que o Rust disser que não é dele, ou quando ele mesmo é o dono (modo python).
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


def test_owner_refusal_reaches_the_caller_unchanged(rust):
    _Rust.reply = (409, {"detail": {"code": "erro_processos_usam_conta", "params": {"pids": [7]}}})
    response = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert response.status_code == 409
    assert response.json()["detail"]["code"] == "erro_processos_usam_conta"


def test_route_the_rust_does_not_own_stays_in_python(rust):
    _Rust.reply = (404, {"code": "account_route_not_owned"})
    response = TestClient(app).get("/api/claude-configs", headers=AUTH)
    assert response.status_code == 200
    assert isinstance(response.json(), list)
    assert len(_Rust.seen) == 1


def test_unauthenticated_request_never_reaches_the_owner(rust):
    response = TestClient(app).post("/api/codex-contas", json={"name": "x"})
    assert response.status_code == 401
    assert _Rust.seen == []


def test_python_owner_answers_itself(rust, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).get("/api/claude-configs", headers=AUTH)
    assert response.status_code == 200
    assert _Rust.seen == []
