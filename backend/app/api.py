from fastapi import FastAPI, Depends, HTTPException, Request
from fastapi.responses import FileResponse
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
    # Registra na fila duravel (sidecar) APOS o envio dar certo: aparece como user_msg em ordem e
    # persiste no reload; o merge dedup-a contra o transcript quando o Claude Code grava o prompt.
    # Slash-commands (/clear etc) NAO entram — sao meta, nao viram bubble. Falha ao gravar a fila
    # nao quebra o envio (a msg ja foi pro tmux).
    if not body.text.lstrip().startswith("/"):
        from app.pqueue import PromptQueue
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
