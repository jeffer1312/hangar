import anyio.to_thread
import asyncio
import functools
import json
import logging
import os
import re
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.parse
import uuid
from datetime import datetime
from contextlib import asynccontextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Literal, Optional
from fastapi import FastAPI, Depends, HTTPException, Query, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import FileResponse, JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.gzip import GZipMiddleware
from starlette.requests import ClientDisconnect
from pydantic import BaseModel, ConfigDict, Field, model_validator
from sse_starlette.sse import EventSourceResponse
from app import (atomico, closed_names, diag,
                 loop_monitor, permission_mode, plugin_bridge, procinfo, quem_chama, tmux,
                 uds_messaging)
from app import groups_bridge, internal_api, list_bridge, migration_status, share_bridge
from app.auth import require_auth
from app.send_executor import send_thread as _send_thread
from app.model_picker import PickerError
from app.mensagens import erro
from app import claude_customizations
from app import model_args
from app import registry as registry_mod
from app.registry import KillFailed, SessionRegistry
from app.names import sanitize_session_name
from app.models import (SessionInfo, CreatedSessionInfo, ChatEvent, CostReport, RunnersResponse, RunBody,
                        RunInfo, Runner, CustomRunnersBody, ProjectStatus,
                        ShortcutAnswerBody, session_key)
from app.pqueue import (PromptQueue, _transcript_start_ts, committed_user_lines,
                        fila_interna_pendente, linha_mais_parecida)
from app.prune import prune_loop as _prune_loop
from app.chain import ThenLink
from app import terminal_input
from app.terminal_input import TerminalInput, drain
from app.adapters import CLAUDE_HEADLESS, get_adapter
from app.adapters.claude_headless import sessions as headless_sessions
from app.adapters.codex import sessions as codex_sessions
from app.sse import invalidate_recent_list, merged_events, nav_confirmar, nav_pendente
from app.state import forget_frame, menu_codex
from app.uploads import MAX_BYTES
from app.config import (list_config_dirs, _backend_config_base, settings,
                        automations_enabled, resolve_bind_ip)
from app import runtime_config
from app.guest_header import guest_of, guest_safe
from app.share_life import session_life
from app import contas, default_model, procinfo
from app.costs import report as costs_report, usd_brl as _usd_brl, PERIODOS as _COST_PERIODOS
from app import costs_sources
from app import loop as loop_mod
from app.transcript import last_assistant_text
from app import runner
from app import projects
from app import archive_providers
from app.archive import (archive_cwd, archive_jsonl,
                         list_folders)
from app.askquestion import clear_pending_askq, read_pending_askq
from app import codex_contas as codex_accounts
from app.codex_contas_login import CodexContasLogin
from app.pair import PairLink
from app.hook_state import hook_state

_log = logging.getLogger("hangar")


def _codex_service():
    return getattr(app.state, "codex_contas_login", None)


def _resolve_codex_account(account_id: str | None):
    try:
        return codex_accounts.resolve_account(account_id or "default")
    except codex_accounts.AccountError as exc:
        raise HTTPException(exc.status, detail=erro(exc.code, "conta Codex inválida", **exc.params)) from None


async def _codex_require_idle_preparation(account, service) -> None:
    if account.is_default:
        return
    if service is None:
        raise HTTPException(503, detail=erro("codex_account_service_unavailable",
                                             "serviço de contas Codex indisponível"))
    # HTTP síncrono ao Rust: no laço, congelaria o backend inteiro até o prazo da ponte.
    status = await asyncio.to_thread(service.preparation_status, account)
    if status.get("status") != "ready":
        _log.warning("conta Codex %s com sincronização %s; pendências: %s",
                     account.id, status.get("status"),
                     [issue.get("code") for issue in status.get("issues", [])])


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
    # Inbox do socket nativo: é o endereço de resposta dos recados que o backend escreve nos
    # sockets do Claude; sem ele o recibo de retenção/recusa não tem pra onde voltar.
    if uds_messaging.INBOX.ligar(_ao_recibo_nativo):
        _log.info("inbox nativo ligado em %s", uds_messaging.INBOX.path)
    await _boot_sessions(runtime)
    _state_dirs =list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    hook_state.on_transition = _on_hook_transition  # drain server-side + confirmacao de entrega
    task = asyncio.create_task(hook_state.watch(_state_dirs))

    def _watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("hook_state.watch crashed", exc_info=exc)

    task.add_done_callback(_watch_done)

    loop_monitor_task = asyncio.create_task(loop_monitor.watch(), name="loop-monitor")

    # Poda periodica dos sidecars de sessao morta (Task G3): varre na subida e depois a cada
    # 24h — ver app/prune.py para o criterio conservador (chave de sessao nao viva + idade
    # minima de 7 dias) e o porquê de periodica em vez de so no startup.
    prune_task = asyncio.create_task(_prune_loop())

    def _prune_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("prune.prune_loop crashed", exc_info=exc)

    prune_task.add_done_callback(_prune_done)

    # usd_brl() tem cache de 1h e timeout de 3s, e é chamado DENTRO do
    # montar(). Sem aquecer aqui, o primeiro /api/costs depois de todo restart paga a coleta fria
    # (657ms medidos) MAIS até 3s de câmbio, contra o AbortSignal.timeout(4000) do cliente.
    threading.Thread(target=_usd_brl, name="usd-brl-warm", daemon=True).start()
    # Mesmo motivo, outro caminho: o drain do Codex e assincrono (app-server) e quem o chama sao
    # threads (Timer da confirmacao, gatilho de hook). Ver `_drenar`.
    global _loop_servidor
    _loop_servidor = asyncio.get_running_loop()
    if runtime.mode == "python":
        _start_transfer_recovery()
    codex_warm_task = asyncio.create_task(get_adapter("codex").watch_sessions())
    codex_contas_login = CodexContasLogin()
    app.state.codex_contas_login = codex_contas_login
    # Referência guardada: task sem dono pode ser coletada no meio.
    app.state.codex_auth_aquecer = asyncio.create_task(codex_contas_login.aquecer())
    try:
        yield
    finally:
        diag.registrar("backend.encerrando")
        await runtime.shutdown()
        await runtime.close_events()
        costs_sources.cancelar_aquecimento()
        # Claude sem terminal fica vivo no cano: só fecha a conexão; o próximo backend religa.
        if _transfer_recovery is not None:
            await asyncio.shield(_transfer_recovery)
        get_adapter(CLAUDE_HEADLESS).desligar_todas()
        codex_warm_task.cancel()
        app.state.codex_auth_aquecer.cancel()
        await asyncio.gather(codex_warm_task, app.state.codex_auth_aquecer, return_exceptions=True)
        task.cancel()
        loop_monitor_task.cancel()
        prune_task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        await asyncio.gather(loop_monitor_task, return_exceptions=True)
        try:
            await prune_task
        except asyncio.CancelledError:
            pass


async def _transfer_guard(name: str):
    from app.conversation_transfer import session_ingress, require_available, TransferError, public_error
    try:
        with session_ingress(name):
            await asyncio.to_thread(require_available, name)
            yield
    except TransferError as exc:
        raise HTTPException(exc.status, detail=public_error(exc)) from None


