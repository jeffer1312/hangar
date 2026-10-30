"""`GET /api/client-api` pelas portas do Python: o número é do Rust.

O dono do Connect segue pela ponte privada e recebe o número dele; sem o Rust, 503 com código (o
cliente lê "desconhecido"), nunca um número inventado.
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, runtime_coordinator
from app.api import app
from app.config import settings

TOKEN = "t-client-api"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


class _Rust(BaseHTTPRequestHandler):
    seen: list = []

    def do_GET(self):
        _Rust.seen.append({"path": self.path, "original": self.headers.get("x-hangar-path"),
                           "secret": self.headers.get("x-hangar-internal")})
        data = json.dumps({"api": 7}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

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
    monkeypatch.setattr(account_bridge, "_preparation_transport",
                        (f"127.0.0.1:{server.server_address[1]}", "segredo-sintetico"))
    yield
    server.shutdown()


def test_connect_owner_gets_the_number_from_rust(rust):
    response = TestClient(app).get("/api/client-api", headers=AUTH)
    assert (response.status_code, response.json()) == (200, {"api": 7})
    [seen] = _Rust.seen
    assert (seen["path"], seen["original"], seen["secret"]) == ("/__hangar_server/forwarded", "/api/client-api", "segredo-sintetico")


def test_unauthenticated_and_guest_never_reach_rust(rust, forwarded_guest):
    assert TestClient(app).get("/api/client-api").status_code == 401
    with forwarded_guest():
        assert TestClient(app).get("/api/client-api", headers=AUTH).status_code == 404
    assert _Rust.seen == []


def test_python_mode_answers_503_with_code(rust, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).get("/api/client-api", headers=AUTH)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "client_api_need_rust_server")
    assert _Rust.seen == []
