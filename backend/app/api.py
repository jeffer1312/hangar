import asyncio
import logging
import mimetypes
import os
import re
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Literal, Optional
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
from app.names import sanitize_session_name
from app.models import (SessionInfo, ChatEvent, CostReport, RunnersResponse, RunBody, RunInfo,
                        ProjectStatus)
from app.pqueue import PromptQueue, _transcript_start_ts, committed_user_lines
from app.chain import ThenLink
from app.terminal_input import TerminalInput, drain
from app.adapters import get_adapter
from app.adapters.codex import sessions as codex_sessions
from app.sse import merged_events
from app.uploads import save_upload, resolve_upload, prune_old, list_uploads, UploadError, MAX_BYTES
from app.video import is_video, extract_frames, extract_audio
from app.transcribe import transcribe, TranscribeError
from app.config import list_config_dirs, ConfigDirInfo, _backend_config_base, settings, automations_enabled
from app import runtime_config
from app.costs import report as costs_report
from app.git_ops import (
    list_branches, switch_branch, git_action, git_log, assign_lanes, changed_files, file_diff, discard_file, commit_files, commit_file_diff, commit, push as push_branch, GitError, branch_of,
)
from app import loop as loop_mod
from app.transcript import last_assistant_text
from app import tunnel
from app import runner
from app import projects
from app.archive import ArchiveEntry, ArchiveFolder, archive_cwd, archive_jsonl, list_conversations, list_folders
from app.search import SearchHit, search, extract_terms, search_terms, build_ask_prompt
from app.askquestion import clear_pending_askq, read_pending_askq
from app import pair
from app import peers
from app.pair import PairLink, contract_path_for
from app.hook_state import hook_state
from app import push
from app import stall_watch
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

    stall_task = asyncio.create_task(stall_watch.watch())

    def _stall_watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("stall_watch.watch crashed", exc_info=exc)

    stall_task.add_done_callback(_stall_watch_done)

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
                m = hook_state.get_state(Path(info.jsonl).stem) if info.jsonl else None
                if m and m[0] == "idle":
                    loop_mod.schedule_tick(info.name, lambda n=info.name: _loop_ctx(n))
        except Exception:
            _log.warning("boot-resume de loops falhou", exc_info=True)

    await asyncio.to_thread(_boot_resume_loops)
    try:
        yield
    finally:
        task.cancel()
        stall_task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        try:
            await stall_task
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

# Snapshot com TTL de registry.list() pros endpoints request/response QUENTES (history/workflows):
# o mount do board dispara dezenas de /history de uma vez e cada list() fresco e um scan completo
# de /proc + fork de tmux list-panes. Mesmo padrao do sse._list_snap (la pros loops de SSE; caches
# separados porque as instancias de SessionRegistry sao separadas). Miss por nome (sessao criada ha
# <1s) -> fallback pro list() fresco, entao o TTL nunca causa 404 falso.
_LIST_TTL = 1.0
_list_snap: dict = {"t": 0.0, "infos": None}


async def _cached_info(name: str) -> SessionInfo | None:
    now = time.monotonic()
    if _list_snap["infos"] is None or now - _list_snap["t"] >= _LIST_TTL:
        _list_snap["infos"] = await asyncio.to_thread(registry.list)
        _list_snap["t"] = time.monotonic()
    info = next((s for s in _list_snap["infos"] if s.name == name), None)
    if info is None:
        _list_snap["infos"] = await asyncio.to_thread(registry.list)
        _list_snap["t"] = time.monotonic()
        info = next((s for s in _list_snap["infos"] if s.name == name), None)
    return info


def _notify_async(session_id: str, send_fn) -> None:
    """Resolve uuid->nome e manda o push escolhido, TUDO numa thread: registry.list() mexe no tmux
    e o webpush e rede — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            name = next(
                (s.name for s in registry.list() if s.jsonl and Path(s.jsonl).stem == session_id),
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
    da TUI, que nao passam pelo AskUserQuestion); 3) fallback estatico se nenhuma deu certo."""
    askq = read_pending_askq(info.jsonl) if info.jsonl else None
    if askq and askq.questions:
        return askq.questions[0].question
    if info.name:
        from app import tmux
        from app.state import classify
        try:
            _, _, question, _ = classify(tmux.capture_pane(info.name))
        except Exception:
            question = None
        if question:
            return question
    return "Aguardando sua resposta"


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
    info = next((s for s in registry.list() if s.jsonl and Path(s.jsonl).stem == session_id), None)
    if info is None:
        return
    def _real() -> bool:
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
            _working_started[session_id] = m[1]
    elif state == "idle":
        started = _working_started.pop(session_id, None)
        if started is not None and runtime_config.get("notify_finished"):
            m = hook_state.get_state(session_id)
            elapsed = (m[1] if m else time.time()) - started
            if elapsed >= runtime_config.get("finish_min_seconds"):
                _notify_async(session_id, push.notify_finished)
    elif state == "dead":
        _working_started.pop(session_id, None)
        if runtime_config.get("notify_dead"):
            _notify_async(session_id, push.notify_dead)

    if state == "awaiting_input":
        # Loop runner: awaiting cobre pedido de permissao tb -> pausa o loop (retoma no idle seguinte).
        # Thread propria (registry.list toca tmux); sem push proprio (o _on_awaiting ja empurra).
        def _pause_loop() -> None:
            try:
                info = next((s for s in registry.list()
                             if s.jsonl and Path(s.jsonl).stem == session_id), None)
                if info:
                    with loop_mod._lock:
                        link = loop_mod.LoopLink(info.name)
                        d = link.get()
                        if d and d["status"] == "running":
                            link.update(status="paused_awaiting")
            except Exception:
                pass
        threading.Thread(target=_pause_loop, daemon=True).start()
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
                # Loop runner: no idle, se ha loop ativo e o drain NAO acabou de digitar algo
                # (sent == 0 -> este idle e fim de turno de trabalho, nao o eco do goal/re-prompt),
                # tica o loop. Loop ativo SUPRIME o chain (senao cada idle entre iteracoes dispararia).
                loop_d = loop_mod.LoopLink(info.name).get()
                loop_active = loop_d is not None and loop_d["status"] in loop_mod.ACTIVE
                if state == "idle" and loop_active and sent == 0:
                    loop_mod.schedule_tick(info.name, lambda: _loop_ctx(info.name))
                # Encadeamento (feature #12): so quando NAO ha loop ativo — turno REALMENTE terminado,
                # reusando o info.name ja resolvido nesta thread — ver _maybe_chain.
                if state == "idle" and not loop_active:
                    _maybe_chain(info.name)
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
    # Qual Adapter cria a sessao (app.adapters.get_adapter). Default "claude" preserva o
    # comportamento de hoje pros clientes que ainda nao mandam o campo.
    provider: str = "claude"
    # Wrapper interativo do Codex pode iniciar a TUI ja com um prompt. Nao e argv arbitrario:
    # evita que um cliente remoto injete flags que afrouxem sandbox/aprovacoes do backend.
    initial_prompt: str | None = None


