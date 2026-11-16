import asyncio
import functools
import json
import re
import time
from contextlib import asynccontextmanager
from fastapi import FastAPI, Depends, Request
from fastapi.responses import Response
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.gzip import GZipMiddleware
from starlette.requests import ClientDisconnect
from app import diag, loop_monitor
from app import internal_api
from app.auth import require_auth
from app.uploads import MAX_BYTES


class _BodyTooLarge(Exception):
    """Sinaliza corpo da request acima do limite (estoura no receive, antes de bufferizar tudo)."""


class _BodySizeLimitMiddleware:
    # Limite GLOBAL de corpo, em ASGI: conta os bytes do stream e aborta com 413 ao passar de max_bytes.
    # Cobre o que o check de Content-Length do /upload NAO pega (chunked, sem header) e roda ANTES do
    # require_auth -> impede o buffer ilimitado pre-auth. ponytail: teto global unico (= MAX_BYTES do
    # upload); se um dia precisar cap menor por rota, da pra escopar por scope["path"].
    def __init__(self, app, max_bytes: int):
        self.app = app
        self.max_bytes = max_bytes

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        headers = dict(scope.get("headers") or [])
        clen = headers.get(b"content-length")
        if clen is not None and clen.isdigit() and int(clen) > self.max_bytes:
            await self._reject(send)
            return
        total = 0
        started = False

        async def limited_receive():
            nonlocal total
            message = await receive()
            if message["type"] == "http.request":
                total += len(message.get("body", b""))
                if total > self.max_bytes:
                    raise _BodyTooLarge()
            return message

        async def tracked_send(message):
            nonlocal started
            if message["type"] == "http.response.start":
                started = True
            await send(message)

        try:
            await self.app(scope, limited_receive, tracked_send)
        except _BodyTooLarge:
            if not started:  # so responde se o handler ainda nao comecou a responder
                await self._reject(send)

    async def _reject(self, send):
        await send({"type": "http.response.start", "status": 413,
                    "headers": [(b"content-type", b"text/plain; charset=utf-8")]})
        await send({"type": "http.response.body", "body": b"request body too large"})


@asynccontextmanager
async def _lifespan(app: FastAPI):
    from app import diag_logging
    diag_logging.instalar()
    diag.registrar("backend.inicio", **diag.recursos())
    loop_monitor_task = asyncio.create_task(loop_monitor.watch(), name="loop-monitor")
    try:
        yield
    finally:
        diag.registrar("backend.encerrando")
        loop_monitor_task.cancel()
        await asyncio.gather(loop_monitor_task, return_exceptions=True)


app = FastAPI(title="hangar", lifespan=_lifespan)


@app.exception_handler(ClientDisconnect)
async def _client_gone(request: Request, exc: ClientDisconnect):
    """Quem pediu fechou antes de mandar o corpo (o Rust estoura o prazo dele e desiste): ninguém
    lê a resposta, e quem desistiu já registrou o próprio prazo."""
    return Response(status_code=499)


@app.middleware("http")
async def _correlaciona_diag(request: Request, call_next):
    """Põe o id do front no contexto, pra o diário poder LIGAR as duas pontas.

    Sem isto, a linha da tela ("POST /select devolveu 409") e a do servidor ("o cursor do picker não
    convergiu") ficam soltas no arquivo, e amarrar uma na outra depende de comparar horário — que
    empata assim que há duas telas abertas. Com o id, quem analisa segue a cadeia inteira de um
    toque só.
    """
    req = request.headers.get("x-hangar-req", "")[:32]
    path = request.url.path.removeprefix(request.scope.get("root_path", ""))
    if not req and re.fullmatch(r"/api/sessions/(?:[^/]+/)?events", path):
        # EventSource nativo não permite acrescentar o header de correlação.
        candidate = request.query_params.get("diag_req", "")
        if re.fullmatch(r"[A-Za-z0-9_-]{1,32}", candidate):
            req = candidate
    token = diag.req_atual.set(req)
    started = time.monotonic()
    response = None
    failure = ""
    error_code = None
    try:
        response = await call_next(request)
        # O código do erro mora no corpo; o de erro é pequeno, lido e devolvido igual.
        size = response.headers.get("content-length", "")
        if (response.status_code >= 400 and size.isdigit() and int(size) <= 8192
                and response.headers.get("content-type", "").startswith("application/json")):
            body = b"".join([chunk async for chunk in response.body_iterator])
            error_code = _codigo_do_erro(body)
            rebuilt = Response(content=body, status_code=response.status_code, background=response.background)
            rebuilt.raw_headers = list(response.raw_headers)
            response = rebuilt
        return response
    except Exception as exc:
        failure = type(exc).__name__
        raise
    finally:
        elapsed = int((time.monotonic() - started) * 1000)
        status = response.status_code if response is not None else 500
        # O template da rota não contém query, caminho de arquivo nem corpo do pedido.
        template = getattr(request.scope.get("route"), "path", None)
        route = template or "(rota desconhecida)"
        # O Rust chama `/internal/*` a cada tique: o POST que deu certo e rápido enchia o teto do dia de madrugada.
        internal_ok = request.url.path.startswith("/internal/") and status < 400 and not failure and elapsed < 1000
        if (response is not None or failure) and not internal_ok and not request.url.path.startswith("/api/diag") and (
                failure or status >= 400 or elapsed >= 1000 or request.method in ("POST", "PUT", "PATCH", "DELETE")):
            diag.registrar("api.servidor", "erro" if status >= 500 else "aviso" if status >= 400 else "ok",
                           detalhe=f"{request.method} {route}", codigo=str(status), ms=elapsed,
                           etapa="cabecalhos", sessao=request.path_params.get("name"), erro_tipo=failure,
                           causa=error_code, rota=None if template else _rota_mascarada(path))
        diag.req_atual.reset(token)


