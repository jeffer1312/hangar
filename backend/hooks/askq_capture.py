#!/usr/bin/env python3
# ponytail: hook minimo — le o JSON do PreToolUse no stdin e grava o payload do AskUserQuestion
# num sidecar por session_id. SEM stdout (o bug conhecido do PreToolUse + AskUserQuestion mora
# em conflito de stdout/stdin -> nada de imprimir). Falha em silencio (nunca trava o prompt).
import json
import os
import sys
try:
    raw = sys.stdin.read()
    o = json.loads(raw)
    if o.get("tool_name") != "AskUserQuestion":
        sys.exit(0)
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    d = os.path.join(base, ".claude-pocket-askq")
    os.makedirs(d, exist_ok=True)
    sid = o.get("session_id") or "unknown"
    with open(os.path.join(d, sid + ".json"), "w", encoding="utf-8") as fh:
        fh.write(raw)
except Exception:
    pass
sys.exit(0)