class PushSubscribeBody(_StrictBody):
    subscription: dict  # PushSubscription do browser: {endpoint, keys:{p256dh, auth}}
    label: str = Field(min_length=1)    # nome do servidor escolhido no celular (Casa/my-org)
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


class BroadcastBody(_StrictBody):
    names: list[str] = Field(min_length=1)
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
async def create_session(body: CreateBody):
    # handler async pra poder `await registry.create_codex` (precisa viver no loop principal —
    # ver docstring de create_codex). O caminho Claude (registry.create) e SINCRONO e spawna um
    # subprocess tmux (bloqueante) -> rodar direto aqui travaria o event loop / o SSE de outras
    # sessoes; vai pro threadpool via asyncio.to_thread, igual aos outros handlers async deste
    # arquivo que chamam registry.list()/save_upload (menor risco de regressao: comportamento e
    # exceções do create() Claude ficam IDENTICOS, so a chamada muda de sync p/ thread).
    if body.provider not in ("claude", "codex"):
        raise HTTPException(400, "provider invalido")
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, "config_dir invalido")
    try:
        if body.provider == "codex":
            return await registry.create_codex(body.name, body.cwd, body.initial_prompt)
        return await asyncio.to_thread(registry.create, body.name, body.cwd, body.config_dir)
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
    new = sanitize_session_name(body.new)
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


class ThenLinkBody(_StrictBody):
    target: str = Field(min_length=1)
    text: str = Field(min_length=1)


