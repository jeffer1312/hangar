import anyio.to_thread
import asyncio
import functools
import json
import logging
import re
import time
import urllib.parse
from contextlib import asynccontextmanager
from pathlib import Path
from fastapi import FastAPI, Depends, HTTPException, Request, WebSocket
from fastapi.responses import JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.gzip import GZipMiddleware
from starlette.requests import ClientDisconnect
from app import diag, loop_monitor, plugin_bridge, quem_chama
from app import internal_api, list_bridge, share_bridge
from app.auth import require_auth
from app.mensagens import erro
from app.registry import SessionRegistry
from app.adapters import CLAUDE_HEADLESS, get_adapter
from app.uploads import MAX_BYTES
from app.config import (list_config_dirs, _backend_config_base)
from app.guest_header import guest_of
from app.hook_state import hook_state

_log = logging.getLogger("hangar")


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
    from app import runtime_coordinator
    runtime = runtime_coordinator.ensure()
    await runtime.start_sessions({"claude":get_adapter(CLAUDE_HEADLESS), "codex":get_adapter("codex")})
    # Uma vez na subida, nunca por request. O Starlette roda cada rota `def` (sao 65 aqui) num
    # anyio.to_thread, cujo limiter default e de 40 tokens — e cada conexao de chat ainda segura
    # DOIS deles PERMANENTEMENTE, num awatch parado (transcript.py:408 e pqueue.py:366). Com ~20
    # abas os 40 acabam e a API inteira congela, sem erro e sem log. Watcher parado nao gasta CPU,
    # so o slot, entao subir o teto e barato.
    anyio.to_thread.current_default_thread_limiter().total_tokens = 200
    _state_dirs =list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    task = asyncio.create_task(hook_state.watch(_state_dirs))

    def _watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("hook_state.watch crashed", exc_info=exc)

    task.add_done_callback(_watch_done)

    loop_monitor_task = asyncio.create_task(loop_monitor.watch(), name="loop-monitor")
    try:
        yield
    finally:
        diag.registrar("backend.encerrando")
        await runtime.shutdown()
        await runtime.close_events()
        # Claude sem terminal fica vivo no cano: só fecha a conexão; o próximo backend religa.
        get_adapter(CLAUDE_HEADLESS).desligar_todas()
        task.cancel()
        loop_monitor_task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        await asyncio.gather(loop_monitor_task, return_exceptions=True)


app = FastAPI(title="hangar", lifespan=_lifespan)


@app.exception_handler(ClientDisconnect)
async def _client_gone(request: Request, exc: ClientDisconnect):
    """Quem pediu fechou antes de mandar o corpo (o Rust estoura o prazo dele e desiste): ninguém
    lê a resposta, e quem desistiu já registrou o próprio prazo."""
    return Response(status_code=499)


