import asyncio
import logging
import mimetypes
import os
import re
import subprocess
import threading
import time
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Literal
from fastapi import FastAPI, Depends, HTTPException, Request
from fastapi.responses import FileResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel, ConfigDict, Field
from sse_starlette.sse import EventSourceResponse
from app.auth import require_auth
from app.commands import list_commands
from app.fs import FsError, list_roots, scan_dir
from app.model_picker import PickerError
from app.registry import SessionRegistry
from app.models import SessionInfo, ChatEvent, CostReport
from app.pqueue import PromptQueue, _transcript_start_ts, committed_user_lines
from app.terminal_input import TerminalInput, drain
from app.sse import merged_events
from app.uploads import save_upload, resolve_upload, UploadError, MAX_BYTES
from app.config import list_config_dirs, ConfigDirInfo, _backend_config_base, settings
from app.costs import report as costs_report
from app.git_ops import (
    list_branches, switch_branch, git_action, changed_files, file_diff, discard_file, GitError,
)
from app.archive import ArchiveEntry, ArchiveFolder, archive_jsonl, list_conversations, list_folders
from app.askquestion import clear_pending_askq
from app.hook_state import hook_state
from app import push
from app.sync import sync_router
from app.deploy import deploy_router

_log = logging.getLogger("claude_pocket")


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
    _state_dirs = list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    hook_state.on_awaiting = _on_awaiting  # transicao -> awaiting_input dispara web push
    hook_state.on_transition = _on_hook_transition  # drain server-side + confirmacao de entrega
    task = asyncio.create_task(hook_state.watch(_state_dirs))

    def _watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("hook_state.watch crashed", exc_info=exc)

    task.add_done_callback(_watch_done)
    try:
        yield
    finally:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


app = FastAPI(title="claude-pocket", lifespan=_lifespan)
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
)
if settings.sync:
    app.include_router(sync_router)
app.include_router(deploy_router)
registry = SessionRegistry()
terminal = TerminalInput()