@app.put("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def set_then_link(name: str, body: ThenLinkBody):
    """Arma o vinculo 'then' (feature #12): quando `name` confirmar idle (turno terminado), `body.text`
    e enviado pra `body.target` -- ver app.chain.ThenLink e app.api._maybe_chain. Um hop so (nao DAG):
    setar de novo so troca alvo/texto, nao encadeia mais niveis."""
    from app import tmux
    if body.target == name:
        raise HTTPException(400, "sessao nao pode encadear pra si mesma")
    if not tmux.has_session(body.target):
        raise HTTPException(404, "sessao alvo nao encontrada")
    ThenLink(name).set(body.target, body.text)
    return {"ok": True}


@app.delete("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def clear_then_link(name: str):
    ThenLink(name).clear()
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

    def deliver(prompt: str) -> bool:
        PromptQueue(name).append(prompt, delivered=False)
        drain(name, jsonl)
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


@app.post("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_create(name: str, body: LoopCreate):
    info = next((i for i in registry.list() if i.name == name), None)
    if info is None:
        raise HTTPException(404, "sessão não encontrada")
    if getattr(info, "provider", "claude") != "claude":
        # Codex e outros nao sao tmux: sem hook de transicao, o tick nunca dispara -> loop ficaria
        # running mudo pra sempre. Recusa cedo em vez de criar um loop-zumbi.
        raise HTTPException(409, "loop runner só suporta sessões claude")
    if not automations_enabled():
        raise HTTPException(409, "automações desligadas (kill-switch)")
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur and cur["status"] in loop_mod.ACTIVE:
            raise HTTPException(409, "já existe um loop ativo nesta sessão")
        br = branch_of(info.cwd) if info.cwd else None
        if body.require_branch and br in ("main", "master"):
            raise HTTPException(409, f"sessão está na branch {br} — crie uma branch ou desligue 'exigir branch'")
        d = loop_mod.new_loop(body.goal, body.check_cmd, body.max_iters, body.require_branch)
        entry = PromptQueue(name).append(body.goal, delivered=False)
        d["goal_entry_id"] = entry["id"]
        link.set(d)
    if info.jsonl:
        drain(name, info.jsonl)   # entrega ja se a sessao estiver entregavel; senao o drain server-side entrega depois
    return {"loop": link.get()}


@app.get("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_get(name: str):
    info = next((i for i in registry.list() if i.name == name), None)
    suggestions = loop_mod.suggest_checks(info.cwd) if info and info.cwd else []
    return {"loop": loop_mod.LoopLink(name).get(), "suggestions": suggestions}


@app.delete("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_stop(name: str):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        if link.get() is None:
            raise HTTPException(404, "nenhum loop nesta sessão")
        d = loop_mod._end(link, name, "stopped", "parado pelo usuário", push.notify_loop)
    return {"loop": d}


@app.post("/api/sessions/{name}/loop/refine", dependencies=[Depends(require_auth)])
def loop_refine(name: str, body: LoopRefine):
    """Refina o objetivo do loop via claude -p efemero (sonnet). Stateless — nao toca a sessao nem o
    sidecar; o {name} da rota so mantem a familia de URLs consistente. Falha do CLI -> 502.
    Sob o kill-switch mestre: refine dispara um agente autonomo, entao respeita automations_enabled."""
    if not automations_enabled():
        raise HTTPException(409, "automações desligadas (kill-switch)")
    try:
        return {"goal": loop_mod.refine_goal(body.goal, body.check_cmd)}
    except loop_mod.ClaudePError as e:
        _log.warning("loop/refine falhou (%s): %s", name, e)
        raise HTTPException(502, str(e))


@app.post("/api/sessions/{name}/loop/resolve", dependencies=[Depends(require_auth)])
def loop_resolve(name: str, body: LoopResolve):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur is None or cur["status"] != "done_claimed":
            raise HTTPException(409, "loop não está aguardando confirmação")
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
async def history(name: str, limit: int | None = None):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.pqueue import merged_history
    # provider: o rollout do Codex tem um shape DIFERENTE do jsonl do Claude (ver
    # app.adapters.codex.rollout) -- sem isto merged_history tentava o parser do Claude em toda
    # linha do rollout, nunca casava e devolvia [] (chat do Codex abria vazio ate o SSE encher via
    # backfill do tail; reabrir apos ficar horas em segundo plano perdia o que passou do tail-200).
    # Com limit, merged_history faz tail-read (parseia so o fim do arquivo); to_thread porque o
    # parse (mesmo da cauda) e CPU/IO sincrono.
    evs = await asyncio.to_thread(merged_history, name, info.jsonl, info.provider, limit)
    # Cauda CRUA de proposito: o corte no 1o
    # user_msg (pra nao desenhar resposta orfa) e preferencia de RENDERIZACAO do card do quadro e vive
    # no BoardCard.svelte. Aplicado AQUI, valia pra todo consumidor e matava a espiada do hover da
    # Sidebar (HP_TAIL=8), que so quer o ultimo assistant_msg: com o proximo prompt ja mandado, o corte
    # jogava fora a resposta anterior -> latestAssistantEvent = None -> popover vazio, cacheado por 30s.
    if limit is not None and limit > 0:
        return evs[-limit:]
    return evs


@app.get("/api/sessions/{name}/workflows", dependencies=[Depends(require_auth)])
async def workflows_list(name: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import list_workflows
    return await asyncio.to_thread(list_workflows, info.jsonl)


@app.get("/api/sessions/{name}/workflows/{run_id}", dependencies=[Depends(require_auth)])
async def workflow_detail(name: str, run_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_workflow
    wf = await asyncio.to_thread(get_workflow, info.jsonl, run_id)
    if wf is None:
        raise HTTPException(404, "workflow run not found")
    return wf


@app.get("/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}", dependencies=[Depends(require_auth)])
async def workflow_agent_detail(name: str, run_id: str, agent_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_agent
    a = await asyncio.to_thread(get_agent, info.jsonl, run_id, agent_id)
    if a is None:
        raise HTTPException(404, "agent not found")
    return a


@app.get("/api/sessions/events", dependencies=[Depends(require_auth)])
async def sessions_events():
    from app.sse import list_events
    return EventSourceResponse(list_events())


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str, request: Request):
    # handler async -> registry.list() (subprocess tmux) vai pro threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
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
        if stem and stem == Path(info.jsonl).stem:
            try:
                start_offset = int(off)
            except ValueError:
                start_offset = None
    # provider da sessao (SessionInfo.provider, ja marcado por registry.list() -- tmux -> "claude",
    # sidecar Codex -> "codex") -> merged_events escolhe o Adapter certo (tail do jsonl, monitor de
    # estado, fonte do preview). Sem isto TODA sessao caia no default "claude" do merged_events e o
    # SSE do Codex nunca ligava (chat vazio, sem estado ao vivo).
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider=info.provider, start_offset=start_offset))


# Pool DEDICADO ao caminho de ENVIO (nucleo sagrado). Separado do executor default do asyncio, que a
# decoracao da lista (git_summary/capture_pane via asyncio.to_thread) pode ocupar em rajada -> sem isto,
# um burst de decoracao lenta atrasaria o POST /input. Poucos workers bastam (single-user; envios a uma
# mesma sessao ja serializam no _send_lock do terminal_input).
_send_executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="cp-send")


def _send_thread(fn, *args):
    """Roda `fn(*args)` no pool DEDICADO de envio (nao no executor default, saturavel pela decoracao)."""
    return asyncio.get_running_loop().run_in_executor(_send_executor, fn, *args)


def _send_one(name: str, text: str) -> dict:
    """Sequencia UNICA de envio de prompt: send_prompt + registro na fila duravel + confirmacao/drain.
    Usada pelo /input (uma sessao) e pelo /broadcast (loop por N sessoes) — o broadcast NAO reimplementa
    entrega, so repete esta mesma sequencia por nome. Nunca levanta (devolve ok/error) pra o broadcast
    reportar falha de uma sessao sem abortar as demais."""
    # ts da entrada carimbado ANTES do send: o send_prompt digita + Enter e o Claude Code grava o
    # prompt no transcript NA HORA, entao o append la embaixo roda DEPOIS do commit. Carimbar no
    # append punha a entrada ~ms apos o proprio commit e o dedup ts-aware do merged_history a
    # mantinha pendente (msg em dobro no historico ate o reconcile). A ordem send->append->drain
    # NAO muda — so o valor gravado, que e o unico dado que o dedup le.
    t0 = time.time()
    try:
        result = terminal.send_prompt(name, text)
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
        # feedback. Agora vira 400 limpo (o frontend mostra). (Multi-linha de verdade: backlog.)
        return {"ok": False, "error": str(e)}
    stripped = text.lstrip()
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
            PromptQueue(name).append(text, delivered=(result == "sent"), ts=t0)
        except OSError as e:
            if result != "sent":
                # NAO digitado na TUI (overlay/picker aberto) + sidecar nao gravou = a msg nao esta em
                # lugar NENHUM. Era aqui que o "ok, na fila" mentia: 200 + delivered=False pra uma msg
                # que sumiu. Vira erro (o front mostra), nunca sucesso.
                _log.exception("fila indisponivel e prompt NAO digitado name=%s", name)
                return {"ok": False, "error": f"fila indisponivel e prompt nao foi digitado: {e}"}
            # Digitado na TUI: a msg CHEGOU, o envio nao falhou. Perder o registro so desliga a rede de
            # seguranca (o _confirm_and_drain abaixo nao vai achar o que reconferir) — nao e motivo pra
            # falhar o envio, mas nao pode passar calado.
            _log.exception("append na fila falhou (prompt ja digitado) name=%s", name)
        if result == "sent":
            # Confirmacao de entrega: em ~8s confere se o transcript gravou; engolida -> re-drena.
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
        else:
            # Kick: fecha a corrida append-depois-da-transicao — se o estado virou entregavel entre
            # o "deferred" do send_prompt e o append acima, o gatilho daquele ciclo nao viu esta
            # entrada (e sem SSE aberto nao havia gatilho nenhum). O drain re-checa deliverable.
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
    # delivered: digitou AGORA na TUI ("sent"); False = ficou na fila durável (sessão ocupada/overlay).
    return {"ok": True, "error": None, "delivered": result == "sent"}


def _provider_of(name: str) -> str:
    """Resolve o provider de uma sessao PELO NOME, barato e sem tmux: sidecar Codex existe -> "codex",
    senao "claude". Default "claude" preserva 100% o caminho tmux de hoje pra qualquer nome que nao seja
    de uma sessao Codex conhecida (regra de ouro: Claude identico, tudo Codex e ramo condicional)."""
    return "codex" if codex_sessions.exists(name) else "claude"


async def _send_one_codex(name: str, text: str) -> dict:
    """Envio de prompt pra sessao Codex pela TUI no tmux. Registra na fila duravel
    (aparece como user_msg em ordem e persiste no reload; o
    merge dedup-a contra o rollout do Codex) e entrega pela TUI no tmux SE a sessao esta idle;
    senao deixa pendente pro drain-on-complete entregar quando o turno terminar. Codex nao tem
    slash-commands do Claude -> envia o texto como esta. Nunca levanta (mesmo contrato do _send_one pro
    broadcast: devolve ok/error por sessao).

    IMPORTANT 2: PromptQueue.append/set_delivered fazem I/O de arquivo sincrono com lock -- chamados
    direto aqui (corrotina) bloqueariam o event loop. Mesmo padrao de to_thread do drain do Codex."""
    adapter = get_adapter("codex")
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
        entry = await asyncio.to_thread(PromptQueue(name).append, text, delivered=False)
    except OSError as e:
        # Mesma regra do _send_one: sidecar nao gravou + NAO entregavel = a msg nao esta em lugar
        # NENHUM, e responder "ok, na fila" era a mentira que o eeba30a tirou do caminho Claude.
        # Vira erro (o front mostra), nunca sucesso.
        if not deliverable:
            _log.exception("fila indisponivel e prompt NAO entregue name=%s", name)
            return {"ok": False, "error": f"fila indisponivel e prompt nao foi entregue: {e}"}
        # Entregavel: a TUI abaixo ainda leva o texto, entao a msg CHEGA. Perder o registro so
        # desliga a rede de seguranca (o drain-on-complete nao acha o que reconferir) — nao e motivo
        # pra falhar o envio, mas nao pode passar calado.
        _log.exception("append na fila falhou (prompt sera entregue) name=%s", name)
        entry = None
    if not deliverable:
        # turno em andamento -> fica pendente na fila; o drain-on-complete entrega no proximo idle.
        return {"ok": True, "error": None, "delivered": False}
    try:
        result = await adapter.send_prompt(name, text)
    except Exception as e:
        _log.exception("codex send_prompt falhou name=%s", name)
        return {"ok": False, "error": str(e)}
    if result == "sent":
        if entry is not None:
            # turno iniciou -> marca entregue pra o drain-on-complete nao reenviar a mesma entrada.
            try:
                await asyncio.to_thread(PromptQueue(name).set_delivered, entry["id"], True)
            except OSError:
                pass
    elif entry is None:
        # "deferred" (corrida idle->working entre o deliverable e o send) + sidecar morto: o texto NAO
        # foi digitado E nao ha entrada pendente pro drain-on-complete drenar -- a msg nao esta em lugar
        # NENHUM. Aqui morre a suposicao do append la em cima ("entregavel -> a TUI leva o texto"):
        # o deferred e exatamente o caso em que nao levou. Ultimo ponto onde o 200 "na fila"
        # ainda seria a mentira do eeba30a.
        _log.error("prompt deferido sem entrada na fila — NAO foi entregue name=%s", name)
        return {"ok": False, "error": "fila indisponivel e o turno nao aceitou o prompt: nao foi entregue"}
    # "deferred" COM entrada na fila: fica pendente (delivered ja e False) -> drain-on-complete entrega.
    return {"ok": True, "error": None, "delivered": result == "sent"}


def _session_exists(name: str) -> bool:
    """Sessão existe DE VERDADE (pane tmux vivo ou sidecar Codex)? Sem esta guarda o /input aceitava
    qualquer nome e enfileirava no VOID: 'ok' pra sessão morta = recado órfão que só seria entregue
    se um dia nascesse outra sessão com o mesmo nome (foi exatamente como um recado 'se perdeu')."""
    from app import tmux
    return codex_sessions.exists(name) or tmux.has_session(name)


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
async def input_prompt(name: str, body: InputBody):
    # Ramifica por provider: Claude via _send_thread (_send_one e SYNC/bloqueante — tmux), Codex via
    # _send_one_codex (async, app-server). Default "claude" pra qualquer nome nao-Codex.
    # NUCLEO SAGRADO: o envio NUNCA disputa executor com feature. _send_thread e um pool DEDICADO
    # (nao o default do asyncio, que a decoracao — git_summary/capture_pane — pode ocupar). Assim um
    # git status pendurado + refine (60s, pool do anyio) + check (600s, thread propria) nao seguram
    # o POST /input. Ver _send_thread.
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, "sessão não encontrada — recado NÃO enfileirado")
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, body.text)
    else:
        res = await _send_thread(_send_one, name, body.text)
    if not res["ok"]:
        raise HTTPException(400, res["error"])
    # delivered: True = digitou agora na TUI; False = na fila durável (entrega no próximo idle).
    return {"ok": True, "delivered": res.get("delivered", False)}


@app.post("/api/broadcast", dependencies=[Depends(require_auth)])
async def broadcast(body: BroadcastBody):
    """Fan-out de UM prompt pra N sessoes (feature #9): mesma sequencia do /input, em loop — sessao
    ocupada enfileira na fila duravel dela (crash-safe), sessao ociosa recebe na hora, sem mecanismo
    de entrega novo. Ramifica por provider por nome (Claude via to_thread, Codex via _send_one_codex),
    reportando por-sessao sem abortar as demais. Slash-commands ficam FORA (rota por sessao so):
    "/clear" pra N sessoes de uma vez e ambiguo/perigoso (o front ja desabilita o envio; isto e defesa
    em profundidade)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, "broadcast nao suporta slash-commands: envie por sessao")
    results: dict[str, dict] = {}
    for name in body.names:
        # Mesma guarda do /input: nome sem sessão viva -> erro POR SESSÃO (não enfileira no void).
        if not await _send_thread(_session_exists, name):
            results[name] = {"ok": False, "error": "sessão não encontrada", "delivered": False}
            continue
        if _provider_of(name) == "codex":
            results[name] = await _send_one_codex(name, body.text)
        else:
            results[name] = await _send_thread(_send_one, name, body.text)
    return {"results": results}


class PairBody(_StrictBody):
    # peer (1) OU peers (N) — peers vence; peer fica por compat (cp-send --pair manda um só).
    peer: str = ""
    peers: list[str] = []
    task: str = ""


def _group_text(me: str, others: list[str], task: str) -> str:
    t = f" na tarefa: {task.strip()}" if task.strip() else ""
    quem = ", ".join(f"'{o}'" for o in others)
    exemplo = others[0]
    # Par remoto (srv::sessao): contrato compartilhado não sincroniza cross-server no MVP — some a
    # linha do arquivo (cada máquina teria o seu, com gid diferente). cp-send já roteia srv::sessao.
    cross = any(peers.is_remote(o) for o in others)
    contrato = "" if cross else (
        f"Contrato/decisões que o grupo precisa consultar: registrar no arquivo compartilhado "
        f"{contract_path_for(me)} (markdown; criar se não existir, manter curto e atual). ")
    return (
        f"[de: claude-pocket] GRUPO DE TRABALHO ATIVO: você ('{me}') trabalha junto com {quem}{t}. "
        f"Cada sessão mexe SÓ no próprio repo; quando precisar de algo de outro membro (contrato, "
        f"endpoint, tipo, dúvida), mande 1:1 por iniciativa própria via Bash: "
        f'cp-send {exemplo} "sua mensagem" — recados 1:1 chegam como [de: <membro>]. '
        f'AVISO pro grupo TODO (marco: "terminei minha parte", "contrato atualizado"): '
        f'cp-send --group "sua mensagem" (uma vez, chega como [grupo: <membro>]). '
        f"REGRA ANTI-LOOP: NUNCA responda um [grupo: ...] com --group (vira tempestade). Aviso de "
        f"grupo é unidirecional; se precisar responder, faça 1:1 (cp-send <membro>) e só se necessário. "
        f"{contrato}"
        f"BRANCH: antes de trabalhar, rode git branch --show-current no SEU repo e alinhe pra "
        f"branch da PM da tarefa (fetch+checkout) — re-verifique após restart/resume da sessão. "
        f"Exceção única: o usuário pedir explicitamente outra branch. Checkout DUPLICADO do repo "
        f"na máquina → alerte o usuário e pergunte qual é o canônico antes de mexer. "
        f"Commit/push e decisões de rumo continuam com o usuário. Confirme em uma linha."
    )


async def _deliver(name: str, text: str) -> str | None:
    # Mesma esteira do /input (fila durável se ocupada), ramificada por provider.
    # Devolve o erro (str) ou None — _send_one/_send_one_codex NUNCA levantam, reportam no dict;
    # engolir isso fazia o pareamento dizer "ok" com o aviso jamais entregue.
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, text)
    else:
        res = await _send_thread(_send_one, name, text)
    return None if res.get("ok") else (res.get("error") or "falha desconhecida no envio")


@app.post("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def pair_session(name: str, body: PairBody):
    """Junta `name` e peer(s) num GRUPO de trabalho (une os grupos existentes de todos) e injeta
    em CADA membro o prompt do grupo atualizado — a partir daí trocam recados via cp-send por
    iniciativa própria, dentro do escopo da tarefa. Badge `pair_peers` aparece na lista."""
    others = [p for p in dict.fromkeys(body.peers or ([body.peer] if body.peer else [])) if p]
    if not others:
        raise HTTPException(400, "informe peer ou peers")
    if name in others:
        raise HTTPException(400, "não dá pra parear uma sessão com ela mesma")
    if any(peers.is_remote(o) for o in others):
        # Cross-server é 1:1 puro (um peer remoto, sem misturar grupo local) — grupo cross-server de
        # N fica pra fase 2. ponytail: 1:1 cobre "trabalhar junto entre máquinas"; N quando doer.
        if len(others) != 1:
            raise HTTPException(400, "pareamento cross-server é 1:1 por enquanto: um peer remoto, "
                                     "sem misturar com grupo local")
        if not settings.server_id:
            raise HTTPException(400, "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                     "pareamento cross-server (é o endereço de resposta srv::sessao)")
        return await _pair_cross_server(name, others[0], body.task)
    names = {s.name for s in await asyncio.to_thread(registry.list)}
    missing = [p for p in [name, *others] if p not in names]
    if missing:
        raise HTTPException(404, f"sessão não encontrada: {', '.join(missing)}")
    # join_group: snapshot + join na MESMA seção crítica (em seções separadas, um join concorrente
    # na janela entre elas entrava no grupo fora do snapshot e um rollback posterior não o
    # reverteria). O snapshot volta pra cá pra desfazer se o aviso não chegar em ninguém.
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, others, body.task)
    except pair.PairMixError as e:
        # Uma das sessões locais já está pareada cross-server (1:1) — não dá pra fundir em grupo local.
        raise HTTPException(400, str(e))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", body.task)
    errs = []
    for m in members:
        e = await _deliver(m, _group_text(m, [x for x in members if x != m], task))
        if e:
            errs.append(f"{m}: {e}")
    if len(errs) == len(members):
        # NINGUÉM foi avisado -> grupo fantasma; restaura o estado anterior e reporta.
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, f"pareamento desfeito: falha ao avisar as sessões ({'; '.join(errs)})")
    # Falha parcial: grupo vale (AO MENOS 1 membro sabe), e o warning nomeia quem ficou sem aviso
    # — o front mostra em vez de fingir sucesso total.
    return {"ok": True, "members": members,
            "warning": ("aviso falhou em: " + "; ".join(errs)) if errs else None}


async def _pair_cross_server(name: str, peer: str, task: str) -> dict:
    """Pareamento 1:1 entre máquinas. Registra o vínculo LOCAL (name.json peers=[srv::sessao];
    sidecar do remoto vive na máquina dele) e chama o /pair-remote do backend peer pra registrar o
    reverso + injetar o protocolo lá. Falha na chamada remota desfaz o vínculo local (mesmo racional
    do 'grupo fantasma' do pair local). Transporte já provado pelo cp-send cross-server (peers.json)."""
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, f"sessão não encontrada: {name}")
    srv, sess = peers.split_addr(peer)
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [peer], task)
    except pair.PairMixError as e:
        # `name` já está num grupo local (ou já pareada cross-server): não dá pra cross-parear.
        raise HTTPException(400, str(e))
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
            raise HTTPException(502, f"pareamento NÃO confirmado (falha de rede com '{srv}'): desfeito "
                                     f"deste lado; se o peer tiver ficado pareado, rode unpair lá. ({e})")
        raise HTTPException(502, f"pareamento desfeito (peer rejeitou): {e}")
    # Reverso registrado. Injeta o protocolo NESTE lado; se este falhar (sessão morreu na janela), o
    # vínculo já vale dos dois lados — só avisa, não desfaz (o par remoto já sabe).
    warn = None
    e = await _deliver(name, _group_text(name, [peer], task))
    if e:
        warn = f"vínculo criado, mas o aviso local falhou ({name}: {e}) — refaça o pair se precisar"
    return {"ok": True, "members": members, "warning": warn}


class PairRemoteBody(_StrictBody):
    initiator: str        # 'srv::nome' — quem iniciou o pareamento, na máquina remota
    task: str = ""


@app.post("/api/sessions/{name}/pair-remote", dependencies=[Depends(require_auth)])
async def pair_remote(name: str, body: PairRemoteBody):
    """Lado RECEPTOR do pareamento cross-server: registra `name` (sessão LOCAL) pareada ao iniciador
    remoto `body.initiator` (srv::nome) e injeta o protocolo. NÃO chama de volta (o iniciador já
    registrou o próprio lado — chamar de volta recursaria). Chamado só pelo backend do outro server
    via peers.call, autenticado pelo token do peers.json."""
    if not peers.is_remote(body.initiator):
        raise HTTPException(400, "initiator precisa ser qualificado (srv::nome)")
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, f"sessão não encontrada: {name}")
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [body.initiator], body.task)
    except pair.PairMixError as e:
        # `name` já está num grupo local aqui — não pode virar par cross-server de outra máquina.
        raise HTTPException(409, str(e))
    e = await _deliver(name, _group_text(name, [body.initiator], body.task))
    if e:
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, f"pareamento desfeito: falha ao avisar '{name}': {e}")
    return {"ok": True, "members": members}


class UnpairRemoteBody(_StrictBody):
    peer: str             # 'srv::nome' que saiu do pareamento, na máquina remota


@app.post("/api/sessions/{name}/unpair-remote", dependencies=[Depends(require_auth)])
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
        e = await _deliver(name, f"[de: claude-pocket] '{body.peer}' saiu do pareamento. "
                                 "Volte a operar independente; use cp-send só quando o usuário pedir.")
        if e:
            warn = f"{name}: {e}"
    return {"ok": True, "warning": warn}


class GroupMsgBody(_StrictBody):
    text: str


@app.post("/api/sessions/{name}/group-message", dependencies=[Depends(require_auth)])
async def group_message(name: str, body: GroupMsgBody):
    """Aviso pro GRUPO todo (cp-send --group): entrega o texto a CADA companheiro de `name` numa
    tacada, como `[grupo: <name>]`. Unidirecional por contrato (o prompt instrui a NUNCA responder
    um [grupo:] com --group) — é o que impede o loop de N sessões se avisando em cascata.
    Slash-command fora (mesmo racional do /broadcast)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, "group-message não suporta slash-commands")
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    peers = link.get("peers") if link else None
    if not peers:
        raise HTTPException(404, "sessão não está num grupo")
    text = f"[grupo: {name}] {body.text}"
    results: dict[str, dict] = {}
    for p in peers:
        if not await _send_thread(_session_exists, p):
            results[p] = {"ok": False, "error": "sessão não encontrada", "delivered": False}
            continue
        if _provider_of(p) == "codex":
            results[p] = await _send_one_codex(p, text)
        else:
            results[p] = await _send_thread(_send_one, p, text)
    failed = [f"{n}: {r.get('error')}" for n, r in results.items() if not r.get("ok")]
    return {"ok": True, "peers": peers,
            "warning": ("falha em: " + "; ".join(failed)) if failed else None}


@app.get("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
def pair_contract(name: str):
    """Contrato compartilhado do GRUPO (markdown que os membros editam via fs; keyed pelo gid —
    estável quando membro entra/sai). 404 sem grupo; content vazio se ainda não existe."""
    p = contract_path_for(name)
    if p is None:
        raise HTTPException(404, "sessão não está pareada")
    link = PairLink(name).get() or {}
    try:
        content = p.read_text(encoding="utf-8")
    except OSError:
        content = ""
    return {"peers": link.get("peers", []), "path": str(p), "content": content}


@app.delete("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def unpair_session(name: str):
    """`name` SAI do grupo (os demais membros continuam entre si; grupo restante de 1 dissolve).
    Avisa quem saiu e quem ficou. Idempotente. Aviso que falhar NÃO refaz o vínculo (fora do grupo
    é o estado desejado) — só reporta no result."""
    expeers = await asyncio.to_thread(pair.leave, name)   # nome próprio: 'peers' é o módulo importado
    if not expeers:
        return {"ok": True, "warning": None}
    errs = []
    # Pares REMOTOS (srv::sessao): avisa o backend deles pra limpar o reverso — não dá pra _deliver
    # local numa sessão de outra máquina. Os locais são avisados no loop abaixo.
    for p in expeers:
        if not peers.is_remote(p):
            continue
        if not settings.server_id:
            errs.append(f"{p}: CP_SERVER_ID ausente — par remoto não avisado")
            continue
        srv, sess = peers.split_addr(p)
        try:
            await asyncio.to_thread(peers.call, srv, "POST",
                                    f"/api/sessions/{sess}/unpair-remote",
                                    {"peer": f"{settings.server_id}::{name}"})
        except peers.PeerError as ex:
            # `name` já saiu localmente (pair.leave acima); o peer não pôde ser avisado -> sidecar
            # remoto fica órfão até alguém desparear lá. Loga pra rastreabilidade (journalctl) além
            # do warning no result. ponytail: sem fila de retry durável — single-user, recuperável na
            # mão; se virar comum, enfileirar via pqueue como o /input faz.
            _log.warning("unpair: peer remoto '%s' não avisado (sidecar de lá fica órfão): %s", p, ex)
            errs.append(f"{p}: {ex}")
    e = await _deliver(name, "[de: claude-pocket] Você saiu do grupo de trabalho "
                             f"({', '.join(expeers)}). Volte a operar independente; use cp-send só "
                             "quando o usuário pedir.")
    if e:
        errs.append(f"{name}: {e}")
    resto = [p for p in expeers if not peers.is_remote(p)]
    for p in resto:
        ainda = [x for x in resto if x != p]
        msg = (f"[de: claude-pocket] '{name}' saiu do grupo de trabalho. "
               + (f"O grupo continua entre você e {', '.join(ainda)}."
                  if ainda else "O grupo foi dissolvido (só restava você); volte a operar independente."))
        e = await _deliver(p, msg)
        if e:
            errs.append(f"{p}: {e}")
    return {"ok": True, "warning": ("aviso de saída falhou: " + "; ".join(errs)) if errs else None}


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    # Mesma guarda do /input — e aqui ela é a ÚNICA: a cadeia abaixo não sabe falhar. terminal.select
    # devolve None, send_keys descarta o returncode e tmux._run converte tmux morto/travado
    # (TimeoutExpired/OSError) num CompletedProcess(returncode=1) que ninguém lê. Sem isto, responder
    # uma opção de sessão morta digitava no vazio e a resposta era {"ok": true} — o catch do card
    # nunca disparava. (O fix de raiz em send_keys/_run é outro diff: interrupt/model_picker/
    # TerminalMirror também passam por lá.)
    if not _session_exists(name):
        raise HTTPException(404, "sessão não encontrada — opção NÃO enviada")
    terminal.select(name, body.option)
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
async def interrupt(name: str, clear: bool = False):
    # Codex: interrompe a propria TUI pelo tmux, mantendo celular e terminal no mesmo controlador.
    if _provider_of(name) == "codex":
        await get_adapter("codex").interrupt(name)
        return {"ok": True}
    # clear=True: alem de interromper, limpa o input (2o Esc). So o front com msg pendente passa isso —
    # garante input nao-vazio, evitando que o Esc-Esc abra o menu de rewind num input ja vazio.
    # terminal.interrupt e SYNC (tmux) -> threadpool pra nao bloquear o event loop (handler async agora).
    await asyncio.to_thread(terminal.interrupt, name, clear=clear)
    return {"ok": True}


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
        raise HTTPException(400, "limits so existe pra sessoes Codex")
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


@app.get("/api/sessions/{name}/models", dependencies=[Depends(require_auth)])
async def codex_models(name: str):
    # Task C: modelo + reasoning effort so pra Codex (via model/list) -- o /model do Claude e o
    # picker interativo dedicado (/model-effort), sem esta rota.
    if _provider_of(name) != "codex":
        raise HTTPException(400, "models so existe pra sessoes Codex")
    adapter = get_adapter("codex")
    return {"models": await adapter.list_models(name), "current": adapter.current_model(name)}


@app.post("/api/sessions/{name}/model", dependencies=[Depends(require_auth)])
async def set_codex_model(name: str, body: CodexModelBody):
    # Grava a escolha e reabre/configura a TUI; se ha turno em voo, aplica ao terminar.
    if _provider_of(name) != "codex":
        raise HTTPException(400, "model so existe pra sessoes Codex")
    await get_adapter("codex").set_model(name, body.model, body.effort)
    return {"ok": True}


@app.get("/api/sessions/{name}/pane", dependencies=[Depends(require_auth)])
def pane(name: str, lines: int = 200):
    # Pane CRU (texto ja composto pelo tmux: sem ANSI/cursor-move). O espelho do pane (TerminalMirror)
    # le isto pra mostrar overlays so-TUI (/status, /config, /help, pickers) que nao caem no .jsonl.
    # `lines` = quanto SCROLLBACK trazer acima da tela visivel (capture-pane -S). O espelho pede mais
    # quando o usuario rola pro topo; clampeado pra uma janela absurda nao virar payload gigante a
    # cada poll de 450ms.
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(404, "sessao nao encontrada")
    # `scrollback` diz se pedir mais linhas ADIANTA. Num TUI de tela alternada (Claude Code) vale 0:
    # o tmux nao guarda historico ali, e quem quer subir tem que rolar o PROPRIO TUI (PageUp), nao o
    # tmux. Sem esse dado a UI ofereceria "carregar mais historico" que nunca traria nada.
    return {"text": tmux.capture_pane(name, lines=max(50, min(lines, 5000))),
            "scrollback": tmux.pane_scrollback(name)}


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


@app.get("/api/config", dependencies=[Depends(require_auth)])
def get_config():
    """Config editavel pelo app + o que e so-leitura (exige reiniciar o servico).

    Segredo NUNCA volta inteiro: `estado()` devolve mascarado (gsk_••••1234) — da pra conferir QUAL
    chave esta la sem conseguir copia-la de volta."""
    return {
        "campos": runtime_config.estado(),
        "somente_leitura": {
            "port": settings.port,
            "lan_bind_ip": settings.lan_bind_ip,
            "server_id": settings.server_id,
            "public_url": settings.public_url,
            "scan_roots": settings.scan_roots,
        },
    }


@app.patch("/api/config", dependencies=[Depends(require_auth)])
async def patch_config(request: Request):
    """Grava overrides. Campo desconhecido e ignorado (o cliente nao inventa setting); tipo errado
    volta 400 com a mensagem, em vez de gravar lixo que so quebraria depois."""
    body = await request.json()
    if not isinstance(body, dict):
        raise HTTPException(400, "corpo deve ser um objeto")
    try:
        await asyncio.to_thread(runtime_config.aplicar, body)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"campos": runtime_config.estado()}


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
    if is_video(path):
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


@app.post("/api/sessions/{name}/transcribe", dependencies=[Depends(require_auth)])
async def transcribe_audio(name: str, request: Request):
    # Salva o audio (pra anexar o path no chat) E transcreve via Groq num round-trip. Mesmo padrao
    # de upload (raw body + X-Filename). Devolve {path, text} -> o front monta "texto — 📎 audio: path".
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
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        path = await asyncio.to_thread(save_upload, info.cwd, data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    # Transcricao (chamada de rede bloqueante) no threadpool pra nao travar o loop.
    try:
        text = await asyncio.to_thread(transcribe, data, filename)
    except TranscribeError as e:
        raise HTTPException(e.status, e.detail)
    return {"path": path, "text": text}


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


@app.get("/api/sessions/{name}/uploads", dependencies=[Depends(require_auth)])
def list_session_uploads(name: str):
    # Galeria de anexos: a retencao vive no servidor, entao o prazo sai daqui pronto (o front so
    # desenha). Le do runtime_config, nao do env cru — senao a galeria mostraria um prazo e o
    # prune usaria outro.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
    return {"files": list_uploads(info.cwd, runtime_config.get("upload_retention_days"))}


class CheckoutBody(_StrictBody):
    branch: str


class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop", "log"]


class GitPathBody(_StrictBody):
    path: str   # validado em git_ops contra a lista real de arquivos alterados (anti-traversal)


class GitCommitBody(_StrictBody):
    message: str = Field(min_length=1)
    paths: list[str] = Field(min_length=1)


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


@app.get("/api/sessions/{name}/git/log", dependencies=[Depends(require_auth)])
def git_log_route(name: str):
    try:
        return {"commits": assign_lanes(git_log(_session_cwd(name)))}
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


@app.post("/api/sessions/{name}/git/commit", dependencies=[Depends(require_auth)])
def git_commit(name: str, body: GitCommitBody):
    try:
        return commit(_session_cwd(name), body.message, body.paths)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/files", dependencies=[Depends(require_auth)])
def git_commit_files(name: str, sha: str):
    try:
        return {"files": commit_files(_session_cwd(name), sha)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/diff", dependencies=[Depends(require_auth)])
def git_commit_diff(name: str, sha: str, path: str):
    try:
        return commit_file_diff(_session_cwd(name), sha, path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/push", dependencies=[Depends(require_auth)])
def git_push(name: str):
    try:
        return push_branch(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/runners", dependencies=[Depends(require_auth)],
         response_model=RunnersResponse)
def list_runners(name: str):
    cwd = _session_cwd(name)
    return RunnersResponse(
        detected=runner.detect_runners(cwd),
        remembered=runner.remembered(cwd),
        running=runner.run_status(cwd),
    )


@app.post("/api/sessions/{name}/run", dependencies=[Depends(require_auth)],
          response_model=RunInfo)
def start_runner(name: str, body: RunBody):
    return runner.start_run(_session_cwd(name), body.command)


@app.post("/api/sessions/{name}/run/stop", dependencies=[Depends(require_auth)])
def stop_runner(name: str):
    runner.stop_run(_session_cwd(name))
    return {"ok": True}


@app.get("/api/sessions/{name}/run/pane", dependencies=[Depends(require_auth)])
def runner_pane(name: str):
    return {"pane": runner.run_pane(_session_cwd(name))}


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


@app.post("/api/sessions/{name}/open-editor", dependencies=[Depends(require_auth)])
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
        raise HTTPException(500, f"editor '{binario}' falhou: {e}")
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


@app.post("/api/archive/{project}/{session_id}/resume", dependencies=[Depends(require_auth)],
          response_model=SessionInfo)
def resume_archived(project: str, session_id: str):
    # "Retomar conversa" do Arquivo: sobe uma sessao tmux NOVA no cwd original com `claude --resume
    # <uuid>` -- reusa registry.create (nome/config_dir/spawn tmux ja tratados), so troca o comando pro
    # uuid EXISTENTE (nao um novo transcript). Nome derivado do basename do cwd, igual ao
    # CreateSessionSheet do front; colisao suffixa -2/-3... (mesmo esquema, do lado do backend pq aqui
    # nao ha form pro usuario escolher nome).
    from app import tmux
    try:
        cwd = archive_cwd(project, session_id)
    except ValueError:
        raise HTTPException(400, "invalid path")
    except FileNotFoundError:
        raise HTTPException(404, "transcript not found")
    if not cwd:
        raise HTTPException(422, "cwd not found in transcript")
    base = sanitize_session_name(Path(cwd).name) or "sessao"
    name, i = base, 2
    while tmux.has_session(name):
        name = f"{base}-{i}"
        i += 1
    try:
        return registry.create(name, cwd, resume_session_id=session_id)
    except ValueError as e:
        raise HTTPException(409, str(e))


# ── Busca de conteudo cross-session: grep (rg) em todos os transcripts (vivos + arquivados) ──
@app.get("/api/search", dependencies=[Depends(require_auth)], response_model=list[SearchHit])
def search_transcripts(q: str = ""):
    # live: realpath(jsonl) -> nome tmux das sessoes VIVAS (mesmo join do /api/archive). A busca marca
    # o hit como vivo e carrega o nome pra a UI abrir o chat (viva) ou o arquivo (morta). q vazia -> [].
    live = {os.path.realpath(s.jsonl): s.name for s in registry.list() if s.jsonl}
    return search(q, live)


class AskHistoryBody(_StrictBody):
    question: str = Field(min_length=1, max_length=500)


@app.post("/api/ask-history", dependencies=[Depends(require_auth)])
def ask_history(body: AskHistoryBody):
    """RAG lexical ("onde falei sobre X"): extrai termos da pergunta -> busca OR nos transcripts ->
    claude -p resume EM QUAL sessao o assunto apareceu. Sob o kill-switch (dispara claude -p). Sem
    trecho -> resposta vazia sem chamar o CLI. v1: so o servidor que recebe a chamada (cross-server v2)."""
    if not automations_enabled():
        raise HTTPException(409, "automações desligadas (kill-switch)")
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


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth)])
def answer(name: str, body: AnswerBody):
    # Dirige o AskUserQuestion tabbed: reproduz as teclas (nav em malha fechada), confere o Review e
    # submete. Input invalido -> 409. Drive falhou (DriveError: nada submetido, sem Escape) ->
    # FALLBACK automatico: Escape (fecha o picker; o "declined" e intencional aqui) + resposta como
    # texto via _send_one (fila duravel: se o pane ainda estiver em overlay vira deferred e o drain
    # entrega). A resposta do usuario NUNCA se perde — pior caso chega como texto, nao como interrupt mudo.
    from app import terminal_input
    answers = [a.model_dump() for a in body.answers]
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    fallback = False
    try:
        terminal_input.answer_questions(name, answers)
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        text = _askq_fallback_text(answers, jsonl)
        _log.warning("ASKQ fallback name=%s reason=%s text=%r", name, e, text[:120])
        terminal.interrupt(name)  # Escape unico: fecha o picker (sem clear — input vazio)
        if text:
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, f"drive falhou e fallback por texto tambem: {res['error']}")
        fallback = True
    # Respondido: limpa o sidecar do hook pra um stale nao reabrir o stepper depois. Resolve o jsonl
    # igual aos outros endpoints; se nao resolver, pula a limpeza sem falhar a request.
    if jsonl:
        clear_pending_askq(jsonl)
    return {"ok": True, "fallback": fallback}


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


@app.get("/api/sessions/{name}/commands", dependencies=[Depends(require_auth)])
def commands(name: str):
    # cwd vem do registry/tmux; se a sessao nao for achada, ainda devolvemos os built-ins
    # + skills globais (lista util mesmo sem cwd casado).
    cwd = next((s.cwd for s in registry.list() if s.name == name), None)
    return list_commands(cwd)
