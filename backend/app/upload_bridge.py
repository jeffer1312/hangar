"""Admissão Python encaminha os anexos ao Rust, sem duplicar o cofre."""
from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request

from fastapi import HTTPException
from starlette.responses import StreamingResponse

from app.uploads import MAX_BYTES


def _too_large():
    return HTTPException(413, detail={"code": "erro_arquivo_grande", "params": {}, "msg": "arquivo maior que 100 MiB"})


def transport():
    from app import account_bridge
    if account_bridge.owner_mode() == "python":
        return None
    config = account_bridge.private_transport()
    if config is None:
        raise HTTPException(503, detail={"code": "upload_bridge_unavailable"})
    return config


def endpoint(config, name, params):
    return ("http://" + config[0] + "/__hangar_server/uploads/"
            + urllib.parse.quote(name, safe="") + "?" + urllib.parse.urlencode(params))


def request_json(name, action, *, data=None, filename="", allow_absolute=False):
    config = transport()
    if config is None:
        return None
    params = {"action": action, "filename": filename, "allow_absolute": str(allow_absolute).lower(),
              "audio_only": "true"}
    from app.account_bridge import _opener
    request = urllib.request.Request(endpoint(config, name, params), data=data,
                                     headers={"x-hangar-internal": config[1], "x-filename": urllib.parse.quote(filename, safe="")},
                                     method="POST" if data is not None else "GET")
    try:
        with _opener.open(request, timeout=60) as response:
            return json.loads(response.read(4 * 1024 * 1024))
    except urllib.error.HTTPError as error:
        try:
            detail = json.loads(error.read(65536)).get("detail", "anexo indisponível")
        except (ValueError, OSError):
            detail = "anexo indisponível"
        raise HTTPException(error.code, detail=detail) from None
    except (OSError, ValueError):
        raise HTTPException(503, detail={"code": "upload_bridge_unavailable"}) from None


async def forward(name, action, request, *, filename="", audio_only=False, download=False):
    """Conserva o stream e fecha o transporte quando o cliente sai."""
    config = transport()
    if config is None:
        return None
    if action == "save":
        length = request.headers.get("content-length", "")
        if length.isdigit() and int(length) > MAX_BYTES:
            raise _too_large()
    import asyncio
    import http.client
    connection = http.client.HTTPConnection(config[0], timeout=480)
    params = {"action": action, "filename": filename, "audio_only": str(audio_only).lower(),
              "download": str(download).lower()}
    target = urllib.parse.urlsplit(endpoint(config, name, params))
    headers = {"x-hangar-internal": config[1]}
    for key in ("x-filename", "range", "if-range", "if-none-match"):
        if key in request.headers:
            headers[key] = request.headers[key]
    if action == "save":
        headers["x-filename"] = headers.get("x-filename") or urllib.parse.quote(request.query_params.get("name", ""), safe="")
        if "content-length" in request.headers:
            headers["content-length"] = request.headers["content-length"]
        else:
            headers["transfer-encoding"] = "chunked"

    def open_headers():
        connection.putrequest("POST" if action == "save" else "GET", target.path + "?" + target.query)
        for key, value in headers.items():
            connection.putheader(key, value)
        connection.endheaders()

    try:
        await asyncio.to_thread(open_headers)
        if action == "save":
            chunked = "transfer-encoding" in headers
            size = 0
            async for chunk in request.stream():
                size += len(chunk)
                if size > MAX_BYTES:
                    raise _too_large()
                if not chunk:
                    continue
                block = (f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n") if chunked else chunk
                await asyncio.to_thread(connection.send, block)
            if chunked:
                await asyncio.to_thread(connection.send, b"0\r\n\r\n")
        response = await asyncio.to_thread(connection.getresponse)
    except (OSError, http.client.HTTPException):
        connection.close()
        raise HTTPException(503, detail={"code": "upload_bridge_unavailable"}) from None
    except BaseException:
        connection.close()
        raise

    async def chunks():
        try:
            while chunk := await asyncio.to_thread(response.read, 64 * 1024):
                yield chunk
        finally:
            response.close()
            connection.close()

    forwarded = {key: value for key, value in response.getheaders()
                 if key.lower() not in {"connection", "transfer-encoding", "keep-alive"}}
    return StreamingResponse(chunks(), status_code=response.status, headers=forwarded)
