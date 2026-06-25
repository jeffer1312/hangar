from typing import Literal, Optional
from pydantic import BaseModel

ChatKind = Literal["user_msg", "assistant_msg", "tool_use", "tool_result"]
State = Literal["working", "idle", "awaiting_input", "dead"]


class SessionInfo(BaseModel):
    name: str
    cwd: Optional[str] = None
    jsonl: Optional[str] = None
    state: State = "idle"
    last_activity: Optional[float] = None


class ChatEvent(BaseModel):
    kind: ChatKind
    id: str
    parent_id: Optional[str] = None
    text: Optional[str] = None
    tool_name: Optional[str] = None
    tool_input: Optional[dict] = None
    tool_use_id: Optional[str] = None
    result: Optional[str] = None
    is_error: Optional[bool] = None
    ts: Optional[float] = None


class StateEvent(BaseModel):
    session: str
    state: State
    label: Optional[str] = None         # working: live status text, e.g. "Elucidating…"
    question: Optional[str] = None       # awaiting_input: the question line
    options: Optional[list[str]] = None  # awaiting_input: selectable option labels
    status_line: Optional[str] = None    # raw bottom chrome from the pane, shown as-is on the web


class CommandInfo(BaseModel):
    # Contrato JSON consumido pelo frontend: argumentHint em camelCase de proposito.
    name: str
    display: str                                   # forma exibida, ex: "/clear"
    description: Optional[str] = None
    argumentHint: Optional[str] = None             # dica de argumento, ex: "<ambiente>"
    source: Literal["builtin", "skill", "plugin"] = "builtin"
    destructive: bool = False                      # exige confirmacao na UI


class FsRoot(BaseModel):
    # Raiz liberada do scanner, virando um chip no app. name = basename do caminho.
    name: str
    path: str


class FsEntry(BaseModel):
    # Subdiretorio imediato listado pelo scanner.
    name: str
    path: str
    is_git: bool = False           # tem .git -> badge "git"
    has_claude_md: bool = False    # tem CLAUDE.md -> badge "CLAUDE.md"
    mtime: Optional[float] = None  # epoch s; o app formata o tempo relativo


class FsScanResult(BaseModel):
    # entries vazio + error preenchido = pasta valida porem ilegivel (ex: sem permissao).
    entries: list[FsEntry] = []
    error: Optional[str] = None
