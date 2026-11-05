"""Reserva pré-aquecida pelo Connect: o Python só autentica e repassa ao Rust; convidado é recusado aqui."""
import contextvars
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, runtime_coordinator


@pytest.fixture
def connect(monkeypatch):
    seen = []

    class Handler(BaseHTTPRequestHandler):
        def _reply(self, status, body=b""):
            length = int(self.headers.get("content-length", "0"))
            seen.append((self.command, self.path, self.headers.get("x-hangar-path"), self.rfile.read(length)))
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_PUT(self):
            self._reply(200, b'{"id":"k1","fingerprint":"ab"}')

        def do_DELETE(self):
            self._reply(204)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    from app.api import app
    from app.config import settings
    monkeypatch.setattr(settings, "auth_token", "tok")
    coordinator = runtime_coordinator.RuntimeCoordinator()
    coordinator.mode = "rust"
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    account_bridge.configure_preparation(f"127.0.0.1:{server.server_port}", "sek")
    yield TestClient(app), seen
    account_bridge.configure_preparation(None, None)
    server.shutdown()
    server.server_close()


AUTH = {"Authorization": "Bearer tok"}
BODY = {"cwd": "/tmp", "client_id": "tela-1"}


def test_owner_put_and_delete_go_to_rust(connect):
    client, seen = connect
    got = client.put("/api/sessions/prewarm", json=BODY, headers=AUTH)
    assert got.status_code == 200
    assert got.json() == {"id": "k1", "fingerprint": "ab"}
    assert seen[-1][:3] == ("PUT", "/__hangar_server/forwarded", "/api/sessions/prewarm")
    assert client.delete("/api/sessions/prewarm/k1", headers=AUTH).status_code == 204
    assert seen[-1][:3] == ("DELETE", "/__hangar_server/forwarded", "/api/sessions/prewarm/k1")


def test_needs_auth(connect):
    client, seen = connect
    assert client.put("/api/sessions/prewarm", json=BODY).status_code == 401
    assert client.delete("/api/sessions/prewarm/k1").status_code == 401
    assert seen == []


def test_guest_is_refused_without_reaching_rust(connect, monkeypatch, forwarded_guest):
    client, seen = connect
    with forwarded_guest():
        got = client.put("/api/sessions/prewarm", json=BODY)
        assert got.status_code == 403
        assert got.json()["detail"]["code"] == "prewarm_owner_only"
        assert client.delete("/api/sessions/prewarm/k1").status_code == 403
        assert seen == []
