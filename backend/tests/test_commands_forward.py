import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def command_port(monkeypatch, tmp_path):
    from app import account_bridge, runtime_coordinator
    from app.config import settings
    from app.api import app

    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("PATH", "")
    monkeypatch.setattr(settings, "auth_token", "commands-fixture")
    coordinator = runtime_coordinator.RuntimeCoordinator()
    coordinator.mode = "rust"
    monkeypatch.setattr(runtime_coordinator, "_current", coordinator)
    monkeypatch.setattr(account_bridge, "_preparation_transport", None)
    seen = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            seen.append((self.path, self.headers.get("x-hangar-path"), self.headers.get("x-hangar-internal")))
            self.reply()

        def reply(self):
            body = json.dumps({"fixture": "commands-response"}).encode()
            self.send_response(207)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_PUT(self):
            body = self.rfile.read(int(self.headers.get("content-length", "0")))
            seen.append((self.path, self.headers.get("x-hangar-path"), self.headers.get("x-hangar-internal"), json.loads(body)))
            self.reply()

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever)
    worker.start()
    account_bridge.configure_preparation(f"127.0.0.1:{server.server_port}", "fixture-private")
    try:
        yield TestClient(app), seen
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
        assert not worker.is_alive(), "o servidor de prova não terminou"


@pytest.mark.parametrize("method", ["GET", "PUT"])
def test_session_defaults_forwards_from_connect(command_port, method):
    client, seen = command_port
    body = {"defaults": {"model": "opus", "effort": "high", "permission": "bypassPermissions"}}
    response = client.request(method, "/api/session-defaults?provider=claude", json=body if method == "PUT" else None,
                              headers={"Authorization": "Bearer commands-fixture"})
    assert response.status_code == 207
    assert seen[0][:3] == ("/__hangar_server/forwarded?provider=claude", "/api/session-defaults", "fixture-private")
    if method == "PUT":
        assert seen[0][3] == body


@pytest.mark.parametrize("method", ["GET", "PUT"])
def test_session_defaults_needs_auth(command_port, method):
    client, seen = command_port
    assert client.request(method, "/api/session-defaults?provider=claude").status_code == 401
    assert not seen
