from fastapi import FastAPI, Depends, HTTPException
from pydantic import BaseModel
from sse_starlette.sse import EventSourceResponse
from app.auth import require_auth
from app.commands import list_commands
from app.fs import FsError, list_roots, scan_dir
from app.registry import SessionRegistry
from app.terminal_input import TerminalInput
from app.sse import merged_events

app = FastAPI(title="claude-pocket")
registry = SessionRegistry()
terminal = TerminalInput()


class CreateBody(BaseModel):
    name: str
    cwd: str


class InputBody(BaseModel):
    text: str


class SelectBody(BaseModel):
    option: int


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
    from app.transcript import parse_transcript
    return parse_transcript(jsonl)


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str):
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    return EventSourceResponse(merged_events(name, jsonl))


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
def input_prompt(name: str, body: InputBody):
    terminal.send_prompt(name, body.text)
    return {"ok": True}


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    terminal.select(name, body.option)
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
def interrupt(name: str):
    terminal.interrupt(name)
    return {"ok": True}


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
