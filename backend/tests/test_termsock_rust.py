"""Terminal com o Rust dono do PTY: o termsock só faz a porta de entrada e liga os bytes ao Rust."""
import asyncio
import threading
import time
from types import SimpleNamespace

import pytest
from fastapi import FastAPI, HTTPException, WebSocket
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect
from websockets.asyncio.server import serve
from websockets.exceptions import ConnectionClosed

from app import diag, list_bridge, runtime_coordinator, termsock, tmux
from app.config import settings

# Convidado de convite não passa mais por aqui: a 8766 e o terminal dele são do Rust.
OWNER_WS = "ws://127.0.0.1:8765"


class FakeRust:
    """A rota privada `/__hangar_server/term` do Rust: manda um quadro, ecoa os bytes e anota tudo."""

    def __init__(self, close_with=None, reject=None):
        self.requests, self.received = [], []
        self.closed = threading.Event()
        self.close_with = close_with
        self.reject = reject
        self.loop = asyncio.new_event_loop()
        started = threading.Event()

        def run():
            asyncio.set_event_loop(self.loop)
            self.stop_fut = self.loop.create_future()

            async def main():
                async with serve(self.handler, "127.0.0.1", 0, process_request=self.gate) as server:
                    self.port = next(iter(server.sockets)).getsockname()[1]
                    started.set()
                    await self.stop_fut
            self.loop.run_until_complete(main())

        self.thread = threading.Thread(target=run, daemon=True)
        self.thread.start()
        assert started.wait(5)

    def gate(self, conn, request):
        if self.reject:
            self.requests.append((request.path, dict(request.headers)))
            return conn.respond(self.reject, "")
        return None

    async def handler(self, conn):
        self.requests.append((conn.request.path, dict(conn.request.headers)))
        try:
            await conn.send(b"hello-from-rust")
            if self.close_with:
                await conn.close(*self.close_with)
                return
            async for msg in conn:
                self.received.append(msg)
                if isinstance(msg, bytes):
                    await conn.send(b"echo:" + msg)
        except ConnectionClosed:
            pass
        finally:
            self.closed.set()

    def stop(self):
        self.loop.call_soon_threadsafe(self.stop_fut.set_result, None)
        self.thread.join(5)
        assert not self.thread.is_alive(), "Rust falso não parou: conexão do repasse ficou aberta"


def _until(cond, timeout=3.0):
    deadline = time.monotonic() + timeout
    while not cond():
        assert time.monotonic() < deadline, "não aconteceu"
        time.sleep(0.02)


@pytest.fixture
def no_pty(monkeypatch):
    opened = []

    async def motor(ws, name, cols, rows):
        opened.append(name)
        await ws.accept()
        await ws.send_bytes(b"python-pty")
        await ws.close()
    monkeypatch.setattr(termsock, "_motor_posix", motor)
    monkeypatch.setattr(termsock, "_motor_windows", motor)
    return opened


@pytest.fixture
def rust(monkeypatch, no_pty):
    monkeypatch.setattr(settings, "auth_token", "secret")
    monkeypatch.setattr(settings, "port", 8765)
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    fake = FakeRust()
    list_bridge.configure(f"127.0.0.1:{fake.port}", "sek")
    yield fake
    list_bridge.configure(None, None)
    fake.stop()


def _app():
    a = FastAPI()

    @a.websocket("/api/sessions/{name}/term")
    async def term(ws: WebSocket, name: str):
        await termsock.term_ws(ws, name)

    return a


def _owner():
    return TestClient(_app(), base_url="http://127.0.0.1:8765", client=("127.0.0.1", 5))


def test_terminal_pipes_to_rust_no_pty(rust, no_pty):
    with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/cc/term?token=secret&cols=100&rows=30") as ws:
        assert ws.receive_bytes() == b"hello-from-rust"
        ws.send_bytes(b"abc")
        assert ws.receive_bytes() == b"echo:abc"
        ws.send_text('{"t":"resize","cols":90,"rows":20}')
        _until(lambda: '{"t":"resize","cols":90,"rows":20}' in rust.received)
    path, headers = rust.requests[0]
    # O token fica no Python: o Rust só recebe o alvo já conferido.
    assert path == "/__hangar_server/term?target=cc&cols=100&rows=30"
    assert headers["x-hangar-internal"] == "sek"
    _until(rust.closed.is_set)
    assert no_pty == []


class _SlowCleanup:
    """`asyncio` do termsock com a espera da limpeza lenta: o cancelamento pega o `finally` no meio."""

    def __getattr__(self, name):
        return getattr(asyncio, name)

    async def wait(self, fs, *args, **kwargs):
        if "return_when" not in kwargs:
            await asyncio.sleep(0.2)
        return await asyncio.wait(fs, *args, **kwargs)


