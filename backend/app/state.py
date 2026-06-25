import asyncio
import re
from typing import AsyncIterator, Optional

from app import tmux
from app.models import StateEvent

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*❯?\s*\d+\.\s+(.*\S)\s*$")
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)


def _question(pane_text: str) -> Optional[str]:
    found = None
    for line in pane_text.splitlines():
        if _OPTION_RE.match(line):
            break
        s = line.strip()
        if s.endswith("?"):
            found = s
    return found


def classify(pane_text: str) -> tuple[str, Optional[str], Optional[str], Optional[list[str]]]:
    """Return (state, label, question, options).

    'working' -> label is the live spinner text; 'awaiting_input' -> question +
    options; otherwise 'idle'. 'dead' is decided by the caller (StateMonitor).
    """
    for line in pane_text.splitlines():
        s = line.strip()
        if len(s) >= 2 and s[0] in SPINNER_GLYPHS and s[1] == " ":
            return ("working", s[2:].strip(), None, None)

    if _CURSOR_RE.search(pane_text):
        options = [m.group(1).strip()
                   for m in (_OPTION_RE.match(ln) for ln in pane_text.splitlines()) if m]
        if options:
            return ("awaiting_input", None, _question(pane_text), options)

    return ("idle", None, None, None)


class StateMonitor:
    def __init__(self, name: str, poll: float = 0.75):
        self.name = name
        self.poll = poll

    async def stream(self) -> AsyncIterator[StateEvent]:
        last_key = object()
        while True:
            if not tmux.has_session(self.name):
                yield StateEvent(session=self.name, state="dead")
                return
            state, label, question, options = classify(tmux.capture_pane(self.name))
            key = (state, label, question, tuple(options or ()))
            if key != last_key:
                last_key = key
                yield StateEvent(session=self.name, state=state, label=label,
                                 question=question, options=options)
            await asyncio.sleep(self.poll)