def _on_awaiting(session_id: str) -> None:
    """hook_state -> transicao awaiting_input. Resolve uuid->nome e manda push, TUDO numa thread:
    registry.list() mexe no tmux e o webpush e rede — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            name = next(
                (s.name for s in registry.list() if s.jsonl and Path(s.jsonl).stem == session_id),
                None,
            )
            if name:
                push.notify_awaiting(name)
        except Exception:
            _log.warning("push on_awaiting falhou (%s)", session_id, exc_info=True)
    threading.Thread(target=_work, daemon=True).start()


_CONFIRM_GRACE = 8.0  # s entre o send e a checagem "o transcript gravou o prompt?"


def _drain_session(name: str) -> None:
    """Entrega enfileiradas pendentes desta sessao (best-effort, roda fora do request)."""
    try:
        info = next((s for s in registry.list() if s.name == name), None)
        if info and info.jsonl:
            drain(name, info.jsonl)
    except Exception:
        pass


def _confirm_and_drain(name: str) -> None:
    """Confirmacao de entrega: delivered=True so diz 'send_keys chamado' — a TUI pode ter engolido
    as teclas e a msg sumia com cara de entregue. Confere contra o transcript; engolida ->
    re-enfileira (reconcile) e re-drena. Best-effort, roda em Timer/thread."""
    try:
        q = PromptQueue(name)
        if not any(r.get("delivered") is True and not r.get("confirmed") for r in q.load()):
            return  # nada a confirmar: nao paga registry nem o scan do transcript
        info = next((s for s in registry.list() if s.name == name), None)
        if not info or not info.jsonl:
            return
        # MID-TURN o prompt entregue ainda pode nao ter virado entrada no transcript (vive na fila
        # interna do Claude Code) — decidir requeue agora arriscaria redigitar mensagem ja recebida.
        # Adia pro proximo ciclo (o turno acabando dispara transicao -> novo timer).
        m = hook_state.get_state(Path(info.jsonl).stem)
        if m and m[0] == "working":
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
            return
        requeued = q.reconcile_delivered(
            committed_user_lines(info.jsonl), _transcript_start_ts(info.jsonl), time.time(),
            grace=_CONFIRM_GRACE,
        )
        if requeued:
            _log.info("REQUEUE name=%s n=%d (TUI engoliu o send; re-drenando)", name, len(requeued))
            drain(name, info.jsonl)
    except Exception:
        pass


def _on_hook_transition(session_id: str, state: str) -> None:
    """hook_state -> mudanca de estado. Drain SERVER-SIDE: o gatilho antigo morava na conexao SSE de
    cada chat — sem celular conectado, entrada deferred ficava parada indefinidamente. idle/working =
    o pane aceita texto (Claude Code enfileira internamente); o drain re-checa deliverable sozinho.
    Tambem agenda a confirmacao de entrega das drenadas."""
    if state == "awaiting_input":
        return
    def _work() -> None:
        try:
            info = next((s for s in registry.list()
                         if s.jsonl and Path(s.jsonl).stem == session_id), None)
            if info and info.jsonl:
                sent = drain(info.name, info.jsonl)
                # Confirmacao em TODO idle (nao so pos-drain): Timers pendentes morrem no restart
                # do backend — sem isto, entrada entregue ficava sem confirmar indefinidamente.
                if sent or state == "idle":
                    threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain,
                                    args=(info.name,)).start()
        except Exception:
            pass
    threading.Thread(target=_work, daemon=True).start()


class _StrictBody(BaseModel):
    # rejeita campos desconhecidos no corpo (contrato estrito; pega typo de campo do cliente -> 422).
    model_config = ConfigDict(extra="forbid")


class CreateBody(_StrictBody):
    name: str = Field(min_length=1)
    cwd: str = Field(min_length=1)
    config_dir: str | None = None


class PushSubscribeBody(_StrictBody):
    subscription: dict  # PushSubscription do browser: {endpoint, keys:{p256dh, auth}}
    label: str = Field(min_length=1)    # nome do servidor escolhido no celular (Casa/My-org)
    serverId: str = Field(min_length=1)  # id local do servidor no celular (pro deep-link da notif)


@app.get("/api/push/vapid", dependencies=[Depends(require_auth)])
def push_vapid():
    # Chave publica VAPID (applicationServerKey) pro browser assinar. Vazia = push desligado no backend.
    return {"key": settings.vapid_public}


@app.post("/api/push/subscribe", dependencies=[Depends(require_auth)])
def push_subscribe(body: PushSubscribeBody):
    try:
        push.add_subscription(body.subscription, body.label, body.serverId)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


class InputBody(_StrictBody):
    text: str


class SelectBody(_StrictBody):
    option: int = Field(ge=1, le=50)  # picker 1-based; teto evita loop de fork tmux (DoS)


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


@app.get("/api/sessions", dependencies=[Depends(require_auth)], response_model=list[SessionInfo])
async def list_sessions():
    # list_with_state: resolucao otimizada (1 scan /proc + 1 chamada tmux em lote) + estado vivo por
    # sessao (working/idle/awaiting_input) classificado do pane. async pq captura os panes concorrente.
    return await registry.list_with_state()


@app.get("/api/claude-configs", dependencies=[Depends(require_auth)], response_model=list[ConfigDirInfo])
def claude_configs():
    return list_config_dirs()


@app.get("/api/costs", dependencies=[Depends(require_auth)], response_model=CostReport)
def costs_endpoint():
    return costs_report()


@app.post("/api/sessions", dependencies=[Depends(require_auth)], response_model=SessionInfo)
def create_session(body: CreateBody):
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, "config_dir invalido")
    try:
        return registry.create(body.name, body.cwd, body.config_dir)
    except ValueError as e:
        raise HTTPException(409, str(e))


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth)])
def kill_session(name: str):
    registry.kill(name)
    return {"ok": True}


class RenameBody(_StrictBody):
    new: str


@app.post("/api/sessions/{name}/rename", dependencies=[Depends(require_auth)])
def rename_session(name: str, body: RenameBody):
    from app import tmux
    # tmux nao aceita espaco/./: no nome -> sanitiza. O transcript NAO depende do nome (resolve por
    # /proc), entao renomear nao quebra o historico. Migra so o sidecar da fila (keyed por nome).
    new = re.sub(r"[^A-Za-z0-9_-]", "-", body.new.strip()).strip("-")
    if not new:
        raise HTTPException(400, "nome invalido")
    if not tmux.has_session(name):
        raise HTTPException(404, "sessao nao encontrada")
    if new == name:
        return {"ok": True, "name": name}
    if tmux.has_session(new):
        raise HTTPException(409, "ja existe uma sessao com esse nome")
    if not tmux.rename_session(name, new):
        raise HTTPException(500, "falha ao renomear")
    registry.rename(name, new)  # migra o cache name->jsonl (senao serve transcript errado pos-rename)
    from app.pqueue import PromptQueue
    try:
        oq, nq = PromptQueue(name).path, PromptQueue(new).path
        if oq.exists():
            oq.replace(nq)
    except OSError:
        pass
    return {"ok": True, "name": new}


class ResumeBody(_StrictBody):
    # None = "escolha por mim" (caso seguro) ou pede confirmacao (caso ambiguo). uuid = o candidato que o
    # usuario escolheu no sheet de confirmacao.
    session_id: str | None = None


@app.post("/api/sessions/{name}/resume", dependencies=[Depends(require_auth)])
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
            raise HTTPException(404, "nenhum transcript pra retomar neste diretorio")
        if ambiguous and len(candidates) > 1:
            return {"ambiguous": True, "candidates": candidates}
        sid = candidates[0]["session_id"]
    try:
        return registry.resume(name, sid)
    except ValueError as e:
        raise HTTPException(409, str(e))


@app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
def history(name: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.pqueue import merged_history
    return merged_history(name, jsonl)


@app.get("/api/sessions/{name}/workflows", dependencies=[Depends(require_auth)])
def workflows_list(name: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import list_workflows
    return list_workflows(jsonl)


@app.get("/api/sessions/{name}/workflows/{run_id}", dependencies=[Depends(require_auth)])
def workflow_detail(name: str, run_id: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_workflow
    wf = get_workflow(jsonl, run_id)
    if wf is None:
        raise HTTPException(404, "workflow run not found")
    return wf


@app.get("/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}", dependencies=[Depends(require_auth)])
def workflow_agent_detail(name: str, run_id: str, agent_id: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_agent
    a = get_agent(jsonl, run_id, agent_id)
    if a is None:
        raise HTTPException(404, "agent not found")
    return a


@app.get("/api/sessions/events", dependencies=[Depends(require_auth)])
async def sessions_events():
    from app.sse import list_events
    return EventSourceResponse(list_events())


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str):
    # handler async -> registry.list() (subprocess tmux) vai pro threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    jsonl = next((s.jsonl for s in sessions if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    return EventSourceResponse(merged_events(name, jsonl))


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
def input_prompt(name: str, body: InputBody):
    try:
        result = terminal.send_prompt(name, body.text)
        # DIAG: correlaciona o send com o jsonl pra onde ESTE nome resolve AGORA -> pega o cross-wire
        # (msg indo pro transcript/terminal errado). Best-effort, nunca quebra o envio.
        try:
            from app import tmux as _tmux
            _cwd = next((p["cwd"] for p in _tmux.list_panes_active() if p["name"] == name), "")
            _j, _t = registry.resolve_tracked(name, _cwd)
            _log.info("SEND name=%s -> jsonl=%s tracked=%s result=%s text=%r",
                      name, (_j or "").rsplit("/", 1)[-1], _t, result, body.text[:80])
        except Exception:
            pass
    except ValueError as e:
        # send_prompt rejeita control chars (ex: '\n'). Sem isto virava 500 -> a msg sumia sem
        # feedback. Agora vira 400 limpo (o frontend mostra). (Multi-linha de verdade: backlog.)
        raise HTTPException(400, str(e))
    stripped = body.text.lstrip()
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
    else:
        # Registra na fila duravel (sidecar) sempre — aparece como user_msg em ordem e persiste no
        # reload; o merge dedup-a contra o transcript quando o Claude Code grava o prompt. delivered =
        # o send_prompt REALMENTE digitou ("sent"); pane em overlay -> "deferred" (nao tocou a TUI) e a
        # entrada fica pendente pro drain entregar quando o overlay fechar. Falha ao gravar a fila nao
        # quebra o envio.
        try:
            PromptQueue(name).append(body.text, delivered=(result == "sent"))
        except OSError:
            pass
        if result == "sent":
            # Confirmacao de entrega: em ~8s confere se o transcript gravou; engolida -> re-drena.
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
        else:
            # Kick: fecha a corrida append-depois-da-transicao — se o estado virou entregavel entre
            # o "deferred" do send_prompt e o append acima, o gatilho daquele ciclo nao viu esta
            # entrada (e sem SSE aberto nao havia gatilho nenhum). O drain re-checa deliverable.
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
    return {"ok": True}


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    terminal.select(name, body.option)
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
def interrupt(name: str, clear: bool = False):
    # clear=True: alem de interromper, limpa o input (2o Esc). So o front com msg pendente passa isso —
    # garante input nao-vazio, evitando que o Esc-Esc abra o menu de rewind num input ja vazio.
    terminal.interrupt(name, clear=clear)
    return {"ok": True}


@app.get("/api/sessions/{name}/pane", dependencies=[Depends(require_auth)])
def pane(name: str):
    # Pane CRU (texto ja composto pelo tmux: sem ANSI/cursor-move). O espelho do pane (TerminalMirror)
    # le isto pra mostrar overlays so-TUI (/status, /config, /help, pickers) que nao caem no .jsonl.
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(404, "sessao nao encontrada")
    return {"text": tmux.capture_pane(name)}


@app.post("/api/sessions/{name}/keys", dependencies=[Depends(require_auth)])
def keys(name: str, body: KeyBody):
    # Uma tecla de navegacao (allowlist) pro pane — dirige overlays so-TUI a partir do espelho.
    try:
        terminal.send_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.post("/api/sessions/{name}/term-input", dependencies=[Depends(require_auth)])
def term_input(name: str, body: TermInputBody):
    # Terminal interativo (so desktop): manda texto digitado (literal) e/ou uma tecla nomeada pro pane.
    try:
        if body.text:
            terminal.send_text(name, body.text)
        if body.key:
            terminal.send_term_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.post("/api/sessions/{name}/upload", dependencies=[Depends(require_auth)])
async def upload(name: str, request: Request):
    # Resolve o cwd da sessao (registry.list() ja traz cwd via tmux #{pane_current_path}).
    # handler async -> registry.list() (subprocess tmux) no threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, "sessao nao encontrada")
    if not info.cwd:
        raise HTTPException(409, "cwd da sessao indisponivel")
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, "arquivo maior que 100 MiB")
    data = await request.body()
    # Filename do cliente (X-Filename, percent-encoded) ou ?name= -> so a EXTENSAO e usada
    # (o nome final e gerado pelo servidor). Qualquer tipo de arquivo.
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        # write_bytes (ate 100 MiB) no threadpool pra nao bloquear o loop durante o disco.
        path = await asyncio.to_thread(save_upload, info.cwd, data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    return {"path": path}


@app.get("/api/sessions/{name}/uploads/{filename}", dependencies=[Depends(require_auth)])
def serve_upload(name: str, filename: str):
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
    try:
        path = resolve_upload(info.cwd, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    return FileResponse(path)


class CheckoutBody(_StrictBody):
    branch: str


class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop"]


class GitPathBody(_StrictBody):
    path: str   # validado em git_ops contra a lista real de arquivos alterados (anti-traversal)


def _session_cwd(name: str) -> str:
    # cwd da sessao tmux (mesmo lookup do upload). 404 se a sessao/cwd nao existe.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
    return info.cwd


@app.get("/api/sessions/{name}/branches", dependencies=[Depends(require_auth)])
def branches(name: str):
    try:
        return list_branches(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/checkout", dependencies=[Depends(require_auth)])
def checkout(name: str, body: CheckoutBody):
    try:
        return switch_branch(_session_cwd(name), body.branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git", dependencies=[Depends(require_auth)])
def git(name: str, body: GitActionBody):
    try:
        return git_action(_session_cwd(name), body.action)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/files", dependencies=[Depends(require_auth)])
def git_files(name: str):
    try:
        return {"files": changed_files(_session_cwd(name))}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/diff", dependencies=[Depends(require_auth)])
def git_diff(name: str, body: GitPathBody):
    try:
        return file_diff(_session_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/discard", dependencies=[Depends(require_auth)])
def git_discard(name: str, body: GitPathBody):
    try:
        return discard_file(_session_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/open-editor", dependencies=[Depends(require_auth)])
def open_editor(name: str):
    # So-desktop: abre o editor na MAQUINA do backend, no cwd da sessao. Binario fixo (settings.editor,
    # nao input do cliente) + arg unico validado -> sem shell, sem injecao. GUI precisa do DISPLAY/
    # WAYLAND_DISPLAY do backend (sessao grafica); sob systemd headless pode nao abrir -> 500.
    cwd = _session_cwd(name)
    try:
        subprocess.Popen([settings.editor, cwd],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError as e:
        raise HTTPException(500, f"editor '{settings.editor}' falhou: {e}")
    return {"ok": True}


@app.get("/api/sessions/{name}/transcript-image/{uuid}/{idx}", dependencies=[Depends(require_auth)])
def transcript_image(name: str, uuid: str, idx: int):
    # Serve uma imagem colada no TERMINAL (base64 no .jsonl) sob demanda. Decodifica por uuid+idx.
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import get_transcript_image
    got = get_transcript_image(jsonl, uuid, idx)
    if got is None:
        raise HTTPException(404, "image not found")
    raw, media = got
    # immutable: o conteudo de um uuid+idx nunca muda -> cache agressivo no cliente.
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


# ── Arquivo: conversas mortas (transcripts sem sessao tmux viva) ──────────────
@app.get("/api/archive", dependencies=[Depends(require_auth)], response_model=list[ArchiveFolder])
def archive_index():
    # Nivel 1: so as PASTAS (agregado barato). As conversas vem por pasta, no endpoint abaixo.
    return list_folders()


@app.get("/api/archive/{project}", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_folder(project: str):
    # live = transcripts em uso agora (badge na lista; a conversa viva abre pelo chat normal).
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    try:
        return list_conversations(project, live)
    except ValueError:
        raise HTTPException(400, "invalid path")
    except FileNotFoundError:
        raise HTTPException(404, "project not found")


@app.get("/api/archive/{project}/{session_id}/history",
         dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
def archive_history(project: str, session_id: str):
    try:
        p = archive_jsonl(project, session_id)
    except ValueError:
        raise HTTPException(400, "invalid path")
    except FileNotFoundError:
        raise HTTPException(404, "transcript not found")
    from app.pqueue import merged_history
    # Nome de fila inexistente -> sem entradas de fila: so os eventos do transcript, ordenados por ts.
    return merged_history("__archive__", str(p))


@app.get("/api/archive/{project}/{session_id}/transcript-image/{uuid}/{idx}",
         dependencies=[Depends(require_auth)])
def archive_image(project: str, session_id: str, uuid: str, idx: int):
    try:
        p = archive_jsonl(project, session_id)
    except (ValueError, FileNotFoundError):
        raise HTTPException(404, "not found")
    from app.transcript import get_transcript_image
    got = get_transcript_image(str(p), uuid, idx)
    if got is None:
        raise HTTPException(404, "image not found")
    raw, media = got
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


@app.get("/api/sessions/{name}/file", dependencies=[Depends(require_auth)])
def serve_file(name: str, path: str):
    # Serve QUALQUER arquivo referenciado na conversa (video/html/codigo/pdf/...). TRAVA de seguranca:
    # so serve se o `path` aparece no transcript desta sessao (citado por voce ou pelo Claude =
    # consentido) E existe E e arquivo regular -> bloqueia leitura arbitraria de disco / path-traversal.
    # FileResponse trata Range -> <video> faz seek/streaming.
    # Path RELATIVO (ex "./mock.png", "sub/x.png") resolve contra o CWD DA SESSAO (onde o Claude criou
    # o arquivo), nao o cwd do processo backend; guard extra: o resolvido nao pode ESCAPAR do cwd.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import path_in_transcript
    if not path_in_transcript(info.jsonl, path):
        raise HTTPException(403, "file not referenced in this conversation")
    expanded = os.path.expanduser(path)
    if os.path.isabs(expanded):
        real = os.path.realpath(expanded)
    else:
        if not info.cwd:
            raise HTTPException(409, "cwd da sessao indisponivel")
        base = os.path.realpath(info.cwd)
        real = os.path.realpath(os.path.join(base, expanded))
        if real != base and not real.startswith(base + os.sep):
            raise HTTPException(403, "path escapes session cwd")
    if not os.path.isfile(real):
        raise HTTPException(404, "file not found")
    media = mimetypes.guess_type(real)[0] or "application/octet-stream"
    return FileResponse(real, media_type=media)


class AnswerItem(_StrictBody):
    kind: str
    indices: list[int] | None = None
    multi: bool = False
    value: str | None = None
    labels: list[str] = []
    type_index: int | None = None
    chat_index: int | None = None


class AnswerBody(_StrictBody):
    answers: list[AnswerItem]


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth)])
def answer(name: str, body: AnswerBody):
    # Dirige o AskUserQuestion tabbed: reproduz as teclas, confere o Review, submete ou 409.
    from app import terminal_input
    try:
        terminal_input.answer_questions(name, [a.model_dump() for a in body.answers])
    except ValueError as e:
        raise HTTPException(409, str(e))
    # Respondido: limpa o sidecar do hook pra um stale nao reabrir o stepper depois. Resolve o jsonl
    # igual aos outros endpoints; se nao resolver, pula a limpeza sem falhar a request.
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if jsonl:
        clear_pending_askq(jsonl)
    return {"ok": True}


@app.post("/api/sessions/{name}/model-effort", dependencies=[Depends(require_auth)])
def model_effort(name: str, body: ModelEffortBody):
    # Dirige o picker interativo do /model pra aplicar modelo/esforco SO na sessao (scope
    # 'session') ou como default ('default'). PickerError -> 409/422; entrada invalida -> 422.
    try:
        return terminal.set_model_effort(name, body.model, body.effort, body.scope)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    except ValueError as e:
        raise HTTPException(422, str(e))


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


@app.get("/api/sessions/{name}/commands", dependencies=[Depends(require_auth)])
def commands(name: str):
    # cwd vem do registry/tmux; se a sessao nao for achada, ainda devolvemos os built-ins
    # + skills globais (lista util mesmo sem cwd casado).
    cwd = next((s.cwd for s in registry.list() if s.name == name), None)
    return list_commands(cwd)
