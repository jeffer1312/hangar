"""Voz do servidor pelas portas do Python (Connect): só o dono, ligado à porta privada do Rust.

A voz fala com o computador inteiro, então, ao contrário do terminal, convidado nunca entra.
"""
import asyncio
import contextlib
import logging
import secrets
import time
import urllib.error
import urllib.request

from fastapi import Request, WebSocket
from starlette.responses import JSONResponse, Response
from starlette.websockets import WebSocketDisconnect
from uvicorn.protocols.utils import ClientDisconnected

from app import guest_users, list_bridge
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings
from app.connect_port import CONNECT_PEER
from app.share_gate import guest_of
from app.termsock import _origem_aceita

_log = logging.getLogger(__name__)
UNAVAILABLE = "voice_unavailable"
# Sem proxy do ambiente: a porta privada é loopback.
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def is_guest(conn) -> bool:
    return guest_of(conn) is not None or guest_users.current.get() is not None


async def _owner_only(ws: WebSocket) -> bool:
    """Mesma trava do `termsock._porta_de_entrada`, sem a exceção do convidado."""
    host = ws.client.host if ws.client else ""
    now = time.time()
    if _blocked(host, now):
        await ws.close(code=1008)
        return False
    if is_guest(ws):
        await ws.close(code=1008)
        return False
    token = ws.query_params.get("token", "")
    if not settings.auth_token or not secrets.compare_digest(token.encode(), settings.auth_token.encode()):
        # Toda conexão do Connect chega com o mesmo endereço: contar as sem token bloquearia o dono.
        if host not in _LOOPBACK and not (host == CONNECT_PEER and not token):
            _record_fail(host, now)
        await ws.close(code=1008)
        return False
    origin = ws.headers.get("origin")
    if origin and not _origem_aceita(origin, ws.headers.get("host")):
        _log.warning("voicesock: origem %r recusada", origin)
        await ws.close(code=1008)
        return False
    return True


async def _unavailable(ws: WebSocket, reason: str) -> None:
    # Aceita antes de fechar: recusa antes do aceite vira 403 e o cliente perde o motivo.
    _log.warning("voicesock: Rust indisponível (%s)", reason)
    await ws.accept()
    await ws.close(code=1013, reason=UNAVAILABLE)


async def voice_ws(ws: WebSocket) -> None:
    if not await _owner_only(ws):
        return
    from websockets.asyncio.client import connect
    from websockets.exceptions import ConnectionClosed
    from websockets.protocol import State
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        await _unavailable(ws, "bridge_off")
        return
    address, secret = endpoint
    try:
        upstream = await connect(f"ws://{address}/__hangar_server/voice",
                                 additional_headers={"x-hangar-internal": secret}, proxy=None,
                                 compression=None, ping_interval=None, open_timeout=5, close_timeout=2)
    except Exception as e:                       # noqa: BLE001 — qualquer falha da ponte é indisponível
        await _unavailable(ws, type(e).__name__)
        return

    async def from_client():
        while True:
            msg = await ws.receive()
            if msg["type"] == "websocket.disconnect":
                return
            if (text := msg.get("text")) is not None:
                await upstream.send(text)

    async def from_rust():
        async for data in upstream:
            if isinstance(data, str):
                await ws.send_text(data)

    tasks: set[asyncio.Future] = set()
    rust_closed = False
    try:
        await ws.accept()
        tasks = {asyncio.ensure_future(from_client()), asyncio.ensure_future(from_rust())}
        await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    finally:
        # Mesmo cuidado do termsock: cancelamento na limpeza não deixa o repasse preso no Rust.
        try:
            for t in tasks:
                t.cancel()
            if tasks:
                await asyncio.wait(tasks)
            for t in tasks:
                error = None if t.cancelled() else t.exception()
                if error is not None and not isinstance(
                        error, (ConnectionClosed, WebSocketDisconnect, ClientDisconnected)):
                    _log.error("voicesock: repasse terminou com %s", type(error).__name__)
            rust_closed = upstream.state is not State.OPEN
            await upstream.close()
        finally:
            if upstream.state is not State.CLOSED:
                upstream.transport.abort()
    if rust_closed:
        code, reason = upstream.close_code, upstream.close_reason or ""
        # O Rust encerra a chamada com Close sem código (1005); sem Close nenhum, caiu.
        if code == 1005:
            code = 1000
        elif code in (None, 1006):
            code, reason = 1011, UNAVAILABLE
        with contextlib.suppress(RuntimeError, WebSocketDisconnect, ClientDisconnected):
            await ws.close(code=code, reason=reason)


async def forward_settings(request: Request) -> Response:
    """Leva o `GET`/`PUT` do dono à rota privada do Rust e devolve a resposta como veio."""
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        return JSONResponse({"error_code": UNAVAILABLE}, status_code=503)
    address, secret = endpoint
    body = await request.body()
    forwarded = urllib.request.Request(
        f"http://{address}/__hangar_server/voice/settings", data=body or None, method=request.method,
        headers={"content-type": request.headers.get("content-type", "application/json"),
                 "x-hangar-internal": secret})

    def send():
        try:
            with _opener.open(forwarded, timeout=30) as response:
                return response.status, response.headers.get("content-type"), response.read(1024 * 1024)
        except urllib.error.HTTPError as error:
            return error.code, error.headers.get("content-type"), error.read(1024 * 1024)

    try:
        status, kind, content = await asyncio.to_thread(send)
    except OSError:
        return JSONResponse({"error_code": UNAVAILABLE}, status_code=503)
    return Response(content, status_code=status, media_type=kind or "application/json")