@app.exception_handler(list_bridge.ListBridgeError)
async def _lista_indisponivel(request: Request, exc: list_bridge.ListBridgeError):
    """Com o Rust dono, a lista vem dele; ponte fora ou Rust ainda subindo é 503 com o código, nunca
    a lista vazia nem a do Python (`_mux_indisponivel`, pelo mesmo motivo)."""
    diag.registrar("lista.indisponivel", "erro", codigo=exc.code, detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(status_code=503, content={"detail": erro(
        "erro_lista_indisponivel", "a lista de sessões está indisponível", detalhe=exc.code)})


async def _repasse_ao_rust(request: Request, destino: str, **repasse):
    """Pedido de uma rota que só o Rust atende, chegado por uma porta do Python (Connect,
    convidado): autenticado aqui, ele segue pela ponte privada e a resposta volta como veio."""
    from app import account_bridge
    try:
        await require_auth(request)
    except HTTPException as error:
        return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
    return await account_bridge.forward_public(request, destino, **repasse)


_PROVIDER_ROUTES = re.compile(r"^/api/sessions/[^/]+/(pi/models?|kimi/models?|orq(?:/(panel|papel|papeis|grupo|comecar))?)$")


@app.middleware("http")
async def _providers_via_rust(request: Request, call_next):
    path = request.url.path
    owned = (_PROVIDER_ROUTES.fullmatch(path) is not None
             or path == "/api/omp/plugin-sync"
             or path == "/api/orq" or path.startswith("/api/orq/")
             or path == "/api/orquestracao/politica" or path.startswith("/api/orquestracao/politica/")
             or path == "/api/model-options" and request.query_params.get("provider") in {"pi", "omp", "kimi"})
    read = re.fullmatch(r"/api/sessions/([^/]+-orq)/(history|events)", path)
    write = re.fullmatch(r"/api/sessions/([^/]+-orq)(?:/(input|interrupt))?", path)
    write = write if write and (request.method == "DELETE" and write.group(2) is None
                               or request.method == "POST" and write.group(2) is not None) else None
    if read or write:
        try:
            await require_auth(request)
        except HTTPException as error:
            return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
        owned = True
    if not owned:
        return await call_next(request)
    if read:
        from app import account_bridge
        try:
            await require_auth(request)
        except HTTPException as error:
            return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
        response = await account_bridge.forward_stream(request, guest=guest_of(request) is not None)
        return await call_next(request) if response.status_code == 421 else response
    response = await _repasse_ao_rust(request, "/__hangar_server/forwarded", limit=None)
    return await call_next(request) if write and response.status_code == 421 else response


@app.middleware("http")
async def _contas_pelo_rust(request: Request, call_next):
    """Contas e cotas têm um dono só, o Rust."""
    from app import account_bridge
    listing = request.method == "GET" and request.url.path == "/api/credenciais"
    if not (listing or request.url.path.startswith(account_bridge.ACCOUNT_PREFIXES)):
        return await call_next(request)
    return await _repasse_ao_rust(request, "/__hangar_server/accounts/public")


@app.middleware("http")
async def _worktrees_pelo_rust(request: Request, call_next):
    """Nas portas do Python, worktrees seguem ao mesmo dono das políticas e da classificação."""
    from app import account_bridge
    path, method = request.url.path, request.method
    owned = (method == "GET" and path in {"/api/worktrees", "/api/worktrees/detail", "/api/worktrees/settings"}
             or method == "PUT" and path == "/api/worktrees/settings"
             or method == "POST" and path in {"/api/worktrees/create", "/api/worktrees/delete", "/api/worktrees/delete-merged", "/api/worktrees/fetch"})
    if not owned:
        return await call_next(request)
    if guest_of(request) is not None:
        return JSONResponse({"detail": "convidado não acessa worktrees"}, status_code=403)
    options = {"limit": None, "default_json_content_type": False} if path == "/api/worktrees/fetch" else {}
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded", **options)


@app.middleware("http")
async def _jev_aux_pelo_rust(request: Request, call_next):
    """A busca de modelos e a disponibilidade do Jev são do Rust; sem ele, o erro diz isso em vez de um 404 que a tela
    leria como servidor antigo."""
    delivery = request.url.path == "/api/jev/delivery"
    if not delivery and not request.url.path.startswith("/api/jev/aux/"):
        return await call_next(request)
    if delivery and (guest_of(request) is not None):
        return JSONResponse({"detail": "Not Found"}, status_code=404)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded" if delivery else "/__hangar_server/jev/aux")


@app.middleware("http")
async def _grupos_pelo_rust(request: Request, call_next):
    """Grupo entre máquinas: rotas só do Rust. Pelas portas do Python (Connect) tudo segue a ele, que
    decide o que atende; o que não é dele volta com código, nunca o 404 cru que a outra máquina leria
    como versão antiga."""
    path = request.url.path
    if not (path.startswith("/api/groups/") or (request.method == "GET" and path.startswith("/api/sessions/") and path.endswith("/group"))):
        return await call_next(request)
    if guest_of(request) is not None:
        return JSONResponse({"detail": erro("erro_fora_do_convite", "fora da sessao compartilhada")}, status_code=403)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")


# Rotas de plano e o catálogo de plugins e skills: o dono é o Rust (`plans::routes`, `claude_customizations`).
_PLANOS_LEITURA = re.compile(r"^/api/sessions/[^/]+/(plan|plans|plan-preview)$")
_PLANOS_ESCRITA = re.compile(r"^/api/sessions/[^/]+/plan-(pin|step|archive)$")
_CATALOGO = "/api/claude/customizations"
# Retomar (Arquivo e sessão viva): a abertura e a reabertura são do Rust.
_RETOMAR = re.compile(r"^/api/(archive/[^/]+/[^/]+|sessions/[^/]+)/resume$")


@app.middleware("http")
async def _planos_pelo_rust(request: Request, call_next):
    """Os porteiros do convidado rodam por fora deste middleware; aqui só o catálogo, que é do
    dono, recusa o convidado."""
    path, metodo = request.url.path, request.method
    if metodo == "GET" and path == _CATALOGO:
        if guest_of(request) is not None:
            return JSONResponse({"detail": erro("erro_fora_da_pasta", "a seleção de plugins pertence ao dono")}, status_code=403)
        return await _repasse_ao_rust(request, "/__hangar_server/forwarded")
    if metodo == "POST" and _RETOMAR.match(path):
        return await _repasse_ao_rust(request, "/__hangar_server/forwarded")
    padrao = _PLANOS_ESCRITA if metodo == "POST" else _PLANOS_LEITURA if metodo == "GET" else None
    if padrao is None or not padrao.match(path):
        return await call_next(request)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")


# Busca, Perguntar e Arquivo são só do Rust: sem ele, indisponível com código, sem reserva.
_SEARCH_ASK_ARCHIVE = re.compile(r"^/api/(search|search/context|archive|archive-por-cwd|archive/recent|archive/[^/]+"
                            r"|archive/[^/]+/[^/]+/history|archive/[^/]+/[^/]+/transcript-image/[^/]+/[^/]+)/?$")


@app.middleware("http")
async def _search_without_rust(request: Request, call_next):
    from app import account_bridge
    path, metodo = request.url.path, request.method
    owned = metodo == "GET" and _SEARCH_ASK_ARCHIVE.match(path) or metodo == "POST" and path.rstrip("/") == "/api/ask-history"
    if not owned:
        return await call_next(request)
    try:
        await require_auth(request)
    except HTTPException as error:
        return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
    # Com o Rust de pé ele atende o dono: o que chega aqui é convidado ou Connect.
    if account_bridge.owner_mode() == "rust":
        return JSONResponse({"detail": erro("erro_busca_so_dono", "busca, Perguntar e Arquivo atendem só o dono do servidor")},
                            status_code=403)
    return JSONResponse({"detail": erro("erro_busca_precisa_rust", "busca, Perguntar e Arquivo precisam do servidor Rust, que não está rodando")},
                        status_code=503)


# Anexos, `/transcribe`, saída de ferramenta e imagem colada no terminal: o dono é o Rust
# (`uploads`, `session_media`).
_MIDIA = re.compile(r"^/api/sessions/[^/]+/(upload|uploads|uploads/[^/]+|transcribe|bash-output|tool-progress/[^/]+|transcript-image/[^/]+/[^/]+)$")


@app.middleware("http")
async def _midia_pelo_rust(request: Request, call_next):
    from app import account_bridge
    match, metodo = _MIDIA.match(request.url.path), request.method
    rota = match.group(1) if match else ""
    owned = (metodo == "POST" and rota in ("upload", "transcribe", "bash-output")
             or metodo in ("GET", "HEAD") and rota.startswith("uploads")
             or metodo == "GET" and (rota.startswith("tool-progress/")
                                     or rota.startswith("transcript-image/")))
    if not owned:
        return await call_next(request)
    try:
        await require_auth(request)
    except HTTPException as error:
        return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
    guest = guest_of(request) is not None
    return await account_bridge.forward_stream(request, guest=guest)


# Cabeçalhos do PNG do diagrama que voltam do Rust; o tipo já volta sempre.
_MERMAID_HEADERS = ("x-hangar-css-width", "x-hangar-css-height", "cache-control", "x-content-type-options")


@app.post("/api/mermaid")
async def mermaid_pelo_rust(request: Request):
    """O desenho é do Rust (`mermaid::serve`). Aqui chegam o dono pelo Connect, que segue pela ponte,
    e quem o Rust não reconheceu como dono. O convidado fica sem a rota, como antes."""
    if guest_of(request) is not None:
        return JSONResponse({"detail": "Not Found"}, status_code=404)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded", keep=_MERMAID_HEADERS, limit=64 * 1024 * 1024)


@app.api_route("/api/session-defaults", methods=["GET", "PUT"], dependencies=[Depends(require_auth)])
async def session_defaults_forward(request: Request):
    """O Connect só transporta o padrão; leitura e escrita pertencem ao Rust."""
    if guest_of(request) is not None:
        raise HTTPException(403, detail=erro("session_defaults_owner_only", "só o dono pode acessar o padrão da abertura"))
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")


@app.put("/api/sessions/prewarm", dependencies=[Depends(require_auth)])
@app.delete("/api/sessions/prewarm/{reserve_id}", dependencies=[Depends(require_auth)])
async def prewarm_forward(request: Request):
    """A reserva pré-aquecida é do Rust (`prewarm::http`); o Connect só autentica e repassa."""
    if _convidado(request):
        raise HTTPException(403, detail=erro("prewarm_owner_only", "só o dono reserva uma sessão pré-aquecida"))
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")


@app.get("/api/client-api")
async def client_api_pelo_rust(request: Request):
    """O número é do Rust (`hangar_api::CLIENT_API`). Aqui chega o dono pelo Connect, que segue pela
    ponte; sem o Rust é 503 com código, nunca um número que o Python não conhece."""
    if _convidado(request):
        return JSONResponse({"detail": "Not Found"}, status_code=404)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")


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


# Porteiro do convidado com login ANTES do CORS: o CORS o envolve, entao ate o 403 dele sai com
# Access-Control-Allow-Origin e o app do convidado le o codigo em vez de "erro de rede".
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
# Roteadores por assunto (Task 1 do plano descoberta-e-configuracao): cada Task do lote escreve
# só no módulo dela. Última edição de api.py deste plano.
app.include_router(plugin_bridge.plugin_router)
# Convites e par externo são do Rust; aqui só o repasse do dono que chega pelo Connect.
app.include_router(share_bridge.router)
app.include_router(internal_api.router)
registry = SessionRegistry()

@app.websocket("/api/sessions/{name}/term")
async def term_ws_route(ws: WebSocket, name: str):
    from app import termsock
    resolve = None
    await termsock.term_ws(ws, name, resolve)


@app.websocket("/api/sessions/{name}/nav-remoto")
async def nav_ws_route(ws: WebSocket, name: str):
    # Acesso remoto ao navegador embutido DAQUELA sessao: quadros pra fora, toque/tecla pra dentro.
    # Mesma porta de entrada do painel de terminal (token + Origin) -- o de la abre um shell, este
    # abre o navegador que o agente esta dirigindo.
    from app import navsock
    await navsock.nav_ws(ws, name)


@app.get("/api/whoami", dependencies=[Depends(require_auth)])
async def whoami(request: Request):
    """Qual sessão está chamando, pelos cabeçalhos X-Hangar-* (ver quem_chama). Diagnóstico e
    teste da resolução que o MCP usa; o CLI continua resolvendo localmente."""
    try:
        nome, origem = await asyncio.to_thread(quem_chama.resolver, request.headers)
    except quem_chama.SessaoDesconhecida as e:
        raise HTTPException(404, detail=erro("erro_sessao_desconhecida", str(e)))
    return {"name": nome, "origem": origem}


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


@app.post("/api/sessions/{name}/rename")
async def rename_session(name: str, request: Request):
    """Trocar o nome é do Rust; aqui chega só quem entrou por uma porta do Python (Connect, convidado
    com conta), e segue pela ponte."""
    return await _repasse_ao_rust(request, f"/__hangar_server/claude/{urllib.parse.quote(name, safe='')}/rename")


@app.get("/api/sessions/{name}/subagents", dependencies=[Depends(require_auth)])
@app.get("/api/sessions/{name}/subagents/{agent_id}", dependencies=[Depends(require_auth)])
async def subagents_pelo_rust(request: Request, name: str, agent_id: str | None = None):
    # Subagentes são do Rust, que sabe qual conversa é a da sessão: convidado e Connect só repassam.
    from app import account_bridge
    return await account_bridge.forward_public(request, "/__hangar_server/subagents")


def _convidado(request: Request) -> bool:
    return guest_of(request) is not None


@app.get("/api/sessions/{name}/navegador", dependencies=[Depends(require_auth)])
async def navegador_da_sessao(name: str, request: Request):
    request = Request({**request.scope, "path": f"/api/sessions/{urllib.parse.quote(name, safe='')}/navegador"}, request.receive)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")
