import mimetypes
import os
import re
from fastapi import FastAPI, Depends, HTTPException, Request
from fastapi.responses import FileResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel
from sse_starlette.sse import EventSourceResponse
from app.auth import require_auth
from app.commands import list_commands
from app.fs import FsError, list_roots, scan_dir
from app.model_picker import PickerError
from app.registry import SessionRegistry
from app.terminal_input import TerminalInput
from app.sse import merged_events
from app.uploads import save_upload, resolve_upload, UploadError

app = FastAPI(title="claude-pocket")
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


class CreateBody(BaseModel):
    name: str
    cwd: str


class InputBody(BaseModel):
    text: str


class SelectBody(BaseModel):
    option: int


class KeyBody(BaseModel):
    key: str  # nome da tecla de navegacao (allowlist em TerminalInput._NAV_KEYS)


class ModelEffortBody(BaseModel):
    # ambos opcionais: so esforco (sem modelo) ainda dirige o picker do /model, deixando o
    # modelo na linha atual. scope: 'session' (aperta `s`) ou 'default' (aperta Enter).
    model: str | None = None
    effort: str | None = None
    scope: str = "session"


@app.get("/api/sessions", dependencies=[Depends(require_auth)])
def list_sessions():
    return registry.list()


@app.post("/api/sessions", dependencies=[Depends(require_auth)])
def create_session(body: CreateBody):
    return registry.create(body.name, body.cwd)


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth)])
def kill_session(name: str):
    registry.kill(name)
    return {"ok": True}


class RenameBody(BaseModel):
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
    from app.pqueue import PromptQueue
    try:
        oq, nq = PromptQueue(name).path, PromptQueue(new).path
        if oq.exists():
            oq.replace(nq)
    except OSError:
        pass
    return {"ok": True, "name": new}


@app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)])
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
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
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
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None:
        raise HTTPException(404, "sessao nao encontrada")
    if not info.cwd:
        raise HTTPException(409, "cwd da sessao indisponivel")
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 10 * 1024 * 1024:
        raise HTTPException(413, "imagem maior que 10 MiB")
    data = await request.body()
    try:
        path = save_upload(info.cwd, data, request.headers.get("content-type"))
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