def test_cancel_during_cleanup_still_closes_upstream(rust, monkeypatch):
    # O TestClient cancela o app logo após o desconectar, como um convidado revogado no meio da
    # limpeza: o repasse ao Rust fecha mesmo assim (o `stop` da fixture confere).
    monkeypatch.setattr(termsock, "asyncio", _SlowCleanup())
    with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/dono/term?token=secret") as ws:
        assert ws.receive_bytes() == b"hello-from-rust"
    _until(rust.closed.is_set)


def test_rust_close_code_reaches_the_client(rust, monkeypatch):
    taken = FakeRust(close_with=(1000, "outra conexao assumiu"))
    list_bridge.configure(f"127.0.0.1:{taken.port}", "sek")
    try:
        with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/cc/term?token=secret") as ws:
            assert ws.receive_bytes() == b"hello-from-rust"
            with pytest.raises(WebSocketDisconnect) as e:
                ws.receive_bytes()
        assert (e.value.code, e.value.reason) == (1000, "outra conexao assumiu")
    finally:
        taken.stop()


@pytest.mark.parametrize("broken", ["bridge_off", "starting"])
def test_bridge_failure_closes_1013_with_code_and_no_pty(rust, no_pty, monkeypatch, broken):
    events = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="info", **campos: events.append((evento, campos)))
    if broken == "bridge_off":
        list_bridge.configure(None, None)
    else:
        async def starting():
            raise runtime_coordinator.RuntimeStarting("subindo")
        monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="pending", await_mode=starting))
    with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/cc/term?token=secret") as ws:
        with pytest.raises(WebSocketDisconnect) as e:
            ws.receive_bytes()
    assert e.value.code == 1013
    assert any(evento == "terminal.ponte" and campos.get("codigo") for evento, campos in events), events
    assert no_pty == [] and rust.requests == []


def test_python_mode_keeps_the_python_pty(rust, no_pty, monkeypatch):
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="python"))
    with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/cc/term?token=secret") as ws:
        assert ws.receive_bytes() == b"python-pty"
    assert no_pty == ["cc"] and rust.requests == []


def test_409_asks_rust_and_503_on_bridge_error(monkeypatch):
    from app import api
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="rust"))
    # Um painel antigo do Python não conta: no modo `rust` o painel é do Rust.
    monkeypatch.setitem(termsock._ativos, "s1", object())
    answers = {"s1": {"active": False}, "s2": {"active": True}}
    asked = []

    def request(operation, arguments, **kw):
        asked.append((operation, arguments))
        if arguments["name"] == "s3":
            raise list_bridge.ListBridgeError("list_bridge_unavailable")
        return answers[arguments["name"]]
    monkeypatch.setattr(list_bridge, "_request", request)
    api._recusa_se_painel_aberto("s1")
    with pytest.raises(HTTPException) as e:
        api._recusa_se_painel_aberto("s2")
    assert e.value.status_code == 409 and e.value.detail["code"] == "erro_terminal_aberto"
    with pytest.raises(HTTPException) as e:
        api._recusa_se_painel_aberto("s3")
    assert e.value.status_code == 503
    assert e.value.detail["code"] == "erro_terminal_indisponivel"
    assert e.value.detail["params"]["detalhe"] == "list_bridge_unavailable"
    assert asked[0] == ("term.active", {"name": "s1"})
    # Modo `python` (reserva): o painel é o do Python, sem perguntar ao Rust.
    monkeypatch.setattr(runtime_coordinator, "_current", SimpleNamespace(mode="python"))
    asked.clear()
    with pytest.raises(HTTPException) as e:
        api._recusa_se_painel_aberto("s1")
    assert e.value.status_code == 409 and asked == []


@pytest.mark.parametrize("status, code", [(403, 1008), (404, 1013), (500, 1013)])
def test_rust_refusal_maps_to_a_close_code_with_status_in_the_diary(rust, monkeypatch, status, code):
    events = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="info", **campos: events.append((evento, campos)))
    refusing = FakeRust(reject=status)
    list_bridge.configure(f"127.0.0.1:{refusing.port}", "sek")
    try:
        with pytest.raises(WebSocketDisconnect) as e:
            with _owner().websocket_connect(f"{OWNER_WS}/api/sessions/cc/term?token=secret") as ws:
                ws.receive_bytes()
        assert e.value.code == code
        if status != 403:
            assert ("terminal.ponte", {"codigo": "terminal_bridge_refused", "sessao": "cc", "status": status}) in events
    finally:
        refusing.stop()
