"""Portas Python: autorização e transporte do navegador remoto ao Rust."""
from __future__ import annotations

import asyncio
import contextlib
import logging
import secrets
import time
from urllib.parse import urlencode

from anyio import CancelScope
from fastapi import WebSocket, WebSocketDisconnect
from uvicorn.protocols.utils import ClientDisconnected

from app import list_bridge
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings

_log = logging.getLogger("hangar.navsock")
UNAVAILABLE = "remote_browser_needs_rust"


async def _autorizado(ws: WebSocket) -> bool:
    from app import shared_auth
    from app.guest_header import guest_of
    decision = await shared_auth.websocket(ws, shared_auth.BROWSER_WS, verified_guest=guest_of(ws) is not None)
    if decision is False:
        return False
    if decision is not None:
        if decision.status not in (shared_auth.OWNER, shared_auth.GUEST):
            await ws.close(code=1008)
            return False
        return True
    host = ws.client.host if ws.client else ""
    agora = time.time()
    if _blocked(host, agora):
        await ws.close(code=1008)
        return False
    from app.guest_header import guest_of
    if guest_of(ws) is not None:
        return True
    token = ws.query_params.get("token", "")
    if not settings.auth_token or not secrets.compare_digest(token.encode(), settings.auth_token.encode()):
        if host not in _LOOPBACK:
            _record_fail(host, agora)
        await ws.close(code=1008)
        return False
    return True


async def _unavailable(ws: WebSocket) -> None:
    await ws.accept()
    await ws.close(code=1013, reason=UNAVAILABLE)


async def nav_ws(ws: WebSocket, name: str, existe=None) -> None:
    if not await _autorizado(ws):
        return
    from websockets.asyncio.client import connect
    from websockets.exceptions import ConnectionClosed, InvalidStatus
    from websockets.protocol import State
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        await _unavailable(ws)
        return
    address, secret = endpoint
    raw_query = ws.scope.get("query_string", b"").decode("ascii")
    query = raw_query + "&" if raw_query else ""
    query += urlencode({"name": name})
    try:
        upstream = await connect(f"ws://{address}/__hangar_server/remote-browser?{query}",
                                 additional_headers={"x-hangar-internal": secret}, proxy=None,
                                 compression=None, ping_interval=None, max_size=32 * 1024 * 1024,
                                 open_timeout=5, close_timeout=2)
    except InvalidStatus as error:
        if error.response.status_code == 403:
            reason = bytes(error.response.body or b"").decode("utf-8", "replace")
            await ws.close(code=1008, reason=reason)
        else:
            await _unavailable(ws)
        return
    except Exception as error:
        _log.warning("navsock: transporte indisponível (%s)", type(error).__name__)
        await _unavailable(ws)
        return

    client_close = (1000, "")

    async def from_client():
        nonlocal client_close
        while True:
            message = await ws.receive()
            if message["type"] == "websocket.disconnect":
                client_close = (message.get("code", 1000), message.get("reason") or "")
                return
            if (text := message.get("text")) is not None:
                await upstream.send(text)
            elif (data := message.get("bytes")) is not None:
                await upstream.send(data)

    async def from_rust():
        async for message in upstream:
            if isinstance(message, str):
                await ws.send_text(message)
            else:
                await ws.send_bytes(message)

    tasks: set[asyncio.Task] = set()
    rust_closed = False
    code, reason = 1000, ""
    try:
        await ws.accept()
        tasks = {asyncio.create_task(from_client()), asyncio.create_task(from_rust())}
        await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    finally:
        # O cancelamento do ASGI precisa esperar o fechamento da conexão privada.
        with CancelScope(shield=True):
            try:
                for task in tasks:
                    task.cancel()
                if tasks:
                    await asyncio.wait(tasks)
                for task in tasks:
                    error = None if task.cancelled() else task.exception()
                    if error is not None and not isinstance(error, (ConnectionClosed, WebSocketDisconnect, ClientDisconnected)):
                        _log.warning("navsock: transporte terminou com %s", type(error).__name__)
                rust_closed = upstream.state is not State.OPEN
                if rust_closed:
                    code, reason = upstream.close_code, upstream.close_reason or ""
                close_code, close_reason = client_close
                if close_code in (1005, 1006, None):
                    close_code = 1000
                await upstream.close(code=close_code, reason=close_reason)
            finally:
                if upstream.state is not State.CLOSED:
                    upstream.transport.abort()
    if rust_closed:
        if code == 1005:
            code = 1000
        elif code in (1006, None):
            code, reason = 1011, UNAVAILABLE
        with contextlib.suppress(RuntimeError, WebSocketDisconnect, ClientDisconnected):
            await ws.close(code=code, reason=reason)
