import anyio.to_thread
import asyncio
import functools
import json
import logging
import os
import re
import threading
import time
import urllib.parse
import uuid
from datetime import datetime
from contextlib import asynccontextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Optional
from fastapi import FastAPI, Depends, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import FileResponse, JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.gzip import GZipMiddleware
from starlette.requests import ClientDisconnect
from pydantic import BaseModel, ConfigDict, Field, model_validator
from app import (atomico, diag,
                 loop_monitor, permission_mode, plugin_bridge, procinfo, quem_chama, tmux,
                 uds_messaging)
from app import internal_api, list_bridge, share_bridge
from app.auth import require_auth
from app.send_executor import send_thread as _send_thread
from app.mensagens import erro
from app import claude_customizations
from app import registry as registry_mod
from app.registry import SessionRegistry
from app.names import sanitize_session_name
from app.models import SessionInfo, CreatedSessionInfo, ChatEvent, session_key
from app.pqueue import (PromptQueue, _transcript_start_ts, committed_user_lines,
                        fila_interna_pendente, linha_mais_parecida)
from app.chain import ThenLink
from app import terminal_input
from app.terminal_input import TerminalInput, drain
from app.adapters import CLAUDE_HEADLESS, get_adapter
from app.adapters.claude_headless import sessions as headless_sessions
from app.adapters.codex import sessions as codex_sessions
from app.uploads import MAX_BYTES
from app.config import (list_config_dirs, _backend_config_base, settings,
                        automations_enabled)
from app import runtime_config
from app.guest_header import guest_of, guest_safe
from app.share_life import session_life
from app import contas, default_model, procinfo
from app.transcript import last_assistant_text
from app import archive_providers
from app.archive import (archive_cwd, archive_jsonl,
                         list_folders)
from app import codex_contas as codex_accounts
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

    # O drain do Codex e assincrono (app-server) e quem o chama sao
    # threads (Timer da confirmacao, gatilho de hook). Ver `_drenar`.
    global _loop_servidor
    _loop_servidor = asyncio.get_running_loop()
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


class _RustReply(Exception):
    """Resposta do Rust a uma escrita repassada pela ponte do Codex com terminal: volta como veio."""


async def _codex_rust_port(name: str, request: Request):
    """Escrita que entrou pelo convite ou pelo Connect num Codex com terminal do Rust: a autenticação foi a
    desta porta, e o pedido vai inteiro à ponte dele, num sentido só. A entrada e a pergunta do Claude vão do mesmo
    jeito à ponte do Claude. Fora disso (421 do Rust) não faz nada."""
    from app import runtime_coordinator
    from app.config import GUEST_PORT
    port = (request.scope.get("server") or (None, None))[1]
    if port == GUEST_PORT and port != settings.port:
        route, body = request.url.path.split("/", 4)[4] + (f"?{request.url.query}" if request.url.query else ""), await request.body()
        reply = None
        if codex_sessions.exists(name):
            reply = await asyncio.to_thread(runtime_coordinator.rust_codex_terminal, name, route, request.method, body)
        # Idem para a ponte do Claude: mesmo critério de provedor das rotas (`info.provider`).
        # O painel da fila (`queue/...`) é do Rust também: o Python só repassa.
        rota = route.split("?")[0]
        # A pergunta lateral vai sempre: a rota do Rust decide provedor e sessão.
        if reply is None and (rota == "btw" or (rota in {"input", "answer", "select", "select/submit", "recarregar"} or rota.startswith("queue/"))
                and getattr(await _cached_info(name), "provider", "claude") == "claude"):
            # O fork sem terminal tem 180 s e a reabertura espera o Claude subir por até 5 min; o prazo padrão da
            # ponte cortaria a resposta. Quem pediu a recarga vai ao diário do Rust.
            reload = rota == "recarregar"
            prazo = (200,) if rota == "btw" else (360,) if reload else ()
            who = {"headers": _who_asked(request)} if reload else {}
            reply = await asyncio.to_thread(runtime_coordinator.rust_claude, name, route, request.method, body, *prazo, **who)
        if reply:
            raise _RustReply(*reply)


def _who_asked(request: Request) -> dict:
    """Cliente, pedido e origem de quem chegou pelo Connect, para o diário do Rust."""
    origin = request.headers.get("x-forwarded-for") or (request.client.host if request.client else "")
    return {key: value for key, value in (("user-agent", request.headers.get("user-agent")),
            ("x-hangar-req", request.headers.get("x-hangar-req")), ("x-forwarded-for", origin)) if value}


app = FastAPI(title="hangar", lifespan=_lifespan)

from app.runtime_terminal import TerminalControlError


@app.exception_handler(_RustReply)
async def _rust_reply(request: Request, exc: _RustReply):
    return Response(exc.args[1], status_code=exc.args[0], media_type="application/json")


@app.exception_handler(TerminalControlError)
async def terminal_control_failed(request: Request, exc: TerminalControlError):
    code = "erro_sem_resposta" if exc.control == "answer_questions" else "erro_opcao_nao_convergiu"
    return JSONResponse(status_code=409, content={"detail":erro(code, str(exc))})


from app.runtime_coordinator import TransferInProgress


@app.exception_handler(TransferInProgress)
async def _ownership_moving(request: Request, exc: TransferInProgress):
    """Posse passando entre Python e Rust (sessão recém-criada, por exemplo): espera curta, não 500."""
    return JSONResponse(status_code=409, content={"detail":erro("session_transfer_busy", str(exc))})


@app.exception_handler(ClientDisconnect)
async def _client_gone(request: Request, exc: ClientDisconnect):
    """Quem pediu fechou antes de mandar o corpo (o Rust estoura o prazo dele e desiste): ninguém
    lê a resposta, e quem desistiu já registrou o próprio prazo."""
    return Response(status_code=499)