async def _transfer_check(name: str):
    from app.conversation_transfer import session_ingress, require_available, TransferError, public_error
    try:
        with session_ingress(name):
            await asyncio.to_thread(require_available, name)
    except TransferError as exc:
        raise HTTPException(exc.status, detail=public_error(exc)) from None


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
        if reply is None and (rota == "btw" or (rota in {"input", "answer", "select", "select/submit", "permission-mode", "permission-modes", "model-effort", "recarregar"} or rota.startswith("queue/"))
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


def _transfer_send_error(name: str) -> dict | None:
    from app.conversation_transfer import transfer_active, TransferError, public_error
    if transfer_active(name):
        return {"ok": False, "error": public_error(TransferError("session_transfer_busy")), "delivered": False}
    return None


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


def _group_unavailable(code: str) -> HTTPException:
    return HTTPException(503, detail=erro("erro_grupo_indisponivel",
                                          "os grupos estão indisponíveis agora", detalhe=code))


@app.exception_handler(groups_bridge.GroupsBridgeError)
async def _grupos_indisponiveis(request: Request, exc: groups_bridge.GroupsBridgeError):
    """Ponte dos grupos fora vira 503 com código, nunca 500 sem explicação."""
    diag.registrar("grupos.indisponivel", "erro", codigo=exc.code, detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(status_code=503, content={"detail": _group_unavailable(exc.code).detail})


async def _repasse_ao_rust(request: Request, sem_rust: dict, destino: str, **repasse):
    """Pedido de uma rota que só o Rust atende, chegado por uma porta do Python (Connect,
    convidado): autenticado aqui, ele segue pela ponte privada e a resposta volta como veio. Sem o
    Rust não há reserva Python: a rota responde indisponível, com o código de `sem_rust`."""
    from app import account_bridge
    try:
        await require_auth(request)
    except HTTPException as error:
        return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
    if account_bridge.owner_mode() == "python":
        return JSONResponse({"detail": sem_rust}, status_code=503)
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
        if account_bridge.owner_mode() == "python":
            return JSONResponse({"detail": {"code": "providers_need_rust_server"}}, status_code=503)
        response = await account_bridge.forward_stream(request, guest=guest_of(request) is not None)
        return await call_next(request) if response.status_code == 421 else response
    unavailable = (erro("orq_conductor_need_rust_server", "o condutor precisa do servidor Rust, que não está rodando")
                   if re.fullmatch(r"/api/orq/[^/]+/conductor", path) else
                   _groups_need_rust() if re.fullmatch(r"/api/sessions/[^/]+/orq/grupo", path) else
                   {"code": "providers_need_rust_server"})
    response = await _repasse_ao_rust(request, unavailable, "/__hangar_server/forwarded", limit=None)
    return await call_next(request) if write and response.status_code == 421 else response


@app.middleware("http")
async def _contas_pelo_rust(request: Request, call_next):
    """Contas e cotas têm um dono só, o Rust."""
    from app import account_bridge
    listing = request.method == "GET" and request.url.path == "/api/credenciais"
    if not (listing or request.url.path.startswith(account_bridge.ACCOUNT_PREFIXES)):
        return await call_next(request)
    return await _repasse_ao_rust(request, account_bridge.NEED_RUST, "/__hangar_server/accounts/public")


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
    if account_bridge.owner_mode() == "python" and path not in {"/api/worktrees/settings", "/api/worktrees/fetch"}:
        return await call_next(request)
    options = {"limit": None, "default_json_content_type": False} if path == "/api/worktrees/fetch" else {}
    return await _repasse_ao_rust(request, {"code": "worktrees_need_rust_server"}, "/__hangar_server/forwarded", **options)


@app.middleware("http")
async def _jev_aux_pelo_rust(request: Request, call_next):
    """A busca de modelos e a disponibilidade do Jev são do Rust; sem ele, o erro diz isso em vez de um 404 que a tela
    leria como servidor antigo."""
    delivery = request.url.path == "/api/jev/delivery"
    if not delivery and not request.url.path.startswith("/api/jev/aux/"):
        return await call_next(request)
    if delivery and (guest_of(request) is not None):
        return JSONResponse({"detail": "Not Found"}, status_code=404)
    return await _repasse_ao_rust(request, erro("erro_jev_aux_precisa_rust", "o servidor está sem o processo Rust; reinicie o Hangar"),
                                  "/__hangar_server/forwarded" if delivery else "/__hangar_server/jev/aux")


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
    return await _repasse_ao_rust(request, _groups_need_rust(), "/__hangar_server/forwarded")


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
        return await _repasse_ao_rust(request, erro("claude_customizations_unavailable", "a seleção de plugins e skills precisa do servidor Rust"),
                                      "/__hangar_server/forwarded")
    if metodo == "POST" and _RETOMAR.match(path):
        return await _repasse_ao_rust(request, erro(registry_mod.SessionOpenNeedsRust.code, "abrir e reabrir sessão precisam do servidor Rust, que não está rodando"),
                                      "/__hangar_server/forwarded")
    padrao = _PLANOS_ESCRITA if metodo == "POST" else _PLANOS_LEITURA if metodo == "GET" else None
    if padrao is None or not padrao.match(path):
        return await call_next(request)
    return await _repasse_ao_rust(request, erro("erro_planos_precisa_rust", "os planos precisam do servidor Rust, que não está rodando"),
                                  "/__hangar_server/forwarded")


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
# (`uploads`, `session_media`). A imagem de conversa transferida fica aqui, como o histórico dela.
_MIDIA = re.compile(r"^/api/sessions/[^/]+/(upload|uploads|uploads/[^/]+|transcribe|bash-output|tool-progress/[^/]+|transcript-image/[^/]+/[^/]+)$")


@app.middleware("http")
async def _midia_pelo_rust(request: Request, call_next):
    from app import account_bridge
    match, metodo = _MIDIA.match(request.url.path), request.method
    rota = match.group(1) if match else ""
    owned = (metodo == "POST" and rota in ("upload", "transcribe", "bash-output")
             or metodo in ("GET", "HEAD") and rota.startswith("uploads")
             or metodo == "GET" and (rota.startswith("tool-progress/")
                                     or rota.startswith("transcript-image/") and not rota.startswith("transcript-image/transfer:")))
    if not owned:
        return await call_next(request)
    try:
        await require_auth(request)
    except HTTPException as error:
        return JSONResponse({"detail": error.detail}, status_code=error.status_code, headers=error.headers)
    if account_bridge.owner_mode() == "python":
        return JSONResponse({"detail": erro("erro_midia_precisa_rust", "anexos, transcrição e saída de ferramenta precisam do servidor Rust, que não está rodando")},
                            status_code=503)
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
    return await _repasse_ao_rust(request, erro("mermaid_indisponivel", "o desenho de diagramas precisa do servidor Rust"),
                                  "/__hangar_server/forwarded", keep=_MERMAID_HEADERS, limit=64 * 1024 * 1024)


@app.api_route("/api/session-defaults", methods=["GET", "PUT"], dependencies=[Depends(require_auth)])
async def session_defaults_forward(request: Request):
    """O Connect só transporta o padrão; leitura e escrita pertencem ao Rust."""
    if guest_of(request) is not None:
        raise HTTPException(403, detail=erro("session_defaults_owner_only", "só o dono pode acessar o padrão da abertura"))
    return await _repasse_ao_rust(request, erro("session_defaults_unavailable", "o padrão da abertura precisa do servidor Rust"),
                                  "/__hangar_server/forwarded")


@app.put("/api/sessions/prewarm", dependencies=[Depends(require_auth)])
@app.delete("/api/sessions/prewarm/{reserve_id}", dependencies=[Depends(require_auth)])
async def prewarm_forward(request: Request):
    """A reserva pré-aquecida é do Rust (`prewarm::http`); o Connect só autentica e repassa."""
    if _convidado(request):
        raise HTTPException(403, detail=erro("prewarm_owner_only", "só o dono reserva uma sessão pré-aquecida"))
    return await _repasse_ao_rust(request, erro(registry_mod.SessionOpenNeedsRust.code, "abrir e reabrir sessão precisam do servidor Rust, que não está rodando"),
                                  "/__hangar_server/forwarded")


@app.get("/api/client-api")
async def client_api_pelo_rust(request: Request):
    """O número é do Rust (`hangar_api::CLIENT_API`). Aqui chega o dono pelo Connect, que segue pela
    ponte; sem o Rust é 503 com código, nunca um número que o Python não conhece."""
    if _convidado(request):
        return JSONResponse({"detail": "Not Found"}, status_code=404)
    return await _repasse_ao_rust(request, erro("client_api_need_rust_server", "a versão da API precisa do servidor Rust"),
                                  "/__hangar_server/forwarded")


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
app.include_router(migration_status.router)
app.include_router(internal_api.router)
registry = SessionRegistry()
terminal = TerminalInput()

@app.websocket("/api/sessions/{name}/term")
async def term_ws_route(ws: WebSocket, name: str):
    from app import termsock
    # `?shortcut=<id>`: terminal de atalho DESTA sessao. O alvo tmux sai do dono conferido no
    # servidor, nunca da query — o id sozinho nao alcanca terminal de outra conversa.
    ident = ws.query_params.get("shortcut")
    resolve = None
    if ident is not None:
        from app import shortcut_terminals
        resolve = lambda: shortcut_terminals.find(name, ident)   # noqa: E731
    await termsock.term_ws(ws, name, resolve)


@app.websocket("/api/hangar-terminals/{ident}/term")
async def hangar_term_ws_route(ws: WebSocket, ident: str):
    # Terminal de nenhuma sessao: convidado de sessao compartilhada nunca chega aqui.
    from app import shortcut_terminals, termsock
    from app.guest_header import guest_of
    if guest_of(ws) is not None:
        await ws.close(code=1008)
        return
    await termsock.term_ws(ws, "hangar", lambda: shortcut_terminals.find_hangar(ident))


@app.websocket("/api/sessions/{name}/nav-remoto")
async def nav_ws_route(ws: WebSocket, name: str):
    # Acesso remoto ao navegador embutido DAQUELA sessao: quadros pra fora, toque/tecla pra dentro.
    # Mesma porta de entrada do painel de terminal (token + Origin) -- o de la abre um shell, este
    # abre o navegador que o agente esta dirigindo.
    from app import navsock
    await navsock.nav_ws(ws, name)


@app.post("/api/sessions/{name}/shell", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def abrir_shell(name: str):
    # Sessao de shell SEPARADA e ESCONDIDA do app (Task 6) -- ver tmux.new_hidden_shell. Sync (nao
    # async): mesmo padrao das rotas POST vizinhas (select/answer acima), que resolvem a sessao via
    # `registry.list()` direto e deixam o FastAPI rodar o handler bloqueante no threadpool.
    from app import tmux
    info = _cached_info_sync(name)
    if info is None:
        raise HTTPException(status_code=404, detail=erro("erro_sessao_inexistente", "sessao nao existe"))
    alvo = f"term-{name}"
    # Achado da revisao (I1, e de novo na rodada 2): `sanitize_session_name` aceita hifen, entao
    # "term-<nome>" pode ja existir como sessao de TERCEIRO (ex: usuario criou "foo" e depois
    # "term-foo" na mao). A 1a versao inferia isso de `registry.list()` -- mas a lista TAMBEM
    # filtra sessao com sidecar Codex de mesmo nome, entao um "term-<nome>" que fosse Codex de
    # verdade sumia da lista por ESSE motivo, nao por ser nosso, e a inferencia concluia (errado)
    # que o nome estava livre. Pergunta DIRETA ao tmux (`is_hidden`), nao mais inferida: cobre
    # Codex tambem, ao custo de 1-2 forks a mais nesta rota de clique unico (nao e o caminho de
    # poll onde fork por sessao e proibido).
    if tmux.has_session(alvo) and not tmux.is_hidden(alvo):
        # L68 da revisao final: o texto NAO afirma mais que a sessao e de terceiro. Ela pode ser o
        # shell DESTE painel que ficou sem a marca (um `set-option` que falhou por tmux ocupado/
        # timeout, ver tmux.new_hidden_shell) -- e como este gate recusa ANTES de chamar aquela
        # funcao, nada se autocorrige sozinho: quem desempata e o usuario.
        raise HTTPException(status_code=409,
                            detail=erro("erro_sessao_tmux_em_uso",
                                        f"ja existe uma sessao tmux chamada {alvo!r} sem a marca do "
                                        "painel -- pode ser uma sessao sua de mesmo nome, ou o shell "
                                        "deste painel que perdeu a marca. Encerre ou renomeie essa "
                                        "sessao antes de abrir o shell", nome=alvo))
    # O cwd vem do REGISTRY, nunca da query: um `?cwd=/` viraria shell em qualquer lugar do disco.
    novo = tmux.new_hidden_shell(name, info.cwd or str(Path.home()))
    if novo is None:
        raise HTTPException(status_code=500, detail=erro("erro_shell_criacao_falhou", "tmux recusou criar o shell"))
    return {"ok": True, "shell": novo}


# Emuladores de terminal conhecidos, na ordem de preferencia da sonda quando CP_TERMINAL nao esta
# setado. Cada valor monta o ARGV completo de attach dado o alvo tmux exato ("=nome:" -- NUNCA sem
# o `=`, senao o tmux cai em prefix-match e abre a sessao errada; o `:` final e a mesma grafia do
# `attach` do termsock, alinhada na revisao final -- medido nesta maquina que as duas formas
# anexam igual, e uma operacao so nao pode ter duas grafias numa branch inteira sobre esse
# detalhe). `wezterm` nao tem `-e`: e
# `start -- comando`. `gnome-terminal -e` esta deprecado e so aceita UM string; `--` e o substituto.
_EMULADORES = {
    "wezterm": lambda alvo: ["wezterm", "start", "--", "tmux", "attach", "-t", alvo],
    "kitty": lambda alvo: ["kitty", "tmux", "attach", "-t", alvo],
    "alacritty": lambda alvo: ["alacritty", "-e", "tmux", "attach", "-t", alvo],
    "konsole": lambda alvo: ["konsole", "-e", "tmux", "attach", "-t", alvo],
    "gnome-terminal": lambda alvo: ["gnome-terminal", "--", "tmux", "attach", "-t", alvo],
    "xterm": lambda alvo: ["xterm", "-e", "tmux", "attach", "-t", alvo],
}
_ORDEM_PROBE = ["wezterm", "kitty", "alacritty", "konsole", "gnome-terminal", "xterm"]


@app.post("/api/sessions/{name}/open-terminal", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def abrir_terminal_nativo(name: str):
    """Abre um emulador de terminal NATIVO (janela propria do SO) anexado a sessao tmux `name` --
    tanto a do agente quanto a do shell escondido, o alvo e so um nome de sessao tmux. Diferente do
    painel embutido (termsock/xterm.js): esta janela nao depende do backend pra existir, entao
    fechar o painel ou reiniciar o servico NAO a desanexa.

    Checa via `tmux.has_session` (nao `registry.list()`): a sessao de shell escondida (Task 6) NAO
    aparece no registry por design, mas continua um alvo valido pra este botao.
    """
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(status_code=404, detail=erro("erro_sessao_inexistente", "sessao nao existe"))
    nome_bin = os.environ.get("CP_TERMINAL")
    if nome_bin:
        # env checada ANTES do PATH: se o usuario apontou um emulador, e ele que vale -- so falha
        # se esse binario especifico nao existir ou nao for suportado (dicionario fechado; NAO
        # inventa um `-e` generico pra emulador desconhecido).
        if nome_bin not in _EMULADORES or shutil.which(nome_bin) is None:
            raise HTTPException(status_code=503,
                                detail=erro("erro_terminal_invalido",
                                            f"CP_TERMINAL={nome_bin!r} nao encontrado no PATH ou nao "
                                            "suportado", nome=nome_bin))
    else:
        nome_bin = next((n for n in _ORDEM_PROBE if shutil.which(n)), None)
        if nome_bin is None:
            raise HTTPException(status_code=503,
                                detail=erro("erro_terminal_ausente", "nenhum emulador de terminal encontrado no PATH"))
    args = tmux._scope_prefix() + _EMULADORES[nome_bin](f"={name}:")
    env = os.environ.copy()
    wl = tmux._wayland_display()
    if wl:
        # sem isto o emulador GUI nao acha o compositor quando o backend roda como servico systemd
        # (env de boot, sem WAYLAND_DISPLAY) -- mesmo problema que o wl-paste do new_session.
        env["WAYLAND_DISPLAY"] = wl
    disp = os.environ.get("DISPLAY")
    if disp:
        # Achado da revisao (I5): X11/XWayland precisa de DISPLAY, nao so WAYLAND_DISPLAY -- sem
        # repassar, um host X11 (ou o servico systemd sem env de sessao grafica) faz o emulador
        # executar e morrer com "cannot open display" LOGO apos o exec, onde o Popen nao pega nada.
        env["DISPLAY"] = disp
    # Arquivo temporario, nao `PIPE` (achado da revisao, rodada 2): a janela que ABRE fica viva
    # bem alem deste request, e um `PIPE` sem leitor enche os 64KB do buffer do kernel e a
    # escrita do emulador TRAVA (medido no wezterm, primeiro da sonda, que loga bastante em
    # stderr) -- mais um fd vazado por clique. Arquivo comum nao tem esse teto.
    err_file = tempfile.TemporaryFile()
    try:
        p = subprocess.Popen(args, env=env, start_new_session=True, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=err_file)
    except OSError as e:
        err_file.close()
        raise HTTPException(status_code=503, detail=erro("erro_terminal_abertura_falhou", f"falha ao abrir o emulador de terminal: {e}", erro=str(e)))
    # Falha aparece, nao some (achado da revisao): o Popen so levanta se o BINARIO nao existe --
    # sem DISPLAY, com o compositor errado, ou qualquer erro pos-exec, o processo sai sozinho em
    # poucos ms e o `except OSError` acima nunca ve nada, devolvendo "ok" pra uma janela que nunca
    # abriu. Espera uma fracao de segundo e confere se ja morreu.
    time.sleep(0.3)
    morreu = p.poll()
    # `morreu != 0`, nao so `morreu is not None` (achado da revisao, rodada 2): sair com rc=0 em
    # poucos ms e COMPORTAMENTO NORMAL de cliente D-Bus/instancia unica -- `gnome-terminal` (na
    # sonda) abre no `gnome-terminal-server` e sai 0 na hora; `wezterm start` com GUI ja de pe e
    # `konsole` reusando instancia fazem o mesmo. Tratar qualquer saida como erro devolvia 503 pra
    # janela que abriu certo.
    if morreu is not None and morreu != 0:
        err_file.seek(0)
        saida = err_file.read().decode(errors="replace").strip()
        err_file.close()
        raise HTTPException(status_code=503,
                            detail=erro("erro_terminal_saiu_cedo",
                                        f"emulador de terminal saiu logo apos abrir: "
                                        f"{saida or f'codigo {morreu}'}",
                                        saida=saida or f"codigo {morreu}"))
    err_file.close()   # nosso handle; o filho, se ainda vivo, segue escrevendo no fd dele
    # Este `Popen` nunca e colhido explicitamente (sem wait(), sem thread de reaper): a janela vive
    # muito alem deste request e esperar por ela seria travar a rota. Quem colhe e o proprio
    # `subprocess`, que varre os filhos ja mortos a cada Popen novo — e o backend chama `tmux` o
    # tempo todo, entao o zumbi some sozinho em segundos. E uma DEPENDENCIA de detalhe interno do
    # modulo, nao um contrato: se um dia o backend parar de disparar subprocessos com frequencia,
    # cada clique aqui deixa um zumbi ate o processo reiniciar.
    return {"ok": True}


# Snapshot com TTL de registry.list() pros endpoints request/response QUENTES (history/workflows):
# o mount do board dispara dezenas de /history de uma vez e cada list() fresco e um scan completo
# de /proc + fork de tmux list-panes. Os loops do SSE leem este mesmo snapshot (sse._cached_list).
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
    """Descarta o snapshot cru e a lista decorada do refresher: quem pedir /api/sessions depois de
    uma mudança de membro ou de modo recalcula em vez de ver a sessão como era."""
    with _list_lock:
        _list_snap["snap"] = None
    invalidate_recent_list()
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
_RECHECA_KIMI = 5.0


# Loop do servidor, pra pontes sync->async (mesmo papel do `INBOX.ligar_loop`). Setado no lifespan.
_loop_servidor: asyncio.AbstractEventLoop | None = None


def _drenar(name: str, jsonl: str, provider: str) -> int:
    if _transfer_send_error(name):
        return 0
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
    if _transfer_send_error(name):
        return 0
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
        if info.provider == "codex" and not info.headless and runtime_coordinator.rust_owns_codex_terminal():
            return
        from app.conversation_history import confirmation_options
        confirmation = confirmation_options(name, info.jsonl, info.provider)
        # MID-TURN o prompt entregue ainda pode nao ter virado entrada no transcript (vive na fila
        # interna do Claude Code) — decidir requeue agora arriscaria redigitar mensagem ja recebida.
        # Adia pro proximo ciclo (o turno acabando dispara transicao -> novo timer).
        m = hook_state.get_state(session_key(info.jsonl))
        headless = _headless(name)
        grace = _CONFIRM_GRACE_HEADLESS if headless else _CONFIRM_GRACE
        # UMA leitura do oraculo pros dois ramos. None = nao deu pra ler o transcript (ver
        # committed_user_lines): sai SEM decidir e SEM reagendar. Sem reagendar de proposito — um
        # Timer a cada `grace` contra um arquivo que nao abre e tempestade sem fim; o proximo fim
        # de turno (`_on_hook_transition`) ou a proxima mensagem chamam isto de novo, e ate la a
        # entrada fica visivel como bolha "na fila". Falha VISIVEL, nunca mensagem duplicada.
        # As DUAS leituras do transcript ficam juntas e falham juntas. `_transcript_start_ts` abre
        # o mesmo arquivo uma segunda vez, e o 0.0 dele desliga a poda por idade — sem ela, entrada
        # de sessao ANTERIOR nao e mais dispensada e vai parar no caminho que REDIGITA. Ou seja: o
        # mesmo defeito, pela porta do lado. Aqui "nao sei" nunca decide nada.
        if headless and not os.path.exists(info.jsonl):
            # Sem terminal, .jsonl que nunca nasceu não é "ilegível": o processo morreu antes de
            # gravar o prompt. Sem isto a entrada ficava entregue e calada pra sempre.
            committed, inicio_ts = [], 0.0
        else:
            committed = committed_user_lines(info.jsonl, info.provider, **confirmation)
            inicio_ts = _transcript_start_ts(info.jsonl)
        # Enfileirada na TUI e ainda nao consumida: entregue, mas sem bolha real — segue visivel
        # como bolha da fila em vez de ser confirmada (escondida) pela linha de enqueue.
        na_fila = fila_interna_pendente(info.jsonl, info.provider, **confirmation)
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


# Sessoes com uma reavaliacao de "idle mentiroso" ja agendada. UMA cadeia por sessao: sem isto, cada
# transicao espuria abria a sua propria corrente de Timers, e duas correntes em paralelo dobram o
# `registry.list()` (que toca tmux) a cada 5s, sem limite e sem ninguem notar.


# Tentativas seguidas com FALHA antes de abandonar a cadeia de reavaliacao de uma sessao.


            # Reagenda MESMO ASSIM quando o idle era suspeito: a falha pode ter sido pontual
            # (registry/tmux piscando), e desistir aqui e o que deixa a sessao presa. Mas com TETO:
            # falha PERMANENTE (jsonl corrompido, erro reproduzivel no registry) reergueria a mesma
            # excecao a cada 5s pra sempre, pagando `registry.list()` (tmux) toda volta. Depois de
            # _RETRY_FALHA tentativas a cadeia para e diz isso no log — sessao presa e ruim, laco
            # eterno tocando tmux e pior.


_recheca_armada: set[str] = set()


_recheca_lock = threading.Lock()


_RETRY_FALHA = 5


_falhas_seguidas: dict[str, int] = {}


def _armar_recheca(session_id: str) -> bool:
    """True se ESTA chamada ficou dona da cadeia de reavaliacao da sessao."""
    with _recheca_lock:
        if session_id in _recheca_armada:
            return False
        _recheca_armada.add(session_id)
        return True


def _recheca_kimi(session_id: str, state: str) -> None:
    """Elo da cadeia: solta a posse ANTES de reavaliar, pra a proxima passada poder rearmar. Soltar
    depois prenderia a cadeia numa unica corrente que morre junto com uma excecao."""
    with _recheca_lock:
        _recheca_armada.discard(session_id)
    _on_hook_transition(session_id, state)


def _on_hook_transition(session_id: str, state: str) -> None:
    if state == "awaiting_input":
        return
    def _work() -> None:
        try:
            info = next((s for s in registry.list()
                         if s.jsonl and session_key(s.jsonl) == session_id), None)
            if info and _rust_conduz(info):
                return
            if info and info.jsonl:
                real = state
                sent = _drenar(info.name, info.jsonl, info.provider)
                # Confirmacao em TODO idle (nao so pos-drain): Timers pendentes morrem no restart
                # do backend — sem isto, entrada entregue ficava sem confirmar indefinidamente.
                if sent or real == "idle":
                    _agendar_confirmacao(info.name, _CONFIRM_GRACE + 0.5)
                loop_d = loop_mod.LoopLink(info.name).get()
                loop_active = loop_d is not None and loop_d["status"] in loop_mod.ACTIVE
                # Encadeamento (feature #12): so quando NAO ha loop ativo — turno REALMENTE terminado,
                # reusando o info.name ja resolvido nesta thread — ver _maybe_chain.
                if real == "idle" and not loop_active:
                    _maybe_chain(info.name)
                # Idle desmentido pelo transcript: o fim de turno REAL nao vai gerar transicao
                # nenhuma (o Stop grava idle sobre idle, e hook_state._apply so avisa quando o
                # estado MUDA). Sem reagendar, o turno terminaria sem drenar a fila, sem ticar o
                # loop e sem disparar o `then`. O reagendamento converge sozinho: quando o turno
                # acaba, o wire.jsonl para de crescer e a proxima passada ve idle de verdade.
                # UMA cadeia por sessao (_recheca_armada): cada transicao espuria abria a sua, e
                # duas cadeias em paralelo dobram `registry.list()` (tmux) a cada 5s pra sempre.
                _falhas_seguidas.pop(session_id, None)   # esta volta foi ate o fim: zera o teto
        except Exception as exc:
            from app.runtime_coordinator import RustCacheInvalid
            # Cache inválido já vai ao diário uma vez por sequência pelo coordenador; aqui repetiria a cada volta.
            if not isinstance(exc, RustCacheInvalid):
                _log.warning("transicao de estado falhou sid=%s state=%s — sem reavaliacao automatica "
                             "ate a proxima transicao", session_id, state, exc_info=True)
            # Reagenda MESMO ASSIM quando o idle era suspeito: a falha pode ter sido pontual
            # (registry/tmux piscando), e desistir aqui e o que deixa a sessao presa. Mas com TETO:
            # falha PERMANENTE (jsonl corrompido, erro reproduzivel no registry) reergueria a mesma
            # excecao a cada 5s pra sempre, pagando `registry.list()` (tmux) toda volta. Depois de
            # _RETRY_FALHA tentativas a cadeia para e diz isso no log — sessao presa e ruim, laco
            # eterno tocando tmux e pior.
            if state == "idle":
                n = _falhas_seguidas.get(session_id, 0) + 1
                if n <= _RETRY_FALHA:
                    _falhas_seguidas[session_id] = n
                    if _armar_recheca(session_id):
                        threading.Timer(_RECHECA_KIMI, _recheca_kimi,
                                        args=(session_id, state)).start()
                elif n == _RETRY_FALHA + 1:
                    # NAO zera o contador aqui: zerando, o proximo evento recomecava a contagem e o
                    # laco voltava a girar de 5 em 5s — teto que reinicia nao e teto. Quem zera e a
                    # volta que COMPLETA (no fim do `_work`), que e a prova de que voltou a
                    # funcionar. Loga uma vez so, na virada.
                    _falhas_seguidas[session_id] = n
                    _log.error("reavaliacao de %s abandonada apos %d falhas seguidas — a sessao so "
                               "volta a drenar sozinha na proxima transicao de estado",
                               session_id, _RETRY_FALHA)
                else:
                    _falhas_seguidas[session_id] = n
    threading.Thread(target=_work, daemon=True).start()


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


class SelectBody(_StrictBody):
    option: int = Field(ge=1, le=50)  # picker 1-based; teto evita loop de fork tmux (DoS)
    interaction_id: str | None = None    # do cliente; quem confere é o Rust


class KeyBody(_StrictBody):
    key: str  # nome da tecla de navegacao (allowlist em TerminalInput._NAV_KEYS)


class TermInputBody(_StrictBody):
    # Terminal interativo (so desktop): texto livre (literal) e/ou uma tecla nomeada (allowlist
    # em TerminalInput._TERM_KEYS). Os dois opcionais -> um POST pode mandar so texto OU so tecla.
    text: str | None = None
    key: str | None = None


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


@app.get("/api/costs", dependencies=[Depends(require_auth)], response_model=CostReport)
def costs_endpoint(period: str = "all", fresco: bool = False):
    # Período inválido cai em "all" em vez de 422: um cliente antigo da malha mandando qualquer
    # coisa não pode derrubar o custo daquela máquina inteira da soma.
    # Lista vem de costs.PERIODOS (fonte única com o montar()); "all" fica de fora do dict porque
    # não tem número de dias, então entra à parte aqui.
    # `fresco` = botão "Atualizar dados": coleta agora em vez de servir a última leitura.
    if period not in _COST_PERIODOS and period != "all":
        period = "all"
    try:
        return costs_report(period=period, fresco=fresco)
    except costs_sources.Aquecendo as e:
        return _aquecendo(e)


@app.get("/api/cotacao", dependencies=[Depends(require_auth)])
def cotacao_endpoint() -> dict:
    # Só a cotação, sem o relatório de custos junto: o custo por sessão aparece no painel, no card
    # do quadro e na folha de uso, e nenhum deles precisa varrer transcript pra saber a taxa.
    # A coleta em si tem cache de 1h e nunca levanta (costs.usd_brl).
    return {"usd_brl": _usd_brl()}


def _aquecendo(e: costs_sources.Aquecendo) -> JSONResponse:
    # Primeira leitura do histórico desta subida ainda rodando: a tela mostra o progresso e
    # pergunta de novo, em vez de esperar 20s e dar a máquina como "não respondeu".
    return JSONResponse({"aquecendo": True, "lidos": e.lidos, "total": e.total}, status_code=202)


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


def _jsonl_of_session(name: str) -> str | None:
    try:
        return next((s.jsonl for s in registry.list() if s.name == name and s.jsonl), None)
    except Exception:
        # A lista falhar não impede fechar; só a conversa fica sem o nome no arquivo.
        _log.warning("transcript of %s not found before close", name, exc_info=True)
        return None


def _sidecar_state(name: str) -> str:
    """`grouped`, `absent` (sem arquivo, ou válido sem grupo) ou `unreadable` (existe e não se lê)."""
    link = PairLink(name)
    if link.get() is not None:
        return "grouped"
    try:
        data = json.loads(link.path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return "absent"
    except (OSError, ValueError):
        return "unreadable"
    if not isinstance(data, dict) or not isinstance(data.get("peers", []), list):
        return "unreadable"
    return "absent"


async def _rust_leave(name: str, phases: dict) -> list[dict]:
    try:
        out = await asyncio.to_thread(groups_bridge.call, "group.leave", name=name, phases=phases)
    except groups_bridge.GroupsBridgeError as e:
        _log.warning("kill(%s): o Rust não tirou a sessão do grupo: %s", name, e.code)
        return [{"sessao": name, "erro": erro("erro_grupo_indisponivel", "os grupos estão indisponíveis agora", detalhe=e.code)}]
    return out.get("warnings") if isinstance(out.get("warnings"), list) else []


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def kill_session(name: str, by: str | None = None, request: Request = None):
    # 500 quando a sessao SOBREVIVE ao kill — mesmo padrao do /rename logo abaixo, que ja confere e
    # responde 404/500. Antes era {"ok": true} incondicional: o card sumia da UI e a sessao reaparecia
    # na varredura seguinte, sem fila e sem pareamento (ver SessionRegistry.kill).
    # Marca do Rust: ele já encerrou o Codex com terminal dele (e o grupo); aqui só as lojas daqui.
    marked = lambda h: bool(request and internal_api._secret) and secrets.compare_digest(request.headers.get(h, "").encode(), internal_api._secret.encode())
    closed = marked("x-hangar-rust-closed")
    # Fechamento que não passou pelo Rust (Connect, convidado) e o do Claude pelo Connect, que o Rust
    # encerrou sem sair do grupo: a saída vai pela ponte, com a fase do `orq` lida aqui antes, para o
    # Rust não perguntar de volta.
    skip_leave = closed
    # Sem sidecar não há grupo a deixar: a ponte nem é chamada. Ilegível não é ausente: sai pela ponte.
    sidecar = "absent" if skip_leave else await asyncio.to_thread(_sidecar_state, name)
    if sidecar == "unreadable":
        diag.registrar("grupos.sidecar_ilegivel", "erro", sessao=name)
    grouped = sidecar != "absent"
    phases = {} if grouped and groups_bridge.rust_owns_groups() else None
    # Sem o Rust a saída do grupo não acontece: fechar segue, e a falta vai no aviso e no diário.
    orphan = grouped and phases is None
    # Fechada pelo Rust a sessão já saiu da lista: o transcript vem no cabeçalho ou na resposta dele.
    if closed:
        jsonl = urllib.parse.unquote(request.headers.get("x-hangar-closed-transcript", "")) or None
    else:
        jsonl = await asyncio.to_thread(_jsonl_of_session, name)
    try:
        errs = await asyncio.to_thread(registry.kill, name, closed_by_rust=closed) or []
        if phases is not None:
            errs = [*errs, *await _rust_leave(name, phases)]
        if orphan:
            diag.registrar("grupos.saida_sem_rust", "erro", sessao=name, codigo="groups_need_rust_server")
            errs = [*errs, {"sessao": name, "erro": _groups_need_rust()}]
    except KillFailed as e:
        raise HTTPException(500, e.detail or str(e)) from None
    finally:
        if not closed:
            await asyncio.to_thread(_invalidate_lists)
    if jsonl:
        try:
            await asyncio.to_thread(closed_names.record, jsonl, name)
        except OSError:
            # Fechar já aconteceu; sem o nome, a conversa só não volta pelo nome antigo.
            _log.warning("closed name not recorded for %s", name, exc_info=True)
    elif closed:
        _log.warning("closed name not recorded for %s: Rust close sent no transcript", name)
    else:
        _log.info("closed name not recorded for %s: no transcript", name)
    plugin_bridge.esquecer(name)
    forget_frame(name)
    # O Rust revoga e desliga o túnel; fora do ar, fechar segue e o aviso volta na resposta. Fechada
    # pelo Rust, ele já revogou: perguntar de volta dentro da chamada dele prenderia o pool daqui.
    share_failed = None if closed else await asyncio.to_thread(share_bridge.revoke_session, name)
    warn = None
    if errs:
        if share_failed:
            errs = [*errs, {"sessao": name, "erro": share_failed}]
        warn = erro("erro_pareamento_saida_falhou",
                    "aviso de saída falhou: " + "; ".join(
                        f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
                    avisos=errs)
    elif share_failed:
        warn = share_failed
    return {"ok": True, "warning": warn}


class ModoExecucaoBody(_StrictBody):
    terminal: bool = Field(strict=True)


@app.post("/api/sessions/{name}/recarregar", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def recarregar_sessao(name: str):
    """Recarregar a sessão é do Rust (Claude e Codex, com e sem terminal); aqui fica a recuperação da
    transferência que falhou ao restaurar, que o Python conduz, e a recusa sem o Rust."""
    from app.conversation_transfer import transfer_for_session, TransferPhase, recover_transfer, TransferError, public_error
    record = await asyncio.to_thread(transfer_for_session, name)
    if record and record.phase == TransferPhase.RESTORE_FAILED:
        operation = asyncio.create_task(_durante_troca(name, recover_transfer(registry, record)))
        try:
            return await asyncio.shield(operation)
        except asyncio.CancelledError:
            try:
                await operation
            finally:
                raise
        except TransferError as exc:
            raise HTTPException(exc.status, detail=public_error(exc)) from None
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


async def _python_owns_headless() -> None:
    """O Python é o dono das sessões Claude sem terminal: religa os canos vivos e reagenda as
    conferências de entrega (Timers em memória que o restart apagou)."""
    try:
        religadas = await get_adapter(CLAUDE_HEADLESS).reconectar_todas()
        if religadas:
            _log.info("claude headless: %d sessão(ões) religada(s) ao cano", religadas)
        for meta in await asyncio.to_thread(headless_sessions.list_all):
            get_adapter(CLAUDE_HEADLESS).apos_entrega(meta["name"])
    except Exception:
        _log.warning("claude headless: religação de canos falhou", exc_info=True)


_transfer_recovery: asyncio.Task | None = None


async def _recover_pending_transfers() -> None:
    from app.conversation_transfer import list_incomplete, recover_transfer, TransferError
    for record in await asyncio.to_thread(list_incomplete):
        try:
            await _durante_troca(record.name, recover_transfer(registry, record))
        except TransferError:
            pass  # A fase durável mantém o erro e a ação Recarregar disponíveis.
        except Exception:
            _log.exception("recuperação da transferência de %s falhou", record.name)


def _start_transfer_recovery() -> None:
    """Uma vez por processo, depois que alguém é dono das sessões (Python, ou Rust de pé)."""
    global _transfer_recovery
    if _transfer_recovery is None:
        _transfer_recovery = asyncio.create_task(_recover_pending_transfers())


async def _boot_sessions(runtime) -> None:
    """Sessões Claude sem terminal na subida. O cano sobrevive ao restart; só morre aqui o de
    sessão encerrada com o backend fora. Com o Rust esperado, nada mais roda antes do desfecho
    dele: o modo `rust` abre as sessões nele, e o `python` (desistência) faz o que vinha aqui.
    A varredura de órfãos tem dono só: com o Rust de pé é dele, na subida dele."""
    async def sweep_orphans():
        try:
            from app.adapters.claude_headless.adapter import matar_orfaos
            mortos = await asyncio.to_thread(matar_orfaos)
            if mortos:
                _log.info("claude headless: %d cano(s) de sessão já encerrada finalizado(s)", mortos)
        except Exception:
            _log.warning("claude headless: varredura de canos órfãos falhou", exc_info=True)

    async def after_rust():
        _start_transfer_recovery()

    async def after_python():
        await sweep_orphans()
        # Cada etapa independe das outras: uma falha não deixa canos sem religar nem transferência parada.
        try:
            await runtime.register_claude_sessions()
        except Exception:
            _log.exception("registro das sessões Claude no Python falhou")
        await _python_owns_headless()
        _start_transfer_recovery()

    global _transfer_recovery
    _transfer_recovery = None       # um lifespan novo no mesmo processo (testes) recupera de novo
    runtime.mode_hooks.update(rust=after_rust, python=after_python)
    if runtime.mode == "python":
        await sweep_orphans()
        await _python_owns_headless()


async def _durante_troca(name: str, troca):
    """A transferência Claude→Codex (e a recuperação dela) muda a identidade da sessão (sidecar <->
    pane tmux); sem atualizar, a varredura revogaria o convite de uma sessão que continua viva. O
    Rust a segura no meio ("não sei", 503)."""
    if name in share_bridge.changing_mode:
        return await troca
    share_bridge.changing_mode.add(name)
    try:
        await asyncio.to_thread(share_bridge.mode_change, name, True)
        from app import runtime_coordinator
        coordinator = runtime_coordinator.current()
        if coordinator is not None and coordinator.managed_queue(name):
            return await coordinator.change(name, lambda: troca)
        return await troca
    finally:
        troca.close()
        # Também na falha: uma troca que morreu no meio pode já ter mudado a identidade.
        try:
            def _move_life():
                life = session_life(name)
                share_bridge.set_life(name, life)
            await asyncio.to_thread(_move_life)
        finally:
            share_bridge.changing_mode.discard(name)
            await asyncio.to_thread(share_bridge.mode_change, name, False)
            await asyncio.to_thread(_invalidate_lists)


class AccountMoveBody(_StrictBody):
    config_dir: str | None = None
    credential_id: str | None = None
    engine_account: str | None = None
    source_life: str | None = None
    source_jsonl: str | None = None
    model: str | None = None
    effort: str | None = None

    @model_validator(mode="after")
    def validate_target(self):
        if sum(v is not None for v in (self.config_dir, self.credential_id, self.engine_account)) != 1:
            raise ValueError("informe só config_dir, credential_id ou engine_account")
        if self.engine_account is not None:
            if self.model_fields_set != {"engine_account"}:
                raise ValueError("engine_account é o corpo completo da troca de conta ChatGPT")
            return self
        if self.config_dir is not None:
            if self.model_fields_set != {"config_dir"}:
                raise ValueError("config_dir é o corpo legado completo")
        else:
            if not self.credential_id.startswith("codex:") or not self.source_life or not self.source_jsonl:
                raise ValueError("destino Codex exige identidade e conversa de origem")
            self.model = self.model.strip() or None if self.model is not None else None
            self.effort = self.effort.strip() or None if self.effort is not None else None
        return self


@app.get("/api/sessions/{name}/conta", dependencies=[Depends(require_auth)])
async def contas_destino(name: str):
    """As contas de destino e a troca de conta são do Rust."""
    raise registry_mod.SessionOpenNeedsRust()


@app.post("/api/sessions/{name}/conta", dependencies=[Depends(require_auth)])
async def trocar_conta(name: str, body: AccountMoveBody):
    """A troca de conta Claude é do Rust; aqui fica a transferência da conversa para o Codex."""
    if body.credential_id is None:
        raise registry_mod.SessionOpenNeedsRust()
    from app.conversation_transfer import transfer_claude_to_codex, TransferError, public_error
    operation = asyncio.create_task(_durante_troca(name, transfer_claude_to_codex(
        registry, name, body.credential_id, body.source_life, body.model, body.effort,
        source_jsonl=body.source_jsonl)))
    try:
        return await asyncio.shield(operation)
    except asyncio.CancelledError:
        try:
            await operation
        finally:
            raise
    except TransferError as exc:
        raise HTTPException(exc.status, detail=public_error(exc)) from None


@app.post("/api/sessions/{name}/rename")
async def rename_session(name: str, request: Request):
    """Trocar o nome é do Rust; aqui chega só quem entrou por uma porta do Python (Connect, convidado
    com conta), e segue pela ponte."""
    return await _repasse_ao_rust(request, erro("rename_needs_rust_server", "trocar o nome da sessão precisa do servidor Rust"),
                                  f"/__hangar_server/claude/{urllib.parse.quote(name, safe='')}/rename")


class ThenLinkBody(_StrictBody):
    target: str = Field(min_length=1)
    text: str = Field(min_length=1)


@app.put("/api/sessions/{name}/then", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.delete("/api/sessions/{name}/then", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def clear_then_link(name: str):
    ThenLink(name).clear()
    return {"ok": True}


@app.delete("/api/sessions/{name}/queue/{entry_id}", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def descartar_da_fila(name: str, entry_id: str):
    # O botao "descartar" da bolha perdida: a entrada desistida sai da fila e do chat. So o id
    # (a bolha `queued-<id>` do front); `remove` recusa o que ainda esta por entregar.
    try:
        removed = await asyncio.to_thread(PromptQueue(name).remove, entry_id)
    except TransferInProgress:
        raise
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    if not removed:
        raise HTTPException(404, erro("erro_fila_entrada_nao_encontrada", "entrada não está na fila"))
    return {"ok": True}


async def _queue_unavailable():
    raise HTTPException(409, erro("queue_action_unavailable", "esta sessão não permite mexer na fila agora"))


# As três rotas do painel só existem para o repasse do `_codex_rust_port` (convite e Connect) ao Rust.
for _method, _path in (("PATCH", "/api/sessions/{name}/queue/{entry_id}"), ("POST", "/api/sessions/{name}/queue/reorder"),
                       ("POST", "/api/sessions/{name}/queue/{entry_id}/steer")):
    app.add_api_route(_path, _queue_unavailable, methods=[_method],
                      dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])


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
    from app.conversation_history import HistoryError
    try:
        etag = await asyncio.to_thread(historico_etag, name, info.jsonl, info.provider, limit)
    except (HistoryError, OSError) as exc:
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
    except (HistoryError, OSError) as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    # Cauda CRUA de proposito: o corte no 1o
    # user_msg (pra nao desenhar resposta orfa) e preferencia de RENDERIZACAO do card do quadro e vive
    # no BoardCard.svelte. Aplicado AQUI, valia pra todo consumidor e matava a espiada do hover da
    # Sidebar (HP_TAIL=8), que so quer o ultimo assistant_msg: com o proximo prompt ja mandado, o corte
    # jogava fora a resposta anterior -> latestAssistantEvent = None -> popover vazio, cacheado por 30s.
    if limit is not None and limit > 0:
        return evs[-limit:]
    return evs


@app.get("/api/sessions/{name}/cost", dependencies=[Depends(require_auth)])
async def codex_session_cost(name: str):
    info = await _cached_info(name)
    if not info or info.provider != "codex" or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "Codex session not found"))
    from app.session_cost import estimate_session_cost
    try:
        return await asyncio.to_thread(estimate_session_cost, info.jsonl)
    except OSError:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "Codex rollout not found")) from None


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
    if account_bridge.owner_mode() == "python":
        raise HTTPException(503, detail=erro("erro_subagentes_precisa_rust",
                                             "o servidor está sem o processo Rust; reinicie o Hangar"))
    return await account_bridge.forward_public(request, "/__hangar_server/subagents")


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str, request: Request):
    # handler async -> registry.list() (subprocess tmux) vai pro threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        diag.registrar("sse.recusado", "aviso", sessao=name,
                       detalhe="fora-da-lista" if not info else "sem-transcript")
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    # Retomada exata: o id que emitimos no transcript e "<stem-do-jsonl>:<offset-em-bytes>". Chega
    # por header (Last-Event-ID, que o browser reenvia sozinho quando o MESMO EventSource reconecta)
    # ou por query param (o app fecha e recria o EventSource no proprio retry, e objeto novo nunca
    # manda o header -> sem o param a retomada nunca dispararia no uso real).
    #
    # O STEM e obrigatorio e tem que bater com o transcript ATUAL: apos um /clear o jsonl e outro
    # arquivo, e honrar um offset do arquivo antigo daria seek no meio do novo, pulando calado todo
    # o inicio da conversa. Nao bateu (ou lixo) -> None, e o tail cai no backfill normal.
    raw = request.query_params.get("last_event_id") or request.headers.get("last-event-id")
    start_offset = None
    if raw:
        stem, _, off = raw.rpartition(":")
        if stem and stem == session_key(info.jsonl):
            try:
                start_offset = int(off)
            except ValueError:
                start_offset = None
    # provider da sessao (SessionInfo.provider, ja marcado por registry.list() -- tmux -> "claude",
    # sidecar Codex -> "codex") -> merged_events escolhe o Adapter certo (tail do jsonl, monitor de
    # estado, fonte do preview). Sem isto TODA sessao caia no default "claude" do merged_events e o
    # SSE do Codex nunca ligava (chat vazio, sem estado ao vivo).
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider=info.provider, start_offset=start_offset,
                      count_app=guest_of(request) is None),
        send_timeout=30)


def _erro_texto(e) -> str:
    """Texto de um erro de envio: string crua (endpoint antigo) ou o `msg` do envelope {code, params, msg}.

    Os avisos compostos (pareamento, group-message) interpolam o TEXTO, nunca o dict — o dict seria
    "[object Object]" no front antigo. O front novo recebe a estrutura via params e traduz por ela;
    o msg montado aqui e a rede para quem nao tem o codigo no mapa.
    """
    return e if isinstance(e, str) else (e.get("msg") if isinstance(e, dict) else str(e))


# Recados escritos no socket nativo, por msg_id: (remetente, alvo, começo do texto). Só serve pro
# recibo de retenção/recusa que o receptor devolve no inbox do backend (uds_messaging.INBOX).
_RECADOS_NATIVOS: dict[str, tuple[str, str, str]] = {}


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
        if _transfer_send_error(name):
            return None
        return _send_native_available(name, text)


def _send_native_available(name: str, text: str) -> Optional[str]:
    """msg_id se o recado foi ESCRITO no socket nativo do Claude de `name`; None = não é recado
    ([de:/grupo:/painel:]), sessão sem socket, ou o socket não aceitou — quem chama cai pro
    próximo degrau (plugin, tmux, fila). Só recado vai por aqui: a fala da pessoa continua
    entrando como prompt dela, nunca embrulhada como mensagem de outra sessão."""
    remetente, corpo = uds_messaging.separar_prefixo(text)
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
    _RECADOS_NATIVOS[mid] = (remetente, name, corpo[:80])
    if len(_RECADOS_NATIVOS) > 500:
        for k in list(_RECADOS_NATIVOS)[:100]:
            _RECADOS_NATIVOS.pop(k, None)
    return mid


def _ao_recibo_nativo(mid: str, estado: str, detalhe: str) -> None:
    """Recibo `peer_message_status` do receptor (retido/recusado): avisa a sessão remetente pelo
    caminho normal. Roda na thread do inbox; o envio vai pro loop do servidor."""
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.loop is not None:
        if run_sync(lambda: coordinator.native_receipt(mid, estado), coordinator.loop):
            diag.registrar("recado.nativo.recibo", "aviso", codigo=estado or "delivered")
            return
    info = _RECADOS_NATIVOS.pop(mid, None)
    diag.registrar("recado.nativo.recibo", "aviso", sessao=info[1] if info else None,
                   detalhe=f"{estado} {detalhe}".strip())
    if estado in ("delivered", "released", ""):
        return
    if not info or _loop_servidor is None:
        # Sem correlacao (recibo de antes do restart, ou msg_id ja purgado) ou sem loop: o aviso
        # nao tem pra quem ir, mas a recusa nao pode virar silencio.
        _log.warning("recibo nativo %s sem destino: msg_id=%s info=%s loop=%s detalhe=%s",
                     estado, mid, bool(info), _loop_servidor is not None, detalhe)
        return
    remetente, alvo, inicio = info
    if sanitize_session_name(remetente) != remetente:
        # Aviso do próprio app ([painel: …]): não há sessão remetente a avisar, a recusa fica no log.
        _log.warning("aviso do app %s por %s: %s (%r)", estado, alvo, detalhe, inicio)
        return
    aviso = (f"[painel: entrega de recado] Seu recado para {alvo} ({inicio!r}) foi {estado}"
             f"{': ' + detalhe if detalhe else ''}. Ele não chegou ao modelo de lá.")
    fut = asyncio.run_coroutine_threadsafe(_enviar(remetente, aviso), _loop_servidor)

    def _feito(f) -> None:
        # Remetente pode ter fechado entre o envio e o recibo: o aviso nao chega, mas fica no log.
        if (exc := f.exception()) is not None:
            _log.warning("aviso de recibo nao entregue a %s: %r", remetente, exc)
    fut.add_done_callback(_feito)


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


def _send_one(name: str, text: str, track_entry: bool = False) -> dict:
    from app.conversation_transfer import session_ingress, TransferError, public_error
    try:
        # A thread conserva a participação mesmo se o await HTTP for cancelado.
        with session_ingress(name):
            return _send_one_available(name, text, track_entry)
    except TransferError as exc:
        return {"ok": False, "error": public_error(exc), "delivered": False}


def _rust_mode(coordinator) -> bool:
    return coordinator is not None and getattr(coordinator, "mode", None) == "rust"


def _no_rust_binding_error() -> dict:
    # Com o Rust de pé a entrega é dele; cair no socket/plugin/tmux do Python escreveria sem a porta.
    return {"ok": False, "error": erro("erro_envio_falhou", "sessão Claude sem vínculo no Rust"), "delivered": False}


def _send_one_available(name: str, text: str, track_entry: bool = False) -> dict:
    if error := _transfer_send_error(name):
        return error

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
        if result != "sent" and (error := _transfer_send_error(name)):
            return error
        if result != "sent" and provider == "claude" and codex_sessions.exists(name):
            from app.conversation_transfer import TransferError, public_error
            return {"ok": False, "delivered": False,
                    "error": public_error(TransferError("session_transfer_source_changed"))}
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
        if error := _transfer_send_error(name):
            return error
        current = await _send_thread(codex_sessions.load, name)
        changed = source is not None and (current or {}).get("thread_id") != source.get("thread_id")
        if changed or not await _send_thread(_session_exists, name):
            return {"ok": False, "error": erro("erro_sessao_inexistente", "sessao nao encontrada")}
        return await _send_one_codex_locked(name, text, track_entry=track_entry)


async def _enviar(name: str, text: str) -> dict:
    from app.conversation_transfer import session_ingress, TransferError, public_error
    try:
        with session_ingress(name):
            operation = asyncio.create_task(_send_available(name, text))
            try:
                return await asyncio.shield(operation)
            except asyncio.CancelledError:
                try:
                    await operation
                finally:
                    raise
    except TransferError as exc:
        return {"ok": False, "error": public_error(exc), "delivered": False}


async def _send_available(name: str, text: str) -> dict:
    if error := _transfer_send_error(name):
        return error

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
        if error := _transfer_send_error(name):
            return error
        if not _headless(name):
            from app.conversation_transfer import TransferError, public_error
            return {"ok": False, "delivered": False,
                    "error": public_error(TransferError("session_transfer_source_changed"))}
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


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
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


@app.post("/api/sessions/{name}/steer", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def steer_session(name: str, body: InputBody | None = None):
    """Orienta o turno do Codex por RPC ou promove a fila da TUI (Kimi: ctrl-s; Claude: ctrl+x ctrl+s)."""
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, "sessão não encontrada")
    if _provider_of(name) == "codex":
        adapter = get_adapter("codex")
        try:
            if body is not None:
                await adapter.steer(name, body.text)
                return {"ok": True, "promoted": False}
            sent = await adapter.steer_queue(name)
            # O rollout confirma cada mensagem; não apaga ecos de envios concorrentes.
            return {"ok": True, "promoted": False, "confirmed": len(sent),
                    "queued_ids": ["queued-" + entry_id for entry_id in sent]}
        except (RuntimeError, ValueError):
            raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    if _headless(name):
        # Mesmo desenho do Codex: texto vai direto pro turno em voo; sem texto, promove a fila.
        adapter = get_adapter(CLAUDE_HEADLESS)
        try:
            if body is not None:
                await adapter.steer(name, body.text)
                return {"ok": True, "promoted": False}
            sent = await adapter.steer_queue(name)
            return {"ok": True, "promoted": False, "confirmed": len(sent),
                    "queued_ids": ["queued-" + entry_id for entry_id in sent]}
        except (RuntimeError, ValueError) as e:
            # ValueError = o ator recusou (sem turno em voo); sem este ramo virava 500.
            raise HTTPException(409, detail=erro("erro_sem_turno", str(e))) from None
        except OSError as e:
            # Processo morreu entre a checagem e a escrita: a mensagem continua na fila.
            raise HTTPException(502, detail=erro("erro_envio_falhou", f"o processo não recebeu: {e}")) from None
    provider, _ = await _send_thread(_pane_info, name)
    if provider == "claude":
        from app import runtime_coordinator
        from app.runtime_terminal import route
        owner = runtime_coordinator.current()
        if owner is not None and getattr(owner, "legacy", None) is not None:
            try:
                result = await route(owner, name, {"kind":"control", "control":"steer", "payload":{}})
                confirmed = await owner.op(name, {"kind":"confirm"}, uuid.uuid4().hex) if result is not None else None
            except TerminalControlError:
                raise       # o handler do app responde 409
            except RuntimeError as e:
                # Falha do runtime (não recusa do controle): erro com código, não 500 genérico.
                raise HTTPException(502, detail=erro("erro_envio_falhou", str(e), erro=str(e))) from None
            if result is not None:
                return {"ok":True, "promoted":result["disposition"] == "accepted" and
                    (result.get("payload") or {}).get("promoted", True), "confirmed":confirmed.get("confirmed", 0)}
    # `is False` e nao `not ...`: o unico produtor de False e o tmux recusando a tecla; um dublê de
    # teste que devolve None nao pode virar erro. Sem esta checagem a rota afirmava entrega de um
    # ctrl-s que nunca saiu (pane morto) — o chip sumia da tela e a msg ficava parada na fila.
    r = await _send_thread(terminal_input.steer_now, name, provider)
    if r is False:
        raise HTTPException(502, "o terminal recusou a tecla — a mensagem continua na fila")
    if r == "sem-fila":
        # A bolha "na fila" existia mas a TUI nao tinha o marcador (a msg ja entrou no turno por
        # outra via, ou o wire ainda nao flushou): NAO confirma nada — quem decide e o reconcile
        # do transcript, nunca um carimbo sobre promocao que nao aconteceu.
        return {"ok": True, "promoted": False}
    n = await _send_thread(PromptQueue(name).confirm_delivered)
    return {"ok": True, "promoted": True, "confirmed": n}


class NavBody(_StrictBody):
    url: str = Field(min_length=1)


_PAGINA_RELATIVA = re.compile(r"^/api/sessions/([^/?#]+)/pages/([A-Za-z0-9_-]{1,128})$")


def _url_pagina_propria(name: str, u: str) -> str:
    """Rascunho de `html_render` vem como caminho sem token: completa com o endereço local do
    servidor e o token do dono, como os links de arquivo. Outro caminho relativo é recusado."""
    m = _PAGINA_RELATIVA.match(u)
    if m is None or urllib.parse.unquote(m.group(1)) != name:
        raise HTTPException(400, "caminho relativo só vale para página desta sessão")
    from app.rust_server import listen_addr
    # Mesmo endereço do pi_inbox: bind em toda interface inclui loopback; IP de LAN só escuta nele.
    bind = resolve_bind_ip(settings)
    host = "127.0.0.1" if bind in ("0.0.0.0", "::") else bind
    return f"http://{listen_addr(host, settings.port)}{u}?token={urllib.parse.quote(settings.auth_token, safe='')}"


@app.post("/api/sessions/{name}/nav", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def abrir_nav_sessao(name: str, body: NavBody):
    """O AGENTE abre o navegador embutido da própria sessão (CLI `hangar-preview open <url>`).

    O backend não cria view — quem cria é o shell desktop, avisado pelo evento 'nav' que sai no
    SSE da sessão E no da lista (este o desktop mantém aberto o tempo todo, então funciona com a
    sessão fora da tela). O marcador fica até o desktop confirmar (DELETE) ou vencer o prazo."""
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, "sessão não encontrada")
    u = body.url.strip()
    if u.startswith("/"):
        u = _url_pagina_propria(name, u)
    elif not re.match(r"^https?://", u, re.I):
        u = "http://" + u
    await asyncio.to_thread(nav_pendente, name, u)   # grava em disco: fora do loop
    return {"ok": True}


@app.delete("/api/sessions/{name}/nav", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def confirmar_nav_sessao(name: str):
    """O shell desktop criou o view da sessão: o marcador 'nav' sai, e nenhuma outra conexão o
    recebe de novo."""
    await asyncio.to_thread(nav_confirmar, name)
    return {"ok": True}


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


class PairBody(_StrictBody):
    # peer (1) OU peers (N) — peers vence; peer fica por compat (hangar-send --pair manda um só).
    peer: str = ""
    peers: list[str] = []
    task: str = ""
    replace_task: bool = False
    # Sem efeito: veterano nunca é avisado. Fica porque o corpo é estrito e o vigia ainda o manda.
    notify_members: bool = True
    # Grupo de orquestração: o kick-off de cada papel já diz canal, contrato e branch, então o
    # pareamento não entrega nada a ninguém (nem protocolo, nem entrada) e o hook reinjeta a versão curta.
    orq: bool = False


def _bridge_reply(out: dict) -> JSONResponse:
    """`{status, body}` de uma rota do Rust, devolvido como veio."""
    status = out.get("status")
    if type(status) is not int:
        raise _group_unavailable("groups_bridge_invalid")
    return JSONResponse(out.get("body"), status_code=status)


def _groups_need_rust() -> dict:
    return erro("groups_need_rust_server", "os grupos precisam do servidor Rust, que não está rodando")


def _require_rust_groups() -> None:
    if not groups_bridge.rust_owns_groups():
        raise HTTPException(503, detail=_groups_need_rust())


async def _group_route(method: str, name: str, route: str, body: BaseModel | None = None):
    """Grupos são do Rust; aqui chega só o que entrou pela 8768 e segue num sentido. Sem o Rust não
    há reserva Python."""
    _require_rust_groups()
    data = body.model_dump() if body is not None else None
    try:
        out = await asyncio.to_thread(groups_bridge.call, "group.route", method=method, name=name, route=route,
                                      body=data, facts={})
    except groups_bridge.GroupsBridgeError as e:
        raise _group_unavailable(e.code) from None
    return _bridge_reply(out)


@app.post("/api/sessions/{name}/pair", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pair_session(name: str, body: PairBody):
    """Junta `name` e peer(s) num GRUPO de trabalho (une os grupos existentes de todos) e injeta
    em CADA membro o prompt do grupo atualizado — a partir daí trocam recados via hangar-send por
    iniciativa própria, dentro do escopo da tarefa. Badge `pair_peers` aparece na lista."""
    return await _group_route("POST", name, "pair", body)


class PairRemoteBody(_StrictBody):
    initiator: str        # 'srv::nome' — quem iniciou o pareamento, na máquina remota
    task: str = ""


@app.post("/api/sessions/{name}/pair-remote", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pair_remote(name: str, body: PairRemoteBody):
    """Lado RECEPTOR do pareamento cross-server: registra `name` (sessão LOCAL) pareada ao iniciador
    remoto `body.initiator` (srv::nome) e injeta o protocolo. NÃO chama de volta (o iniciador já
    registrou o próprio lado — chamar de volta recursaria). Chamado só pelo backend do outro server
    via peers.call, autenticado pelo token do peers.json."""
    return await _group_route("POST", name, "pair-remote", body)


class UnpairRemoteBody(_StrictBody):
    peer: str             # 'srv::nome' que saiu do pareamento, na máquina remota


@app.post("/api/sessions/{name}/unpair-remote", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def unpair_remote(name: str, body: UnpairRemoteBody):
    """`name` (local) tinha um par remoto que saiu — remove o vínculo local e avisa. Idempotente
    (sair de quem não está pareado é no-op). Chamado pelo backend peer no unpair do outro lado."""
    return await _group_route("POST", name, "unpair-remote", body)


class GroupMsgBody(_StrictBody):
    text: str
    # O hangar-send diz se o REMETENTE tem socket ($CLAUDE_CODE_MESSAGING_SOCKET); o backend
    # decide o resto: peer local com inbox = caminho nativo alcança os dois lados = não digita
    # nele, devolve em `pulados` pro modelo mandar por SendMessage. Mesmo critério do 1:1.
    remetente_nativo: bool = False
    forcar_tmux: bool = False


@app.post("/api/sessions/{name}/group-message", dependencies=[Depends(require_auth), Depends(_transfer_check)])
async def group_message(name: str, body: GroupMsgBody):
    """Aviso pro GRUPO todo (hangar-send --group): entrega o texto a CADA companheiro de `name` numa
    tacada, como `[grupo: <name>]`. Unidirecional por contrato (o prompt instrui a NUNCA responder
    um [grupo:] com --group) — é o que impede o loop de N sessões se avisando em cascata.
    Slash-command fora (mesmo racional do /broadcast)."""
    return await _group_route("POST", name, "group-message", body)


@app.get("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
async def pair_contract(name: str):
    """Contrato compartilhado do GRUPO (markdown que os membros editam via fs; keyed pelo gid —
    estável quando membro entra/sai). 404 sem grupo; content vazio se ainda não existe."""
    return await _group_route("GET", name, "contract")


class PairContractBody(_StrictBody):
    content: str
    mtime: str | None = None


@app.put("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
async def pair_contract_put(name: str, body: PairContractBody):
    """Grava o contrato com o `mtime` lido; o Rust decide, aqui só o repasse da 8768."""
    return await _group_route("PUT", name, "contract", body)


@app.delete("/api/sessions/{name}/pair", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def unpair_session(name: str):
    """`name` SAI do grupo (os demais membros continuam entre si; grupo restante de 1 dissolve).
    Avisa quem saiu e quem ficou. Idempotente. Aviso que falhar NÃO refaz o vínculo (fora do grupo
    é o estado desejado) — só reporta no result."""
    return await _group_route("DELETE", name, "pair")


# Quanto esperar o picker sumir da tela depois do Escape, antes de digitar a resposta por texto.
_FECHA_PICKER_TIMEOUT = 3.0


def _espera_picker_fechar(name: str, timeout: float = _FECHA_PICKER_TIMEOUT) -> bool:
    """Espera o overlay (picker/menu) sair do pane depois de um Escape. True se saiu.

    Sem isto, o Escape e a digitacao saiam juntos e o texto era ENGOLIDO pela TUI que ainda estava
    fechando o picker — a resposta do usuario sumia e a bolha ficava presa no fim do chat pra sempre
    (medido em 13/08/2026 numa sessao Kimi: `result=sent` no log e o texto nunca no wire.jsonl).
    O gate normal (`_wait_input_ready`) nao pega este caso no Pi/Kimi: os marcadores de "TUI pronta"
    la sao pedacos de moldura (`─ ╰ │`), e o proprio picker desenha moldura — a primeira leitura ja
    devolve True com o picker ainda em tela.

    Estourou o prazo: devolve False e quem chama envia mesmo assim (nao piora o caso de hoje) —
    mesma politica do _wait_input_ready."""
    from app import tmux                      # import local: mesmo padrao das rotas vizinhas
    from app.state import _FOOTER_RE
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        # O PANE INTEIRO, nao o `is_overlay` (que so olha as 8 ultimas linhas): pergunta longa, com
        # muitas opcoes ou tela de Review, empurra o rodape de navegacao pra fora dessa janela e o
        # `is_overlay` responde False com o picker AINDA aberto — furo ja medido noutro consumidor
        # (tests/test_askquestion.py: "is_overlay e falso p/ AskUserQuestion"). Aqui os dois erros
        # custam coisas MUITO diferentes: falso-negativo devolve a corrida que esta funcao existe pra
        # matar; falso-positivo (a frase citada na conversa) so gasta o timeout e envia do mesmo
        # jeito. Entao erra-se pro lado de esperar.
        if not _FOOTER_RE.search(tmux.capture_pane(name)):
            return True
        time.sleep(0.1)
    _log.warning("picker de %s nao fechou em %.1fs apos o Escape; enviando o texto assim mesmo",
                 name, timeout)
    return False


def _recusa_se_so_enfileirou(name: str, res: dict) -> None:
    """Plano B do /answer: o texto foi ACEITO pela fila mas NAO digitado (o gate recusou, porque o
    picker segue aberto). Ate 01/09/2026 os tres provedores devolviam ok=true aqui e o app pintava a
    bolha como enviada — a pessoa esperava por uma resposta que nunca sairia da fila, ja que a fila
    so drena quando a sessao deixa de aguardar e quem a segurava era a propria pergunta.

    Levanta 409 e NAO limpa o sidecar do hook: a pergunta continua aberta, e apaga-lo devolveria a
    sessao pra `idle` na lista (ver askquestion.pergunta_aberta)."""
    if res.get("delivered"):
        return
    _log.warning("resposta name=%s: texto do plano B ficou na fila, nao digitado", name)
    raise HTTPException(409, detail=erro(
        "erro_resposta_nao_entregue",
        "nao consegui responder por aqui — a pergunta segue aberta, responda no terminal"))


def _recusa_se_painel_aberto(name: str) -> None:
    # Com o Rust dono, pergunta pela ponte (HTTP): rota `async` chama por `asyncio.to_thread`.
    # Com o painel anexado, a janela do tmux esta no tamanho DELE (~120x20). Quem conta linha no
    # pane — o seletor de opcao, o stepper do AskUserQuestion (terminal_input.answer_questions /
    # answer_question_pi) e o model_picker (lista e troca de modelo, que dirige o /model contando
    # linhas do pane) — leria um pane truncado e escolheria errado.
    #
    # O termsock NAO importa `pty` no topo justamente pra este import funcionar no Windows.
    from app import termsock
    try:
        aberto = termsock.painel_aberto(name)
    except list_bridge.ListBridgeError as e:
        # Sem resposta do Rust não dá pra dizer que o painel está fechado; a ponte já foi ao diário.
        raise HTTPException(status_code=503, detail=erro(
            "erro_terminal_indisponivel", "nao consegui conferir o painel de terminal", detalhe=e.code))
    if aberto:
        raise HTTPException(status_code=409,
                            detail=erro("erro_terminal_aberto",
                                        "Terminal aberto nesta sessao. Feche o painel pra responder "
                                        "por aqui."))


_SEM_CONFIRMACAO = ("resposta enviada, mas nao deu pra confirmar a tempo — "
                    "confira na sessao antes de responder de novo")


def _falha_do_runtime(e: Exception) -> HTTPException:
    """O runtime não respondeu a uma escrita: 502 com código, como o hangar-server (antes era 500)."""
    return HTTPException(502, detail=erro("erro_envio_falhou", str(e), erro=str(e)))


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def select(name: str, body: SelectBody):
    from app.runtime_terminal import TerminalOutcomeUnknown, route_sync
    info = _cached_info_sync(name)
    # A rota do terminal só conhece o vínculo Claude: para outro provedor (Codex sem terminal
    # incluído) ela suspendia a escrita antes de chegar ao ramo dele.
    if getattr(info, "provider", "claude") == "claude":
        payload = {"option":body.option}
        _recusa_se_painel_aberto(name)
        try:
            routed = route_sync(name, {"kind":"control", "control":"select", "payload":payload})
        except TerminalControlError as e:
            # O ator disse "incerto" ou "recusei": são respostas diferentes de "adiado".
            if e.disposition == "unknown":
                raise HTTPException(409, detail=erro("erro_sem_confirmacao_resposta", _SEM_CONFIRMACAO)) from None
            if e.disposition == "rejected":
                diag.registrar("opcao.nao_convergiu", "erro", sessao=name, detalhe=str(e.code))
                raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu",
                    "não consegui marcar essa opção no terminal — tente de novo", detalhe=e.code or "rejected")) from None
            raise
        except TransferInProgress:
            raise
        except TerminalOutcomeUnknown as e:
            _log.warning("SELECT name=%s resultado incerto no terminal: %s", name, e)
            raise HTTPException(409, detail=erro("erro_sem_confirmacao_resposta", _SEM_CONFIRMACAO)) from None
        except RuntimeError as e:
            # Antes da entrega (vínculo, posse, Rust subindo): nada chegou ao pane.
            _log.warning("SELECT name=%s rota do terminal falhou: %s", name, e, exc_info=True)
            raise HTTPException(503, detail=erro("erro_opcao_nao_convergiu",
                "não consegui responder pelo terminal — opção NÃO enviada", detalhe=str(e))) from None
        if routed is not None:
            return {"ok": True}
    # Mesma guarda do /input — e aqui ela é a ÚNICA: a cadeia abaixo não sabe falhar. terminal.select
    # devolve None, send_keys descarta o returncode e tmux._run converte tmux morto/travado
    # (TimeoutExpired/OSError) num CompletedProcess(returncode=1) que ninguém lê. Sem isto, responder
    # uma opção de sessão morta digitava no vazio e a resposta era {"ok": true} — o catch do card
    # nunca disparava. (O fix de raiz em send_keys/_run é outro diff: interrupt/model_picker/
    # TerminalMirror também passam por lá.)
    _recusa_se_painel_aberto(name)
    if not _session_exists(name):
        raise HTTPException(404, detail=erro("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada"))
    codex_sem_terminal = getattr(info, "provider", "claude") == "codex" and getattr(info, "headless", False)
    if _headless(name) or codex_sem_terminal:
        # Opção = resposta ao pedido de permissão em aberto (1 permite, 2 nega), pelo stdin.
        if _loop_servidor is None or not _loop_servidor.is_running():
            raise HTTPException(503, detail=erro("erro_opcao_nao_convergiu", "servidor sem loop pra responder"))
        adapter = get_adapter("codex" if codex_sem_terminal else CLAUDE_HEADLESS)
        fut = asyncio.run_coroutine_threadsafe(adapter.select(name, body.option), _loop_servidor)
        try:
            ok = fut.result(timeout=15)
        except Exception as e:
            raise HTTPException(503, detail=erro("erro_opcao_nao_convergiu", f"não consegui responder: {e}"))
        if not ok:
            raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "nenhum pedido de permissão pendente"))
        return {"ok": True}
    try:
        terminal.select(name, body.option)
    except terminal_input.DriveError as e:
        # Cursor do picker nao convergiu pra opcao pedida: nada foi enviado (o Enter fica de fora).
        # 409 com texto na tela em vez de 500 calado — sem isso o toque some sem nenhum sinal.
        diag.registrar("opcao.nao_convergiu", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "não consegui marcar essa opção no terminal — tente de novo", detalhe=str(e)))
    return {"ok": True}


@app.post("/api/sessions/{name}/select/submit", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def select_submit(name: str):
    """Envia as opções JÁ MARCADAS de um picker de múltipla escolha.

    Existe porque marcar e enviar são coisas diferentes ali: pelo celular dava pra marcar e não
    dava pra enviar — a lista crua só oferecia Cancelar. Ver `terminal_input.submeter_multipla`
    pro caminho na TUI (aba Submit) e pro porquê de o Enter sozinho não servir.
    """
    _recusa_se_painel_aberto(name)
    if not _session_exists(name):
        raise HTTPException(404, detail=erro("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada"))
    try:
        terminal.submeter_multipla(name)
    except TerminalControlError as e:
        # Ator recusou: mesma resposta do DriveError. Incerto e adiado seguem o handler de TerminalControlError.
        if e.disposition != "rejected":
            raise
        diag.registrar("opcao.envio_falhou", "erro", sessao=name, detalhe=str(e.code))
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "não consegui enviar as opções marcadas — tente de novo", detalhe=e.code or "rejected")) from None
    except TransferInProgress:
        raise
    except terminal_input.DriveError as e:
        # Mesma política do /select: 409 com o motivo na tela, nunca 500 calado nem "ok" mentiroso.
        diag.registrar("opcao.envio_falhou", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "não consegui enviar as opções marcadas — tente de novo", detalhe=str(e)))
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    return {"ok": True}


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


@app.post("/api/sessions/{name}/plugin/press", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def plugin_press(name: str, body: PluginPressBody):
    """Clique num botão que um mod desenhou na faixa ou num painel, pedido pelo app."""
    raise HTTPException(503, detail=_MOD_SEM_RUST)


@app.post("/api/sessions/{name}/plugin/close", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def interrupt(name: str, clear: bool = False):
    # Codex: interrompe a propria TUI pelo tmux, mantendo celular e terminal no mesmo controlador.
    if _provider_of(name) == "codex":
        try:
            interrompeu = await get_adapter("codex").interrupt(name)
        except TransferInProgress:
            raise
        except (ValueError, RuntimeError):
            # Sem terminal o ator do Rust recusou ou não respondeu: código do Codex, não 500.
            if not _codex_sem_terminal(name):
                raise
            _log.warning("codex interrupt falhou name=%s", name, exc_info=True)
            raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
        if not interrompeu:
            raise HTTPException(409, detail=erro(
                "erro_codex_controle", "Não há turno Codex ativo para interromper."))
        return {"ok": True}
    if _headless(name):
        # Sem turno em voo não há o que interromper; responder ok seria fingir.
        try:
            interrompeu = await get_adapter(CLAUDE_HEADLESS).interrupt(name)
        except TransferInProgress:
            raise
        except (ValueError, RuntimeError) as e:
            # O ator recusou ou não respondeu: sem turno para interromper, com o motivo dele.
            raise HTTPException(409, detail=erro("erro_sem_turno", str(e))) from None
        if not interrompeu:
            raise HTTPException(409, detail=erro("erro_sem_turno", "Não há turno ativo para interromper."))
        return {"ok": True}
    # clear=True: alem de interromper, limpa o input (2o Esc). So o front com msg pendente passa isso —
    # garante input nao-vazio, evitando que o Esc-Esc abra o menu de rewind num input ja vazio.
    # terminal.interrupt e SYNC (tmux) -> threadpool pra nao bloquear o event loop (handler async agora).
    try:
        await asyncio.to_thread(terminal.interrupt, name, clear=clear)
    except (TerminalControlError, TransferInProgress):
        raise
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    return {"ok": True}


# A pergunta lateral é do Rust (`session_write/btw.rs`); pelo Connect o `_codex_rust_port` a repassa.
@app.api_route("/api/sessions/{name}/btw", methods=["GET", "POST"], dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def pergunta_lateral_precisa_do_rust(name: str):
    raise HTTPException(503, detail=erro("btw_needs_rust_server", "a pergunta lateral precisa do servidor Rust"))


def _normalize_rate_window(window: dict | None) -> dict | None:
    # RateLimitWindow (app-server) -> shape neutro do front: usedPercent/windowMins/resetsAt.
    # window None (secondary/credits costumam vir null) -> None, o front so mostra o que existe.
    if window is None:
        return None
    return {
        "usedPercent": window.get("usedPercent"),
        "windowMins": window.get("windowDurationMins"),
        "resetsAt": window.get("resetsAt"),
    }


@app.get("/api/sessions/{name}/limits", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def limits(name: str):
    # So Codex tem rate limits expostos pelo app-server (account/rateLimits/read) -- Claude tem o
    # proprio chip de rate-limit (status_line), fora do escopo aqui (regra de ouro: Claude intocado).
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_limits_so_codex", "limits so existe pra sessoes Codex"))
    snapshot = await get_adapter("codex").read_rate_limits(name)
    if snapshot is None:
        # app-server indisponivel/recusou -- resposta neutra (sem erro), o front so nao mostra nada.
        return {"primary": None, "secondary": None, "planType": None}
    return {
        "primary": _normalize_rate_window(snapshot.get("primary")),
        "secondary": _normalize_rate_window(snapshot.get("secondary")),
        "planType": snapshot.get("planType"),
    }


class CodexModelBody(_StrictBody):
    model: str
    effort: str | None = None


@app.get("/api/sessions/{name}/models", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def modelos_da_sessao_codex(name: str):
    # Task C: modelo + reasoning effort so pra Codex (via model/list) -- o /model do Claude e o
    # picker interativo dedicado (/model-effort), sem esta rota.
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_models_so_codex", "models so existe pra sessoes Codex"))
    adapter = get_adapter("codex")
    try:
        current = await adapter.read_settings(name)
    except RuntimeError:
        raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    return {"models": await adapter.list_models(name), "current": current}


@app.post("/api/sessions/{name}/model", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def set_codex_model(name: str, body: CodexModelBody):
    # A thread compartilha a escolha com a TUI, sem reiniciar o processo.
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_model_so_codex", "model so existe pra sessoes Codex"))
    try:
        await get_adapter("codex").set_model(name, body.model, body.effort)
    except RuntimeError:
        raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    return {"ok": True}


class CodexServiceTierBody(_StrictBody):
    service_tier: Literal["default", "priority"]


@app.post("/api/sessions/{name}/service-tier", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def set_codex_service_tier(name: str, body: CodexServiceTierBody):
    if _provider_of(name) == "claude":
        # O Fast do Claude reabre a sessão com o tier novo: é do Rust.
        raise registry_mod.SessionOpenNeedsRust()
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_fast_indisponivel", "Fast exige Codex ou Claude com GPT no CLIProxyAPI local"))
    try:
        tier = await get_adapter("codex").set_service_tier(name, body.service_tier)
    except (RuntimeError, ValueError, TimeoutError):
        raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    return {"ok": True, "service_tier": tier}


class CodexPermissionBody(_StrictBody):
    mode: str


async def _guard_permissao_codex(name: str) -> None:
    """Recusa dirigir o `/permissions` quando o pane nao pode receber comando AGORA.

    Turno em voo: o texto nao vira comando, cai no composer do Codex e o Enter o ENFILEIRA como
    mensagem — a troca de permissao viraria um "/permissions" mandado pro modelo ler. Quem sabe se
    ha turno e o app-server (`deliverable`); o guard do Claude ao lado (`_require_drivable`) compara
    dois quadros do spinner porque la nao ha essa fonte.

    Atalho BARATO, nao a palavra final: `deliverable` responde True quando o app-server nao tem a
    sessao (backend reiniciado, TUI ainda nao reconectada) — sem olhar a tela. Quem le o pane e o
    `_require_drivable` que `_abrir_picker_permissoes` chama, e e ele que cobre esse caso e o do
    menu aberto por cima.
    """
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_permissao_so_codex",
                                             "este modo de permissao so vale para sessoes Codex"))
    await asyncio.to_thread(_recusa_se_painel_aberto, name)
    if not await get_adapter("codex").deliverable(name):
        raise HTTPException(409, detail=erro("erro_permissao_ocupada",
                                             "a sessao esta trabalhando — espere ela terminar"))


def _codex_sem_terminal(name: str) -> bool:
    from app.adapters.codex import sessions as codex_sessions
    return bool((codex_sessions.load(name) or {}).get("headless"))


@app.get("/api/sessions/{name}/codex-permissions", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def permissoes_do_codex(name: str):
    if _codex_sem_terminal(name):
        if _provider_of(name) != "codex":
            raise HTTPException(400, detail=erro("erro_permissao_so_codex",
                                                 "este modo de permissao so vale para sessoes Codex"))
        return get_adapter("codex").permission_modes_sem_terminal(name)
    await _guard_permissao_codex(name)
    try:
        return await asyncio.to_thread(terminal.list_codex_permissions, name)
    except (PickerError, terminal.NaoDigitou) as exc:
        raise HTTPException(exc.status, detail=erro("erro_permissao_picker", exc.detail))


@app.post("/api/sessions/{name}/codex-permissions", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def trocar_permissao_do_codex(name: str, body: CodexPermissionBody):
    if _codex_sem_terminal(name):
        from app.adapters.codex.sem_terminal import Ocupada
        try:
            return await get_adapter("codex").set_permission_mode_sem_terminal(name, body.mode)
        except ValueError as exc:
            raise HTTPException(400, detail=erro("erro_permissao_picker", str(exc)))
        except Ocupada as exc:
            raise HTTPException(409, detail=erro("erro_permissao_ocupada", str(exc)))
        except RuntimeError as exc:
            raise HTTPException(503, detail=erro("erro_permissao_picker", f"não consegui reabrir o Codex: {exc}"))
    await _guard_permissao_codex(name)
    try:
        return await asyncio.to_thread(terminal.set_codex_permission, name, body.mode)
    except (PickerError, terminal.NaoDigitou) as exc:
        raise HTTPException(exc.status, detail=erro("erro_permissao_picker", exc.detail))


class CodexModeBody(_StrictBody):
    mode: Literal["default", "plan"]


@app.post("/api/sessions/{name}/codex/mode", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def set_codex_mode(name: str, body: CodexModeBody):
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_model_so_codex", "model só existe para sessões Codex"))
    try:
        return await get_adapter("codex").set_mode(name, body.mode)
    except RuntimeError:
        raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None


def _menu_de_implementar_plano(pane: str) -> bool:
    menu = menu_codex(pane)
    return bool(menu and menu[0] == "Implement this plan?"
                and menu[1][0].startswith("Yes, implement this plan"))


@app.post("/api/sessions/{name}/codex/plan/implement", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def implementar_plano_codex(name: str):
    if not _session_exists(name):
        raise HTTPException(404, detail=erro(
            "erro_sessao_opcao_nao_enviada", "sessão não encontrada — plano NÃO iniciado"))
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro(
            "erro_model_so_codex", "esta ação só existe para sessões Codex"))

    fim = time.monotonic() + 2.0
    while True:
        pane = tmux.capture_pane(name)
        if _menu_de_implementar_plano(pane):
            break
        if menu_codex(pane) is not None or time.monotonic() >= fim:
            raise HTTPException(409, detail=erro(
                "erro_codex_controle", "O seletor de implementação não está aberto na sessão."))
        time.sleep(0.05)

    try:
        terminal.select(name, 1, require_cursor=True)
    except terminal_input.DriveError as exc:
        diag.registrar("plano_codex.nao_convergiu", "erro", sessao=name, detalhe=str(exc))
        raise HTTPException(409, detail=erro(
            "erro_opcao_nao_convergiu", "não consegui iniciar o plano pelo terminal",
            detalhe=str(exc))) from None

    fim = time.monotonic() + 2.0
    while time.monotonic() < fim:
        pane = tmux.capture_pane(name)
        if pane and not _menu_de_implementar_plano(pane):
            return {"ok": True}
        time.sleep(0.05)
    raise HTTPException(409, detail=erro(
        "erro_codex_controle", "O Codex não fechou o seletor de implementação."))


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


@app.post("/api/sessions/{name}/keys", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def keys(name: str, body: KeyBody):
    # Uma tecla de navegacao (allowlist) pro pane — dirige overlays so-TUI a partir do espelho.
    try:
        terminal.send_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except (TerminalControlError, TransferInProgress):
        raise
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    return {"ok": True}


@app.post("/api/sessions/{name}/term-input", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def term_input(name: str, body: TermInputBody):
    # Terminal interativo (so desktop): manda texto digitado (literal) e/ou uma tecla nomeada pro pane.
    try:
        if body.text:
            terminal.send_text(name, body.text)
        if body.key:
            terminal.send_term_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except (TerminalControlError, TransferInProgress):
        raise
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    return {"ok": True}


def _session_info_with_cwd(name: str):
    # Mesmo lookup do upload. 404 se a sessão ou o cwd não existe.
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    return info


def _session_cwd(name: str) -> str:
    return _session_info_with_cwd(name).cwd


@app.get("/api/sessions/{name}/runners", dependencies=[Depends(require_auth)],
         response_model=RunnersResponse)
def list_runners(name: str):
    cwd = _session_cwd(name)
    return RunnersResponse(
        detected=runner.detect_runners(cwd),
        custom=runner.custom_commands(cwd),
        remembered=runner.remembered(cwd),
        running=runner.run_status(cwd),
    )


# POST, nao PUT/PATCH: mesmo motivo do /api/config — proxy na frente do backend ja barrou
# metodo fora do par GET/POST.
@app.post("/api/sessions/{name}/runners/custom", dependencies=[Depends(require_auth), Depends(_transfer_guard)],
          response_model=list[Runner])
def set_custom_runners(name: str, body: CustomRunnersBody):
    # A lista vai INTEIRA (add/editar/remover sao a mesma gravacao). Item vazio e recusado aqui,
    # apontando qual: gravado, ele viraria linha morta descartada calada no proximo GET.
    cwd = _session_cwd(name)
    for i, c in enumerate(body.commands, start=1):
        if not c.label.strip() or not c.command.strip():
            raise HTTPException(400, detail=erro(
                "erro_runner_custom", f"comando personalizado {i}: rotulo e comando sao obrigatorios"))
    runner.set_custom_commands(cwd, [c.model_dump() for c in body.commands])
    return runner.custom_commands(cwd)


@app.post("/api/sessions/{name}/run", dependencies=[Depends(require_auth), Depends(_transfer_guard)],
          response_model=RunInfo)
def start_runner(name: str, body: RunBody):
    # RunnerError = "o play NAO aconteceu" (a sessao velha sobreviveu, ou o new-session falhou).
    # Antes isso virava 200 com o estado da sessao VELHA dentro; 500 cru tambem nao serve, porque
    # o texto diz o que houve e a tela do run mostra o detail, igual ao painel de projetos.
    try:
        return runner.start_run(_session_cwd(name), body.command)
    except runner.RunnerError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/run/stop", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def stop_runner(name: str):
    try:
        runner.stop_run(_session_cwd(name))
    except runner.RunnerError as e:
        raise HTTPException(e.status, e.detail)   # `{"ok": True}` com o processo vivo era mentira
    return {"ok": True}


@app.get("/api/sessions/{name}/run/pane", dependencies=[Depends(require_auth)])
def runner_pane(name: str):
    return {"pane": runner.run_pane(_session_cwd(name))}


_DISPLAY_VARS = ("DISPLAY", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "HYPRLAND_INSTANCE_SIGNATURE")


def _shortcut_env() -> dict[str, str]:
    """Ambiente do atalho com as variaveis de tela do gerenciador systemd do usuario.

    A unit do backend sobe antes do compositor exportar DISPLAY/WAYLAND_DISPLAY, entao o
    ambiente herdado nao tem tela e programa grafico (xfreerdp3, editor) morre sem abrir janela.
    O que o processo ja tiver vence; systemctl ausente (Windows, container) = ambiente herdado."""
    env = os.environ.copy()
    if os.name == "nt":
        return env
    try:
        out = subprocess.run(["systemctl", "--user", "show-environment"],
                             capture_output=True, text=True, errors="replace", timeout=3).stdout
    except (OSError, subprocess.SubprocessError):
        return env
    for line in (out or "").splitlines():
        key, _, value = line.partition("=")
        if key in _DISPLAY_VARS and key not in env:
            env[key] = value
    return env


def _focus_hangar_terminal(row: dict | None) -> bool:
    if not row or not row["alive"] or not row["pid"]:
        return False
    from app import window_focus
    return window_focus.focus_tree(row["pid"], _shortcut_env())


@app.get("/api/sessions/{name}/shortcut-terminals", dependencies=[Depends(require_auth)])
def shortcut_terminals_list(name: str):
    from app import shortcut_terminals
    return {"terminals": shortcut_terminals.list_for(name)}


def _answer(target: str | None, text: str, missing: HTTPException):
    # Uma linha so, sem tecla de controle: `\n` viraria dois comandos, `\x03` um Ctrl+C, `\x1b` um Esc.
    if any(ord(c) < 0x20 or ord(c) == 0x7F for c in text):
        raise HTTPException(400, detail=erro("erro_shortcut_resposta_invalida", "a resposta e uma linha so"))
    if target is None:
        raise missing
    from app import terminal_prompt
    if not terminal_prompt.answer(target, text):
        raise HTTPException(500, detail=erro("erro_shortcut_resposta", "o terminal recusou a resposta"))
    return {"ok": True}


@app.post("/api/sessions/{name}/shortcut-terminals/{ident}/answer", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def shortcut_terminal_answer(name: str, ident: str, body: ShortcutAnswerBody):
    from app import shortcut_terminals
    return _answer(shortcut_terminals.find(name, ident), body.text,
                   HTTPException(404, detail=erro("erro_shortcut_terminal_inexistente",
                                                  "terminal do atalho nao encontrado")))


# POST, nao DELETE: o proxy da frente so deixa passar GET/POST.
@app.post("/api/sessions/{name}/shortcut-terminals/{ident}/close", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def shortcut_terminal_close(name: str, ident: str):
    from app import shortcut_terminals
    closed = shortcut_terminals.close(name, ident)
    if closed is None:
        raise HTTPException(404, detail=erro("erro_shortcut_terminal_inexistente",
                                             "terminal do atalho nao encontrado"))
    if not closed:
        raise HTTPException(500, detail=erro("erro_shortcut_terminal_fechar",
                                             "o terminal do atalho nao fechou"))
    return {"ok": True}


@app.get("/api/hangar-terminals", dependencies=[Depends(require_auth)])
def hangar_terminals_list():
    from app import shortcut_terminals
    return {"terminals": [t for t in shortcut_terminals.list_all() if not t["owner"]]}


def _hangar_404():
    return HTTPException(404, detail=erro("erro_hangar_terminal_inexistente", "terminal No Hangar nao encontrado"))


# POST, nao DELETE: o proxy da frente so deixa passar GET/POST.
@app.post("/api/hangar-terminals/{ident}/close", dependencies=[Depends(require_auth)])
def hangar_terminal_close(ident: str):
    from app import shortcut_terminals
    closed = shortcut_terminals.close_hangar(ident)
    if closed is None:
        raise _hangar_404()
    if not closed:
        raise HTTPException(500, detail=erro("erro_hangar_terminal_fechar", "o terminal No Hangar nao fechou"))
    return {"ok": True}


@app.post("/api/hangar-terminals/{ident}/answer", dependencies=[Depends(require_auth)])
def hangar_terminal_answer(ident: str, body: ShortcutAnswerBody):
    from app import shortcut_terminals
    return _answer(shortcut_terminals.find_hangar(ident), body.text, _hangar_404())


@app.post("/api/hangar-terminals/{ident}/focus", dependencies=[Depends(require_auth)])
def hangar_terminal_focus(ident: str):
    from app import shortcut_terminals
    row = shortcut_terminals.hangar_row(ident)
    if row is None:
        raise _hangar_404()
    return {"focused": _focus_hangar_terminal(row)}


class ShortcutImportBody(BaseModel):
    # O conteudo do arquivo: `{"version": 1, "shortcuts": [...]}` ou a lista crua.
    data: dict | list
    apply: bool = False
    # {id do atalho: {nome do marcador: valor}}. Nunca vai pro log.
    secrets: dict[str, dict[str, str]] = Field(default_factory=dict)


@app.get("/api/shortcuts/export", dependencies=[Depends(require_auth)])
def shortcuts_export(ids: list[str] | None = Query(default=None), include_scripts: bool = True):
    # Sem credencial: cada valor de segredo sai como marcador (app/shortcut_transfer.py).
    from app import shortcut_transfer
    try:
        return shortcut_transfer.export_payload([] if ids == [""] else ids, include_scripts=include_scripts)
    except ValueError as e:
        raise HTTPException(400, detail=str(e))


# POST: o import tem corpo e muda a config; o GET/POST e o par que o proxy da frente aceita.
@app.post("/api/shortcuts/import", dependencies=[Depends(require_auth)])
def shortcuts_import(body: ShortcutImportBody):
    from app import shortcut_transfer
    try:
        return shortcut_transfer.import_shortcuts(body.data, apply=body.apply, secrets=body.secrets)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_shortcut_import_invalido", str(e), motivo=str(e)))


class ShortcutVerifyBody(BaseModel):
    ids: list[str]
    # Caminhos dos scripts instalados pela importação, citados no pedido de correção.
    scripts: list[str] = Field(default_factory=list)


@app.post("/api/shortcuts/verify", dependencies=[Depends(require_auth)])
def shortcuts_verify(body: ShortcutVerifyBody):
    from app import shortcut_transfer
    return shortcut_transfer.run_checks(body.ids, body.scripts[:50], _shortcut_env())


# --- launcher de projetos (standalone, chaveado pelo projects.json — nao por sessao viva) ----

@app.get("/api/projects", dependencies=[Depends(require_auth)],
         response_model=list[ProjectStatus])
def projects_list():
    try:
        return projects.list_projects()
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/projects/{name}/start", dependencies=[Depends(require_auth)],
          response_model=ProjectStatus)
def project_start(name: str):
    try:
        return projects.start(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/projects/{name}/stop", dependencies=[Depends(require_auth)])
def project_stop(name: str):
    try:
        projects.stop(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True}


@app.get("/api/projects/{name}/pane", dependencies=[Depends(require_auth)])
def project_pane(name: str):
    try:
        return {"pane": projects.pane(name)}
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


class ProjectUpsert(BaseModel):
    name: str
    cwd: str
    command: str
    port: Optional[int] = None          # Pydantic coage "3000" (string do form QML) -> int
    stop_command: Optional[str] = None


@app.post("/api/projects", dependencies=[Depends(require_auth)], response_model=ProjectStatus)
def project_upsert(body: ProjectUpsert):
    try:
        return projects.upsert(body.name, body.cwd, body.command, body.port, body.stop_command)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.delete("/api/projects/{name}", dependencies=[Depends(require_auth)])
def project_delete(name: str):
    try:
        projects.remove(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True}


@app.post("/api/sessions/{name}/open-editor", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def open_editor(name: str):
    # So-desktop: abre o editor na MAQUINA do backend, no cwd da sessao. Binario fixo (settings.editor,
    # nao input do cliente) + arg unico validado -> sem shell, sem injecao. GUI precisa do DISPLAY/
    # WAYLAND_DISPLAY do backend (sessao grafica); sob systemd headless pode nao abrir -> 500.
    cwd = _session_cwd(name)
    binario = runtime_config.get("editor")
    # Rastro: com o editor editavel pelo app, um exec silencioso seria o caminho menos auditavel do
    # backend (o fluxo normal de comando fica gravado no transcript; este nao ficava em lugar nenhum).
    _log.info("OPEN-EDITOR name=%s bin=%r cwd=%r", name, binario, cwd)
    try:
        subprocess.Popen([binario, cwd],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError as e:
        raise HTTPException(500, detail=erro("erro_editor_falhou", f"editor '{binario}' falhou: {e}", binario=binario, erro=str(e)))
    return {"ok": True}


@app.get("/api/sessions/{name}/transcript-image/{uuid}/{idx}", dependencies=[Depends(require_auth)])
def transcript_image(name: str, uuid: str, idx: int):
    # Serve uma imagem colada no TERMINAL (base64 no .jsonl) sob demanda. Decodifica por uuid+idx.
    info = _cached_info_sync(name)
    jsonl = info.jsonl if info else None
    if not jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.conversation_history import transcript_image as conversation_image, HistoryError
    try:
        got = conversation_image(info, uuid, idx)
    except (HistoryError, OSError) as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    if got is None:
        raise HTTPException(404, detail=erro("erro_imagem_nao_encontrada", "image not found"))
    raw, media = got
    # Só imagem raster passa, como no Rust: SVG executa script e o resto não é imagem.
    media = media.lower() if media.lower() in {"image/png", "image/jpeg", "image/gif", "image/webp"} else "application/octet-stream"
    # immutable: o conteudo de um uuid+idx nunca muda -> cache agressivo no cliente.
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable",
                                                            "X-Content-Type-Options": "nosniff"})


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


# ── MCP hangar-computer-control: liga/desliga e configura nos .claude.json de todas as contas ──
class ComputerControlBody(_StrictBody):
    enabled: bool
    mode: Literal["package", "local"] | None = None   # None = mantém o modo atual
    project_dir: str = ""
    agent_config: str = ""
    # Cliente antigo: o modelo é o auxiliar da página Jev; igual passa, diferente volta erro.
    llm_url: str = ""
    llm_model: str = ""
    llm_effort: str = ""
    llm_key: str | None = None
    jev_key: str | None = None     # cliente antigo: vazio passa, chave digitada volta erro (o Jev vem da página dele)
    use_cliproxy_key: bool = False


def _computer_control_call(fn, *a):
    from app import computer_control as cc
    try:
        return fn(*a)
    except cc.ComputerControlError as e:
        raise HTTPException(e.status, detail=erro(e.code, e.msg, **e.params)) from None


@app.get("/api/computer-control", dependencies=[Depends(require_auth)])
def computer_control_get():
    from app import computer_control as cc
    return _computer_control_call(cc.state)


@app.put("/api/computer-control", dependencies=[Depends(require_auth)])
def computer_control_put(body: ComputerControlBody):
    from app import computer_control as cc
    return _computer_control_call(cc.save, body.model_dump())


class ComputerControlTargetBody(_StrictBody):
    project_dir: str
    name: str
    transport: Literal["ssh", "local"] = "ssh"
    host: str = ""
    proxy_command: str = ""
    request_timeout: int | None = Field(default=None, ge=1, le=600)


class ComputerControlTestHostBody(_StrictBody):
    host: str
    proxy_command: str = ""


@app.post("/api/computer-control/test-host", dependencies=[Depends(require_auth)])
def computer_control_test_host(body: ComputerControlTestHostBody):
    from app import computer_control as cc
    return _computer_control_call(cc.test_host, body.host, body.proxy_command)


@app.get("/api/computer-control/windows-setup", dependencies=[Depends(require_auth)])
def computer_control_windows_setup(host: str = ""):
    from app import computer_control as cc
    return _computer_control_call(cc.windows_setup, host)


@app.post("/api/computer-control/install", dependencies=[Depends(require_auth)])
def computer_control_install():
    from app import computer_control as cc
    return _computer_control_call(cc.install)


@app.post("/api/computer-control/targets", dependencies=[Depends(require_auth)])
def computer_control_new_target(body: ComputerControlTargetBody):
    from app import computer_control as cc
    return _computer_control_call(cc.create_target, body.model_dump())


class AnswerItem(_StrictBody):
    kind: str
    question_id: str | None = None
    indices: list[int] | None = None
    multi: bool = False
    value: str | None = None
    labels: list[str] = []
    type_index: int | None = None
    chat_index: int | None = None


class AnswerBody(_StrictBody):
    answers: list[AnswerItem]
    request_id: int | str | None = None
    interaction_id: str | None = None


def _askq_fallback_text(answers: list[dict], jsonl: str | None) -> str:
    """Monta a resposta em TEXTO pro fallback do AskUserQuestion (drive da TUI falhou): pareia cada
    answer com a pergunta do sidecar (mesma ordem — o stepper monta answers via questions.map) e vira
    linhas "pergunta → resposta". Sem sidecar, so as respostas. kind=chat nao vira linha (o usuario
    escolheu conversar — o Escape do fallback ja o poe no chat)."""
    questions = []
    if jsonl:
        askq = read_pending_askq(jsonl)
        if askq:
            questions = askq.questions
    lines = []
    for i, a in enumerate(answers):
        if a["kind"] == "option":
            resp = ", ".join(a.get("labels") or [])
        elif a["kind"] == "text":
            resp = a.get("value") or ""
        else:  # chat: sem resposta estruturada
            continue
        if not resp:
            continue
        q = questions[i].question if i < len(questions) else None
        lines.append(f"- {q} → {resp}" if q else f"- {resp}")
    if not lines:
        return ""
    return "Respondendo as perguntas (o seletor de opções falhou, vai por texto):\n" + "\n".join(lines)


def _askq_conversar_text(answers: list[dict], jsonl: str | None) -> str:
    """Texto do "Conversar sobre isso" com terminal: a TUI cancela o picker INTEIRO ao escolher
    "Chat about this" e descarta o que já foi respondido, então as respostas dadas viram mensagem em
    vez de sumir. Sem nada a preservar, devolve "" — aí o drive normal faz o certo pela opção nativa.
    """
    questions = []
    if jsonl:
        askq = read_pending_askq(jsonl)
        if askq:
            questions = askq.questions

    def pergunta(i: int) -> str | None:
        return questions[i].question if i < len(questions) else None

    respondidas: list[str] = []
    conversar: list[str] = []
    for i, a in enumerate(answers):
        if a["kind"] == "chat":
            q = pergunta(i)
            if q:
                conversar.append(q)
            continue
        resp = (a.get("value") or "") if a["kind"] == "text" else ", ".join(a.get("labels") or [])
        if not resp:
            continue
        q = pergunta(i)
        respondidas.append(f"- {q} → {resp}" if q else f"- {resp}")
    if not respondidas:
        return ""
    if len(conversar) == 1:
        linhas = [f"Sobre «{conversar[0]}» prefiro conversar antes de responder."]
    elif conversar:
        linhas = ["Sobre estas perguntas prefiro conversar antes de responder:"]
        linhas.extend(f"- {q}" for q in conversar)
    else:
        linhas = ["Sobre uma das perguntas prefiro conversar antes de responder."]
    linhas.append("Já respondi:")
    linhas.extend(respondidas)
    return "\n".join(linhas)


class SkipQuestionBody(_StrictBody):
    request_id: str
    interaction_id: str | None = None


@app.post("/api/sessions/{name}/question/skip", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def skip_question(name: str, body: SkipQuestionBody):
    if getattr(_cached_info_sync(name), "provider", "claude") != "codex":
        raise HTTPException(409, detail=erro("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente."))
    if _loop_servidor is None or not _loop_servidor.is_running():
        raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível confirmar o envio da resposta ao Codex."))
    future = asyncio.run_coroutine_threadsafe(
        get_adapter("codex").skip_question(name, body.request_id), _loop_servidor)
    try:
        future.result(timeout=10)
    except ValueError as exc:
        raise HTTPException(409, detail=erro("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.")) from exc
    except Exception as exc:
        future.cancel()
        _log.warning("Falha ao pular pergunta do Codex: %s", type(exc).__name__)
        raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível confirmar o envio da resposta ao Codex.")) from exc
    return {"ok": True}


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
def answer(name: str, body: AnswerBody):
    # Dirige o AskUserQuestion tabbed: reproduz as teclas (nav em malha fechada), confere o Review e
    # submete. Input invalido -> 409. Drive falhou (DriveError: nada submetido, sem Escape) ->
    # FALLBACK automatico: Escape (fecha o picker; o "declined" e intencional aqui) + resposta como
    # texto via _send_one (fila duravel: se o pane ainda estiver em overlay vira deferred e o drain
    # entrega). A resposta do usuario NUNCA se perde — pior caso chega como texto, nao como interrupt mudo.
    from app import terminal_input
    answers = [a.model_dump() for a in body.answers]
    info = _cached_info_sync(name)
    if getattr(info, "provider", "claude") == "claude" and not _headless(name):
        from app.runtime_terminal import answer_sync
        _recusa_se_painel_aberto(name)
        try:
            result = answer_sync(name, answers, body.request_id, getattr(info, "jsonl", None))
        except ValueError as exc:
            raise HTTPException(409, detail=erro("erro_sem_resposta", str(exc))) from exc
        except (TerminalControlError, TransferInProgress):
            raise
        except RuntimeError as exc:
            # Falha do runtime antes da resposta: 502 com código, como o hangar-server (antes era 500).
            raise _falha_do_runtime(exc) from None
        if result is not None:
            if getattr(info, "jsonl", None):
                clear_pending_askq(info.jsonl)
            return {"ok": True, "fallback":False}
    if getattr(info, "provider", "claude") == "codex":
        if _loop_servidor is None or not _loop_servidor.is_running():
            raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível confirmar o envio da resposta ao Codex."))
        future = asyncio.run_coroutine_threadsafe(
            get_adapter("codex").answer_questions(name, body.request_id, answers), _loop_servidor
        )
        try:
            future.result(timeout=35)
        except ValueError as exc:
            raise HTTPException(409, detail=erro("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.")) from exc
        except Exception as exc:
            _log.warning("Falha ao enviar resposta nativa ao Codex: %s", type(exc).__name__)
            raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível confirmar o envio da resposta ao Codex.")) from exc
        return {"ok": True, "fallback": False}
    if _headless(name):
        # AskUserQuestion respondida pelo stdin (`control_response` com `answers`), sem picker.
        if _loop_servidor is None or not _loop_servidor.is_running():
            raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível enviar a resposta."))
        fut = asyncio.run_coroutine_threadsafe(
            get_adapter(CLAUDE_HEADLESS).answer_questions(name, body.request_id, answers), _loop_servidor)
        try:
            fut.result(timeout=15)
        except ValueError as exc:
            raise HTTPException(409, detail=erro("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.")) from exc
        except Exception as exc:
            # O ator do Rust recusa a resposta inválida com este código: é recusa, não falha de envio.
            if getattr(exc, "code", None) == "claude_command":
                raise HTTPException(409, detail=erro("erro_codex_resposta_invalida", "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.")) from exc
            _log.warning("resposta ao Claude sem terminal falhou: %s", type(exc).__name__)
            raise HTTPException(503, detail=erro("erro_codex_resposta_envio", "Não foi possível enviar a resposta.")) from exc
        return {"ok": True, "fallback": False}
    _recusa_se_painel_aberto(name)
    jsonl = info.jsonl if info else None
    fallback = False


    texto_conversar = _askq_conversar_text(answers, jsonl) if any(
        a.get("kind") == "chat" for a in answers) else ""
    if texto_conversar:
        terminal.interrupt(name)      # Escape unico: fecha o picker (sem clear — input vazio)
        _espera_picker_fechar(name)
        res = _send_one(name, texto_conversar)
        if not res["ok"]:
            raise HTTPException(409, detail=erro("erro_drive_fallback_falhou",
                                                 f"nao deu pra entregar a resposta por texto: {_erro_texto(res['error'])}",
                                                 erro=res["error"]))
        _recusa_se_so_enfileirou(name, res)
        if jsonl:
            clear_pending_askq(jsonl)
        return {"ok": True, "fallback": True}
    try:
        terminal_input.answer_questions(name, answers)
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        text = _askq_fallback_text(answers, jsonl)
        _log.warning("ASKQ fallback name=%s reason=%s text=%r", name, e, text[:120])
        # Diario: o log do servico vive o que a maquina deixar viver (o journal do dia seguinte ja
        # nao tinha as duas quedas de 28/08/2026), e sem o MOTIVO nao da pra separar "picker preso"
        # de "nav drift" quando o relato chega dias depois.
        diag.registrar("pergunta.fallback_texto", "erro", sessao=name, detalhe=str(e))
        if not text:
            raise HTTPException(409, detail=erro(
                "erro_drive_sem_fallback",
                f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
        terminal.interrupt(name)  # Escape unico: fecha o picker (sem clear — input vazio)
        _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
        res = _send_one(name, text)
        if not res["ok"]:
            raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
        _recusa_se_so_enfileirou(name, res)
        fallback = True
    # Respondido: limpa o sidecar do hook pra um stale nao reabrir o stepper depois. Resolve o jsonl
    # igual aos outros endpoints; se nao resolver, pula a limpeza sem falhar a request.
    if jsonl:
        clear_pending_askq(jsonl)
    return {"ok": True, "fallback": fallback}


class ModelEffortBody(_StrictBody):
    # scope: 'session' (aperta `s`) ou 'default' (aperta Enter).
    model: str | None = None
    effort: str | None = None
    scope: Literal["session", "default"] = "session"


@app.post("/api/sessions/{name}/model-effort", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def model_effort(name: str, body: ModelEffortBody):
    info = await _cached_info(name)
    if info is not None and getattr(info, "provider", "claude") == "claude":
        raise HTTPException(503, detail=erro("model_effort_needs_rust_server", "trocar modelo e esforço precisa do servidor Rust"))
    await asyncio.to_thread(_recusa_se_painel_aberto, name)
    try:
        return await asyncio.to_thread(terminal.set_model_effort, name, body.model, body.effort, body.scope)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    except ValueError as e:
        raise HTTPException(422, str(e))


# ── Modo de permissão em sessão viva (Task 5) ─────────────────────────────────────────
# Leitura pelo rodapé do pane (⏸/⏵⏵) e troca via BTab (Shift+Tab). Medido em
# 2026-08-20: stdin da statusline não traz o modo, /permissions não aceita arg,
# BTab cicla 4 (plan/auto/manual/acceptEdits) ou 5 com bypassPermissions no arranque,
# dontAsk só no arranque e sai do ciclo. Ver docs/superpowers/specs/2026-08-19-medicao-permissao-viva.md
import app.permission_mode as perm_mode

class PermissionModeBody(_StrictBody):
    mode: str | None = None
    permission_mode: str | None = None

# Cache da lista viva por sessão (enquanto ela viver). Chave = "nome::jsonl" ou
# "nome::sem-jsonl" quando ainda sem transcript; valor = (current, modos).
_perm_modes_cache: dict[str, tuple[str, list[str]]] = {}

def _cache_key_perm(name: str, info) -> str:
    j = getattr(info, "jsonl", None) if info else None
    return f"{name}::{j or 'sem-jsonl'}"

def _tracking_key_perm(name: str, info) -> str:
    j = getattr(info, "jsonl", None) if info else None
    return Path(j).stem if j else name

def _guard_perm(name: str, info) -> None:
    """409 quando sessão não é claude, painel aberto, ou há menu aberto no pane."""
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if getattr(info, "provider", "claude") not in (None, "claude"):
        raise HTTPException(409, detail=erro("erro_permissao_so_claude", "modo de permissao so vale para claude"))
    _recusa_se_painel_aberto(name)
    # Sem `_require_drivable` aqui, de propósito: ele recusa sessão trabalhando porque `/model` é
    # TEXTO e cairia no campo. BTab é tecla e o Claude a aplica no meio do turno (medido). Só um
    # menu aberto engoliria a tecla.
    from app import tmux
    from app.state import is_overlay
    if not tmux.has_session(name):
        raise HTTPException(409, "sessao nao esta viva")
    try:
        pane = tmux.capture_pane(name)
    except Exception:
        pane = ""
    if pane and is_overlay(pane):
        raise HTTPException(409, "ha um menu aberto no terminal da sessao")

@app.get("/api/sessions/{name}/permission-modes", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def permission_modes(name: str, sondar: bool = False):
    """Lista dos modos de permissão.

    Sem sondar (default): só lê o modo atual via capture-pane (zero teclas) e devolve
    o cache de `modes` se já existir, ou [] — não sonda. Com `?sondar=1`: dá a volta
    completa de BTab, anota os modos, volta ao original e cacheia. `sondavel` diz se
    a sessão pode ser sondada (false quando current == dontAsk, que não tem volta).
    """
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if _headless(name):
        # Sem rodapé pra ler: o modo é o que o processo confirmou (ou o do sidecar, parada), e a
        # lista é a fechada da CLI — `set_permission_mode` aceita qualquer um, sem sondar.
        hl = get_adapter(CLAUDE_HEADLESS)
        from app.runtime_adapter import runtime_data
        if (view := runtime_data(name)) is not None:
            return {"current":view.get("permission_mode"), "modes":list(model_args.MODOS_PERMISSAO_CLAUDE),
                "sondavel":False, "previous_non_plan":view.get("previous_non_plan")}
        vivo = hl._sessions.get(name)
        meta = headless_sessions.load(name) or {}
        atual = (vivo.permission_mode if vivo and vivo.vivo else None) or meta.get("permission_mode")
        anterior = (vivo.modo_nao_plan if vivo and vivo.vivo else None) or meta.get("previous_non_plan")
        return {"current": atual, "modes": list(model_args.MODOS_PERMISSAO_CLAUDE), "sondavel": False,
                "previous_non_plan": anterior}
    await asyncio.to_thread(_guard_perm, name, info)
    key = _cache_key_perm(name, info)
    # leitura do atual sem tecla (bloqueador 1)
    try:
        cur_now = await asyncio.to_thread(perm_mode.ler_modo, name)
    except Exception:
        # Pane em transição e bug de parse caem no mesmo 409; sem o log os dois ficam
        # indistinguíveis pra quem for depurar.
        _log.debug("permission-modes: leitura do modo falhou em %s", name, exc_info=True)
        cur_now = None
    if cur_now is None:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", "não consegui ler o modo atual no rodapé"))
    tracking_key = _tracking_key_perm(name, info)
    cur_now, anterior_nao_plan = perm_mode.observar_ou_confirmado(
        tracking_key, cur_now, sessao=name)
    sondavel = cur_now != "dontAsk"
    if not sondar:
        # sem sondar: devolver cache se houver, ou []
        hit = _perm_modes_cache.get(key)
        if hit is not None:
            _, modos_cached = hit
            # revalida current mas mantém modos do cache
            return {"current": cur_now, "modes": modos_cached, "sondavel": sondavel,
                    "previous_non_plan": anterior_nao_plan}
        return {"current": cur_now, "modes": [], "sondavel": sondavel,
                "previous_non_plan": anterior_nao_plan}
    # com sondar=1: comportamento de antes (listar_modos + cache)
    # se não sondável (dontAsk), não chamar listar_modos (bloqueador 2)
    if not sondavel:
        return {"current": cur_now, "modes": [], "sondavel": False,
                "previous_non_plan": anterior_nao_plan}
    hit = _perm_modes_cache.get(key)
    if hit is not None:
        _, modos_cached = hit
        return {"current": cur_now, "modes": modos_cached, "sondavel": sondavel,
                "previous_non_plan": anterior_nao_plan}
    try:
        cur, modos = await asyncio.to_thread(
            perm_mode.executar_controlado, name, perm_mode.listar_modos, name)
    except RuntimeError as e:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", str(e)))
    # Chave é nome::jsonl, então sessão nova nunca reusa entrada: sem poda o dict cresce pela
    # vida do processo. ponytail: teto burro, o cache é só pra evitar re-sondar a mesma sessão.
    if len(_perm_modes_cache) > 200:
        _perm_modes_cache.clear()
    _perm_modes_cache[key] = (cur, modos)
    cur, anterior_nao_plan = perm_mode.observar_ou_confirmado(
        tracking_key, cur, sessao=name)
    # A sonda dá voltas de BTab de verdade. Se não conseguiu voltar, a sessão FICOU noutro modo de
    # permissão por causa de uma chamada que o usuário leu como leitura — isso não pode sair calado.
    restaurado = cur == cur_now
    if not restaurado:
        _log.warning("permission-modes: sonda deixou %s em %s (era %s)", name, cur, cur_now)
    return {"current": cur, "modes": modos, "sondavel": cur != "dontAsk",
            "restaurado": restaurado, "previous_non_plan": anterior_nao_plan}

@app.post("/api/sessions/{name}/permission-mode", dependencies=[Depends(require_auth), Depends(_transfer_guard), Depends(_codex_rust_port)])
async def permission_mode_set(name: str, body: PermissionModeBody):
    """Troca o modo de permissão via BTab até casar o alvo (teto 6 teclas).

    Devolve SEMPRE o modo que FICOU, nunca o pedido. Teto estourado ou alvo fora do
    ciclo → 409 com o modo que ficou. 409 também quando sessão não é claude,
    painel aberto, ou estado recusa digitação.
    """
    alvo = body.mode if body.mode is not None else body.permission_mode
    if not alvo:
        raise HTTPException(422, detail=erro("erro_permissao_invalida", "informe o modo desejado"))
    # valida contra lista fechada antes de qualquer efeito
    if alvo not in model_args.MODOS_PERMISSAO_CLAUDE:
        raise HTTPException(409, detail=erro("erro_permissao_invalida", f"permission_mode: use um de {', '.join(model_args.MODOS_PERMISSAO_CLAUDE)}"))
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if _headless(name):
        # `control_request set_permission_mode` no stdin: o processo responde com o modo que ficou.
        hl = get_adapter(CLAUDE_HEADLESS)
        try:
            ficou = await hl.set_permission_mode(name, alvo)
        except Exception as e:
            if alvo != "bypassPermissions":
                raise HTTPException(409, detail=erro("erro_permissao_leitura", f"não consegui trocar o modo: {e}"))
            _log.warning("permissão: %s recusou bypass sem reiniciar (%s); reabrindo em bypass", name, e)
            ficou = None
        if alvo == "bypassPermissions" and ficou != alvo:
            # Entrar em bypass reabrindo a sessão é do Rust.
            raise registry_mod.SessionOpenNeedsRust()
        vivo = hl._sessions.get(name)
        await asyncio.to_thread(_invalidate_lists)
        from app.runtime_adapter import runtime_data
        view = runtime_data(name)
        return {"mode": ficou, "current": ficou,
                "previous_non_plan":view.get("previous_non_plan") if view is not None else vivo.modo_nao_plan if vivo else None}
    await asyncio.to_thread(_guard_perm, name, info)
    if alvo == "bypassPermissions" and not await asyncio.to_thread(_bypass_no_ciclo, name):
        raise registry_mod.SessionOpenNeedsRust()
    tracking_key = _tracking_key_perm(name, info)
    try:
        inicial = await asyncio.to_thread(perm_mode.ler_modo, name)
    except Exception:
        inicial = None
    if inicial is not None:
        perm_mode.observar_ou_confirmado(tracking_key, inicial, sessao=name)
    try:
        ficou = await asyncio.to_thread(
            perm_mode.executar_controlado, name, perm_mode.trocar_modo, name, alvo)
    except RuntimeError as e:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", str(e)))
    except ValueError as e:
        raise HTTPException(409, detail=erro("erro_permissao_invalida", str(e)))
    finally:
        await asyncio.to_thread(_invalidate_lists)
    # cache da lista pode ter ficado com current velho; atualiza o current mas mantém modos
    key = _cache_key_perm(name, info)
    hit = _perm_modes_cache.get(key)
    if hit is not None:
        _, modos_cached = hit
        _perm_modes_cache[key] = (ficou, modos_cached)
    if ficou != alvo:
        raise HTTPException(status_code=409, detail=erro("erro_permissao_teto", f"não alcançou {alvo!r} em {perm_mode.TETO_TECLAS} teclas — ficou em {ficou!r}", alvo=alvo, ficou=ficou, mode=ficou))
    return {"mode": ficou, "current": ficou,
            "previous_non_plan": perm_mode.observar_modo(tracking_key, ficou)}


_FLAGS_BYPASS = ("--dangerously-skip-permissions", "--allow-dangerously-skip-permissions",
                 "--permission-mode bypassPermissions")


def _bypass_no_ciclo(name: str) -> bool:
    """O Claude só põe o bypass no Shift+Tab de quem nasceu com ele. Sem a flag no comando do
    processo, reabrir custa um restart; BTab às cegas giraria o modo sem chegar lá."""
    agent = registry_mod._pid_do_agente((registry._pane_of(name) or {}).get("pid"))
    cmd = " ".join(procinfo._cmdline(agent).split()).replace("--permission-mode=", "--permission-mode ") if agent else ""
    return any(f in cmd for f in _FLAGS_BYPASS)




@app.get("/api/sessions/{name}/navegador", dependencies=[Depends(require_auth)])
async def navegador_da_sessao(name: str, request: Request):
    request = Request({**request.scope, "path": f"/api/sessions/{urllib.parse.quote(name, safe='')}/navegador"}, request.receive)
    return await _repasse_ao_rust(request, erro("browser_need_rust_server", "o navegador precisa do servidor Rust, que não está rodando"),
                                  "/__hangar_server/forwarded")


@app.get("/api/sessions/{name}/commands", dependencies=[Depends(require_auth), Depends(_codex_rust_port)])
async def commands(name: str, request: Request):
    if _provider_of(name) == "codex":
        try:
            return [{"name": "compact", "display": "/compact", "source": "builtin",
                     "description": "Resume e compacta o contexto", "destructive": True},
                    *[{k: v for k, v in s.items() if k not in {"path", "native_name"}}
                      for s in await get_adapter("codex").list_skills(name)
                      if s["name"] != "compact"]]
        except RuntimeError:
            raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    return await _repasse_ao_rust(request, erro("commands_need_rust_server", "os comandos precisam do servidor Rust, que não está rodando"),
                                  "/__hangar_server/forwarded")
