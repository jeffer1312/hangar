import asyncio
import mimetypes
import os
import re
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
from app.models import SessionInfo, ChatEvent
from app.terminal_input import TerminalInput
from app.sse import merged_events
from app.uploads import save_upload, resolve_upload, UploadError, MAX_BYTES
from app.config import list_config_dirs, ConfigDirInfo
from app.git_ops import list_branches, switch_branch, git_action, GitError
from app.askquestion import clear_pending_askq


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


app = FastAPI(title="claude-pocket")
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
registry = SessionRegistry()
terminal = TerminalInput()


class _StrictBody(BaseModel):
    # rejeita campos desconhecidos no corpo (contrato estrito; pega typo de campo do cliente -> 422).
    model_config = ConfigDict(extra="forbid")


class CreateBody(_StrictBody):
    name: str = Field(min_length=1)
    cwd: str = Field(min_length=1)
    config_dir: str | None = None


class InputBody(_StrictBody):
    text: str


class SelectBody(_StrictBody):
    option: int = Field(ge=1, le=50)  # picker 1-based; teto evita loop de fork tmux (DoS)


class KeyBody(_StrictBody):
    key: str  # nome da tecla de navegacao (allowlist em TerminalInput._NAV_KEYS)


class ModelEffortBody(_StrictBody):
    # ambos opcionais: so esforco (sem modelo) ainda dirige o picker do /model, deixando o
    # modelo na linha atual. scope: 'session' (aperta `s`) ou 'default' (aperta Enter).
    model: str | None = None
    effort: str | None = None
    scope: Literal["session", "default"] = "session"


@app.get("/api/sessions", dependencies=[Depends(require_auth)], response_model=list[SessionInfo])
def list_sessions():
    return registry.list()


@app.get("/api/claude-configs", dependencies=[Depends(require_auth)], response_model=list[ConfigDirInfo])
def claude_configs():
    return list_config_dirs()


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
        terminal.send_prompt(name, body.text)
    except ValueError as e:
        # send_prompt rejeita control chars (ex: '\n'). Sem isto virava 500 -> a msg sumia sem
        # feedback. Agora vira 400 limpo (o frontend mostra). (Multi-linha de verdade: backlog.)
        raise HTTPException(400, str(e))
    from app.pqueue import PromptQueue
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
        # Registra na fila duravel (sidecar) APOS o envio dar certo: aparece como user_msg em ordem e
        # persiste no reload; o merge dedup-a contra o transcript quando o Claude Code grava o prompt.
        # Falha ao gravar a fila nao quebra o envio (a msg ja foi pro tmux).
        try:
            PromptQueue(name).append(body.text)
        except OSError:
            pass
    return {"ok": True}


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    terminal.select(name, body.option)
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
def interrupt(name: str):
    terminal.interrupt(name)
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
    action: Literal["status", "pull"]  # allowlist declarativa no schema (alem do git_ops)


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


@app.get("/api/sessions/{name}/file", dependencies=[Depends(require_auth)])
def serve_file(name: str, path: str):
    # Serve QUALQUER arquivo referenciado na conversa (video/html/codigo/pdf/...). TRAVA de seguranca:
    # so serve se o `path` aparece no transcript desta sessao (citado por voce ou pelo Claude =
    # consentido) E existe E e arquivo regular -> bloqueia leitura arbitraria de disco / path-traversal.
    # FileResponse trata Range -> <video> faz seek/streaming.
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import path_in_transcript
    real = os.path.realpath(os.path.expanduser(path))
    if not path_in_transcript(jsonl, path):
        raise HTTPException(403, "file not referenced in this conversation")
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