@app.exception_handler(tmux.MuxIndisponivel)
async def _mux_indisponivel(request: Request, exc: tmux.MuxIndisponivel):
    """503 em QUALQUER rota que esbarre num multiplexador que não responde.

    `registry.list()` levanta isto, e ele é chamado por umas quinze rotas (shell, loop, uploads,
    plano, arquivos...). Tratar rota por rota deixaria as não-tocadas devolvendo 500 com traceback
    justamente no cenário que esta mudança existe pra consertar — e a próxima rota a chamar `list()`
    nasceria com o mesmo furo. Um handler só fecha todas de uma vez.

    503 e não a lista vazia de antes: "não sei quais sessões existem" não pode continuar sendo
    entregue como "você não tem nenhuma".
    """
    diag.registrar("mux.indisponivel", "erro", detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(
        status_code=503,
        content={"detail": erro("erro_mux_indisponivel",
                                "o tmux não respondeu — a lista de sessões está indisponível",
                                detalhe=str(exc))})


@app.exception_handler(registry_mod.SessionOpenNeedsRust)
async def _abrir_precisa_do_rust(request: Request, exc: Exception):
    """Abrir Claude com terminal é do Rust: no modo `python` do Supervisor a recusa tem código e frase."""
    diag.registrar("sessao.criar_recusada", "erro", codigo=registry_mod.SessionOpenNeedsRust.code,
                   detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(status_code=503, content={"detail": erro(
        registry_mod.SessionOpenNeedsRust.code, "abrir e reabrir sessão precisam do servidor Rust, que não está rodando")})


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
terminal = TerminalInput()

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


# Snapshot com TTL de registry.list() pros endpoints request/response QUENTES (history/workflows):
# o mount do board dispara dezenas de /history de uma vez e cada list() fresco e um scan completo
# de /proc + fork de tmux list-panes.
# Miss por nome (sessao criada ha
# <1s) -> fallback pro list() fresco, entao o TTL nunca causa 404 falso.
_LIST_TTL = 1.0
# UMA chave, guardando o par (quando, lista). Guardar `t` e `infos` em chaves separadas deixava as
# duas escritas se entrelacarem entre threads — e desde que o `_cached_info` async passou a entrar
# por aqui via to_thread, sao threads de verdade, nao mais so o laco de eventos. O pior caso era
# pequeno (a lista de uma thread carimbada com o relogio da outra, dezenas de ms a mais de atraso),
# mas o par num STORE_SUBSCR so custa o mesmo e nao deixa a pergunta em aberto.
_list_snap: dict = {"snap": None}
# Single-flight: SEM ele o TTL nao segura nada sob carga. N threads que erram o cache juntas viram N
# `registry.list()` completos; com dez em paralelo brigando pelo GIL cada uma passa de ~70ms pra
# ~700ms, o TTL de 1s vence antes da leva seguinte e todas erram de novo — a avalanche se
# auto-alimenta. Medido: 10,4 threads dentro de list() em TODA amostra, backend em 1,5 core parado.
# Com o lock, a primeira varre e as outras esperam por ela; o resultado e o mesmo, o custo e 1/N.
_list_lock = threading.Lock()


def _guardar_snap(forcar: bool = False) -> list[SessionInfo]:
    inicio, inicio_epoca = time.monotonic(), time.time()
    with _list_lock:
        # Re-checa DENTRO do lock: quem ficou na fila enquanto a primeira varria ja tem snapshot
        # fresco esperando e nao precisa varrer de novo. `forcar` e o miss por nome (sessao criada
        # ha <1s): ai o que se quer e justamente uma varredura NOVA, entao so vale o snapshot que
        # nasceu DEPOIS desta chamada comecar — senao o fallback devolveria o mesmo 404.
        snap = _list_snap["snap"]
        if snap is not None and (snap[0] > inicio if forcar
                                 else time.monotonic() - snap[0] < _LIST_TTL):
            return snap[1]
        if forcar:
            # Sessao criada ha <1s: o mapa de processos cacheado ainda nao a enxerga, e a varredura
            # nova e justamente o que se quer aqui.
            procinfo._invalidar_children_map()
        infos = registry.list(newer_than=inicio_epoca) if forcar else registry.list()
        _list_snap["snap"] = (time.monotonic(), infos)
        return infos


def _invalidate_lists() -> None:
    """Descarta o snapshot cru: quem pedir /api/sessions depois de uma mudança de membro ou de modo
    recalcula em vez de ver a sessão como era."""
    with _list_lock:
        _list_snap["snap"] = None
    if registry_mod._rust_caches():
        # A mudança já aconteceu: a falha fica no diário (`lista.ponte`), o retrato do Rust vence
        # em 2 s e quem procura a sessão nova pede descoberta mais nova (`newer_than`).
        try:
            list_bridge.invalidate()
        except (list_bridge.ListBridgeError, tmux.MuxIndisponivel):
            pass


def _cached_info_sync(name: str) -> SessionInfo | None:
    """Gemeo SINCRONO do _cached_info, pro handler `def` (que o FastAPI ja roda na threadpool e
    portanto pode chamar registry.list() direto). Mesmo dicionario dos dois lados: um hit vindo de
    qualquer caminho serve o outro. Duas threads podem recarregar ao mesmo tempo — inofensivo, a
    recarga e idempotente e a ultima vence."""
    snap = _list_snap["snap"]
    infos = (snap[1] if snap is not None and time.monotonic() - snap[0] < _LIST_TTL
             else _guardar_snap())
    info = next((s for s in infos if s.name == name), None)
    if info is None:
        info = next((s for s in _guardar_snap(forcar=True) if s.name == name), None)
    return info


async def _cached_info(name: str) -> SessionInfo | None:
    return await asyncio.to_thread(_cached_info_sync, name)


def _rust_conduz(info) -> bool:
    """Sessão que o Rust de pé conduz (Claude e Codex): fila, loop, `then` e avisos dela são dele."""
    from app import runtime_coordinator
    c = runtime_coordinator.current()
    return c is not None and c.mode in {"pending", "rust"} and c.rust_owns(info.provider, bool(getattr(info, "headless", False)))


_CONFIRM_GRACE = 8.0  # s entre o send e a checagem "o transcript gravou o prompt?"
_CONFIRM_GRACE_HEADLESS = 60.0
# Cada checagem relê o transcript inteiro (MBs). Um prompt parado na fila interna da TUI durante um
# turno longo reagendava a cada `grace` pelo turno todo; espaça até este teto.
_CONFIRM_WORKING_MAX = 120.0

# Uma checagem pendente por sessão: send, fim de turno e a própria checagem agendavam cada um o seu
# Timer, e as cadeias se somavam (dezenas de Timers relendo o mesmo arquivo).
_confirm_lock = threading.Lock()
_confirm_pend: dict[str, tuple[threading.Timer, float]] = {}
_confirm_working_streak: dict[str, int] = {}


def _agendar_confirmacao(name: str, delay: float) -> None:
    """Agenda `_confirm_and_drain(name)`; se já há uma pendente que roda antes, ela basta. Uma
    pendente mais tardia é trocada: o prazo mais curto (fim de turno, send novo) não espera o espaçado."""
    due = time.monotonic() + delay
    with _confirm_lock:
        atual = _confirm_pend.get(name)
        if atual is not None:
            timer, quando = atual
            if callable(getattr(timer, "is_alive", None)) and timer.is_alive():
                if quando <= due:
                    return
                timer.cancel()
        timer = threading.Timer(delay, _confirm_and_drain, args=(name,))
        timer.daemon = True
        _confirm_pend[name] = (timer, due)
    timer.start()


# Loop do servidor, pra pontes sync->async (mesmo papel do `INBOX.ligar_loop`). Setado no lifespan.
_loop_servidor: asyncio.AbstractEventLoop | None = None


def _drenar(name: str, jsonl: str, provider: str) -> int:
    """Entrega a fila pendente pelo caminho DAQUELE provider.

    O `terminal_input.drain` digita no pane, e no Codex isso poria a mensagem do usuario duas vezes
    na conversa (a entrega de verdade e o `turn/start` do app-server). Como o adapter do Codex e
    assincrono e quem chama isto e sempre uma thread (Timer, hook, request fora do loop), a ponte e
    a mesma do `pi_inbox.entregar_sync`: agendar no loop do servidor e esperar o resultado.

    O Claude sem terminal segue o mesmo caminho: seu provider e "claude", mas nao ha pane — o
    `terminal_input.drain` reivindicava a entrada e falhava ao resolver o pane, e o prompt ficava
    pendente ate alguem abrir o chat (o drain do SSE)."""
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.managed_runtime(name):
        try:
            return run_sync(lambda: coordinator.op(name, {"kind":"drain"}, uuid.uuid4().hex), coordinator.loop)["sent"]
        except runtime_coordinator.TransferInProgress:
            return 0        # o novo dono drena quando a sessão fica entregável
    if provider == "codex":
        chave = "codex"
    elif _headless(name):
        chave = CLAUDE_HEADLESS
    elif provider == "claude" and coordinator is not None and coordinator.mode in {"pending", "rust"} and coordinator.rust_owns("claude", False):
        return 0    # a fila do Claude com terminal é do Rust: quem drena é o Monitor dele, e a trava é dele
    else:
        return drain(name, jsonl, provider)
    loop = _loop_servidor
    if loop is None:
        _log.warning("drain %s name=%s: sem loop do servidor (fila fica pendente)", chave, name)
        return 0
    fut = None
    try:
        fut = asyncio.run_coroutine_threadsafe(get_adapter(chave).drain(name, jsonl), loop)
        # Teto so pra nao pendurar a thread se o loop morrer no meio (restart): quem manda no
        # relogio e o proprio adapter, que ja tem os timeouts do app-server.
        return fut.result(120)
    except Exception:
        # `cancel()` pelo mesmo motivo do `pi_inbox.entregar_sync`: sem ele a corrotina segue viva
        # no loop e pode ENTREGAR depois de este chamador ja ter decidido "ficou pendente" — a
        # proxima rodada entrega de novo e a mesma mensagem chega duas vezes ao agente. So tem
        # efeito se ela ainda nao passou do proximo await; passou disso, a fila ja esta marcada.
        # `fut` continua None se o proprio run_coroutine_threadsafe levantar (loop fechando).
        if fut is not None:
            fut.cancel()
        # Falha VISIVEL, nunca mensagem duplicada: a entrada segue pendente e o proximo fim de
        # turno tenta de novo; a bolha "na fila" continua na tela enquanto isso.
        _log.warning("drain %s name=%s falhou (fila segue pendente)", chave, name, exc_info=True)
        return 0


def _drain_session(name: str) -> None:
    """Entrega enfileiradas pendentes desta sessao (best-effort, roda fora do request)."""
    try:
        info = _cached_info_sync(name)
        if info and info.jsonl:
            _drenar(name, info.jsonl, info.provider)
    except Exception:
        pass


def _confirm_and_drain(name: str) -> None:
    """Confirmacao de entrega: delivered=True so diz 'send_keys chamado' — a TUI pode ter engolido
    as teclas e a msg sumia com cara de entregue. Confere contra o transcript; engolida ->
    re-enfileira (reconcile) e re-drena. Best-effort, roda em Timer/thread."""
    with _confirm_lock:
        atual = _confirm_pend.get(name)
        if atual is not None and atual[0] is threading.current_thread():
            del _confirm_pend[name]
    try:
        from app import runtime_coordinator
        from app.runtime_adapter import run_sync
        coordinator = runtime_coordinator.current()
        if coordinator is not None and coordinator.managed_runtime(name):
            async def confirm():
                await coordinator.op(name, {"kind":"confirm"}, uuid.uuid4().hex)
                await coordinator.op(name, {"kind":"drain"}, uuid.uuid4().hex)
            run_sync(confirm, coordinator.loop)
            return
        q = PromptQueue(name)
        if not any(r.get("delivered") is True and not r.get("confirmed") for r in q.load()):
            return  # nada a confirmar: nao paga registry nem o scan do transcript
        info = _cached_info_sync(name)
        if not info or not info.jsonl:
            return
        # Fila e confirmação do Codex com terminal são do Rust: gravar na projeção dele seria um segundo escritor.
        if info.provider == "codex" and not info.headless:
            return
        # MID-TURN o prompt entregue ainda pode nao ter virado entrada no transcript (vive na fila
        # interna do Claude Code) — decidir requeue agora arriscaria redigitar mensagem ja recebida.
        # Adia pro proximo ciclo (o turno acabando dispara transicao -> novo timer).
        m = hook_state.get_state(session_key(info.jsonl))
        headless = _headless(name)
        grace = _CONFIRM_GRACE_HEADLESS if headless else _CONFIRM_GRACE
        # UMA leitura do oraculo pros dois ramos. None = nao deu pra ler o transcript (ver
        # committed_user_lines): sai SEM decidir e SEM reagendar. Sem reagendar de proposito — um
        # Timer a cada `grace` contra um arquivo que não abre é tempestade sem fim; a próxima
        # mensagem chama isto de novo, e até lá a entrada fica visível como bolha "na fila".
        # Falha VISÍVEL, nunca mensagem duplicada.
        # As DUAS leituras do transcript ficam juntas e falham juntas. `_transcript_start_ts` abre
        # o mesmo arquivo uma segunda vez, e o 0.0 dele desliga a poda por idade — sem ela, entrada
        # de sessao ANTERIOR nao e mais dispensada e vai parar no caminho que REDIGITA. Ou seja: o
        # mesmo defeito, pela porta do lado. Aqui "nao sei" nunca decide nada.
        if headless and not os.path.exists(info.jsonl):
            # Sem terminal, .jsonl que nunca nasceu não é "ilegível": o processo morreu antes de
            # gravar o prompt. Sem isto a entrada ficava entregue e calada pra sempre.
            committed, inicio_ts = [], 0.0
        else:
            committed = committed_user_lines(info.jsonl, info.provider)
            inicio_ts = _transcript_start_ts(info.jsonl)
        # Enfileirada na TUI e ainda nao consumida: entregue, mas sem bolha real — segue visivel
        # como bolha da fila em vez de ser confirmada (escondida) pela linha de enqueue.
        na_fila = fila_interna_pendente(info.jsonl, info.provider)
        if committed is None or inicio_ts is None:
            _log.warning("confirmacao adiada name=%s: transcript ilegivel agora (nada foi "
                         "reenfileirado nem dado por perdido)", name)
            return
        if info.provider == "codex" or m and m[0] == "working":
            # RPC do Codex pode ter chegado sem resposta: só o transcript autoriza confirmar.
            # Turno vivo: REDIGITAR e DESISTIR no meio do turno sao perigosos (o texto pode ainda
            # estar na fila interna da TUI — desistiu viraria aviso falso de "nao chegou" sobre
            # msg que chega depois). CONFIRMAR nao: o transcript e a fonte de verdade, e texto
            # comprovadamente la = a bolha real ja cobre, o eco da fila so duplica. Sem isto, uma
            # sessao que trabalha HORAS sem ficar ociosa nunca confirmava e o follow reemitia a
            # fila inteira como bolha fantasma a cada reconexao do SSE. `confirm_only` carimba so
            # o provado e deixa o resto pra proxima checagem (reagendada la embaixo).
            q.reconcile_delivered(
                committed, inicio_ts,
                time.time(),
                grace=grace,
                confirm_only=True,
                na_fila_tui=na_fila,
            )
        else:
            max_attempts = 0 if (m is None or headless) else 2
            requeued = q.reconcile_delivered(
                committed, inicio_ts,
                time.time(),
                grace=grace,
                max_attempts=max_attempts,
                na_fila_tui=na_fila,
            )
            if requeued:
                # Log com o TEXTO e com a linha mais parecida do transcript. `REQUEUE name=X n=1`
                # sozinho nao diz nada: o oraculo e comparacao de string, entao o que resolve o
                # caso e o DIFF (um espaco a mais, uma barra invertida comida pelo multiplexador,
                # um prefixo prependado pelo harness). E redigitar e a acao destrutiva daqui —
                # quando ela sai errada, o usuario ve a propria mensagem entrar 3x na conversa,
                # e sem estas duas linhas so restava reler o codigo.
                for r in requeued:
                    txt = str(r.get("text") or "").strip()
                    _log.info("REQUEUE name=%s id=%s tentativa=%s texto=%r | mais parecida no "
                              "transcript=%r", name, r.get("id"), r.get("attempts"), txt[:200],
                              (linha_mais_parecida(txt, committed) or "")[:200])
                _log.info("REQUEUE name=%s n=%d (TUI engoliu o send; re-drenando)", name, len(requeued))
                _drenar(name, info.jsonl, info.provider)
        working = bool(m and m[0] == "working")
        streak = _confirm_working_streak.get(name, 0) + 1 if working else 0
        _confirm_working_streak[name] = streak
        if any(r.get("delivered") is True and not r.get("confirmed") and not r.get("desistiu")
               for r in q.load()):
            delay = grace + 0.5
            if streak > 1:
                delay = max(delay, min(delay * 2 ** (streak - 1), _CONFIRM_WORKING_MAX))
            _agendar_confirmacao(name, delay)
    except Exception:
        # LOGA, nao `pass` mudo: isto roda num Timer, entao ninguem ve a excecao — e o que mora
        # aqui e a confirmacao de entrega. Falhando calado, a msg do usuario fica sem confirmar pra
        # sempre e nao ha onde olhar. Best-effort segue (o proximo idle tenta de novo).
        _log.warning("confirmacao de entrega falhou name=%s", name, exc_info=True)


# Sem terminal, quem entrega a fila é o drain do adapter (fim de turno, initialize), fora do /input:
# sem este gatilho nenhuma confirmação era agendada e a entrega que morreu com o processo sumia.
get_adapter(CLAUDE_HEADLESS).apos_entrega = (
    lambda name: _agendar_confirmacao(name, _CONFIRM_GRACE_HEADLESS + 0.5))


def _maybe_chain(name: str) -> None:
    """Encadeamento de sessao (feature #12): `name` acabou de confirmar idle no MESMO ponto do push de
    'terminou' (state == 'idle' em _on_hook_transition — so vira idle no hook Stop, entao ja e turno
    REALMENTE terminado, nao redraw). Se ha um vinculo 'then' armado, enfileira o prompt na sessao ALVO
    e dispara o drain dela (mesmo mecanismo do /input), depois consome o vinculo (one-shot: nao e DAG,
    so 1 hop -- sem isto o alvo levaria o MESMO prompt de novo no proximo turno da fonte).
    Kill-switch mestre no topo (app.config.automations_enabled) -- desliga esta e a auto-resume junto."""
    if not automations_enabled():
        return
    link = ThenLink(name)
    data = link.get()
    if not data:
        return
    target, text = data.get("target"), data.get("text")
    try:
        if target and text:
            PromptQueue(target).append(text, delivered=False)
            _drain_session(target)
    finally:
        link.clear()  # one-shot sempre, mesmo se target/text vier malformado -> nao fica repetindo lixo


class _StrictBody(BaseModel):
    # rejeita campos desconhecidos no corpo (contrato estrito; pega typo de campo do cliente -> 422).
    model_config = ConfigDict(extra="forbid")


class InputBody(_StrictBody):
    text: str
    # Recados 1:1 pedem orientação imediata; envios comuns conservam a fila normal.
    steer: bool = False


class BroadcastBody(_StrictBody):
    names: list[str] = Field(min_length=1)
    text: str


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


def _codex_as_claude_mode(name: str) -> str | None:
    """Modo do Claude equivalente ao da sessão Codex `name` (None se não for Codex)."""
    from app.adapters.codex import sessions as codex_sessions
    from app.adapters.codex.lancador import PERMISSION_POLICIES
    policy = PERMISSION_POLICIES.get(str((codex_sessions.load(name) or {}).get("permission_mode")))
    if policy is None:
        return None
    approval, sandbox = policy
    if approval == "never":
        return "bypassPermissions"
    return "acceptEdits" if sandbox == "workspace-write" else "manual"


@app.post("/api/sessions", dependencies=[Depends(require_auth)], response_model=CreatedSessionInfo)
async def create_session(request: Request) -> CreatedSessionInfo:
    raise registry_mod.SessionOpenNeedsRust()


class ModoExecucaoBody(_StrictBody):
    terminal: bool = Field(strict=True)


@app.post("/api/sessions/{name}/recarregar", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def recarregar_sessao(name: str):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider in ("claude", "codex"):
        raise registry_mod.SessionOpenNeedsRust()
    raise HTTPException(409, detail=erro("erro_recarregar_so_sem_terminal",
                                         "recarregar só vale para sessão Claude sem terminal"))


@app.post("/api/sessions/{name}/modo-execucao", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def modo_execucao(name: str, body: ModoExecucaoBody):
    """Trocar entre terminal e sem terminal é do Rust; aqui só as recusas sem ele."""
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider in ("claude", "codex"):
        raise registry_mod.SessionOpenNeedsRust()
    raise HTTPException(409, detail=erro("erro_modo_so_claude", "a troca de modo só vale para sessões Claude"))


@app.get("/api/sessions/{name}/conta", dependencies=[Depends(require_auth)])
async def contas_destino(name: str):
    """As contas de destino e a troca de conta são do Rust."""
    raise registry_mod.SessionOpenNeedsRust()


@app.post("/api/sessions/{name}/rename")
async def rename_session(name: str, request: Request):
    """Trocar o nome é do Rust; aqui chega só quem entrou por uma porta do Python (Connect, convidado
    com conta), e segue pela ponte."""
    return await _repasse_ao_rust(request, f"/__hangar_server/claude/{urllib.parse.quote(name, safe='')}/rename")


class ThenLinkBody(_StrictBody):
    target: str = Field(min_length=1)
    text: str = Field(min_length=1)


@app.put("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def set_then_link(name: str, body: ThenLinkBody):
    """Arma o vinculo 'then' (feature #12): quando `name` confirmar idle (turno terminado), `body.text`
    e enviado pra `body.target` -- ver app.chain.ThenLink e app.api._maybe_chain. Um hop so (nao DAG):
    setar de novo so troca alvo/texto, nao encadeia mais niveis."""
    if body.target == name:
        raise HTTPException(400, detail=erro("erro_encadeamento_proprio", "sessao nao pode encadear pra si mesma"))
    if not _session_exists(body.target):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao alvo nao encontrada"))
    ThenLink(name).set(body.target, body.text)
    return {"ok": True}


@app.delete("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def clear_then_link(name: str):
    ThenLink(name).clear()
    return {"ok": True}


@app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
async def history(request: Request, response: Response, name: str, limit: int | None = None):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.pqueue import historico_etag, merged_history
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.managed_queue(name):
        try:
            await coordinator.op(name, {"kind":"ensure_projection"}, uuid.uuid4().hex)
        except runtime_coordinator.TransferInProgress:
            # Na passagem ninguém grava: a projeção em disco é a última dos dois donos.
            diag.registrar("runtime.info_during_transfer", "aviso", sessao=name)
        except Exception as exc:
            from app.runtime_coordinator import failure_reason
            diag.registrar("runtime.history_failed", "erro", sessao=name, **failure_reason(exc))
            raise HTTPException(503, detail=erro("erro_envio_falhou", "projeção da fila indisponível; tente novamente")) from None
    # Entrar numa sessao e a leitura mais repetida do app, e quase sempre nada mudou desde a
    # ultima: medido em 06/09/2026 na `pr-junior` (transcript de 31,9 MB), a cauda custava 313 KB
    # POR ENTRADA pelo caminho do celular. O validador sai de dois `stat` -- barato aqui e, do lado
    # do cliente, dispensa qualquer regra de "quando invalidar o cache": quem responde e o disco,
    # entao msg deste aparelho, de outro, do terminal, /clear e sessao que continuou trabalhando
    # caem todos no mesmo caminho.
    try:
        etag = await asyncio.to_thread(historico_etag, name, info.jsonl, info.provider, limit)
    except OSError as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    if etag:
        if request.headers.get("if-none-match") == etag:
            return Response(status_code=304, headers={"ETag": etag})
        response.headers["ETag"] = etag
    # provider: o rollout do Codex tem um shape DIFERENTE do jsonl do Claude (ver
    # app.adapters.codex.rollout) -- sem isto merged_history tentava o parser do Claude em toda
    # linha do rollout, nunca casava e devolvia [] (chat do Codex abria vazio ate o SSE encher via
    # backfill do tail; reabrir apos ficar horas em segundo plano perdia o que passou do tail-200).
    # Com limit, merged_history faz tail-read (parseia so o fim do arquivo); to_thread porque o
    # parse (mesmo da cauda) e CPU/IO sincrono.
    try:
        evs = await asyncio.to_thread(merged_history, name, info.jsonl, info.provider, limit)
    except OSError as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    # Cauda CRUA de proposito: o corte no 1o
    # user_msg (pra nao desenhar resposta orfa) e preferencia de RENDERIZACAO do card do quadro e vive
    # no BoardCard.svelte. Aplicado AQUI, valia pra todo consumidor e matava a espiada do hover da
    # Sidebar (HP_TAIL=8), que so quer o ultimo assistant_msg: com o proximo prompt ja mandado, o corte
    # jogava fora a resposta anterior -> latestAssistantEvent = None -> popover vazio, cacheado por 30s.
    if limit is not None and limit > 0:
        return evs[-limit:]
    return evs


@app.get("/api/sessions/{name}/peer-address", dependencies=[Depends(require_auth)])
async def peer_address(name: str):
    """Endereço do inbox nativo desta sessão (cross-session messaging), ou `null`.

    Existe pro `hangar-send` decidir com FATO se o caminho nativo alcança este alvo, em vez de supor
    pelo tipo da sessão: quem não tem socket (sessão aberta antes da liberação da Anthropic, Codex,
    Pi) não aparece no `ListAgents` de ninguém, e mandar o modelo usar `SendMessage` ali seria
    mandá-lo bater numa porta que não existe. `null` nunca é erro — é a resposta "aqui não tem".
    """
    # `registry` aqui é a INSTÂNCIA (SessionRegistry); inbox_socket_of é função de MÓDULO.
    from app.registry import inbox_socket_of
    return {"uds": await asyncio.to_thread(inbox_socket_of, name)}


@app.get("/api/sessions/{name}/subagents", dependencies=[Depends(require_auth)])
@app.get("/api/sessions/{name}/subagents/{agent_id}", dependencies=[Depends(require_auth)])
async def subagents_pelo_rust(request: Request, name: str, agent_id: str | None = None):
    # Subagentes são do Rust, que sabe qual conversa é a da sessão: convidado e Connect só repassam.
    from app import account_bridge
    return await account_bridge.forward_public(request, "/__hangar_server/subagents")


def _erro_texto(e) -> str:
    """Texto de um erro de envio: string crua (endpoint antigo) ou o `msg` do envelope {code, params, msg}.

    Os avisos compostos (pareamento, group-message) interpolam o TEXTO, nunca o dict — o dict seria
    "[object Object]" no front antigo. O front novo recebe a estrutura via params e traduz por ela;
    o msg montado aqui e a rede para quem nao tem o codigo no mapa.
    """
    return e if isinstance(e, str) else (e.get("msg") if isinstance(e, dict) else str(e))



def _sessao_claude_de(name: str) -> tuple[Optional[str], Optional[str]]:
    """(session_id, config_dir) da sessão Claude `name`, com ou sem terminal; (None, None) se não der."""
    meta = headless_sessions.load(name)
    if meta:
        return meta.get("session_id"), meta.get("config_dir")
    from app import tmux as _tmux
    cwd = next((p["cwd"] for p in _tmux.list_panes_active() if p["name"] == name), "")
    jsonl, _ = registry.resolve_tracked(name, cwd)
    if not jsonl:
        return None, None
    p = Path(jsonl)
    return p.stem, str(p.parent.parent.parent)


def _classe_modo(remetente: str, alvo: str, cfg_alvo: Optional[str],
                 jsonl_alvo: Optional[str] = None) -> str:
    """`from-mode` do envelope nativo: a classe (bypass/prompting) do REMETENTE quando conhecida;
    senão a do alvo, que é o que o caminho pelo tmux sempre fez (sem checagem nenhuma). O modo da
    conta é o último recurso: errado, ele faz o receptor reter o recado (decisoes/plataforma.md)."""
    def _modo(n: str, jsonl: Optional[str] = None) -> Optional[str]:
        m = headless_sessions.load(n)
        if m and m.get("permission_mode"):
            return str(m["permission_mode"])
        if (codex := _codex_as_claude_mode(n)) is not None:
            return codex
        return permission_mode.session_non_plan_mode(jsonl or _jsonl_atual(n))
    # Rótulo de aviso do app ([painel: …]) não é sessão; resolvê-lo só gastaria chamadas ao tmux.
    modo_remetente = _modo(remetente) if sanitize_session_name(remetente) == remetente else None
    modo = modo_remetente or _modo(alvo, jsonl_alvo)
    if not modo:
        modo = permission_mode.modo_da_conta(cfg_alvo)
        # Errado, faz o receptor reter o recado: precisa aparecer.
        _log.info("from-mode de %s para %s sem modo da sessão; vale o da conta (%s)", remetente, alvo, modo)
    return "bypass" if "bypass" in modo.lower() else "prompting"


def _enviar_nativo(name: str, text: str) -> Optional[str]:
    with terminal_input._send_lock(name):
        return _send_native_available(name, text)


def _send_native_available(name: str, text: str) -> Optional[str]:
    """msg_id se o recado foi ESCRITO no socket nativo do Claude de `name`; None = não é recado
    ([de:/grupo:/painel:]), sessão sem socket, ou o socket não aceitou — quem chama cai pro
    próximo degrau (plugin, tmux, fila). Só recado vai por aqui: a fala da pessoa continua
    entrando como prompt dela, nunca embrulhada como mensagem de outra sessão."""
    remetente, _ = uds_messaging.separar_prefixo(text)
    if remetente is None:
        return None
    try:
        sid, cfg = _sessao_claude_de(name)
        sock = uds_messaging.socket_da_sessao(sid, cfg) if sid else None
        if not sock:
            return None
        mid = uds_messaging.enviar(sock, text, remetente, _classe_modo(remetente, name, cfg))
    except Exception:                                # noqa: BLE001
        _log.warning("socket nativo falhou name=%s; caindo pro caminho de sempre", name, exc_info=True)
        return None
    return mid


def _jsonl_atual(name: str) -> str | None:
    """O transcript para onde o nome resolve AGORA (não o do cache da lista: depois de um `/clear`
    a prova leria o transcript velho e autorizaria digitar de novo)."""
    try:
        from app import tmux as _tmux
        _cwd = next((p["cwd"] for p in _tmux.list_panes_active() if p["name"] == name), "")
        return registry.resolve_tracked(name, _cwd)[0] or None
    except Exception:
        _log.exception("jsonl da sessão %s não resolvido", name)
        return None


def _rust_mode(coordinator) -> bool:
    return coordinator is not None and getattr(coordinator, "mode", None) == "rust"


def _no_rust_binding_error() -> dict:
    # Com o Rust de pé a entrega é dele; cair no socket/plugin/tmux do Python escreveria sem a porta.
    return {"ok": False, "error": erro("erro_envio_falhou", "sessão Claude sem vínculo no Rust"), "delivered": False}


def _send_one(name: str, text: str, track_entry: bool = False) -> dict:

    """Sequencia UNICA de envio de prompt: send_prompt + registro na fila duravel + confirmacao/drain.
    Usada pelo /input (uma sessao) e pelo /broadcast (loop por N sessoes) — o broadcast NAO reimplementa
    entrega, so repete esta mesma sequencia por nome. Nunca levanta (devolve ok/error) pra o broadcast
    reportar falha de uma sessao sem abortar as demais."""
    # ts da entrada carimbado ANTES do send: o send_prompt digita + Enter e o Claude Code grava o
    # prompt no transcript NA HORA, entao o append la embaixo roda DEPOIS do commit. Carimbar no
    # append punha a entrada ~ms apos o proprio commit e o dedup ts-aware do merged_history a
    # mantinha pendente (msg em dobro no historico ate o reconcile). A ordem send->append->drain
    # NAO muda — so o valor gravado, que e o unico dado que o dedup le.
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    coordinator = runtime_coordinator.current()
    # Claude com terminal, com o Rust de pé: o Rust o adota e entrega, numa fila só (um sentido).
    if _rust_mode(coordinator) and not _headless(name) and _provider_of(name) == "claude" and (
            rust := runtime_coordinator.rust_claude(name, "input", "POST", json.dumps({"text": text, "steer": False}).encode())):
        reply = json.loads(rust[1] or b"{}")
        return reply if rust[0] < 300 else {"ok": False, "error": reply.get("detail"), "delivered": False}
    if coordinator is not None and coordinator.managed_runtime(name):
        return run_sync(lambda: _send_managed(name, text, coordinator.slot(name).binding.provider,
            track_entry=track_entry), coordinator.loop)
    t0 = time.time()
    provider, pane_id = _pane_info(name)
    if coordinator is not None and provider == "claude" and coordinator.legacy is not None:
        managed = run_sync(lambda: _send_managed(name, text, provider, track_entry=track_entry), coordinator.loop)
        if managed is not None:
            return managed
        if _rust_mode(coordinator):
            return _no_rust_binding_error()
    stripped = text.lstrip()
    entry = None
    if hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"):
        del terminal_input._ULTIMA_LIMPEZA.limpou
    # Escada de entrega, um degrau só depois que o anterior não deu: socket nativo do Claude
    # (recado de sessão-irmã, entra no meio do turno) → tmux → fila.
    nativo = _enviar_nativo(name, text) if provider == "claude" and not stripped.startswith("/") else None
    if nativo:
        _log.info("SEND name=%s pelo socket nativo msg_id=%s text=%r", name, nativo, text[:80])
    try:
        result = "sent" if nativo else terminal.send_prompt(
            name, text, provider, pane_id=pane_id)
        # DIAG: correlaciona o send com o jsonl pra onde ESTE nome resolve AGORA -> pega o cross-wire
        # (msg indo pro transcript/terminal errado). Best-effort, nunca quebra o envio.
        try:
            from app import tmux as _tmux
            _cwd = next((p["cwd"] for p in _tmux.list_panes_active() if p["name"] == name), "")
            _j, _t = registry.resolve_tracked(name, _cwd)
            _log.info("SEND name=%s -> jsonl=%s tracked=%s result=%s text=%r",
                      name, (_j or "").rsplit("/", 1)[-1], _t, result, text[:80])
        except Exception:
            pass
    except ValueError as e:
        # send_prompt rejeita control chars (ex: '\n'). Sem isto virava 500 -> a msg sumia sem
        # feedback. Agora vira 400 com envelope (o frontend traduz o prefixo e mostra a causa em
        # params.erro). (Multi-linha de verdade: backlog.)
        return {"ok": False, "error": erro("erro_envio_falhou",
                                           f"falha ao enviar: {e}", erro=str(e))}
    if result == "partial":
        # Entrega PARCIAL no fatiamento do Windows: parte do texto ficou no composer e o Enter NAO foi
        # enviado (ver terminal_input.send_prompt). Reporta erro em vez de seguir pro caminho de
        # sucesso, que gravaria a entrada na fila como delivered e afirmaria entrega de uma mensagem
        # cortada. Sem entrada na fila, o drain nao reentra digitando em cima do residuo.
        #
        # A mensagem pro usuario depende do que _partial() conseguiu fazer no composer (mesma thread,
        # lida logo apos o send_prompt acima que a escreveu): o conserto de 07/08/2026 LIMPA o
        # composer antes de devolver "partial", entao a mensagem antiga ("confira o terminal, o
        # residuo esta a vista") ficou FALSA no caso comum — quem abre o terminal depois de uma
        # limpeza confirmada nao acha nada. Le e ja APAGA a flag: e o mesmo apagar que fecha dois
        # achados menores — o valor nao pode ficar escrito pra sempre, e sem apagar aqui o
        # threading.local reusado pelo pool vazaria esta leitura pro proximo envio desta thread que
        # tambem cair em "partial".
        limpou = getattr(terminal_input._ULTIMA_LIMPEZA, "limpou", False)
        if hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"):
            del terminal_input._ULTIMA_LIMPEZA.limpou
        if limpou:
            return {"ok": False, "error": erro("erro_envio_incompleto_limpo",
                                               "envio incompleto: o composer foi limpo e a mensagem NAO foi enviada — pode "
                                               "reenviar sem risco de duplicar.")}
        return {"ok": False, "error": erro("erro_envio_incompleto_composer",
                                           "envio incompleto: parte do texto ficou no composer da sessao e nada foi "
                                           "submetido. Confira o terminal antes de reenviar.")}
    if stripped.startswith("/"):
        # Slash-commands NAO entram na fila — sao meta, nao viram bubble. Excecao /clear: ele reinicia
        # a sessao do Claude Code (novo session-id/transcript), mas a fila e keyed pelo NOME da sessao
        # e sobreviveria -> entradas velhas nunca casariam com o transcript novo e virariam fantasma.
        # Zera a fila junto do /clear pra ela seguir o ciclo da sessao.
        if stripped[1:].split(maxsplit=1)[:1] == ["clear"]:
            try:
                PromptQueue(name).clear()
            except OSError:
                pass
    else:
        # Registra na fila duravel (sidecar) sempre — aparece como user_msg em ordem e persiste no
        # reload; o merge dedup-a contra o transcript quando o Claude Code grava o prompt. delivered =
        # o send_prompt REALMENTE digitou ("sent"); pane em overlay -> "deferred" (nao tocou a TUI) e a
        # entrada fica pendente pro drain entregar quando o overlay fechar. Falha ao gravar a fila nao
        # quebra o envio.
        try:
            entry = PromptQueue(name).append(text, delivered=(result == "sent"), ts=t0)
        except OSError as e:
            if result != "sent":
                # NAO digitado na TUI (overlay/picker aberto) + sidecar nao gravou = a msg nao esta em
                # lugar NENHUM. Era aqui que o "ok, na fila" mentia: 200 + delivered=False pra uma msg
                # que sumiu. Vira erro (o front mostra), nunca sucesso.
                _log.exception("fila indisponivel e prompt NAO digitado name=%s", name)
                return {"ok": False, "error": erro("erro_fila_nao_digitada",
                                                           f"fila indisponivel e prompt nao foi digitado: {e}",
                                                           erro=str(e))}
            # Digitado na TUI: a msg CHEGOU, o envio nao falhou. Perder o registro so desliga a rede de
            # seguranca (o _confirm_and_drain abaixo nao vai achar o que reconferir) — nao e motivo pra
            # falhar o envio, mas nao pode passar calado.
            _log.exception("append na fila falhou (prompt ja digitado) name=%s", name)
        if result == "sent":
            # Confirmacao de entrega: em ~8s confere se o transcript gravou; engolida -> re-drena.
            _agendar_confirmacao(name, _CONFIRM_GRACE + 0.5)
        else:
            # Kick: fecha a corrida append-depois-da-transicao — se o estado virou entregavel entre
            # o "deferred" do send_prompt e o append acima, o gatilho daquele ciclo nao viu esta
            # entrada (e sem SSE aberto nao havia gatilho nenhum). O drain re-checa deliverable.
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
    # delivered: digitou AGORA na TUI ("sent"); False = ficou na fila durável (sessão ocupada/overlay).
    return {"ok": True, "error": None, "delivered": result == "sent", "native": bool(nativo),
            **({"entry_id": entry["id"]} if track_entry and entry is not None else {})}


def _provider_of(name: str) -> str:
    """Resolve o provider de uma sessao PELO NOME, barato e sem tmux: sidecar Codex existe -> "codex",
    senao "claude". Default "claude" preserva 100% o caminho tmux de hoje pra qualquer nome que nao seja
    de uma sessao Codex conhecida (regra de ouro: Claude identico, tudo Codex e ramo condicional)."""
    return "codex" if codex_sessions.exists(name) else "claude"


def _pane_info(name: str) -> tuple[str, str | None]:
    from app import agentpane
    return agentpane.pane_info(name)


async def _send_one_codex(name: str, text: str, *, track_entry: bool = False) -> dict:
    # Codex com terminal do Rust (broadcast, grupo, par, MCP): o envio vai à ponte dele, num sentido só.
    from app import runtime_coordinator
    # A checagem de posse é memória: só a ponte (rede) vai a uma thread, e sem o Rust dono a entrega não
    # depende do executor padrão.
    if runtime_coordinator.rust_owns_codex_terminal() and (rust := await asyncio.to_thread(
            runtime_coordinator.rust_codex_terminal, name, "input", "POST", json.dumps({"text": text}).encode())):
        reply = json.loads(rust[1] or b"{}")
        return reply if rust[0] < 300 else {"ok": False, "error": reply.get("detail"), "delivered": False}
    managed = await _send_managed(name, text, "codex", track_entry=track_entry)
    if managed is not None:
        return managed
    source = await _send_thread(codex_sessions.load, name)
    async with get_adapter("codex").delivery_lock(name):
        current = await _send_thread(codex_sessions.load, name)
        changed = source is not None and (current or {}).get("thread_id") != source.get("thread_id")
        if changed or not await _send_thread(_session_exists, name):
            return {"ok": False, "error": erro("erro_sessao_inexistente", "sessao nao encontrada")}
        return await _send_one_codex_locked(name, text, track_entry=track_entry)


async def _enviar(name: str, text: str) -> dict:
    operation = asyncio.create_task(_send_available(name, text))
    try:
        return await asyncio.shield(operation)
    except asyncio.CancelledError:
        try:
            await operation
        finally:
            raise


async def _send_available(name: str, text: str) -> dict:
    """Envio comum ramificado por transporte (Codex, Claude sem terminal, pane) — a mesma esteira
    do /input pra quem manda por fora dele (broadcast, grupo, par, orquestração). Nunca levanta."""
    if _provider_of(name) == "codex":
        return await _send_one_codex(name, text)
    if _headless(name):
        return await _send_one_headless(name, text)
    return await _send_thread(_send_one, name, text)


async def _send_one_headless(name: str, text: str, *, track_entry: bool = False) -> dict:
    """Mesmo caminho de fila do Codex (adapter em vez de tty), com o adapter do Claude sem terminal."""
    managed = await _send_managed(name, text, "claude", track_entry=track_entry)
    if managed is not None:
        return managed
    from app import runtime_coordinator
    if _rust_mode(runtime_coordinator.current()):
        return _no_rust_binding_error()
    adapter = get_adapter(CLAUDE_HEADLESS)
    async with adapter.delivery_lock(name):
        if not await asyncio.to_thread(_session_exists, name):
            return {"ok": False, "error": erro("erro_sessao_inexistente", "sessao nao encontrada")}
        # Recado de sessão-irmã vai pelo socket nativo do `claude` filho do cano quando ele existe
        # (entra no meio do turno); a fila registra como entregue pra bolha e dedup.
        if not text.lstrip().startswith("/") and (mid := await asyncio.to_thread(_enviar_nativo, name, text)):
            _log.info("SEND name=%s pelo socket nativo (sem terminal) msg_id=%s text=%r", name, mid, text[:80])
            try:
                q = PromptQueue(name)
                entry = await _send_thread(q.append, text, delivered=True)
                # Confirmada na hora: o CLI grava o recado no transcript assim que lê o socket, e
                # sem o carimbo a entrada da fila ficava em dobro com a linha do transcript até o
                # reconcile do idle (medido: bolha duplicada na sessão sem terminal).
                await _send_thread(q.confirm_delivered, lambda r: r.get("id") == entry["id"])
            except OSError:
                _log.exception("append na fila falhou (recado já no socket) name=%s", name)
                return {"ok": True, "error": None, "delivered": True, "native": True}
            return {"ok": True, "error": None, "delivered": True, "native": True, "entry_id": entry["id"]}
        res = await _send_one_codex_locked(name, text, track_entry=track_entry, chave=CLAUDE_HEADLESS)
    if res.get("ok") and not res.get("delivered"):
        # Parada: o prompt já está na fila e a resposta sai agora; a sessão sobe e entrega depois.
        adapter.acordar(name)
    return res


async def _send_managed(name: str, text: str, provider: str, *, track_entry: bool = False) -> dict | None:
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None:
        return None
    operation_id = uuid.uuid4().hex
    try:
        if not await coordinator.prepare_session(name, provider, launch=True):
            return None
        if provider == "codex" and text.strip().split(maxsplit=1)[0:1] == ["/compact"]:
            if text.strip() != "/compact":
                raise ValueError("O /compact do Codex não aceita argumentos.")
            command = {"kind":"control", "control":"compact", "payload":{}}
        else:
            command = {"kind":"submit", "text":text}
        reply = await coordinator.op(name, command, operation_id)
        disposition = reply.get("disposition")
        queued = command["kind"] == "submit" and not text.lstrip().startswith("/")
        # Com terminal, a entrega incerta é confirmada depois pelo transcript (uma vez, sem reenvio).
        proved_later = (disposition == "unknown" and queued
                        and isinstance(coordinator.slot(name).binding.meta.get("terminal"), dict))
        if disposition == "unknown" and (proved_later or (reply.get("payload") or {}).get("transport_lost") is True):
            # Entrega sem prova (aviso do plugin atrasado, ou o Rust caiu no meio): a mensagem está na
            # fila durável, que só a confirma pelo transcript e nunca a reenvia; a bolha espera.
            diag.registrar("runtime.send_uncertain", "aviso", sessao=name,
                           codigo=str((reply.get("payload") or {}).get("code")
                                      or ("terminal_delivery_unknown" if proved_later else "transport_lost")))
            return {"ok":True, "error":None, "delivered":False, "uncertain":True,
                **({"entry_id":operation_id} if track_entry and queued else {})}
        if disposition not in {"accepted", "deferred"}:
            raise RuntimeError("resultado incerto; entrada conservada sem reenvio" if disposition == "unknown" else "entrada recusada pelo runtime")
        if (disposition == "deferred" and command["kind"] == "submit" and not queued and provider == "claude"
                and isinstance(coordinator.slot(name).binding.meta.get("terminal"), dict)):
            # Comando de barra não tem linha na fila: adiado, ele não roda depois sozinho.
            motivo = str((reply.get("payload") or {}).get("code") or "deferred")
            comando = text.split()[0]
            diag.registrar("runtime.command_deferred", "aviso", sessao=name, codigo=motivo[:60])
            return {"ok":False, "error":erro("erro_comando_nao_executado",
                f"{comando} não foi executado: o terminal não aceitou agora ({motivo}). Mande de novo.",
                comando=comando, motivo=motivo)}
        return {"ok":True, "error":None, "delivered":disposition == "accepted",
            **({"native":True} if (reply.get("payload") or {}).get("native") is True else {}),
            **({"entry_id":operation_id} if track_entry and command["kind"] == "submit" and not text.lstrip().startswith("/") else {})}
    except Exception as exc:
        from app.runtime_coordinator import failure_reason
        diag.registrar("runtime.send_failed", "erro", sessao=name, **failure_reason(exc))
        return {"ok":False, "error":erro("erro_envio_falhou", str(exc), erro=str(exc)),
            **({"entry_id":operation_id} if track_entry else {})}


async def _send_one_codex_locked(name: str, text: str, *, track_entry: bool = False,
                                 chave: str = "codex") -> dict:
    """Envio de prompt pra sessao Codex pela TUI no tmux. Registra na fila duravel
    (aparece como user_msg em ordem e persiste no reload; o
    merge dedup-a contra o rollout do Codex) e entrega pela TUI no tmux SE a sessao esta idle;
    senao deixa pendente pro drain-on-complete entregar quando o turno terminar. Codex nao tem
    slash-commands do Claude -> envia o texto como esta. Nunca levanta (mesmo contrato do _send_one pro
    broadcast: devolve ok/error por sessao).

    IMPORTANT 2: PromptQueue.append/set_delivered fazem I/O de arquivo sincrono com lock -- chamados
    direto aqui (corrotina) bloqueariam o event loop. O pool de envio evita disputar com funcionalidades secundárias."""
    adapter = get_adapter(chave)
    if chave == "codex" and text.strip().split(maxsplit=1)[0:1] == ["/compact"]:
        try:
            if text.strip() != "/compact":
                raise ValueError("O /compact do Codex não aceita argumentos.")
            await adapter.compact(name)
        except Exception as exc:
            _log.warning("codex: compactação recusada name=%s: %s", name, exc)
            return {"ok": False, "error": erro("erro_envio_falhou", str(exc), erro=str(exc))}
        return {"ok": True, "error": None, "delivered": True}
    try:
        deliverable = await adapter.deliverable(name)
    except Exception:
        # Adapter quebrado/fora do ar nao pode passar calado: sem o log, um erro aqui virava um
        # "delivered: false" indistinguivel de turno em andamento. Segue como NAO-entregavel -> a
        # fila abaixo segura o prompt e o drain-on-complete tenta de novo no proximo idle.
        _log.exception("codex deliverable falhou name=%s", name)
        deliverable = False
    # A reserva precede o RPC; confirmação de entrega vem do transcript.
    try:
        entry = await _send_thread(PromptQueue(name).append, text, delivered=False)
    except OSError as e:
        # Mesma regra do _send_one: sidecar nao gravou + NAO entregavel = a msg nao esta em lugar
        # NENHUM, e responder "ok, na fila" era a mentira que o eeba30a tirou do caminho Claude.
        # Vira erro (o front mostra), nunca sucesso.
        if not deliverable:
            _log.exception("fila indisponivel e prompt NAO entregue name=%s", name)
            return {"ok": False, "error": erro("erro_fila_nao_entregue",
                                                       f"fila indisponivel e prompt nao foi entregue: {e}",
                                                       erro=str(e))}
        # Entregavel: a TUI abaixo ainda leva o texto, entao a msg CHEGA. Perder o registro so
        # desliga a rede de seguranca (o drain-on-complete nao acha o que reconferir) — nao e motivo
        # pra falhar o envio, mas nao pode passar calado.
        _log.exception("append na fila falhou (prompt sera entregue) name=%s", name)
        entry = None
    if not deliverable:
        # turno em andamento -> fica pendente na fila; o drain-on-complete entrega no proximo idle.
        return {"ok": True, "error": None, "delivered": False,
                **({"entry_id": entry["id"]} if track_entry else {})}
    if entry is not None:
        try:
            claimed = await _send_thread(PromptQueue(name).claim_undelivered, entry_id=entry["id"])
        except OSError as exc:
            return {"ok": False, "error": erro("erro_fila_nao_entregue", str(exc), erro=str(exc))}
        if not claimed:
            return {"ok": True, "error": None, "delivered": bool(
                await _send_thread(PromptQueue(name).entry_delivered, entry["id"])),
                **({"entry_id": entry["id"]} if track_entry else {})}
    try:
        result = await adapter.send_prompt(name, text)
    except Exception as e:
        if entry is not None:
            try:
                await _send_thread(PromptQueue(name).set_delivered, entry["id"], False)
            except OSError:
                _log.exception("Não foi possível liberar a entrada sem envio name=%s", name)
        _log.exception("codex send_prompt falhou name=%s", name)
        return {"ok": False, "error": erro("erro_envio_falhou",
                                           f"falha ao enviar: {e}", erro=str(e))}
    if result == "unknown":
        return {"ok": True, "error": None, "delivered": False, "uncertain": True,
                **({"entry_id": entry["id"]} if track_entry and entry is not None else {})}
    if result == "deferred" and entry is not None:
        try:
            await _send_thread(PromptQueue(name).set_delivered, entry["id"], False)
        except OSError as exc:
            return {"ok": False, "error": erro("erro_fila_nao_entregue", str(exc), erro=str(exc))}
    if result != "sent" and entry is None:
        # "deferred" (corrida idle->working entre o deliverable e o send) + sidecar morto: o texto NAO
        # foi digitado E nao ha entrada pendente pro drain-on-complete drenar -- a msg nao esta em lugar
        # NENHUM. Aqui morre a suposicao do append la em cima ("entregavel -> a TUI leva o texto"):
        # o deferred e exatamente o caso em que nao levou. Ultimo ponto onde o 200 "na fila"
        # ainda seria a mentira do eeba30a.
        _log.error("prompt deferido sem entrada na fila — NAO foi entregue name=%s", name)
        return {"ok": False, "error": erro("erro_fila_nao_entregue",
                                                   "fila indisponivel e o turno nao aceitou o prompt: nao foi entregue")}
    # "deferred" COM entrada na fila: fica pendente (delivered ja e False) -> drain-on-complete entrega.
    return {"ok": True, "error": None, "delivered": result == "sent",
            **({"entry_id": entry["id"]} if track_entry and entry is not None else {})}


def _session_exists(name: str) -> bool:
    """Sessão existe DE VERDADE (pane tmux vivo ou sidecar Codex)? Sem esta guarda o /input aceitava
    qualquer nome e enfileirava no VOID: 'ok' pra sessão morta = recado órfão que só seria entregue
    se um dia nascesse outra sessão com o mesmo nome (foi exatamente como um recado 'se perdeu')."""
    from app import tmux
    return codex_sessions.exists(name) or headless_sessions.exists(name) or tmux.has_session(name)


def _headless(name: str) -> bool:
    """Sessão Claude SEM terminal (sidecar do adapter headless). Provider continua "claude"; só o
    transporte muda — quem ramifica por isto é a entrada, o interrupt, a opção e a resposta."""
    return headless_sessions.exists(name)


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def input_prompt(name: str, body: InputBody):
    # Ramifica por provider: Claude via _send_thread (_send_one e SYNC/bloqueante — tmux), Codex via
    # _send_one_codex (async, app-server). Default "claude" pra qualquer nome nao-Codex.
    # NUCLEO SAGRADO: o envio NUNCA disputa executor com feature. _send_thread e um pool DEDICADO
    # (nao o default do asyncio, que a decoracao — git_summary/capture_pane — pode ocupar). Assim um
    # git status pendurado + refine (60s, pool do anyio) + check (600s, thread propria) nao seguram
    # o POST /input. Ver _send_thread.
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, detail=erro("erro_sessao_recado_nao_enfileirado", "sessão não encontrada — recado NÃO enfileirado"))
    provider = _provider_of(name)
    tracking = {"track_entry": True} if body.steer else {}
    if provider == "codex":
        res = await _send_one_codex(name, body.text, **tracking)
    elif _headless(name):
        res = await _send_one_headless(name, body.text, **tracking)
    else:
        res = await _send_thread(_send_one, name, body.text, True) if body.steer else await _send_thread(_send_one, name, body.text)
    if not res["ok"]:
        raise HTTPException(400, res["error"])
    steered = False
    entry_id = res.get("entry_id")
    if body.steer and entry_id:
        try:
            if provider == "codex" and not res.get("delivered"):
                # steer_queue disputa a mesma trava do envio; só pode rodar depois dele.
                sent = await get_adapter("codex").steer_queue(name, entry_id=entry_id)
                steered = entry_id in sent
            elif _headless(name) and not res.get("delivered"):
                sent = await get_adapter(CLAUDE_HEADLESS).steer_queue(name, entry_id=entry_id)
                steered = entry_id in sent
        except Exception:
            # O recado já existe na fila. Falha de orientação nunca faz outro append/envio.
            _log.exception("falha apos persistir recado name=%s entry=%s steered=%s", name, entry_id, steered)
        if provider == "codex":
            # Outra promoção ou o drain pode ter concluído a entrega enquanto esperávamos.
            try:
                rows = await _send_thread(PromptQueue(name).load)
                receipt = next((row for row in rows if row.get("id") == entry_id), None)
                if receipt is not None:
                    steered = steered or receipt.get("steered") is True
                    res["delivered"] = res.get("delivered", False) or receipt.get("delivered") is True
            except OSError:
                _log.exception("recibo ilegivel apos orientacao name=%s entry=%s steered=%s", name, entry_id, steered)
    # A orientação confirmada também conta como entrega, sem redigitar o recado na TUI.
    # `native` = escrito no socket do Claude do destino: entra no meio do turno dele, sem tecla.
    return {"ok": True, "delivered": res.get("delivered", False) or steered, "steered": steered,
            "native": bool(res.get("native"))}


@app.post("/api/broadcast", dependencies=[Depends(require_auth)])
async def broadcast(body: BroadcastBody):
    """Fan-out de UM prompt pra N sessoes (feature #9): mesma sequencia do /input, em loop — sessao
    ocupada enfileira na fila duravel dela (crash-safe), sessao ociosa recebe na hora, sem mecanismo
    de entrega novo. Ramifica por provider por nome (Claude via to_thread, Codex via _send_one_codex),
    reportando por-sessao sem abortar as demais. Slash-commands ficam FORA (rota por sessao so):
    "/clear" pra N sessoes de uma vez e ambiguo/perigoso (o front ja desabilita o envio; isto e defesa
    em profundidade)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, detail=erro("erro_broadcast_slash", "broadcast nao suporta slash-commands: envie por sessao"))
    results: dict[str, dict] = {}
    for name in body.names:
        # Mesma guarda do /input: nome sem sessão viva -> erro POR SESSÃO (não enfileira no void).
        if not await _send_thread(_session_exists, name):
            results[name] = {"ok": False, "error": erro("erro_sessao_inexistente", "sessão não encontrada"), "delivered": False}
            continue
        results[name] = await _enviar(name, body.text)
    return {"results": results}


class PluginPressBody(_StrictBody):
    site: str = Field(min_length=1, max_length=64)
    # O mod do botão: a `key` só é única dentro de um mod. O app de antes desta versão não o manda.
    plugin: str | None = Field(default=None, min_length=1, max_length=256)
    key: str = Field(min_length=1, max_length=256)


class PluginCloseBody(_StrictBody):
    site: str = Field(min_length=1, max_length=64)


# O clique dos mods é do Rust, sem reserva aqui: modo `python` ou sessão que ele não atende.
_MOD_SEM_RUST = erro("mods_need_rust_server", "O clique do mod precisa do servidor Rust de pé e da sessão aberta nele.")


def _convidado(request: Request) -> bool:
    return guest_of(request) is not None


@app.post("/api/sessions/{name}/plugin/press", dependencies=[Depends(require_auth)])
async def plugin_press(name: str, body: PluginPressBody):
    """Clique num botão que um mod desenhou na faixa ou num painel, pedido pelo app."""
    raise HTTPException(503, detail=_MOD_SEM_RUST)


@app.post("/api/sessions/{name}/plugin/close", dependencies=[Depends(require_auth)])
async def plugin_close(name: str, body: PluginCloseBody):
    """Fecha um painel de mod pelo `✕` do cabeçalho, pedido pelo app."""
    raise HTTPException(503, detail=_MOD_SEM_RUST)


@app.post("/api/sessions/{name}/plugin/show", dependencies=[Depends(require_auth)])
async def plugin_show(name: str):
    """Troca de aba de mod. Fora do Rust ela fica no app, que lê o 404 como "sem a rota"."""
    raise HTTPException(404, detail=erro("erro_mod_aba_no_app", "Nesta sessão a troca de aba do mod fica no app."))


@app.post("/api/sessions/{name}/plugin/input", dependencies=[Depends(require_auth)])
async def plugin_input(name: str):
    """Digitação num campo de mod: só a sessão sem terminal que o Rust atende a aceita."""
    raise HTTPException(409, detail=erro("erro_mod_sem_digitacao",
        "Nesta sessão, o campo do mod só aceita digitação no terminal ou não está ligado ao app."))


# A pergunta lateral é do Rust (`session_write/btw.rs`); pelo Connect o `_codex_rust_port` a repassa.
@app.api_route("/api/sessions/{name}/btw", methods=["GET", "POST"], dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def pergunta_lateral_precisa_do_rust(name: str):
    raise HTTPException(503, detail=erro("btw_needs_rust_server", "a pergunta lateral precisa do servidor Rust"))


@app.get("/api/sessions/{name}/pane", dependencies=[Depends(require_auth)])
def pane(name: str, lines: int = 200):
    # Pane CRU (texto ja composto pelo tmux: sem ANSI/cursor-move). O espelho do pane (TerminalMirror)
    # le isto pra mostrar overlays so-TUI (/status, /config, /help, pickers) que nao caem no .jsonl.
    # `lines` = quanto SCROLLBACK trazer acima da tela visivel (capture-pane -S). O espelho pede mais
    # quando o usuario rola pro topo; clampeado pra uma janela absurda nao virar payload gigante a
    # cada poll de 450ms.
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    # `scrollback` diz se pedir mais linhas ADIANTA. Num TUI de tela alternada (Claude Code) vale 0:
    # o tmux nao guarda historico ali, e quem quer subir tem que rolar o PROPRIO TUI (PageUp), nao o
    # tmux. Sem esse dado a UI ofereceria "carregar mais historico" que nunca traria nada.
    return {"text": tmux.capture_pane(name, lines=max(50, min(lines, 5000))),
            "scrollback": tmux.pane_scrollback(name)}


def _erro_conta_codex(exc: codex_accounts.AccountError) -> HTTPException:
    mensagens = {
        "codex_account_ambiguous_rollout": "a conversa Codex existe em mais de uma conta; escolha a conta",
        "codex_account_archive_mismatch": "a conta escolhida não é a origem desta conversa Codex",
        "codex_account_not_found": "conta Codex não encontrada",
        "codex_account_invalid_name": "nome de conta Codex inválido",
        "codex_account_invalid_marker": "conta Codex inválida",
    }
    return HTTPException(exc.status, detail=erro(
        exc.code, mensagens.get(exc.code, "operação de arquivo Codex recusada"), **exc.params))


@app.get("/api/sessions/{name}/navegador", dependencies=[Depends(require_auth)])
async def navegador_da_sessao(name: str, request: Request):
    request = Request({**request.scope, "path": f"/api/sessions/{urllib.parse.quote(name, safe='')}/navegador"}, request.receive)
    return await _repasse_ao_rust(request, "/__hangar_server/forwarded")
