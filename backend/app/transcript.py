import json
from pathlib import Path
from typing import AsyncIterator, Optional
from watchfiles import awatch
from app.models import ChatEvent


def _first(content: list, type_name: str) -> Optional[dict]:
    for item in content:
        if isinstance(item, dict) and item.get("type") == type_name:
            return item
    return None


# Claude Code logs slash-commands and local command I/O as synthetic "user" entries
# wrapped in these tags. They are tooling meta, not conversation — keep them out of the chat.
_COMMAND_META_PREFIXES = (
    "<command-name>", "<command-message>", "<command-args>",
    "<local-command-caveat>", "<local-command-stdout>", "<local-command-stderr>",
    "<bash-input>", "<bash-stdout>", "<bash-stderr>",
    # Invocacao de skill (/handoff, etc): o Claude Code injeta o corpo do SKILL.md como
    # entrada "user" sintetica que comeca com esta linha. E meta de tooling, nao conversa —
    # mesmo tratamento dos comandos acima (nao renderiza bubble).
    "Base directory for this skill:",
    # Notificacao de Workflow concluido: o harness injeta um <task-notification>...</task-notification>
    # como entrada "user" sintetica. Tooling meta, nao conversa — fora do chat.
    "<task-notification>",
)


def _is_command_meta(text: str) -> bool:
    return text.lstrip().startswith(_COMMAND_META_PREFIXES)


def parse_line(line: str) -> Optional[ChatEvent]:
    line = line.strip()
    if not line:
        return None
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return None

    etype = obj.get("type")
    uid = obj.get("uuid", "")
    parent = obj.get("parentUuid")
    msg = obj.get("message")
    if not isinstance(msg, dict):
        return None
    content = msg.get("content")

    if etype == "user":
        if isinstance(content, str):
            if _is_command_meta(content):
                return None
            return ChatEvent(kind="user_msg", id=uid, parent_id=parent, text=content)
        if isinstance(content, list):
            tr = _first(content, "tool_result")
            if tr is not None:
                res = tr.get("content")
                if isinstance(res, list):
                    res = " ".join(str(b.get("text", "")) for b in res if isinstance(b, dict))
                return ChatEvent(
                    kind="tool_result", id=uid, parent_id=parent,
                    tool_use_id=tr.get("tool_use_id"),
                    result=str(res) if res is not None else None,
                    is_error=bool(tr.get("is_error", False)),
                )
            txt = _first(content, "text")
            if txt is not None:
                t = txt.get("text", "")
                if _is_command_meta(t):
                    return None
                return ChatEvent(kind="user_msg", id=uid, parent_id=parent, text=t)
        return None

    if etype == "assistant" and isinstance(content, list):
        tu = _first(content, "tool_use")
        if tu is not None:
            return ChatEvent(
                kind="tool_use", id=uid, parent_id=parent,
                tool_name=tu.get("name"), tool_use_id=tu.get("id"),
                tool_input=tu.get("input") or {},
            )
        txt = _first(content, "text")
        if txt is not None:
            return ChatEvent(kind="assistant_msg", id=uid, parent_id=parent, text=txt.get("text", ""))
    return None


def parse_transcript(path: str | Path) -> list[ChatEvent]:
    events = []
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        ev = parse_line(line)
        if ev is not None:
            events.append(ev)
    return events


class TranscriptTailer:
    def __init__(self, path: str | Path):
        self.path = Path(path)

    def history(self) -> list[ChatEvent]:
        return parse_transcript(self.path)

    async def follow(self) -> AsyncIterator[ChatEvent]:
        pos = 0
        # emit existing content first
        if self.path.exists():
            with self.path.open(encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    ev = parse_line(line)
                    if ev is not None:
                        yield ev
                pos = fh.tell()
        # then watch for appends
        async for _ in awatch(self.path.parent):
            if not self.path.exists():
                continue
            with self.path.open(encoding="utf-8", errors="replace") as fh:
                fh.seek(pos)
                for line in fh:
                    ev = parse_line(line)
                    if ev is not None:
                        yield ev
                pos = fh.tell()
