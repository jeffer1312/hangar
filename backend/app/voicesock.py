"""Transporte HTTP das rotas legadas de compartilhamento."""
import asyncio
import urllib.error
import urllib.request

from fastapi import Request
from starlette.responses import JSONResponse, Response

from app import list_bridge

UNAVAILABLE = "share_needs_rust_server"
# Sem proxy do ambiente: a porta privada é loopback.
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


async def forward_settings(request: Request, target: str, *,
                           timeout: float = 30, unavailable=None) -> Response:
    """Leva o pedido do dono à rota privada do Rust e devolve a resposta como veio."""
    def down() -> Response:
        return unavailable() if unavailable else JSONResponse({"error_code": UNAVAILABLE}, status_code=503)
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        return down()
    address, secret = endpoint
    body = await request.body()
    forwarded = urllib.request.Request(
        f"http://{address}{target}", data=body or None, method=request.method,
        headers={"content-type": request.headers.get("content-type", "application/json"),
                 "x-hangar-internal": secret})

    def send():
        try:
            with _opener.open(forwarded, timeout=timeout) as response:
                return response.status, response.headers.get("content-type"), response.read(1024 * 1024)
        except urllib.error.HTTPError as error:
            return error.code, error.headers.get("content-type"), error.read(1024 * 1024)

    try:
        status, kind, content = await asyncio.to_thread(send)
    except OSError:
        return down()
    return Response(content, status_code=status, media_type=kind or "application/json")
