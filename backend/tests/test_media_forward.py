"""Anexos, `/transcribe`, saída de ferramenta e imagem colada pelas portas do Python: o dono é o Rust.

O pedido autenticado aqui segue em fluxo pela ponte privada (`/__hangar_server/forwarded`) com a marca de
convidado quando é um; sem o Rust a resposta é indisponível com código, sem reserva Python. A imagem de
uma conversa transferida continua no Python.
"""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import account_bridge, guest_users, runtime_coordinator
from app.api import app
from app.config import settings

TOKEN = "t-midia-ponte"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
AUDIO = bytes(range(256)) * 512
ROUTES = [("POST", "/api/sessions/s/upload?audio_only=1"), ("GET", "/api/sessions/s/uploads"),
          ("GET", "/api/sessions/s/uploads/1-abc.png?w=320"), ("POST", "/api/sessions/s/transcribe?limpar=1"),
          ("POST", "/api/sessions/s/bash-output"), ("GET", "/api/sessions/s/tool-progress/toolu_x"),
          ("GET", "/api/sessions/s/transcript-image/u-1/0")]


class _Rust(BaseHTTPRequestHandler):
    seen: list = []

    def _answer(self):
        length = self.headers.get("content-length")
        if length is not None:
            body = self.rfile.read(int(length))
        elif self.headers.get("transfer-encoding") == "chunked":
            body = b""
            while size := int(self.rfile.readline().strip(), 16):
                body += self.rfile.read(size)
                self.rfile.readline()
            self.rfile.readline()
        else:
            body = b""
        _Rust.seen.append({"method": self.command, "path": self.path, "body": body,
                           "original": self.headers.get("x-hangar-path"), "guest": self.headers.get("x-hangar-forwarded-guest"),
                           "secret": self.headers.get("x-hangar-internal"), "filename": self.headers.get("x-filename"),
                           "range": self.headers.get("range")})
        self.send_response(206)
        self.send_header("content-type", "image/png")
        self.send_header("content-range", "bytes 0-2/3")
        self.send_header("content-length", "3")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(b"png")

    do_GET = do_POST = do_HEAD = _answer

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


@pytest.mark.parametrize("method,url", ROUTES)
def test_every_media_route_goes_to_rust_with_query_and_reply_intact(rust, method, url):
    response = TestClient(app).request(method, url, headers={**AUTH, "Range": "bytes=0-2", "X-Filename": "a%20b.webm"},
                                       content=AUDIO if method == "POST" else None)
    assert (response.status_code, response.content, response.headers["content-range"]) == (206, b"png", "bytes 0-2/3")
    [seen] = _Rust.seen
    path, _, query = url.partition("?")
    assert seen["path"] == "/__hangar_server/forwarded" + (f"?{query}" if query else "")
    assert (seen["original"], seen["secret"], seen["guest"]) == (path, "segredo-sintetico", None)
    assert (seen["filename"], seen["range"]) == ("a%20b.webm", "bytes=0-2")
    assert seen["body"] == (AUDIO if method == "POST" else b"")


def test_stream_never_borrows_the_default_thread_pool(rust, monkeypatch):
    """Um upload grande não pode prender uma thread do executor padrão por pedaço: com vários ao
    mesmo tempo, o pool de 8 threads fica sem vaga e o backend inteiro espera."""
    import asyncio
    from app import auth

    original_to_thread = asyncio.to_thread
    offloads = []

    def auth_only(function, *args, **kwargs):
        assert function is auth._authenticate_request, "o repasse usou asyncio.to_thread"
        offloads.append(function)
        return original_to_thread(function, *args, **kwargs)

    monkeypatch.setattr(asyncio, "to_thread", auth_only)
    response = TestClient(app).post("/api/sessions/s/upload?audio_only=1", headers=AUTH, content=AUDIO)
    assert (response.status_code, response.content) == (206, b"png")
    assert _Rust.seen[0]["body"] == AUDIO
    assert offloads == [auth._authenticate_request]


def test_head_of_an_upload_also_goes_to_rust(rust):
    response = TestClient(app).head("/api/sessions/s/uploads/1-abc.png", headers=AUTH)
    assert response.status_code == 206
    assert [s["method"] for s in _Rust.seen] == ["HEAD"]


def test_guest_with_login_is_marked_for_the_rust_limits(rust):
    marker = guest_users.current.set(object())
    try:
        TestClient(app).post("/api/sessions/s/transcribe?arquivo=/abs.webm", headers=AUTH)
    finally:
        guest_users.current.reset(marker)
    assert [s["guest"] for s in _Rust.seen] == ["1"]


def test_transport_failure_is_503_without_a_second_attempt(rust, monkeypatch):
    import socket
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(4)
    accepted = []

    def drop():
        while True:
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            accepted.append(1)
            conn.close()

    threading.Thread(target=drop, daemon=True).start()
    monkeypatch.setattr(account_bridge, "_preparation_transport", (f"127.0.0.1:{listener.getsockname()[1]}", "s"))
    try:
        response = TestClient(app).post("/api/sessions/s/upload", headers=AUTH, content=b"x")
    finally:
        listener.close()
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "account_bridge_unavailable")
    assert accepted == [1]


def test_unauthenticated_request_never_reaches_rust(rust):
    assert TestClient(app).post("/api/sessions/s/upload", content=b"x").status_code == 401
    assert _Rust.seen == []


@pytest.mark.parametrize("method,url", ROUTES)
def test_python_mode_answers_unavailable_with_code(rust, monkeypatch, method, url):
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(mode="python"))
    response = TestClient(app).request(method, url, headers=AUTH, content=b"x" if method == "POST" else None)
    assert (response.status_code, response.json()["detail"]["code"]) == (503, "erro_midia_precisa_rust")
    assert _Rust.seen == []


def test_transferred_conversation_image_stays_in_python(rust, monkeypatch):
    from app import api
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: None)
    response = TestClient(app).get("/api/sessions/s/transcript-image/transfer:r:claude:u/0", headers=AUTH)
    assert (response.status_code, response.json()["detail"]["code"]) == (404, "erro_sessao_inexistente")
    assert _Rust.seen == []


def test_transferred_image_never_serves_active_content(rust, monkeypatch):
    from app import api, conversation_history
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: SimpleNamespace(jsonl="/x.jsonl", provider="codex", name=name))
    for media, served in (("text/html", "application/octet-stream"), ("image/svg+xml", "application/octet-stream"),
                          ("image/JPEG", "image/jpeg")):
        monkeypatch.setattr(conversation_history, "transcript_image", lambda info, uuid, idx, m=media: (b"img", m))
        response = TestClient(app).get("/api/sessions/s/transcript-image/transfer:r:claude:u/0", headers=AUTH)
        assert (response.status_code, response.headers["content-type"], response.headers["x-content-type-options"]) == (200, served, "nosniff")
