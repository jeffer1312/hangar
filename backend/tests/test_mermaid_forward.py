"""Diagrama Mermaid pelas portas do Python (Connect): o desenho é do Rust.

O dono que chega ao Python, já autenticado, segue pela ponte privada (`/__hangar_server/forwarded`) e
recebe o PNG com os cabeçalhos de tamanho. O convidado continua sem a rota; sem o Rust, indisponível.
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

TOKEN = "t-mermaid-ponte"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
BODY = {"source": "graph TD; A-->B", "theme": "dark", "width": 600}
# Todos os valores de byte: prova que o corpo binário volta intacto.
PNG = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR" + bytes(range(256)) * 64


class _Rust(BaseHTTPRequestHandler):
    seen: list = []
    reply: tuple = (200, "image/png", PNG, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        _Rust.seen.append({"path": self.path, "body": body, "original": self.headers.get("x-hangar-path"),
                           "secret": self.headers.get("x-hangar-internal")})
        status, kind, data, extra = _Rust.reply
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        for name, value in extra.items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format, *args):
        pass


PNG_HEADERS = {"x-hangar-css-width": "500", "x-hangar-css-height": "150",
               "cache-control": "private, max-age=31536000, immutable", "x-content-type-options": "nosniff",
               "x-hangar-interno": "nao-vaza"}


@pytest.fixture
def rust(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Rust)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _Rust.seen = []
    _Rust.reply = (200, "image/png", PNG, PNG_HEADERS)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="rust"))
    monkeypatch.setattr(account_bridge, "_preparation_transport",
                        (f"127.0.0.1:{server.server_address[1]}", "segredo-sintetico"))
    yield
    server.shutdown()


def test_owner_png_comes_back_byte_for_byte_with_its_headers(rust):
    response = TestClient(app).post("/api/mermaid", headers={**AUTH, "Origin": "https://outra.hangar.dev.br"}, json=BODY)
    assert response.status_code == 200
    assert response.content == PNG
    h = response.headers
    assert h["content-type"] == "image/png"
    assert (h["x-hangar-css-width"], h["x-hangar-css-height"]) == ("500", "150")
    assert h["cache-control"] == "private, max-age=31536000, immutable"
    assert h["x-content-type-options"] == "nosniff"
    assert "x-hangar-interno" not in h, "só os cabeçalhos do desenho voltam"
    exposed = h["access-control-expose-headers"].lower()
    assert "x-hangar-css-width" in exposed and "x-hangar-css-height" in exposed
    [seen] = _Rust.seen
    assert (seen["path"], seen["original"], seen["secret"]) == ("/__hangar_server/forwarded", "/api/mermaid", "segredo-sintetico")
    assert json.loads(seen["body"]) == BODY


def test_parse_error_and_unavailable_reach_the_caller_as_they_came(rust):
    for status, payload in ((422, {"code": "mermaid_invalido", "message": "Parse error on line 1"}),
                            (503, {"code": "mermaid_indisponivel", "message": "sem Chromium no servidor"})):
        _Rust.reply = (status, "application/json", json.dumps(payload).encode(), {})
        response = TestClient(app).post("/api/mermaid", headers=AUTH, json=BODY)
        assert (response.status_code, response.json()) == (status, payload)


def test_unauthenticated_request_never_reaches_the_owner(rust):
    assert TestClient(app).post("/api/mermaid", json=BODY).status_code == 401
    assert _Rust.seen == []


def test_guest_stays_without_the_route(rust, forwarded_guest):
    with forwarded_guest():
        response = TestClient(app).post("/api/mermaid", headers=AUTH, json=BODY)
    assert response.status_code == 404
    assert _Rust.seen == []


def test_python_mode_answers_unavailable_without_the_owner(rust, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).post("/api/mermaid", headers=AUTH, json=BODY)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "mermaid_indisponivel")
    assert _Rust.seen == []
