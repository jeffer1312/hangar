import anyio.to_thread
import asyncio
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
import urllib.request
import uuid
from datetime import datetime
from contextlib import AsyncExitStack, asynccontextmanager, contextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Annotated, Literal, Optional
from fastapi import FastAPI, Depends, HTTPException, Query, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import FileResponse, JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from starlette.middleware.gzip import GZipMiddleware
from pydantic import BaseModel, ConfigDict, Field, PrivateAttr, StrictBool, model_validator
from sse_starlette.sse import EventSourceResponse
from app import (agentes_sync, atomico, atualizacoes, atualizar, btw, diag, harness_api,
                 loop_monitor, pensamento_pt, permission_mode, plugin_bridge, procinfo, quem_chama, tmux,
                 uds_messaging)
from app import external_pair_api, external_pairs, internal_api, list_bridge, migration_status, update_channel
from app.auth import require_auth, require_loopback
from app.send_executor import send_thread as _send_thread
from app import bastao as bastao_mod   # `bastao` sem sufixo é a ROTA GET, mais abaixo neste arquivo
from app.bastao import montar as bastao_montar
from app.commands import comandos_da_cli, list_commands
from app.fs import FsError, allowed_roots, list_roots, make_dir, scan_dir
from app.model_picker import PickerError
from app.mensagens import erro
from app import kimi_models
from app import claude_models
from app import claude_customizations
from app import cliproxy
from app import codex_models
from app import model_args
from app import filesearch, filetree, git_ops, worktrees
from app.file_response import file_response
from app.filesearch import SearchError
from app.filetree import FileError
from app import orq, orq_conductor, orq_context, orq_md, orq_papeis, orq_politica, orq_start, orq_timeline
from app import pi_catalog
from app import cli_probe
from app import pi_models
from app import pi_inbox
from app.pi_inbox import INBOX
from app import registry as registry_mod
from app.registry import KillFailed, SessionRegistry, sanitize_cwd
from app.names import sanitize_session_name
from app.models import (SessionInfo, CreatedSessionInfo, ChatEvent, CostReport, UsoReport, RunnersResponse, RunBody,
                        RunInfo, Runner, CustomRunnersBody, ProjectStatus, ShortcutShellBody, RunCodeBody,
                        ProjectShortcutsBody, ShortcutAnswerBody, session_key)
from app import uso_report
from app.planprog import (plan_progress, list_plans, write_pin, is_safe_stem, _plans_dir,
                          PlanPinError, PIN_NONE, marcar_step, arquivar, caminho_do_plano,
                          PlanWriteError)
from app.pqueue import (PromptQueue, _saida_local, _transcript_start_ts, committed_user_lines,
                        fila_interna_pendente, linha_mais_parecida)
from app.prune import prune_loop as _prune_loop
from app.renova_token import laco as _renova_token_loop
from app.chain import ThenLink
from app import terminal_input
from app.terminal_input import TerminalInput, drain
from app.adapters import CLAUDE_HEADLESS, get_adapter
from app.adapters.claude_headless import sessions as headless_sessions
from app.adapters.codex import sessions as codex_sessions
from app.adapters.orq import runs as orq_runs
from app.sse import invalidate_recent_list, merged_events, nav_confirmar, nav_pendente
from app.state import corrige_ocioso_kimi, forget_frame, menu_codex
from app.uploads import (save_upload, resolve_upload, resolve_session_audio, prune_old, list_uploads,
                         UploadError, MAX_BYTES)
from app.video import is_video, extract_frames, extract_audio
from app.transcribe import (transcribe, transcribe_with_provider, providers_status, Transcription,
                            TranscribeError, DICTATION_LIMITS, FILE_LIMITS)
from app.config import (list_config_dirs, ConfigDirInfo, _backend_config_base, settings,
                        resolve_scan_roots,
                        automations_enabled, resolve_bind_ip, variaveis_env)
from app import runtime_config
from app import share_api, share_guest_api, share_store
from app.share_guest_api import guest_safe
from app.guest_user_gate import GuestUserGate
from app.connect_port import ConnectPortGate
from app import guest_users, guest_users_api
from app.share_gate import GUEST_TOKEN_KEY, ShareGate, guest_of
from app.share_life import session_life
from app import tts
from app.tts_text import preparar as tts_preparar
from app import narrar
from app import contas, default_model, engine_probe, engines, procinfo
from app.costs import report as costs_report, usd_brl as _usd_brl, PERIODOS as _COST_PERIODOS
from app import costs_sources, pricing
from app.git_ops import (
    list_branches, switch_branch, create_worktree, remove_worktree, git_action, git_log, assign_lanes, changed_files, file_diff, discard_file, commit_files, commit_file_diff, commit_diff, revert_commit, cherry_pick, reset_to, create_branch_at, create_tag, diff_vs_worktree, branches_containing, commit, last_commit_message, push as push_branch, sequencer_state, GitError, branch_of, git_summary,
    folder_status, folder_fetch, folder_pull, folder_switch, folder_create_branch,
)
from app import loop as loop_mod
from app.transcript import last_assistant_text
from app import tunnel
from app import runner
from app import project_shortcuts
from app import projects
from app import archive_providers
from app.archive import (ArchiveEntry, ArchiveFolder, archive_cwd, archive_jsonl, conta_de,
                         list_conversations, list_folders, list_recent, move_conversation,
                         tail_events)
from app.search import SearchHit, search, extract_terms, search_terms, build_ask_prompt
from app.askquestion import clear_pending_askq, read_pending_askq
from app import pair
from app import pair_texto
from app import peers
from app import alcance, conta_estado, cotas, credenciais, peers_api
from app import config_sync_api
from app import codex_contas as codex_accounts
from app import codex_contas_api
from app.codex_contas_login import CodexContasLogin, codex_session_alive
from app.pair import PairLink, contract_path_for
from app.hook_state import hook_state
from app import push
from app import stall_watch
from app.omp_plugin_sync import PluginSynchronizer, PluginSyncLoop
from app.sync import sync_admin_router, sync_router
from app.deploy import deploy_router
from app import desktop_palette
from app import plano_claude

_log = logging.getLogger("hangar")


_codex_live_leases: dict[str, dict] = {}


def _codex_lease_released(name: str) -> None:
    state = _codex_live_leases.pop(name, None)
    if state is not None:
        state["lease"].release()


def _codex_lease_renamed(old: str, new: str) -> None:
    state = _codex_live_leases.pop(old, None)
    if state is not None:
        state["renaming"] = True
        state["revision"] = state.get("revision", 0) + 1
        state["name"] = new
        _codex_live_leases[new] = state


def _codex_lease_rename_finished(name: str) -> None:
    state = _codex_live_leases.get(name)
    if state is not None:
        state["renaming"] = False
        state["revision"] = state.get("revision", 0) + 1


async def _watch_codex_lease(state: dict) -> None:
    def session_alive(name):
        state["lease"].retire_birth()
        return tmux.has_session(name)

    try:
        while True:
            if state.get("renaming"):
                await asyncio.sleep(0.05)
                continue
            name = state["name"]
            revision = state.get("revision", 0)
            try:
                alive = await asyncio.to_thread(session_alive, name)
            except Exception:
                alive = True
            if (state["name"] != name or state.get("revision", 0) != revision
                    or state.get("renaming")):
                continue
            if not alive:
                break
            await asyncio.sleep(1)
    finally:
        current = _codex_live_leases.get(state["name"])
        if current is state:
            _codex_live_leases.pop(state["name"], None)
        state["lease"].release()


async def _hold_codex_lease(name: str, lease) -> None:
    from app.account_lifecycle import complete_on_cancel
    await complete_on_cancel(asyncio.to_thread(lease.mark_live, name))
    state = {"name": name, "lease": lease, "renaming": False, "revision": 0}
    _codex_live_leases[name] = state
    task = asyncio.create_task(_watch_codex_lease(state), name=f"codex-lease-{name}")
    tasks = getattr(app.state, "codex_creation_tasks", None)
    if tasks is None:
        tasks = set()
        app.state.codex_creation_tasks = tasks
    tasks.add(task)
    task.add_done_callback(tasks.discard)


def _codex_service():
    return getattr(app.state, "codex_contas_login", None)


def _resolve_codex_account(account_id: str | None):
    try:
        return codex_accounts.resolve_account(account_id or "default")
    except codex_accounts.AccountError as exc:
        raise HTTPException(exc.status, detail=erro(exc.code, "conta Codex inválida", **exc.params)) from None


def _codex_require_idle_preparation(account, service) -> None:
    if account.is_default:
        return
    if service is None:
        raise HTTPException(503, detail=erro("codex_account_service_unavailable",
                                             "serviço de contas Codex indisponível"))
    status = service.preparation_status(account)
    if status.get("status") != "ready":
        _log.warning("conta Codex %s com sincronização %s; pendências: %s",
                     account.id, status.get("status"),
                     [issue.get("code") for issue in status.get("issues", [])])


def _codex_account_in_use(account) -> bool:
    """Consulta sessões Codex vivas sem alterar a identidade do processo do backend."""
    from app.adapters.codex import sessions as codex_sessions
    wanted = account.home.expanduser().resolve(strict=False)
    for info in registry.list():
        if getattr(info, "provider", None) != "codex":
            continue
        meta = codex_sessions.load(info.name) or {}
        if not codex_session_alive(info.name, meta):
            continue
        selected = getattr(info, "codex_home", None)
        if selected and Path(selected).expanduser().resolve(strict=False) == wanted:
            return True
        rollout = getattr(info, "jsonl", None)
        if rollout:
            owner = codex_accounts.account_for_rollout(Path(rollout))
            if owner is not None and owner.id == account.id:
                return True
    return False


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
    try:
        # Em thread: a publicação sonda o `claude` (versão e flags) e não pode segurar o laço.
        await asyncio.to_thread(plugin_bridge.publish_address)
    except OSError:
        _log.warning("plugin: endereço da ponte não gravado; sessão de terminal fica no tmux",
                     exc_info=True)
    await _boot_sessions(runtime)
    _state_dirs =list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    hook_state.on_awaiting = _on_awaiting  # transicao -> awaiting_input dispara web push
    hook_state.on_transition = _on_hook_transition  # drain server-side + confirmacao de entrega
    task = asyncio.create_task(hook_state.watch(_state_dirs))

    def _watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("hook_state.watch crashed", exc_info=exc)

    task.add_done_callback(_watch_done)

    stall_task = asyncio.create_task(stall_watch.watch())

    def _stall_watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("stall_watch.watch crashed", exc_info=exc)

    stall_task.add_done_callback(_stall_watch_done)

    loop_monitor_task = asyncio.create_task(loop_monitor.watch(), name="loop-monitor")

    # Poda periodica dos sidecars de sessao morta (Task G3): varre na subida e depois a cada
    # 24h — ver app/prune.py para o criterio conservador (chave de sessao nao viva + idade
    # minima de 7 dias) e o porquê de periodica em vez de so no startup.
    # Renovação de token das contas PARADAS (Task de 18/08). Sem ela, conta que você não abre há
    # dias fica com o accessToken vencido: a cota dela some da faixa do rodapé e, no limite do prazo
    # do refresh (~26 dias), a conta pede login de novo. Abrir a sessão é o que renova — medido.
    renova_task = asyncio.create_task(_renova_token_loop())

    def _renova_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("renova_token.laco crashed", exc_info=exc)

    renova_task.add_done_callback(_renova_done)

    fetch_task = asyncio.create_task(_fetch_loop())

    def _fetch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("_fetch_loop crashed", exc_info=exc)

    fetch_task.add_done_callback(_fetch_done)

    auto_update_task = asyncio.create_task(_auto_update_loop())

    def _auto_update_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("_auto_update_loop crashed", exc_info=exc)

    auto_update_task.add_done_callback(_auto_update_done)

    prune_task = asyncio.create_task(_prune_loop())

    def _prune_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("prune.prune_loop crashed", exc_info=exc)

    prune_task.add_done_callback(_prune_done)

    # Primeira varredura na subida já religa o túnel se há convite ativo.
    share_task = asyncio.create_task(share_api.sweep_loop(), name="share-sweep")
    pair_sweep_task = asyncio.create_task(_pair_sweep_loop(), name="pair-sweep")

    # Boot-resume dos loops: flags em memoria (tick em voo) morrem no restart; o sidecar e a verdade.
    # Loop ACTIVE cuja sessao existe e esta idle -> reagenda o tick; sessao sumida -> failed.
    def _boot_resume_loops() -> None:
        try:
            live = {loop_mod._sanitize(i.name): i for i in registry.list()}
            for p in loop_mod._loop_dir().glob("*.json"):
                stem = p.stem
                link = loop_mod.LoopLink(stem)
                d = link.get()
                if not d or d["status"] not in loop_mod.ACTIVE:
                    continue
                info = live.get(stem)
                if info is None:
                    loop_mod._end(link, stem, "failed", "sessão morta no boot", push.notify_loop)
                    continue
                m = hook_state.get_state(session_key(info.jsonl)) if info.jsonl else None
                if m and m[0] == "idle":
                    loop_mod.schedule_tick(info.name, lambda n=info.name: _loop_ctx(n))
        except Exception as e:
            # Com o Rust ainda subindo a lista levanta; os loops ativos ficam sem reagendar.
            _log.warning("boot-resume de loops falhou", exc_info=True)
            diag.registrar("loop.boot_resume_falhou", "erro",
                           codigo=getattr(e, "code", None) or type(e).__name__)

    # Em segundo plano: com o Rust esperado, a lista espera o desfecho dele (até 30 s), e a subida
    # do servidor não pode ficar presa nisso.
    app.state.boot_resume_task = asyncio.create_task(asyncio.to_thread(_boot_resume_loops))
    pricing.atualizar_em_background()  # NUNCA num request: o cliente aborta em 4s
    # Mesmo motivo, outra rede: usd_brl() tem cache de 1h e timeout de 3s, e é chamado DENTRO do
    # montar(). Sem aquecer aqui, o primeiro /api/costs depois de todo restart paga a coleta fria
    # (657ms medidos) MAIS até 3s de câmbio, contra o AbortSignal.timeout(4000) do cliente.
    threading.Thread(target=_usd_brl, name="usd-brl-warm", daemon=True).start()
    # Catálogo do pi/omp em fundo: a primeira lista de modelos depois do restart levava segundos.
    pi_catalog.warm()
    # Primeira coleta de custos/uso desta subida, em background e só depois de o boot assentar:
    # máquina nova varre 1 GB+ de transcript sem ninguém ter clicado, e a tela já abre pronta.
    costs_sources.agendar_aquecimento(30)
    from app import transcript_index
    transcript_index.start_background()
    # A linha vive no loop do servidor, mas o send_prompt roda em thread — ver pi_inbox.entregar_sync.
    INBOX.ligar_loop(asyncio.get_running_loop())
    # Mesmo motivo, outro caminho: o drain do Codex e assincrono (app-server) e quem o chama sao
    # threads (Timer da confirmacao, gatilho de hook). Ver `_drenar`.
    global _loop_servidor
    _loop_servidor = asyncio.get_running_loop()
    if runtime.mode == "python":
        _start_transfer_recovery()
    codex_warm_task = asyncio.create_task(get_adapter("codex").watch_sessions())
    from app.codex_integracao import SERVICO as integracao_codex
    codex_contas_login = CodexContasLogin(
        account_in_use=_codex_account_in_use,
        atualizar_principal=integracao_codex.atualizar_e_aguardar,
    )
    app.state.codex_contas_login = codex_contas_login
    cotas.registrar_codex_auth_cache(codex_contas_login.cached_auth)
    # Referência guardada: task sem dono pode ser coletada no meio.
    app.state.codex_auth_aquecer = asyncio.create_task(codex_contas_login.aquecer())
    app.state.codex_creation_tasks = set()
    omp_sync = PluginSyncLoop(
        PluginSynchronizer(home=Path.home(), claude_dir=_backend_config_base()),
        enabled=settings.omp_plugin_sync_enabled,
        interval=settings.omp_plugin_sync_interval,
        permitted=automations_enabled,
    )
    app.state.omp_plugin_sync = omp_sync
    await omp_sync.start()
    from app import mcp_server
    # Em tarefa: o download dos binários na primeira vez não pode segurar a subida. Criada colada no
    # `try`: falha no meio da subida não pode deixar o frpc e o Caddy sem quem os pare.
    from app import connect as connect_mod
    connect_task = asyncio.create_task(connect_mod.start(), name="connect-start")
    try:
        async with mcp_server.lifespan():
            yield
    finally:
        diag.registrar("backend.encerrando")
        await runtime.shutdown()
        await runtime.close_events()
        connect_task.cancel()
        await asyncio.gather(connect_task, return_exceptions=True)
        await connect_mod.stop()
        costs_sources.cancelar_aquecimento()
        # Claude sem terminal fica vivo no cano: só fecha a conexão; o próximo backend religa.
        if _transfer_recovery is not None:
            await asyncio.shield(_transfer_recovery)
        get_adapter(CLAUDE_HEADLESS).desligar_todas()
        codex_warm_task.cancel()
        app.state.codex_auth_aquecer.cancel()
        await asyncio.gather(codex_warm_task, app.state.codex_auth_aquecer, return_exceptions=True)
        creation_tasks = list(getattr(app.state, "codex_creation_tasks", ()))
        for creation_task in creation_tasks:
            creation_task.cancel()
        if creation_tasks:
            await asyncio.gather(*creation_tasks, return_exceptions=True)
        await codex_contas_login.close()
        cotas.registrar_codex_auth_cache(None)
        try:
            await integracao_codex.fechar()
        except Exception:
            _log.exception("Falha ao encerrar a integração Codex")
        task.cancel()
        stall_task.cancel()
        loop_monitor_task.cancel()
        prune_task.cancel()
        share_task.cancel()
        pair_sweep_task.cancel()
        renova_task.cancel()
        await omp_sync.close()
        try:
            await task
        except asyncio.CancelledError:
            pass
        try:
            await stall_task
        except asyncio.CancelledError:
            pass
        await asyncio.gather(loop_monitor_task, return_exceptions=True)
        try:
            await prune_task
        except asyncio.CancelledError:
            pass
        # Esperar, e não só cancelar: a rodada de renovação roda em to_thread e abre uma janela
        # tmux que só morre no `finally` dela. Sair sem esperar deixaria a janela órfã justo no
        # restart do backend, que aqui é rotina.
        try:
            await renova_task
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


def _transfer_send_error(name: str) -> dict | None:
    from app.conversation_transfer import transfer_active, TransferError, public_error
    if transfer_active(name):
        return {"ok": False, "error": public_error(TransferError("session_transfer_busy")), "delivered": False}
    return None


app = FastAPI(title="hangar", lifespan=_lifespan)

from app.runtime_terminal import TerminalControlError


@app.exception_handler(TerminalControlError)
async def terminal_control_failed(request: Request, exc: TerminalControlError):
    code = "erro_sem_resposta" if exc.control == "answer_questions" else "erro_opcao_nao_convergiu"
    return JSONResponse(status_code=409, content={"detail":erro(code, str(exc))})


from app.runtime_coordinator import TransferInProgress


@app.exception_handler(TransferInProgress)
async def _ownership_moving(request: Request, exc: TransferInProgress):
    """Posse passando entre Python e Rust (sessão recém-criada, por exemplo): espera curta, não 500."""
    return JSONResponse(status_code=409, content={"detail":erro("session_transfer_busy", str(exc))})


@app.exception_handler(GitError)
async def _git_failed(request: Request, exc: GitError):
    """GitError que escapou da rota (citação, resolver) sai com o status dele, nunca 500 sem corpo.

    Falha da ponte de Git/arquivos traz o código (`workspace_busy`...), que o front traduz."""
    code = getattr(exc, "code", None)
    detail = erro(code, exc.detail, motivo=exc.detail) if code else exc.detail
    return JSONResponse(status_code=exc.status, content={"detail": detail})


@app.get("/api/omp/plugin-sync", dependencies=[Depends(require_auth)])
async def omp_plugin_sync_status(request: Request):
    service = getattr(request.app.state, "omp_plugin_sync", None)
    if service is None:
        return {"enabled": settings.omp_plugin_sync_enabled,
                "state": "idle" if settings.omp_plugin_sync_enabled else "disabled",
                "interval": settings.omp_plugin_sync_interval, "last_report": None}
    return service.status()


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


@app.exception_handler(list_bridge.ListBridgeError)
async def _lista_indisponivel(request: Request, exc: list_bridge.ListBridgeError):
    """Com o Rust dono, a lista vem dele; ponte fora ou Rust ainda subindo é 503 com o código, nunca
    a lista vazia nem a do Python (`_mux_indisponivel`, pelo mesmo motivo)."""
    diag.registrar("lista.indisponivel", "erro", codigo=exc.code, detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(status_code=503, content={"detail": erro(
        "erro_lista_indisponivel", "a lista de sessões está indisponível", detalhe=exc.code)})


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
    try:
        response = await call_next(request)
        return response
    except Exception as exc:
        failure = type(exc).__name__
        raise
    finally:
        elapsed = int((time.monotonic() - started) * 1000)
        status = response.status_code if response is not None else 500
        # O template da rota não contém query, caminho de arquivo nem corpo do pedido.
        route = getattr(request.scope.get("route"), "path", "(rota desconhecida)")
        # O long-poll do plugin espera de propósito: sucesso dele seria uma linha "lenta" a cada
        # janela, e enchia o teto do dia.
        long_poll_ok = route == "/api/plugin/pull" and status < 400 and not failure
        # O Rust chama `/internal/*` a cada tique: o POST que deu certo e rápido enchia o teto do dia de madrugada.
        internal_ok = request.url.path.startswith("/internal/") and status < 400 and not failure and elapsed < 1000
        if (response is not None or failure) and not long_poll_ok and not internal_ok and not request.url.path.startswith("/api/diag") and (
                failure or status >= 400 or elapsed >= 1000 or request.method in ("POST", "PUT", "PATCH", "DELETE")):
            diag.registrar("api.servidor", "erro" if status >= 500 else "aviso" if status >= 400 else "ok",
                           detalhe=f"{request.method} {route}", codigo=str(status), ms=elapsed,
                           etapa="cabecalhos", sessao=request.path_params.get("name"), erro_tipo=failure)
        diag.req_atual.reset(token)


# Porteiro da porta do convidado ANTES do CORS: o CORS o envolve, entao ate o 403/410 dele sai com
# Access-Control-Allow-Origin e o app do convidado le o codigo em vez de "erro de rede".
app.add_middleware(ShareGate)
app.add_middleware(GuestUserGate)
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
    expose_headers=["ETag"],
)
# JSON e assets grandes cruzam LAN/VPN; o Starlette exclui `text/event-stream`, sem segurar o SSE.
app.add_middleware(GZipMiddleware, minimum_size=1024, compresslevel=5)
# Por fora de tudo (o último registrado é o mais externo): os porteiros e o CORS já veem o cliente
# trocado de quem chegou pela porta do Connect.
app.add_middleware(ConnectPortGate)
app.include_router(sync_admin_router)
app.include_router(sync_router)
app.include_router(guest_users_api.router)
app.include_router(deploy_router)
# Roteadores por assunto (Task 1 do plano descoberta-e-configuracao): cada Task do lote escreve
# só no módulo dela. Última edição de api.py deste plano.
app.include_router(alcance.alcance_router)
app.include_router(conta_estado.conta_estado_router)
app.include_router(cotas.cotas_router)
app.include_router(credenciais.credenciais_router)
app.include_router(codex_contas_api.codex_contas_router)
app.include_router(harness_api.harness_router)
app.include_router(peers_api.peers_router)
app.include_router(update_channel.router)
app.include_router(plugin_bridge.plugin_router)
app.include_router(config_sync_api.config_sync_router)
app.include_router(share_api.router)
app.include_router(share_guest_api.router)
app.include_router(external_pair_api.router)
app.include_router(migration_status.router)
app.include_router(internal_api.router)
registry = SessionRegistry()
registry_mod.apos_saida_codex = _codex_lease_released
registry_mod.apos_renomear_codex = _codex_lease_renamed
terminal = TerminalInput()

# Teto de mensagem: o _BodySizeLimitMiddleware ignora scope != http de propósito (api.py:83), então
# a rota WebSocket nasceria sem limite nenhum. 256 KiB cobre com folga qualquer texto de chat.
_WS_MAX = 256 * 1024
# Heartbeat DO SERVIDOR, não eco do cliente: socket aberto não prova que tem alguém lendo (extensão
# com laço travado, notebook suspenso). Sem isto, uma linha zumbi faria toda mensagem pagar o
# PRAZO_ACK inteiro antes de cair pro fallback — a lentidão que o caminho de tecla não tinha.
_WS_PING = 20.0
# Teto de heartbeats SEGUIDOS sem resposta antes de fechar. `send_json` sozinho não pega o zumbi que
# o comentário acima descreve: com o laço de eventos travado (ou notebook suspenso), o buffer TCP do
# SO absorve o ping sem erro nenhum — é exatamente esse caso que o heartbeat existe pra cobrir. Dois
# pings perdidos (~2×_WS_PING) é rápido o bastante pra não atrasar a decisão "linha ou tecla" da
# Task 4, e generoso o bastante pra não fechar por uma rajada de latência isolada.
_WS_PINGS_SEM_RESPOSTA_MAX = 2


# Aviso-uma-vez-ate-mudar da recusa de conexao (achado ALTA da revisao 02/08/2026): sem isto, um
# token girado / bind mudado / firewall no meio faz TODA tentativa de retry da extensao (laco com
# recuo, hangar-state.ts) virar linha de log — e a mesma enxurrada que o retry em si tenta evitar do
# lado dela. Mesma politica de terminal_input._avisa_deferred/_limpa_deferred: WARNING na 1a recusa
# por host, calado ate uma conexao daquele host DAR CERTO (o que também reabre o aviso se a falha
# voltar depois — nao e "avisa uma vez na vida do processo").
_ws_origem_avisada: set[str] = set()
_ws_token_avisado: set[str] = set()


# Única rota WebSocket do backend. Quem LIGA é a extensão do Pi; o backend nunca procura ninguém —
# é isso que faz o custo ser zero pra quem não tem Pi.
# Auth pela query e não por header: é o mesmo caminho que o SSE já usa (auth.py:86-94), e cliente
# WebSocket não manda Authorization de forma portável.
@app.websocket("/api/pi/inbox")
async def pi_inbox_ws(ws: WebSocket):
    host = ws.client.host if ws.client else ""
    # bind_host: o endereco em que o PROPRIO uvicorn subiu (resolve_bind_ip == main.py). Com
    # CP_LAN_BIND_IP=auto ou IP fixo de LAN (modo celular documentado), o processo nao escuta em
    # loopback -- so aceitar 127.0.0.1 fechava a linha do Pi em silencio pra sempre (achado da
    # revisao final). Aceitar TAMBEM o bind continua seguro: uma conexao TCP com origem igual ao
    # endereco que o proprio host bindou so acontece self-connect (a mesma maquina falando com uma
    # interface dela mesma) -- um host remoto na LAN nunca aparece aqui com ESSE endereco de
    # origem, porque a origem eh o IP DELE, nao o do servidor (TCP nao deixa forjar isso).
    if host not in ("127.0.0.1", "::1", "localhost", resolve_bind_ip(settings)):
        # A defesa real é esta. Em loopback o token não protege de quem já está logado na máquina
        # (o próprio auth.py:42-46 registra isso), mas conexão de FORA não tem o que fazer aqui.
        # Achado ALTA da revisao 02/08/2026: ate aqui a recusa era MUDA — nem no terminal do Pi nem
        # no log do backend sobrava rastro de por que a linha rapida nunca ligava.
        if host not in _ws_origem_avisada:
            _ws_origem_avisada.add(host)
            _log.warning("pi_inbox: origem recusada host=%s (fora do bind aceito) — linha do Pi "
                         "vai continuar caindo pra tecla (aviso unico ate mudar)", host)
        await ws.close(code=1008)
        return
    if not settings.auth_token or not secrets.compare_digest(
            ws.query_params.get("token", ""), settings.auth_token):
        # Mesmo achado: inconsistente com auth.py:75, que loga toda virada de bloqueio de token —
        # aqui nao logava NADA. Token girado / sidecar desatualizado ficava indistinguivel de
        # "extensao nao instalada".
        if host not in _ws_token_avisado:
            _ws_token_avisado.add(host)
            _log.warning("pi_inbox: token recusado host=%s — linha do Pi vai continuar caindo pra "
                         "tecla (aviso unico ate acertar)", host)
        await ws.close(code=1008)
        return
    # Conectou: qualquer recusa ANTERIOR deste host era um estado velho — se voltar a falhar depois,
    # merece aviso de novo (nao e "avisou uma vez na vida do processo, calado pra sempre").
    _ws_origem_avisada.discard(host)
    _ws_token_avisado.discard(host)
    await ws.accept()
    pane = ""
    linha = None
    try:
        # Texto cru primeiro, igual ao loop abaixo: receive_json() direto pularia o teto de
        # tamanho pra ESTA mensagem (achado da revisão — só as do loop passavam pelo len(bruto)).
        bruto = await asyncio.wait_for(ws.receive_text(), _WS_PING)
        if len(bruto) > _WS_MAX:
            _log.warning("pi_inbox: primeira mensagem de %d bytes — fechando", len(bruto))
            await ws.close(code=1009)
            return
        primeira = json.loads(bruto)
        # `chave` e o que a extensao declara como identidade da sessao: o nome do psmux quando
        # existe, o pane quando nao (tmux). Extensao ANTIGA nao manda o campo e cai no pane, que e
        # exatamente o comportamento de antes — ninguem precisa dar /reload pra continuar
        # funcionando no Linux. Ver pi_inbox: no psmux o pane e `%1` em TODA sessao, e por isso a
        # linha da segunda sessao Pi tomava o lugar da primeira.
        pane = str(primeira.get("chave") or primeira.get("pane") or "")
        if not pane:
            await ws.close(code=1008)
            return
        linha = INBOX.registrar(pane, ws.send_json)
        _log.info("pi_inbox: linha aberta chave=%s", pane)
        pings_sem_resposta = 0
        while True:
            try:
                bruto = await asyncio.wait_for(ws.receive_text(), _WS_PING)
            except asyncio.TimeoutError:
                # Silêncio: cobra sinal de vida. Se o socket estiver morto, o send levanta e a
                # linha cai aqui mesmo, em vez de ficar registrada como viva pra sempre. Mas o send
                # sozinho não pega o zumbi (buffer do SO absorvendo o ping) — daí o contador: sem
                # NENHUMA resposta por _WS_PINGS_SEM_RESPOSTA_MAX rodadas, fecha por conta própria.
                pings_sem_resposta += 1
                if pings_sem_resposta > _WS_PINGS_SEM_RESPOSTA_MAX:
                    _log.warning("pi_inbox: %d heartbeats sem resposta pane=%s — linha zumbi, "
                                 "fechando", pings_sem_resposta, pane)
                    await ws.close(code=1000)
                    return
                await ws.send_json({"ping": True})
                continue
            # Qualquer mensagem é sinal de vida, não só o pong: zera antes de olhar o conteúdo.
            pings_sem_resposta = 0
            if len(bruto) > _WS_MAX:
                _log.warning("pi_inbox: mensagem de %d bytes pane=%s — fechando", len(bruto), pane)
                await ws.close(code=1009)
                return
            msg = json.loads(bruto)
            if msg.get("pong") or msg.get("ping"):
                continue
            msg_id = str(msg.get("id") or "")
            if not msg_id:
                continue
            if "resposta" in msg:
                # Resposta de PERGUNTA (leitura), nao confirmacao de entrega — chaves diferentes
                # de proposito: a extensao responde `{id, resposta}` e nunca `ok`, entao uma
                # mensagem jamais cai nos dois caminhos. `None` explicito (a extensao dizendo "nao
                # sei") chega como None e o backend cai no plano B; string vazia e resposta.
                valor = msg.get("resposta")
                INBOX.responder(pane, msg_id, valor if isinstance(valor, str) else None)
                continue
            INBOX.confirmar(pane, msg_id, bool(msg.get("ok")), msg.get("erro"))
    except WebSocketDisconnect:
        pass
    except Exception as e:
        _log.warning("pi_inbox: linha caiu pane=%s: %r", pane, e)
    finally:
        if linha is not None:
            INBOX.remover(pane, linha)


@app.websocket("/api/sessions/{name}/term")
async def term_ws_route(ws: WebSocket, name: str):
    # Sem a trava de loopback do /api/pi/inbox: aquela existe porque quem liga la e uma extensao
    # LOCAL. Aqui o celular vai precisar entrar de fora na fase 2.
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
    from app.share_gate import guest_of
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
    await navsock.nav_ws(ws, name, _session_exists)


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


_PAIR_SWEEP_S = 2.0


def _pair_sweep_list() -> list[SessionInfo]:
    """No Rust, o retrato: a descoberta dele não traz as linhas de transferência nem as `orq`
    (vêm dos fatos), e um nome fora dela seria dado como morto."""
    return list_bridge.snapshot() if registry_mod.rust_owns_list() else _guardar_snap()


async def _pair_sweep_loop() -> None:
    """Pares mortos fora do app, fora da descoberta (`registry.sweep_pairs`). Lista que falha não
    vira "ninguém vivo": a rodada não varre, e a falha vai ao diário uma vez por sequência."""
    failing = None
    while True:
        try:
            await asyncio.to_thread(registry.sweep_pairs, _pair_sweep_list)
            if failing is not None:
                diag.registrar("pares.varredura_voltou", codigo=failing)
            failing = None
        except Exception as e:
            code = getattr(e, "code", None) or type(e).__name__
            if code != failing:
                _log.warning("varredura de pares: lista indisponível (%s); os grupos ficam", code)
                diag.registrar("pares.varredura_falhou", "aviso", codigo=code)
            failing = code
        await asyncio.sleep(_PAIR_SWEEP_S)


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


def _notify_async(session_id: str, send_fn) -> None:
    """Resolve uuid->nome e manda o push escolhido, TUDO numa thread: registry.list() mexe no tmux
    e o webpush e rede — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            name = next(
                (s.name for s in registry.list() if s.jsonl and session_key(s.jsonl) == session_id),
                None,
            )
            if name:
                send_fn(name)
        except Exception:
            _log.warning("push falhou (%s)", session_id, exc_info=True)
    threading.Thread(target=_work, daemon=True).start()


def _awaiting_body(info) -> str:
    """Corpo rico da notif de awaiting (feature #5): 1) a pergunta do AskUserQuestion nativo (sidecar
    gravado pelo hook PreToolUse); 2) senao a pergunta lida do PANE (classify — cobre pickers/permissao
    da TUI, que nao passam pelo AskUserQuestion); 3) None se nenhuma deu certo — o push (app.push)
    resolve o fallback no idioma da inscricao, em vez de mandar texto fixo em pt."""
    askq = read_pending_askq(info.jsonl) if info.jsonl else None
    if askq and askq.questions:
        return askq.questions[0].question
    if getattr(info, "headless", False):
        return info.question or None   # sem pane: a pergunta/permissão vem do processo, já na lista
    if info.name:
        from app import tmux
        from app.state import classify
        try:
            _, _, question, _ = classify(tmux.capture_pane(info.name))
        except Exception:
            question = None
        if question:
            return question
    return None  # fallback resolvido pelo push, no idioma da inscricao


_AWAITING_PUSH_RETRY_S = 1.5  # Notification chega junto do pedido; o menu pode atrasar um frame


def _pane_wants_input(name: str) -> bool:
    """Pane mostra menu (classify awaiting) ou overlay de teclas — algo REAL esperando resposta.
    Falha de captura -> True (erro de leitura nao pode segurar um push legitimo)."""
    from app import tmux
    from app.state import classify, is_overlay
    try:
        pane = tmux.capture_pane(name)
    except Exception:
        _log.warning("_pane_wants_input falhou name=%s", name, exc_info=True)
        return True
    return classify(pane)[0] == "awaiting_input" or is_overlay(pane)


def _do_notify_awaiting(session_id: str) -> None:
    """Logica sincrona de _on_awaiting: resolve nome+corpo rico e manda pro push (que decide
    mute/quiet-hours/coalescing). Extraida da thread pra ficar testavel direto, sem mockar Thread.

    Gate anti-fantasma: o state_hook mapeia QUALQUER Notification pra awaiting — inclusive a de
    "idle ha 60s" do Claude Code, que chega DEPOIS do Stop com a sessao apenas parada. Sem o gate,
    toda sessao parada >60s empurrava push falso "Aguardando sua resposta". Push so sai com awaiting
    REAL: askq pendente no sidecar OU menu/overlay no pane (retry curto cobre o frame de render)."""
    info = next((s for s in registry.list() if s.jsonl and session_key(s.jsonl) == session_id), None)
    if info is None:
        return
    def _real() -> bool:
        if getattr(info, "headless", False):
            # Sem pane: quem sabe da permissão/pergunta em aberto é o adapter. `registry.list`
            # não calcula estado (sai sempre idle), então ler dali nunca mandava o push.
            snap = get_adapter(CLAUDE_HEADLESS).snapshot(info.name)
            if snap is None:
                return False
            info.state, info.question = snap.state, snap.question
            return snap.state == "awaiting_input"
        askq = read_pending_askq(info.jsonl) if info.jsonl else None
        return bool(askq and askq.questions) or _pane_wants_input(info.name)

    real = _real()
    if not real:
        time.sleep(_AWAITING_PUSH_RETRY_S)
        real = _real()  # re-le askq TAMBEM: o sidecar pode ser o que atrasou, nao so o pane
    if real:
        push.notify_awaiting(info.name, _awaiting_body(info))


def _on_awaiting(session_id: str) -> None:
    """hook_state -> transicao awaiting_input. Roda numa thread (registry.list mexe no tmux; resolver
    o corpo toca pane/disco) — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            _do_notify_awaiting(session_id)
        except Exception:
            _log.warning("push awaiting falhou (%s)", session_id, exc_info=True)
    threading.Thread(target=_work, daemon=True).start()


_CONFIRM_GRACE = 8.0  # s entre o send e a checagem "o transcript gravou o prompt?"
# Kimi: o mesmo prazo, esticado. Ali nao existe segunda tentativa (redigitar duplicaria a msg), e
# sem segunda chance o prazo tem que ser generoso — senao ruido de timing carimba `desistiu` numa
# msg que so estava esperando a vez na fila da propria TUI.
_CONFIRM_GRACE_KIMI = 30.0
# Claude sem terminal: mesma regra do Kimi (nunca redigita). O .jsonl só ganha a linha depois dos
# hooks de UserPromptSubmit, que com plugins passam de 8s; o prazo cobre isso e ainda termina em
# `desistiu` visível quando a entrega morreu de verdade (processo caiu logo após a escrita).
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
# Kimi: de quanto em quanto tempo reavaliar um "idle" que o transcript desmentiu. Nao ha evento pra
# esperar (o fim de turno real grava idle sobre idle e nao gera transicao), entao a saida e reolhar.
# 5s: a sessao demora isso pra aparecer parada, e enquanto o turno anda o custo e um getmtime.
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
        from app.conversation_history import confirmation_options
        confirmation = confirmation_options(name, info.jsonl, info.provider)
        # MID-TURN o prompt entregue ainda pode nao ter virado entrada no transcript (vive na fila
        # interna do Claude Code) — decidir requeue agora arriscaria redigitar mensagem ja recebida.
        # Adia pro proximo ciclo (o turno acabando dispara transicao -> novo timer).
        m = hook_state.get_state(session_key(info.jsonl))
        # No Kimi o marcador mente pra baixo (idle congelado do turno anterior, ver
        # state.corrige_ocioso_kimi). Sem corrigir AQUI, o guard de mid-turn abaixo nunca segurava
        # nada nesse provider — e ai toda entrega virava `desistiu` 8s depois do envio, mesmo a que
        # so estava esperando a vez na fila da propria TUI.
        if m and info.provider == "kimi":
            m = corrige_ocioso_kimi(m, info.jsonl)
        # Kimi espera MAIS antes de declarar perdida (30s contra 8s): ver o comentario no else.
        headless = _headless(name)
        grace = (_CONFIRM_GRACE_KIMI if info.provider == "kimi"
                 else _CONFIRM_GRACE_HEADLESS if headless else _CONFIRM_GRACE)
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
        if m and m[0] == "working":
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
            # Estado DESCONHECIDO (marcador ausente): nao da pra provar que a sessao nao esta no meio de
            # um turno — e redigitar e a acao destrutiva daqui (mete texto num prompt em uso). Entao
            # confirma sem NUNCA redigitar (max_attempts=0). O caso real e sessao RESSUSCITADA: o
            # kill-server de 2026-08-11 13:55 matou o tmux, a sessao voltou por `claude --resume` e a
            # fila duravel (arquivo por NOME) sobreviveu ao pane — o guard acima caiu pra frente com
            # get_state()=None e o backend redigitou dentro do turno vivo (log REQUEUE 14:01:48).
            # Pior caso agora = comportamento antigo: envio engolido fica visivel como bolha da fila,
            # que e falha VISIVEL. Duplicar a msg do usuario nao e.
            # Kimi NUNCA redigita. Prompt digitado durante um turno fica na fila da TUI e so entra no
            # wire.jsonl quando o turno chega nele — nao ha o equivalente do `queue-operation` do Claude
            # Code, que e o registro feito NO MOMENTO da digitacao. Entao, no Kimi, "ausente do
            # transcript" nao prova engolido, e redigitar e a acao destrutiva. Some a isso o marcador de
            # estado dizer "ociosa" no meio do turno (o Stop do SUBAGENTE grava na chave do pai) e o
            # guard de working acima nao segura nada: medido em 13/08/2026, a mesma mensagem entrou 3x na
            # fila de uma sessao Kimi (REQUEUE n=3 no log das 08:29). Pior caso agora e o mesmo aceito
            # logo acima pro estado desconhecido: envio de verdade engolido fica VISIVEL como bolha da
            # fila (`desistiu`), que e falha visivel — duplicar a msg do usuario nao e.
            # Sem terminal também nunca: não há tecla engolida (a escrita no stdin do processo vivo é
            # a entrega), e "ausente do transcript" antes dos hooks terminarem virava redigitação —
            # cada recado chegava duas vezes ao agente (REQUEUE medido em sessões headless).
            max_attempts = 0 if (m is None or info.provider == "kimi" or headless) else 2
            # Kimi espera MAIS antes de declarar perdida: com max_attempts=0 nao ha segunda chance — a
            # primeira checagem depois do prazo ja carimba `desistiu`. Subir pra 1 nao serve (no
            # reconcile, attempts < max REDIGITA, a duplicacao que este provider nao pode ter). Entao
            # o que se estica e o PRAZO (grace=30s): cobre o tempo entre a TUI aceitar o texto e ele
            # aparecer no wire.jsonl, sem nunca digitar duas vezes.
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
        # Sobrou entrada AINDA DENTRO do prazo (o reconcile a pulou por "recente demais")? Volta a
        # olhar. Os agendamentos usam _CONFIRM_GRACE (8,5s) e o prazo do Kimi e 30s, entao num turno
        # curto a unica checagem caia cedo demais e a entrada ficava sem confirmar E sem desistir —
        # presa ate a proxima mensagem do usuario, ou pra sempre se nao houvesse proxima. O laco
        # termina sozinho: passado o prazo, toda linha vira `confirmed` ou `desistiu`.
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


_working_started: dict[str, float] = {}  # session_id -> ts de quando entrou em "working" (mede duracao do turno pro push de "terminou")
# Lock PROPRIO do `_working_started`, separado do da cadeia de recheck: quem escreve ali roda no laco
# do hook_state.watch e quem le/consome roda numa thread de `_work`. Um compare-and-delete protegido
# so de um lado nao protege nada — o produtor concorrente passaria por cima entre o get e o del, e o
# turno seguinte terminaria com `started is None`, sem aviso e sem rastro. Nunca aninhar com
# `_recheca_lock`: sao independentes de proposito.
_turno_lock = threading.Lock()
# Sessoes com uma reavaliacao de "idle mentiroso" ja agendada. UMA cadeia por sessao: sem isto, cada
# transicao espuria abria a sua propria corrente de Timers, e duas correntes em paralelo dobram o
# `registry.list()` (que toca tmux) a cada 5s, sem limite e sem ninguem notar.
_recheca_armada: set[str] = set()
_recheca_lock = threading.Lock()


# Tentativas seguidas com FALHA antes de abandonar a cadeia de reavaliacao de uma sessao.
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


def _push_terminou(session_id: str, started: Optional[float]) -> None:
    """Push de 'terminou': avisa se o turno que comecou em `started` passou do minimo configurado.

    Mora numa funcao propria porque o disparo saiu do caminho sincrono — no Kimi so o `_work` sabe
    se o 'idle' que chegou e de verdade, e avisar 'terminou' no meio do trabalho e tao errado quanto
    re-promptar a sessao.

    `started` vem de FORA, lido no inicio do `_work`, e o consumo aqui e CONDICIONAL. Popar direto
    seria uma corrida real: entre o inicio do `_work` e este ponto rodam `registry.list()` e
    `drain()`, e o proprio drain pode largar um prompt novo — a sessao volta a "working" e o
    caminho sincrono grava o inicio do turno NOVO. Um `pop` cego levaria embora esse valor: o push
    deste turno sairia com duracao errada e o turno seguinte, ao acabar de verdade, acharia
    `started is None` e nunca avisaria."""
    if started is None:
        return
    with _turno_lock:
        if _working_started.get(session_id) != started:
            return                       # outro turno ja tomou o lugar: nao e nosso pra consumir
        del _working_started[session_id]
    if not runtime_config.get("notify_finished"):
        return
    m = hook_state.get_state(session_id)
    elapsed = (m[1] if m else time.time()) - started
    if elapsed >= runtime_config.get("finish_min_seconds"):
        _notify_async(session_id, push.notify_finished)


def _on_hook_transition(session_id: str, state: str) -> None:
    """hook_state -> mudanca de estado. Drain SERVER-SIDE: o gatilho antigo morava na conexao SSE de
    cada chat — sem celular conectado, entrada deferred ficava parada indefinidamente. idle/working =
    o pane aceita texto (Claude Code enfileira internamente); o drain re-checa deliverable sozinho.
    Tambem agenda a confirmacao de entrega das drenadas.

    Alem do drain, e o choke-point dos pushes de 'terminou' (working -> idle apos turno longo, com
    debounce) e 'caiu' (-> dead, sempre) — ver push.py. Tambem o choke-point do encadeamento de sessao
    (feature #12, _maybe_chain): idle == turno realmente terminado (so o hook Stop escreve idle), entao
    e o ponto certo pra disparar o vinculo 'then' sem correr risco de pegar um redraw no meio do turno."""
    if state == "working":
        m = hook_state.get_state(session_id)
        if m:
            with _turno_lock:
                _working_started[session_id] = m[1]
    elif state == "idle":
        # O push de "terminou" NAO sai daqui: ele espera o `_work` decidir se este idle e de verdade
        # (no Kimi ele pode ser o marcador congelado do turno anterior — ver corrige_ocioso_kimi).
        # Antes disso, avisar "terminou" no meio do trabalho era o mesmo erro das outras automacoes,
        # com o agravante de o `pop` abaixo consumir o inicio do turno: o fim REAL viria sem saber
        # ha quanto tempo a sessao trabalhava, e o debounce de turno longo nunca mais dispararia.
        pass
    elif state == "dead":
        with _turno_lock:
            _working_started.pop(session_id, None)
        if runtime_config.get("notify_dead"):
            _notify_async(session_id, push.notify_dead)

    if state == "awaiting_input":
        # Loop runner: awaiting cobre pedido de permissao tb -> pausa o loop (retoma no idle seguinte).
        # Thread propria (registry.list toca tmux); sem push proprio (o _on_awaiting ja empurra).
        def _pause_loop() -> None:
            try:
                info = next((s for s in registry.list()
                             if s.jsonl and session_key(s.jsonl) == session_id), None)
                if info:
                    with loop_mod._lock:
                        link = loop_mod.LoopLink(info.name)
                        d = link.get()
                        if d and d["status"] == "running":
                            link.update(status="paused_awaiting")
            except Exception as e:
                # Sem a lista o loop segue rodando com a sessão parada na pergunta: a falha aparece.
                _log.warning("loop: pausa no awaiting de %s falhou: %s", session_id[:8], type(e).__name__)
                diag.registrar("loop.pausa_falhou", "aviso", sessao=session_id[:8],
                               codigo=getattr(e, "code", None) or type(e).__name__)
        threading.Thread(target=_pause_loop, daemon=True).start()
        return
    # Inicio do turno lido AQUI, antes de qualquer subprocess: o `drain` la embaixo pode largar um
    # prompt novo e a sessao voltar pra "working", e ai o valor no dict ja seria de OUTRO turno.
    # Quem consome (`_push_terminou`) confere que ainda e este antes de tirar.
    with _turno_lock:
        inicio_do_turno = _working_started.get(session_id) if state == "idle" else None

    def _work() -> None:
        try:
            info = next((s for s in registry.list()
                         if s.jsonl and session_key(s.jsonl) == session_id), None)
            # Sessao nao encontrada (morreu, ou nao esta no tmux): o push de "terminou" continua
            # saindo pelo caminho de sempre. Nao ha como desconfiar do idle sem o transcript.
            if state == "idle" and not (info and info.jsonl):
                _push_terminou(session_id, inicio_do_turno)
            if info and info.jsonl:
                # Kimi: este "idle" pode ser MENTIRA. Um turno que comeca a partir de um prompt
                # enfileirado na TUI nao dispara evento nenhum, entao o marcador fica congelado no
                # idle do turno ANTERIOR enquanto o novo roda (ver state.corrige_ocioso_kimi). Sem
                # esta checagem, tres automacoes disparam com a sessao trabalhando: o loop
                # re-prompta, o `then` e CONSUMIDO (one-shot: o fim de turno real nao o dispara de
                # novo) e o push diz "terminou". O drain segue — enfileirar texto e sempre seguro,
                # e e o que o Claude/Kimi ja fazem sozinhos.
                real = state
                if state == "idle" and getattr(info, "provider", "claude") == "kimi":
                    m = hook_state.get_state(session_id)
                    if m and corrige_ocioso_kimi(m, info.jsonl)[0] == "working":
                        real = "working"
                sent = _drenar(info.name, info.jsonl, info.provider)
                # Confirmacao em TODO idle (nao so pos-drain): Timers pendentes morrem no restart
                # do backend — sem isto, entrada entregue ficava sem confirmar indefinidamente.
                if sent or real == "idle":
                    _agendar_confirmacao(info.name, _CONFIRM_GRACE + 0.5)
                # Loop runner: no idle, se ha loop ativo e o drain NAO acabou de digitar algo
                # (sent == 0 -> este idle e fim de turno de trabalho, nao o eco do goal/re-prompt),
                # tica o loop. Loop ativo SUPRIME o chain (senao cada idle entre iteracoes dispararia).
                loop_d = loop_mod.LoopLink(info.name).get()
                loop_active = loop_d is not None and loop_d["status"] in loop_mod.ACTIVE
                if real == "idle" and loop_active and sent == 0:
                    loop_mod.schedule_tick(info.name, lambda: _loop_ctx(info.name))
                # Encadeamento (feature #12): so quando NAO ha loop ativo — turno REALMENTE terminado,
                # reusando o info.name ja resolvido nesta thread — ver _maybe_chain.
                if real == "idle" and not loop_active:
                    _maybe_chain(info.name)
                if real == "idle":
                    # so no fim de turno PROVADO (ver o elif la em cima)
                    _push_terminou(session_id, inicio_do_turno)
                # Idle desmentido pelo transcript: o fim de turno REAL nao vai gerar transicao
                # nenhuma (o Stop grava idle sobre idle, e hook_state._apply so avisa quando o
                # estado MUDA). Sem reagendar, o turno terminaria sem drenar a fila, sem ticar o
                # loop e sem disparar o `then`. O reagendamento converge sozinho: quando o turno
                # acaba, o wire.jsonl para de crescer e a proxima passada ve idle de verdade.
                # UMA cadeia por sessao (_recheca_armada): cada transicao espuria abria a sua, e
                # duas cadeias em paralelo dobram `registry.list()` (tmux) a cada 5s pra sempre.
                _falhas_seguidas.pop(session_id, None)   # esta volta foi ate o fim: zera o teto
                if real != state and _armar_recheca(session_id):
                    threading.Timer(_RECHECA_KIMI, _recheca_kimi,
                                    args=(session_id, state)).start()
        except Exception as exc:
            # LOGA, nao `pass` mudo: e daqui que saem o drain da fila, o tick do loop, o vinculo
            # `then` e o push de "terminou". Falha calada aqui devolve exatamente o sintoma que este
            # bloco existe pra matar — sessao que nunca drena — sem uma linha pra investigar. E o
            # texto diz a CONSEQUENCIA, nao so "falhou": no Kimi o fim de turno real nao gera
            # transicao nova (idle sobre idle), entao sem reavaliacao a sessao pode ficar parada
            # sem drenar ate a proxima msg do usuario.
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


class ClaudeCustomizationBody(_StrictBody):
    plugins: dict[str, StrictBool] = Field(default_factory=dict)
    skills: dict[str, StrictBool] = Field(default_factory=dict)
    blocked_skills: list[str] = Field(default_factory=list)


class CreateBody(_StrictBody):
    _engine_catalog: list[dict] | None = PrivateAttr(default=None)
    name: str = Field(min_length=1)
    cwd: str = Field(min_length=1)
    branch: str | None = Field(default=None, min_length=1)
    # Com `new_branch`, `branch` é o nome da branch NOVA e `base` a de partida (None = a atual).
    new_branch: bool = Field(default=False, strict=True)
    base: str | None = Field(default=None, min_length=1)
    config_dir: str | None = None
    # O campo omitido é resolvido pelo servidor antes de validar as opções do provedor.
    provider: str = "claude"
    # Somente a abertura humana muda o padrão; criação automatizada pode escolher outro provedor.
    remember_provider: bool = Field(default=False, strict=True)
    # Conta Codex escolhida pelo usuário. Ausente mantém a conta padrão para clientes antigos.
    codex_account: str | None = None
    # Wrapper interativo do Codex pode iniciar a TUI ja com um prompt. Nao e argv arbitrario:
    # evita que um cliente remoto injete flags que afrouxem sandbox/aprovacoes do backend.
    initial_prompt: str | None = None
    # Motor de modelo (nome no engines.json). None = conta Anthropic, comportamento de hoje.
    engine: str | None = None
    engine_account: str | None = None
    # Escolhidos na tela de abertura. None = padrão do binário (comportamento de hoje). Validado
    # aqui, nunca no front: o valor entra num comando de shell.
    model: str | None = None
    effort: str | None = None
    service_tier: Literal["default", "priority"] | None = None
    # Modo de permissão do Claude Code. None = padrão da conta (comportamento de hoje).
    permission_mode: str | None = None
    # CLAUDE_CODE_SUBAGENT_MODEL. Só claude sem motor: o motor exporta o dele e ganharia calado.
    subagent_model: str | None = None
    claude_customizations: ClaudeCustomizationBody | None = None
    # Jev (typesafe.ai) no `hangar-preview objetivo`. Escolha da ABERTURA: ligado, a sessão nasce
    # com a chave no ambiente; desligado, só com o marcador, e o verbo recusa. É o que
    # permite rodar a mesma tarefa com e sem, sem apagar a configuração.
    # AUSENTE (None) ≠ `false`: quem não disse nada herda o `jev_padrao` do servidor, e é assim que
    # a escolha feita uma vez vale pros três caminhos de criação. `false` explícito continua
    # desligando aquela sessão mesmo com o padrão ligado.
    jev: bool | None = Field(default=None, strict=True)
    # Perfil do omp (`omp --profile x`): login, sessões e config em ~/.omp/profiles/x/agent.
    # None = sem perfil. Só vale com provider omp; o nome é validado no registry.
    omp_profile: str | None = None

    read_only: bool = Field(default=False, strict=True)
    # Claude ou Codex SEM terminal roda atrás do cano, sem tmux. O que depende de pane
    # (painel de terminal, espelho) não existe.
    headless: bool | None = Field(default=None, strict=True)
    # Sessão que pediu a criação (MCP `new_session`, `hangar-send --new`). O que vier omitido
    # (modo de permissão, sem terminal) herda dela; sem ela, vale o padrão do servidor.
    creator: str | None = Field(default=None, min_length=1)


@app.get("/api/claude/customizations", dependencies=[Depends(require_auth)])
async def claude_customization_catalog(request: Request, cwd: str = Query(min_length=1),
                                       config_dir: str | None = None):
    if guest_of(request) is not None or guest_users.current.get() is not None:
        raise HTTPException(403, detail=erro("erro_fora_da_pasta", "a seleção de plugins pertence ao dono"))
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    path = await asyncio.to_thread(lambda: str(Path(cwd).expanduser().resolve()))
    if not await asyncio.to_thread(os.path.isdir, path):
        raise HTTPException(400, detail=erro("erro_cwd_inexistente", "a pasta não existe", cwd=cwd))
    try:
        return await asyncio.to_thread(claude_customizations.catalog, path, tmux.config_dir_de(config_dir))
    except claude_customizations.CustomizationsError as exc:
        raise HTTPException(exc.status, detail=erro(exc.code, exc.detail)) from None


def _jev_efetivo(pedido: bool | None) -> bool:
    """Jev desta sessão: o que o chamador pediu, ou o padrão do servidor quando ele não disse nada.

    Um lugar só porque os três caminhos de criação (folha, `hangar-send --new`, MCP `new_session`)
    desembocam todos no `create_session` — resolver em cada um faria o padrão valer em dois e
    faltar no terceiro sem ninguém perceber."""
    return bool(runtime_config.get("jev_padrao")) if pedido is None else pedido


class TtsBody(_StrictBody):
    # max_length: sem teto, um corpo de 100 MB era parseado INTEIRO antes do 413 do preparo/teto
    # de caracteres poder recusar. 200_000 e bem acima do teto real (_TTS_TETO) — so evita o corpo
    # gigante, quem recusa por caractere de verdade continua sendo a rota.
    text: str = Field(min_length=1, max_length=200_000)
    voice: str = ""
    provider: str = "elevenlabs"
    # Confirmacao explicita do usuario pra passar do limite de aviso. Ver _TTS_TETO abaixo: o teto
    # duro nao e confirmavel, so o limite de aviso.
    confirm: bool = False
    # Fase 2 (narracao guiada): instrucao que ja tratou este `text` via POST /api/tts/narrar ("" =
    # leitura direta, o caminho de hoje). So entra na chave do cache (tts.hash_de) — chega aqui
    # depois que o texto ja esta pronto pra virar audio, nunca dispara a Groq.
    instruction: str = ""


class NarrarBody(_StrictBody):
    text: str = Field(min_length=1, max_length=200_000)
    code_blocks: list[str] = Field(default_factory=list)
    instruction: str = Field(min_length=1, max_length=2000)


class PushSubscribeBody(_StrictBody):
    subscription: dict  # PushSubscription do browser: {endpoint, keys:{p256dh, auth}}
    label: str = Field(min_length=1)    # nome do servidor escolhido no celular (Casa/my-org)
    serverId: str = Field(min_length=1)  # id local do servidor no celular (pro deep-link da notif)
    # Idioma da inscricao: o front manda o escolhido na tela Geral; ausente (front velho) cai em
    # "en", o baseLocale do app — a leitura do registro antigo e que trata o campo ausente como pt.
    locale: str = "en"


@app.get("/api/push/vapid", dependencies=[Depends(require_auth)])
def push_vapid():
    # Chave publica VAPID (applicationServerKey) pro browser assinar. Vazia = push desligado no backend.
    return {"key": settings.vapid_public}


@app.post("/api/push/subscribe", dependencies=[Depends(require_auth)])
def push_subscribe(body: PushSubscribeBody):
    try:
        push.add_subscription(body.subscription, body.label, body.serverId, body.locale)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.get("/api/push/settings", dependencies=[Depends(require_auth)])
def push_settings():
    # Estado atual (mute por sessao + quiet hours global) pro app refletir na UI.
    return push.get_push_prefs()


class PushMuteBody(_StrictBody):
    session: str = Field(min_length=1)
    muted: bool


@app.post("/api/push/mute", dependencies=[Depends(require_auth)])
def push_mute(body: PushMuteBody):
    push.set_muted(body.session, body.muted)
    return {"ok": True}


class PushQuietHoursBody(_StrictBody):
    # HH:MM. Ambos None desliga a janela; so ha janela com os dois presentes.
    start: str | None = None
    end: str | None = None


@app.post("/api/push/quiet-hours", dependencies=[Depends(require_auth)])
def push_quiet_hours(body: PushQuietHoursBody):
    try:
        push.set_quiet_hours(body.start, body.end)
    except ValueError as e:
        raise HTTPException(422, str(e))
    return {"ok": True}


class InputBody(_StrictBody):
    text: str
    # Recados 1:1 pedem orientação imediata; envios comuns conservam a fila normal.
    steer: bool = False


class BroadcastBody(_StrictBody):
    names: list[str] = Field(min_length=1)
    text: str


class SelectBody(_StrictBody):
    option: int = Field(ge=1, le=50)  # picker 1-based; teto evita loop de fork tmux (DoS)


class BtwBody(_StrictBody):
    question: str = Field(min_length=1, max_length=4000)


class KeyBody(_StrictBody):
    key: str  # nome da tecla de navegacao (allowlist em TerminalInput._NAV_KEYS)


class TermInputBody(_StrictBody):
    # Terminal interativo (so desktop): texto livre (literal) e/ou uma tecla nomeada (allowlist
    # em TerminalInput._TERM_KEYS). Os dois opcionais -> um POST pode mandar so texto OU so tecla.
    text: str | None = None
    key: str | None = None


class ModelEffortBody(_StrictBody):
    # ambos opcionais: so esforco (sem modelo) ainda dirige o picker do /model, deixando o
    # modelo na linha atual. scope: 'session' (aperta `s`) ou 'default' (aperta Enter).
    model: str | None = None
    effort: str | None = None
    scope: Literal["session", "default"] = "session"


@app.get("/api/whoami", dependencies=[Depends(require_auth)])
async def whoami(request: Request):
    """Qual sessão está chamando, pelos cabeçalhos X-Hangar-* (ver quem_chama). Diagnóstico e
    teste da resolução que o MCP usa; o CLI continua resolvendo localmente."""
    try:
        nome, origem = await asyncio.to_thread(quem_chama.resolver, request.headers)
    except quem_chama.SessaoDesconhecida as e:
        raise HTTPException(404, detail=erro("erro_sessao_desconhecida", str(e)))
    return {"name": nome, "origem": origem}


@app.get("/api/sessions", dependencies=[Depends(require_auth)], response_model=list[SessionInfo])
async def list_sessions(request: Request):
    # list_with_state: resolucao otimizada (1 scan /proc + 1 chamada tmux em lote) + estado vivo por
    # sessao (working/idle/awaiting_input) classificado do pane. async pq captura os panes concorrente.
    # MuxIndisponivel nao e tratada aqui: o handler de `_mux_indisponivel` cobre esta rota e as
    # outras quinze que chamam registry.list(). Um try/except so nesta seria a mesma resposta
    # escrita duas vezes, e a que envelhece primeiro.
    #
    # A RESOLUCAO (scan de /proc + fork de tmux) vem do snapshot de `_guardar_snap`, com TTL de 1s e
    # single-flight; o ESTADO continua sendo classificado a cada chamada, entao a resposta nao fica
    # velha. Medido em 06/09/2026: sem isto as chamadas nao se sobrepoem — 1 custa 13ms, 3 custam
    # 35ms de parede, 6 custam 67ms e 12 custam 134ms, linear, porque cada uma refaz a varredura
    # inteira. E o front chama isto a cada 2s POR cliente. Com o snapshot, N clientes que caem na
    # mesma janela pagam uma varredura so. `to_thread` porque `_guardar_snap` bloqueia (mesma regra
    # do git status na corrotina, o incidente de 2026-07-23).
    #
    # CADA requisicao decora as SUAS copias, e isto nao e zelo: `list_with_state` escreve NOS
    # objetos (`info.state`, `info.last_activity`, `info.question`...), e o snapshot e a mesma lista
    # servida a todo mundo dentro do TTL. Sem a copia, duas chamadas concorrentes — que e justamente
    # o que este cache existe pra permitir — se intercalam nos MESMOS SessionInfo entre os awaits da
    # decoracao, e o estado decorado ainda vazaria pro snapshot que `/history` e `/workflows` leem
    # esperando a lista crua. `model_copy` rasa basta: a decoracao ATRIBUI campos, nunca muta em
    # lugar o que ja esta neles.
    # Com a lista SSE aberta, o refresher já decorou isto há menos de um tique: serve dele. Com o
    # Rust dono, o retrato é dele (até 2 s, produzido na hora se mais velho).
    from app.sse import recent_list
    guest = guest_of(request)
    viewer = guest_users.current.get()
    decorated = (await asyncio.to_thread(list_bridge.snapshot) if await registry_mod.rust_owns_list_async()
                 else recent_list(2.0))
    if decorated is not None:
        if guest is not None:
            decorated = [i for i in decorated if guest.sees(i.name)]
        # Mesmo recorte do caminho sem cache: convidado com login próprio só vê o que lhe cabe.
        if viewer is not None or guest_users.has_claims():
            decorated = await asyncio.to_thread(guest_users.filter_visible, viewer, decorated,
                                                lambda i: i.name)
        return decorated if guest is None else [guest_safe(i, guest) for i in decorated]
    snap = await asyncio.to_thread(_guardar_snap)
    # Convidado ve so a sessao compartilhada; o filtro fica depois do snapshot para nao tocar no cache.
    if guest is not None:
        snap = [i for i in snap if guest.sees(i.name)]
    if viewer is not None or guest_users.has_claims():
        snap = await asyncio.to_thread(guest_users.filter_visible, viewer, snap, lambda i: i.name)
    decorated = await registry.list_with_state([i.model_copy() for i in snap])
    # O pareamento e o encadeamento são decorados acima e citam outras sessões do dono.
    return decorated if guest is None else [guest_safe(i, guest) for i in decorated]


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


@app.get("/api/claude-configs", dependencies=[Depends(require_auth)], response_model=list[ConfigDirInfo])
def claude_configs():
    return list_config_dirs()


class ContaBody(_StrictBody):
    # pattern com \z (fim absoluto da crate regex do pydantic — o \Z do Python não é aceito lá,
    # e o $ casaria antes de uma quebra de linha final, deixando 'conta2\n' passar: pasta com
    # controle de linha no nome). O mesmo padrão do contas._NOME_OK, que é fullmatch; aqui no
    # schema o pedido inválido nem chega no módulo.
    nome: str = Field(min_length=1, max_length=32,
                      pattern=r"^[a-z0-9][a-z0-9_-]{0,31}\z")


@app.post("/api/claude-configs", dependencies=[Depends(require_auth)])
async def post_claude_config(body: ContaBody):
    """Cria a pasta da conta. NÃO loga: o OAuth abre navegador e é interativo — quem roda o
    /login é o usuário, dentro da primeira sessão aberta nessa conta."""
    if os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip():
        # Com a lista fixa por env, list_config_dirs ignora o auto-scan: a conta seria criada e
        # nunca apareceria no seletor. Recusar com o motivo é melhor que um 200 inútil.
        raise HTTPException(409, detail=erro("erro_config_dirs_fixo",
                                 "CP_CLAUDE_CONFIG_DIRS está setado: a lista de contas é fixa por "
                                 "ambiente. Remova a variável ou acrescente a conta nela."))
    try:
        p = await asyncio.to_thread(contas.criar, body.nome)
    except contas.ContaError as e:
        raise HTTPException(e.status, e.detail) from None
    return {"path": str(p), "label": body.nome, "active": False}


@app.delete("/api/claude-configs/{nome}", dependencies=[Depends(require_auth)])
async def delete_claude_config(nome: str):
    """Apaga a conta e os transcripts dela. Recusa se alguma sessão viva estiver usando, se a
    conta for a configuração ativa do backend, se estiver na lista fixa do ambiente ou se algum
    processo vivo tiver o config dir dela — apagar debaixo de um deles deixa o CLI escrevendo
    num caminho que sumiu."""
    try:
        alvo = contas.caminho(nome)
    except contas.ContaError as e:
        # Nome fora do alfabeto da conta (ex: pasta de backup com ponto no nome): envelope pra
        # o front traduzir no idioma do app, em vez de mostrar a string crua do módulo.
        raise HTTPException(e.status, detail=erro("erro_conta_nome_invalido", e.detail)) from None
    if alvo.resolve() == _backend_config_base().resolve():
        # A config ativa do backend é o ~/.claude (ou o CLAUDE_CONFIG_DIR dele): settings,
        # custos e transcripts do próprio app moram lá — apagar derrubaria o app em si.
        raise HTTPException(409, detail=erro("erro_conta_ativa_backend",
                                 "esta conta é a configuração ativa do backend — não dá pra "
                                 "apagar por aqui"))
    if os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip():
        # Com a lista fixa por env, o GET continua devolvendo esta conta MESMO apagada: sobraria
        # um fantasma no seletor, e a próxima sessão recriaria a pasta sem marcador nem atalhos.
        if alvo.resolve() in {Path(c.path).resolve() for c in list_config_dirs()}:
            raise HTTPException(409, detail=erro("erro_conta_lista_fixa",
                                     "CP_CLAUDE_CONFIG_DIRS está setado: esta conta está na "
                                     "lista fixa por ambiente. Remova-a da variável antes de "
                                     "apagar."))
    # O ciclo segura a trava da conta (a mesma do create_session) ao redor da checagem e do
    # rmtree: sem ele, o DELETE passaria na janela entre a reconciliação e o registry.create de
    # uma sessão que está subindo, e apagaria a pasta embaixo dela.
    def _checar_e_apagar():
        # TUDO numa thread só: o `ciclo_conta` pega `flock` no __enter__, que BLOQUEIA. Chamado
        # direto da rota async, uma segunda operação de conta concorrente congelava o event loop
        # inteiro — todas as rotas do app, não só esta — até a primeira soltar. E a janela é longa:
        # o laço abaixo roda um `subprocess` do tmux por sessão viva, também síncrono.
        with contas.ciclo_conta(nome) as ciclo:
            for s in registry.list():
                cfg, confiavel = _session_config_dir_strict(s.name)
                if not confiavel:
                    raise HTTPException(409, detail=erro("erro_config_dir_sessao",
                                             f"não consegui confirmar o config dir da sessão "
                                             f"'{s.name}' — apagar recusado", nome=s.name))
                if cfg is not None and cfg.resolve() == alvo.resolve():
                    raise HTTPException(409, detail=erro("erro_sessao_usa_conta",
                                             f"a sessão '{s.name}' está usando esta conta", nome=s.name))
            # CLI aberto FORA do tmux não aparece no registry: a varredura por CLAUDE_CONFIG_DIR
            # no /proc é quem segura o apagar debaixo dele.
            pids, varredura_ok = procinfo._pids_com_config_dir(alvo)
            if not varredura_ok:
                # "Não consegui olhar" não é "olhei e não achei": seguir aqui apagaria a pasta
                # debaixo de um `claude` vivo que a varredura não chegou a enxergar.
                raise HTTPException(409, detail=erro("erro_varredura_processos",
                                         "não consegui varrer os processos da máquina — apagar "
                                         "recusado (pode haver um claude aberto nesta conta)"))
            if pids:
                raise HTTPException(409, detail=erro("erro_processos_usam_conta",
                                         f"processo(s) {pids} estão usando esta conta", pids=pids))
            ciclo.apagar()

    try:
        await asyncio.to_thread(_checar_e_apagar)
    except contas.ContaError as e:
        # Pasta não carimbada (ou conta que sumiu): mesmo 404 do apagar() antigo, agora como
        # envelope — a mesma chave do login (erro_conta_inexistente) traduz nos dois fluxos.
        raise HTTPException(e.status, detail=erro("erro_conta_inexistente", e.detail,
                                                  nome=nome)) from None
    return {"ok": True}


@app.post("/api/claude-configs/{nome}/logout", dependencies=[Depends(require_auth)])
async def logout_claude_config(nome: str):
    """Sai da conta sem apagar a pasta. Sessão aberta na conta não impede: ela só perde o login
    (se renovar o token em memória, pode regravar a credencial).

    `nome` é o rótulo da lista (o apelido, quando a conta foi renomeada), igual ao login."""
    from app import account_bridge
    native = await asyncio.to_thread(account_bridge.request_claude, "logout", label=nome)
    if native is not None:
        return native
    conta = next((c for c in list_config_dirs() if c.label == nome), None)
    if conta is None:
        raise HTTPException(404, detail=erro("erro_conta_inexistente", f"conta {nome} não existe", nome=nome))
    alvo = Path(conta.path)
    pasta = alvo.name.removeprefix(".claude-")

    def _sair():
        try:
            conta_estado._auth_logout(alvo)
        except RuntimeError as e:
            raise HTTPException(502, detail=erro("erro_logout_nao_confirmado", str(e))) from None
        conta_estado.esquecer_conta(conta.path)
        estado = conta_estado._estado_login(conta_estado._auth_status(alvo))
        if estado.estado != "ok" or estado.loggedIn:
            raise HTTPException(502, detail=erro("erro_logout_nao_confirmado",
                                     "a conta não apareceu deslogada depois do logout"))

    def _checar_e_sair():
        # A config ativa do backend (~/.claude) não é conta criada pelo hangar, então não tem
        # trava de ciclo; sair dela só tira o login, é o caminho pra entrar com outra.
        if alvo.resolve() == _backend_config_base().resolve():
            _sair()
            return
        with contas.ciclo_conta(pasta):
            if contas.caminho(pasta).resolve() != alvo.resolve():
                raise contas.ContaError(404, f"{alvo} não é uma conta criada pelo hangar")
            _sair()

    try:
        await asyncio.to_thread(_checar_e_sair)
    except contas.ContaError as e:
        raise HTTPException(e.status, detail=erro("erro_conta_inexistente", e.detail,
                                                  nome=nome)) from None
    return {"ok": True}


@app.get("/api/desktop/palette", dependencies=[Depends(require_auth), Depends(require_loopback)])
def desktop_palette_get():
    # 404 e resposta de negocio, nao erro: e como o front sabe que nao ha rice nesta maquina e
    # esconde a opcao.
    p = desktop_palette.ler()
    if p is None:
        raise HTTPException(status_code=404, detail=erro("erro_sem_paleta", "sem paleta"))
    return p


@app.get("/api/desktop/wallpaper", dependencies=[Depends(require_auth), Depends(require_loopback)])
def desktop_wallpaper_get():
    # A imagem que o rice esta usando agora, pro modo "Vidro" do fundo Desktop desenhar ela DENTRO da
    # pagina (backdrop-filter so enxerga o que a propria pagina pintou; atras de janela transparente
    # nao ha pixel nenhum pra virar vidro). 404 = sem rice/sem foto, e o front esconde a opcao.
    p = desktop_palette.wallpaper()
    if p is None:
        raise HTTPException(status_code=404, detail=erro("erro_sem_papel_de_parede", "sem papel de parede"))
    # Sem cache do navegador: trocar o papel de parede mantem a URL e so muda o conteudo, entao um
    # 304 deixaria a foto velha na tela ate alguem limpar o cache.
    return FileResponse(p, headers={"Cache-Control": "no-store"})


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


@app.get("/api/uso", dependencies=[Depends(require_auth)], response_model=UsoReport)
def uso_endpoint(period: str = "all", conta: list[str] = Query([]), projeto: list[str] = Query([]),
                 modelo: list[str] = Query([]), plugin: list[str] = Query([]), foco: str = "",
                 fresco: bool = False):
    """Uso de skills/tools/hooks/MCP/agentes do Claude Code, do mesmo cache que o /api/costs.
    Filtros repetíveis (`?conta=a&conta=b`); vazio = tudo; as chaves são as de
    `by_conta`/`by_projeto`/`by_modelo`/`by_plugin`. `foco` = nome de um item: só a série diária
    (`by_day`) recorta por ele."""
    if period not in _COST_PERIODOS and period != "all":
        period = "all"
    limpo = lambda xs: [x for x in xs if x]  # `?conta=` (vazio) é "todas", não a conta ""
    try:
        return uso_report.report(period=period, fresco=fresco, conta=limpo(conta),
                                 projeto=limpo(projeto), modelo=limpo(modelo),
                                 plugin=limpo(plugin), foco=foco or None)
    except costs_sources.Aquecendo as e:
        return _aquecendo(e)


# Passo em curso de cada criação, pelo nome da sessão nova: a tela de criar consulta enquanto
# espera, e quem olha sabe que não travou.
_criacao_passo: dict[str, dict] = {}


def _passo(nome: str, passo: str, **params) -> None:
    _criacao_passo[sanitize_session_name(nome)] = {"step": passo, "params": params}


@contextmanager
def _acompanhar_criacao(nome: str):
    """Só quem abriu o registro o apaga: o bastão chama o create_session por dentro e ainda tem
    passo depois dele."""
    chave = sanitize_session_name(nome)
    dono = chave not in _criacao_passo
    if dono:
        _criacao_passo[chave] = {"step": "preparando", "params": {}}
    try:
        yield
    finally:
        if dono:
            _criacao_passo.pop(chave, None)


@app.get("/api/sessions/creation-progress", dependencies=[Depends(require_auth)])
async def creation_progress(name: str):
    return _criacao_passo.get(sanitize_session_name(name)) or {"step": None, "params": {}}


async def _kill_unclaimed(name: str) -> None:
    try:
        await asyncio.to_thread(registry.kill, name)
    except Exception:
        _log.exception("[guests] sessao %s sem dono nao encerrou", name)
    finally:
        await asyncio.to_thread(_invalidate_lists)


def _creator_permission_mode(name: str, jsonl: str | None) -> str | None:
    """Modo fora de `plan` da sessão Claude `name`, no nome que o `--permission-mode` aceita."""
    meta = headless_sessions.load(name)
    modo = None
    if meta:
        modo = meta.get("permission_mode")
        if modo == "plan":
            modo = meta.get("previous_non_plan")
    modo = modo or permission_mode.session_non_plan_mode(jsonl or _jsonl_atual(name))
    modo = "manual" if modo == "default" else modo
    return modo if modo in model_args.MODOS_PERMISSAO_CLAUDE and modo != "plan" else None


async def _inherit_from_creator(body: CreateBody) -> tuple[CreateBody, str | None, list[str]]:
    """Preenche o que veio omitido com o da sessão criadora; devolve também `"inherited"` quando a
    conta veio dela e os avisos do que não deu para herdar. O modo conta porque o primeiro recado
    de uma criadora em bypass para uma irmã em Manual fica retido no receptor (`mode-mismatch`)."""
    # Convidado não herda a conta nem o modo de uma sessão do dono.
    if not body.creator or guest_users.current.get() is not None:
        return body, None, []
    info = await _cached_info(body.creator)
    if info is None:
        return body, None, [f"sessão criadora '{body.creator}' não encontrada; nada foi herdado"]
    update: dict = {}
    avisos: list[str] = []
    account_source = None
    # Perfil do omp já define a conta.
    if body.config_dir is None and body.provider in ("claude", "pi", "omp") and not body.omp_profile:
        cfg, confiavel = await asyncio.to_thread(_caller_config_dir, info.name)
        if not confiavel:
            # Criar assim nasceria na conta padrão, calado: gasta a cota de quem ninguém escolheu.
            raise HTTPException(409, detail=erro(
                "erro_conta_criadora", f"não consegui confirmar a conta da sessão '{info.name}' — "
                "escolha a conta (`conta` no MCP, `--conta` no hangar-send)"))
        account_source = "inherited"
        if cfg:
            update["config_dir"] = str(cfg)
    if body.headless is None and body.provider in ("claude", "codex") and not body.read_only:
        update["headless"] = bool(info.headless)
    if body.permission_mode is None and body.provider == "claude" and info.provider == "claude":
        modo = await asyncio.to_thread(_creator_permission_mode, info.name, info.jsonl)
        if modo:
            update["permission_mode"] = modo
        else:
            avisos.append(f"não consegui ler o modo de permissão de '{info.name}'; vale o padrão da conta")
    return (body.model_copy(update=update) if update else body), account_source, avisos


@app.post("/api/sessions", dependencies=[Depends(require_auth)], response_model=CreatedSessionInfo)
async def create_session(body: CreateBody) -> CreatedSessionInfo:
    if "provider" not in body.model_fields_set:
        provider = await _default_session_provider(body.config_dir, body.engine, body.codex_account,
                                                   body.omp_profile, body.subagent_model)
        body = body.model_copy(update={"provider": provider})
    explicit_account = body.config_dir is not None
    body, account_source, avisos_extra = await _inherit_from_creator(body)
    # Convidado fica na conta Codex padrão: as outras contas são do dono.
    if body.provider == "codex" and body.codex_account is None and guest_users.current.get() is None:
        connected = await _connected_codex_accounts()
        if connected and not any(account.is_default for account in connected):
            body = body.model_copy(update={"codex_account": connected[0].id})
            avisos_extra.append(f"A conta Codex padrão não está conectada; a sessão usa a conta {connected[0].id}.")
    if not explicit_account and body.provider == "claude" and not body.engine:
        # Sem conta pedida, a herdada ou a padrão só vale se não estiver acabando; senão nasce na de
        # mais folga, e a resposta diz que trocou.
        from app import cotas
        # cotas_claude() consulta a ponte do Rust por HTTP síncrono: fica fora do event loop.
        config_dir, aviso = await asyncio.to_thread(
            lambda: cotas.conta_com_cota(body.config_dir, cotas.cotas_claude()))
        if aviso:
            _log.warning("create_session %s: %s", body.name, aviso)
            body = body.model_copy(update={"config_dir": config_dir})
            avisos_extra.append(aviso)
            account_source = "quota"
    if body.provider == "claude" and not body.engine and (
            body.config_dir is None or body.config_dir in {c.path for c in list_config_dirs()}):
        cfg = Path(body.config_dir) if body.config_dir else None
        removidos, falhas = await asyncio.to_thread(default_model.drop_foreign, cfg)
        for valor in removidos:
            _log.warning("create_session %s: modelo padrão %r não é da Anthropic; removido do settings.json",
                         body.name, valor)
            avisos_extra.append(f"O modelo padrão '{valor}' do settings.json não é da Anthropic "
                                "(provavelmente veio de um /model numa sessão de motor) e foi removido.")
        avisos_extra += [f"Não consegui tirar do settings.json um modelo que não é da Anthropic: {f}"
                         for f in falhas]
    with _acompanhar_criacao(body.name):
        worktree: dict = {}
        try:
            info = await _criar_sessao(body, worktree)
            if body.branch is not None:
                # O cwd da worktree vem do dict: `_criar_sessao` pode trabalhar numa cópia do body.
                cwd = worktree.get("cwd", body.cwd)
                is_wt = Path(cwd, ".git").is_file()
                info = info.model_copy(update={"cwd": cwd, "branch": body.branch, "worktree": is_wt,
                                               "worktree_path": cwd if is_wt else None})
            guest = guest_users.current.get()
            if guest is not None:
                try:
                    await asyncio.to_thread(guest_users.claim, info.name, guest.id)
                except Exception:
                    # Sem o dono registrado a sessão ficaria à vista do dono e sumida para o convidado.
                    _log.exception("[guests] claim de %s falhou; encerrando a sessao", info.name)
                    await _kill_unclaimed(info.name)
                    raise
                info = info.model_copy(update={"owner": guest.name})
            if avisos_extra:
                info = info.model_copy(update={"avisos": [*info.avisos, *avisos_extra]})
            info = CreatedSessionInfo(**info.model_dump(), config_dir=body.config_dir,
                                      account_source=account_source)
            if (guest is None and body.remember_provider
                    and runtime_config.get("last_session_provider") != info.provider):
                try:
                    await asyncio.to_thread(runtime_config.aplicar, {"last_session_provider": info.provider})
                except Exception as exc:  # noqa: BLE001 — a sessão já existe; falhar aqui faria o cliente recriá-la
                    _log.warning("não consegui lembrar o provedor da sessão %s: %s", info.name, exc)
                    info = info.model_copy(update={"avisos": [*info.avisos,
                        f"A sessão foi criada, mas não consegui lembrar o provedor: {exc}"]})
            return info
        except BaseException:
            if worktree.get("path") and not worktree.get("session_created"):
                try:
                    await asyncio.shield(asyncio.to_thread(_undo_worktree, worktree))
                except GitError as exc:
                    raise HTTPException(500, detail=erro("erro_criacao_sessao",
                                                          f"falha ao desfazer a worktree: {exc.detail}")) from exc
            raise


def _undo_worktree(worktree: dict) -> None:
    """Desfaz a worktree que o próprio pedido criou. Força: os arquivos de config copiados podem
    não estar ignorados na base. A branch nova vai junto, senão a nova tentativa daria 409."""
    remove_worktree(worktree["source"], worktree["path"], force=True)
    branch = worktree.get("new_branch")
    if branch:
        deleted = git_ops._run(worktree["source"], "branch", "-D", branch)
        if deleted.returncode != 0:
            raise GitError(500, git_ops._scrub(deleted.stderr.strip()) or "não consegui apagar a branch")


def _allowed_scan_root(path: str) -> Path:
    target = Path(os.path.realpath(os.path.expanduser(path)))
    root = next((r for r in allowed_roots() if target.is_relative_to(r)), None)
    if root is None:
        raise FsError(403, "root not allowed")
    scan_dir(str(root), str(target))
    return root


async def _criar_sessao(body: CreateBody, worktree: dict):
    from app.account_lifecycle import complete_on_cancel
    return await complete_on_cancel(_create_session_owned(body, worktree))


async def _create_session_owned(body: CreateBody, worktree: dict):
    if body.headless is None:
        body = body.model_copy(update={"headless": not body.read_only and body.provider in ("claude", "codex")
                                      and bool(runtime_config.get("headless_default"))})
    # Handler async por causa da trava de conta mais abaixo. Todo provider passa pelo MESMO
    # registry.create — o Codex tambem, desde que o lancador unico virou o comando do pane dele.
    # registry.create e SINCRONO e spawna um
    # subprocess tmux (bloqueante) -> rodar direto aqui travaria o event loop / o SSE de outras
    # sessoes; vai pro threadpool via asyncio.to_thread, igual aos outros handlers async deste
    # arquivo que chamam registry.list()/save_upload (menor risco de regressao: comportamento e
    # exceções do create() Claude ficam IDENTICOS, so a chamada muda de sync p/ thread).
    # Pi entra pelo MESMO registry.create do Claude (pane tmux + spawn_command do PiAdapter); o que
    # muda la dentro e so o transcript, que nao e pre-semeado (layout proprio, arquivo so no 1o turno).
    # Validar provider, config_dir e engine ANTES de qualquer efeito no disco: um pedido que vai
    # ser rejeitado aqui não pode ter reconciliado a conta (deriva movida, memória criada) à toa.
    if body.provider not in ("claude", "codex", "pi", "kimi", "omp"):
        raise HTTPException(400, detail=erro("erro_provider_sessao_invalido", "provider invalido"))
    if body.claude_customizations is not None:
        if body.provider != "claude":
            raise HTTPException(400, detail=erro("claude_customizations_invalid",
                                                 "plugins e skills por sessão só valem para Claude"))
        if guest_users.current.get() is not None:
            raise HTTPException(403, detail=erro("claude_customizations_owner_only",
                                                 "a seleção de plugins pertence ao dono"))
    if body.service_tier is not None and body.provider != "codex":
        if body.provider != "claude" or not await asyncio.to_thread(cliproxy.supports_fast, body.engine, body.model):
            raise HTTPException(400, detail=erro("erro_criacao_sessao", "Fast exige Codex ou Claude com GPT no CLIProxyAPI local"))
    # Antes de qualquer efeito (worktree, registry.create): convidado só abre dentro da pasta dele.
    guest = guest_users.current.get()
    if guest is not None and not guest_users.inside_root(guest, body.cwd):
        raise HTTPException(403, detail=erro("erro_fora_da_pasta",
                                             "o convidado só abre sessão dentro da pasta dele"))
    # Sem isto a sessão sem terminal nasce e só quebra ao subir o processo, com um ENOENT que não
    # diz qual arquivo faltou.
    if not await asyncio.to_thread(os.path.isdir, os.path.expanduser(body.cwd)):
        raise HTTPException(400, detail=erro("erro_cwd_inexistente", f"a pasta {body.cwd} não existe",
                                             cwd=body.cwd))
    account_models = None
    if body.engine_account is not None:
        if body.provider != "claude" or not body.engine:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", "conta ChatGPT exige Claude com motor CLIProxyAPI local"))
        account = await asyncio.to_thread(_fixed_engine_account, body.engine, body.engine_account)
        cfg = engines.listar()[body.engine]
        from app.cliproxy_accounts import base_model
        account_models = body._engine_catalog if body._engine_catalog is not None else await _engine_models(body.engine, fresco=True)
        try:
            base = base_model(body.model or cfg["model"], account["prefix"])
            catalog = cliproxy.validate_models(cfg, base, account, account_models)
        except ValueError as exc:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
        body = body.model_copy(update={"model": base})
    if body.read_only:
        from app.orq_readonly import prepare
        try:
            await asyncio.to_thread(prepare, body.cwd, runtime_dirs=(body.config_dir or "",))
        except ValueError as exc:
            raise HTTPException(400, detail=erro("erro_criacao_sessao", str(exc))) from None
    if body.codex_account is not None and body.provider != "codex":
        raise HTTPException(400, detail=erro("codex_account_so_codex",
                                             "codex_account só vale para provider codex"))
    codex_account_obj = None
    codex_service = _codex_service() if body.provider == "codex" else None
    if body.provider == "codex":
        codex_account_obj = _resolve_codex_account(body.codex_account)
        if body.read_only:
            try:
                await asyncio.to_thread(prepare, body.cwd, runtime_dirs=(str(codex_account_obj.home),))
            except ValueError as exc:
                raise HTTPException(400, detail=erro("erro_criacao_sessao", str(exc))) from None
        if body.codex_account is not None:
            _codex_require_idle_preparation(codex_account_obj, codex_service)
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    # Mesma guarda do config_dir. Codex nao usa spawn_command/tmux desse jeito, entao motor + codex e
    # pedido incoerente — 400, nao "ignora e segue".
    if body.engine is not None:
        if body.provider != "claude":
            raise HTTPException(400, detail=erro("erro_motor_sem_claude", "motor so vale para provider claude"))
        if body.engine not in await asyncio.to_thread(engines.listar):
            raise HTTPException(400, detail=erro("erro_motor_invalido", "motor invalido"))
    # permission_mode só vale para claude — e para o Codex sem terminal (vira sandbox/approval).
    if body.permission_mode is not None and body.provider != "claude" \
            and not (body.provider == "codex" and body.headless):
        raise HTTPException(409, detail=erro("erro_permissao_so_claude", "modo de permissao so vale para claude"))
    if body.omp_profile and body.provider != "omp":
        raise HTTPException(400, detail=erro("erro_perfil_so_omp", "perfil so vale para provider omp"))
    if body.subagent_model is not None:
        if body.provider != "claude" or body.engine:
            raise HTTPException(400, detail=erro("erro_subagente_so_claude",
                                                 "modelo dos subagentes so vale para claude sem motor"))
        try:
            model_args.validar("claude", body.subagent_model, None)
        except ValueError as e:
            raise HTTPException(400, str(e).replace("model:", "subagent_model:", 1)) from None
    # Mesma regra das linhas acima, pro model/effort: recusa ANTES de qualquer efeito no disco,
    # inclusive pro provedor fora de escopo (codex/kimi) quando alguem pedir escolha — o valor
    # entraria num comando de shell montado por concatenacao.
    try:
        # O modo do Codex sem terminal tem lista própria (sem_terminal.MODOS), validada no registry.
        model_args.validar(body.provider, body.model, body.effort,
                           None if body.provider == "codex" else body.permission_mode)
    except ValueError as e:
        # permission_mode fora da lista deve ser 409 com código específico, não 400 genérico
        msg = str(e)
        if "permission_mode" in msg:
            raise HTTPException(409, detail=erro("erro_permissao_invalida", msg)) from None
        raise HTTPException(400, str(e)) from None
    # O nível do Codex não tem lista fechada em model_args (varia POR MODELO), então quem cruza
    # modelo×nível é o catálogo. Sem isto, `--effort ultra` num `gpt-5.5` sobe a sessão e o binário
    # descarta o nível calado — sucesso reportado sobre escolha que não valeu.
    if body.provider == "codex" and (body.model or body.effort or body.service_tier is not None):
        try:
            checar_kw = ({"codex_home": codex_account_obj.home}
                         if body.codex_account is not None else {})
            if body.service_tier is not None:
                checar_kw["service_tier"] = body.service_tier
            await asyncio.to_thread(codex_models.checar_escolha, body.model, body.effort,
                                    **checar_kw)
        except ValueError as e:
            raise HTTPException(422, detail=erro("erro_codex_escolha_invalida", str(e), erro=str(e))) from None
        except codex_models.CodexIndisponivel as e:
            if body.service_tier == "priority":
                raise HTTPException(502, detail=erro("erro_codex_catalogo_invalido",
                                                     f"Fast não pôde ser conferido: {e}", erro=str(e))) from None
            # Catálogo fora do ar (ou `codex` ausente — o CodexAusente é um RuntimeError) não pode
            # IMPEDIR de abrir sessão: mesma decisão da janela do motor, logo abaixo. A escolha
            # segue pro comando e o CLI decide. A falha não some — fica no log.
            _log.warning("codex: catalogo indisponivel, escolha nao conferida: %s", e)
        except (codex_models.CodexRecusado, codex_models.CodexRespostaInvalida) as e:
            raise HTTPException(502, detail=erro("erro_codex_catalogo_invalido", str(e),
                                                 erro=str(e))) from None

    if body.new_branch and body.branch is None:
        raise HTTPException(400, detail=erro("erro_criacao_sessao", "branch nova sem nome"))
    if body.branch is not None:
        try:
            root = await asyncio.to_thread(_allowed_scan_root, body.cwd)
            source = body.cwd
            # Recusa antes de criar a worktree, em vez de criar e desfazer no registry.create.
            if not sanitize_session_name(body.name):
                raise GitError(400, "nome de sessão inválido")
            worker = asyncio.create_task(asyncio.to_thread(
                git_ops.create_branch_worktree, source, body.branch, root,
                new_branch=body.new_branch, base=body.base))
            try:
                path, created = await asyncio.shield(worker)
            except asyncio.CancelledError:
                path, created = await asyncio.shield(worker)
                if created:
                    worktree.update(source=source, path=path,
                                    new_branch=body.branch if body.new_branch else None)
                raise
        except (FsError, GitError) as exc:
            raise HTTPException(exc.status, detail=erro("erro_criacao_sessao", exc.detail)) from None
        if created:
            worktree.update(source=source, path=path,
                            new_branch=body.branch if body.new_branch else None)
        body.cwd = worktree["cwd"] = path

    # Janela do modelo escolhido, pra entrar no env do motor (Task 3). O número já está no cache do
    # catálogo do provedor (_engine_models); vir do navegador seria deixar um terceiro escolher uma
    # variável de ambiente — e ainda ficaria None justamente nos provedores que não reportam
    # context_length. Com motor mas sem modelo (ou vice-versa), nada a resolver: o env segue o motor.
    janela = None
    if body.engine and body.model:
        catalog_id = await asyncio.to_thread(engines.catalog_model, body.model)
        try:
            for m in await (_fixed_engine_models(body.engine, body.engine_account)
                            if body.engine_account else _engine_models(body.engine)):
                if m["id"] == catalog_id:
                    janela = 1_000_000 if catalog_id != body.model else m.get("context_length")
                    break
        except HTTPException:
            # _engine_models devolve 502 quando o cache expirou e o /v1/models não responde, e 409
            # quando o motor sumiu do arquivo entre a validação e aqui. A janela é enfeite: deixar
            # essa chamada derrubar a criação faria o provedor fora do ar IMPEDIR de abrir sessão —
            # coisa que hoje não acontece, e que contradiz o Step 5 da Task 5 ("provedor parado: a
            # sessão ainda cria"). A sessão sobe sem a var e o CLI usa o default dele.
            janela = None

    codex_lease = None
    if body.provider == "codex" and (body.codex_account is not None or codex_service is not None):
        if codex_service is None:
            raise HTTPException(503, detail=erro("codex_account_service_unavailable",
                                                 "serviço de contas Codex indisponível"))
        try:
            codex_lease = codex_service.reserve_creation(codex_account_obj)
        except codex_accounts.AccountError as exc:
            raise HTTPException(exc.status, detail=erro(exc.code, "criação da conta Codex recusada",
                                                        **exc.params)) from None

    async def _create_registry(kwargs: dict):
        """A criação é bloqueante; se o request morrer, o worker ainda precisa terminar."""
        nonlocal codex_lease
        def create():
            if body.engine_account is not None:
                kwargs["engine_account"] = body.engine_account
                kwargs["engine_models"] = account_models
            from app.account_lifecycle import AccountKey, acquire
            from contextlib import nullcontext
            home = (codex_account_obj.home if body.provider == "codex"
                    else Path(tmux.config_dir_de(body.config_dir)))
            guard = (acquire(AccountKey.new("codex" if body.provider == "codex" else "claude", home))
                     if body.provider in ("claude", "codex", "pi", "omp") else nullcontext())
            with guard:
                try:
                    info = registry.create(body.name, body.cwd, body.config_dir, **kwargs)
                except claude_customizations.CustomizationsError as exc:
                    raise HTTPException(exc.status, detail=erro(exc.code, exc.detail)) from None
            worktree["session_created"] = True
            # O mesmo nome pode estar no snapshot com o transcript da sessão encerrada.
            _invalidate_lists()
            return info

        worker = asyncio.create_task(asyncio.to_thread(create))
        if codex_lease is None:
            try:
                return await asyncio.shield(worker)
            except asyncio.CancelledError:
                await asyncio.shield(worker)
                raise
        try:
            info = await asyncio.shield(worker)
        except asyncio.CancelledError:
            try:
                info = await asyncio.shield(worker)
            except BaseException:
                codex_lease.release()
                codex_lease = None
                raise
            await _hold_codex_lease(info.name, codex_lease)
            codex_lease = None
            raise
        except BaseException:
            codex_lease.release()
            codex_lease = None
            raise
        await _hold_codex_lease(info.name, codex_lease)
        codex_lease = None
        return info

    # Um montador só: com conta nomeada e sem ela a sessão nasce com as mesmas escolhas.
    def _registry_kwargs() -> dict:
        kw = dict(provider=body.provider, engine=body.engine, model=body.model,
                  effort=body.effort, context_window=janela)
        if body.permission_mode is not None:
            kw["permission_mode"] = body.permission_mode
        if body.subagent_model is not None:
            kw["subagent_model"] = body.subagent_model
        if body.claude_customizations is not None:
            kw["claude_customizations"] = body.claude_customizations.model_dump()
        if _jev_efetivo(body.jev):
            kw["jev"] = True
        if body.initial_prompt is not None:
            kw["initial_prompt"] = body.initial_prompt
        if body.omp_profile:
            kw["omp_profile"] = body.omp_profile
        if body.codex_account is not None:
            kw["codex_account"] = body.codex_account
        if body.service_tier is not None:
            kw["service_tier"] = body.service_tier
        if body.read_only:
            kw["read_only"] = True
        if body.headless:
            kw["headless"] = True
        return kw

    # Reconciliar e criar a sessão sob a MESMA trava (ciclo_conta), só no caminho que consome o
    # config dir (Claude/Pi — o Codex tem conta propria e nao le config dir do Claude). Sem o ciclo, um DELETE da
    # conta no meio via a lista de sessões ainda vazia e apagaria a pasta embaixo da sessão que
    # está subindo (a criação roda em thread).
    if body.config_dir is not None and body.provider in ("claude", "pi", "omp"):
        alvo = Path(body.config_dir)
        if contas.e_conta(alvo):
            nome_conta = alvo.name.removeprefix(".claude-")
            _passo(body.name, "conta", conta=nome_conta)
            try:
                # `ciclo_conta` numa thread pelo mesmo motivo do DELETE: o `flock` do __enter__
                # bloqueia, e no event loop isso congelava o app inteiro quando duas operações de
                # conta se cruzavam. flock pertence ao descritor aberto, não à thread — tomar e
                # soltar de threads diferentes é válido.
                from app.account_lifecycle import GuardMode
                cm = contas.ciclo_conta(nome_conta, mode=GuardMode.SHARED)
                ciclo = await asyncio.to_thread(cm.__enter__)
                try:
                    try:
                        avisos = await asyncio.to_thread(ciclo.reconciliar,
                                                         sanitize_cwd(body.cwd))
                    except contas.ContaError as e:
                        # ContaError já carrega status HTTP (o hangar-conta imprime o detail). Deixar
                        # escapar viraria 500 com traceback — o usuário não saberia por que a
                        # abertura falhou (ex: Windows sem Modo Desenvolvedor recusando symlink).
                        raise HTTPException(e.status, e.detail) from None
                    except OSError as e:
                        raise HTTPException(500, detail=erro("erro_conta_reconciliacao_falhou",
                                             f"não consegui reconciliar a conta "
                                             f"{nome_conta}: {e}", nome_conta=nome_conta,
                                             erro=str(e))) from None
                    for aviso in avisos:
                        _log.warning("conta %s: %s", alvo.name, aviso)
                    try:
                        _passo(body.name, "criando")
                        info = await _create_registry(_registry_kwargs())
                        if body.headless:
                            # Hooks de SessionStart rodam enquanto a pessoa digita, não no 1º envio.
                            wake = {"engine_models": account_models} if body.engine_account else {}
                            get_adapter(CLAUDE_HEADLESS).acordar(info.name, **wake)
                        return info.model_copy(update={"avisos": list(avisos)})
                    except ValueError as e:
                        code = "erro_nome_em_uso" if "ja existe uma sessao" in str(e) else "erro_criacao_sessao"
                        raise HTTPException(409, detail=erro(code, str(e)))
                finally:
                    # Solta a trava sempre — inclusive quando o corpo levanta HTTPException.
                    await asyncio.to_thread(cm.__exit__, None, None, None)
            except contas.ContaError as e:
                # Conta sumiu entre a validação e a trava (ex: DELETE concorrente).
                raise HTTPException(e.status, e.detail) from None
    try:
        _passo(body.name, "criando")
        info = await _create_registry(_registry_kwargs())
        if body.headless and body.provider == "codex":
            # Aquece já: o app-server sobe e abre a thread agora, não no primeiro prompt.
            _tarefas_soltas.add(asyncio.create_task(_aquecer_codex_sem_terminal(info.name)))
        elif body.headless:
            wake = {"engine_models": account_models} if body.engine_account else {}
            get_adapter(CLAUDE_HEADLESS).acordar(info.name, **wake)
        return info
    except ValueError as e:
        code = "erro_nome_em_uso" if "ja existe uma sessao" in str(e) else "erro_criacao_sessao"
        raise HTTPException(409, detail=erro(code, str(e)))


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def kill_session(name: str, by: str | None = None):
    # 500 quando a sessao SOBREVIVE ao kill — mesmo padrao do /rename logo abaixo, que ja confere e
    # responde 404/500. Antes era {"ok": true} incondicional: o card sumia da UI e a sessao reaparecia
    # na varredura seguinte, sem fila e sem pareamento (ver SessionRegistry.kill).
    # Os peers são lidos ANTES do kill: registry.kill -> _clear_pair já limpa o sidecar, e depois
    # dele ninguém sabe quem ficou.
    await asyncio.to_thread(_recusa_orq, name)
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    try:
        await asyncio.to_thread(registry.kill, name)
    except KillFailed as e:
        raise HTTPException(500, str(e))
    finally:
        await asyncio.to_thread(_invalidate_lists)
    plugin_bridge.esquecer(name)
    forget_frame(name)
    if await asyncio.to_thread(share_store.revoke_session, name):
        await asyncio.to_thread(share_api.sync_tunnel)
    warn = None
    if link:
        errs = await _avisar_saida(name, link["peers"])
        if errs:
            warn = erro("erro_pareamento_saida_falhou",
                        "aviso de saída falhou: " + "; ".join(
                            f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
                        avisos=errs)
    return {"ok": True, "warning": warn}


class ModoExecucaoBody(_StrictBody):
    terminal: bool = Field(strict=True)


_OCUPADA = {
    "erro_sessao_iniciando": "a sessão ainda está iniciando — espere ela ficar pronta",
    "erro_sessao_esperando_resposta": "há uma permissão ou pergunta esperando resposta",
    "erro_sessao_trabalhando": "a sessão está trabalhando — espere ela terminar",
    "erro_fila_pendente": "há mensagens na fila esperando entrega",
}


async def _motivo_ocupada(name: str, headless: bool) -> str | None:
    """Código de `_OCUPADA` dizendo por que a sessão não pode trocar de modo (None = ociosa)."""
    if headless:
        from app.runtime_adapter import runtime_data
        from app import runtime_coordinator
        coordinator = runtime_coordinator.current()
        try:
            view = coordinator.source_view(name) if coordinator is not None else None
        except RuntimeError:
            return "erro_sessao_iniciando"
        view = view if view is not None else runtime_data(name)
        if view is not None:
            state = view.get("public_state") or {}
            if not view.get("initialized") or view.get("iniciando"):
                return "erro_sessao_iniciando"
            if state.get("state") == "awaiting_input" or view.get("pending") or view.get("question"):
                return "erro_sessao_esperando_resposta"
            if state.get("state") == "working" or view.get("in_progress"):
                return "erro_sessao_trabalhando"
            if state.get("state") not in ("idle", "dead"):
                return "erro_sessao_iniciando"
            # Entregue sem confirmação não segura a troca: ociosa, o ator já conferiu o transcript, e a
            # que não chegou (limite da conta, desistida) prendia a sessão para sempre. Quem reinicia
            # o processo a devolve à fila (`_requeue_unanswered`).
        else:
            sess = get_adapter(CLAUDE_HEADLESS)._sessions.get(name)
            if sess is not None and sess.vivo:
                if sess.iniciando:
                    return "erro_sessao_iniciando"
                if sess.pending or sess.question:
                    return "erro_sessao_esperando_resposta"
                if sess.in_progress:
                    return "erro_sessao_trabalhando"
    else:
        info = next((i for i in await registry.list_with_state() if i.name == name), None)
        if info is not None and info.state == "awaiting_input":
            return "erro_sessao_esperando_resposta"
        if info is not None and info.state != "idle":
            return "erro_sessao_trabalhando"
    fila = await asyncio.to_thread(PromptQueue(name).load)
    if any(e.get("delivered") is False for e in fila):
        return "erro_fila_pendente"
    return None


def _requeue_unanswered(name: str) -> int:
    """Volta à fila a mensagem entregue que a sessão ociosa nunca confirmou: o processo que a
    recebeu não a gravou, e a vida nova (outra conta, outro modo) precisa responder. Só sob a trava
    de entrega e dentro da troca, para nenhum drain correr no meio. Desistida e saída local ficam.

    Volta como linha NOVA: a antiga carrega a operação já aceita no diário da fila, e o drain com o
    mesmo id devolveria a resposta guardada sem enviar nada. A antiga é confirmada (sai da tela).
    O que já está no transcript só tinha a confirmação atrasada: é confirmado, nunca reenviado."""
    meta = headless_sessions.load(name)
    if meta is None:
        return 0
    committed = committed_user_lines(str(get_adapter(CLAUDE_HEADLESS).transcript_path_de(meta)))
    if committed is None:
        _log.warning("transcript de %s ilegível: mensagens sem confirmação ficam como estão", name)
        return 0
    queue = PromptQueue(name)
    requeued = 0
    for row in queue.load():
        if (row.get("delivered") is not True or row.get("confirmed") or row.get("desistiu")
                or _saida_local(row)):
            continue
        text, row_id = row.get("text") or "", str(row.get("id"))
        if text.strip() and text.strip() not in committed:
            queue.append(text, pre_transcript=bool(row.get("pre_transcript")))
            requeued += 1
        queue.confirm_delivered(apenas=lambda r, row_id=row_id: str(r.get("id")) == row_id)
    if requeued:
        _log.info("%d mensagem(ns) sem confirmação de %s voltaram à fila", requeued, name)
    return requeued


@app.post("/api/sessions/{name}/recarregar", dependencies=[Depends(require_auth)])
async def recarregar_sessao(name: str):
    """Recicla o processo de uma sessão Claude sem terminal na mesma conversa (`--resume`): é o
    jeito de ela reler MCP, hooks e settings da conta. Só ociosa e sem nada em aberto."""
    from app.conversation_transfer import transfer_for_session, TransferPhase, recover_transfer, TransferError, public_error
    record = await asyncio.to_thread(transfer_for_session, name)
    if record and record.phase == TransferPhase.RESTORE_FAILED:
        operation = asyncio.create_task(_durante_troca(name, recover_transfer(registry, record), transfer=True))
        try:
            return await asyncio.shield(operation)
        except asyncio.CancelledError:
            try:
                await operation
            finally:
                raise
        except TransferError as exc:
            raise HTTPException(exc.status, detail=public_error(exc)) from None
    from app.conversation_transfer import transfer_operation, require_available
    try:
        async with transfer_operation(name):
            await asyncio.to_thread(require_available, name)
            return await _reload_session(name)
    except TransferError as exc:
        raise HTTPException(exc.status, detail=public_error(exc)) from None


async def _reload_session(name: str):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider == "codex":
        # No Codex é a saída de um turno que nunca fecha, então vale com a sessão trabalhando.
        codex = get_adapter("codex")
        async with codex.delivery_lock(name):
            try:
                await codex.restart(name)
            except ValueError as exc:
                raise HTTPException(409, detail=erro("erro_recarregar_so_sem_terminal", str(exc)))
            except Exception as exc:
                # A sessão antiga já saiu da memória; o motivo fica em `problema` e o
                # watch_sessions tenta de novo. Quem clicou precisa saber que não voltou.
                _log.warning("codex: reiniciar falhou name=%s: %s", name, exc)
                raise HTTPException(502, detail=erro("erro_codex_nao_reiniciou",
                                                     f"o Codex não subiu de novo: {str(exc)[:200]}"))
        return {"ok": True}
    if info.provider != "claude" or not _headless(name):
        raise HTTPException(409, detail=erro("erro_recarregar_so_sem_terminal",
                                             "recarregar só vale para sessão Claude sem terminal"))
    hl = get_adapter(CLAUDE_HEADLESS)
    async with hl.delivery_lock(name):
        motivo = await _motivo_ocupada(name, True)
        if motivo:
            raise HTTPException(409, detail=erro(motivo, _OCUPADA[motivo]))
        await hl.recarregar(name)
    return {"ok": True}


@app.post("/api/sessions/{name}/modo-execucao", dependencies=[Depends(require_auth)])
async def modo_execucao(name: str, body: ModoExecucaoBody):
    """Troca uma sessão entre terminal (pane tmux) e sem terminal, na mesma conversa.
    Só ociosa; o processo novo sobe já no clique, pra a primeira mensagem não pagar a largada."""
    return await _durante_troca(name, _trocar_modo(name, body))


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
            await _durante_troca(record.name, recover_transfer(registry, record), transfer=True)
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


async def _durante_troca(name: str, troca, *, transfer: bool = False):
    from app.conversation_transfer import transfer_operation, require_available, TransferError, public_error
    if transfer:
        return await _during_transfer_life(name, troca)
    try:
        async with transfer_operation(name):
            await asyncio.to_thread(require_available, name)
            return await _during_transfer_life(name, troca, require_idle=True)
    except TransferError as exc:
        troca.close()
        raise HTTPException(exc.status, detail=public_error(exc)) from None
    except BaseException:
        troca.close()       # corrotina que nunca rodou não pode ficar sem await
        raise


async def _during_transfer_life(name: str, troca, *, require_idle: bool = False):
    # A troca muda a identidade da sessão (sidecar <-> pane tmux); sem atualizar, a varredura
    # revogaria o convite de uma sessão que continua viva. `changing_mode` a segura no meio.
    if name in share_api.changing_mode:
        return await troca
    share_api.changing_mode.add(name)
    try:
        from app import runtime_coordinator
        coordinator = runtime_coordinator.current()
        if coordinator is not None and coordinator.managed_queue(name):
            async def check_idle():
                motivo = await _motivo_ocupada(name, _headless(name))
                if motivo:
                    raise HTTPException(409, detail=erro(motivo, _OCUPADA[motivo]))
            if require_idle:
                return await coordinator.change(name, lambda: troca, preflight=check_idle)
            return await coordinator.change(name, lambda: troca)
        return await troca
    finally:
        troca.close()
        # Também na falha: uma troca que morreu no meio pode já ter mudado a identidade.
        try:
            def _move_life():
                life = session_life(name)
                share_store.set_life(name, life)
                try:
                    guest_users.set_life(name, life)
                except Exception:
                    # Não troca o resultado da troca de modo pelo erro de gravar o dono.
                    _log.exception("[guests] dono de %s nao acompanhou a troca de modo", name)
            await asyncio.to_thread(_move_life)
        finally:
            share_api.changing_mode.discard(name)
            await asyncio.to_thread(_invalidate_lists)


async def _trocar_modo(name: str, body: ModoExecucaoBody):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider == "codex":
        if info.headless != body.terminal:
            return {"ok": True, "terminal": body.terminal}
        codex = get_adapter("codex")
        async with codex.delivery_lock(name):
            task = asyncio.create_task(codex.open_terminal(name) if body.terminal else codex.open_headless(name))
            try:
                await asyncio.shield(task)
            except asyncio.CancelledError:
                await task
                raise
            except Exception as exc:
                raise HTTPException(409, detail=erro("erro_troca_modo", f"não troquei de modo: {exc}", erro=str(exc))) from exc
        # Fora do laço: com o Rust dono, esquecer é uma chamada à ponte.
        await asyncio.to_thread(registry._forget, name)
        return {"ok": True, "terminal": body.terminal}
    if info.provider != "claude":
        raise HTTPException(409, detail=erro("erro_modo_so_claude", "a troca de modo só vale para sessões Claude"))
    headless = _headless(name)
    if headless != body.terminal:
        return {"ok": True, "terminal": body.terminal}
    hl = get_adapter(CLAUDE_HEADLESS)
    # A trava de entrega do headless: nenhum drain sobe processo no meio da troca.
    async with hl.delivery_lock(name):
        motivo = await _motivo_ocupada(name, headless)
        if motivo:
            raise HTTPException(409, detail=erro(motivo, _OCUPADA[motivo]))
        if headless:
            await asyncio.to_thread(_requeue_unanswered, name)
            try:
                await asyncio.to_thread(registry.para_terminal, name)
            except ValueError as e:
                if headless_sessions.exists(name):
                    hl.acordar(name)   # sidecar restaurado: religa o processo que a troca matou
                raise HTTPException(409, detail=erro("erro_troca_modo", f"não troquei de modo: {e}", erro=str(e)))
        else:
            modo = await asyncio.to_thread(perm_mode.ler_modo, name)
            try:
                await asyncio.to_thread(registry.para_headless, name, modo)
            except KillFailed as e:
                raise HTTPException(500, str(e))
            except (ValueError, OSError) as e:
                raise HTTPException(409, detail=erro("erro_troca_modo", f"não troquei de modo: {e}", erro=str(e)))
            try:
                await hl.ensure_running(name, esperar_pronta=False)
            except Exception as e:
                _log.warning("troca para sem terminal: processo nao subiu name=%s; voltando ao terminal", name, exc_info=True)
                try:
                    await asyncio.to_thread(registry.para_terminal, name)
                except Exception:
                    _log.exception("troca para sem terminal: volta ao terminal falhou name=%s", name)
                raise HTTPException(409, detail=erro("erro_troca_modo", f"não troquei de modo: {e}", erro=str(e)))
    return {"ok": True, "terminal": body.terminal}


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


# Continuar reenvia o contexto inteiro no primeiro turno: conta quase no fim acaba nele. A partir de
# LOW a tela avisa e pede confirmação; a partir de FULL não aceita.
ACCOUNT_LOW_PCT = 95.0
ACCOUNT_FULL_PCT = 99.0


def _account_targets(atual: str | None) -> list[dict]:
    """Contas Claude para onde a conversa pode ir, sem a atual: a de mais folga primeiro, sem
    leitura de cota depois e as cheias no fim. `pct` é a janela mais cheia (None = sem leitura)."""
    from app import cotas
    lidas = {c.id.removeprefix("claude:"): c for c in cotas.cotas_claude()
             if c.estado == "lida" and c.janelas}
    out = []
    for c in list_config_dirs(ordered=False):
        if atual and Path(c.path).resolve() == Path(atual).resolve():
            continue
        cota = lidas.get(c.path)
        pct = max(j.pct for j in cota.janelas) if cota else None
        out.append({"path": c.path, "label": c.label, "pct": pct,
                    "low": pct is not None and pct >= ACCOUNT_LOW_PCT,
                    "full": pct is not None and pct >= ACCOUNT_FULL_PCT})
    return sorted(out, key=lambda d: (d["full"], d["pct"] is None, d["pct"] or 0))


@app.get("/api/sessions/{name}/conta", dependencies=[Depends(require_auth)])
async def contas_destino(name: str):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    atual = ((headless_sessions.load(name) or {}).get("config_dir") if info.headless else
             str(_session_config_dir(name) or Path.home() / ".claude"))
    # No motor, mesmo o diretório atual é um destino: ele volta para a conta Claude sem mover nada.
    return await asyncio.to_thread(_account_targets, None if info.engine else atual)


@app.post("/api/sessions/{name}/conta", dependencies=[Depends(require_auth)])
async def trocar_conta(name: str, body: AccountMoveBody):
    """A mesma conversa continua noutra conta Claude, com o mesmo nome: para o processo, muda o
    transcript de conta e reabre com `--resume`. Só ociosa, como a troca de modo."""
    if body.credential_id is not None:
        from app.conversation_transfer import transfer_claude_to_codex, TransferError, public_error
        operation = asyncio.create_task(_durante_troca(name, transfer_claude_to_codex(
            registry, name, body.credential_id, body.source_life, body.model, body.effort,
            source_jsonl=body.source_jsonl), transfer=True))
        try:
            return await asyncio.shield(operation)
        except asyncio.CancelledError:
            try:
                await operation
            finally:
                raise
        except TransferError as exc:
            raise HTTPException(exc.status, detail=public_error(exc)) from None
    if body.engine_account is not None:
        return await _durante_troca(name, _trocar_conta(name, None, engine_account=body.engine_account))
    alvo = next((d for d in await asyncio.to_thread(_account_targets, None) if d["path"] == body.config_dir), None)
    if alvo is None:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if alvo["full"]:
        raise HTTPException(409, detail=erro("erro_conta_cheia", f"a conta {alvo['label']} está em {alvo['pct']:.0f}% da cota",
                                             conta=alvo["label"], pct=alvo["pct"]))
    return await _durante_troca(name, _trocar_conta(name, body.config_dir))


def _engine_fast_selection(name: str) -> tuple[str | None, str]:
    meta = headless_sessions.load(name)
    if meta is not None:
        from app import runtime_coordinator
        coordinator = runtime_coordinator.current()
        try:
            view = coordinator.source_view(name) if coordinator is not None else None
        except RuntimeError:
            raise HTTPException(409, detail=erro("erro_sessao_iniciando", "estado da sessão indisponível; aguarde a reposição")) from None
        model = (view or {}).get("model")
        if model is None:
            model = get_adapter(CLAUDE_HEADLESS).escolhas(name)[0]
        return model or meta.get("model"), meta.get("service_tier") or "default"
    pane = registry._pane_of(name)
    agent = registry_mod._pid_do_agente((pane or {}).get("pid"))
    if not agent or pane is None:
        return None, "default"
    model = procinfo._model_of(agent)[0]
    jsonl, tracked = registry.resolve_tracked(name, pane["cwd"])
    if jsonl and tracked:
        current = registry_mod._escolhas_status(Path(jsonl).stem)[0]
        model = current or model
    return model, procinfo._env_var_of(agent, "CP_ENGINE_SERVICE_TIER") or "default"


async def _trocar_conta(name: str, destino: str | None, *, engine_account: str | None = None,
                       model: str | None = None, effort: str | None = None,
                       context_window: int | None = None, engine_models: list[dict] | None = None,
                       service_tier: str | None = None):
    hl = get_adapter(CLAUDE_HEADLESS)
    async with hl.delivery_lock(name):
        # A troca anterior pode ter mudado motor, conta e transporte enquanto este pedido esperava.
        await asyncio.to_thread(_invalidate_lists)
        info = await _cached_info(name)
        if not info:
            raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
        if info.provider != "claude":
            raise HTTPException(409, detail=erro("erro_conta_so_claude", "só sessão Claude troca de conta por esta rota"))
        headless = _headless(name)
        current_meta = headless_sessions.load(name) if headless else None
        if current_meta is not None:
            info = info.model_copy(update={"engine": current_meta.get("engine"),
                                           "engine_account": current_meta.get("engine_account"),
                                           "headless": True})
        if service_tier is not None:
            current_model, _ = await asyncio.to_thread(_engine_fast_selection, name)
            if not await asyncio.to_thread(cliproxy.supports_fast, info.engine, current_model):
                raise HTTPException(400, detail=erro("erro_fast_indisponivel", "Fast exige GPT no CLIProxyAPI local"))
        elif engine_account is not None:
            if not info.engine:
                raise HTTPException(400, detail=erro("erro_cliproxy_conta", "esta sessão não usa o CLIProxyAPI local"))
            if (model is not None or effort is not None) and info.engine_account != engine_account:
                raise HTTPException(409, detail=erro("erro_cliproxy_conta", "a conta da sessão mudou; atualize a lista de modelos"))
            account = await asyncio.to_thread(_fixed_engine_account, info.engine, engine_account)
        elif info.engine:
            try:
                local_engine = cliproxy.is_local_engine(engines.listar().get(info.engine, {}))
            except ValueError as exc:
                raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
            if not local_engine:
                raise HTTPException(409, detail=erro("erro_conta_so_claude", "só conta Claude ou motor CLIProxyAPI local troca de conta"))
        atual = ((current_meta or {}).get("config_dir") if headless else
                 str(_session_config_dir(name) or Path.home() / ".claude"))
        if engine_account is None and not info.engine and atual and Path(atual).resolve() == Path(destino).resolve():
            return {"ok": True, "config_dir": destino}
        motivo = await _motivo_ocupada(name, headless)
        if motivo:
            raise HTTPException(409, detail=erro(motivo, _OCUPADA[motivo]))
        chosen_model = None
        if engine_account is not None:
            source_model, _ = await asyncio.to_thread(_engine_fast_selection, name)
            source_model = model or source_model or engines.listar()[info.engine]["model"]
            base = source_model.split("/", 1)[-1]
            from app.cliproxy_accounts import prefix_model
            try:
                chosen_model = prefix_model(base, account["prefix"])
            except ValueError as exc:
                raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
            account_models = engine_models if engine_models is not None else await _engine_models(info.engine, fresco=True)
            try:
                cliproxy.validate_models(engines.listar()[info.engine], chosen_model, account, account_models)
                model_args.validar("claude", chosen_model, effort)
            except (ValueError, KeyError) as exc:
                raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
            if info.engine_account == engine_account and model is None and effort is None:
                return {"ok": True, "engine_account": engine_account}
        # Terminal passa por sem terminal parada: o sidecar guarda as escolhas e a conta, e nenhum
        # processo sobe até a conversa estar no lugar.
        if headless:
            cano = (((headless_sessions.load(name) or {}).get("cano")) or {}).get("pid")
            pids = await asyncio.to_thread(_arvore_de, cano)
            await hl.parar(name)
        else:
            pane = await asyncio.to_thread(registry._pane_of, name)
            pids = await asyncio.to_thread(_arvore_de, (pane or {}).get("pid"))
            modo = await asyncio.to_thread(perm_mode.ler_modo, name)
            try:
                extra = {"for_account_move": True} if info.engine else {}
                await asyncio.to_thread(registry.para_headless, name, modo, **extra)
            except KillFailed as e:
                raise HTTPException(500, str(e))
            except (ValueError, OSError) as e:
                raise HTTPException(409, detail=erro("erro_troca_conta", f"não troquei de conta: {e}", erro=str(e)))

        async def reabrir() -> str | None:
            """Reabre como estava; devolve o motivo quando o terminal não voltou (a sessão segue sem terminal)."""
            if headless:
                if engine_account is not None or info.engine:
                    try:
                        hl.reset_start_attempts(name)
                        if await hl.ensure_running(name, require_initialize=True,
                                                   engine_models=account_models if engine_account is not None else None) is None:
                            raise RuntimeError("a sessão não reabriu")
                    except Exception as exc:
                        return str(exc)
                else:
                    hl.acordar(name)
                return None
            expected_meta = headless_sessions.load(name)
            started = False
            try:
                kwargs = {"engine_models": account_models} if engine_account is not None else {}
                await asyncio.to_thread(registry.para_terminal, name, **kwargs)
                started = True
                if engine_account is not None or info.engine:
                    if expected_meta is None:
                        raise RuntimeError("não consegui conferir a identidade da sessão retomada")
                    await asyncio.to_thread(registry.wait_for_claude, name, expected_meta)
                return None
            except Exception as e:
                _log.exception("troca de conta: terminal de %s não voltou", name)
                if started and expected_meta is not None:
                    pane = await asyncio.to_thread(registry._pane_of, name)
                    new_pids = await asyncio.to_thread(_arvore_de, (pane or {}).get("pid"))
                    if (await asyncio.to_thread(registry_mod.tmux.has_session, name)
                            and not await asyncio.to_thread(registry_mod.tmux.kill_session, name)):
                        raise HTTPException(409, detail=erro("erro_troca_conta", "a reabertura falhou e o terminal não encerrou para restaurar a sessão")) from e
                    if not await asyncio.to_thread(_saiu, new_pids):
                        raise HTTPException(409, detail=erro("erro_troca_conta", "o Claude novo não saiu; restauração recusada para não duplicar a conversa")) from e
                    headless_sessions.restaurar(expected_meta)
                if engine_account is None and not info.engine:
                    hl.acordar(name)   # sidecar restaurado: a conversa segue sem terminal
                return str(e)

        # O claude grava as últimas linhas pelo caminho ao sair: mover antes disso recria o arquivo na conta de
        # origem, com o mesmo id, e o processo novo teria companhia no mesmo .jsonl.
        if not await asyncio.to_thread(_saiu, pids):
            await reabrir()
            raise HTTPException(409, detail=erro("erro_troca_conta", "não troquei de conta: o processo antigo não saiu; a sessão segue na conta de antes",
                                                 erro="processo vivo"))
        falha = None
        original_meta = None
        movida: tuple[str, str, str | None] | None = None
        try:
            if headless:
                await asyncio.to_thread(_requeue_unanswered, name)
            meta = headless_sessions.load(name)
            if meta is None:
                raise RuntimeError("sessão sem o arquivo de estado")
            original_meta = dict(meta)
            jsonl = Path(hl.transcript_path_de(meta))
            if service_tier is not None:
                if current_model is not None:
                    original_meta["model"] = current_model
                changes = {"service_tier": service_tier, "problema": None,
                           **({"model": current_model} if current_model is not None else {})}
            elif engine_account is not None:
                changes = {"engine_account": engine_account, "engine_credential_id": account["credential_id"],
                           "engine_account_base_url": account["base_url"],
                           "model": chosen_model, "problema": None}
                if model is not None:
                    changes["context_window"] = context_window
                if effort is not None:
                    changes["effort"] = effort
            else:
                if (jsonl.exists() and Path(meta.get("config_dir") or Path.home() / ".claude").resolve()
                        != Path(destino).resolve()
                        and await asyncio.to_thread(move_conversation, jsonl.parent.name, meta["session_id"], destino)):
                    movida = (jsonl.parent.name, meta["session_id"], meta.get("config_dir"))
                changes = {"config_dir": destino, "problema": None}
                if info.engine:
                    changes.update(engine=None, engine_account=None, engine_credential_id=None,
                                   engine_account_base_url=None, model=None, context_window=None, service_tier=None)
                # Confiança na pasta é por conta: sem isto o terminal (agora ou na troca de modo) abre no aviso, em "No, exit".
                await asyncio.to_thread(registry_mod._pretrust_cwd, meta["cwd"], destino)
            # O aviso da conta anterior (limite batido, sem login) não vale na nova.
            if headless_sessions.update(name, **changes) is None:
                raise RuntimeError("não gravei a conta nova no arquivo de estado da sessão")
            hl.esquecer_problema(name)
        except FileExistsError:
            falha = HTTPException(409, detail=erro("erro_conversa_ja_na_conta", "a conta destino ja tem esta conversa"))
        except Exception as e:
            _log.exception("mover conversa de %s para %s falhou", name, destino)
            onde = "na conta de antes"
            if movida:
                # A conversa já foi, mas a sessão continua apontando para a conta de antes: ela volta junto.
                try:
                    await asyncio.to_thread(move_conversation, *movida)
                except Exception:
                    _log.exception("troca de conta: a conversa de %s ficou em %s", name, destino)
                    onde = f"em {destino}, mas a sessão aponta para a conta de antes"
            if original_meta is not None:
                try:
                    headless_sessions.restaurar(original_meta)
                except OSError:
                    _log.exception("troca de conta: não consegui restaurar as escolhas de %s", name)
                    onde = "com o arquivo de estado da sessão indisponível"
            falha = HTTPException(500, detail=erro("erro_mover_conversa", f"nao consegui mover a conversa de conta ({e}); ela ficou {onde}", erro=str(e)))
        motivo_terminal = await reabrir()
        if motivo_terminal and falha is None and original_meta is not None and (engine_account is not None or info.engine):
            rollback_error = None
            try:
                await hl.parar(name)
                if movida:
                    await asyncio.to_thread(move_conversation, *movida)
                headless_sessions.restaurar(original_meta)
                rollback_error = await reabrir()
                if rollback_error and not headless:
                    hl.acordar(name)
            except Exception as exc:
                rollback_error = str(exc)
                _log.exception("troca de conta: restauração da sessão %s falhou", name)
            message = "a troca falhou; as escolhas anteriores foram restauradas"
            if rollback_error:
                message = "a troca falhou e a sessão anterior não reabriu"
            falha = HTTPException(409, detail=erro("erro_troca_conta", message,
                                                  erro=motivo_terminal, rollback_error=rollback_error))
        await asyncio.to_thread(registry._forget, name)
    if falha:
        raise falha
    if motivo_terminal:
        raise HTTPException(409, detail=erro("erro_troca_conta", f"a conversa foi para a conta nova, mas o terminal não voltou ({motivo_terminal}); ela segue sem terminal",
                                             erro=motivo_terminal))
    if service_tier is not None:
        return {"ok": True, "service_tier": service_tier}
    return {"ok": True, "engine_account": engine_account} if engine_account is not None else {"ok": True, "config_dir": destino}


def _arvore_de(pid: object) -> list[int]:
    """O processo e os descendentes dele; sem pid legível, nenhum."""
    try:
        root = int(pid)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return []
    return [root, *procinfo._descendant_pids(root)]


def _saiu(pids: list[int]) -> bool:
    """Espera os processos saírem; quem passar do prazo é morto à força. False = algum seguiu vivo mesmo assim."""
    windows = os.name == "nt"
    # No Windows a árvore sai em décimos de segundo quando sai; quem passou de 1,5 s (neto fora do
    # psmux, servidor MCP preso) não sai sozinho, e esperar 15 s por ele só atrasa a troca.
    prazo = 1.5 if windows else 15.0
    registry_mod._esperar_saida(pids, prazo)
    vivos = [p for p in pids if procinfo.pid_vivo(p)]
    if vivos and windows:
        taskkill = shutil.which("taskkill")
        if not taskkill:
            _log.warning("troca de conta: taskkill não encontrado; processos %s seguem vivos", vivos)
        else:
            # Uma chamada para todos e sem /T: o /T segue o ppid de agora, e num pid já reaproveitado
            # levaria junto a árvore de um processo alheio. Os netos já estão na foto tirada antes de parar.
            try:
                r = subprocess.run([taskkill, "/F", *(a for p in vivos for a in ("/PID", str(p)))],
                                   capture_output=True, text=True, errors="replace", timeout=10)
                if r.returncode != 0:
                    _log.warning("troca de conta: taskkill saiu com %s: %s", r.returncode,
                                 (r.stderr or r.stdout or "").strip()[:400])
            except (OSError, subprocess.SubprocessError):
                _log.warning("troca de conta: não consegui matar os processos %s", vivos, exc_info=True)
    elif vivos:
        import signal
        for p in vivos:
            try:
                os.kill(p, signal.SIGKILL)
            except OSError:
                _log.warning("troca de conta: não consegui matar o processo %s", p, exc_info=True)
    if vivos:
        _log.warning("troca de conta: processos %s não saíram em %.1f s; tentei matá-los", vivos, prazo)
        registry_mod._esperar_saida(vivos, 3.0)
    return not any(procinfo.pid_vivo(p) for p in pids)


class RenameBody(_StrictBody):
    new: str


@app.post("/api/sessions/{name}/rename", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def rename_session(name: str, body: RenameBody):
    # Claim, envio e compensação precisam terminar antes de mover a fila e cancelar a bomba.
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.legacy is not None:
        await coordinator.prepare_session(name, _provider_of(name))
        if coordinator.managed_queue(name) and coordinator.slot(name).binding.meta.get("terminal"):
            async def action():
                return await asyncio.to_thread(_rename_session, name, body)
            return await coordinator.change(name, action, new_name=sanitize_session_name(body.new), advance=False)
    async with AsyncExitStack() as stack:
        adapter = get_adapter("codex")
        for key in sorted({name, sanitize_session_name(body.new)}):
            await stack.enter_async_context(adapter.delivery_lock(key))
        task = asyncio.create_task(asyncio.to_thread(_rename_session, name, body))
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            await task
            raise


def _rename_guest_claim(name: str, new: str) -> None:
    # A sessão já foi renomeada; falhar aqui não pode pular a migração da fila e do bastão.
    try:
        guest_users.rename_session(name, new)
    except Exception:
        _log.exception("[guests] dono de %s nao acompanhou o rename para %s", name, new)


def _rename_session(name: str, body: RenameBody):
    from app import tmux
    _recusa_orq(name)
    # tmux nao aceita espaco/./: no nome -> sanitiza. O transcript NAO depende do nome (resolve por
    # /proc), entao renomear nao quebra o historico. Migra so o sidecar da fila (keyed por nome).
    new = sanitize_session_name(body.new)
    if not new:
        raise HTTPException(400, detail=erro("erro_nome_invalido", "nome invalido"))
    if _headless(name) or _codex_sem_terminal(name):
        # Sem pane: é só o sidecar (e o que é keyed por nome) que muda.
        if new == name:
            return {"ok": True, "name": name}
        if _session_exists(new):
            raise HTTPException(409, detail=erro("erro_nome_em_uso", "ja existe uma sessao com esse nome"))
        try:
            registry.rename(name, new)
        except ValueError as e:
            raise HTTPException(409, detail=erro("erro_nome_em_uso", str(e)))
        od, nd = bastao_mod.caminho(name), bastao_mod.caminho(new)
        if od.exists():
            atomico.substituir(od, nd)
        _invalidate_lists()
        forget_frame(name)
        share_store.rename(name, new)
        external_pairs.rename_local(name, new)
        _rename_guest_claim(name, new)
        return {"ok": True, "name": new}
    if not tmux.has_session(name):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if new == name:
        return {"ok": True, "name": name}
    if tmux.has_session(new) or headless_sessions.exists(new):
        raise HTTPException(409, detail=erro("erro_nome_em_uso", "ja existe uma sessao com esse nome"))
    # Atualiza a reserva antes do rename do tmux: durante o boot o sidecar ainda pode não existir.
    if registry_mod.apos_renomear_codex:
        registry_mod.apos_renomear_codex(name, new)
    try:
        renamed = tmux.rename_session(name, new)
    except Exception:
        if registry_mod.apos_renomear_codex:
            registry_mod.apos_renomear_codex(new, name)
        _codex_lease_rename_finished(name)
        raise
    if not renamed:
        if registry_mod.apos_renomear_codex:
            registry_mod.apos_renomear_codex(new, name)
        _codex_lease_rename_finished(name)
        raise HTTPException(500, detail=erro("sessao_falha_renomear", "falha ao renomear"))
    _codex_lease_rename_finished(new)
    registry.rename(name, new)  # migra o cache name->jsonl (senao serve transcript errado pos-rename)
    share_store.rename(name, new)
    external_pairs.rename_local(name, new)
    _rename_guest_claim(name, new)
    from app.pqueue import PromptQueue
    try:
        PromptQueue(name).rename(new)
        # O dossiê da passagem de bastão é keyed por nome do MESMO jeito que a fila: sem migrar
        # junto, a sucessora renomeada fica com um kick-off apontando pro caminho antigo e o
        # `prune` apaga o arquivo em 7 dias por não achar sessão viva com aquele nome.
        od, nd = bastao_mod.caminho(name), bastao_mod.caminho(new)
        if od.exists():
            atomico.substituir(od, nd)
    except OSError as e:
        # Não derruba o rename (que já aconteceu no tmux), mas APARECE: sidecar que não migrou é
        # fila perdida ou dossiê órfão, e nenhum dos dois pode sumir calado.
        _log.warning("rename %s -> %s: sidecar nao migrou: %s", name, new, e)
    return {"ok": True, "name": new}


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


@app.delete("/api/sessions/{name}/queue/{entry_id}", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


# --- Loop runner (harness bloco A) -------------------------------------------

class LoopCreate(_StrictBody):
    goal: str = Field(min_length=1)
    check_cmd: str | None = None
    max_iters: int = Field(default=10, ge=1, le=100)  # teto: loop nao vira gerador infinito de prompts
    require_branch: bool = True


class LoopResolve(_StrictBody):
    accept: bool


class LoopRefine(_StrictBody):
    goal: str = Field(min_length=1, max_length=2000)
    check_cmd: str | None = None


def _loop_ctx(name: str) -> "loop_mod.TickCtx | None":
    """Monta o TickCtx real da sessao CORRENTE (nome -> jsonl/cwd via registry, sobrevive /clear).
    deliver = enfileira delivered=False + drain (caminho unico de entrega; a entrada e duravel, entao
    o drain server-side reentrega depois) -> retorna True sempre; enqueue nunca dispara o fallback do
    run_tick (senao duplicaria a entrada). Sessao sumida -> loop failed + notify, return None."""
    info = next((i for i in registry.list() if i.name == name), None)
    if info is None or not info.jsonl:
        loop_mod._end(loop_mod.LoopLink(name), name, "failed", "sessão morta", push.notify_loop)
        return None
    jsonl = info.jsonl
    provider = info.provider

    def deliver(prompt: str) -> bool:
        PromptQueue(name).append(prompt, delivered=False)
        _drenar(name, jsonl, provider)
        return True

    return loop_mod.TickCtx(
        cwd=info.cwd or "",
        jsonl=jsonl,
        deliver=deliver,
        enqueue=lambda p: PromptQueue(name).append(p, delivered=False),
        notify=push.notify_loop,
        automations=automations_enabled,
        branch=branch_of,
        last_assistant=last_assistant_text,
        run_check=loop_mod._run_check,
        entry_delivered=lambda eid: PromptQueue(name).entry_delivered(eid),
    )


@app.post("/api/sessions/{name}/loop", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def loop_create(name: str, body: LoopCreate):
    info = next((i for i in registry.list() if i.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if getattr(info, "provider", "claude") != "claude":
        # Codex e outros nao sao tmux: sem hook de transicao, o tick nunca dispara -> loop ficaria
        # running mudo pra sempre. Recusa cedo em vez de criar um loop-zumbi.
        raise HTTPException(409, detail=erro("erro_loop_provider_invalido", "loop runner só suporta sessões claude"))
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur and cur["status"] in loop_mod.ACTIVE:
            raise HTTPException(409, detail=erro("erro_loop_ja_ativo", "já existe um loop ativo nesta sessão"))
        br = branch_of(info.git_dir) if info.git_dir else None
        if body.require_branch and br in ("main", "master"):
            raise HTTPException(409, detail=erro("erro_loop_branch_invalida", f"sessão está na branch {br} — crie uma branch ou desligue 'exigir branch'", br=br))
        d = loop_mod.new_loop(body.goal, body.check_cmd, body.max_iters, body.require_branch)
        entry = PromptQueue(name).append(body.goal, delivered=False)
        d["goal_entry_id"] = entry["id"]
        link.set(d)
    if info.jsonl:
        _drenar(name, info.jsonl, info.provider)   # entrega ja se a sessao estiver entregavel; senao o drain server-side entrega depois
    return {"loop": link.get()}


@app.get("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_get(name: str):
    info = next((i for i in registry.list() if i.name == name), None)
    suggestions = loop_mod.suggest_checks(info.cwd) if info and info.cwd else []
    return {"loop": loop_mod.LoopLink(name).get(), "suggestions": suggestions}


@app.delete("/api/sessions/{name}/loop", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def loop_stop(name: str):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        if link.get() is None:
            raise HTTPException(404, detail=erro("erro_loop_inexistente", "nenhum loop nesta sessão"))
        d = loop_mod._end(link, name, "stopped", "parado pelo usuário", push.notify_loop)
    return {"loop": d}


@app.post("/api/sessions/{name}/loop/refine", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def loop_refine(name: str, body: LoopRefine):
    """Refina o objetivo do loop via claude -p efemero (sonnet). Stateless — nao toca a sessao nem o
    sidecar; o {name} da rota so mantem a familia de URLs consistente. Falha do CLI -> 502.
    Sob o kill-switch mestre: refine dispara um agente autonomo, entao respeita automations_enabled."""
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    try:
        return {"goal": loop_mod.refine_goal(body.goal, body.check_cmd)}
    except loop_mod.ClaudePError as e:
        _log.warning("loop/refine falhou (%s): %s", name, e)
        raise HTTPException(502, str(e))


@app.post("/api/sessions/{name}/loop/resolve", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def loop_resolve(name: str, body: LoopResolve):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur is None or cur["status"] != "done_claimed":
            raise HTTPException(409, detail=erro("erro_loop_estado_errado", "loop não está aguardando confirmação"))
        if body.accept:
            d = loop_mod._end(link, name, "done", "confirmado pronto", push.notify_loop)
            return {"loop": d}
        # reject: conta iteracao e re-prompta (reusa o MESMO helper do run_tick)
        cur["status"] = "running"
        link.set(cur)
        if cur["iter"] + 1 > cur["max_iters"]:
            d = loop_mod._end(link, name, "exhausted", f"esgotou {cur['max_iters']} iterações",
                              push.notify_loop)
            return {"loop": d}
        ctx = _loop_ctx(name)
        if ctx is None:
            return {"loop": link.get()}
        loop_mod._reprompt(link, cur, "conclusão rejeitada pelo usuário", None,
                           ctx.deliver, ctx.enqueue)
    return {"loop": link.get()}


class ResumeBody(_StrictBody):
    # None = "escolha por mim" (caso seguro) ou pede confirmacao (caso ambiguo). uuid = o candidato que o
    # usuario escolheu no sheet de confirmacao.
    session_id: str | None = None


@app.post("/api/sessions/{name}/resume", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def resume_session(name: str, body: ResumeBody):
    # Relança uma sessao "sem id" com `claude --resume <uuid>` pra passar a rastrea-la (chat volta a abrir,
    # continuando a conversa). Sem session_id: se so ha esta sessao no cwd, retoma o transcript mais
    # recente direto; se ha outras (ambiguo), devolve os candidatos pro app confirmar antes.
    sid = body.session_id
    if sid is None:
        try:
            _, ambiguous, candidates = registry.resume_candidates(name)
        except ValueError as e:
            raise HTTPException(404, str(e))
        if not candidates:
            raise HTTPException(404, detail=erro("erro_transcript_ausente", "nenhum transcript pra retomar neste diretorio"))
        if ambiguous and len(candidates) > 1:
            return {"ambiguous": True, "candidates": candidates}
        sid = candidates[0]["session_id"]
    try:
        return registry.resume(name, sid)
    except ValueError as e:
        raise HTTPException(409, str(e))
    finally:
        _invalidate_lists()


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


@app.get("/api/sessions/{name}/plan-preview", dependencies=[Depends(require_auth)])
async def plan_preview(name: str, content: bool = True):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    if info.provider != "claude":
        return None
    plano = await asyncio.to_thread(plano_claude.descobrir, info.jsonl, info.cwd)
    if plano is None:
        return None
    resposta = {"name": plano.nome, "path": str(plano.caminho)}
    if plano.anchor_id is not None:
        resposta["anchor_id"] = plano.anchor_id
    if not content:
        return resposta
    try:
        resposta["markdown"] = await asyncio.to_thread(plano.caminho.read_text, encoding="utf-8")
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_plano_removido", "arquivo do plano não encontrado")) from None
    except OSError:
        raise HTTPException(500, detail=erro("erro_plano_ilegivel", "não foi possível ler o plano")) from None
    return resposta


async def _bastao_alvo(name: str, project: str | None, session_id: str | None,
                       config_dir: str | None, provider: str,
                       origem_codex_account: str | None = None) -> SessionInfo:
    """Origem VIVA pelo registry; morta pelo archive (project + session_id, como o resume do
    Arquivo). O gatilho automático pode chegar depois de o 429 derrubar o pane — sem isto o
    bastão só existia enquanto a origem respirava."""
    info = await _cached_info(name)
    if info and info.jsonl:
        if info.provider == "codex":
            try:
                owner = codex_accounts.account_for_rollout(Path(info.jsonl))
            except codex_accounts.AccountError as e:
                raise _erro_conta_codex(e) from None
            if owner is None and info.codex_home:
                raiz = Path(info.codex_home).expanduser().resolve(strict=False)
                owner = next((account for account in codex_accounts.list_accounts()
                              if account.home.expanduser().resolve(strict=False) == raiz), None)
            if owner is None and origem_codex_account is not None:
                raise _erro_conta_codex(codex_accounts.AccountError(
                    409, "codex_account_archive_mismatch",
                    {"account_id": origem_codex_account}))
            if owner is not None:
                if origem_codex_account is not None and owner.id != origem_codex_account:
                    raise _erro_conta_codex(codex_accounts.AccountError(
                        409, "codex_account_archive_mismatch",
                        {"account_id": origem_codex_account, "origin_account": owner.id}))
                info.codex_home = str(owner.home.resolve(strict=False))
                info.conta = f"codex:{info.codex_home}"
        return info
    if not (project and session_id):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    # Mesma guarda de archive_history/resume_archived: sem ela, qualquer pasta da máquina vira
    # "conta" e o GET lê <pasta>/projects/<proj>/<uuid>.jsonl.
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    try:
        if provider == "codex":
            p = await asyncio.to_thread(archive_jsonl, project, session_id, config_dir, provider,
                                        origem_codex_account)
            owner = codex_accounts.account_for_rollout(p)
            if owner is None:
                raise FileNotFoundError(session_id)
            if origem_codex_account is not None and owner.id != origem_codex_account:
                raise _erro_conta_codex(codex_accounts.AccountError(
                    409, "codex_account_archive_mismatch",
                    {"account_id": origem_codex_account, "origin_account": owner.id}))
            cwd = await asyncio.to_thread(archive_cwd, project, session_id, config_dir, provider,
                                          owner.id)
            return SessionInfo(name=name, cwd=cwd, jsonl=str(p), provider=provider,
                               codex_home=str(owner.home.resolve(strict=False)),
                               conta=f"codex:{owner.home.resolve(strict=False)}")
        p = await asyncio.to_thread(archive_jsonl, project, session_id, config_dir, provider)
        cwd = await asyncio.to_thread(archive_cwd, project, session_id, config_dir, provider)
    except codex_accounts.AccountError as e:
        raise _erro_conta_codex(e) from None
    except (ValueError, FileNotFoundError):
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    return SessionInfo(name=name, cwd=cwd, jsonl=str(p), provider=provider)


@app.get("/api/sessions/{name}/bastao", dependencies=[Depends(require_auth)])
async def bastao(name: str, project: str | None = None, session_id: str | None = None,
                 config_dir: str | None = None, provider: str = "claude",
                 origem_codex_account: str | None = None):
    """Dossiê de continuidade da sessão, em markdown. SÓ leitura — não cria nada e não grava nada.

    to_thread não é detalhe: `montar` roda `git status`/`git diff` (subprocess) e parseia a cauda de
    um transcript que pode ter MB. Trabalho desses dentro da corrotina trava o loop e leva junto o
    SSE de TODAS as sessões — é o incidente de 2026-07-23, quando um `git status` no tick da lista
    derrubou a conexão inteira.
    """
    info = await _bastao_alvo(name, project, session_id, config_dir, provider,
                              origem_codex_account)
    texto = await asyncio.to_thread(bastao_montar, info.jsonl, info.cwd, info.provider, name,
                                    info.codex_home)
    return Response(content=texto, media_type="text/markdown; charset=utf-8")


@app.get("/api/sessions/{name}/bastao/dossie", dependencies=[Depends(require_auth)])
async def bastao_dossie(name: str):
    """O dossiê que ESTA sessão RECEBEU, lido do disco — não um novo.

    Irmão do GET acima e diferente dele de propósito: aquele MONTA o dossiê da sessão pedida agora,
    e serve pra prévia de quem vai passar o bastão. Este devolve o arquivo gravado na hora da
    passagem, que é o que a sucessora leu. Mostrar um dossiê remontado no lugar dele seria exibir
    um texto que ninguém leu como se fosse a instrução recebida.
    """
    alvo = bastao_mod.caminho(name)
    if not alvo.exists():
        raise HTTPException(404, detail=erro("erro_bastao_sem_dossie", "no handover dossier for this session"))
    texto = await asyncio.to_thread(alvo.read_text, encoding="utf-8")
    return Response(content=texto, media_type="text/markdown; charset=utf-8")


class BastaoBody(_StrictBody):
    """Sessão NOVA que vai continuar o trabalho de `{name}`.

    Corpo próprio, e não `CreateBody`: aquele é `extra="forbid"` e tem `cwd` obrigatório — aqui o
    padrão é o cwd da ORIGEM (a passagem continua o mesmo trabalho, na mesma árvore). Os campos
    que sobrepõem o `CreateBody` são repassados pra ele tal e qual, então a validação de
    provider/conta/motor/modelo/esforço/permissão continua num lugar só (`model_args` + o handler
    de criação) — `provider` aqui é sempre o da SUCESSORA, nunca o da origem.
    """
    name: str = Field(min_length=1)          # nome da sessão nova (o destino)
    cwd: str | None = None                   # None = o cwd da origem
    config_dir: str | None = None
    provider: str = "claude"
    remember_provider: bool = Field(default=False, strict=True)
    engine: str | None = None
    engine_account: str | None = None
    model: str | None = None
    effort: str | None = None
    permission_mode: str | None = None
    omp_profile: str | None = None
    codex_account: str | None = None
    # Modo de execução da SUCESSORA (Claude/Codex sem terminal). A sessão que recebe o trabalho é
    # nova e nasce onde a pessoa escolher — não herda o modo da origem.
    headless: bool | None = None
    # Pedir ao modelo que reescreva o resumo antes de gravar. Gasta cota DA ORIGEM e é por isso
    # que é escolha, não padrão; falhando, o resumo montado por código vai pro disco do mesmo jeito.
    resumo_por_modelo: bool = False
    # Endereçam a origem MORTA no archive (project + session_id); nunca a sucessora, e são
    # ignorados quando a origem está viva.
    project: str | None = None
    session_id: str | None = None
    origem_config_dir: str | None = None
    origem_codex_account: str | None = None
    origem_provider: str | None = None       # None = mesmo provider da sucessora (`provider`)


def _nome_ocupado(nome: str) -> bool:
    """Já existe sessão com esse nome? Mesmas duas fontes que `registry.create` consulta antes de
    levantar `ValueError` — tmux (Claude/Pi/Kimi) e o sidecar do Codex."""
    from app.adapters.codex import sessions as codex_sessions
    return tmux.has_session(nome) or codex_sessions.exists(nome) or headless_sessions.exists(nome)


def _bastao_preparar(info: SessionInfo, origem: str, destino: str,
                     por_modelo: bool = False) -> tuple[str, Path, str, str | None]:
    """Monta o resumo, GRAVA e devolve (texto, caminho, kick-off, aviso). Tudo sync, numa thread só.

    Gravar antes de criar a sessão é o que fecha o caso "sessão nova viva apontando pra um arquivo
    que não existe": se o disco recusar, a exceção sobe daqui e nada foi criado ainda.

    Com `por_modelo`, o texto de código passa pelo modelo ANTES de gravar — e só ele vai pro disco
    se der certo. A reescrita nunca levanta: falhando, grava o de código e devolve o aviso, porque
    uma continuação sem a camada interpretada é muito melhor que continuação nenhuma.
    """
    _passo(destino, "resumo")
    texto = bastao_montar(info.jsonl, info.cwd, info.provider, origem, info.codex_home)
    aviso = None
    if por_modelo:
        # A conta TEM de ser a da ORIGEM: é o trabalho dela que está sendo resumido e é a cota dela
        # que a pessoa aceitou gastar. O transcript do Claude mora em
        # `<config_dir>/projects/<projeto>/<uuid>.jsonl`, daí os três níveis.
        #
        # Não dando pra determinar a conta (origem Codex/Pi/Kimi, ou caminho fora do formato), a
        # reescrita NÃO acontece. Rodar sem o env usaria a conta Claude padrão da máquina: daria
        # certo, devolveria um resumo bonito e cobraria de quem não foi escolhido — pior que
        # recusar, porque ninguém ficaria sabendo.
        cfg = None
        if info.provider == "claude" and info.jsonl:
            p = Path(info.jsonl).parents
            if len(p) >= 3:
                cfg = str(p[2])
        if cfg is None:
            aviso = "só dá pra usar o modelo quando a sessão de origem é Claude nesta máquina"
        else:
            _passo(destino, "resumo_modelo")
            texto, aviso = bastao_mod.reescrever_com_modelo(texto, cfg)
    alvo = bastao_mod.gravar(destino, texto)
    conta, modelo = bastao_mod.origem_resumida(info.jsonl, info.provider, info.codex_home)
    return texto, alvo, bastao_mod.kickoff(origem, alvo, conta, modelo), aviso


@app.post("/api/sessions/{name}/bastao", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def bastao_passar(name: str, body: BastaoBody):
    if "provider" not in body.model_fields_set:
        provider = await _default_session_provider(body.config_dir, body.engine, body.codex_account, body.omp_profile)
        body = body.model_copy(update={"provider": provider})
    with _acompanhar_criacao(body.name):
        return await _passar_bastao(name, body)


async def _passar_bastao(name: str, body: BastaoBody):
    """Passa o bastão de `{name}` pra uma sessão nova: dossiê no disco + sessão criada + kick-off
    na fila durável dela.

    A ordem é a feature: **monta → grava → cria → enfileira**. Invertida, um erro de disco deixaria
    uma sessão viva com um kick-off mandando ler um arquivo inexistente.

    A entrega NÃO passa pelo caminho do `/input` (que digita PRIMEIRO e só depois grava na fila):
    numa sessão criada há milissegundos a TUI ainda está subindo e as teclas se perdem. Entra como
    `append(delivered=False)` — durável — e o drain entrega quando ela aceitar texto.

    `to_thread` no preparo pelo mesmo motivo do GET: `montar` roda `git status` (subprocess) e
    parseia a cauda do transcript; no loop isso derruba o SSE de todas as sessões (2026-07-23).
    """
    info = await _bastao_alvo(name, body.project, body.session_id, body.origem_config_dir,
                              body.origem_provider or body.provider, body.origem_codex_account)
    sucessora_codex_account = body.codex_account
    if sucessora_codex_account is not None and body.provider != "codex":
        raise HTTPException(400, detail=erro("codex_account_so_codex",
                                             "codex_account só vale para provider codex"))
    if body.provider == "codex":
        try:
            if sucessora_codex_account is not None:
                codex_accounts.resolve_account(sucessora_codex_account)
            if info.provider == "codex":
                origem = codex_accounts.account_for_rollout(Path(info.jsonl)) if info.jsonl else None
                if origem is None and info.codex_home:
                    origem = next((account for account in codex_accounts.list_accounts()
                                   if account.home.resolve(strict=False)
                                   == Path(info.codex_home).resolve(strict=False)), None)
                if origem is None:
                    raise codex_accounts.AccountError(
                        409, "codex_account_archive_mismatch", {})
                if origem is not None:
                    if sucessora_codex_account is None:
                        sucessora_codex_account = origem.id
                    elif sucessora_codex_account != origem.id:
                        raise codex_accounts.AccountError(
                            409, "codex_account_archive_mismatch",
                            {"account_id": sucessora_codex_account, "origin_account": origem.id},
                        )
        except codex_accounts.AccountError as e:
            raise _erro_conta_codex(e) from None
    # UM nome só, sanitizado pelo MESMO lugar que a criação usa (`registry.create` chama isto), e
    # daqui pra frente é ele quem nomeia o arquivo e a sessão. Sanitizar duas vezes por dois
    # caminhos diferentes era o bug: `api.v2` gravava `api.v2.md` mas nascia como `api-v2`, e aí o
    # `prune` não achava chave viva pro dossiê e APAGAVA o sidecar de uma sessão viva. De quebra,
    # a guarda do "bastão pra si mesma" passa a pegar `"cc "` contra `"cc"`.
    destino = sanitize_session_name(body.name)
    if not destino:
        raise HTTPException(400, detail=erro("erro_nome_invalido", "nome invalido"))
    if destino == name:
        raise HTTPException(400, detail=erro("erro_bastao_para_si_mesma",
                                             "a sessão não passa o bastão pra si mesma"))
    # Nome JÁ OCUPADO recusa aqui, ANTES de gravar. O `create_session` lá embaixo também recusa
    # (registry.create: `ja existe uma sessao com esse nome` -> 409), só que tarde demais: o dossiê
    # é `<destino>.md`, keyed por nome, então digitar o nome de uma sessão viva que já recebeu um
    # bastão SOBRESCREVIA o dossiê dela — e o kick-off dela aponta pra aquele caminho. Mesma dupla
    # de fontes do registry (tmux + sidecar do Codex), pra recusar o mesmo conjunto de nomes.
    # Sobra uma janela estreita: se ALGUÉM MAIS criar esse nome entre esta checagem e o create, o
    # dossiê já terá sido gravado por cima. O 409 do `create_session` impede duas sessões com o
    # mesmo nome — ele NÃO desfaz essa escrita. Fechar de vez exigiria criar a sessão antes de
    # gravar, que é a ordem que a feature proíbe (sessão viva apontando pra arquivo inexistente).
    if await asyncio.to_thread(_nome_ocupado, destino):
        raise HTTPException(409, detail=erro("erro_nome_em_uso", "ja existe uma sessao com esse nome"))
    cwd = body.cwd or info.cwd
    if not cwd:
        # A origem sem cwd conhecido é o caso do transcript resolvido sem pane utilizável: sem
        # diretório não há onde criar a sessão nova, e chutar um seria pior que recusar.
        raise HTTPException(400, detail=erro("erro_bastao_sem_cwd",
                                             "a sessão de origem não tem diretório conhecido; "
                                             "escolha o cwd da sessão nova"))
    # Pasta renomeada/movida com a origem morta (viva, o registry já devolve a atual): recusa antes
    # de gravar o dossiê.
    if not await asyncio.to_thread(os.path.isdir, os.path.expanduser(cwd)):
        raise HTTPException(400, detail=erro("erro_bastao_cwd_inexistente",
                                             f"a pasta {cwd} não existe mais; se ela foi renomeada "
                                             f"ou movida, escolha a pasta nova", cwd=cwd))
    account_models = None
    if body.engine_account is not None:
        if body.provider != "claude" or not body.engine:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", "conta ChatGPT exige Claude com motor CLIProxyAPI local"))
        account = await asyncio.to_thread(_fixed_engine_account, body.engine, body.engine_account)
        cfg = engines.listar()[body.engine]
        account_models = await _engine_models(body.engine, fresco=True)
        try:
            cliproxy.validate_models(cfg, body.model or cfg["model"], account, account_models)
        except ValueError as exc:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
    try:
        texto, alvo, kick, aviso_resumo = await asyncio.to_thread(
            _bastao_preparar, info, name, destino, body.resumo_por_modelo)
    except OSError as e:
        # Falha APARECE, e a sessão não nasce órfã: nada foi criado até aqui.
        _log.warning("bastao: não deu pra gravar o dossiê de %s -> %s: %s", name, destino, e)
        # `motivo` como PARAM, e não só embutido no `msg`: sem ele o front não tem como traduzir a
        # frase sem jogar fora o erro do sistema de arquivos, que é a única parte acionável dela.
        raise HTTPException(500, detail=erro("erro_bastao_gravar",
                                             f"não consegui gravar o dossiê: {e}",
                                             motivo=str(e))) from None
    # Reusa o handler de criação inteiro (validação de provider/conta/motor/model_args, ciclo da
    # conta, Codex): duplicar aquilo aqui seria uma segunda porta de criação pra manter em dia.
    # HTTPException dele sobe tal e qual — o dossiê já gravado vira sidecar órfão, que o `prune`
    # recolhe pelo nome (ver bastao_mod.caminho).
    creation = CreateBody(
        name=destino, cwd=cwd, config_dir=body.config_dir, provider=body.provider,
        remember_provider=body.remember_provider,
        engine=body.engine, engine_account=body.engine_account, model=body.model, effort=body.effort,
        permission_mode=body.permission_mode, omp_profile=body.omp_profile,
        codex_account=sucessora_codex_account,
        # `CreateBody.headless` é estrito: None (cliente antigo, que não manda o campo) tem de
        # virar False, e não chegar como None num campo que só aceita bool.
        headless=bool(body.headless))
    creation._engine_catalog = account_models
    novo = await create_session(creation)
    _passo(destino, "recado")
    try:
        await asyncio.to_thread(lambda: PromptQueue(novo.name).append(
            kick, delivered=False, pre_transcript=True))
    except OSError as e:
        # A sessão JÁ existe: o erro tem de dizer isso, senão o 500 nomeia a coisa errada e quem
        # lê acha que nada aconteceu — e vai criar outra. Aqui o conserto é humano (mandar o
        # kick-off na mão), então o caminho do dossiê vai junto.
        _log.error("bastao: sessão %s criada, mas o kick-off não entrou na fila: %s", novo.name, e)
        raise HTTPException(500, detail=erro(
            "erro_bastao_fila", f"a sessão {novo.name} nasceu, mas o kick-off não entrou na fila "
            f"dela ({e}) — mande você mesmo o pedido apontando pra {alvo}",
            nome=novo.name, dossie=str(alvo))) from None
    # Drena numa thread: `send_prompt` espera a TUI ficar interativa (`_wait_input_ready`), o que
    # pode levar segundos numa sessão recém-criada — segurar o request nisso não ajuda ninguém.
    # Vale só pro Claude na prática (Pi/Kimi nascem com `jsonl=None` e a thread sai calada); não há
    # perda, a fila é durável e o drain do próximo idle/SSE entrega.
    threading.Thread(target=_drain_session, args=(novo.name,), daemon=True).start()
    # `aviso`: a reescrita pelo modelo foi pedida e não deu (cota, tempo, CLI ausente). A sessão
    # nasceu e o resumo de código está lá — quem pediu precisa saber que recebeu o outro.
    return {"name": novo.name, "dossie": str(alvo), "texto": texto, "kickoff": kick,
            "aviso": aviso_resumo, "avisos": novo.avisos}


@app.get("/api/sessions/{name}/workflows", dependencies=[Depends(require_auth)])
async def workflows_list(name: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import list_workflows
    return await asyncio.to_thread(list_workflows, info.jsonl)


@app.get("/api/sessions/{name}/workflows/{run_id}", dependencies=[Depends(require_auth)])
async def workflow_detail(name: str, run_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import get_workflow
    wf = await asyncio.to_thread(get_workflow, info.jsonl, run_id)
    if wf is None:
        raise HTTPException(404, detail=erro("erro_workflow_inexistente", "workflow run not found"))
    return wf


@app.get("/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}", dependencies=[Depends(require_auth)])
async def workflow_agent_detail(name: str, run_id: str, agent_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import get_agent
    a = await asyncio.to_thread(get_agent, info.jsonl, run_id, agent_id)
    if a is None:
        raise HTTPException(404, detail=erro("erro_agente_inexistente", "agent not found"))
    return a


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
async def subagents_list(name: str):
    # Subagentes soltos (tool Agent). O transcript de cada um mora em <session-dir>/subagents/ —
    # e é a ÚNICA fonte do que ele está chamando enquanto roda; o jsonl do pai só tem o pedido.
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.subagents import list_subagents
    return await asyncio.to_thread(list_subagents, info.jsonl)


@app.get("/api/sessions/{name}/subagents/{agent_id}", dependencies=[Depends(require_auth)])
async def subagent_detail(name: str, agent_id: str, events: int = 0):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.subagents import get_subagent
    # events=N -> devolve tambem o transcript do subagente nos MESMOS ChatEvent do chat, pra a UI
    # reusar a lista de mensagens em vez de desenhar um formato proprio.
    a = await asyncio.to_thread(get_subagent, info.jsonl, agent_id, 40, events)
    if a is None:
        raise HTTPException(404, detail=erro("erro_subagente_inexistente", "subagent not found"))
    return a


@app.get("/api/sessions/events", dependencies=[Depends(require_auth)])
async def sessions_events(request: Request):
    from app.sse import list_events
    guest = guest_of(request)
    return EventSourceResponse(
        list_events(only=guest, viewer=guest_users.current.get(),
                    token=request.scope.get(GUEST_TOKEN_KEY)),
        send_timeout=30)


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str, request: Request):
    # handler async -> registry.list() (subprocess tmux) vai pro threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        # No diário COM o motivo: o 404 sozinho não separa sessão que sumiu da lista de sessão viva
        # sem transcript (Pi/Kimi nos primeiros segundos, ou tmux sem responder).
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
                      count_app=guest_of(request) is None and guest_users.current.get() is None),
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
        return permission_mode.session_non_plan_mode(jsonl or _jsonl_atual(n))
    # Rótulo de aviso do app ([painel: …]) não é sessão; resolvê-lo só gastaria chamadas ao tmux.
    modo_remetente = _modo(remetente) if sanitize_session_name(remetente) == remetente else None
    modo = modo_remetente or _modo(alvo, jsonl_alvo) or permission_mode.modo_da_conta(cfg_alvo)
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
    # Pi COM LINHA: cria a entrada da fila ANTES do 1o envio, pra ter um id ESTAVEL pra oferecer
    # como msg_id (achado ALTA da revisao 02/08/2026 — "Porta A"). A extensao chama sendUserMessage
    # ANTES de confirmar, entao a PRIMEIRA tentativa (esta aqui) e a que mais importa: sem id nela,
    # um retry do drain() (apos "deferred" por ACK perdido) nao tem como a extensao reconhecer como
    # a MESMA mensagem.
    #
    # SO com linha (achado da re-revisao 02/08/2026): pre-criar a entrada TAMBEM pra Pi sem linha
    # (fallback de teclado) abre uma janela de duplo envio que nao existia antes deste commit. Entre
    # este append() e o send_prompt() abaixo nao ha trava nenhuma — o _send_lock so e adquirido
    # DENTRO do send_prompt (terminal_input.py) — e o claim_undelivered do drain() usa so o
    # _append_lock da fila, que nao tem relacao com aquele. Um drain() concorrente (hook, /input
    # duplo, _maybe_chain) podia reivindicar essa entrada na janela e digitar o MESMO texto de novo
    # assim que o send_lock liberasse. E o msg_id nao ajuda em nada nesse caminho: quem digita no
    # tty nunca le esse valor (ver comentario em terminal_input.send_prompt). Claude/Codex-via-tty
    # e Pi-sem-linha ficam todos no fluxo de sempre (append DEPOIS do send, delivered ja resolvido)
    # — e ali NAO ha janela, porque a entrada so nasce depois que o unico send_prompt desta chamada
    # ja terminou.
    entry = None
    # ponytail: JANELA RESIDUAL CONHECIDA (nao fechada agora, registrada por decisao do usuario). A
    # decisao "vai por linha ou por tecla" e tomada DUAS vezes — aqui, FORA de qualquer trava, e de
    # novo dentro do _send_lock (terminal_input.py, perto de "provider == pi and pane_id and
    # INBOX.tem_linha"). Entre as duas nao ha trava compartilhada: claim_undelivered (pqueue.py) usa
    # so o _append_lock da fila, sem relacao com o _send_lock. Se a linha cair ENTRE esta leitura de
    # tem_linha() e a segunda checagem dentro do lock, quem perde a corrida pelo _send_lock ve
    # tem_linha=False, cai pro teclado — que NUNCA le msg_id (ver terminal_input.send_prompt). Sai
    # pela linha de um lado, e redigitado do outro. Nao e regressao deste commit: e o buraco
    # original encolhido de "qualquer sessao Pi" pra "sessao com linha viva no instante do append, e
    # a linha caiu bem nessa janela". Medido: entregar_sync segura o _send_lock por ate PRAZO_ACK+2.0
    # = 5s (pi_inbox.py) — janela de segundos, nao de microssegundos, tempo de sobra pra um drain de
    # reconexao de SSE ou de transicao de hook entrar.
    # CUIDADO no upgrade: so mover o append() pra dentro do _send_lock fecha a corrida entre as DUAS
    # LEITURAS de tem_linha() (o TOCTOU vira leitura unica) mas NAO fecha a duplicata. A entrada
    # nasce delivered=False aqui e so vira True quando o set_delivered(...) do fim desta funcao roda
    # DEPOIS que send_prompt() retorna — tambem fora de qualquer trava. Nesse intervalo (que inclui
    # a espera inteira pelo _send_lock MAIS os ate 5s do entregar_sync) a entrada continua
    # reivindicavel por claim_undelivered. Upgrade completo precisa das DUAS coisas juntas: o
    # append() E o set_delivered() final dentro da MESMA trava — ou claim_undelivered passar a
    # respeitar/disputar o _send_lock. Mover so o append() e necessario, mas sozinho e insuficiente.
    # `pi_inbox.linha_de` (nome primeiro, pane depois), nunca o pane cru: no psmux o pane e `%1`
    # em toda sessao Pi e a busca por pane achava a linha da OUTRA — ver pi_inbox.
    is_pi = (provider in ("pi", "omp") and pi_inbox.linha_de(name, pane_id) is not None
             and not stripped.startswith("/"))
    if is_pi:
        try:
            entry = PromptQueue(name).append(text, delivered=False, ts=t0)
        except OSError:
            # Fail-soft, mas NAO calado: sem log aqui, um disco ruim degrada pro uuid4-por-tentativa
            # de sempre (vulneravel a duplicata) exatamente na hora em que este conserto deveria
            # entrar em acao — achado da re-revisao 02/08/2026.
            _log.exception("fila indisponivel antes do envio (Pi com linha) name=%s", name)
            entry = None
    # Limpa a flag ANTES de chamar send_prompt (nao so depois de ler, mais abaixo): assim a AUSENCIA
    # dela depois so pode significar "esta chamada nao passou pelo _partial", em vez de herdar o
    # valor de uma chamada anterior na MESMA thread do pool. `_ULTIMA_LIMPEZA` e threading.local
    # (ver o comentario ao lado da declaracao em terminal_input.py) e o pool REUSA thread — sem isto,
    # um "partial" que um dia devolvesse sem passar por `_partial()` leria a sobra de outro envio.
    if hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"):
        del terminal_input._ULTIMA_LIMPEZA.limpou
    # Caminho NATIVO: sessão Claude com o function-hook ouvindo recebe por
    # `$.prompt.submit` e nenhuma tecla é emitida. Slash-command fica de fora — é
    # meta, e depende do menu que só existe na TUI. Ninguém ouvindo = pane, como sempre.
    # `deliverable` antes de tudo: o rascunho entra pela API do engine, mas o Enter é tecla, e
    # tecla em overlay navega o menu em vez de submeter. Mesmo gate do caminho de sempre.
    # Escada de entrega, um degrau só depois que o anterior não deu: socket nativo do Claude
    # (recado de sessão-irmã, entra no meio do turno) → plugin sem tecla → tmux → fila.
    nativo = _enviar_nativo(name, text) if provider == "claude" and not stripped.startswith("/") else None
    if nativo:
        _log.info("SEND name=%s pelo socket nativo msg_id=%s text=%r", name, nativo, text[:80])
    entrega_plugin = False
    if (not nativo and provider == "claude" and not stripped.startswith("/")
            and plugin_bridge.aguardando(name)
            and terminal_input.deliverable(name)):
        modo = plugin_bridge.choose_mode(name, text)
        entrega_plugin = plugin_bridge.entregar(name, text, modo,
                                                _jsonl_atual(name) if modo == "user" else None)
    # INCERTO conta como entregue para não digitar por cima; a reconciliação decide depois.
    pelo_plugin = entrega_plugin is True or entrega_plugin == plugin_bridge.INCERTO
    if pelo_plugin:
        _log.info("SEND name=%s pelo plugin (sem tecla) modo=%s resultado=%s text=%r",
                  name, modo, entrega_plugin, text[:80])
    try:
        result = "sent" if (nativo or pelo_plugin) else terminal.send_prompt(
            name, text, provider, pane_id=pane_id,
            **({"msg_id": entry["id"]} if entry is not None else {}))
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
        if entry is not None:
            # A entrada do Pi ja existe (criada acima, ANTES de saber o resultado) — sem isto ficaria
            # delivered=False pra sempre e o proximo drain reentraria digitando em cima do residuo.
            try:
                PromptQueue(name).set_delivered(entry["id"], True)
            except OSError:
                # Achado da re-revisao 02/08/2026: falhar calado aqui e o MESMO bug que o comentario
                # acima descreve (residuo redigitado por cima) voltando sem deixar rastro nenhum.
                _log.exception("fechar entrada apos 'partial' falhou name=%s", name)
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
            # ponytail: o sidecar do AskUserQuestion NAO e limpo aqui — /clear abre um transcript com
            # session_id novo, entao o sidecar antigo vira lixo inofensivo (nao reabre nada).
    elif entry is not None:
        # Pi com id estavel: a entrada JA existe (criada antes do send) — so atualiza o delivered,
        # nunca um segundo append (duplicaria a bubble na fila).
        try:
            PromptQueue(name).set_delivered(entry["id"], result == "sent")
        except OSError:
            _log.exception("atualizar fila apos envio falhou name=%s", name)
        if result == "sent":
            _agendar_confirmacao(name, _CONFIRM_GRACE + 0.5)
        else:
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
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
    """(provider, pane_id) numa leitura só — era `_pane_provider`, que pagava seu próprio
    `tmux list-panes -t <name>` (via `tmux.pane_pid`); agora usa `list_panes_all()` (MESMA chamada
    `list-panes -a` que o antigo `list_panes_active` já fazia — um fork só), e o `pane_id` sai de
    carona, sem tmux novo no caminho quente. Provider do pane (claude/pi) continua lido do /proc
    como antes: o gate de "TUI pronta" do terminal_input casa marcas do rodape do Claude, que o Pi
    nao imprime, e sem saber o provider todo envio a uma sessao Pi queimava os 12s de timeout
    antes de digitar.

    Task 6: resolve pelo pane do AGENTE (`SessionRegistry._agent_pane`, Task 5.5), nao mais pelo
    pane ATIVO — reusa a MESMA resolucao que `registry.list()` ja usa, nao uma terceira. Com um
    split (o shell escondido, ou qualquer split manual), o pane ativo podia ser o do shell: uma
    sessao Pi virava ("claude", pane_id do shell) neste caminho de ENVIO — o gate esperava as
    marcas de rodape do Claude e queimava os 12s por mensagem, e `INBOX.tem_linha(pane_id)`
    falhava (derrubava a linha rapida do Pi), porque o pane_id devolvido era do pane errado.

    Erro/pane sumido -> ("claude", None) — comportamento de hoje, marcas do Claude, sem pane_id
    (cai pra tecla, igual a antes desta task).

    Revisao final (I1): o corpo mudou de casa pro `agentpane.pane_info` — o drain da fila duravel e
    o adapter do Pi precisavam da MESMA resolucao e estavam no pane ativo. Esta funcao fica como o
    nome que as rotas (e os testes) ja conhecem."""
    from app import agentpane
    return agentpane.pane_info(name)


async def _send_one_codex(name: str, text: str, *, track_entry: bool = False) -> dict:
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
    # Enfileira sempre como pendente; so marca entregue apos a TUI REALMENTE receber o prompt.
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
    try:
        result = await adapter.send_prompt(name, text)
    except Exception as e:
        _log.exception("codex send_prompt falhou name=%s", name)
        return {"ok": False, "error": erro("erro_envio_falhou",
                                           f"falha ao enviar: {e}", erro=str(e))}
    if result == "sent":
        if entry is not None:
            # turno iniciou -> marca entregue pra o drain-on-complete nao reenviar a mesma entrada.
            try:
                await _send_thread(PromptQueue(name).set_delivered, entry["id"], True)
            except OSError:
                pass
    elif entry is None:
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


def _recusa_orq(name: str) -> None:
    """O orquestrador não tem pane nem processo: sem esta recusa, entrada, nome, fim e interrupção
    cairiam no tmux de uma sessão que não existe."""
    if orq_runs.find(name):
        raise HTTPException(409, detail=erro("erro_sessao_orq",
                                             "o orquestrador não recebe mensagens; fale com o árbitro"))


def _headless(name: str) -> bool:
    """Sessão Claude SEM terminal (sidecar do adapter headless). Provider continua "claude"; só o
    transporte muda — quem ramifica por isto é a entrada, o interrupt, a opção e a resposta."""
    return headless_sessions.exists(name)


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def input_prompt(name: str, body: InputBody):
    # Ramifica por provider: Claude via _send_thread (_send_one e SYNC/bloqueante — tmux), Codex via
    # _send_one_codex (async, app-server). Default "claude" pra qualquer nome nao-Codex.
    # NUCLEO SAGRADO: o envio NUNCA disputa executor com feature. _send_thread e um pool DEDICADO
    # (nao o default do asyncio, que a decoracao — git_summary/capture_pane — pode ocupar). Assim um
    # git status pendurado + refine (60s, pool do anyio) + check (600s, thread propria) nao seguram
    # o POST /input. Ver _send_thread.
    await _send_thread(_recusa_orq, name)
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
            elif provider != "codex" and not _headless(name):
                provider, _ = await _send_thread(_pane_info, name)
                q = PromptQueue(name)
                # Só Kimi: o steer dele injeta no turno em curso. O "send now" do Claude INTERROMPE o
                # turno (mata o comando rodando), então lá a promoção é só pelo botão, nunca colada
                # num recado.
                if provider == "kimi" and await _send_thread(q.entry_delivered, entry_id):
                    if await _send_thread(terminal_input.steer_now, name, provider) is True:
                        steered = True
                        await _send_thread(q.confirm_delivered)
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


@app.post("/api/sessions/{name}/steer", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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
    if provider not in ("kimi", "claude"):
        raise HTTPException(409, "só sessão Kimi ou Claude tem steer pela fila do terminal")
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
    # Promovido de verdade: baixa a fila duravel AGORA. O Kimi so grava o append_message da msg
    # steerada no FIM do turno (medido: 34s depois do ctrl-s), entao esperar o transcript confirmar
    # deixava o chip "N na fila" aceso o turno inteiro — e clicavel, sobre um no-op.
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


def _group_text(me: str, others: list[str], task: str, harness: dict[str, str]) -> str:
    # Par remoto (srv::sessao): o contrato não sincroniza cross-server, então a linha dele some.
    # contract_path_for devolve None em sidecar legado sem gid — str(None) viraria "None" no prompt.
    cross = any(peers.is_remote(o) for o in others)
    caminho = None if cross else contract_path_for(me)
    return pair_texto.texto_grupo(me, others, task, str(caminho) if caminho else None, harness)


async def _deliver(name: str, text: str) -> dict | None:
    # Mesma esteira do /input (fila durável se ocupada), ramificada por provider.
    # Devolve o envelope {code, params, msg} (ou string crua de erro tecnico ainda nao migrado)
    # ou None — _send_one/_send_one_codex NUNCA levantam, reportam no dict; engolir isso fazia o
    # pareamento dizer "ok" com o aviso jamais entregue.
    res = await _enviar(name, text)
    return None if res.get("ok") else (res.get("error")
                                           or erro("erro_envio_falhou_desconhecida", "falha desconhecida no envio"))


@app.post("/api/sessions/{name}/pair", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pair_session(name: str, body: PairBody):
    """Junta `name` e peer(s) num GRUPO de trabalho (une os grupos existentes de todos) e injeta
    em CADA membro o prompt do grupo atualizado — a partir daí trocam recados via hangar-send por
    iniciativa própria, dentro do escopo da tarefa. Badge `pair_peers` aparece na lista."""
    others = [p for p in dict.fromkeys(body.peers or ([body.peer] if body.peer else [])) if p]
    # Sem peer só o grupo de orquestração: o árbitro do `orquestrar-auto` precisa do gid antes do
    # `orq init`, e o time só chega depois, aberto pelo orquestrador.
    if not others and not body.orq:
        raise HTTPException(400, detail=erro("erro_peer_nao_informado", "informe peer ou peers"))
    if name in others:
        raise HTTPException(400, detail=erro("erro_autopareamento", "não dá pra parear uma sessão com ela mesma"))
    for n in (name, *others):
        await asyncio.to_thread(_recusa_orq, n)
    if any(peers.is_remote(o) for o in others):
        # Cross-server é 1:1 puro (um peer remoto, sem misturar grupo local) — grupo cross-server de
        # N fica pra fase 2. ponytail: 1:1 cobre "trabalhar junto entre máquinas"; N quando doer.
        if len(others) != 1:
            raise HTTPException(400, detail=erro("erro_pareamento_cross_1_1",
                                             "pareamento cross-server é 1:1 por enquanto: um peer remoto, "
                                             "sem misturar com grupo local"))
        if not settings.server_id:
            raise HTTPException(400, detail=erro("erro_pareamento_server_id_ausente",
                                             "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                             "pareamento cross-server (é o endereço de resposta srv::sessao)"))
        return await _pair_cross_server(name, others[0], body.task, body.replace_task)
    harness = {s.name: s.provider for s in await asyncio.to_thread(registry.list)}
    names = set(harness)
    missing = [p for p in [name, *others] if p not in names]
    if missing:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {', '.join(missing)}", detalhe=", ".join(missing)))
    # join_group: snapshot + join na MESMA seção crítica (em seções separadas, um join concorrente
    # na janela entre elas entrava no grupo fora do snapshot e um rollback posterior não o
    # reverteria). O snapshot volta pra cá pra desfazer se o aviso não chegar em ninguém.
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, others, body.task, substituir_task=body.replace_task, harness=harness, orq=body.orq)
    except pair.PairMixError as e:
        # Uma das sessões locais já está pareada cross-server (1:1) — não dá pra fundir em grupo local.
        raise HTTPException(400, detail=erro("erro_pareamento_mistura_cross", str(e)))
    except pair.TaskConflito as e:
        raise HTTPException(409, detail=erro("erro_pareamento_tarefa_existente",
                                             f"o grupo já tem tarefa: {e.existente!r} — repita com "
                                             f"--substituir-tarefa pra trocar", existente=e.existente))
    except (orq_context.PromotionConflict, orq_md.Conflito) as e:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou", str(e)))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", body.task)
    # Só quem estava SOLTO recebe o protocolo; veterano não é acordado (consulta o grupo quando
    # precisar). O protocolo pós-/clear é do hook.
    avisos = [(m, _group_text(m, [x for x in members if x != m], task, harness))
              for m in ([] if link.get("orq") else members) if snap.get(m) is None]
    errs = []
    for m, texto in avisos:
        e = await _deliver(m, texto)
        if e:
            errs.append({"sessao": m, "erro": e})
    if avisos and len(errs) == len(avisos):
        # NINGUÉM foi avisado -> grupo fantasma; restaura o estado anterior e reporta.
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, detail=erro("erro_pareamento_desfeito",
                            f"pareamento desfeito: falha ao avisar as sessões "
                            f"({'; '.join(f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs)})",
                            avisos=errs))
    return {"ok": True, "members": members, "gid": link.get("gid"),
            "warning": erro("erro_pareamento_aviso_parcial",
                            "aviso falhou em: " + "; ".join(
                                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
                            avisos=errs)
            if errs else None}


async def _pair_cross_server(name: str, peer: str, task: str, replace_task: bool) -> dict:
    """Pareamento 1:1 entre máquinas. Registra o vínculo LOCAL (name.json peers=[srv::sessao];
    sidecar do remoto vive na máquina dele) e chama o /pair-remote do backend peer pra registrar o
    reverso + injetar o protocolo lá. Falha na chamada remota desfaz o vínculo local (mesmo racional
    do 'grupo fantasma' do pair local). Transporte já provado pelo hangar-send cross-server (peers.json)."""
    harness = {s.name: s.provider for s in await asyncio.to_thread(registry.list)}
    if name not in harness:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {name}", detalhe=name))
    srv, sess = peers.split_addr(peer)
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [peer], task, substituir_task=replace_task, harness=harness)
    except pair.PairMixError as e:
        # `name` já está num grupo local (ou já pareada cross-server): não dá pra cross-parear.
        raise HTTPException(400, detail=erro("erro_pareamento_mistura_cross", str(e)))
    except pair.TaskConflito as e:
        raise HTTPException(409, detail=erro("erro_pareamento_tarefa_existente",
                                             f"o grupo já tem tarefa: {e.existente!r} — repita com "
                                             f"--substituir-tarefa pra trocar", existente=e.existente))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", task)
    initiator = f"{settings.server_id}::{name}"
    try:
        await asyncio.to_thread(
            peers.call, srv, "POST", f"/api/sessions/{sess}/pair-remote",
            {"initiator": initiator, "task": task})
    except peers.PeerError as e:
        await asyncio.to_thread(pair.restore, snap)
        if e.transport:
            # Rede caiu / resposta perdida: o /pair-remote PODE ter comitado no peer antes de a
            # resposta se perder. Desfiz este lado; tento limpar o outro por garantia (best-effort —
            # se o peer está mesmo inacessível isto também falha, e aí o usuário desapareia lá na mão).
            try:
                await asyncio.to_thread(peers.call, srv, "POST",
                                        f"/api/sessions/{sess}/unpair-remote", {"peer": initiator})
            except peers.PeerError:
                pass
            raise HTTPException(502, detail=erro("erro_pareamento_nao_confirmado",
                                             f"pareamento NÃO confirmado (falha de rede com '{srv}'): desfeito "
                                             f"deste lado; se o peer tiver ficado pareado, rode unpair lá. ({e})",
                                             srv=srv, erro=str(e)))
        raise HTTPException(502, detail=erro("erro_pareamento_rejeitado", f"pareamento desfeito (peer rejeitou): {e}", erro=str(e)))
    # Reverso registrado. Injeta o protocolo NESTE lado; se este falhar (sessão morreu na janela), o
    # vínculo já vale dos dois lados — só avisa, não desfaz (o par remoto já sabe).
    warn = None
    e = await _deliver(name, _group_text(name, [peer], task, harness))
    if e:
        warn = erro("erro_pareamento_aviso_local",
                    f"vínculo criado, mas o aviso local falhou ({name}: {_erro_texto(e)}) — refaça o pair se precisar",
                    sessao=name, erro=e)
    return {"ok": True, "members": members, "warning": warn}


class PairRemoteBody(_StrictBody):
    initiator: str        # 'srv::nome' — quem iniciou o pareamento, na máquina remota
    task: str = ""


@app.post("/api/sessions/{name}/pair-remote", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pair_remote(name: str, body: PairRemoteBody):
    """Lado RECEPTOR do pareamento cross-server: registra `name` (sessão LOCAL) pareada ao iniciador
    remoto `body.initiator` (srv::nome) e injeta o protocolo. NÃO chama de volta (o iniciador já
    registrou o próprio lado — chamar de volta recursaria). Chamado só pelo backend do outro server
    via peers.call, autenticado pelo token do peers.json."""
    if not peers.is_remote(body.initiator):
        raise HTTPException(400, detail=erro("erro_initiator_invalido", "initiator precisa ser qualificado (srv::nome)"))
    await asyncio.to_thread(_recusa_orq, name)
    harness = {s.name: s.provider for s in await asyncio.to_thread(registry.list)}
    if name not in harness:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {name}", detalhe=name))
    try:
        # substituir_task=True: a task que chega aqui é a combinada do iniciador, sempre vence.
        members, snap = await asyncio.to_thread(pair.join_group, name, [body.initiator], body.task, substituir_task=True, harness=harness)
    except pair.PairMixError as e:
        # `name` já está num grupo local aqui — não pode virar par cross-server de outra máquina.
        raise HTTPException(409, detail=erro("erro_pareamento_mistura_cross", str(e)))
    e = await _deliver(name, _group_text(name, [body.initiator], body.task, harness))
    if e:
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, detail=erro("erro_pareamento_aviso_falhou",
                                    f"pareamento desfeito: falha ao avisar '{name}': {_erro_texto(e)}",
                                    nome=name, erro=e))
    return {"ok": True, "members": members}


class UnpairRemoteBody(_StrictBody):
    peer: str             # 'srv::nome' que saiu do pareamento, na máquina remota


@app.post("/api/sessions/{name}/unpair-remote", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def unpair_remote(name: str, body: UnpairRemoteBody):
    """`name` (local) tinha um par remoto que saiu — remove o vínculo local e avisa. Idempotente
    (sair de quem não está pareado é no-op). Chamado pelo backend peer no unpair do outro lado."""
    # Defesa: só dissolve se `name` está MESMO pareado com quem diz estar saindo. Sem isto, um
    # /unpair-remote perdido, duplicado ou com peer errado dissolvia um pareamento legítimo de `name`
    # e mandava aviso falso (era o vetor de dano cross-máquina do achado crítico do review).
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    if not link or body.peer not in (link.get("peers") or []):
        return {"ok": True, "warning": None, "noop": f"'{name}' não está pareado com '{body.peer}'"}
    ex = await asyncio.to_thread(pair.leave, name)
    warn = None
    if ex:
        e = await _deliver(name, f"{pair_texto.PREFIXO} '{body.peer}' saiu do pareamento. "
                                 "Volte a operar independente; use hangar-send só quando o usuário pedir.")
        if e:
            warn = erro("erro_pareamento_aviso_unpair", f"{name}: {_erro_texto(e)}",
                        sessao=name, erro=e)
    return {"ok": True, "warning": warn}


# Anti-tempestade: o prompt manda nunca responder [grupo:] com --group, mas prompt é disciplina,
# não trava. 5 avisos/min por grupo cobre "terminei" + "contrato atualizado" de N membros; um loop
# N×N passa disso em segundos. ponytail: dict em memória, zera no restart — é o que basta.
_GROUP_MAX_NA_JANELA = 5
_GROUP_JANELA_S = 60
_group_envios: dict[str, list[float]] = {}


def _group_estourou(gid: str, agora: float) -> bool:
    ts = [t for t in _group_envios.get(gid, []) if agora - t < _GROUP_JANELA_S]
    ts.append(agora)
    _group_envios[gid] = ts
    return len(ts) > _GROUP_MAX_NA_JANELA


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
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, detail=erro("erro_group_message_slash", "group-message não suporta slash-commands"))
    txt = body.text.lstrip()
    if txt.startswith("[grupo:") or txt.startswith("[de:"):
        raise HTTPException(400, detail=erro("erro_group_message_resposta",
                                             "aviso de grupo não pode reencaminhar um [grupo:]/[de:] — responda 1:1"))
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    membros = link.get("peers") if link else None
    if not membros:
        raise HTTPException(404, detail=erro("erro_sessao_sem_grupo", "sessão não está num grupo"))
    if _group_estourou(link.get("gid") or name, time.time()):
        raise HTTPException(429, detail=erro("erro_group_message_tempestade",
                                             f"mais de {_GROUP_MAX_NA_JANELA} avisos de grupo em "
                                             f"{_GROUP_JANELA_S}s — parece loop; espere ou responda 1:1",
                                             max=_GROUP_MAX_NA_JANELA, janela=_GROUP_JANELA_S))
    # `pulados` fica vazio de propósito: o backend entrega a cada membro pela escada do _send_one
    # (socket nativo primeiro); nunca devolve o trabalho pro modelo fazer por SendMessage.
    pulados: list[str] = []
    text = f"[grupo: {name}] {body.text}"
    results: dict[str, dict] = {}
    for p in membros:
        if not await _send_thread(_session_exists, p):
            results[p] = {"ok": False, "error": erro("erro_sessao_inexistente", "sessão não encontrada"), "delivered": False}
            continue
        results[p] = await _enviar(p, text)
    failed = [{"sessao": n, "erro": r.get("error")} for n, r in results.items() if not r.get("ok")]
    return {"ok": True, "peers": membros, "pulados": pulados,
            "warning": erro("erro_pareamento_grupo_falha",
                            "falha em: " + "; ".join(
                                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in failed),
                            avisos=failed)
            if failed else None}


# ----------------------------------------------------------------- orquestração (política + papéis)

def _catalogo_claude_cache(dir_conta: Path) -> tuple[list[dict], bool]:
    """Leitor do cache do picker pro inventário — o MESMO cache de /api/model-options."""
    cacheado = _models_cache_get(_chave_config(dir_conta))
    if cacheado is not None:
        return list(cacheado.get("models") or []), False
    return orq_politica._modelos_claude_reduzidos(dir_conta)


def _inventario() -> list[orq_politica.ContaInventario]:
    return orq_politica.inventario(_catalogo_claude_cache)


class PoliticaContaBody(_StrictBody):
    provider: str
    apelido: str = ""
    modelos: list[str] = ["*"]
    trocar: bool = True
    ligada: bool = True
    mtime: float


@app.get("/api/orquestracao/politica", dependencies=[Depends(require_auth)])
async def orq_politica_get():
    texto, mtime = orq_md.ler_arquivo(orq_politica.caminho())
    inv = await asyncio.to_thread(_inventario)
    return {"arquivo": str(orq_politica.caminho()), "mtime": mtime,
            "politica": [asdict(c) for c in orq_politica.ler(texto)],
            "inventario": [asdict(i) for i in inv]}


@app.put("/api/orquestracao/politica/{conta}", dependencies=[Depends(require_auth)])
async def orq_politica_put(conta: str, body: PoliticaContaBody):
    inv = await asyncio.to_thread(_inventario)
    item = next((i for i in inv if i.provider == body.provider
                 and orq_md.normalizar(i.conta) == orq_md.normalizar(conta)), None)
    if item is None:
        raise HTTPException(400, detail=erro("erro_orq_conta_desconhecida",
                                             f"conta {conta!r} ({body.provider}) não existe nesta máquina"))
    modelos = tuple(m.strip() for m in body.modelos if m.strip()) or ("*",)
    if "*" not in modelos and not item.reduced and item.modelos:
        conhecidos = {m["id"] for m in item.modelos}
        ruim = [m for m in modelos if m not in conhecidos]
        if ruim:
            raise HTTPException(400, detail=erro("erro_orq_modelo_desconhecido",
                                                 f"modelo(s) fora do catálogo da conta: {', '.join(ruim)}",
                                                 modelos=ruim))
    try:
        for v in (conta, body.apelido, *modelos):
            orq_md.validar_celula(v)
        if body.ligada:
            c = orq_politica.ContaPolitica(item.conta, body.provider, body.apelido, modelos, body.trocar)
            mtime = await asyncio.to_thread(orq_politica.gravar_conta, c, body.mtime)
        else:
            mtime = await asyncio.to_thread(orq_politica.desligar, item.conta, body.mtime)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", str(e)))
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o arquivo mudou desde a leitura — recarregue"))
    return {"ok": True, "mtime": mtime}


class PapelBody(_StrictBody):
    papel: str
    sessao: str = ""
    provider: str
    conta: str
    modelo: str = ""
    esforco: str = ""
    headless: bool | None = None
    permissao: str = ""
    motor: str = ""
    jev: bool = False
    subagente: str = ""
    perfil: str = ""
    mtime: float


def _context_de(name: str) -> orq_context.Context:
    try:
        return orq_context.resolve(name)
    except orq_context.IdentityUnavailable as e:
        raise HTTPException(409, detail=erro("erro_orq_celula_invalida", str(e)))
    except (OSError, ValueError) as e:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou", str(e)))


def _papeis_de(gid: str) -> tuple[str, float, list[orq_papeis.Papel]]:
    texto, mtime = orq_md.ler_arquivo(orq_papeis.regras_path(gid))
    return texto, mtime, orq_papeis.ler(texto)


def _orq_identity(name: str) -> str | None:
    try:
        return orq_context.identity(name)
    except orq_context.IdentityUnavailable:
        return None


@app.get("/api/sessions/{name}/orq", dependencies=[Depends(require_auth)])
async def orq_get(name: str):
    context = await asyncio.to_thread(_context_de, name)
    _texto, mtime, papeis = await asyncio.to_thread(_papeis_de, context.gid)
    # A lista fresca do registry (sem git nem pane): `casar_viva` só precisa de nome + last_activity.
    infos = await asyncio.to_thread(registry.list)
    arbitro = next((p for p in papeis if p.e_arbitro()), None)
    cwd = next((s.cwd for s in infos if s.name == name), None)
    pronto = await asyncio.to_thread(orq_start.readiness, cwd, context.grouped, bool(papeis))
    return {
        "gid": context.gid, "grouped": context.grouped, "session_prefix": context.session_prefix,
        "session_identity": await asyncio.to_thread(_orq_identity, name),
        "arquivo": str(context.path), "mtime": mtime, "prontidao": pronto,
        "arbitro": orq_papeis.casar_viva(arbitro, infos) if arbitro else None,
        "papeis": [{**asdict(p), "viva": orq_papeis.casar_viva(p, infos),
                    "id_cota": orq_politica.id_cota(p.provider, p.conta)} for p in papeis],
    }


@app.get("/api/sessions/{name}/orq/panel", dependencies=[Depends(require_auth)])
async def orq_panel(name: str):
    """Painel da sessão do orquestrador sem LLM: um retrato por execução, lido dos arquivos dela."""
    # A pasta vem da linha em cache da lista: `runs.find` releria todas as execuções a cada pedido.
    info = await asyncio.to_thread(_cached_info_sync, name)
    if info is None or info.provider != "orq" or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "execucao nao encontrada"))
    return await asyncio.to_thread(orq_timeline.panel, Path(info.jsonl).parent, _guardar_snap)


class PapelItem(_StrictBody):
    papel: str
    sessao: str = ""
    provider: str
    conta: str
    modelo: str = ""
    esforco: str = ""
    # Vazio = o papel roda numa conta só (formato original). "1", "2", "3"… = rodízio, e a Task N
    # cabe à conta de índice (N-1) % total. "par" = todas ao mesmo tempo.
    vez: str = ""
    # Abertura da sessão do papel: as mesmas escolhas da criação de sessão, gravadas como flags.
    headless: bool | None = None
    permissao: str = ""
    motor: str = ""
    jev: bool = False
    subagente: str = ""
    perfil: str = ""
    # Teto de contexto do papel, em % da janela da sessão ("" = 50%). O vigia lê daqui.
    # None = cliente que não conhece o campo: mantém o valor gravado em vez de apagá-lo.
    janela: str | None = None


async def _validar_abertura(p: orq_papeis.Papel) -> None:
    """Mesmas regras da criação de sessão: o árbitro não pode receber uma linha que o
    `hangar-send --new` recusaria."""
    def recusa(codigo: str, msg: str):
        raise HTTPException(400, detail=erro(codigo, f"{msg}: {p.papel}"))
    if p.headless and p.provider not in ("claude", "codex"):
        recusa("erro_orq_headless_provider", "sem terminal só vale para claude ou codex")
    if p.headless and "--read-only" in (p.abertura_extra or ""):
        recusa("erro_orq_headless_read_only",
               "sem terminal não aceita --read-only (o backend recusa a sessão): desligue o sem terminal")
    if p.motor:
        if p.provider != "claude":
            recusa("erro_motor_sem_claude", "motor so vale para provider claude")
        if p.motor not in await asyncio.to_thread(engines.listar):
            recusa("erro_motor_invalido", "motor invalido")
    if p.permissao:
        if p.provider == "codex" and p.headless:
            from app.adapters.codex import sem_terminal
            if p.permissao not in {m[0] for m in sem_terminal.MODOS}:
                recusa("erro_permissao_invalida", "modo de permissao invalido")
        elif p.provider != "claude":
            recusa("erro_permissao_so_claude", "modo de permissao so vale para claude")
        else:
            try:
                model_args.validar("claude", None, None, p.permissao)
            except ValueError:
                recusa("erro_permissao_invalida", "modo de permissao invalido")
    if p.subagente:
        if p.provider != "claude" or p.motor:
            recusa("erro_subagente_so_claude", "modelo dos subagentes so vale para claude sem motor")
        try:
            model_args.validar("claude", p.subagente, None)
        except ValueError as e:
            recusa("erro_orq_celula_invalida", str(e))
    if p.perfil:
        if p.provider != "omp":
            recusa("erro_perfil_so_omp", "perfil so vale para provider omp")
        # A mesma regra de nome do omp que a criação de sessão usa.
        from app.omp_plugin_sync import InventoryError, resolve_omp_directories
        try:
            resolve_omp_directories(Path.home(), {"OMP_PROFILE": p.perfil}, Path.home())
        except InventoryError as e:
            recusa("erro_orq_celula_invalida", str(e))


class PapeisBody(_StrictBody):
    papeis: list[PapelItem]
    mtime: float
    # Sem efeito: salvar nunca acorda o árbitro. Fica porque o corpo é estrito e clientes antigos o mandam.
    avisar: bool = True


async def _aplicar_papeis(name: str, itens: list[PapelItem], mtime_lido: float) -> dict:
    """Grava TODAS as linhas numa escrita só: o usuário edita vários papéis e salva no fim, e
    salvar um por vez descartava o resto sem aviso."""
    if not itens:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", "nenhum papel"))
    context = await asyncio.to_thread(_context_de, name)
    texto, _mtime, papeis = await asyncio.to_thread(_papeis_de, context.gid)
    novos: list[orq_papeis.Papel] = []
    try:
        for it in itens:
            # Herda a sessão da linha de MESMO papel e MESMA vez: num papel que reveza, cada conta
            # tem a sua, e casar só pelo papel copiaria a sessão da primeira pras demais.
            #
            # Consequência a saber ao escrever um chamador novo: converter um papel de conta única
            # (`vez` vazia) em rodízio (`vez` = "1") não casa linha nenhuma, então a sessão NÃO é
            # herdada — quem faz essa conversão tem de mandar `sessao` explícito, como o painel faz
            # em `adicionarConta`. Omitir ali perderia a sessão viva do papel, calado.
            vez = it.vez.strip()
            atual = next((p for p in papeis
                          if orq_md.normalizar(p.papel) == orq_md.normalizar(it.papel)
                          and orq_md.normalizar(p.vez) == orq_md.normalizar(vez)), None)
            novo = orq_papeis.Papel(it.papel.strip(), (it.sessao or (atual.sessao if atual else "")).strip(),
                                    it.provider.strip().lower(), it.conta.strip(),
                                    it.modelo.strip(), it.esforco.strip(), vez,
                                    it.headless, it.permissao.strip(), it.motor.strip(), it.jev,
                                    it.subagente.strip(), perfil=it.perfil.strip(),
                                    abertura_extra=atual.abertura_extra if atual else "",
                                    janela=(atual.janela if atual else "") if it.janela is None
                                    else it.janela.strip().rstrip("%").strip())
            motivo = await asyncio.to_thread(orq_politica.permitido, novo.provider, novo.conta, novo.modelo, novo.esforco)
            if motivo:
                raise HTTPException(400, detail=erro(motivo, "a política de contas não permite esta escolha: " + novo.papel))
            await _validar_abertura(novo)
            # ponytail: validar_celula roda dentro de escrever_papel — texto do cliente nunca chega
            # ao arquivo sem passar por ali.
            texto = orq_papeis.escrever_papel(texto, novo)
            novos.append(novo)
        mtime = await asyncio.to_thread(orq_context.write, context, texto, mtime_lido)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", str(e)))
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o contrato mudou desde a leitura — recarregue"))
    # A linha vale na próxima sessão de cada papel: o árbitro lê a tabela ao abrir. Nada que está
    # rodando é fechado nem trocado por causa de um salvar.
    return {"papeis": [asdict(p) for p in novos], "papel": asdict(novos[0]), "mtime": mtime,
            "arbitro": None, "aviso": "proxima_sessao", "erro": None}


@app.post("/api/sessions/{name}/orq/papel", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def orq_papel_set(name: str, body: PapelBody):
    return await _aplicar_papeis(name, [PapelItem(**body.model_dump(exclude={"mtime"}))], body.mtime)


@app.post("/api/sessions/{name}/orq/papeis", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def orq_papeis_set(name: str, body: PapeisBody):
    return await _aplicar_papeis(name, body.papeis, body.mtime)


class OrqGroupBody(_StrictBody):
    gid: str
    mtime: float


@app.post("/api/sessions/{name}/orq/grupo", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def orq_group_set(name: str, body: OrqGroupBody):
    """Associa o time da planejadora ao grupo real, inclusive com um árbitro novo."""
    try:
        context = await asyncio.to_thread(orq_context.associate, name, body.gid, body.mtime)
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o time mudou desde a leitura — recarregue"))
    except ValueError as e:
        raise HTTPException(409, detail=erro("erro_orq_celula_invalida", str(e)))
    except OSError as e:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou", str(e)))
    return {"ok": True, "gid": context.gid, "grouped": context.grouped,
            "session_prefix": context.session_prefix, "arquivo": str(context.path),
            "mtime": (await asyncio.to_thread(orq_md.ler_arquivo, context.path))[1]}


class ComecarBody(_StrictBody):
    mtime: float = 0.0


@app.post("/api/sessions/{name}/orq/comecar", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def orq_comecar(name: str, body: ComecarBody):
    """Põe ESTA sessão pra tocar a orquestração como árbitra. Quem planejou vira árbitro (é o que a
    skill manda: a sessão da fase 1 assume a fase 2), então o alvo é a própria sessão, não uma nova
    — uma sessão nova começaria relendo tudo que esta já sabe.

    Recusa sem plano: o árbitro despacha Tasks e as Tasks vêm do plano. Um botão que acorda alguém
    sem ter o que despachar é pior que botão nenhum."""
    info = next((s for s in await asyncio.to_thread(registry.list) if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    gid, pronto = await asyncio.to_thread(_prontidao, name, info.cwd)
    res = await _enviar(name, orq_start.kickoff(pronto, str(orq_papeis.regras_path(gid))))
    if not res["ok"]:
        raise HTTPException(409, detail=erro("erro_orq_comecar_falhou",
                                             f"não deu pra avisar a sessão: {_erro_texto(res['error'])}",
                                             erro=res["error"]))
    plano = pronto["plan"]
    return {"ok": True, "entregue": bool(res.get("delivered")), "fase": pronto["phase"],
            "plano": Path(plano["path"]).name if plano else ""}


def _prontidao(name: str, cwd: str | None) -> tuple[str, dict]:
    context = _context_de(name)
    _texto, _mt, papeis = _papeis_de(context.gid)
    return context.gid, orq_start.readiness(cwd, context.grouped, bool(papeis))


class RemoverPapelBody(_StrictBody):
    papel: str
    vez: str = ""
    mtime: float


@app.delete("/api/sessions/{name}/orq/papel", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def orq_papel_del(name: str, body: RemoverPapelBody):
    """Tira UMA linha da tabela: um papel inteiro (sem `vez`) ou uma conta do rodízio dele. NÃO
    avisa o árbitro — quem mexe na fila normalmente mexe em várias linhas seguidas, e o aviso sai
    uma vez no fim, pelo botão. A sessão viva daquele papel não é tocada: o contrato diz quem
    DEVE rodar, não mata quem está rodando."""
    context = await asyncio.to_thread(_context_de, name)
    texto, _mtime, papeis = await asyncio.to_thread(_papeis_de, context.gid)
    alvo = next((p for p in papeis
                 if orq_md.normalizar(p.papel) == orq_md.normalizar(body.papel)
                 and orq_md.normalizar(p.vez) == orq_md.normalizar(body.vez)), None)
    if alvo is None:
        raise HTTPException(404, detail=erro("erro_orq_papel_inexistente",
                                             f"não há linha para {body.papel!r} nesta configuração"))
    cab = orq_papeis.cabecalho_atual(texto) or orq_papeis.CABECALHO
    texto = orq_md.remover_linha(texto, cab, orq_papeis.chave_da_linha(cab, alvo.papel, alvo.vez))
    try:
        mtime = await asyncio.to_thread(orq_context.write, context, texto, body.mtime)
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o contrato mudou desde a leitura — recarregue"))
    return {"papeis": [asdict(p) for p in orq_papeis.ler(texto)], "mtime": mtime}


@app.get("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
def pair_contract(name: str):
    """Contrato compartilhado do GRUPO (markdown que os membros editam via fs; keyed pelo gid —
    estável quando membro entra/sai). 404 sem grupo; content vazio se ainda não existe."""
    p = contract_path_for(name)
    if p is None:
        raise HTTPException(404, detail=erro("erro_sessao_nao_pareada", "sessão não está pareada"))
    link = PairLink(name).get() or {}
    try:
        content = p.read_text(encoding="utf-8")
    except OSError:
        content = ""
    return {"peers": link.get("peers", []), "path": str(p), "content": content}


async def _avisar_saida(name: str, expeers: list[str]) -> list[dict]:
    """Depois de `name` sair do grupo (o sidecar dele já foi limpo), desfaz o vínculo nos pares
    REMOTOS via /unpair-remote, senão o sidecar de lá fica órfão. Uma esteira só pra unpair e kill."""
    errs: list[dict] = []
    for p in expeers:
        if not peers.is_remote(p):
            continue
        # O sidecar de `name` prova que o par é dele: a busca é pela sessão, não só pelo endereço.
        rec = next((r for r in external_pairs.by_local(name) if r.address == p), None)
        if rec is not None:
            if external_pairs.ambiguous(rec.alias):
                # Só o aviso ao outro lado é pulado (o alias também é máquina tua); a limpeza local vale.
                errs.append({"sessao": p, "erro": erro(
                    "erro_par_endereco_ambiguo",
                    f"'{rec.alias}' é ao mesmo tempo máquina tua e par externo", peer=p)})
            else:
                try:
                    await asyncio.to_thread(external_pairs.call, rec.peer_address, rec.peer_token,
                                            "DELETE", "/api/pair")
                except (peers.PeerError, ValueError) as ex:
                    if getattr(ex, "status", None) != 410:
                        # Texto do outro lado vai rotulado: a tela não deve tomá-lo por mensagem do app.
                        texto = (external_pair_api._REMOTE_LABEL if getattr(ex, "status", None) else "") + str(ex)[:300]
                        errs.append({"sessao": p, "erro": erro("erro_peer_nao_avisado", texto, peer=p)})
            await external_pair_api._guarded_async("remover o registro", external_pairs.remove, rec.share_id)
            await external_pair_api._guarded_async("revogar o convite", share_store.revoke, rec.share_id)
            continue
        if not settings.server_id:
            errs.append({"sessao": p,
                         "erro": erro("erro_pareamento_server_id_ausente",
                                      "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                      "pareamento cross-server (é o endereço de resposta srv::sessao)")})
            continue
        srv, sess = peers.split_addr(p)
        try:
            await asyncio.to_thread(peers.call, srv, "POST",
                                    f"/api/sessions/{sess}/unpair-remote",
                                    {"peer": f"{settings.server_id}::{name}"})
        except peers.PeerError as ex:
            # Sidecar remoto fica órfão até alguém desparear lá. ponytail: sem fila de retry — single-user.
            _log.warning("saida do grupo: peer remoto '%s' não avisado (sidecar de lá fica órfão): %s", p, ex)
            errs.append({"sessao": p, "erro": erro("erro_peer_nao_avisado", str(ex), peer=p)})
    # Locais não são avisados: recado para quem saiu volta "sessão não encontrada".
    return errs


class GroupTaskSuggestionBody(_StrictBody):
    sessions: list[str]


def _conversa_para_resumo(name: str, info: SessionInfo) -> str:
    from app.pqueue import merged_history
    msgs = [ev for ev in merged_history(name, info.jsonl, info.provider, 30)
            if ev.kind in ("user_msg", "assistant_msg") and (ev.text or "").strip()]
    if not msgs:
        return ""
    quem = lambda ev: "usuário" if ev.kind == "user_msg" else "assistente"
    anteriores = "\n".join(f"{quem(ev)}: {ev.text.strip()[:500]}" for ev in msgs[:-1])[-3000:]
    # É na última mensagem que o agente costuma deixar o próximo passo: ela vai inteira e marcada.
    ultima = f"ÚLTIMA MENSAGEM ({quem(msgs[-1])}): {msgs[-1].text.strip()[:2000]}"
    return f"{anteriores}\n{ultima}" if anteriores else ultima


@app.post("/api/pair/task-suggestion", dependencies=[Depends(require_auth)])
async def group_task_suggestion(body: GroupTaskSuggestionBody):
    """Sugere a tarefa do grupo pelo fim da conversa de cada sessão. Sessão sem conversa (ou de
    outro servidor) fica de fora; nenhuma com conversa → 422, sem chamar o LLM."""
    conversas: dict[str, str] = {}
    for nome in dict.fromkeys(body.sessions):
        info = await _cached_info(nome)
        if not info or not info.jsonl:
            continue
        texto = await asyncio.to_thread(_conversa_para_resumo, nome, info)
        if texto:
            conversas[nome] = texto
    if not conversas:
        raise HTTPException(422, detail=erro("erro_grupo_sem_conversa",
                                             "nenhuma das sessões tem conversa para resumir"))
    try:
        tarefa = await asyncio.to_thread(narrar.sugerir_tarefa_grupo, conversas)
    except narrar.NarrarError as e:
        raise HTTPException(e.status, e.detail)
    return {"task": tarefa}


@app.delete("/api/sessions/{name}/pair", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def unpair_session(name: str):
    """`name` SAI do grupo (os demais membros continuam entre si; grupo restante de 1 dissolve).
    Avisa quem saiu e quem ficou. Idempotente. Aviso que falhar NÃO refaz o vínculo (fora do grupo
    é o estado desejado) — só reporta no result."""
    await asyncio.to_thread(_recusa_orq, name)
    expeers = await asyncio.to_thread(pair.leave, name)   # nome próprio: 'peers' é o módulo importado
    if not expeers:
        return {"ok": True, "warning": None}
    errs = await _avisar_saida(name, expeers)
    e = await _deliver(name, f"{pair_texto.PREFIXO} Você saiu do grupo de trabalho "
                             f"({', '.join(expeers)}). Volte a operar independente; use hangar-send só "
                             "quando o usuário pedir.")
    if e:
        errs.append({"sessao": name, "erro": e})
    return {"ok": True, "warning": erro("erro_pareamento_saida_falhou",
            "aviso de saída falhou: " + "; ".join(
                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
            avisos=errs)
            if errs else None}


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


# Prazo pro `tool.result` do picker do Kimi aparecer no wire depois do Submit.
_RESULT_KIMI_TIMEOUT = 5.0


def _espera_resposta_kimi(jsonl: str | None, call_id: str,
                          timeout: float = _RESULT_KIMI_TIMEOUT) -> bool:
    """True quando o `tool.result` daquele toolCallId chega no wire. A escrita nao e instantanea —
    sem a espera, a checagem rodaria antes do Kimi gravar e todo drive bem-sucedido cairia no
    fallback por texto, entregando a resposta DUAS vezes (uma pela ferramenta, outra como msg)."""
    if not jsonl:
        return False
    from app.adapters.kimi.transcript import resposta_chegou
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        if resposta_chegou(jsonl, call_id):
            return True
        time.sleep(0.2)
    return False


# Prazo pra escolha no painel de aprovacao do Kimi aterrissar (mesmo criterio do picker).
_APROV_KIMI_TIMEOUT = 5.0


def _espera_escolha_kimi(name: str, jsonl: str, req_id: str, pede_feedback: bool,
                         timeout: float = _APROV_KIMI_TIMEOUT) -> bool:
    """True quando a escolha no painel de aprovacao do Kimi esta comprovadamente entregue.

    Duas provas, porque as escolhas do painel terminam de dois jeitos diferentes: a comum vira
    `interaction.resolved` no wire, e a que pede justificativa (`Revise`, `Reject with feedback`)
    NAO resolve nada na hora — o painel troca o rodape por um campo de texto e espera a pessoa
    escrever. Sem a segunda prova, escolher `Revise` gastaria o prazo inteiro e voltaria erro numa
    tecla que pegou."""
    from app.adapters.kimi.transcript import interacao_resolvida
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        if interacao_resolvida(jsonl, req_id):
            return True
        if pede_feedback and terminal_input.feedback_kimi_aberto(name):
            return True
        time.sleep(0.2)
    return False


def _select_aprovacao_kimi(name: str, info, option: int) -> dict:
    """Escolhe no painel de APROVACAO do Kimi (plano/comando/arquivo).

    As opcoes vem do WIRE (`read_pending_interaction`), nao do pane — e a mesma fonte que o estado
    usou pra desenhar os botoes, entao o numero que chega aqui casa com o que a pessoa leu.

    Sem pedido pendente, 409 — NUNCA cair no `terminal.select` generico. Ele conta a linha do cursor
    e, quando nao acha (`_cursor_row` so le `❯`, e o Kimi desenha `▶`), manda Down x(n-1) + Enter as
    CEGAS. Numa sessao Kimi isso nao tem alvo: ou o painel ja fechou e as teclas caem na conversa em
    execucao, ou ele esta aberto e o Enter confirma a linha errada — nos dois casos a rota devolveria
    {"ok": true}, que e o sucesso falso que este projeto proibe. E nao ha o que perder: `_menu_block`
    exige cursor `❯`/`>`, entao o pane do Kimi nunca produziu opcao por raspagem — este endpoint so
    e alcancavel, nesse provider, pelos botoes que o wire desenhou."""
    from app.adapters.kimi.transcript import read_pending_interaction
    jsonl = info.jsonl if info else None
    pend = read_pending_interaction(jsonl) if jsonl else None
    if pend is None:
        # Cobre "ja foi respondida no terminal" e "nao deu pra ler o wire agora" — pro usuario a
        # saida e a mesma (nada foi enviado, olhe a sessao). O caso ilegivel nao some calado: o
        # `_objetos_da_cauda` loga uma vez por arquivo.
        raise HTTPException(409, detail=erro(
            "erro_sem_pergunta_kimi",
            "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
    escolhas = pend["escolhas"]
    if not 1 <= option <= len(escolhas):
        raise HTTPException(409, detail=erro(
            "erro_opcao_fora_da_lista",
            f"opção {option} não existe neste pedido (são {len(escolhas)}) — opção NÃO enviada"))
    try:
        terminal_input.select_kimi(name, option, jsonl, pend["id"])
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        # O wire dizia pendente e o painel ja saiu da tela: respondido no terminal entre o toque e
        # aqui. Mesmo caso (e mesma frase) do picker do Kimi no /answer.
        diag.registrar("aprovacao_kimi.painel_fechado", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro(
            "erro_sem_pergunta_kimi",
            "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
    if not _espera_escolha_kimi(name, jsonl, pend["id"], escolhas[option - 1]["requires_feedback"]):
        # Prazo estourado NAO prova que nada pegou — pode ser o Kimi demorando pra gravar. Igual ao
        # /answer do picker: so se o painel CONTINUA na tela e que a tecla comprovadamente nao pegou.
        if terminal_input.aprovacao_kimi_aberta(name):
            raise HTTPException(409, detail=erro(
                "erro_opcao_nao_convergiu", "não consegui marcar essa opção no terminal — tente de novo",
                detalhe="o painel de aprovação continua aberto e nada foi resolvido no wire"))
        raise HTTPException(409, detail=erro(
            "erro_sem_confirmacao_resposta",
            "resposta enviada, mas nao deu pra confirmar a tempo — "
            "confira na sessao antes de responder de novo"))
    return {"ok": True, "feedback_pendente": escolhas[option - 1]["requires_feedback"]}


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


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def select(name: str, body: SelectBody):
    from app.runtime_terminal import TerminalOutcomeUnknown, route_sync
    info = _cached_info_sync(name)
    # A rota do terminal só conhece o vínculo Claude: para outro provedor (Codex sem terminal
    # incluído) ela suspendia a escrita antes de chegar ao ramo dele.
    if getattr(info, "provider", "claude") == "claude":
        pending = plugin_bridge.pergunta_pendente(name)
        payload = {"option":body.option}
        if pending is not None:
            payload["request_id"] = pending["id"]
            if str(pending["id"]).startswith("perm:") and body.option not in (1, 2):
                raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "opção fora do pedido de permissão"))
        if pending is None or not str(pending["id"]).startswith("perm:"):
            # Pergunta `ask:` pode acabar no teclado da TUI: aí vale a trava do painel e o cursor tem de ser lido.
            _recusa_se_painel_aberto(name)
            if pending is not None:
                payload["require_cursor"] = True
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
    # Pedido de permissão que o plugin segura: o card do app é o de DUAS opções montado em
    # state.py (1 = Yes, 2 = No), e a resposta entra sem tecla. Não cai na tecla se o plugin não
    # pegar: segurado, o terminal não tem menu nenhum para dirigir.
    pend = plugin_bridge.pergunta_pendente(name)
    if pend is not None and str(pend["id"]).startswith("perm:"):
        if body.option not in (1, 2):
            raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "opção fora do pedido de permissão"))
        if plugin_bridge.responder_pergunta(name, {"permitir": body.option == 1}, pend["id"]):
            _log.info("SELECT name=%s permissão pelo plugin (sem tecla) opcao=%d", name, body.option)
            return {"ok": True}
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu",
                                             "o pedido de permissão não está mais aberto — confira a sessão"))
    _recusa_se_painel_aberto(name)
    if not _session_exists(name):
        raise HTTPException(404, detail=erro("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada"))
    # Kimi: os botoes de aprovacao (plano/comando/arquivo) sao desenhados a partir do WIRE, entao a
    # escolha volta pelo wire tambem — tecla numerica + `interaction.resolved` como prova. O drive
    # generico abaixo NAO atende este provider em hipotese nenhuma (ver _select_aprovacao_kimi).
    if getattr(info, "provider", "claude") == "kimi":
        return _select_aprovacao_kimi(name, info, body.option)
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


@app.post("/api/sessions/{name}/select/submit", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


_MOD_CONVIDADO = erro("erro_mod_convidado",
    "Só o dono da sessão aciona os mods dela pelo app; quem acompanha como convidado vê, mas não clica.")


def _convidado(request: Request) -> bool:
    from app import guest_users
    return guest_of(request) is not None or guest_users.current.get() is not None


def _recusa_convidado_no_terminal_do_rust(name: str, request: Request) -> None:
    """Convidado (com login ou de convite) não clica em mod de sessão cujo terminal é do Rust.

    O pane é do executor do Rust: o `plugin_click` daqui o dirigiria por fora dele. O Rust repassa ao
    Python todo pedido que não é do dono, e o convite chega pela porta 8766 sem passar pelo Rust, por
    isso a recusa mora aqui, depois da autenticação. Esta é só a recusa rápida, por uma fotografia da
    posse; a que vale é a do empréstimo do teclado (`runtime_terminal._borrow_keyboard`), sob a
    barreira da sessão, que alcança também a sessão aberta no Rust pelo próprio clique.
    """
    from app import runtime_coordinator
    if not _convidado(request):
        return
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.terminal_in_rust(name):
        raise HTTPException(403, detail=_MOD_CONVIDADO)


@app.post("/api/sessions/{name}/plugin/press", dependencies=[Depends(require_auth),
    Depends(_recusa_convidado_no_terminal_do_rust), Depends(_transfer_guard)])
async def plugin_press(name: str, body: PluginPressBody, request: Request):
    """Clique num botão que um mod desenhou na faixa ou num painel, pedido pelo app."""
    from app import plugin_click
    # O app de antes da rota `close` fechava o painel pelo `press` com a `key` reservada.
    if body.plugin is None and body.key == plugin_click.CLOSE_KEY:
        return await _acao_de_mod(request, plugin_click.close(name, body.site))
    return await _acao_de_mod(request, plugin_click.press(name, body.site, body.key, body.plugin))


@app.post("/api/sessions/{name}/plugin/close", dependencies=[Depends(require_auth),
    Depends(_recusa_convidado_no_terminal_do_rust), Depends(_transfer_guard)])
async def plugin_close(name: str, body: PluginCloseBody, request: Request):
    """Fecha um painel de mod pelo `✕` do cabeçalho, pedido pelo app."""
    from app import plugin_click
    return await _acao_de_mod(request, plugin_click.close(name, body.site))


async def _acao_de_mod(request: Request, acao):
    """Roda o clique pela tela com a marca de convidado e traduz as recusas para o app."""
    from app import plugin_click
    from app.runtime_terminal import GuestRefused, guest_admin
    marca = guest_admin.set(_convidado(request))
    try:
        return await acao
    except plugin_click.PressRefused as e:
        raise HTTPException(409, detail=e.detail)
    except GuestRefused:
        raise HTTPException(403, detail=_MOD_CONVIDADO) from None
    finally:
        guest_admin.reset(marca)


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def interrupt(name: str, clear: bool = False):
    await asyncio.to_thread(_recusa_orq, name)
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
    pergunta = (plugin_bridge.pergunta_pendente(name) or {}).get("id")
    try:
        await asyncio.to_thread(terminal.interrupt, name, clear=clear)
    except (TerminalControlError, TransferInProgress):
        raise
    except RuntimeError as e:
        raise _falha_do_runtime(e) from None
    plugin_bridge.interrompeu(name, pergunta)
    return {"ok": True}


def _exige_claude_de_terminal(name: str) -> None:
    # O /btw é da TUI do Claude Code: Codex, Pi, omp e Kimi não têm o comando nem o overlay.
    # (Claude SEM terminal tem caminho próprio — o fork da conversa — e não passa por aqui.)
    provider = "codex" if _provider_of(name) == "codex" else _pane_info(name)[0]
    if provider != "claude":
        raise HTTPException(400, detail=erro("erro_btw_so_claude", "pergunta lateral só existe em sessão Claude"))


@app.post("/api/sessions/{name}/btw", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pergunta_lateral(name: str, body: BtwBody):
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    # Sem terminal não há overlay pra dirigir: a pergunta vira um fork descartável da conversa.
    sem_terminal = await _send_thread(_headless, name)
    if not sem_terminal:
        await _send_thread(_exige_claude_de_terminal, name)
        await asyncio.to_thread(_recusa_se_painel_aberto, name)
    try:
        perguntar = btw.perguntar_sem_terminal if sem_terminal else btw.perguntar
        item = await asyncio.to_thread(perguntar, name, body.question)
    except btw.BtwError as e:
        raise HTTPException(e.status, detail=erro(e.code, e.detail))
    # A TUI já respondeu e gastou a chamada: falha ao guardar o histórico não pode virar 500 e
    # levar o cliente a perguntar de novo. Vai marcada, não escondida.
    try:
        await asyncio.to_thread(btw.registrar, name, item)
        item["salvo"] = True
    except OSError:
        _log.exception("btw de %s: resposta entregue, historico nao gravado", name)
        item["salvo"] = False
    return item


@app.get("/api/sessions/{name}/btw", dependencies=[Depends(require_auth)])
async def historico_lateral(name: str):
    return await asyncio.to_thread(btw.historico, name)


_TOOL_USE_ID = re.compile(r"^toolu_[A-Za-z0-9_-]{1,80}$")


def _etapas_da_ferramenta(tool_use_id: str) -> list[dict]:
    # O Claude sem terminal não repassa o progresso do MCP; o MCP grava as etapas aqui por conta própria.
    arquivo = Path.home() / ".hangar" / "tool-progress" / f"{tool_use_id}.jsonl"
    etapas = []
    try:
        linhas = arquivo.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return etapas
    for linha in linhas[-200:]:
        try:
            item = json.loads(linha)
        except ValueError:
            continue
        if isinstance(item, dict) and isinstance(item.get("message"), str):
            etapas.append({"t": item.get("t"), "message": item["message"][:500]})
    return etapas


class BashOutputBody(_StrictBody):
    command: str = Field(max_length=200_000)


# POST porque o comando vai inteiro no corpo: numa URL ele estoura o limite com heredoc.
@app.post("/api/sessions/{name}/bash-output", dependencies=[Depends(require_auth), Depends(_transfer_check)])
async def bash_output(name: str, body: BashOutputBody):
    return {"text": await asyncio.to_thread(procinfo.saida_de_comando, body.command)}


@app.get("/api/sessions/{name}/tool-progress/{tool_use_id}", dependencies=[Depends(require_auth)])
async def tool_progress(name: str, tool_use_id: str):
    if not _TOOL_USE_ID.match(tool_use_id):
        raise HTTPException(400, detail="invalid tool_use_id")
    return await asyncio.to_thread(_etapas_da_ferramenta, tool_use_id)


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


@app.get("/api/sessions/{name}/limits", dependencies=[Depends(require_auth)])
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


@app.websocket("/api/sessions/{name}/codex/voice")
async def codex_voice_socket(ws: WebSocket, name: str):
    from app.codex_voice import voice_ws
    await voice_ws(ws, name, get_adapter("codex"), _provider_of)


@app.get("/api/sessions/{name}/codex/voices", dependencies=[Depends(require_auth)])
async def codex_voice_options(name: str):
    from app.codex_voice import VOICES
    if runtime_config.get("codex_voice_beta") is not True:
        raise HTTPException(404, detail=erro("erro_recurso_desligado", "Recurso não habilitado."))
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_models_so_codex", "Somente sessões Codex."))
    return {"voices": VOICES}


class CodexModelBody(_StrictBody):
    model: str
    effort: str | None = None


@app.get("/api/sessions/{name}/models", dependencies=[Depends(require_auth)])
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


@app.post("/api/sessions/{name}/model", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/service-tier", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def set_codex_service_tier(name: str, body: CodexServiceTierBody):
    if _provider_of(name) == "claude":
        info = await _cached_info(name)
        model, _ = await asyncio.to_thread(_engine_fast_selection, name)
        if not info or not await asyncio.to_thread(cliproxy.supports_fast, info.engine, model):
            raise HTTPException(400, detail=erro("erro_fast_indisponivel", "Fast exige GPT no CLIProxyAPI local"))
        _recusa_se_painel_aberto(name)
        operation = asyncio.create_task(_durante_troca(name, _trocar_conta(name, None, service_tier=body.service_tier)))
        try:
            return await asyncio.shield(operation)
        except asyncio.CancelledError:
            await operation
            raise
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


_tarefas_soltas: set[asyncio.Task] = set()


async def _aquecer_codex_sem_terminal(name: str) -> None:
    try:
        await get_adapter("codex").ensure_running(name)
    except Exception:
        # Aqui só o log: no modo python o watch_sessions tenta de novo (até o teto de subidas); com o
        # Rust dono, o próximo envio abre a sessão nele.
        _log.warning("codex sem terminal: aquecimento na criação falhou name=%s", name, exc_info=True)
    finally:
        _tarefas_soltas.discard(asyncio.current_task())


@app.get("/api/sessions/{name}/codex-permissions", dependencies=[Depends(require_auth)])
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


@app.post("/api/sessions/{name}/codex-permissions", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/codex/mode", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/codex/plan/implement", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/keys", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/term-input", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


def _painel_disponivel() -> bool:
    # O termsock NAO importa `pty` no topo justamente pra este import funcionar no Windows.
    from app import termsock
    return termsock.painel_disponivel()


def _traducao_pensamento_disponivel() -> bool:
    from app import narrar
    if not runtime_config.get("traduzir_pensamento"):
        return False
    try:
        return bool(narrar._provedor()[1])
    except Exception:  # noqa: BLE001 — capacidade: config estranha vale como "sem provedor"
        return False


def _origem_do_terminal_ok(request: Request) -> bool:
    """A mesma pergunta que o handshake do terminal faz, respondida por HTTP (que tem corpo).

    Sem `Origin` (cliente que nao e navegador) e True: o handshake tambem so cobra origem quando o
    cabecalho existe, e responder False aqui poria um aviso de recusa numa tela que abre normal.
    """
    from app import termsock
    origem = request.headers.get("origin")
    if not origem:
        return True
    return termsock._origem_aceita(origem, request.headers.get("host"))


# ─── Atualizar ─────────────────────────────────────────────────────────────────────────────────

_ALVOS_AUSENTES_AVISADOS: set[str] = set()


def _mudancas_pendentes() -> list[dict] | None:
    """Os commits que entraram em `origin/<alvo>` e ainda não estão aqui — o changelog da tela.

    Título de commit, e não um `CHANGELOG.md` mantido à mão: as mensagens deste repo já são
    descritivas, e um arquivo à parte seria uma segunda cópia pra envelhecer. Passo que merecer
    texto próprio ganha um arquivo em `docs/atualizacoes/`, cujo corpo entra junto.

    `None` quando o git não compara (ref do alvo ainda não buscada): a tela oferece o Atualizar, e
    é o motor que diz o motivo. Lista vazia ali seria "Tudo em dia" sem sinal nenhum.
    """
    destino = atualizar.alvo()
    p = atualizar._git("log", "--format=%h%x00%s", f"HEAD..origin/{destino}", timeout=30)
    if p.returncode != 0:
        # Uma vez por alvo enquanto falhar: o polling da tela passa aqui a cada 2s.
        if destino not in _ALVOS_AUSENTES_AVISADOS:
            _ALVOS_AUSENTES_AVISADOS.add(destino)
            diag.registrar("atualizacao.alvo_ausente", "aviso",
                           detalhe=f"origin/{destino} rc={p.returncode}: {atualizar._cauda(p, 3)}")
        return None
    _ALVOS_AUSENTES_AVISADOS.discard(destino)
    linhas = []
    for linha in p.stdout.splitlines():
        sha, _, titulo = linha.partition("\x00")
        if titulo:
            linhas.append({"sha": sha, "titulo": titulo})
    return linhas


@app.get("/api/atualizacao", dependencies=[Depends(require_auth)])
async def get_atualizacao(procurar: bool = False):
    """Estado da atualização: as três versões, o que há de novo, e o que o motor está fazendo.

    As TRÊS versões porque "a versão instalada" tem três respostas — o disco, o processo vivo e o
    bundle que o navegador carregou — e elas divergem justamente na janela em que alguém está
    atualizando. Comparar só o disco com `origin/main` diria "tudo em dia" pra quem está olhando
    uma tela de dias atrás.

    Tudo em `to_thread`: são chamadas de `git` (subprocess), e o precedente de 23/07 é que elas
    nunca podem rodar na corrotina.
    """
    def _ler() -> dict:
        # `procurar=1` vai à REDE antes de comparar. Sem isto, o botão "Procurar de novo" só relia
        # o `origin/main` que já estava no disco — a foto do último fetch do laço, que roda a cada
        # 30min — e respondia "Tudo em dia" com informação velha, afirmando ter procurado. Não é
        # o padrão porque o polling da tela bate aqui a cada 2s durante uma atualização, e um
        # `git fetch` nessa cadência é rede à toa.
        if procurar:
            atualizar._git("fetch", "origin", timeout=120)
        pre = atualizar.checar()
        mudancas = _mudancas_pendentes()
        # Checkout fora da branch do alvo (campo esvaziado na branch de teste, ou branch já contida
        # na main) não tem commit a puxar, mas tem troca a fazer.
        troca = atualizar._troca_de_branch(pre) and not pre.get("branch_de_trabalho")
        disponivel = mudancas is None or bool(mudancas) or troca
        mudancas = mudancas or []
        return {
            "versoes": {"repo": diag._git_describe(), "backend": diag.VERSAO_EM_EXECUCAO},
            # A versão que a pessoa lê: data do commit + hash. `remoto` é o que está em
            # origin/<alvo> desde o último fetch; `atras` é quantos commits faltam.
            "versao_legivel": {"repo": diag.versao_legivel(),
                               "backend": diag.VERSAO_LEGIVEL_EM_EXECUCAO,
                               "remoto": diag.versao_legivel(f"origin/{atualizar.alvo()}")},
            "atras": len(mudancas),
            "atualizacao_disponivel": disponivel,
            "mudancas": mudancas,
            "passos": [{"id": s["id"], "titulo": s["titulo"], "texto": s["texto"]}
                       for s in atualizacoes.pendentes()],
            "pre_voo": pre,
            # `estado_para_tela`, não `estado`: converte "rodando" com o processo morto na falha
            # que ele não conseguiu escrever. Sem isso a tela fica presa numa atualização que já
            # não existe, e só sai editando o JSON na mão.
            "estado": atualizar.estado_para_tela(),
        }
    return await asyncio.to_thread(_ler)


@app.post("/api/atualizacao/iniciar", dependencies=[Depends(require_auth)])
async def post_atualizacao_iniciar():
    """Lança a atualização e devolve na hora — ela roda FORA deste processo, que vai reiniciar."""
    pre = await asyncio.to_thread(atualizar.checar)
    # Recusa ANTES de lançar o motor: a atualização alinha o disco com `origin/<alvo>` e arrastaria a
    # branch de trabalho junto (medido em 25/08/2026 numa máquina com `mobile-expo` no checkout).
    if pre.get("branch_de_trabalho"):
        raise HTTPException(409, detail=erro(
            "erro_atualizacao_branch",
            f"este checkout esta na branch {pre.get('branch')}, nao na {pre.get('alvo') or 'main'}",
            branch=pre.get("branch"), alvo=pre.get("alvo") or "main"))
    if not pre.get("pode"):
        faltando = pre.get("faltando") or []
        raise HTTPException(409, detail=erro(
            "erro_atualizacao_dependencia", f"falta o que a atualizacao precisa: {', '.join(faltando)}",
            faltando=faltando))
    r = await asyncio.to_thread(atualizar.iniciar, settings.port)
    if not r.get("ok"):
        raise HTTPException(409, detail=erro("erro_atualizacao_ja_rodando",
                                             "ja existe uma atualizacao rodando"))
    return r


@app.post("/api/atualizacao/reiniciar", dependencies=[Depends(require_auth)])
async def post_atualizacao_reiniciar():
    """Reinicia o servidor sem atualizar nada — o caso do disco já estar à frente do processo."""
    r = await asyncio.to_thread(atualizar.reiniciar_agora, settings.port)
    if r.get("erro") == "ja_rodando":
        raise HTTPException(409, detail=erro("erro_atualizacao_ja_rodando", "ja existe uma atualizacao rodando"))
    if not r.get("ok"):
        raise HTTPException(409, detail=erro(
            "erro_reinicio_indisponivel",
            f"esta maquina nao reinicia sozinha (topologia {r.get('topologia')})",
            topologia=r.get("topologia")))
    return r


async def _fetch_loop():
    """`git fetch` de tempos em tempos, senão `origin/main` é a foto do último pull de alguém.

    Sem isto o botão simplesmente nunca apareceria numa máquina que ninguém puxa à mão. Fail-soft
    e em `to_thread`: máquina sem rede não pode virar erro na tela nem derrubar o laço.
    """
    while True:
        try:
            await asyncio.to_thread(atualizar._git, "fetch", "origin", timeout=120)
        except Exception:                            # noqa: BLE001 — sem rede é o caso comum
            _log.debug("fetch periodico falhou", exc_info=True)
        await asyncio.sleep(1800)


# ─── Auto-update ────────────────────────────────────────────────────────────────────────────────
# O botão pede um clique; a correção de bug não pode esperar o clique em cada máquina. Este laço
# checa a cada hora e dispara o MESMO motor do botão (`atualizar.iniciar`), sem ninguém tocar em
# nada. Os gates vivem em `_auto_update_motivo`: qualquer um deles recusando, o tick vira um log e
# a próxima hora tenta de novo.
_AUTO_UPDATE_INTERVALO = 3600
_AUTO_UPDATE_FALHA_JANELA_S = 86400   # uma atualização que falhou segura novas tentativas por 24h
_DIST_SHA_URL = "https://github.com/jeffer1312/hangar/releases/download/dist-latest/frontend-dist.sha"


def _auto_update_motivo() -> Optional[str]:
    """Por que o auto-update NÃO deve disparar neste tick. None = dispara.

    Diferenças pro botão, de propósito: árvore suja ou commits locais adiante BLOQUEIAM aqui (o
    botão resguarda e pergunta; o automático não pode decidir sobre trabalho de ninguém), e sem o
    dist do CI deste commit exato NÃO cai no build local — espera o próximo tick (sha publicado =
    CI verde + build pronto, que é condição, não aceleração). Sobrava uma corrida de segundos — push entre o gate e o fetch do motor, ou release `dist-latest` móvel —
    em que o build local de fallback podia ainda acontecer: consequencia e lentidao, nao tela errada, entao ficou aceita e registrada aqui.
    """
    # O CI só publica o dist da main: branch de teste atualiza pelo botão, que compila a tela aqui.
    if atualizar.alvo() != "main":
        return "branch de teste configurada (CP_UPDATE_BRANCH)"
    pre = atualizar.checar()
    if not pre.get("pode"):
        return "dependencias faltando"
    if pre.get("branch_de_trabalho"):
        return f"checkout na branch {pre.get('branch')}"
    # Trocar de branch é decisão de quem aperta o botão, nunca do laço.
    if atualizar._troca_de_branch(pre):
        return f"checkout na branch {pre.get('branch')}, fora do alvo {pre.get('alvo')}"
    # divergiu ANTES de ahead: divergiu = ahead>0 AND behind>0, e o motivo mais preciso e o dela.
    if pre.get("divergiu"):
        return "checkout divergiu de origin/main"
    if pre.get("ahead"):
        return "checkout adiante de origin/main (commits locais nao pushados)"
    if pre.get("ahead_incerto"):
        return "nao deu pra contar os commits locais"
    if not pre.get("behind"):
        return "em dia"
    if pre.get("sujo"):
        return "arvore suja (trabalho nao commitado)"
    # estado_para_tela, NAO estado: o cru congela em "rodando" se o motor morrer sem gravar o
    # desfecho (kill, queda de energia), e cada tick devolveria "ja rodando" pra sempre — o auto-update
    # morria em silencio ate alguem abrir a tela. A conversao de dono-morto ja existe aqui.
    est = atualizar.estado_para_tela()
    if est.get("fase") == "rodando":
        return "atualizacao ja rodando"
    if est.get("ok") is False:
        try:
            idade = (datetime.now().astimezone() - datetime.fromisoformat(est.get("ts"))).total_seconds()
        except (TypeError, ValueError):
            # ts corrompido: abre, mas com log — senao a maquina re-tenta a cada hora com zero linha de log
            # explicando por que a janela de repeticao nao segurou.
            _log.warning("auto-update: ts invalido no estado da atualizacao (%r)", est.get("ts"))
            idade = _AUTO_UPDATE_FALHA_JANELA_S
        if idade < _AUTO_UPDATE_FALHA_JANELA_S:
            return "ultima atualizacao falhou"
    # Como o dist: o automático espera o topo inteiro publicado; parar antes dele é só no botão.
    ate = atualizar.pinned_target("main")[0]
    if ate is None:
        return "nao deu pra conferir o binario do Rust publicado"
    if ate != "origin/main":
        return "binario do Rust do topo ainda nao publicado para este sistema"
    try:
        with urllib.request.urlopen(_DIST_SHA_URL, timeout=15) as r:
            sha_dist = r.read().decode().strip()
    except Exception:                                  # noqa: BLE001 — qualquer falha aqui = dist ilegivel
        return "sem acesso ao dist do CI"
    p = atualizar._git("rev-parse", "origin/main", timeout=30)
    sha_alvo = p.stdout.strip()
    if p.returncode != 0 or not sha_alvo:
        return "rev-parse origin/main falhou"
    if sha_dist != sha_alvo:
        return "dist do CI ainda nao e deste commit"
    return None



async def _auto_update_loop():
    """Checa a cada hora e dispara a atualização quando TODOS os gates abrem. Fail-soft inteiro:
    qualquer exceção vira log e o próximo tick, nunca derruba o laço nem o backend."""
    await asyncio.sleep(_AUTO_UPDATE_INTERVALO)   # nunca na subida: o boot já é o ponto de ruído
    while True:
        try:
            if automations_enabled():
                # rc checado, NAO so o lance de excecao: fetch falho por rc (rede fora, auth quebrada) nao lanca nada — a origin/main fica velha, o gate reporta "em dia" e a máquina NUNCA se atualiza com zero log. rc checado, loga warning com a cauda: "sem rede" deixa de parecer "sem novidade".
                p = await asyncio.to_thread(atualizar._git, "fetch", "origin", timeout=120)
                if p.returncode != 0:
                    _log.warning("auto-update: fetch falhou (rc=%s): %s", p.returncode, atualizar._cauda(p))
                motivo = await asyncio.to_thread(_auto_update_motivo)
                if motivo is None:
                    # Sessão trabalhando é gate async (classify captura os panes), não cabe no helper.
                    infos = await registry.list_with_state()
                    if any(i.state == "working" for i in infos):
                        motivo = "sessao trabalhando"
                if motivo is None:
                    _log.info("auto-update: disparando atualizacao")
                    r = await asyncio.to_thread(atualizar.iniciar, settings.port, expected_branch="main")
                    if not r.get("ok"):
                        _log.warning("auto-update: iniciar recusou (%s)", r.get("erro"))
                elif motivo != "em dia":
                    _log.info("auto-update: pulando (%s)", motivo)
        except Exception:                            # noqa: BLE001 — sem rede/sem tmux é comum
            _log.exception("auto-update: tick falhou")
        await asyncio.sleep(_AUTO_UPDATE_INTERVALO)


@app.get("/api/config", dependencies=[Depends(require_auth)])
def get_config(request: Request):
    """Config editavel pelo app + o que e so-leitura (exige reiniciar o servico).

    O bloco so-leitura (`_somente_leitura`) volta tambem no POST: parte dele depende de campo
    editavel. Segredo NUNCA volta inteiro: `estado()` devolve mascarado (gsk_••••1234) — da pra
    conferir QUAL chave esta la sem conseguir copia-la de volta."""
    return {
        "campos": runtime_config.estado(),
        "somente_leitura": _somente_leitura(request),
        # IRMÃ do `somente_leitura`, nunca dentro dele: aquele bloco é um mapa chave -> valor
        # simples, tipado assim no core e desenhado linha a linha pela tela. Uma lista lá dentro
        # quebraria o tipo e desenharia "[object Object]".
        "variaveis_env": variaveis_env(),
    }


def _somente_leitura(request: Request) -> dict:
    """O que a tela mostra sem poder editar. Volta também no PATCH: parte dela (a capacidade de
    traduzir o pensamento) muda com um campo editável, e a linha ficava velha até reabrir o modal."""
    return {
            "port": settings.port,
            "lan_bind_ip": settings.lan_bind_ip,
            "server_id": settings.server_id,
            "public_url": settings.public_url,
            # CAPACIDADE, nao nome de sistema: "da pra abrir o painel aqui?". O
            # `os.name == "posix"` que estava aqui respondia outra pergunta — e a diferenca deixou
            # de ser teorica em 22/08/2026, quando o Windows ganhou motor (ConPTY): a resposta la
            # virou True sem ninguem tocar nesta linha, que e o ponto de perguntar por capacidade.
            # Ela tambem responde False num POSIX sem `pty`. Import tardio pelo mesmo motivo de
            # sempre: o termsock nao pode ser importado no topo deste modulo.
            "terminal_panel": _painel_disponivel(),
            # Ha provedor de LLM com chave? Sem isso o front nem pede a traducao do pensamento —
            # cada bloco visivel virava um POST que so voltava 503.
            "traducao_pensamento": _traducao_pensamento_disponivel(),
            # A ORIGEM DESTE cliente abriria o terminal aqui? O handshake do WebSocket recusa com
            # 403 e o navegador nao entrega corpo nem motivo — a tela dizia so "desconectado", e o
            # unico lugar com a explicacao era o log do servidor. Com este campo a propria tela
            # nomeia a origem recusada e manda pro campo que a libera.
            "terminal_origem_ok": _origem_do_terminal_ok(request),
            # A versao do PROCESSO VIVO, nao a do checkout. Durante a janela entre o `git pull` e o
            # restart as duas divergem, e e exatamente ai que o botao Atualizar vive: dizer a do
            # disco aqui seria afirmar estar rodando codigo que ninguem carregou (o defeito que
            # `diag.VERSAO_EM_EXECUCAO` corrigiu em f4013343).
            "versao": diag.VERSAO_EM_EXECUCAO,
    }


# POST **e** PATCH: o PATCH morria em erro de CORS cross-origin e a culpa foi posta no "proxy na
# frente do backend". ERRADO — o culpado era o plugin apiCorsPreflight do frontend/vite.config.ts,
# que respondia TODO preflight de /api com uma LISTA FIXA de metodos que nao tinha PATCH (nem PUT,
# o que so apareceu em 2026-07-31, derrubando salvar motor). Hoje o plugin ECOA o metodo pedido e
# nao ha mais lista pra envelhecer. O cliente segue no POST porque funciona; o PATCH vale pra quem
# chamar a API na mao.
@app.post("/api/config", dependencies=[Depends(require_auth)])
@app.patch("/api/config", dependencies=[Depends(require_auth)])
async def patch_config(request: Request):
    """Grava overrides. Campo desconhecido e ignorado (o cliente nao inventa setting); tipo errado
    volta 400 com a mensagem, em vez de gravar lixo que so quebraria depois."""
    body = await request.json()
    if not isinstance(body, dict):
        raise HTTPException(400, detail=erro("erro_corpo_deve_ser_objeto", "corpo deve ser um objeto"))
    remover = {campo for campo, valor in body.items() if valor is None}
    mudancas = {campo: valor for campo, valor in body.items() if valor is not None}
    try:
        await asyncio.to_thread(runtime_config.aplicar, mudancas, remover=remover)
    except ValueError as e:
        raise HTTPException(400, str(e))
    if "claude_function_hooks" in body:
        # O wrapper do shell lê o caminho do plugin de um arquivo; ele acompanha o interruptor.
        try:
            await asyncio.to_thread(plugin_bridge.publish_address)
        except OSError:
            _log.warning("plugin: caminho do plugin não regravado", exc_info=True)
    return {"campos": runtime_config.estado(), "somente_leitura": _somente_leitura(request)}


def _motores_para_cliente() -> dict[str, dict]:
    """Motores com a api_key MASCARADA: dá para conferir QUAL chave está lá sem copiá-la de volta
    (mesma regra do groq_api_key no runtime_config)."""
    out = {}
    for nome, e in engines.listar().items():
        visivel = dict(e)
        chave = visivel.pop("api_key", "")
        visivel["api_key"] = runtime_config.mascarar(chave)
        visivel["api_key_definida"] = bool(chave)
        try:
            if cliproxy.is_local_engine(e):
                from app.cliproxy_accounts import list_accounts
                visivel["cliproxy_accounts"] = []
                visivel["cliproxy_accounts"] = list_accounts()
                if not visivel["cliproxy_accounts"]:
                    visivel["cliproxy_error"] = "nenhuma conta ChatGPT do proxy corresponde às contas cadastradas no Hangar"
        except ValueError as exc:
            visivel["cliproxy_error"] = str(exc)
        out[nome] = visivel
    return out


@app.get("/api/providers", dependencies=[Depends(require_auth)])
async def get_providers():
    return await _session_provider_catalog()


async def _session_provider_catalog() -> dict[str, dict]:
    from app import conta_estado, session_defaults

    probes = {provider: dict(probe) for provider, probe in
              (await asyncio.to_thread(cli_probe.sondar_providers)).items()}
    connected: set[str] = set()
    disconnected: set[str] = set()
    if probes.get("claude", {}).get("disponivel"):
        configs = await asyncio.to_thread(list_config_dirs, False)
        try:
            logins = await asyncio.to_thread(conta_estado.logins, configs)
        except Exception:  # noqa: BLE001 — sem o login a escolha só deixa de preferir quem está conectado
            _log.warning("catálogo de provedores: login Claude ilegível", exc_info=True)
            logins = []
        if any(login.loggedIn is True for login in logins) or engines.listar():
            connected.add("claude")
        elif logins and all(login.loggedIn is False for login in logins):
            disconnected.add("claude")
    if probes.get("codex", {}).get("disponivel"):
        auth = [login for _account, login in await _codex_auth_states()]
        if any(item.get("status") == "connected" for item in auth):
            connected.add("codex")
        elif auth and all(item.get("status") == "disconnected" for item in auth):
            disconnected.add("codex")
    default = session_defaults.choose_provider(probes, runtime_config.get("last_session_provider"),
                                               connected, disconnected)
    return {provider: {**probe, "default": provider == default} for provider, probe in probes.items()}


async def _codex_auth_states() -> list[tuple]:
    service = _codex_service()
    if service is None:
        return []
    accounts = await asyncio.to_thread(codex_accounts.list_visible_accounts)
    results = await asyncio.gather(*(service.read_auth_rapido(account) for account in accounts),
                                   return_exceptions=True)
    states = []
    for account, result in zip(accounts, results):
        if isinstance(result, BaseException):
            _log.warning("login Codex de %s ilegível: %s", account.id, result)
            result = {"status": "unavailable"}
        states.append((account, result))
    return states


async def _connected_codex_accounts() -> list:
    return [account for account, login in await _codex_auth_states() if login.get("status") == "connected"]


async def _default_session_provider(config_dir=None, engine=None, codex_account=None, omp_profile=None,
                                    subagent_model=None) -> str:
    # Opções exclusivas de um provedor continuam identificando o destino dos clientes antigos.
    if config_dir is not None or engine or subagent_model is not None:
        return "claude"
    if codex_account is not None:
        return "codex"
    if omp_profile:
        return "omp"
    probes = await _session_provider_catalog()
    return next((provider for provider, probe in probes.items() if probe["default"]), "claude")


@app.get("/api/engines", dependencies=[Depends(require_auth)])
def get_engines():
    # arquivo_corrompido: distingue "ninguém configurou motor" de "engines.json existe mas não
    # pôde ser lido" — as duas batem em {} no listar() de propósito (não pode derrubar sessão nem
    # o tick do SSE por um hand-edit ruim), mas a tela precisa saber a diferença (item 1 do
    # review): sem isto o usuário vê "nenhum motor ainda" com um arquivo quebrado escondendo
    # motores reais, re-adiciona um, e a próxima gravação apaga os outros.
    return {
        "motores": _motores_para_cliente(),
        "arquivo_corrompido": engines.arquivo_corrompido(),
        "arquivo_caminho": str(engines.caminho()),
    }


@app.put("/api/engines/{nome}", dependencies=[Depends(require_auth)])
async def put_engine(nome: str, request: Request):
    """Cria/atualiza um motor.

    api_key ausente, vazia, ou IGUAL à máscara que o cliente recebeu = preserva a atual. Sem isso,
    salvar o formulário só para trocar o modelo apagava a chave, sem volta — o bug pago em 22ae599.

    Campo AUSENTE do corpo herda o valor do disco; campo presente vale, inclusive `""`, que é como
    se LIMPA (_normalizar descarta vazio, então o campo sai do registro — texto ou numérico). `0`
    NÃO limpa: num campo numérico é valor inválido, e volta 400 "deve ser maior que zero".
    engines.salvar() substitui o registro inteiro, e sem a herança um cliente que só conhece parte
    do schema — um PUT de script, uma versão antiga do front — apagava o resto calado. Medido: o
    probe de modelos devolve `context_length: null` para provedor que não informa (opencode), o PUT
    seguinte vinha sem `context_window` e o motor perdia a janela de 1M, voltando a compactar em
    200k sem avisar.

    `null` conta como AUSENTE de propósito (é o que o probe manda quando não sabe). Quem quer limpar
    manda `""` — a tela de Motores faz isso nos campos opcionais."""
    body = await request.json()
    if not isinstance(body, dict):
        raise HTTPException(400, detail=erro("erro_corpo_deve_ser_objeto", "corpo deve ser um objeto"))
    if body.pop("use_cliproxy_key", None) is True:
        try:
            inst = await asyncio.to_thread(cliproxy.local)
        except ValueError as e:
            raise HTTPException(400, str(e))
        if not inst:
            raise HTTPException(400, detail=erro("erro_cliproxy_ausente", "CLIProxyAPI local sem config ou sem api-keys"))
        # A chave do config só vai para a instância dele, nunca para um endereço escolhido pelo cliente.
        if cliproxy.normalize_base(str(body.get("base_url") or "")) != inst["base_url"]:
            raise HTTPException(400, detail=erro("erro_cliproxy_endereco", "endereço diferente do CLIProxyAPI local"))
        body["api_key"] = inst["api_key"]
    # I/O de disco no threadpool, igual ao resto deste handler (ver comentário acima de create_session).
    atual = (await asyncio.to_thread(engines.listar)).get(nome, {})
    chave_atual = atual.get("api_key", "")
    enviada = body.get("api_key")
    if chave_atual and (
        not isinstance(enviada, str)
        or not enviada.strip()
        or enviada.strip() == runtime_config.mascarar(chave_atual)
    ):
        body["api_key"] = chave_atual
    # `campo not in body` (não `body.get(campo) is None`): a segunda forma tratava `""` como ausente
    # e reinjetava o valor do disco, então LIMPAR um campo opcional na tela virava no-op com HTTP 200
    # — o usuário escolhia "mesmo que o principal" em subagent_model, salvava, e o modelo antigo
    # voltava sem aviso. `null` segue herdando (é o que o probe manda quando não sabe o valor).
    for campo, valor_atual in atual.items():
        if campo not in body or body[campo] is None:
            body[campo] = valor_atual
    try:
        await asyncio.to_thread(engines.salvar, nome, body)
    except ValueError as e:
        raise HTTPException(400, str(e))
    # A chave do cache é o NOME do motor: trocar base_url ou api_key mantendo o nome serviria a
    # lista do provedor ANTIGO por até 5 minutos.
    _engine_models_cache.pop(nome, None)
    return {"motores": await asyncio.to_thread(_motores_para_cliente)}


@app.delete("/api/engines/{nome}", dependencies=[Depends(require_auth)])
async def delete_engine(nome: str):
    try:
        if not await asyncio.to_thread(engines.remover, nome):
            raise HTTPException(404, detail=erro("erro_motor_nao_encontrado", "motor nao encontrado"))
    except ValueError as e:
        # engines.json corrompido: remover() recusa escrever por cima (item 1 do review) em vez de
        # apagar os outros motores. Vira 400 com a mensagem em vez de 500 cru.
        raise HTTPException(400, str(e))
    # Mesma invalidação do PUT: a chave do cache é o NOME do motor.
    _engine_models_cache.pop(nome, None)
    # O espelho nos outros agentes sai junto; sem isso o Kimi seguia listando um provedor cuja
    # chave o app já não conhece.
    espelhos = await asyncio.to_thread(agentes_sync.remover, nome)
    for alvo, r in espelhos.items():
        if not r["ok"] and r["motivo"] not in ("nao-gerenciado", "nao-instalado"):
            _log.warning("motor %r apagado, mas o espelho no %s ficou: %s", nome, alvo, r["motivo"])
    return {"ok": True}


class EngineProbeBody(_StrictBody):
    # `nome` de um motor já salvo (reusa a key do disco, que o cliente não tem inteira) OU
    # base_url+api_key de um motor sendo criado. Os dois modos são MUTUAMENTE EXCLUSIVOS — ver
    # o guard em engine_modelos: misturar `nome` com um `base_url` do cliente mandaria a api_key
    # REAL do motor salvo, no header Authorization, para qualquer host que o cliente escolher.
    nome: str | None = None
    base_url: str | None = None
    api_key: str | None = None


@app.post("/api/engines/modelos", dependencies=[Depends(require_auth)])
async def engine_modelos(body: EngineProbeBody):
    """Modelos que a key pode usar, direto do provedor. É também o 'Testar' da tela: 200 = key boa,
    502 = a mensagem do provedor (401, host errado, endpoint ausente).

    `nome` e `base_url`/`api_key` não se combinam: com `nome`, SÓ o base_url e a key salvos valem
    — um base_url do cliente junto seria exfiltração da key real para host arbitrário, não SSRF
    comum (o app já aceita SSRF cego por trás do token; isto seria mais forte, key sai de propósito).
    Recusa em vez de ignorar em silêncio: um ignore silencioso deixaria o cliente achando que testou
    o host que mandou."""
    if body.nome:
        if body.base_url or body.api_key:
            raise HTTPException(400, detail=erro("erro_motor_nome_com_dados", "nome já usa o motor salvo; não envie base_url/api_key junto"))
        # I/O de disco no threadpool, igual ao resto deste handler (ver comentário acima de create_session).
        salvo = (await asyncio.to_thread(engines.listar)).get(body.nome)
        if not salvo:
            raise HTTPException(404, detail=erro("erro_motor_nao_encontrado", "motor nao encontrado"))
        base_url, api_key = salvo["base_url"], salvo["api_key"]
    else:
        base_url, api_key = body.base_url, body.api_key
        if not base_url or not api_key:
            raise HTTPException(400, detail=erro("erro_motor_nome_ou_dados", "informe nome de um motor salvo, ou base_url + api_key"))
    try:
        # Mesma guarda do salvar: a key vai no header, http para host público a expõe na rede.
        base_url = engines.validar_base_url(base_url)
    except ValueError as e:
        raise HTTPException(400, str(e))
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, base_url, api_key)
    except RuntimeError as e:
        raise HTTPException(502, str(e))
    except ValueError as e:
        # \r/\n na key ou no base_url (item 3 do review): engine_probe recusa ANTES de montar o
        # Request — sem isto o urllib levantaria com a key crua na mensagem, e a rota abaixo relança
        # RuntimeError pro uvicorn logar (traceback com a key no journal). 400 sem ecoar o valor.
        raise HTTPException(400, str(e))
    # Diz à tela que é o CLIProxyAPI desta máquina, que aceita os campos beta (ver MotorForm).
    try:
        local = await asyncio.to_thread(cliproxy.is_local_engine, {"base_url": base_url})
    except ValueError as e:
        # Config do proxy quebrada não derruba o teste; a tela de CLIProxyAPI mostra o motivo.
        _log.warning("CLIProxyAPI local ilegível ao testar motor: %s", e)
        local = False
    return {"modelos": modelos, "gateway": "cliproxyapi" if local else None}


@app.get("/api/engines/cliproxy", dependencies=[Depends(require_auth)])
async def engine_cliproxy():
    """CLIProxyAPI desta máquina: endereço e modelos, sem a chave."""
    try:
        inst = await asyncio.to_thread(cliproxy.local)
    except ValueError as e:
        return {"found": False, "base_url": None, "models": [], "error": str(e)}
    if not inst:
        return {"found": False, "base_url": None, "models": [], "error": None}
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, inst["base_url"], inst["api_key"])
    except (RuntimeError, ValueError) as e:
        return {"found": True, "base_url": inst["base_url"], "models": [],
                "error": cliproxy.redact(str(e), inst["api_key"])}
    return {"found": True, "base_url": inst["base_url"], "error": None,
            "models": [m for m in modelos if cliproxy.is_engine_model(m["id"])],
            **await asyncio.to_thread(_cliproxy_naming)}


def _cliproxy_naming() -> dict:
    """Nomeia as contas que der e diz quantas ficaram sem nome — a tela pede a senha só aí."""
    erro_nome = None
    try:
        cliproxy.name_accounts()
    except ValueError as e:
        erro_nome = str(e)
    try:
        faltam: int | None = len(cliproxy.unnamed_accounts())
    except ValueError as e:
        # Credencial ilegível: a senha não resolve, então a tela mostra só o motivo.
        faltam, erro_nome = None, erro_nome or str(e)
    return {"unnamed_accounts": faltam, "management_key_set": cliproxy.management_key() is not None,
            "naming_error": erro_nome}


class CliproxyManagementBody(_StrictBody):
    management_key: str = Field(default="", max_length=512)


@app.put("/api/engines/cliproxy/management-key", dependencies=[Depends(require_auth)])
async def engine_cliproxy_management_key(body: CliproxyManagementBody):
    """Guarda a senha de gerenciamento do CLIProxyAPI e já nomeia as contas; o valor nunca volta."""
    try:
        await asyncio.to_thread(cliproxy.set_management_key, body.management_key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return await asyncio.to_thread(_cliproxy_naming)


def _id_upload(info: SessionInfo) -> str:
    """Id durável da sessão, que é como a pasta de anexos é chaveada — nome muda no rename, id não.
    Sessão sem transcript ainda cai no nome: janela curta, e um anexo mandado nela fica para trás
    se ela for renomeada depois."""
    return session_key(info.jsonl) if info.jsonl else info.name


@app.post("/api/sessions/{name}/upload", dependencies=[Depends(require_auth), Depends(_transfer_check)])
async def upload(name: str, request: Request, audio_only: bool = False):
    from app import upload_bridge
    forwarded = await upload_bridge.forward(name, "save", request, audio_only=audio_only)
    if forwarded is not None:
        return forwarded
    # Resolve o cwd da sessao (registry.list() ja traz cwd via tmux #{pane_current_path}).
    # handler async -> registry.list() (subprocess tmux) no threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.cwd:
        raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, detail=erro("erro_arquivo_grande", "arquivo maior que 100 MiB"))
    data = await request.body()
    # Filename do cliente (X-Filename, percent-encoded) ou ?name= -> so a EXTENSAO e usada
    # (o nome final e gerado pelo servidor). Qualquer tipo de arquivo.
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        # write_bytes (ate 100 MiB) no threadpool pra nao bloquear o loop durante o disco.
        path = await asyncio.to_thread(save_upload, info.cwd, _id_upload(info), data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)

    # Higiene: varre anexos velhos DESTA sessao. Barato (um listdir) e sem agendador pra manter.
    # Falhar aqui nao pode custar o upload que acabou de dar certo.
    try:
        await asyncio.to_thread(prune_old, info.cwd, runtime_config.get("upload_retention_days"))
    except Exception:
        _log.exception("prune de uploads falhou (upload seguiu)")

    # Video: o Read nao abre mp4, entao o anexo virava um caminho morto pro modelo. Extrai quadros
    # ao longo da duracao + transcreve o audio -> vira coisa legivel. Best-effort: sem ffmpeg/sem
    # audio/sem chave da Groq, devolve o que conseguiu e o upload segue igual.
    frames: list[str] = []
    fala = ""
    # `audio_only`: o ditado manda o áudio aqui e transcreve no /transcribe?arquivo=; tratar o webm
    # como vídeo extrairia quadros e pagaria uma segunda transcrição.
    if is_video(path) and not audio_only:
        try:
            frames = await asyncio.to_thread(extract_frames, path)
        except Exception:
            _log.exception("extracao de quadros falhou (upload seguiu)")
        try:
            audio = await asyncio.to_thread(extract_audio, path)
            if audio:
                bytes_audio = await asyncio.to_thread(Path(audio).read_bytes)
                fala = await asyncio.to_thread(transcribe, bytes_audio, "audio.m4a")
        except TranscribeError as e:
            _log.info("video sem transcricao: %s", e.detail)
        except Exception:
            _log.exception("transcricao do video falhou (upload seguiu)")
    return {"path": path, "frames": frames, "transcript": fala.strip()}


@app.post("/api/sessions/{name}/transcribe", dependencies=[Depends(require_auth), Depends(_transfer_check)])
async def transcribe_audio(name: str, request: Request, limpar: bool = False, estilo: str | None = None,
                           arquivo: str | None = None):
    # Com corpo: salva o áudio (anexo de áudio/vídeo) e transcreve num round-trip, raw body +
    # X-Filename. Com `arquivo`: transcreve um áudio já enviado pelo /upload, sem gravar outra cópia
    # — o ditado faz assim para o cliente ter o caminho antes da transcrição e poder tentar de novo.
    # `limpar` só o microfone manda: áudio ANEXADO (arquivo de até 10min) não pode pagar a limpeza.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.cwd:
        raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
    from app import upload_bridge
    if arquivo:
        try:
            forwarded = await asyncio.to_thread(upload_bridge.request_json, name, "resolve",
                                                filename=arquivo, allow_absolute=not _convidado(request))
            path = forwarded["path"] if forwarded is not None else await asyncio.to_thread(
                resolve_session_audio, info.cwd, _id_upload(info), arquivo,
                allow_absolute=not _convidado(request))
        except UploadError as e:
            if e.status == 404:
                raise HTTPException(404, detail=erro("erro_upload_inexistente",
                                                     "audio nao encontrado na pasta da sessao"))
            if e.status == 403:
                raise HTTPException(403, detail=erro("erro_arquivo_caminho_convidado",
                                                     "convidado so transcreve audio da pasta da sessao"))
            raise HTTPException(e.status, e.detail)
        data = await asyncio.to_thread(Path(path).read_bytes)
        filename = Path(path).name
    else:
        clen = request.headers.get("content-length")
        if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
            raise HTTPException(413, detail=erro("erro_arquivo_grande", "arquivo maior que 100 MiB"))
        data = await request.body()
        filename = request.headers.get("x-filename") or request.query_params.get("name")
        try:
            forwarded = await asyncio.to_thread(upload_bridge.request_json, name, "save",
                                                data=data, filename=filename or "")
            path = forwarded["path"] if forwarded is not None else await asyncio.to_thread(
                save_upload, info.cwd, _id_upload(info), data, filename)
        except UploadError as e:
            raise HTTPException(e.status, e.detail)
    limits = DICTATION_LIMITS if limpar else FILE_LIMITS
    # Transcricao (chamada de rede bloqueante) no threadpool pra nao travar o loop.
    try:
        t = await asyncio.to_thread(transcribe_with_provider, data, filename, limits)
    except TranscribeError as e:
        raise HTTPException(e.status, e.detail)
    if not limpar:
        return _with_provider({"path": path, "text": t.text}, t)
    return {"path": path, **_with_provider(await _cleaned_dictation(t.text, estilo), t)}


@app.post("/api/dictation/transcribe", dependencies=[Depends(require_auth)])
async def transcribe_dictation(request: Request, estilo: str | None = None, limpar: bool = True):
    # Antes de existir uma sessão, o áudio não tem uma pasta onde ser guardado.
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, detail=erro("erro_arquivo_grande", "arquivo maior que 100 MiB"))
    data = await request.body()
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        t = await asyncio.to_thread(transcribe_with_provider, data, filename, DICTATION_LIMITS)
    except TranscribeError as e:
        raise HTTPException(e.status, e.detail)
    if not limpar:
        return _with_provider({"text": t.text}, t)
    return _with_provider(await _cleaned_dictation(t.text, estilo), t)


def _with_provider(result: dict, t: Transcription) -> dict:
    """Junta à resposta quem transcreveu. O aviso da reserva vem antes do da limpeza e nenhum dos
    dois some; `estilo_aplicado` já foi decidido só pelo aviso da limpeza."""
    out = {**result, "provider": t.provider}
    avisos = [a for a in (t.aviso, result.get("aviso")) if a]
    if avisos:
        out["aviso"] = " · ".join(avisos)
    return out


async def _cleaned_dictation(text: str, estilo: str | None) -> dict:
    # `estilo` = o que a PILL do composer mostrava quando a pessoa falou. Vence a config do
    # servidor (narrar.estilo_efetivo); ausente/desconhecido, a config manda como sempre.
    texto_limpo, aviso = await asyncio.to_thread(narrar.limpar_ditado, text, estilo)
    # `estilo_aplicado` = qual versao o texto de fato recebeu, pra barra do ditado no composer marcar
    # o botao certo. NAO da pra deduzir na tela: o backend rebaixa briefing pra prosa em ditado curto
    # e cai na config quando a pill ainda nao leu o servidor, entao marcar "Briefing" pelo que foi
    # PEDIDO faria o botao mentir. Com aviso, o texto que voltou e o cru — nao um estilo. Idem
    # quando limpar_ditado devolve o proprio texto sem tocar (ditado de menos de 5 palavras, ou
    # comecando com "/"): ali nao houve estilo nenhum, e dizer "prosa" seria a mesma mentira.
    aplicado = "cru" if (aviso or texto_limpo == text) else narrar.estilo_efetivo(text, estilo)
    return {"text": texto_limpo, "raw": text, "aviso": aviso,
            "estilo_aplicado": aplicado}


class RelimparBody(_StrictBody):
    texto: str = Field(min_length=1)
    estilo: str


@app.post("/api/ditado/relimpar", dependencies=[Depends(require_auth)])
async def relimpar_ditado(body: RelimparBody):
    """Aplica OUTRO estilo ao texto CRU de um ditado que ja foi transcrito.

    Sem audio e sem sessao de proposito. A parte cara (Whisper) ja foi paga na transcricao e o cru
    volta de la no campo `raw`; trocar de estilo e so a limpeza de novo. Reenviar o audio custaria
    uma segunda transcricao — dinheiro e ~10s — pra chegar no mesmo texto cru. E limpeza nao le nada
    da sessao (nem cwd, nem provider), entao exigir `name` aqui so acrescentaria um registry.list()
    e um 404 possivel num caminho que nao precisa de nenhum dos dois.

    Estilo invalido e 400 e nao "cai no padrao": aqui a pessoa CLICOU num estilo, entao entregar
    outro calado seria mentir sobre o botao que ela apertou (na transcricao o estilo e um palpite da
    tela e cair na config e o certo)."""
    if body.estilo not in narrar.ESTILOS_DITADO:
        raise HTTPException(400, detail=erro(
            "erro_estilo_invalido",
            f"estilo '{body.estilo}' nao existe. Use um de: {', '.join(narrar.ESTILOS_DITADO)}."))
    texto, aviso = await asyncio.to_thread(narrar.limpar_ditado, body.texto, body.estilo)
    aplicado = "cru" if (aviso or texto == body.texto) else narrar.estilo_efetivo(body.texto, body.estilo)
    return {"text": texto, "aviso": aviso, "estilo_aplicado": aplicado}


@app.get("/api/transcription/providers/status", dependencies=[Depends(require_auth)])
def transcription_providers_status():
    """Espera por cota de cada serviço de transcrição, para a tela de configuração. `def` e não
    `async`: lê um arquivo, e o FastAPI já roda isso na threadpool."""
    return {"providers": providers_status()}


class PensamentoPtBody(_StrictBody):
    # Teto por ITEM só contra abuso: o pensamento do Pi e do Kimi vem CRU, sem tamanho previsível,
    # e recusar com 422 acima de `MAX_CHARS` deixava a pessoa abrindo o bloco sem nada acontecer.
    # O corte em `MAX_CHARS` é feito no handler; aqui só o que nem cabe num pedido razoável.
    textos: list[Annotated[str, Field(max_length=200_000)]] = Field(min_length=1, max_length=20)


@app.post("/api/pensamento/pt", dependencies=[Depends(require_auth)])
async def pensamento_para_pt(body: PensamentoPtBody):
    """Resumo do pensamento em portugues, curto. Chamado quando a pessoa ABRE o bloco.

    Nunca 502: falha de provedor devolve o texto ORIGINAL (ver pensamento_pt.traduzir). O bloco ja
    esta aberto na tela quando esta chamada sai — trocar o conteudo por uma mensagem de erro seria
    apagar o que ela acabou de pedir pra ler.
    """
    # Corta em vez de recusar: um resumo do começo do pensamento é o que a pessoa pediu; um 422
    # calado não é (medido: 198 no diário de uma semana, 110 num dia só).
    textos = [t[:pensamento_pt.MAX_CHARS] for t in body.textos]
    if not runtime_config.get("traduzir_pensamento"):
        return {"textos": textos}
    saida = await asyncio.to_thread(pensamento_pt.traduzir_varios, textos)
    return {"textos": saida}


@app.get("/api/sessions/{name}/uploads/{filename}", dependencies=[Depends(require_auth)])
async def serve_upload(name: str, filename: str, request: Request, download: bool = False):
    from app import upload_bridge
    forwarded = await upload_bridge.forward(name, "download", request, filename=filename, download=download)
    if forwarded is not None:
        return forwarded

    # O caminho local lê sessão e disco: fora do loop, como quando a rota era síncrona.
    def local():
        info = _cached_info_sync(name)
        if info is None or not info.cwd:
            raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
        try:
            path = resolve_upload(info.cwd, _id_upload(info), filename)
        except UploadError as e:
            raise HTTPException(e.status, e.detail)
        return file_response(path, download=download)

    return await asyncio.to_thread(local)


@app.get("/api/sessions/{name}/uploads", dependencies=[Depends(require_auth)])
def list_session_uploads(name: str):
    from app import upload_bridge
    forwarded = upload_bridge.request_json(name, "list")
    if forwarded is not None:
        return forwarded
    # Galeria de anexos: a retencao vive no servidor, entao o prazo sai daqui pronto (o front so
    # desenha). Le do runtime_config, nao do env cru — senao a galeria mostraria um prazo e o
    # prune usaria outro.
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    return {"files": list_uploads(info.cwd, _id_upload(info), runtime_config.get("upload_retention_days"))}


class CheckoutBody(_StrictBody):
    branch: str


class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop", "log",
                    "revert-abort", "cherry-pick-abort"]


class GitPathBody(_StrictBody):
    path: str   # validado em git_ops contra a lista real de arquivos alterados (anti-traversal)


class GitPathDiffBody(_StrictBody):
    # `escopo` e str de proposito, nao Literal: o Literal era validado pelo FastAPI e o
    # cliente recebia 422 com detail em LISTA — o envelope erro_git_diff nunca chegava a
    # nascer. Quem rejeita o valor e o git_ops.path_diff, e o erro sai no envelope.
    path: str
    escopo: str = "branch"


class GitCommitBody(_StrictBody):
    message: str = Field(min_length=1)
    paths: list[str] = []        # sem min_length: amend=True aceita [] (reword); git_ops barra [] sem amend
    amend: bool = False
    new_branch: str | None = None


def _session_info_with_cwd(name: str):
    # Mesmo lookup do upload. 404 se a sessão ou o cwd não existe.
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    return info


def _session_cwd(name: str) -> str:
    return _session_info_with_cwd(name).cwd


def _session_git_cwd(name: str) -> str:
    """O git segue o agente até a worktree; arquivos, uploads e execução ficam no cwd."""
    return _session_info_with_cwd(name).git_dir


@app.get("/api/sessions/{name}/plan", dependencies=[Depends(require_auth)])
async def session_plan(name: str):
    """Detalhe do plano ativo da sessao + o markdown cru. O markdown vem JUNTO de proposito: o
    GET /sessions/{name}/file so serve path que aparece no transcript, e um plano descoberto por
    varredura (sessao nova, pos-/clear) nunca aparece la. O arquivo ja foi lido e parseado aqui."""
    # to_thread e obrigatorio: _session_cwd chama registry.list(), que forka `tmux list-panes` e
    # varre /proc inteiro. As outras 16 rotas que usam _session_cwd sao `def` sync (o FastAPI as
    # joga no threadpool sozinho); esta e async, entao I/O direto na corrotina travaria o loop
    # (mesma classe do incidente 2026-07-23). A HTTPException do 404 propaga pelo to_thread normal.
    cwd = await asyncio.to_thread(_session_cwd, name)   # ja levanta 404 sem sessao/cwd
    p = await asyncio.to_thread(plan_progress, cwd)
    if p is None:
        raise HTTPException(404, detail=erro("erro_sem_plano_ativo", "sem plano ativo"))
    try:
        markdown = await asyncio.to_thread(
            lambda: Path(p.path).read_text(encoding="utf-8", errors="replace"))
    except OSError:
        # plan_progress vem de cache; o arquivo pode ter sumido/perdido permissao entre a leitura
        # cacheada e esta segunda leitura. Degrada pra markdown vazio, mas NAO engole em silencio.
        _log.warning("falha lendo markdown do plano path=%s", p.path, exc_info=True)
        markdown = ""
    return {
        # `stem` (nome do arquivo) alem do `name` (ja sem o prefixo de data): e a chave que o
        # cliente devolve pra marcar step e arquivar, e sao os dois caminhos que o `name`, com a
        # data cortada, nao consegue reabrir.
        "name": p.name, "path": p.path, "stem": Path(p.path).stem,
        "task": p.task_idx, "task_total": p.task_total,
        "done": p.done, "total": p.total, "complete": p.complete,
        "tasks": [{"title": t.title, "done": t.done, "total": t.total,
                   "steps": [{"title": s.title, "done": s.done, "manual": s.manual, "idx": s.idx}
                             for s in t.steps]}
                  for t in p.tasks],
        "markdown": markdown,
    }


@app.get("/api/sessions/{name}/plans", dependencies=[Depends(require_auth)])
async def session_plans(name: str):
    """Todos os planos do repo, pro seletor. Inclui os nao-comecados e os completos — que a eleicao
    automatica descarta, e que sao exatamente os que o usuario precisa poder escolher."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    r = await asyncio.to_thread(list_plans, cwd)
    if r is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    return r


@app.get("/api/orq", dependencies=[Depends(require_auth)])
async def orq_lista():
    """Execucoes de orquestracao (eventos.jsonl escrito pelo arbitro), mais recentes primeiro.
    A lista vem SEM os eventos crus — quem quer a linha do tempo pede o detalhe. Cada uma leva o
    `watchdog` do chip do card: um systemctl por pedido, nunca um por execução."""
    raiz = orq.raiz_padrao()
    execs = await asyncio.to_thread(orq.listar_execucoes, raiz)
    unidades = await asyncio.to_thread(orq_conductor.units)
    vigias = await asyncio.to_thread(
        lambda: {e.id: orq_conductor.watchdog(raiz / e.id, unidades) for e in execs})

    def _resumo(e):
        d = asdict(e)
        d.pop("eventos_execucao", None)
        for t in d["tasks"]:
            t.pop("eventos", None)
        d["watchdog"] = vigias[e.id]
        d["metadata"] = orq.run_metadata(raiz / e.id, e.plano)
        return d

    return await asyncio.to_thread(lambda: {"execucoes": [_resumo(e) for e in execs], "fichas": orq.fichas(execs)})


@app.get("/api/orq/{exec_id}", dependencies=[Depends(require_auth)])
async def orq_detalhe(exec_id: str):
    e = await asyncio.to_thread(orq.detalhe, orq.raiz_padrao(), exec_id)
    if e is None:
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "execucao nao encontrada"))
    metadata = await asyncio.to_thread(orq.run_metadata, orq.raiz_padrao() / e.id, e.plano)
    return {**asdict(e), "metadata": metadata}


@app.get("/api/orq/{exec_id}/panel", dependencies=[Depends(require_auth)])
async def orq_history_panel(exec_id: str):
    """O mesmo painel por execução, sem exigir uma sessão viva."""
    d = orq.exec_dir(orq.raiz_padrao(), exec_id)
    if d is None or not await asyncio.to_thread((d / "eventos.jsonl").is_file):
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "execucao nao encontrada"))
    return await asyncio.to_thread(orq_timeline.panel, d, _guardar_snap)


@app.get("/api/orq/{exec_id}/conductor", dependencies=[Depends(require_auth)])
async def orq_conductor_panel(exec_id: str):
    """Painel do condutor: o vigia vivo ou parado e o feed do que passou pelo orq. Só leitura."""
    d = orq.exec_dir(orq.raiz_padrao(), exec_id)
    if d is None or not await asyncio.to_thread(d.is_dir):
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "execucao nao encontrada"))
    unidades = await asyncio.to_thread(orq_conductor.units)
    return await asyncio.to_thread(orq_conductor.conductor, d, unidades)


class PlanPinBody(_StrictBody):
    stem: str | None = None   # None = solta o pin e volta pra eleicao automatica


@app.post("/api/sessions/{name}/plan-pin", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def session_plan_pin(name: str, body: PlanPinBody):
    """Fixa qual plano o painel mostra. Vale ate o plano fechar: em 100% o pin e ignorado e a
    eleicao automatica volta (ver planprog.plan_progress)."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    root = await asyncio.to_thread(_plans_dir, cwd)
    if root is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    if body.stem is not None and body.stem != PIN_NONE:
        # So um plano que existe DE VERDADE nesta raiz. Sem isto, o stem viraria nome de arquivo
        # vindo do cliente — e a checagem de traversal do read_pin nao cobriria um nome valido
        # apontando pra plano de outro repo. A guarda de separador vem ANTES do isfile: com um
        # `../..` o proprio isfile ja responderia se existe .md fora da pasta de planos.
        if not is_safe_stem(body.stem):
            raise HTTPException(400, detail=erro("erro_nome_plano_invalido", f"nome de plano invalido: {body.stem}", nome=body.stem))
        alvo = os.path.join(root, body.stem + ".md")
        if not await asyncio.to_thread(os.path.isfile, alvo):
            raise HTTPException(404, detail=erro("erro_plano_nao_encontrado", f"plano nao encontrado: {body.stem}", nome=body.stem))
    try:
        await asyncio.to_thread(write_pin, root, body.stem)
    except PlanPinError as e:
        raise HTTPException(500, detail=erro("erro_gravar_pin", f"nao deu pra gravar o pin: {e}", erro=str(e)))
    return {"pinned": body.stem}


async def _plans_root(name: str) -> tuple[str, str]:
    """(cwd, raiz de planos). Devolve os DOIS porque `_session_cwd` chama `registry.list()`, que
    forka tmux e varre /proc — pedir o cwd de novo depois dobraria esse scan por clique."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    root = await asyncio.to_thread(_plans_dir, cwd)
    if root is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    return cwd, root


class PlanStepBody(_StrictBody):
    stem: str
    idx: int      # 0-based, na ordem do documento — vem do proprio /plan
    done: bool


@app.post("/api/sessions/{name}/plan-step", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def session_plan_step(name: str, body: PlanStepBody):
    """Marca/desmarca UM step no .md do plano. Quem marca no fluxo normal e o agente; isto existe
    pro caso dele esquecer — sem marcacao o plano nunca fecha e trava o painel na etapa errada."""
    cwd, root = await _plans_root(name)
    try:
        path = caminho_do_plano(root, body.stem)
        await asyncio.to_thread(marcar_step, path, body.idx, body.done)
    except PlanWriteError as e:
        # 409 e nao 500: os modos de falha reais aqui sao "o arquivo mudou/nao serve", nao bug do
        # servidor — e a UI PRECISA mostrar o texto (o clique some sem explicacao, senao).
        raise HTTPException(409, detail=erro("erro_marcar_step", f"nao deu pra marcar o step: {e}", erro=str(e)))
    p = await asyncio.to_thread(plan_progress, cwd)
    return {"done": p.done if p else None, "total": p.total if p else None,
            "complete": bool(p and p.complete)}


class PlanArchiveBody(_StrictBody):
    stem: str


@app.post("/api/sessions/{name}/plan-archive", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def session_plan_archive(name: str, body: PlanArchiveBody):
    """Encerra o plano: move o .md (e o .html irmao) pra docs/superpowers/plans/feitos/."""
    _, root = await _plans_root(name)
    try:
        movidos = await asyncio.to_thread(arquivar, root, body.stem)
    except PlanWriteError as e:
        raise HTTPException(409, detail=erro("erro_arquivar_plano", f"nao deu pra arquivar: {e}", erro=str(e)))
    return {"moved": movidos}


@app.get("/api/sessions/{name}/branches", dependencies=[Depends(require_auth)])
def branches(name: str):
    try:
        return list_branches(_session_git_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/checkout", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def checkout(name: str, body: CheckoutBody):
    try:
        return switch_branch(_session_git_cwd(name), body.branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git(name: str, body: GitActionBody):
    try:
        return git_action(_session_git_cwd(name), body.action)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/files", dependencies=[Depends(require_auth)])
def git_files(name: str):
    try:
        cwd = _session_git_cwd(name)
        # sequencer: revert/cherry-pick em andamento (conflito) — o front deriva o botao de abort
        # DAQUI, nao de memoria de sessao (ver gitStore.svelte.ts:pendingAbort).
        return {"files": changed_files(cwd), "sequencer": sequencer_state(cwd)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/log", dependencies=[Depends(require_auth)])
def git_log_route(name: str, q: str | None = None, n: int = 50):
    try:
        cwd = _session_git_cwd(name)
        # `n` vem do "carregar mais" da coluna: a lista pede o dobro a cada vez. Teto de 2000 pra
        # uma URL forjada não fazer o git montar o histórico inteiro de um repo grande.
        commits = git_log(cwd, n=max(1, min(n, 2000)), grep=q)
        resumo = git_summary(cwd) or {}
        # Com busca ativa (q), NAO monta o grafo: --grep tira commits do meio e assign_lanes
        # desenharia arestas pra parents que sumiram da lista (lane que nunca fecha).
        return {"commits": commits if q else assign_lanes(commits),
                "ahead": resumo.get("ahead"), "behind": resumo.get("behind")}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/diff", dependencies=[Depends(require_auth), Depends(_transfer_check)])
def git_diff(name: str, body: GitPathBody):
    try:
        return file_diff(_session_git_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/discard", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_discard(name: str, body: GitPathBody):
    try:
        return discard_file(_session_git_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/commit", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_commit(name: str, body: GitCommitBody):
    try:
        return commit(_session_git_cwd(name), body.message, body.paths, body.amend, body.new_branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/last-message", dependencies=[Depends(require_auth)])
def git_last_message(name: str):
    try:
        return last_commit_message(_session_git_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/files", dependencies=[Depends(require_auth)])
def git_commit_files(name: str, sha: str):
    try:
        return {"files": commit_files(_session_git_cwd(name), sha)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/diff", dependencies=[Depends(require_auth)])
def git_commit_diff(name: str, sha: str, path: str):
    try:
        return commit_file_diff(_session_git_cwd(name), sha, path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


class GitShaBody(_StrictBody):
    sha: str   # validado em git_ops por _SHA_RE (hex 7-40) antes de virar argv


class GitResetBody(_StrictBody):
    sha: str
    mode: Literal["soft", "mixed", "hard"]   # enum no schema E no git_ops


class GitBranchBody(_StrictBody):
    name: str
    sha: str | None = None
    switch_after: bool = False


class GitTagBody(_StrictBody):
    name: str
    sha: str | None = None
    message: str | None = None


@app.get("/api/sessions/{name}/git/commit/{sha}/diff-full", dependencies=[Depends(require_auth)])
def git_commit_diff_full(name: str, sha: str):
    try:
        return commit_diff(_session_git_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/revert", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_revert(name: str, body: GitShaBody):
    try:
        return revert_commit(_session_git_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/cherry-pick", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_cherry_pick(name: str, body: GitShaBody):
    try:
        return cherry_pick(_session_git_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/push", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_push(name: str):
    try:
        return push_branch(_session_git_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/reset", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_reset(name: str, body: GitResetBody):
    try:
        return reset_to(_session_git_cwd(name), body.sha, body.mode)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/branch", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_branch_create(name: str, body: GitBranchBody):
    try:
        return create_branch_at(_session_git_cwd(name), body.name, body.sha, body.switch_after)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/tag", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def git_tag_create(name: str, body: GitTagBody):
    try:
        return create_tag(_session_git_cwd(name), body.name, body.sha, body.message)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/diff-worktree", dependencies=[Depends(require_auth)])
def git_commit_diff_worktree(name: str, sha: str):
    try:
        return diff_vs_worktree(_session_git_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/branches", dependencies=[Depends(require_auth)])
def git_commit_branches(name: str, sha: str):
    try:
        return branches_containing(_session_git_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


# Textos FIXOS do envelope, por familia de erro: o detalhe interno (e.msg/str(e) do
# SearchError e do GitError pode carregar caminho absoluto ou stderr do git, e atravessar
# a API exporia segredo. O detalhe vai SO para o log, passando pelo _scrub (redige
# userinfo de remote). O front mostra a chave traduzida; o `msg` do envelope e a rede
# quando o front nao conhece o code — e ele tambem e fixo, por isso.
_MSG_ARQ = "Não deu para acessar esse arquivo ou pasta."
_MSG_BUSCA = "Não deu para completar a busca."
_MSG_DIFF = "Não deu para montar o diff."


def _erro_arq(e: FileError | SearchError) -> HTTPException:
    # As chaves `erro_arq_busca_falhou` e `erro_git_diff` trazem `{msg}` no texto, e a
    # funcao do paraglide exige o argumento — sem ele o front renderiza `undefined` ou
    # nem compila. O `erro()` tem `msg` como parametro nomeado, entao o valor entra no
    # dict de params DEPOIS, por chave.
    fixo = e.msg if e.code.startswith("workspace_") else (_MSG_BUSCA if isinstance(e, SearchError) else _MSG_ARQ)
    _log.warning("files: %s", git_ops._scrub(e.msg))
    d = erro(e.code, fixo)
    d["params"]["msg"] = fixo
    return HTTPException(status_code=e.status, detail=d)


@app.get("/api/sessions/{name}/files/list", dependencies=[Depends(require_auth)])
def files_list(name: str, path: str | None = None, so_modificados: bool = True):
    try:
        return filetree.list_dir(_session_cwd(name), path, so_modificados)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/read", dependencies=[Depends(require_auth)])
def files_read(name: str, path: str):
    try:
        return filetree.read_file(_session_cwd(name), path)
    except FileError as e:
        raise _erro_arq(e)


class FileWriteBody(_StrictBody):
    path: str
    text: str
    # A impressão da leitura. Sem ela a gravação é recusada: escrever às cegas por cima do que o
    # agente da sessão acabou de mudar é o desfecho que este campo existe pra impedir.
    digest: str | None = None


@app.post("/api/sessions/{name}/files/write", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def files_write(name: str, body: FileWriteBody):
    try:
        return filetree.write_file(_session_cwd(name), body.path, body.text, body.digest)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/search", dependencies=[Depends(require_auth)])
def files_search(name: str, q: str, mode: str = "names"):
    # `mode` e str de proposito, nao Literal: o Literal era validado pelo FastAPI e o 422
    # com detail em LISTA engolia o erro_arq_modo_invalido. Quem rejeita e o filesearch.
    try:
        return filesearch.search(_session_cwd(name), q, mode)
    except SearchError as e:
        raise _erro_arq(e)


class ResolverBody(_StrictBody):
    caminhos: list[str]


_ELSEWHERE_MAX = 30


@app.post("/api/sessions/{name}/files/resolver", dependencies=[Depends(require_auth), Depends(_transfer_check)])
def files_resolver(name: str, body: ResolverBody):
    """Visão "citados": confere de uma vez quais caminhos citados existem (e resolve os relativos
    a outra pasta pelo sufixo). Quem não existe não entra na lista."""
    try:
        info = _cached_info_sync(name)
        if info is None or not info.cwd:
            raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
        from app.transcript import citation_cwds
        rows = _conversation_rows(info) if info.jsonl else None
        cited = citation_cwds(info.jsonl, body.caminhos, rows=rows) if info.jsonl else {}
        found: dict[str, dict] = {}
        elsewhere = 0
        for path in body.caminhos:
            bases = list(dict.fromkeys([*(cited.get(path) or []), info.cwd]))
            for suffix in (False, True):
                for base in bases:
                    result = filesearch.resolver(base, [path], suffix=suffix)
                    target = result["ok"].get(path)
                    if target is None:
                        continue
                    if os.path.realpath(base) != os.path.realpath(info.cwd):
                        # O FilesStore lê relativos pelo cwd de nascimento. Outro cwd usa /file.
                        target["relativo"] = None
                    found[path] = target
                    break
                if path in found:
                    break
            # Nome solto ou relativo de outro repositório, só se a conversa o citou; a leitura vai pela rota de arquivo
            # citado, que faz a mesma busca. Cada um relê o transcript: teto por pedido.
            if path not in found and info.jsonl and path in cited and elsewhere < _ELSEWHERE_MAX:
                elsewhere += 1
                whole = _cited_elsewhere(info.jsonl, info.cwd, path, cited[path], siblings=True, rows=rows)
                if whole:
                    found[path] = {"relativo": None, "real": os.path.realpath(whole)}
        return {"ok": {path: found[path] for path in body.caminhos if path in found},
                "faltam": [path for path in body.caminhos if path not in found]}
    except SearchError as e:
        raise _erro_arq(e)


@app.post("/api/sessions/{name}/git/path-diff", dependencies=[Depends(require_auth), Depends(_transfer_check)])
def git_path_diff(name: str, body: GitPathDiffBody):
    try:
        return git_ops.path_diff(_session_git_cwd(name), body.path, body.escopo)
    except GitError as e:
        _log.warning("path-diff: %s", git_ops._scrub(str(e)))
        d = erro("erro_git_diff", _MSG_DIFF)
        d["params"]["msg"] = _MSG_DIFF
        raise HTTPException(status_code=e.status, detail=d)


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


def _project_error(e: project_shortcuts.ProjectError) -> HTTPException:
    return HTTPException(409, detail=erro("erro_project_shortcuts_projeto", str(e), detalhe=str(e)))


def _project_file_error(e: project_shortcuts.FileError) -> HTTPException:
    return HTTPException(500, detail=erro("erro_project_shortcuts_arquivo", str(e), detalhe=str(e)))


@app.get("/api/sessions/{name}/project-shortcuts", dependencies=[Depends(require_auth)])
def get_project_shortcuts(name: str):
    try:
        return project_shortcuts.describe(_session_cwd(name))
    except project_shortcuts.ProjectError as e:
        raise _project_error(e)
    except project_shortcuts.FileError as e:
        raise _project_file_error(e)


@app.put("/api/sessions/{name}/project-shortcuts", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def put_project_shortcuts(name: str, body: ProjectShortcutsBody):
    # Lista INTEIRA do projeto (add/editar/remover sao a mesma gravacao); vazia remove o projeto.
    cwd = _session_cwd(name)
    try:
        project = project_shortcuts.project_of(cwd)
        project_shortcuts.save_items(project["key"], body.items)
        return {**project, "items": project_shortcuts.load_items(project["key"])}
    except project_shortcuts.ProjectError as e:
        raise _project_error(e)
    except project_shortcuts.FileError as e:
        raise _project_file_error(e)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_project_shortcuts", str(e), detalhe=str(e)))


_DISPLAY_VARS = ("DISPLAY", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "HYPRLAND_INSTANCE_SIGNATURE")
_SHORTCUT_FAIL_WINDOW = 2.0


def _shortcut_output_tail(raw: bytes, lines: int = 5, chars: int = 400) -> str:
    """Ultimas linhas da saida do atalho, curtas o bastante pra caber no aviso da tela."""
    text = raw.decode("utf-8", errors="replace")
    tail = " | ".join(l.strip() for l in text.strip().splitlines()[-lines:] if l.strip())
    return tail if len(tail) <= chars else "…" + tail[-chars:]


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


@app.post("/api/sessions/{name}/shortcut-shell", dependencies=[Depends(require_auth), Depends(_transfer_guard)],
          status_code=202)
def shortcut_shell(name: str, body: ShortcutShellBody, request: Request):
    return _shortcut_shell(name, body, request, powershell=False)


@app.post("/api/sessions/{name}/run-code", dependencies=[Depends(require_auth), Depends(_transfer_guard)], status_code=202)
def run_code(name: str, body: RunCodeBody, request: Request):
    if len(body.command) > 4096:
        raise HTTPException(400, detail=erro("erro_run_code_longo", "comando longo demais"))
    if "\0" in body.command:
        raise HTTPException(400, detail=erro("erro_run_code_invalido", "comando invalido"))
    if body.key and (not body.key.startswith("run-code:") or not all(c.isascii() and (c.isalnum() or c in ":-") for c in body.key)):
        raise HTTPException(400, detail=erro("erro_run_code_invalido", "identificador invalido"))
    language = (body.language or "").strip().lower()
    unix = {"bash", "sh", "zsh", "fish"}
    powershell = {"powershell", "ps1", "pwsh"}
    if language and language not in unix | powershell | {"shell"}:
        raise HTTPException(400, detail=erro("erro_run_code_linguagem", "linguagem de terminal invalida"))
    if (os.name == "nt" and language in unix) or (os.name != "nt" and language in powershell):
        raise HTTPException(409, detail=erro("erro_run_code_shell_incompativel", "o bloco nao combina com o sistema deste servidor",
                                              linguagem=language, sistema="Windows" if os.name == "nt" else "Linux"))
    shell = None
    if os.name == "nt" and language == "pwsh":
        shell = shutil.which("pwsh.exe")
    elif os.name != "nt" and language not in ("", "shell"):
        shell = shutil.which(language)
    if language not in ("", "shell") and shell is None and (os.name != "nt" or language == "pwsh"):
        raise HTTPException(409, detail=erro("erro_run_code_shell_ausente", "interpretador nao instalado", linguagem=language))
    from app import termsock
    if not termsock.painel_disponivel():
        raise HTTPException(409, detail=erro("erro_run_code_terminal", "terminal indisponivel nesta maquina"))
    # O nome da aba vai ao SSE; nunca derive dos bytes do comando, que podem conter credencial.
    return _shortcut_shell(name, ShortcutShellBody(command=body.command, label="Terminal", key=body.key), request, powershell=True, shell=shell)


def _shortcut_shell(name: str, body: ShortcutShellBody, request: Request, *, powershell: bool, shell: str | None = None):
    # Atalho "shell" da fileira. Cada execucao ganha um terminal escondido proprio
    # (app/shortcut_terminals.py, no tmux e no psmux): a pessoa ve a saida numa aba do painel e
    # fecha quando quiser, e o programa sobrevive a restart do backend. `runs_in="hangar"` cria uma
    # copia unica do servidor, sem dono.
    from app.share_gate import guest_of
    # Convidado nao cria nem reaproveita copia No Hangar: ele nao a ve, nao a fecha, e o reuso traria
    # uma janela pra frente na tela do dono.
    guest_user = guest_users.current.get()
    if body.runs_in == "hangar" and (guest_of(request) is not None or guest_user is not None):
        raise HTTPException(403, detail=erro("erro_shortcut_hangar_convidado",
                                             "convidado nao roda atalho No Hangar"))
    cwd = _session_cwd(name)
    command = body.command.strip()
    if not command:
        raise HTTPException(400, detail=erro("erro_shortcut_vazio", "comando vazio"))
    # No Hangar a pasta e a home, mesmo com `pasta` configurada; com `home` desligado vale a pasta.
    if body.runs_in == "hangar" and body.home:
        cwd = os.path.expanduser("~")
    elif body.pasta is not None:
        try:
            cwd = project_shortcuts.resolve_folder(cwd, body.pasta)
        except project_shortcuts.ProjectError as e:
            raise _project_error(e)
        except ValueError as e:
            raise HTTPException(400, detail=erro("erro_shortcut_pasta", str(e), detalhe=str(e)))
    # `pasta` absoluta passa direto pelo resolve_folder: o convidado não sai da pasta dele.
    if guest_user is not None and not guest_users.inside_root(guest_user, cwd):
        raise HTTPException(403, detail=erro("erro_fora_da_pasta",
                                             "o convidado só abre sessão dentro da pasta dele"))
    # Atalho importado com a credencial em branco: rodar mandaria o marcador literal pro programa.
    from app.shortcut_transfer import has_placeholder
    missing = has_placeholder(command)
    if missing:
        # Na rota nova, 422 fica reservado para comando que abriu terminal e falhou.
        raise HTTPException(400 if powershell else 422, detail=erro("erro_shortcut_segredo",
                                             f"preencha a credencial {missing} antes de usar",
                                             nome=missing))
    if body.runs_in == "hangar":
        return _shortcut_shell_hangar(name, cwd, command, body)
    from app import shortcut_terminals
    term = shortcut_terminals.start(name, cwd, command, body.label or "", _shortcut_display_env(),
                                    key=body.key, ask=body.ask, powershell=powershell, shell=shell)
    if term is None:
        raise HTTPException(500, detail=erro("erro_shortcut_shell", "o multiplexador recusou criar o terminal"))
    # Sem o texto do comando: ele pode carregar credencial.
    _log.info("shortcut-shell: sessao=%s terminal=%s", name, term["tmux"])
    return _shortcut_started(name, term)


def _shortcut_display_env() -> dict[str, str]:
    return {k: v for k, v in _shortcut_env().items() if k in _DISPLAY_VARS}


def _shortcut_started(name: str, term: dict) -> dict:
    """202 quando o processo ainda vive (ou saiu 0) na janela; 422 com o fim da saida se morreu."""
    from app import shortcut_terminals
    public = {"id": term["id"], "label": term["label"]}
    # Quem clicou precisa saber que falhou. Comando que erra (nao existe, sintaxe, VPN fora)
    # morre em segundos; o que ainda roda depois da janela e programa longo e conta como ok.
    limit = time.monotonic() + _SHORTCUT_FAIL_WINDOW
    alive, code = True, None
    while True:
        alive, code = shortcut_terminals.status(term["tmux"])
        if not alive or time.monotonic() >= limit:
            break
        time.sleep(0.1)
    if alive or code == 0:
        return {"ok": True, "terminal": {**public, "alive": alive, "exit_code": code}}
    tail = _shortcut_output_tail(shortcut_terminals.output(term["tmux"]).encode())
    _log.info("shortcut-shell: sessao=%s terminal=%s saiu com %s", name, term["tmux"], code)
    msg = f"o comando saiu com o código {code}" + (f": {tail}" if tail else "")
    # O terminal fica: a aba mostra a saida inteira que o aviso resume.
    raise HTTPException(422, detail=erro("erro_shortcut_falhou", msg, codigo=code, saida=tail,
                                         terminal={**public, "alive": False, "exit_code": code}))


def _focus_hangar_terminal(row: dict | None) -> bool:
    if not row or not row["alive"] or not row["pid"]:
        return False
    from app import window_focus
    return window_focus.focus_tree(row["pid"], _shortcut_env())


def _shortcut_reused(term: dict) -> dict:
    from app import shortcut_terminals
    try:
        focused = _focus_hangar_terminal(shortcut_terminals.hangar_row(term["id"]))
    except shortcut_terminals.MuxUnavailable:
        # O terminal foi reaproveitado; so o foco da janela ficou sem resposta.
        _log.warning("shortcut-shell: multiplexador sem resposta ao focar o terminal %s", term["id"])
        focused = False
    return {"ok": True, "reused": True, "focused": focused,
            "terminal": {"id": term["id"], "label": term["label"], "alive": True, "exit_code": None}}


def _shortcut_mux_unavailable() -> HTTPException:
    # Sem resposta nao da pra saber se a copia ja existe: abrir outra derrubaria a primeira.
    return HTTPException(500, detail=erro("erro_shortcut_mux_indisponivel",
                                          "o multiplexador nao respondeu; tente de novo"))


def _shortcut_shell_hangar(name: str, cwd: str, command: str, body: ShortcutShellBody):
    # Copia unica do servidor: clicar de novo reaproveita em vez de abrir outra (a VM do RDP so
    # aceita uma conexao por usuario, e a segunda derrubava a primeira).
    key = body.key.strip()
    if not key:
        raise HTTPException(400, detail=erro("erro_shortcut_sem_chave", "atalho No Hangar sem chave"))
    from app import shortcut_terminals
    try:
        term, reused = shortcut_terminals.start_hangar(key, cwd, command, body.label or "",
                                                       _shortcut_display_env(), name, body.ask)
    except shortcut_terminals.MuxUnavailable:
        raise _shortcut_mux_unavailable()
    if term is None:
        raise HTTPException(500, detail=erro("erro_shortcut_shell", "o multiplexador recusou criar o terminal"))
    _log.info("shortcut-shell: hangar terminal=%s reaproveitou=%s", term["tmux"], reused)
    if reused:
        return _shortcut_reused(term)
    return {**_shortcut_started(name, term), "reused": False, "focused": False}


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


@app.post("/api/hangar-terminals/{ident}/restart", dependencies=[Depends(require_auth)], status_code=202)
def hangar_terminal_restart(ident: str):
    from app import shortcut_terminals
    try:
        term, reused = shortcut_terminals.restart_hangar(ident, _shortcut_display_env())
    except shortcut_terminals.RestartError:
        raise HTTPException(500, detail=erro("erro_hangar_terminal_rodar_de_novo",
                                             "nao foi possivel recuperar o comando do terminal No Hangar"))
    except shortcut_terminals.MuxUnavailable:
        raise _shortcut_mux_unavailable()
    if term is None:
        raise _hangar_404()
    if reused:
        return _shortcut_reused(term)
    return {**_shortcut_started("hangar", term), "reused": False, "focused": False}


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
    # immutable: o conteudo de um uuid+idx nunca muda -> cache agressivo no cliente.
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


def _json_dict(linha: str) -> dict | None:
    try:
        o = json.loads(linha)
    except (json.JSONDecodeError, ValueError):
        return None
    return o if isinstance(o, dict) else None


# ── Arquivo: conversas mortas (transcripts sem sessao tmux viva) ──────────────
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


@app.get("/api/archive", dependencies=[Depends(require_auth)], response_model=list[ArchiveFolder])
def archive_index():
    # Nivel 1: so as PASTAS (agregado barato). As conversas vem por pasta, no endpoint abaixo.
    return list_folders()


@app.get("/api/archive-por-cwd", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_por_cwd(cwd: str, config_dir: str | None = None, cap: int = 12,
                    provider: str = "claude", codex_account: str | None = None):
    """Conversas retomaveis de UM cwd, do agente e da conta pedidos — o que o modal de sessao nova
    lista embaixo do formulario. Path proprio (nao `/api/archive/{project}`) pra nao disputar a rota
    com um nome de projeto. Pasta sem conversa nenhuma = lista vazia, nao 404: no modal isso e o
    caso comum (pasta nova), nao erro. `cap` baixo porque aqui a lista e um atalho, nao o Arquivo.

    `config_dir` so vale pro Claude — Pi, Kimi e Codex nao tem conta, e passa-lo os excluiria."""
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    try:
        todas = list_conversations(sanitize_cwd(cwd), live, cap=cap,
                                   config_dir=config_dir if provider == "claude" else None,
                                   codex_account=codex_account if provider == "codex" else None,
                                   provider=provider)
    except codex_accounts.AccountError as e:
        raise _erro_conta_codex(e) from None
    except (ValueError, FileNotFoundError):
        return []
    return [e for e in todas if e.provider == provider][:cap]


@app.get("/api/archive/recent", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_recent(cap: int = 40):
    # A lista "Conversas" do celular e do nativo: as vivas vêm da lista de sessões, as fechadas daqui.
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    return list_recent(live, cap=max(1, min(cap, 100)))


@app.get("/api/archive/{project}", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_folder(project: str, codex_account: str | None = None):
    # live = transcripts em uso agora (badge na lista; a conversa viva abre pelo chat normal).
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    try:
        return list_conversations(project, live, codex_account=codex_account)
    except codex_accounts.AccountError as e:
        raise _erro_conta_codex(e) from None
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_projeto_nao_encontrado", "project not found"))


@app.get("/api/archive/{project}/{session_id}/history",
         dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
def archive_history(project: str, session_id: str, tail: int = 0, config_dir: str | None = None,
                    provider: str = "claude", codex_account: str | None = None):
    from app.conversation_history import HistoryError
    # `tail=N` = so as N ultimas mensagens, lidas pelo FIM do arquivo (a previa do modal de sessao
    # nova). Sem ele, o historico inteiro, como sempre — e um transcript de 19MB carregado inteiro
    # so pra mostrar cinco balões era o que essa via evita.
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    try:
        if provider == "codex":
            p = archive_jsonl(project, session_id, config_dir, provider, codex_account)
            composed = archive_providers.transferred_history(p)
            if composed is not None:
                if tail > 0:
                    return [event for event in composed if event.kind in ("user_msg", "assistant_msg") and event.text][-min(tail, 200):]
                return composed
        if tail > 0:
            return tail_events(project, session_id, min(tail, 200), config_dir, provider,
                               codex_account)
        p = archive_jsonl(project, session_id, config_dir, provider, codex_account)
        if provider != "claude":
            # Fora do Claude nao ha fila duravel keyed por este arquivo: o transcript e a conversa
            # inteira, e cada provider tem o parser dele.
            # Linha a linha: rollout de dezenas de MB inteiro na memória, mais a lista das linhas.
            with open(p, encoding="utf-8", errors="replace") as fh:
                return [ev for linha in fh
                        if (o := _json_dict(linha)) is not None
                        for ev in archive_providers.parse_obj(provider, o)]
    except codex_accounts.AccountError as e:
        raise _erro_conta_codex(e) from None
    except HistoryError as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    from app.pqueue import merged_history
    # Nome de fila inexistente -> sem entradas de fila: so os eventos do transcript, ordenados por ts.
    return merged_history("__archive__", str(p))


@app.get("/api/archive/{project}/{session_id}/transcript-image/{uuid}/{idx}",
         dependencies=[Depends(require_auth)])
def archive_image(project: str, session_id: str, uuid: str, idx: int,
                  config_dir: str | None = None, provider: str = "claude",
                  codex_account: str | None = None):
    from app.conversation_history import archive_transfer, historical_image, archived_image, HistoryError
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    try:
        if uuid.startswith("transfer:"):
            got = archived_image(session_id, uuid, idx, codex_account)
        else:
            p = archive_jsonl(project, session_id, config_dir, provider, codex_account)
            record = archive_transfer(p) if provider == "codex" else None
            if record:
                got = historical_image(record, uuid, idx)
            else:
                from app.transcript import get_transcript_image
                got = get_transcript_image(str(p), uuid, idx)
    except codex_accounts.AccountError as exc:
        raise _erro_conta_codex(exc) from None
    except HistoryError as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    except (ValueError, FileNotFoundError):
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "not found"))
    if got is None:
        raise HTTPException(404, detail=erro("erro_imagem_nao_encontrada", "image not found"))
    raw, media = got
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


class ResumeArchivedBody(_StrictBody):
    # Motor de modelo pro resume do Arquivo. O pane que rodava a sessao original ja morreu -> nao ha
    # /proc pra descobrir o motor de entao (ver registry._engine_of); quem retoma escolhe de novo.
    # Sem escolha, volta na conta Anthropic (comportamento de hoje).
    engine: str | None = None
    engine_account: str | None = None
    model: str | None = None
    # A CONTA em que a conversa continua. Omitida, e a dona do transcript (descoberta no disco). Outra
    # conta: o transcript MUDA de conta antes do `--resume`, porque rodado na conta errada ele morre
    # na hora com "No conversation found with session ID".
    config_dir: str | None = None
    # Agente dono da conversa. Pi e Kimi retomam com o comando DELES (`pi --session-id`,
    # `kimi --session`); Codex nao tem via de resume aqui e e recusado logo abaixo.
    provider: str = "claude"
    codex_account: str | None = None


def _sessao_com_transcript(jsonl: Path) -> str | None:
    """Nome da sessao viva que escreve neste transcript, ou None."""
    alvo = os.path.realpath(str(jsonl))
    for s in registry.list():
        if s.jsonl and os.path.realpath(s.jsonl) == alvo:
            return s.name
    return None


@app.post("/api/archive/{project}/{session_id}/resume", dependencies=[Depends(require_auth)],
          response_model=SessionInfo)
def resume_archived(project: str, session_id: str, body: ResumeArchivedBody = ResumeArchivedBody()):
    from app.conversation_history import archive_transfer, source_rows, verify_boundary, HistoryError
    # "Retomar conversa" do Arquivo: sobe uma sessao tmux NOVA no cwd original com `claude --resume
    # <uuid>` -- reusa registry.create (nome/config_dir/spawn tmux ja tratados), so troca o comando pro
    # uuid EXISTENTE (nao um novo transcript). Nome derivado do basename do cwd, igual ao
    # CreateSessionSheet do front; colisao suffixa -2/-3... (mesmo esquema, do lado do backend pq aqui
    # nao ha form pro usuario escolher nome).
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if body.provider != "claude" and body.provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    if body.codex_account is not None and body.provider != "codex":
        raise HTTPException(400, detail=erro("codex_account_so_codex",
                                             "codex_account só vale para provider codex"))
    # O id da conversa Codex e o uuid do FIM do nome do rollout, e e ele que o `codex resume` recebe.
    # Um nome fora desse padrao nao tem id pra retomar — e dizer "caminho invalido" (o que o
    # ValueError generico daqui a pouco daria) manda procurar defeito no lugar errado.
    if body.provider == "codex" and not archive_providers.UUID_RE.match(session_id):
        raise HTTPException(400, detail=erro("erro_rollout_sem_id",
                                             "nome de rollout sem id de conversa"))
    # Conta omitida (link antigo, chamador que nao sabe): descobre no disco. Deixar None aqui subia
    # o pane na conta do backend e o `--resume` morria com "No conversation found with session ID".
    # Conversa que nao existe nao vira erro AQUI: o archive_cwd logo abaixo faz a mesma busca e e
    # ele quem devolve o 400/404 -- duplicar a recusa so daria duas mensagens pro mesmo caso.
    # So o Claude tem conta: Pi e Kimi guardam transcript fora do config dir.
    cfg = body.config_dir
    # Conta pedida diferente da dona: a conversa MUDA de conta, mas so no fim, depois de toda
    # recusa possivel -- mover e depois negar deixaria a conversa fora do lugar por um 409.
    # `mover` guarda a conta de origem (o "None" da conta do processo tambem e origem valida).
    mover: tuple[str | None] | None = None
    if body.provider == "claude":
        try:
            dona = conta_de(project, session_id)
            if cfg is None:
                cfg = dona
            elif dona != cfg:
                # Conversa ABERTA nao muda de conta: o processo dela ainda escreve no arquivo, e o
                # rename deixaria ele gravando num inode que a lista nao acha mais.
                viva = _sessao_com_transcript(archive_jsonl(project, session_id, dona))
                if viva:
                    raise HTTPException(409, detail=erro("erro_conversa_viva",
                                                         "conversa aberta nao muda de conta",
                                                         sessao=viva))
                mover = (dona,)
        except (ValueError, FileNotFoundError):
            pass
    origem_codex_account = body.codex_account
    try:
        if body.provider == "codex":
            origem_path = archive_jsonl(project, session_id, cfg, body.provider,
                                        body.codex_account)
            owner = codex_accounts.account_for_rollout(origem_path)
            if owner is None:
                raise FileNotFoundError(session_id)
            if origem_codex_account is not None and owner.id != origem_codex_account:
                raise codex_accounts.AccountError(
                    409, "codex_account_archive_mismatch",
                    {"account_id": origem_codex_account, "origin_account": owner.id},
                )
            origem_codex_account = owner.id
            transfer = archive_transfer(origem_path)
            if transfer:
                source_rows(transfer.source)
                verify_boundary(transfer, origem_path)
            cwd = archive_cwd(project, session_id, cfg, body.provider, origem_codex_account)
        else:
            cwd = archive_cwd(project, session_id, mover[0] if mover else cfg, body.provider)
    except codex_accounts.AccountError as e:
        raise _erro_conta_codex(e) from None
    except HistoryError as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    if not cwd:
        raise HTTPException(422, detail=erro("erro_cwd_ausente", "cwd not found in transcript"))
    # Mesmo transcript ja aberto numa sessao: um segundo `--resume` poria dois processos gravando
    # no mesmo arquivo. Antes do move e de qualquer spawn.
    try:
        viva = _sessao_com_transcript(archive_jsonl(
            project, session_id, mover[0] if mover else cfg, body.provider,
            origem_codex_account if body.provider == "codex" else None))
    except (ValueError, FileNotFoundError):
        viva = None
    if viva:
        raise HTTPException(409, detail=erro("erro_conversa_viva", "conversa já está aberta",
                                             sessao=viva))
    if body.engine is not None and body.engine not in engines.listar():
        raise HTTPException(400, detail=erro("erro_motor_invalido", "motor invalido"))
    fixed_model = body.model
    if body.engine_account is not None:
        if body.provider != "claude" or not body.engine:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", "conta ChatGPT exige Claude com motor CLIProxyAPI local"))
        account = _fixed_engine_account(body.engine, body.engine_account)
        cfg_engine = engines.listar()[body.engine]
        from app.cliproxy_accounts import base_model
        try:
            fixed_model = base_model(body.model or cfg_engine["model"], account["prefix"])
            account_models = engine_probe.listar_modelos(cfg_engine["base_url"], cfg_engine["api_key"])
            models = cliproxy.validate_models(cfg_engine, fixed_model, account, account_models)
        except ValueError as exc:
            raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None
        except RuntimeError:
            raise HTTPException(502, detail=erro("erro_cliproxy_conta", "catálogo do CLIProxyAPI indisponível")) from None
        catalog_id = engines.catalog_model(fixed_model)
        if not any(m["id"] == catalog_id for m in models):
            raise HTTPException(422, detail=erro("erro_modelo_fora_catalogo", "modelo indisponível nesta conta ChatGPT"))
    elif body.model is not None:
        try:
            model_args.validar(body.provider, body.model, None)
        except ValueError as exc:
            raise HTTPException(400, str(exc)) from None
    base = sanitize_session_name(Path(cwd).name) or "sessao"
    name, i = base, 2
    # As MESMAS fontes que a criacao normal consulta (registry.create). Olhando so o tmux, um nome
    # ja usado por uma sessao Codex ou sem terminal passava por aqui e o conflito estourava la
    # dentro, como um 409 com a mensagem de outro assunto.
    while _nome_ocupado(name):
        name = f"{base}-{i}"
        i += 1
    if mover:
        try:
            move_conversation(project, session_id, cfg)
        except FileExistsError:
            raise HTTPException(409, detail=erro("erro_conversa_ja_na_conta",
                                                 "a conta destino ja tem esta conversa"))
        except OSError as e:
            _log.exception("mover conversa %s para %s falhou", session_id, cfg)
            raise HTTPException(500, detail=erro("erro_mover_conversa",
                                                 f"nao consegui mover a conversa de conta: {e}",
                                                 erro=str(e)))
    try:
        extras = {"codex_account": origem_codex_account} \
            if body.provider == "codex" and origem_codex_account is not None else {}
        if body.engine_account is not None:
            extras["engine_account"] = body.engine_account
            extras["engine_models"] = account_models
        if fixed_model is not None:
            extras["model"] = fixed_model
        if body.provider == "codex" and transfer:
            extras.update(transfer_id=transfer.id, transfer_rollout_path=str(origem_path),
                          tool_output_token_limit=transfer.destination_meta["tool_output_token_limit"])
        info = registry.create(name, cwd, config_dir=cfg, provider=body.provider,
                               resume_session_id=session_id, engine=body.engine, **extras)
        _invalidate_lists()
        return info
    except ValueError as e:
        if mover:
            # Sessao nao nasceu: a conversa volta pra conta de origem. Falha aqui nao pode
            # esconder o 409 original, mas tambem nao pode passar calada.
            try:
                move_conversation(project, session_id, mover[0])
            except OSError:
                _log.exception("conversa %s ficou em %s: rollback do move falhou", session_id, cfg)
        raise HTTPException(409, str(e))


# ── MCP hangar-computer-control: liga/desliga e configura nos .claude.json de todas as contas ──
class ComputerControlBody(_StrictBody):
    enabled: bool
    mode: Literal["package", "local"] | None = None   # None = mantém o modo atual
    project_dir: str = ""
    agent_config: str = ""
    llm_url: str = ""
    llm_model: str = ""
    llm_effort: str = ""
    llm_key: str | None = None     # None/vazio = mantém a gravada
    jev_key: str | None = None
    use_cliproxy_key: bool = False


class ComputerControlModelsBody(_StrictBody):
    llm_url: str
    llm_key: str | None = None
    use_saved_key: bool = False
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


@app.post("/api/computer-control/models", dependencies=[Depends(require_auth)])
def computer_control_models(body: ComputerControlModelsBody):
    from app import computer_control as cc
    return {"models": _computer_control_call(cc.list_models, body.llm_url, body.llm_key,
                                             body.use_saved_key, body.use_cliproxy_key)}


class ConnectBody(_StrictBody):
    code: str = Field(min_length=1, max_length=4096)


@app.get("/api/connect", dependencies=[Depends(require_auth)])
def connect_get():
    from app import connect
    return connect.status()


@app.put("/api/connect", dependencies=[Depends(require_auth)])
async def connect_put(body: ConnectBody):
    from app import connect
    try:
        connect.save(body.code)
    except connect.ConnectError as e:
        raise HTTPException(e.status, detail=erro(e.code, e.msg)) from None
    await connect.start()
    return connect.status()


@app.delete("/api/connect", dependencies=[Depends(require_auth)])
async def connect_delete():
    from app import connect
    await connect.forget()
    return connect.status()


# ── Busca de conteudo cross-session: grep (rg) em todos os transcripts (vivos + arquivados) ──
@app.get("/api/search", dependencies=[Depends(require_auth)], response_model=list[SearchHit])
def search_transcripts(q: str = ""):
    # live: realpath(jsonl) -> nome tmux das sessoes VIVAS (mesmo join do /api/archive). A busca marca
    # o hit como vivo e carrega o nome pra a UI abrir o chat (viva) ou o arquivo (morta). q vazia -> [].
    live = {os.path.realpath(s.jsonl): s.name for s in registry.list() if s.jsonl}
    return search(q, live)


@app.get("/api/search/context", dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
def search_context(project: str, session_id: str, event_id: str, around: int = 3):
    """Mensagens em volta de um trecho da busca: ele e as `around` anteriores e posteriores, só as
    suas e as do assistente (é o que se lê pra entender o trecho)."""
    try:
        p = archive_jsonl(project, session_id)
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    from app.pqueue import merged_history
    msgs = [ev for ev in merged_history("__archive__", str(p))
            if ev.kind in ("user_msg", "assistant_msg") and ev.text]
    i = next((k for k, ev in enumerate(msgs) if ev.id == event_id), None)
    if i is None:
        raise HTTPException(404, detail=erro("erro_trecho_nao_encontrado",
                                             "a mensagem não está mais nessa conversa"))
    n = max(0, min(around, 10))
    return msgs[max(0, i - n):i + n + 1]


class AskHistoryBody(_StrictBody):
    question: str = Field(min_length=1, max_length=500)


@app.post("/api/ask-history", dependencies=[Depends(require_auth)])
def ask_history(body: AskHistoryBody):
    """RAG lexical ("onde falei sobre X"): extrai termos da pergunta -> busca OR nos transcripts ->
    claude -p resume EM QUAL sessao o assunto apareceu. Sob o kill-switch (dispara claude -p). Sem
    trecho -> resposta vazia sem chamar o CLI. v1: so o servidor que recebe a chamada (cross-server v2)."""
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    live = {os.path.realpath(s.jsonl): s.name for s in registry.list() if s.jsonl}
    hits = search_terms(extract_terms(body.question), live)
    if not hits:
        return {"answer": "não achei nada sobre isso nas conversas", "hits": []}
    try:
        answer = loop_mod._claude_p(build_ask_prompt(body.question, hits))
    except loop_mod.ClaudePError as e:
        _log.warning("ask-history falhou: %s", e)
        raise HTTPException(502, str(e))
    return {"answer": answer, "hits": hits}


# Anexo citado na conversa: 60s sem perguntar + ETag pro resto. A miniatura de 96px do
# FileAttachment carrega o arquivo ORIGINAL, entao sem cache toda repintura da lista rebaixava o
# PNG inteiro. Starlette 1.3.1 poe o ETag no FileResponse mas NAO responde 304 — o 304 abaixo e
# nosso. ponytail: arquivo reescrito no MESMO caminho so aparece depois dos 60s; e o preco de nao
# perguntar. Documento (html/pdf) que o agente regenera e o caso que mais sente isso.
_CACHE_ARQUIVO = "max-age=60"


def _cited_elsewhere(jsonl: str, cwd: str | None, path: str, worked: list[str], *,
                     siblings: bool, rows=None) -> str | None:
    """Arquivo de um nome solto ou relativo que não está na pasta da sessão: o absoluto que a conversa citou antes,
    ou um relativo citado (`docs/x/nome`) dentro das pastas onde a conversa trabalhou (`worked`, o cwd das linhas
    que o citaram) e da pasta da sessão. `siblings` também tenta as pastas ao lado da sessão (outro repositório,
    como num `cd ../outro && git status`): só para LER, porque ali o mesmo relativo pode ser de outro projeto."""
    from app.transcript import cited_elsewhere
    absolutes, cited_relatives = cited_elsewhere(jsonl, path, rows=rows)
    if absolutes:
        return absolutes[0]
    if not cwd:
        return None
    rel = path.replace("\\", "/").removeprefix("./")
    if ".." in rel.split("/"):
        return None
    relatives = [rel] if "/" in rel else cited_relatives
    base = os.path.realpath(cwd)
    folders = list(dict.fromkeys([*(os.path.realpath(w) for w in worked), base]))
    if siblings:
        parent = os.path.dirname(base)
        try:
            # ponytail: só o primeiro nível ao lado da sessão, e no máximo 200 pastas.
            folders += sorted(e.path for e in os.scandir(parent) if e.is_dir() and e.path not in folders)[:200]
        except OSError:
            _log.warning("pastas ao lado de %s ilegíveis ao procurar %s citado", base, path, exc_info=True)
    for relative in relatives:
        for folder in folders:
            candidate = os.path.realpath(os.path.join(folder, relative))
            if candidate.startswith(folder + os.sep) and os.path.isfile(candidate):
                return candidate
    return None


def _conversation_rows(info):
    from app.conversation_history import citation_rows, HistoryError
    try:
        return citation_rows(info)
    except (HistoryError, OSError) as exc:
        raise HTTPException(409, detail=erro("session_transfer_history_invalid", "histórico da transferência indisponível")) from exc


from app.workspace_bridge import delegate as _workspace_delegate, text_rows as _text_rows
_cited_elsewhere = _workspace_delegate("find_elsewhere", GitError, prepare=_text_rows)(_cited_elsewhere)


def _resolver_citado(name: str, path: str, *, write: bool = False) -> str:
    """Devolve o caminho REAL de um arquivo citado no transcript desta sessao.

    TRAVA de seguranca compartilhada por quem le e por quem grava fora da raiz da sessao: so
    resolve se o `path` aparece no transcript (citado por voce ou pelo agente = consentido) E
    existe E e arquivo regular -> bloqueia leitura/escrita arbitraria de disco e path-traversal.
    Path RELATIVO (ex "./mock.png", "sub/x.png") resolve contra o CWD DA SESSAO (onde o agente
    criou o arquivo), nao o cwd do processo backend; guard extra: o resolvido nao pode ESCAPAR
    do cwd.
    """
    info = _cached_info_sync(name)
    if info is None or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app import workspace_bridge
    rows = _conversation_rows(info)
    # Sessão Codex transferida: a conversa inclui o histórico de antes da troca, que o Rust não lê.
    if rows is None and (info.cwd or os.path.isabs(os.path.expanduser(path))):
        result = workspace_bridge.request("resolve_cited", {"cwd": info.cwd or "", "jsonl": info.jsonl,
            "path": path, "write": write})
        if result is not None:
            if result["ok"]:
                return result["result"]
            failure = result["error"]
            detail = failure["detail"]
            if failure.get("code"):
                detail = erro(failure["code"], str(detail), motivo=str(detail))
            raise HTTPException(failure["status"], detail=detail)
    from app.transcript import citation_cwds
    cited = citation_cwds(info.jsonl, [path], rows=rows)
    if path not in cited:
        raise HTTPException(403, detail=erro("erro_arquivo_nao_citado", "file not referenced in this conversation"))
    expanded = os.path.expanduser(path)
    if os.path.isabs(expanded):
        real = os.path.realpath(expanded)
    else:
        if not info.cwd:
            raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
        if ".." in path.replace("\\", "/").split("/"):
            raise HTTPException(403, detail=erro("erro_caminho_fora_cwd", "path escapes session cwd"))
        bases = list(dict.fromkeys([*cited[path], info.cwd]))
        real = ""
        for raw_base in bases:
            base = os.path.realpath(raw_base)
            candidate = os.path.realpath(os.path.join(base, expanded))
            if candidate != base and not candidate.startswith(base + os.sep):
                continue
            if os.path.isfile(candidate):
                real = candidate
                break
        if not real:
            whole = _cited_elsewhere(info.jsonl, info.cwd, path, cited[path], siblings=not write, rows=rows)
            real = os.path.realpath(whole) if whole else ""
        if not real:
            raise HTTPException(404, detail=erro("erro_arquivo_nao_encontrado", "file not found"))
    if not os.path.isfile(real):
        raise HTTPException(404, detail=erro("erro_arquivo_nao_encontrado", "file not found"))
    # Mesma regra do filetree para a arvore da sessao: nenhum caminho que passe por uma pasta
    # `.git` e servido. Comparada sobre o realpath, entao `atalho -> .git` tambem nao escapa.
    if ".git" in Path(real).parts:
        raise HTTPException(403, detail=erro("erro_arq_area_do_git", "area interna do git"))
    return real


@app.get("/api/sessions/{name}/file", dependencies=[Depends(require_auth)])
def serve_file(name: str, path: str, request: Request, download: bool = False):
    # FileResponse trata Range -> <video> faz seek/streaming.
    real = _resolver_citado(name, path)
    st = os.stat(real)
    representation = "download" if download else "isolated"
    etag = f'"{representation}-{st.st_mtime_ns:x}-{st.st_size:x}"'
    cabecalhos = {"etag": etag, "cache-control": _CACHE_ARQUIVO}
    # Depois da trava do transcript, nunca antes: 304 e resposta sobre um arquivo, e quem nao pode
    # ver o arquivo tambem nao pode saber que ele mudou.
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers=cabecalhos)
    return file_response(real, headers=cabecalhos, download=download)


# Arquivo CITADO na conversa, como texto editavel. O par com `/files/read` e `/files/write` da
# arvore: mesma mecanica do filetree (teto, binario, digest, escrita atomica), outra politica de
# caminho — a raiz da sessao la, a citacao no transcript aqui.
@app.get("/api/sessions/{name}/file/text", dependencies=[Depends(require_auth)])
def serve_file_text(name: str, path: str):
    try:
        return filetree.read_at(Path(_resolver_citado(name, path)), path)
    except FileError as e:
        raise _erro_arq(e)


@app.post("/api/sessions/{name}/file/text", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
def write_file_text(name: str, body: FileWriteBody):
    try:
        return filetree.write_at(
            Path(_resolver_citado(name, body.path, write=True)), body.path, body.text, body.digest
        )
    except FileError as e:
        raise _erro_arq(e)


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


def _pi_answer_fallback_text(a: dict) -> str:
    """Resposta em TEXTO pro fallback da pergunta do Pi (drive do picker falhou). Mesma filosofia
    do _askq_fallback_text do Claude: a resposta do usuario NUNCA se perde — vira mensagem normal."""
    if a.get("kind") == "option":
        resp = ", ".join(a.get("labels") or [])
    elif a.get("kind") == "text":
        resp = a.get("value") or ""
    else:
        resp = ""
    if not resp:
        return ""
    # Texto NEUTRO. Dizia "o seletor de opções falhou" — e no Kimi isso era mentira ate hoje: la nao
    # havia drive de teclas, o texto era o caminho NORMAL e unico. O usuario lia "falhou" na propria
    # conversa e achava que a resposta tinha dado errado (relatado em 13/08/2026), e o agente lia a
    # mesma frase e respondia ao fantasma.
    #
    # Quem soube que houve fallback foi o LOG do servidor. O `fallback: true` volta no corpo da
    # resposta, mas o front descarta (`api.ts answerQuestions` tipa so `{ok}`) — entao nao prometa
    # aqui que o usuario ve isso. O que ele ve e a propria resposta virando mensagem no chat, que ja
    # diz "foi por texto" sem precisar de aviso.
    return f"Respondendo à pergunta: {resp}"


class SkipQuestionBody(_StrictBody):
    request_id: str


@app.post("/api/sessions/{name}/question/skip", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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
        pending = plugin_bridge.pergunta_pendente(name)
        if pending is None:
            _recusa_se_painel_aberto(name)
        try:
            result = answer_sync(name, answers, body.request_id or (pending or {}).get("id"),
                getattr(info, "jsonl", None))
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
    # Function hook do plugin segurando a pergunta: a resposta entra sem tecla. Se ele não pegar
    # (hook morto, pergunta já fechada no terminal), o caminho de tecla abaixo assume.
    if getattr(info, "provider", "claude") == "claude":
        pend = plugin_bridge.pergunta_pendente(name)
        # Só PERGUNTA: o mesmo canal segura pedido de permissão (`perm:`), que é do /select.
        if pend is not None and pend["questions"] and not str(pend["id"]).startswith("perm:"):
            from app.adapters.claude_headless.adapter import _mensagem_conversar, respostas_do_app
            try:
                respostas, conversar = respostas_do_app(pend["questions"], answers)
            except ValueError as exc:
                raise HTTPException(409, detail=erro("erro_sem_resposta", str(exc))) from exc
            corpo = ({"deny": _mensagem_conversar(respostas, conversar)} if conversar
                     else {"answers": respostas})
            if plugin_bridge.responder_pergunta(name, corpo, pend["id"]):
                _log.info("ANSWER name=%s pelo plugin (sem tecla)", name)
                if info and info.jsonl:
                    clear_pending_askq(info.jsonl)
                return {"ok": True, "fallback": False}
            _log.warning("ANSWER name=%s: plugin não pegou a resposta — indo pela tecla", name)
    _recusa_se_painel_aberto(name)
    jsonl = info.jsonl if info else None
    fallback = False

    # Pi: a pergunta nativa (tool `question`) mora no proprio transcript — o front sintetiza o
    # payload do AskUserQuestion a partir do tool_use pendente e posta aqui igual; o drive e outro
    # (picker ascii do Pi, sem tela de Review). A pergunta some da fila (respondida no terminal)
    # entre o card abrir e o toque -> 409 legivel, nunca drive as cegas. O omp e o mesmo caminho
    # com outra tool (`ask`, uma lista de perguntas que o parser normaliza) e outro cursor.
    if getattr(info, "provider", "claude") in ("pi", "omp"):
        from app.adapters.pi.transcript import read_pending_question
        tool = "ask" if info.provider == "omp" else "question"
        q = read_pending_question(jsonl, tool=tool) if jsonl else None
        if q is None:
            raise HTTPException(409, detail=erro("erro_sem_pergunta_pi", "nenhuma pergunta do Pi pendente (ja respondida no terminal?)"))
        if not answers:
            raise HTTPException(409, detail=erro("erro_sem_resposta", "sem resposta"))
        try:
            terminal_input.answer_question_pi(name, answers[0], q, provider=info.provider)
        except ValueError as e:
            raise HTTPException(409, str(e))
        except terminal_input.DriveError as e:
            text = _pi_answer_fallback_text(answers[0])
            _log.warning("PI-QUESTION fallback name=%s reason=%s text=%r", name, e, text[:120])
            if not text:
                # Sem texto de fallback, NAO manda o Escape: picker aberto = usuario ainda responde
                # no terminal. Fechar e devolver ok sem entregar nada seria a pior saida (silencio).
                raise HTTPException(409, detail=erro("erro_drive_sem_fallback", f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
            terminal.interrupt(name)  # Escape unico: fecha o picker do Pi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
            _recusa_se_so_enfileirou(name, res)
            fallback = True
        return {"ok": True, "fallback": fallback}

    # Kimi: a pergunta nativa (tool AskUserQuestion) mora no wire — o front sintetiza o card a
    # partir do tool_use pendente, igual ao Pi. O drive do picker foi medido em 13/08/2026 (Kimi
    # 0.36.0) e e mais confiavel que o dos outros dois: as opcoes sao numeradas e a tecla numerica
    # escolhe e avanca (sem contar linha), e a CONFIRMACAO nao e visual — o `tool.result` daquele
    # toolCallId aparecendo no wire prova que a ferramenta recebeu. Drive falhou -> Escape +
    # fallback por texto, igual Claude/Pi: a resposta do usuario nunca se perde.
    if getattr(info, "provider", "claude") == "kimi":
        from app.adapters.kimi.transcript import read_pending_call, resposta_chegou
        pend = read_pending_call(jsonl) if jsonl else None
        if pend is None:
            raise HTTPException(409, detail=erro("erro_sem_pergunta_kimi", "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
        if not answers:
            raise HTTPException(409, detail=erro("erro_sem_resposta", "sem resposta"))
        call_id, args = pend
        perguntas = args.get("questions") if isinstance(args.get("questions"), list) else []
        try:
            terminal_input.answer_question_kimi(name, answers, perguntas)
            # PROVA no transcript, nao no pane: o Kimi so escreve o tool.result depois de a
            # ferramenta receber as respostas. Sem esta checagem, um Submit que nao pegou voltaria
            # 200 com cara de sucesso — o mesmo "sent sem chegar" que ja custou uma resposta perdida.
            if not _espera_resposta_kimi(jsonl, call_id):
                # Prazo estourado NAO prova que nada foi submetido — pode ser o Kimi demorando pra
                # gravar. Os outros dois drivers so levantam DriveError com prova estrutural (o
                # picker AINDA na tela), e aqui vale o mesmo: se o picker sumiu, alguem submeteu.
                # Cair no fallback nesse caso mandaria Escape num turno que ja processa a resposta
                # certa e entregaria a mesma resposta DUAS vezes — uma pela ferramenta, outra como
                # mensagem. Entre duplicar calado e admitir a duvida, admite-se a duvida.
                if terminal_input.picker_kimi_aberto(name):
                    raise terminal_input.DriveError(
                        "Submit nao pegou: o picker continua aberto e o tool.result nao apareceu")
                raise HTTPException(409, detail=erro("erro_sem_confirmacao_resposta",
                                             "resposta enviada, mas nao deu pra confirmar a tempo — "
                                             "confira na sessao antes de responder de novo"))
        except ValueError as e:
            raise HTTPException(409, str(e))
        except terminal_input.DriveError as e:
            text = _pi_answer_fallback_text(answers[0])
            _log.warning("KIMI-QUESTION fallback name=%s reason=%s text=%r", name, e, text[:120])
            if not text:
                # Sem texto de fallback, NAO manda o Escape: picker aberto = o usuario ainda pode
                # responder no terminal. Fechar e devolver ok sem entregar nada seria a pior saida.
                raise HTTPException(409, detail=erro("erro_drive_sem_fallback", f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
            terminal.interrupt(name)  # Escape unico: fecha o picker do Kimi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
            _recusa_se_so_enfileirou(name, res)
            return {"ok": True, "fallback": True}
        return {"ok": True, "fallback": False}
    # "Conversar sobre isso" junto de perguntas já respondidas: dirigir o picker perde as respostas
    # — a TUI cancela tudo ao chegar nessa opção. Fecha por Escape e entrega o que ele escolheu como
    # texto, o mesmo caminho do plano B do drive.
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
            # Sem texto de fallback (resposta `chat`, ou rotulos vazios) nao ha o que entregar. Nao
            # manda o Escape e nao limpa o sidecar: o picker segue aberto pra quem responder no
            # terminal. Ate 01/09/2026 este ramo caia em `fallback = True` e apagava o sidecar como
            # se tivesse respondido, sem uma tecla ter saido — os ramos Pi e Kimi ja barravam.
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


@app.post("/api/sessions/{name}/model-effort", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def model_effort(name: str, body: ModelEffortBody):
    info = await _cached_info(name)
    if info and info.engine_account:
        if body.scope != "session":
            raise HTTPException(409, detail=erro("erro_cliproxy_modelo_rota",
                                                 "a conta ChatGPT fixa não altera o padrão global de modelo ou esforço"))
        if body.model is not None:
            await engine_model_set(name, EngineModelBody(model=body.model, effort=body.effort))
        elif body.effort is not None:
            await _durante_troca(name, _trocar_conta(name, None, engine_account=info.engine_account,
                                                   effort=body.effort))
        return {"ok": True, "scope": "session", "result": None}
    if _headless(name):
        if _loop_servidor is None or not _loop_servidor.is_running():
            raise HTTPException(503, detail=erro("erro_modelo_indisponivel", "servidor sem loop pra aplicar"))
        try:
            model_args.validar("claude", body.model, body.effort, None)
        except ValueError as e:
            raise HTTPException(422, str(e))
        try:
            await asyncio.wait_for(get_adapter(CLAUDE_HEADLESS).set_model(name, body.model, body.effort), 40)
        except Exception as e:
            raise HTTPException(409, detail=erro("erro_modelo_indisponivel", f"não consegui trocar: {e}"))
        return {"ok": True, "scope": "session", "result": None}
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

@app.get("/api/sessions/{name}/permission-modes", dependencies=[Depends(require_auth)])
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

@app.post("/api/sessions/{name}/permission-mode", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
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
            return await _bypass_reopen(name, info)
        vivo = hl._sessions.get(name)
        await asyncio.to_thread(_invalidate_lists)
        from app.runtime_adapter import runtime_data
        view = runtime_data(name)
        return {"mode": ficou, "current": ficou,
                "previous_non_plan":view.get("previous_non_plan") if view is not None else vivo.modo_nao_plan if vivo else None}
    await asyncio.to_thread(_guard_perm, name, info)
    if alvo == "bypassPermissions" and not await asyncio.to_thread(_bypass_no_ciclo, name):
        return await _bypass_reopen(name, info)
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


async def _bypass_reopen(name: str, info):
    # Com o Rust dono, dentro da barreira a sessão já foi fechada e a vista dele (trabalhando,
    # pergunta pendente) não é mais lida: a ociosidade se confere antes de fechar.
    motivo = await _motivo_ocupada(name, _headless(name))
    if motivo:
        raise HTTPException(409, detail=erro(motivo, "para entrar em bypass a sessão reinicia: " + _OCUPADA[motivo]))
    return await _durante_troca(name, _reabrir_em_bypass(name, info))


async def _reabrir_em_bypass(name: str, info):
    """Reabre a mesma conversa (`--resume`) já em bypass, como a troca de conta. Só ociosa."""
    hl = get_adapter(CLAUDE_HEADLESS)
    async with hl.delivery_lock(name):
        # Uma troca que esperava na trava pode ter mudado o transporte da sessão.
        await asyncio.to_thread(_invalidate_lists)
        headless = _headless(name)
        motivo = await _motivo_ocupada(name, headless)
        if motivo:
            raise HTTPException(409, detail=erro(motivo, "para entrar em bypass a sessão reinicia: " + _OCUPADA[motivo]))
        if headless:
            antes = headless_sessions.load(name) or {}
            await asyncio.to_thread(_requeue_unanswered, name)
            await hl.parar(name)
            if headless_sessions.update(name, permission_mode="bypassPermissions",
                                        previous_non_plan="bypassPermissions") is None:
                hl.acordar(name)
                raise HTTPException(409, detail=erro("erro_permissao_reabrir", "não gravei o modo novo no arquivo de estado da sessão"))
            hl.reset_start_attempts(name)
            try:
                await hl.ensure_running(name, require_initialize=True)
            except Exception as e:
                _log.exception("permissão: %s não reabriu em bypass; voltando ao modo de antes", name)
                # O processo pode ter subido em bypass e seguir vivo (initialize recusado): sem
                # parar, ele continua em bypass com o arquivo dizendo outro modo.
                try:
                    await hl.parar(name)
                except Exception as stop_error:
                    # Processo talvez vivo em bypass: o arquivo segue dizendo bypass, não o modo de antes.
                    _log.exception("permissão: %s não parou depois de falhar em bypass", name)
                    raise HTTPException(409, detail=erro("erro_permissao_reabrir",
                        f"a sessão não reabriu em bypass ({e}) e não parou ({stop_error}); ela pode seguir em bypass",
                        erro=str(e), stop_error=str(stop_error))) from e
                headless_sessions.update(name, permission_mode=antes.get("permission_mode"),
                                         previous_non_plan=antes.get("previous_non_plan"))
                hl.acordar(name)
                raise HTTPException(409, detail=erro("erro_permissao_reabrir", f"a sessão não reabriu em bypass e voltou ao modo de antes: {e}", erro=str(e)))
        else:
            try:
                await asyncio.to_thread(registry.para_headless, name, "bypassPermissions")
            except KillFailed as e:
                raise HTTPException(500, str(e))
            except (ValueError, OSError) as e:
                raise HTTPException(409, detail=erro("erro_permissao_reabrir", f"não reabri em bypass: {e}", erro=str(e)))
            try:
                await asyncio.to_thread(registry.para_terminal, name)
            except Exception as e:
                _log.exception("permissão: terminal de %s não voltou após reabrir em bypass", name)
                if headless_sessions.exists(name):
                    hl.acordar(name)
                    msg = f"o terminal não voltou; a sessão seguiu sem terminal, em bypass: {e}"
                else:
                    msg = f"o terminal não voltou e a sessão ficou sem processo: {e}"
                raise HTTPException(409, detail=erro("erro_permissao_reabrir", msg, erro=str(e)))
    _perm_modes_cache.pop(_cache_key_perm(name, info), None)
    return {"mode": "bypassPermissions", "current": "bypassPermissions",
            "previous_non_plan": "bypassPermissions", "reopened": True}


# ── Catalogo de modelos de uma sessao Claude Code ───────────────────────────────────────────────
# Duas fontes, escolhidas pelo que a sessao E, porque medimos que so uma funciona em cada caso:
#   * sessao de MOTOR -> o /v1/models do provedor (o mesmo probe da tela de Motores). O picker do
#     Claude Code ali lista so os 4 aliases, todos apontando pro mesmo ANTHROPIC_MODEL — inutil.
#   * sessao da CONTA -> as linhas do proprio picker, lidas ao vivo. A lista muda com a conta e com
#     a versao do CC (o Fable entrou e a lista chumbada no front nao soube), entao ela nao pode
#     morar no codigo.

class EngineModelBody(_StrictBody):
    model: str
    effort: str | None = None


# Catalogo por motor: e uma chamada de REDE ao provedor, e abrir a folha nao pode pagar isso toda
# vez. TTL curto porque a lista muda com o plano do usuario, nao a cada minuto.
_ENGINE_MODELS_TTL = 300.0
_engine_models_cache: dict[str, tuple[float, list[dict]]] = {}

# Catalogo da conta Anthropic: ler o picker DIRIGE O TERMINAL, e isso deixa RASTRO — o `❯ /model` e
# o `⎿ Kept model as …` (o Esc de saida) ficam no scrollback do tmux pra sempre. Nao aparece no chat
# do app (entra no jsonl como `type: system`, que o transcript ignora), mas aparece pra quem estiver
# com aquele terminal aberto: foi o que pareceu bug quando 5 leituras seguidas empilharam ali.
# Trinta dias, porque a lista muda quando a Anthropic lanca modelo ou o plano do usuario muda —
# eventos de meses, nao de horas; e cache vencido cai na lista reduzida, que e pior que uma lista
# de algumas semanas atras. Uma hora (o valor antigo) fazia o `/model` reaparecer no
# terminal do usuario "sozinho" no meio de sessoes longas, e cada restart do backend zerava o
# cache em memoria e relia tudo de novo — dai o espelho em DISCO, dentro do proprio config dir
# (`.hangar-models.json`): a leitura dirigida do picker vira acontecimento raro.
# A chave e o config dir, nao a sessao: a lista vem da CONTA, e a mesma pra todas as sessoes dela.
_CLAUDE_MODELS_TTL = 30 * 24 * 3600.0
_claude_models_cache: dict[str, tuple[float, dict]] = {}


def _models_cache_path(chave: str) -> Path:
    return Path(chave) / ".hangar-models.json"


def _leitura_cortada(resp: dict) -> bool:
    """Picker lido com o pane estreito: o nome vem cortado ("Opus (1M con…") e a lista, pela metade.
    Serve pra tela uma vez, mas guardada viraria a lista da conta por 30 dias."""
    return any(str(m.get("name", "")).rstrip().endswith(("…", "...")) for m in resp.get("models") or [])


def _sem_repetidos(models: list[dict]) -> list[dict]:
    """Versões antigas saem do picker com a keyword da família (`opus`): escolher "Opus 4.6" abriria o
    Opus atual. Só a primeira linha de cada id é escolhível de verdade."""
    vistos: set = set()
    return [m for m in models if not (m.get("id") in vistos or vistos.add(m.get("id")))]


def _models_cache_get(chave: str) -> dict | None:
    hit = _claude_models_cache.get(chave)
    if hit and time.monotonic() - hit[0] < _CLAUDE_MODELS_TTL and not _leitura_cortada(hit[1]):
        return {**hit[1], "models": _sem_repetidos(hit[1].get("models") or [])}
    try:
        # Sem a ponte do nome antigo, ao contrário dos outros sidecars: `.claude-pocket-models.json`
        # ficou SYMLINKADO pro ~/.claude dentro de toda conta (o `_NAO_LIGAR` do contas.py só
        # conhece o nome novo), então lê-lo servia o cache da conta padrão pra todas as outras —
        # o vazamento que `test_cache_de_outra_conta_nao_vaza` proíbe. Cache perdido custa uma
        # leitura; cache de outra conta mente sobre quais modelos aquele login tem.
        bruto = json.loads(_models_cache_path(chave).read_text(encoding="utf-8"))
        resp = bruto["resp"]
        if not isinstance(resp, dict) or time.time() - float(bruto["ts"]) >= _CLAUDE_MODELS_TTL or _leitura_cortada(resp):
            return None
    except (OSError, ValueError, KeyError, TypeError):
        return None
    # Promove pra memoria DESCONTANDO a idade que o registro ja tem no disco — carimbar com o
    # monotonic de agora zerava o relogio e um dado de 6d23h passava a valer mais 7 dias.
    idade = time.time() - float(bruto["ts"])
    _claude_models_cache[chave] = (time.monotonic() - idade, resp)
    # Cache gravado antes da trava de repetidos ainda vale pelos 30 dias: a limpeza vale na leitura também.
    return {**resp, "models": _sem_repetidos(resp.get("models") or [])}


def _models_cache_put(chave: str, resp: dict) -> None:
    if _leitura_cortada(resp):
        return
    _claude_models_cache[chave] = (time.monotonic(), resp)
    alvo = _models_cache_path(chave)
    tmp = alvo.with_name(f"{alvo.name}.{os.getpid()}.tmp")
    try:
        tmp.write_text(json.dumps({"ts": time.time(), "resp": resp}), encoding="utf-8")
        atomico.substituir(tmp, alvo)
    except OSError:
        # Config dir somente-leitura ou inexistente: o cache em memoria segue valendo.
        tmp.unlink(missing_ok=True)


def _chave_config(p) -> str:
    """Chave única do cache de modelos da conta. As duas rotas TÊM que passar por aqui: a da sessão
    viva deriva do /proc (vazio = "~") e a da abertura recebe caminho do cliente."""
    s = str(p or "").strip()
    if not s or s == "~":
        return str(Path.home() / ".claude")
    return str(Path(s).expanduser().resolve())


def _fixed_engine_account(engine: str, account: str) -> dict:
    cfg = engines.listar().get(engine)
    if not cfg:
        raise HTTPException(400, detail=erro("erro_motor_invalido", "motor inválido"))
    try:
        return cliproxy.account_for_engine(cfg, account)
    except ValueError as exc:
        raise HTTPException(400, detail=erro("erro_cliproxy_conta", str(exc))) from None


async def _fixed_engine_models(engine: str, account: str, *, fresco: bool = False) -> list[dict]:
    from app.cliproxy_accounts import models_for
    selected = await asyncio.to_thread(_fixed_engine_account, engine, account)
    return models_for(await _engine_models(engine, fresco=fresco), selected["prefix"])


async def _engine_models(nome: str, fresco: bool = False) -> list[dict]:
    """Catalogo do provedor. `fresco=True` ignora o cache.

    Quem VALIDA uma troca pede fresco: o cache existe pra folha abrir rapido, mas a promessa do
    check ("recusa aqui em vez de deixar a falha aparecer so no proximo turno") nao sobrevive a 5
    minutos de lista velha — modelo tirado do plano passaria pela validacao e falharia depois. Uma
    chamada de rede numa acao deliberada do usuario e barata; num tick de tela, nao.
    """
    hit = _engine_models_cache.get(nome)
    if hit and not fresco and time.monotonic() - hit[0] < _ENGINE_MODELS_TTL:
        return hit[1]
    cfg = engines.listar().get(nome)
    if not cfg:
        raise HTTPException(409, detail=erro("erro_motor_ausente", f"motor {nome!r} nao esta mais no engines.json", nome=nome))
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, cfg["base_url"], cfg["api_key"])
    except RuntimeError as e:
        # O proxy pode repetir a chave no erro; ela não vai para o cliente.
        message = cliproxy.redact(str(e), cfg["api_key"])
        raise HTTPException(502, detail=erro("erro_provedor_offline", f"o provedor do motor {nome!r} nao respondeu: {message}", nome=nome, erro=message))
    _engine_models_cache[nome] = (time.monotonic(), modelos)
    return modelos


def _engine_picker_models(engine: str, models: list[dict]) -> list[dict]:
    return [{"id": model["id"], "context_length": model.get("context_length"),
             "vision": model.get("vision"), "supports_fast": cliproxy.supports_fast(engine, model["id"])}
            for model in models]


@app.get("/api/sessions/{name}/model/options", dependencies=[Depends(require_auth)])
async def model_options(name: str):
    """Modelos que ESTA sessao pode escolher. `kind` diz de onde vieram e como aplicar."""
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if info.provider not in (None, "claude"):
        raise HTTPException(400, detail=erro("erro_rota_so_claude", "esta rota so existe pra sessoes Claude Code"))
    if info.engine:
        # Motor: catalogo vem do /v1/models do provedor (HTTP), nao do pane -- nao conta linha,
        # nao depende do tamanho da janela. A guarda so vale pro ramo abaixo (le o picker).
        modelos = await (_fixed_engine_models(info.engine, info.engine_account)
                         if info.engine_account else _engine_models(info.engine))
        models = await asyncio.to_thread(_engine_picker_models, info.engine, modelos)
        result = {"kind": "engine", "engine": info.engine, "models": models}
        if any(model["supports_fast"] for model in models):
            model, tier = await asyncio.to_thread(_engine_fast_selection, name)
            model = model or engines.listar()[info.engine].get("model")
            result.update(supports_fast=True,
                          current={"model": model.rsplit("/", 1)[-1] if model else None, "service_tier": tier})
        return result
    if _headless(name):
        # Sem terminal: a lista vem do `control_request list_models` do próprio processo — sem
        # picker, sem rastro no scrollback e sem cache de 1h.
        hl = get_adapter(CLAUDE_HEADLESS)
        try:
            modelos = await hl.list_models(name)
        except Exception as e:
            raise HTTPException(503, detail=erro("erro_modelos_indisponiveis", f"não consegui listar os modelos: {e}"))
        meta = headless_sessions.load(name) or {}
        atual = hl.escolhas(name)[0] or meta.get("model")
        return {"kind": "claude", "engine": None, "effort": meta.get("effort"),
                "models": claude_models.para_tela(modelos, atual)}
    # Conta Anthropic: le o picker de verdade. Abre e fecha um overlay — nao vai pro scrollback,
    # nao entra no transcript e nao gasta token.
    await asyncio.to_thread(_recusa_se_painel_aberto, name)
    chave = _chave_config(_session_config_dir(name))
    cacheado = _models_cache_get(chave)
    if cacheado is not None:
        return cacheado
    try:
        lido = await asyncio.to_thread(terminal.list_model_options, name)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    resp = {"kind": "claude", "engine": None, "effort": lido["effort"],
            # `id` (único por linha), não `keyword`: duas linhas do picker compartilham a keyword
            # `opus` ("Opus" e "Opus (1M context)"), e id repetido derrubava a lista na tela.
            "models": [{"id": r["id"], "name": r["name"], "desc": r["desc"],
                        "active": r["active"]} for r in lido["models"]]}
    resp["models"] = _sem_repetidos(resp["models"])
    _models_cache_put(chave, resp)
    return resp


@app.get("/api/model-options", dependencies=[Depends(require_auth)])
async def model_options_sem_sessao(provider: str = "claude", engine: str = "",
                                   config_dir: str = "", codex_account: str = "",
                                   engine_account: str | None = None):
    """Modelos oferecidos na tela de ABERTURA, onde ainda não existe sessão.

    Irmã de /api/sessions/{name}/model/options, que não serve aqui: no ramo da conta Anthropic
    aquela LÊ O PICKER dirigindo o terminal de uma sessão viva. Sem sessão, o melhor que existe é o
    cache por config dir que aquela rota já alimentou — e, frio, os aliases mínimos, ditos como
    reduzidos em vez de fingirem ser a lista completa (ver o comentário acima sobre a lista
    chumbada que não soube do Fable).
    """
    if engine_account is not None and (provider != "claude" or not engine):
        raise HTTPException(400, detail=erro("erro_cliproxy_conta", "conta ChatGPT exige Claude com motor CLIProxyAPI local"))
    if provider in ("pi", "omp"):
        try:
            return {"kind": provider, "reduced": False,
                    "models": await asyncio.to_thread(pi_catalog.listar, provider)}
        except pi_catalog.PiAusente as e:
            # Codigo proprio: "nao achei o pi" nao e "o pi falhou". Antes isso chegava como
            # `[WinError 2] O sistema nao pode encontrar o arquivo especificado` dentro da mensagem
            # de falha do comando — a pessoa ia procurar defeito no `pi --list-models` de um pi que
            # nem estava instalado ali.
            # Codigo proprio por provider, mesmo motivo do erro_omp_list_models: o front traduz por
            # `code`, entao um codigo so mandaria a sessao omp instalar o Pi.
            codigo = "erro_omp_ausente" if provider == "omp" else "erro_pi_ausente"
            raise HTTPException(502, detail=erro(codigo, str(e), erro=str(e)))
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            # Codigo proprio por provider: o front traduz por `code` (a `msg` do backend so aparece
            # pra codigo DESCONHECIDO), entao um so codigo pros dois faria a falha do omp renderizar
            # o texto fixo "pi --list-models falhou" na tela.
            if provider == "omp":
                raise HTTPException(502, detail=erro("erro_omp_list_models", f"omp models --json falhou: {e}", erro=str(e)))
            raise HTTPException(502, detail=erro("erro_pi_list_models", f"pi --list-models falhou: {e}", erro=str(e)))
    if provider == "kimi":
        # Sem subprocess aqui (não existe `kimi --list-models`): o catálogo é o config.toml.
        cat = kimi_models.read_catalog()
        if cat is None:
            raise HTTPException(409, detail=erro("erro_catalogo_kimi_indisponivel",
                                                 "catalogo do Kimi indisponível — ~/.kimi-code/config.toml "
                                                 "ausente ou sem seções [models.*]"))
        return {"kind": "kimi", "reduced": False, "models": cat["models"], "default": cat["default"]}
    if provider == "codex":
        account = _resolve_codex_account(codex_account or None)
        _codex_require_idle_preparation(account, _codex_service())
        # Nem config no disco (o ~/.codex/config.toml guarda o modelo escolhido, nunca a lista) nem
        # `codex --list-models`: a fonte e o `model/list` de um app-server efemero em stdio, a MESMA
        # que a folha da sessao viva usa. Ver app/codex_models.py.
        try:
            return {"kind": "codex", "reduced": False,
                    "models": await asyncio.to_thread(
                        codex_models.listar,
                        **({"codex_home": account.home} if not account.is_default else {}))}
        except codex_models.CodexAusente as e:
            # Codigo proprio pelo mesmo motivo do Pi: "nao achei o codex" nao e "o codex falhou".
            raise HTTPException(502, detail=erro("erro_codex_ausente", str(e), erro=str(e)))
        except (codex_models.CodexIndisponivel, codex_models.CodexRecusado,
                codex_models.CodexRespostaInvalida, RuntimeError, OSError) as e:
            # Sem `TimeoutExpired` aqui, ao contrario do ramo do Pi: o teto de tempo do
            # `codex_models` mata o processo por um Timer, entao ele vira "nao respondeu" (um
            # RuntimeError) — capturar a outra seria um ramo que o codigo nunca produz.
            raise HTTPException(502, detail=erro("erro_codex_model_list", f"codex app-server model/list falhou: {e}", erro=str(e)))
    if provider != "claude":
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider deve ser 'claude', 'pi', 'omp', 'kimi' ou 'codex'"))
    if engine:
        modelos = await (_fixed_engine_models(engine, engine_account)
                         if engine_account is not None else _engine_models(engine))
        models = await asyncio.to_thread(_engine_picker_models, engine, modelos)
        return {"kind": "engine", "reduced": False, "models": models,
                "supports_fast": any(model["supports_fast"] for model in models)}
    chave = _chave_config(config_dir)
    cacheado = _models_cache_get(chave)
    if cacheado is not None:
        return {**cacheado, "reduced": False}
    try:
        crus = await asyncio.to_thread(claude_models.listar, config_dir or None)
    except claude_models.ClaudeIndisponivel as e:
        # Fallback, nao 502: a lista reduzida abre a sessao, e recusar a tela inteira porque o
        # catalogo nao veio seria pior que oferecer os aliases. O motivo vai pro log.
        _log.warning("catalogo claude sem sessao falhou config_dir=%s: %s", chave, e)
    else:
        # Sem `effort`: quem sabe o nivel atual e a SESSAO, e aqui nao ha uma. A chave e a mesma do
        # picker de proposito (a lista vem da conta, nao da sessao); nenhum leitor do cache usa o
        # campo, e inventar um nivel aqui seria pior que a ausencia dele.
        resp = {"kind": "claude", "engine": None, "models": claude_models.para_tela(crus)}
        _models_cache_put(chave, resp)
        return {**resp, "reduced": False}
    return {"kind": "claude", "reduced": True,
            "models": [{"id": a} for a in ("opus", "fable", "sonnet", "haiku")]}


@app.post("/api/sessions/{name}/engine/model", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def engine_model_set(name: str, body: EngineModelBody):
    """Troca o modelo (e opcionalmente o esforco) de uma sessao que roda num motor.

    O `/model <id>` do Claude Code aplica na sessao E grava o id como default GLOBAL pra sessoes
    novas — inclusive as da conta Anthropic, que nao conhecem esse id. Por isso o valor anterior do
    settings.json e capturado antes e reposto depois: a troca vale onde foi pedida e em lugar nenhum
    mais. Ver app/default_model.py.
    """
    await asyncio.to_thread(_recusa_se_painel_aberto, name)
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.engine:
        raise HTTPException(400, detail=erro("erro_rota_so_motor", "esta rota so existe pra sessoes que rodam num motor"))
    # fresco=True: a validacao promete "recusa aqui em vez de deixar a falha aparecer so no proximo
    # turno", e essa promessa nao sobrevive ao cache de 5 min (ver _engine_models).
    account_models = await _engine_models(info.engine, fresco=True)
    modelos = account_models
    if info.engine_account:
        from app.cliproxy_accounts import models_for
        account = await asyncio.to_thread(_fixed_engine_account, info.engine, info.engine_account)
        modelos = models_for(account_models, account["prefix"])
    catalog_id = await asyncio.to_thread(engines.catalog_model, body.model)
    if not any(m["id"] == catalog_id for m in modelos):
        # Recusar aqui em vez de digitar: o CC aceitaria o id, a sessao passaria a mandar request
        # pra um modelo que o provedor nao tem, e a falha apareceria so no proximo turno.
        raise HTTPException(422, detail=erro("erro_modelo_fora_catalogo", f"modelo fora do catalogo do motor {info.engine!r}: {body.model}", motor=info.engine, modelo=body.model))

    _, tier = await asyncio.to_thread(_engine_fast_selection, name)
    if tier == "priority" and not await asyncio.to_thread(cliproxy.supports_fast, info.engine, body.model):
        raise HTTPException(409, detail=erro("erro_fast_indisponivel", "Desligue Fast antes de escolher um modelo que não o suporta"))

    if info.engine_account:
        selected = next(m for m in modelos if m["id"] == catalog_id)
        window = 1_000_000 if catalog_id != body.model else selected.get("context_length")
        await _durante_troca(name, _trocar_conta(name, None, engine_account=info.engine_account,
                                               model=body.model, effort=body.effort,
                                               context_window=window, engine_models=account_models))
        return {"ok": True, "model": body.model}
    if _headless(name):
        # Sem pane: `set_model` por control_request, que (medido) NÃO grava o default global —
        # nada a repor no settings.json. O esforço vai como `/effort <x>` pelo stdin.
        try:
            esforco_ja_vale = await get_adapter(CLAUDE_HEADLESS).set_model(name, body.model, body.effort)
        except Exception as e:
            _log.exception("claude headless: troca de modelo falhou name=%s", name)
            raise HTTPException(409, detail=erro("erro_headless_modelo", f"troca de modelo falhou: {e}", erro=str(e)))
        res = {"ok": True, "model": body.model}
        if body.effort and not esforco_ja_vale:
            res["effort_error"] = "esforço entra no fim do turno em andamento"
        return res

    cfg_dir = _session_config_dir(name)  # mesma leitura de /proc que resolve o config dir das outras rotas
    antes = await asyncio.to_thread(default_model.snapshot, cfg_dir)
    digitou = True
    try:
        res = await asyncio.to_thread(terminal.set_engine_model, name, body.model)
    except TerminalInput.NaoDigitou as e:
        # Recusado antes de qualquer tecla (sessao ocupada/morta, menu aberto): o settings.json esta
        # intocado, entao esperar a escrita aterrissar so faria o erro demorar ~3.6s a aparecer.
        digitou = False
        raise HTTPException(e.status, e.detail)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    except ValueError as e:
        raise HTTPException(422, str(e))
    finally:
        # No finally de proposito: se o comando foi digitado mas a confirmacao nao pode ser lida, o
        # settings.json PODE ja ter sido reescrito — deixar o default global vazado por causa de um
        # erro de leitura seria a pior combinacao.
        if digitou:
            await asyncio.to_thread(default_model.restore_quando_aterrissar, cfg_dir, antes)

    if body.effort:
        # Esforco continua saindo do picker (Left/Right): medido que ele funciona igual em sessao de
        # motor — o chip `(high✦)` e real, nao maquiagem. Falha aqui nao desfaz o modelo, que ja
        # pegou; reporta junto pra tela nao dizer que tudo deu certo.
        try:
            await asyncio.to_thread(terminal.set_model_effort, name, None, body.effort, "session")
        except (PickerError, ValueError) as e:
            return {**res, "model": body.model, "effort_error": str(e)}
    return {**res, "model": body.model}


# ── Modelo + raciocinio de uma sessao Pi ────────────────────────────────────────────────────────
# Rotas separadas das do Claude (/model-effort, picker do TUI) e das do Codex (/models, app-server)
# porque o mecanismo e um terceiro: a extensao hangar-state.ts publica o catalogo num sidecar e expoe
# dois comandos que aplicam a troca pela API do Pi. Ver app/pi_models.py pro porque de nao raspar
# o TUI aqui.

class PiModelBody(_StrictBody):
    provider: str | None = None
    model: str | None = None
    effort: str | None = None


def _session_config_dir(name: str) -> Path | None:
    """CLAUDE_CONFIG_DIR do processo do pane (o sidecar do Pi e o settings.json moram la dentro).
    None -> ~/.claude.
    Usa o mesmo `_config_dir_of` do registry (privado do pacote) que ja resolve o transcript do Pi:
    duas leituras diferentes do /proc dariam respostas diferentes pra mesma sessao."""
    from app import registry as registry_mod
    from app import tmux
    try:
        pid = tmux.pane_pid(name)
        return registry_mod._config_dir_of(pid) if pid else None
    except Exception:
        # Cair no ~/.claude CALADO transformava um bug de resolucao (pane sem pid, /proc ilegivel)
        # num 409 mentiroso "extensao desatualizada": o sidecar existe, so estavamos procurando na
        # pasta errada. Nao propaga — o default ainda e o certo pra maioria das sessoes.
        _log.warning("pi: nao consegui resolver o config dir de %s; usando ~/.claude", name,
                     exc_info=True)
        return None


def _session_config_dir_strict(name: str) -> tuple[Path | None, bool]:
    """CLAUDE_CONFIG_DIR da sessão pro DELETE de conta: (Path | None, confiável).

    A irmã acima (fallback silencioso pro ~/.claude) é certa pra LEITURA e perigosa numa operação
    DESTRUTIVA: falha de resolução virava None, None não casa com o alvo, e o apagar seguia como
    se a sessão usasse a conta padrão. Aqui falha devolve confiável=False e quem chama recusa —
    na dúvida, não apaga. None + True = processo vivo SEM a var no ambiente: usa a conta padrão,
    não a que está sendo apagada.
    """
    from app import tmux
    # Sem terminal não há pane: a conta vem do sidecar, mesmo com o processo estacionado (ele
    # volta com --resume na mesma conta).
    if headless_sessions.exists(name):
        meta = headless_sessions.load(name)
        if not isinstance(meta, dict):
            return None, False
        cfg = meta.get("config_dir")
        return (Path(cfg) if cfg else None), True
    try:
        pid = tmux.pane_pid(name)
    except Exception:
        return None, False
    if not pid:
        return None, True   # sem processo vivo: ninguém está usando nada
    return procinfo._config_dir_of_strict(pid)


def _caller_config_dir(name: str) -> tuple[Path | None, bool]:
    """CLAUDE_CONFIG_DIR de quem PEDE uma sessão nova: (Path | None, confiável).

    Difere da irmã do DELETE num ponto só: pane sem processo. Lá "ninguém está usando" libera o
    apagar; aqui a conta de quem chama ficou desconhecida, e criar assim nasce na conta padrão —
    a falha calada que cobra a conta errada. Sem terminal e processo vivo sem a var seguem como
    lá: os dois sabem a conta (a do sidecar, a padrão).
    """
    from app import tmux
    if not headless_sessions.exists(name):
        try:
            if not tmux.pane_pid(name):
                return None, False
        except Exception:
            return None, False
    return _session_config_dir_strict(name)


async def _pi_catalog(name: str) -> tuple[dict, str]:
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao ou transcript nao encontrado"))
    if info.provider not in ("pi", "omp"):
        raise HTTPException(400, detail=erro("erro_rota_so_pi", "esta rota so existe pra sessoes Pi"))
    cat = await asyncio.to_thread(pi_models.read_catalog, info.jsonl, _session_config_dir(name))
    if cat is None:
        # Falha ALTA: sem o sidecar nao ha catalogo real, e inventar um faria o app oferecer
        # modelos que o `/cp-model` nao encontraria. Instrucao junto porque a causa e sempre a
        # mesma (extensao velha/ausente) — mas o conserto E o CODIGO mudam por provider: o front
        # traduz pelo `code` (a `msg` do backend so aparece pra codigo DESCONHECIDO), entao um so
        # codigo pros dois faria uma sessao omp mostrar a instrucao do Pi.
        if info.provider == "omp":
            raise HTTPException(409, detail=erro(
                "erro_catalogo_omp_indisponivel",
                "catalogo do omp indisponivel — rode ./scripts/install-claude-wrapper.sh e feche "
                "e reabra a sessao (o /reload do omp nao recarrega a extensao)"))
        raise HTTPException(409, detail=erro(
            "erro_catalogo_pi_indisponivel",
            "catalogo do Pi indisponivel — rode ./scripts/install-claude-wrapper.sh e reinicie a "
            "sessao (extensao hangar-state.ts desatualizada)"))
    return cat, info.jsonl


@app.get("/api/sessions/{name}/pi/models", dependencies=[Depends(require_auth)])
async def pi_models_list(name: str):
    cat, _ = await _pi_catalog(name)
    return {"models": cat.get("models", []), "current": cat.get("current"),
            "thinking": cat.get("thinking"), "levels": cat.get("levels", [])}


@app.post("/api/sessions/{name}/pi/model", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def pi_model_set(name: str, body: PiModelBody):
    cat, jsonl = await _pi_catalog(name)
    cmds: list[str] = []
    try:
        if body.model:
            if not body.provider:
                raise pi_models.PiModelError(422, "provider obrigatorio junto com model")
            pi_models.check_known(cat, body.provider, body.model)
            cmds.append(pi_models.model_command(body.provider, body.model))
        if body.effort:
            cmds.append(pi_models.think_command(body.effort))
    except pi_models.PiModelError as e:
        raise HTTPException(e.status, e.detail)
    if not cmds:
        raise HTTPException(422, detail=erro("erro_model_effort_faltando", "informe model (com provider) e/ou effort"))
    try:
        await asyncio.to_thread(terminal.send_pi_commands, name, cmds)
    except terminal_input.DriveError as e:
        raise HTTPException(409, str(e))
    # Re-le o sidecar ATE ele confirmar (ou estourar 2s): o Pi CLAMPA o nivel pro que o modelo
    # suporta, entao o que voltamos e o que FICOU, nao o que foi pedido — e o `/cp-model` pode
    # RECUSAR sem levantar nada (sem chave pro provedor: notifica no TUI e o sidecar segue no modelo
    # velho). Devolver ok=True sem comparar era declarar sucesso sobre um no-op, com a folha
    # fechando calada.
    after = await asyncio.to_thread(pi_models.read_back, jsonl, _session_config_dir(name),
                                    body.provider, body.model, body.effort)
    if after is not None and pi_models.confirms(after, body.provider, body.model, body.effort):
        return {"ok": True, "current": after.get("current"), "thinking": after.get("thinking"),
                "levels": after.get("levels", [])}
    # Nao confirmou. As duas causas pedem acoes diferentes do usuario, entao nao viram a mesma frase:
    # sidecar ilegivel ou parado no MESMO `ts` = o Pi nem republicou o catalogo (comando pode nao ter
    # chegado) -> indeterminado; `ts` novo com o modelo velho = o Pi processou e RECUSOU.
    if after is None or after.get("ts") == cat.get("ts"):
        raise HTTPException(409, detail=erro("erro_sem_confirmacao_troca",
                                             "comandos digitados, mas o Pi nao republicou o catalogo — nao da "
                                             "pra confirmar a troca; veja o modelo no proprio terminal"))
    cur = after.get("current") or {}
    raise HTTPException(409, detail=erro("erro_pi_recusou_troca",
                                             f"o Pi recusou a troca — segue em "
                                             f"{cur.get('provider')}/{cur.get('id')} (raciocinio "
                                             f"{after.get('thinking')}). Causa mais comum: sem chave configurada "
                                             f"pro provedor pedido (o Pi avisa dentro do TUI)",
                                             provider=cur.get("provider"), id=cur.get("id"),
                                             thinking=after.get("thinking")))


# ── Modelo de uma sessão Kimi ─────────────────────────────────────────────────────────────────
# Quarto mecanismo, diferente dos três vizinhos: sem picker legível (Claude), sem extensão com
# sidecar (Pi), sem app-server (Codex). O catálogo mora no ~/.kimi-code/config.toml e a troca
# dirige a busca do picker + Alt+S, confirmada pela linha "Switched to …" do scrollback — ver
# app/kimi_models.py pro que foi medido na TUI.

class KimiModelBody(_StrictBody):
    model: str | None = None
    effort: str | None = None


def _kimi_catalog() -> dict:
    cat = kimi_models.read_catalog()
    if cat is None:
        # Mesma política do _pi_catalog: falha ALTA com instrução, nunca lista inventada.
        raise HTTPException(409, detail=erro("erro_catalogo_kimi_indisponivel",
                                             "catalogo do Kimi indisponível — ~/.kimi-code/config.toml "
                                             "ausente ou sem seções [models.*]"))
    return cat


async def _kimi_info(name: str):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider != "kimi":
        raise HTTPException(400, detail=erro("erro_rota_so_kimi", "esta rota só existe pra sessões Kimi"))
    return info


@app.get("/api/sessions/{name}/kimi/models", dependencies=[Depends(require_auth)])
async def kimi_models_list(name: str):
    await _kimi_info(name)
    cat = _kimi_catalog()
    # "current" ao vivo não tem fonte barata (a TUI não expõe e o marcador da statusline é display
    # name, que repete entre providers): quem mostra o atual é a pill do composer, que já lê a
    # statusline. Aqui vai o default do config como referência da abertura.
    return {"models": cat["models"], "default": cat["default"]}


@app.post("/api/sessions/{name}/kimi/model", dependencies=[Depends(require_auth), Depends(_transfer_guard)])
async def kimi_model_set(name: str, body: KimiModelBody):
    info = await _kimi_info(name)
    await asyncio.to_thread(_recusa_se_painel_aberto, name)
    # Sessão TRABALHANDO: o `/model` digitado cairia no composer e o Enter o enfileiraria como
    # MENSAGEM — a troca viraria um "/model" pro modelo ler. No Claude o _require_drivable cobre
    # isso pelo spinner; o do Kimi são fases de lua, fora do que ele detecta, então a guarda é o
    # marcador do hook (corrigido: pode ser o idle CONGELADO do turno anterior).
    if info.jsonl:
        m = hook_state.get_state(session_key(info.jsonl))
        if m:
            m = corrige_ocioso_kimi(m, info.jsonl)
        if m and m[0] == "working":
            raise HTTPException(409, detail=erro("erro_sessao_trabalhando",
                                                 "a sessão está trabalhando — espere ela terminar"))
    cat = _kimi_catalog()
    try:
        alvo = kimi_models.check_known(cat, body.model) if body.model else None
        nivel = None
        if body.effort:
            # Com modelo junto, valida contra o support_efforts DELE. Sozinho, quem valida é o
            # picker ao vivo (a linha Thinking mostra os níveis do modelo ATUAL — o backend não
            # sabe o alias vigente sem perguntar à TUI).
            nivel = (kimi_models.check_effort(alvo, body.effort) if alvo
                     else kimi_models.clean_alias(body.effort).lower())
    except kimi_models.KimiModelError as e:
        raise HTTPException(e.status, e.detail)
    if alvo is None and nivel is None:
        raise HTTPException(422, detail=erro("erro_model_effort_faltando",
                                             "informe model e/ou effort"))
    try:
        res = await asyncio.to_thread(terminal.set_kimi_model, name,
                                      alvo and alvo["alias"], alvo and alvo["name"], nivel)
    except terminal_input.DriveError as e:
        raise HTTPException(409, str(e))
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True,
            "current": {"alias": alvo["alias"], "name": alvo["name"]} if alvo else None,
            "effort": nivel, "result": res.get("result")}


@app.get("/api/fs/roots", dependencies=[Depends(require_auth)])
def fs_roots():
    return list_roots()


@app.get("/api/fs/scan", dependencies=[Depends(require_auth)])
def fs_scan(root: str, path: str | None = None):
    # A seguranca (allowlist + rejeicao de escape) vive em scan_dir; aqui so traduzimos
    # a FsError pro status HTTP correspondente.
    try:
        return scan_dir(root, path)
    except FsError as e:
        raise HTTPException(e.status, e.detail)


class FsMkdirBody(BaseModel):
    root: str
    path: str | None = None
    name: str


@app.post("/api/fs/mkdir", dependencies=[Depends(require_auth)])
def fs_mkdir(body: FsMkdirBody):
    try:
        return make_dir(body.root, body.path, body.name)
    except FsError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/fs/branches", dependencies=[Depends(require_auth)])
def fs_branches(root: str, path: str | None = None):
    try:
        scan_dir(root, path)
        return list_branches(str(Path(os.path.realpath(os.path.expanduser(path or root)))))
    except (FsError, GitError) as exc:
        raise HTTPException(exc.status, detail=erro("erro_criacao_sessao", exc.detail)) from None


def _no_guest() -> None:
    if guest_users.current.get() is not None:
        raise HTTPException(403, detail="convidado não acessa worktrees")


def _allowed_repo(path: str) -> str:
    """Repo/worktree dentro de uma raiz autorizada; pasta sumida valida pela pasta-mãe."""
    probe = path if os.path.isdir(path) else str(Path(path).parent)
    try:
        _allowed_scan_root(probe)
    except FsError as exc:
        if not _registered_in_allowed_repo(path):
            raise HTTPException(exc.status, detail=exc.detail) from None
    return os.path.realpath(path) if os.path.isdir(path) else path


def _registered_in_allowed_repo(path: str) -> bool:
    """Worktree fora das raízes (o Codex cria em `~/.codex/worktrees`) vale pelo repo principal que
    a registra, se ele estiver numa raiz: a mesma regra da lista e do lote."""
    if os.path.isdir(path):
        root = worktrees.repo_root_of(path)
        if not root or os.path.realpath(root) != os.path.realpath(path):
            return False
        main = worktrees.main_repo_of(root)
    else:
        main = worktrees._main_of_missing(path)
    if os.path.realpath(main) == os.path.realpath(path):
        return False
    try:
        _allowed_scan_root(main)
    except FsError:
        return False
    real = os.path.realpath(path)
    return any(p == path or os.path.realpath(p) == real for p in worktrees.worktree_paths(main))


async def _worktree_inputs():
    """Sessões, pastas dentro das raízes (vivas + com conversa nos últimos 30 dias) e raízes: o que
    a lista de worktrees lê aqui e o hangar-server recebe por `/internal/worktrees/context`."""
    sessions = await asyncio.to_thread(registry.list)
    corte = time.time() - 30 * 86400
    folders = await asyncio.to_thread(list_folders)
    cwds = [s.cwd for s in sessions] + [f.cwd for f in folders if f.cwd and f.mtime >= corte]
    roots = allowed_roots()
    allowed = await asyncio.to_thread(
        lambda: [c for c in cwds if c and any(Path(os.path.realpath(c)).is_relative_to(r) for r in roots)])
    return sessions, allowed, roots


@app.get("/api/worktrees", dependencies=[Depends(require_auth)])
async def worktrees_list(repo: str | None = None, sizes: bool = True):
    """`repo`: só as desse repositório; `sizes=false`: não agenda medir o espaço (menu de branch)."""
    _no_guest()
    sessions, allowed, roots = await _worktree_inputs()
    if repo is not None:
        repo = await asyncio.to_thread(_allowed_repo, repo)
    return {"repos": await asyncio.to_thread(worktrees.list_all, allowed, sessions, roots, repo, sizes)}


def _allowed_worktree(path: str) -> str:
    path = _allowed_repo(path)
    # Pasta que existe mas não é raiz de repo/worktree daria uma situação inventada.
    if os.path.isdir(path) and not os.path.exists(os.path.join(path, ".git")):
        raise HTTPException(404, detail="não é um repositório git")
    return path


@app.get("/api/worktrees/detail", dependencies=[Depends(require_auth)])
async def worktrees_detail(path: str):
    _no_guest()
    path = await asyncio.to_thread(_allowed_worktree, path)
    sessions = await asyncio.to_thread(registry.list)
    return await asyncio.to_thread(worktrees.status, path, sessions)


class WorktreeRepoBody(_StrictBody):
    repo: str = Field(min_length=1)


@app.post("/api/worktrees/fetch", dependencies=[Depends(require_auth)])
async def worktrees_fetch(body: WorktreeRepoBody):
    _no_guest()
    try:
        await asyncio.to_thread(lambda: worktrees.fetch(_allowed_repo(body.repo)))
    except GitError as exc:
        raise HTTPException(exc.status, detail=exc.detail) from None
    return {"ok": True}


class WorktreeDeleteBody(_StrictBody):
    repo: str = Field(min_length=1)
    path: str = Field(min_length=1)
    confirm: bool = Field(default=False, strict=True)
    delete_branch: bool = Field(default=False, strict=True)


@app.post("/api/worktrees/delete", dependencies=[Depends(require_auth)])
async def worktrees_delete(body: WorktreeDeleteBody):
    _no_guest()
    repo, path = await asyncio.to_thread(lambda: (_allowed_repo(body.repo), _allowed_repo(body.path)))
    sessions = await asyncio.to_thread(registry.list)
    try:
        return await asyncio.to_thread(worktrees.delete, repo, path, sessions,
                                       confirm=body.confirm, delete_branch=body.delete_branch)
    except GitError as exc:
        raise HTTPException(exc.status, detail=exc.detail) from None
    finally:   # falha no meio também muda a lista (conversa que não voltou, worktree que saiu)
        await asyncio.to_thread(_invalidate_lists)


class WorktreeDeleteMergedBody(_StrictBody):
    repo: str = Field(min_length=1)
    # As worktrees que a tela mostrou na confirmação; sem a lista, só as que não perdem nada.
    paths: list[str] | None = None
    confirm: bool = Field(default=False, strict=True)
    # Das mostradas, as que a confirmação exibiu perdendo arquivos; as demais só saem se limpas.
    lossy: list[str] | None = None


@app.post("/api/worktrees/delete-merged", dependencies=[Depends(require_auth)])
async def worktrees_delete_merged(body: WorktreeDeleteMergedBody):
    _no_guest()
    if body.confirm and body.paths is None:
        raise HTTPException(422, detail="confirmar exige a lista das worktrees mostradas")
    repo = await asyncio.to_thread(_allowed_repo, body.repo)
    sessions = await asyncio.to_thread(registry.list)
    try:
        return {"removed": await asyncio.to_thread(worktrees.delete_merged, repo, sessions,
                                                   body.paths, body.confirm, body.lossy)}
    except GitError as exc:
        raise HTTPException(exc.status, detail=exc.detail) from None
    finally:   # as que saíram antes do erro também mudam a lista
        await asyncio.to_thread(_invalidate_lists)


class WorktreeCreateBody(_StrictBody):
    repo: str = Field(min_length=1)
    branch: str = Field(min_length=1)
    # Sufixo da pasta: a worktree nasce em `<repo>-<name>`, como as das sessões.
    name: str = Field(min_length=1)
    new_branch: bool = Field(default=False, strict=True)
    base: str | None = None
    fetch: bool = Field(default=False, strict=True)


@app.post("/api/worktrees/create", dependencies=[Depends(require_auth)])
async def worktrees_create(body: WorktreeCreateBody):
    """Só a worktree, sem sessão; com sessão, o caminho continua sendo o `POST /api/sessions`."""
    _no_guest()
    name = sanitize_session_name(body.name)
    if not name:
        raise HTTPException(400, detail="nome de pasta inválido")
    def create() -> tuple[str, bool]:
        root = _allowed_scan_root(body.repo)
        if body.fetch:
            worktrees.fetch(body.repo)
        path, created = create_worktree(body.repo, body.branch, name, root,
                                        new_branch=body.new_branch, base=body.base)
        # Na mesma thread: cliente que desconecta cancela o await, não a criação já em curso.
        if created:
            _invalidate_lists()
        return path, created

    try:
        path, created = await asyncio.to_thread(create)
    except (FsError, GitError) as exc:
        raise HTTPException(exc.status, detail=exc.detail) from None
    if not created:
        raise HTTPException(409, detail="essa branch já é a da pasta principal")
    return {"path": path}


# Git da pasta escolhida na tela de nova conversa: a mesma fronteira do seletor de pastas
# (`scan_dir`), e o git sempre fora do laço de eventos.
class FolderGitBody(_StrictBody):
    root: str
    path: str | None = None


class FolderSwitchBody(FolderGitBody):
    branch: str = Field(min_length=1)
    confirm_sessions: bool = False


class FolderBranchBody(FolderGitBody):
    name: str = Field(min_length=1)
    base: str | None = None
    checkout: bool = False
    confirm_sessions: bool = False


def _folder_cwd(root: str, path: str | None) -> str:
    scan_dir(root, path)
    return str(Path(os.path.realpath(os.path.expanduser(path or root))))


def _sessions_in(toplevel: str | None) -> list[str]:
    """Sessões vivas no mesmo checkout: trocar a branch dele muda os arquivos delas."""
    if not toplevel:
        return []
    top = Path(toplevel)
    return sorted(s.name for s in _guardar_snap()
                  if s.cwd and Path(os.path.realpath(s.cwd)).is_relative_to(top))


def _folder_git_sync(root: str, path: str | None, op: str, body=None) -> dict:
    try:
        cwd = _folder_cwd(root, path)
        if op == "status":
            st = folder_status(cwd)
        else:
            # Pasta dentro da raiz mas repositório acima dela (ex.: um repo na home): escrever nele
            # mexeria em arquivos fora da raiz liberada.
            top = folder_status(cwd).get("toplevel")
            if top and not Path(top).is_relative_to(Path(os.path.realpath(os.path.expanduser(root)))):
                raise GitError(400, "repositório fora da raiz autorizada")
            sessions = _sessions_in(top)
        if op == "fetch":
            st = folder_fetch(cwd)
        elif op == "pull":
            st = folder_pull(cwd)
        elif op != "status":
            if op == "switch":
                st = folder_switch(cwd, body.branch, sessions, body.confirm_sessions)
            else:
                st = folder_create_branch(cwd, body.name, body.base, body.checkout, sessions, body.confirm_sessions)
    except FsError as exc:
        raise HTTPException(exc.status, detail=erro("erro_criacao_sessao", exc.detail)) from None
    except GitError as exc:
        raise HTTPException(exc.status, detail=exc.detail) from None
    if st.get("repo"):
        st["sessions"] = _sessions_in(st.get("toplevel"))
    return st


@app.get("/api/fs/git", dependencies=[Depends(require_auth)])
async def fs_git_status(root: str, path: str | None = None):
    return await asyncio.to_thread(_folder_git_sync, root, path, "status")


@app.post("/api/fs/git/fetch", dependencies=[Depends(require_auth)])
async def fs_git_fetch(body: FolderGitBody):
    return await asyncio.to_thread(_folder_git_sync, body.root, body.path, "fetch")


@app.post("/api/fs/git/pull", dependencies=[Depends(require_auth)])
async def fs_git_pull(body: FolderGitBody):
    return await asyncio.to_thread(_folder_git_sync, body.root, body.path, "pull")


@app.post("/api/fs/git/switch", dependencies=[Depends(require_auth)])
async def fs_git_switch(body: FolderSwitchBody):
    return await asyncio.to_thread(_folder_git_sync, body.root, body.path, "switch", body)


@app.post("/api/fs/git/branch", dependencies=[Depends(require_auth)])
async def fs_git_branch(body: FolderBranchBody):
    return await asyncio.to_thread(_folder_git_sync, body.root, body.path, "branch", body)


# ── Preview: expoe um projeto local (porta) via tailscale serve, pro app ver num iframe ──
# GLOBAL por maquina (nao por sessao): o tunel usa uma porta-slot unica (10000), entao qualquer
# sessao que ligar o preview compartilha o mesmo slot. O backend que atende E o da maquina onde o
# projeto roda -> o preview sai da maquina certa sem config extra.
class PreviewBody(_StrictBody):
    port: int = Field(ge=1, le=65535)


@app.get("/api/preview", dependencies=[Depends(require_auth)])
def preview_status():
    try:
        return tunnel.status()
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/preview", dependencies=[Depends(require_auth)])
def preview_start(body: PreviewBody):
    try:
        return tunnel.start(body.port)
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.delete("/api/preview", dependencies=[Depends(require_auth)])
def preview_stop():
    try:
        return tunnel.stop()
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/navegador", dependencies=[Depends(require_auth)])
def navegador_da_sessao(name: str):
    """URL que o navegador embutido (app desktop) da sessão está mostrando, pelo sidecar que o shell
    grava em ~/.hangar/nav/ — o mesmo que `scripts/hangar-preview` lê. Sem navegador: url null."""
    pasta = Path.home() / ".hangar" / "nav"
    if pasta.is_dir():
        for arq in pasta.glob("*.json"):
            try:
                sc = json.loads(arq.read_text(encoding="utf-8"))
            except (OSError, ValueError) as e:
                _log.warning("sidecar de navegador ilegivel %s: %s", arq, e)
                continue
            chave = sc.get("chave", "") if isinstance(sc, dict) else ""
            if chave == name or chave.endswith(f"::{name}"):
                return {"url": sc.get("url")}
    return {"url": None}


@app.get("/api/sessions/{name}/commands", dependencies=[Depends(require_auth)])
async def commands(name: str):
    if _provider_of(name) == "codex":
        try:
            return [{"name": "compact", "display": "/compact", "source": "builtin",
                     "description": "Resume e compacta o contexto", "destructive": True},
                    *[{k: v for k, v in s.items() if k not in {"path", "native_name"}}
                      for s in await get_adapter("codex").list_skills(name)
                      if s["name"] != "compact"]]
        except RuntimeError:
            raise HTTPException(409, detail=erro("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.")) from None
    return await asyncio.to_thread(_commands_claude, name)


def _commands_claude(name: str):
    # Nomes vêm da CLI: sessão sem terminal usa o que o processo dela informou; as demais, a sonda
    # cacheada por (binário, config_dir). Enquanto nenhum dos dois existe, lista fixa + scans.
    if _headless(name):
        meta = headless_sessions.load(name) or {}
        cli, so_tui = get_adapter(CLAUDE_HEADLESS).comandos(name)
        if not cli:
            cli = comandos_da_cli(meta.get("config_dir"))
        # Sem o `init` ainda, os só-de-TUI conhecidos saem mesmo assim: não rodam sem terminal.
        return list_commands(meta.get("cwd"), cli, so_tui or frozenset({"color", "doctor", "reload-plugins"}),
                             com_tui=False)
    cdir = _session_config_dir(name)
    cli = comandos_da_cli(str(cdir) if cdir else None)
    # cwd vem do registry/tmux; se a sessao nao for achada, ainda devolvemos os built-ins
    # + skills globais (lista util mesmo sem cwd casado).
    #
    # `tmux.cwd_de` antes do `registry.list()`: esta rota so quer o cwd de UMA sessao, e a varredura
    # completa cobra tmux de todas as sessoes + /proc + `git` por sessao pra devolver tudo o mais.
    # Medido em 28/08/2026, ela era o companheiro mais caro da abertura de sessao — sozinha levava
    # o `/history` de 0,29s pra 0,53s. A varredura fica como plano B pros dois casos em que ela sabe
    # mais: a sessao Codex (vive num sidecar duravel, pode nao ter pane nenhum) e a sessao com 2+
    # panes (ali quem escolhe o cwd e o `_agent_pane`, que acha o pane do AGENTE — ver `cwd_de`).
    cwd = tmux.cwd_de(name)
    if cwd is None:
        cwd = next((s.cwd for s in registry.list() if s.name == name), None)
    if cwd is None:
        # Nem o tmux nem a varredura acharam a sessao. A lista SAI MESMO ASSIM (built-ins + skills
        # globais), que e util e e o comportamento de sempre — mas nao pode sair calada: sem cwd
        # faltam as skills e comandos DO PROJETO, e do lado de fora isso e indistinguivel de um
        # projeto que nao tem nenhuma.
        _log.warning("commands: sem cwd pra '%s' (tmux e registry nao acharam) — lista sem o que "
                     "e do projeto", name)
    return list_commands(cwd, cli)


_TTS_LIMITE_PADRAO = 5000
# Teto DURO, que nenhuma confirmacao passa — derivado do MODELO EM USO (tts.TETO_CARACTERES), nao
# um numero solto aqui: um teto maior que o do modelo deixaria confirmar um gasto que a ElevenLabs
# ainda ia recusar. Dois numeros diferentes de proposito: o limite configuravel e um AVISO de
# custo (o usuario confirma e passa); este e o que impede um cliente autenticado de mandar
# megabytes numa requisicao.
_TTS_TETO = tts.TETO_CARACTERES


def _tts_limite() -> int:
    """Limite de AVISO em caracteres. Configuravel; 0/ausente cai no padrao."""
    try:
        v = int(runtime_config.get("tts_max_chars") or 0)
    except (TypeError, ValueError):
        v = 0
    return v if v > 0 else _TTS_LIMITE_PADRAO


@app.post("/api/tts", dependencies=[Depends(require_auth)])
async def tts_sintetizar(body: TtsBody):
    texto = tts_preparar(body.text)
    if not texto:
        raise HTTPException(400, detail=erro("erro_tts_sem_texto", "nao sobrou nada pra falar depois de limpar o texto"))
    if len(texto) > _TTS_TETO:
        raise HTTPException(413, detail=erro("erro_tts_teto", f"texto com {len(texto)} caracteres passa do teto de {_TTS_TETO} — selecione um trecho menor", n=len(texto), teto=_TTS_TETO))
    limite = _tts_limite()
    if len(texto) > limite and not body.confirm:
        # 409, nao 413: nao e "grande demais", e "confirme que voce quer gastar isso". O front pede
        # a confirmacao e repete o POST com confirm=true. Checado AQUI e nao so na tela porque a
        # tela evita o susto e o servidor e quem guarda a conta.
        raise HTTPException(409, detail=erro("erro_tts_limite", f"são {len(texto)} caracteres, acima do limite de {limite} — confirme para gerar", n=len(texto), limite=limite))
    try:
        h, veio_do_cache, provedor_final = await asyncio.to_thread(
            tts.sintetizar, texto, body.voice, body.provider, body.instruction)
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)
    # provider ecoa o que RESPONDEU de fato (pode ter virado "local" pelo fallback sem chave) — o
    # front usa isso pra avisar na barra, em vez de trocar de voz caladamente.
    return {"url": f"/api/tts/audio/{h}", "chars": len(texto), "cached": veio_do_cache, "provider": provedor_final}


@app.post("/api/tts/narrar", dependencies=[Depends(require_auth)])
async def tts_narrar(body: NarrarBody):
    """Fase 2 (narracao guiada): trata o texto falavel de uma selecao pela Groq ANTES de virar
    audio — o resultado volta pro front pra REVISAO (o usuario confere antes de gastar credito da
    ElevenLabs), nao sintetiza nada aqui."""
    try:
        texto_tratado = await asyncio.to_thread(narrar.narrar, body.text, body.code_blocks, body.instruction)
    except narrar.NarrarError as e:
        raise HTTPException(e.status, e.detail)
    usou_groq = not narrar.eh_instrucao_padrao(body.instruction)
    chars_sent = (len(body.text) + sum(len(b) for b in body.code_blocks) + len(body.instruction)) if usou_groq else 0
    return {"text": texto_tratado, "chars_sent": chars_sent, "used_groq": usou_groq}


@app.get("/api/tts/voices", dependencies=[Depends(require_auth)])
async def tts_vozes():
    try:
        return {"voices": await asyncio.to_thread(tts.listar_vozes)}
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/tts/saldo", dependencies=[Depends(require_auth)])
async def tts_saldo():
    try:
        return await asyncio.to_thread(tts.saldo)
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/tts/audio/{h}", dependencies=[Depends(require_auth)])
async def tts_audio(h: str):
    # Hash validado ANTES de tocar no disco: o parametro vem da URL e sem isto viraria path
    # traversal. Mesmo espirito do guard de resolve_upload.
    if not re.fullmatch(r"[0-9a-f]{64}", h):
        raise HTTPException(400, detail=erro("erro_tts_audio_invalido", "identificador de audio invalido"))
    caminho = tts.caminho_do_cache(h)
    if not caminho.exists():
        raise HTTPException(404, detail=erro("erro_tts_sem_cache", "audio nao esta mais em cache"))
    # Extensao real do arquivo em cache, nao suposicao: o motor local pode ter devolvido WAV
    # (ver tts.extensao_de) — servir isso como audio/mpeg quebra o <audio> no WebKit.
    media_type = "audio/wav" if caminho.suffix == ".wav" else "audio/mpeg"
    return FileResponse(caminho, media_type=media_type)


# ── Interface (dist do frontend) ────────────────────────────────────────────────────────────────
# POR ÚLTIMO, depois de TODAS as rotas: o mount na raiz casa qualquer caminho, então registrado
# antes engoliria /api. Serve o build do Vite — arquivos estáticos comuns; o Vite em si não roda
# aqui e não precisa. Com isto o 8765 entrega tela E API num endereço só, e o servidor de
# desenvolvimento deixa de ser infraestrutura: vira ferramenta de quem mexe no layout.
#
# Medido em 05/08/2026: o `tailscale serve` publicava só o 5173, então parar o front (um `npm run
# dev`, que nem servia a tela usada — ela vem da VPS) derrubava a API do celular junto, com 502 em
# /api/sessions/events e o backend vivo o tempo todo.
#
# Ausente = instalação com --no-frontend, ou repo sem build. Sobe igual, só não serve tela.
class _UIStatic(StaticFiles):
    """`index.html` sempre revalida; o resto (nome com hash) segue cacheável à vontade.

    O `StaticFiles` manda só `etag`/`last-modified`, sem `cache-control` — e sem essa diretiva o
    navegador aplica FRESCOR HEURÍSTICO: serve o `index.html` que ele guardou sem nem perguntar ao
    servidor. Como o nome dos bundles tem hash, a página velha continua pedindo o CSS/JS velho: o
    build novo está no disco, servido corretamente, e a tela não muda. Fica parecendo bug de CSS.

    Medido em 10/08/2026: uma aba NOVA em 127.0.0.1:8765 carregou `index-DYyp82gq.css` enquanto
    `curl /` na mesma máquina entregava `index-CDPetMR_.css`; só `reloadIgnoringCache` consertou.
    Custou uma investigação inteira de "costura vertical na sidebar" que já estava consertada no
    código. A janela do Electron carrega deste mesmo endereço, então ela sofria igual.

    `no-cache` NÃO é `no-store`: o arquivo continua guardado, só volta a perguntar antes de usar —
    e com o ETag a resposta vira um 304 de algumas dezenas de bytes. Os assets ficam de fora de
    propósito; o hash no nome já os torna imutáveis, e revalidar cada um seria pagar ida e volta
    por arquivo sem ganhar nada.

    Decide pelo CAMINHO, não pelo `content-type` da resposta pronta: quando o pedido chega com
    `If-None-Match` batendo, o starlette devolve um `NotModifiedResponse`, que copia só uma lista
    fixa de headers e NÃO inclui o `content-type` — olhar o header ali deixaria justamente a
    resposta de revalidação sem diretiva nenhuma. Navegador nenhum regride por isso (o 304 mescla
    com o que ele já guardou), mas um proxy na frente do backend, sim — e tem um: a tela do celular
    passa por Traefik.
    """

    def file_response(self, full_path, *args, **kwargs) -> Response:  # type: ignore[no-untyped-def]
        resp = super().file_response(full_path, *args, **kwargs)
        if str(full_path).endswith(".html"):
            resp.headers["cache-control"] = "no-cache"
        elif Path(full_path).parent.name == "assets":
            resp.headers["cache-control"] = "public, max-age=31536000, immutable"
        return resp


from app import mcp_server
# Antes do estático em "/": mount é resolvido na ordem de registro.
app.mount("/mcp", mcp_server.asgi, name="mcp")

_DIST = Path(__file__).resolve().parents[2] / "frontend" / "dist"
if _DIST.is_dir():
    app.mount("/", _UIStatic(directory=_DIST, html=True), name="ui")
