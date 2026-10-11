"""Voz do servidor pelo Connect: o Python confere o dono e liga os quadros à porta privada do Rust."""
import asyncio
import contextvars
import json
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect
from websockets.asyncio.server import serve
from websockets.exceptions import ConnectionClosed

from app import auth, guest_users, list_bridge, voicesock
from app.api import app
from app.config import settings
from app.connect_port import CONNECT_PORT

TOKEN = "dono-voz"
VOICE_URL = f"ws://127.0.0.1:{CONNECT_PORT}/api/voice?token={TOKEN}"


class FakeRustVoice:
    """A rota privada `/__hangar_server/voice`: anota o que chega e responde `pong` ao `ping`."""

    def __init__(self):
        self.requests, self.received = [], []
        self.loop = asyncio.new_event_loop()
        started = threading.Event()

        def run():
            asyncio.set_event_loop(self.loop)
            self.stop_fut = self.loop.create_future()

            async def main():
                async with serve(self.handler, "127.0.0.1", 0) as server:
                    self.port = next(iter(server.sockets)).getsockname()[1]
                    started.set()
                    await self.stop_fut
            self.loop.run_until_complete(main())

        self.thread = threading.Thread(target=run, daemon=True)
        self.thread.start()
        assert started.wait(5)

    async def handler(self, conn):
        self.requests.append((conn.request.path, dict(conn.request.headers)))
        try:
            async for msg in conn:
                self.received.append(msg)
                if json.loads(msg) == {"type": "ping"}:
                    await conn.send(json.dumps({"type": "pong"}))
        except ConnectionClosed:
            pass

    def stop(self):
        self.loop.call_soon_threadsafe(self.stop_fut.set_result, None)
        self.thread.join(5)


class _RustSettings(BaseHTTPRequestHandler):
    seen: list = []

    def _handle(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        _RustSettings.seen.append({"method": self.command, "path": self.path, "body": body,
                                   "secret": self.headers.get("x-hangar-internal")})
        data = json.dumps({"from": "rust", "method": self.command}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_PUT = _handle

    def log_message(self, format, *args):
        pass


def _client():
    return TestClient(app, base_url=f"http://127.0.0.1:{CONNECT_PORT}", client=("127.0.0.1", 5))


def _as_logged_guest(monkeypatch):
    monkeypatch.setattr(guest_users, "current", contextvars.ContextVar("guest", default=object()))


@pytest.fixture(autouse=True)
def owner(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(settings, "port", 8765)
    yield
    list_bridge.configure(None, None)
    auth.reset_backoff()


@pytest.fixture
def rust():
    fake = FakeRustVoice()
    list_bridge.configure(f"127.0.0.1:{fake.port}", "sek")
    yield fake
    fake.stop()


def test_owner_text_frames_cross_both_ways(rust):
    with _client().websocket_connect(VOICE_URL) as ws:
        ws.send_text(json.dumps({"type": "ping"}))
        assert json.loads(ws.receive_text()) == {"type": "pong"}
    assert rust.received == ['{"type": "ping"}']
    path, headers = rust.requests[0]
    # O token do dono fica no Python: o Rust só recebe o segredo interno.
    assert path == "/__hangar_server/voice"
    assert headers["x-hangar-internal"] == "sek"


def test_guest_is_refused(rust, monkeypatch):
    monkeypatch.setattr(voicesock, "guest_of", lambda conn: object())
    with pytest.raises(WebSocketDisconnect) as e:
        with _client().websocket_connect(VOICE_URL) as ws:
            ws.receive_text()
    assert e.value.code == 1008
    assert rust.requests == []


def test_guest_with_own_login_is_refused(rust, monkeypatch):
    _as_logged_guest(monkeypatch)
    with pytest.raises(WebSocketDisconnect) as e:
        with _client().websocket_connect(VOICE_URL) as ws:
            ws.receive_text()
    assert e.value.code == 1008
    assert rust.requests == []


def test_wrong_token_is_refused(rust):
    with pytest.raises(WebSocketDisconnect) as e:
        with _client().websocket_connect(f"ws://127.0.0.1:{CONNECT_PORT}/api/voice?token=x") as ws:
            ws.receive_text()
    assert e.value.code == 1008
    assert rust.requests == []


@pytest.mark.parametrize("bridge", ["off", "closed_port"])
def test_rust_down_closes_1013(bridge):
    if bridge == "closed_port":
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        list_bridge.configure(f"127.0.0.1:{port}", "sek")
    with _client().websocket_connect(VOICE_URL) as ws:
        with pytest.raises(WebSocketDisconnect) as e:
            ws.receive_text()
    assert (e.value.code, e.value.reason) == (1013, "voice_unavailable")


@pytest.fixture
def rust_settings():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _RustSettings)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    _RustSettings.seen = []
    list_bridge.configure(f"127.0.0.1:{server.server_address[1]}", "sek")
    yield
    server.shutdown()


def test_settings_get_and_put_reach_rust_for_owner_only(rust_settings, monkeypatch):
    headers = {"Authorization": f"Bearer {TOKEN}"}
    got = _client().get("/api/voice/settings", headers=headers)
    assert (got.status_code, got.json()) == (200, {"from": "rust", "method": "GET"})
    body = b'{"codex_account":"a","voice":"marin"}'
    put = _client().put("/api/voice/settings", headers={**headers, "content-type": "application/json"}, content=body)
    assert (put.status_code, put.json()) == (200, {"from": "rust", "method": "PUT"})
    assert [(s["method"], s["path"], s["secret"]) for s in _RustSettings.seen] == [
        ("GET", "/__hangar_server/voice/settings", "sek"), ("PUT", "/__hangar_server/voice/settings", "sek")]
    assert _RustSettings.seen[1]["body"] == body

    _as_logged_guest(monkeypatch)
    assert _client().get("/api/voice/settings").status_code == 403
    assert _client().put("/api/voice/settings", content=body).status_code == 403
    assert len(_RustSettings.seen) == 2


def test_settings_without_rust_is_503():
    response = _client().get("/api/voice/settings", headers={"Authorization": f"Bearer {TOKEN}"})
    assert (response.status_code, response.json()) == (503, {"error_code": "voice_unavailable"})
