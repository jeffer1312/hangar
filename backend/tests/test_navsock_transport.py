"""Borda Python: autorização e transporte opaco ao WebSocket privado do Rust."""
import asyncio
import socket
import threading

import pytest
from fastapi import FastAPI, WebSocket
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect
from websockets.asyncio.server import serve
from websockets.exceptions import ConnectionClosed

from app import auth, list_bridge, navsock
from app.config import settings

TOKEN = "nav-transport-owner"


@pytest.fixture
def client(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    list_bridge.configure(None, None)
    auth.reset_backoff()
    app = FastAPI()

    @app.websocket("/api/sessions/{name}/nav-remoto")
    async def remote(ws: WebSocket, name: str):
        await navsock.nav_ws(ws, name)

    with TestClient(app, client=("127.0.0.1", 1)) as result:
        yield result
    list_bridge.configure(None, None)
    auth.reset_backoff()


class RustBoundary:
    def __init__(self):
        self.requests = []
        self.messages = []
        self.closes = []
        self.closed = threading.Event()
        self.loop = asyncio.new_event_loop()
        ready = threading.Event()

        async def handler(conn):
            self.requests.append((conn.request.path, dict(conn.request.headers)))
            try:
                async for message in conn:
                    self.messages.append(message)
                    if message == "close-rust":
                        await conn.close(4410, "convite encerrado")
                    else:
                        await conn.send(message)
            except ConnectionClosed:
                pass
            finally:
                self.closes.append((conn.close_code, conn.close_reason))
                self.closed.set()

        def run():
            asyncio.set_event_loop(self.loop)
            self.stop_future = self.loop.create_future()

            async def main():
                async with serve(handler, "127.0.0.1", 0) as server:
                    self.address = f"127.0.0.1:{server.sockets[0].getsockname()[1]}"
                    ready.set()
                    await self.stop_future
            self.loop.run_until_complete(main())
            self.loop.close()

        self.thread = threading.Thread(target=run)
        self.thread.start()
        assert ready.wait(5)

    def close(self):
        self.loop.call_soon_threadsafe(self.stop_future.set_result, None)
        self.thread.join(5)
        assert not self.thread.is_alive(), "transporte deixou a borda viva"


@pytest.fixture
def rust():
    boundary = RustBoundary()
    list_bridge.configure(boundary.address, "nav-private-secret")
    yield boundary
    boundary.close()


def test_without_rust_is_explicit_1013(client):
    with pytest.raises(WebSocketDisconnect) as error:
        with client.websocket_connect(f"/api/sessions/x/nav-remoto?token={TOKEN}") as ws:
            ws.receive_text()
    assert (error.value.code, error.value.reason) == (1013, "remote_browser_needs_rust")


def test_bidirectional_transport_preserves_raw_query_messages_and_close(client, rust):
    query = f"token={TOKEN}&w=390&q=80&opaque=a%2Fb&opaque=second"
    with client.websocket_connect(f"/api/sessions/nav-contract/nav-remoto?{query}",
                                  headers={"origin": "https://foreign.invalid"}) as ws:
        ws.send_text("{texto sem JSON: ação 😀}")
        assert ws.receive_text() == "{texto sem JSON: ação 😀}"
        ws.send_bytes(b"\x00\xff")
        assert ws.receive_bytes() == b"\x00\xff"
        ws.send_text("close-rust")
        with pytest.raises(WebSocketDisconnect) as error:
            ws.receive_text()
    assert (error.value.code, error.value.reason) == (4410, "convite encerrado")
    assert rust.messages == ["{texto sem JSON: ação 😀}", b"\x00\xff", "close-rust"]
    path, headers = rust.requests[0]
    assert path == f"/__hangar_server/remote-browser?{query}&name=nav-contract"
    assert headers["x-hangar-internal"] == "nav-private-secret"
    assert "authorization" not in headers


def test_wrong_token_is_refused_before_rust(client, rust):
    with pytest.raises(WebSocketDisconnect) as error:
        with client.websocket_connect("/api/sessions/x/nav-remoto?token=wrong"):
            pass
    assert error.value.code == 1008
    assert rust.requests == []


def test_closed_private_port_is_explicit_1013(client):
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        address = f"127.0.0.1:{reserved.getsockname()[1]}"
    list_bridge.configure(address, "nav-private-secret")
    with pytest.raises(WebSocketDisconnect) as error:
        with client.websocket_connect(f"/api/sessions/x/nav-remoto?token={TOKEN}") as ws:
            ws.receive_text()
    assert (error.value.code, error.value.reason) == (1013, "remote_browser_needs_rust")


def test_query_name_cannot_replace_the_authorized_route_target(client, rust):
    query = f"token={TOKEN}&name=other-session&name=third-session&w=390"
    with client.websocket_connect(f"/api/sessions/nav-contract/nav-remoto?{query}") as ws:
        ws.send_text("alvo da rota")
        assert ws.receive_text() == "alvo da rota"
    assert rust.requests[0][0] == f"/__hangar_server/remote-browser?{query}&name=nav-contract"


def test_normal_client_disconnect_closes_the_private_connection(client, rust):
    with client.websocket_connect(f"/api/sessions/nav-contract/nav-remoto?token={TOKEN}") as ws:
        ws.send_text("antes de sair")
        assert ws.receive_text() == "antes de sair"
    assert rust.closed.wait(3), "desconexão deixou a ponte aberta"
    assert rust.closes == [(1000, "")]
