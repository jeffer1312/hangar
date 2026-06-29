#!/usr/bin/env python3
# ponytail: hook minimo — le o JSON do evento no stdin, mapeia hook_event_name -> estado e grava
# um marcador por session_id. SEM stdout. Falha em silencio (nunca trava o prompt). Espelha o
# padrao do askq_capture.py. Usado pelo backend (hook_state.py) pra saber o estado da LISTA sem
# raspar o pane.
import json
import os
import sys
import time

_STATE = {
    "UserPromptSubmit": "working",
    "PreToolUse": "working",
    "PostToolUse": "working",
    "Notification": "awaiting_input",
    "Stop": "idle",
}

try:
    o = json.loads(sys.stdin.read())
    state = _STATE.get(o.get("hook_event_name"))
    sid = o.get("session_id")
    if state and sid:
        base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
        d = os.path.join(base, ".claude-pocket-state")
        os.makedirs(d, exist_ok=True)
        tmp = os.path.join(d, sid + ".json.tmp")
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump({"state": state, "ts": time.time()}, fh)
        os.replace(tmp, os.path.join(d, sid + ".json"))  # escrita atomica (watcher nunca le parcial)
except Exception:
    pass
sys.exit(0)