@functools.cache
def _palavras_das_rotas() -> frozenset[str]:
    return frozenset(seg for r in app.routes for seg in getattr(r, "path", "").split("/") if seg and "{" not in seg)


def _rota_mascarada(path: str) -> str:
    """O caminho sem rota conhecida: diz qual chamada errou o endereço. Só ficam as palavras que
    existem nas rotas; nome, id e qualquer outro texto do caminho viram `*`."""
    known = _palavras_das_rotas()
    return "/".join(p if not p or p in known else "*" for p in path.split("/"))[:120]


def _codigo_do_erro(body: bytes) -> str | None:
    """`detail.code` ou `code` do corpo de erro; o resto do corpo nunca vai ao diário."""
    try:
        value = json.loads(body)
    except ValueError:
        return None
    if not isinstance(value, dict):
        return None
    detail = value.get("detail")
    code = detail.get("code") if isinstance(detail, dict) else value.get("code")
    return code[:64] if isinstance(code, str) else None


# Body-size ANTES do CORS no codigo -> CORS fica por FORA (envolve ate o 413, adicionando headers CORS
# na rejeicao). Ver _BodySizeLimitMiddleware.
app.add_middleware(_BodySizeLimitMiddleware, max_bytes=MAX_BYTES)
# CORS liberado (token-gated): deixa o app servido por UMA origem (ex: tunnel de casa) falar com o
# backend de OUTRA maquina (ex: trabalho) cross-origin — API via header Bearer, SSE via ?token. Sem
# cookies cross-site (allow_credentials=False), entao "*" e seguro: continua exigindo o token.
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
    allow_credentials=False,
    # `allow_headers` cobre o pedido; o navegador so deixa o JS LER um header de resposta que esteja
    # aqui. Sem o ETag exposto, o PWA servido pela VPS falando com o backend de casa (cross-origin)
    # recebe o validador e nao consegue le-lo: o cache do chat nunca teria o que mandar no
    # If-None-Match e cairia calado no download inteiro, em toda entrada.
    expose_headers=["ETag", "x-hangar-css-width", "x-hangar-css-height"],
)
# JSON e assets grandes cruzam LAN/VPN; o Starlette exclui `text/event-stream`, sem segurar o SSE.
app.add_middleware(GZipMiddleware, minimum_size=1024, compresslevel=5)
app.include_router(internal_api.router)


@app.post("/api/diag", dependencies=[Depends(require_auth)])
async def diag_anotar(request: Request):
    """Lote de eventos da TELA pro diário de uso (backend/app/diag.py).

    Aceita o que reconhece e descarta o resto em silêncio — de propósito. Este endpoint não pode ser
    um caminho que falha: ele descreve o uso, e um 400 aqui viraria um erro na tela causado pelo
    próprio mecanismo de registrar erros.
    """
    try:
        corpo = await request.json()
    except Exception:                                # noqa: BLE001 — ver docstring
        return {"gravadas": 0}
    lote = corpo.get("eventos") if isinstance(corpo, dict) else corpo
    return {"gravadas": await asyncio.to_thread(diag.anotar_da_tela, lote)}


@app.get("/api/diag", dependencies=[Depends(require_auth)])
async def diag_resumo(ultimas: int = 60):
    resumo = await asyncio.to_thread(diag.resumo)
    # As últimas linhas junto: a tela precisa PROVAR que está gravando, e um segundo pedido só pra
    # isso seria mais latência pra mostrar a mesma coisa.
    resumo["ultimas"] = await asyncio.to_thread(diag.ultimas, max(1, min(ultimas, 200)))
    return resumo


@app.get("/api/diag/arquivo", dependencies=[Depends(require_auth)])
async def diag_arquivo():
    # Baixa como anexo pra pessoa mandar no chat — é o único caminho do diário até quem analisa.
    texto = await asyncio.to_thread(diag.ler_tudo)
    return Response(
        content=texto, media_type="application/x-ndjson",
        headers={"Content-Disposition": 'attachment; filename="hangar-uso.jsonl"'})
