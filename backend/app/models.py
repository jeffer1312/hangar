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
