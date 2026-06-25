import re
from typing import Optional

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*[❯ ]?\s*\d+\.\s+(.*\S)\s*$")
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)


def _question(pane_text: str) -> Optional[str]:
    found = None
    for line in pane_text.splitlines():
        s = line.strip()
        if s.endswith("?"):
            found = s
    return found


def classify(pane_text: str):
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
