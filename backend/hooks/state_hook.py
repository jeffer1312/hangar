#!/usr/bin/env python3
# ponytail: hook minimo — le o JSON do evento no stdin e grava um marcador. SEM stdout. Falha em
# silencio (nunca trava o prompt). Espelha o padrao do askq_capture.py. Usado pelo backend.
#
# Dois marcadores:
#  - .claude-pocket-state/<session_id>.json  {state,ts}  -> estado da LISTA sem raspar o pane.
#  - .claude-pocket-active/<boot_id>.json    {jsonl,ts}  -> transcript ATIVO (gravado em qualquer
#    evento). O claude pode rodar `--session-id X` mas escrever no <Y> resumido (X.jsonl nunca nasce);
#    o backend so tem o X do cmdline -> sem isto resolvia pro path fantasma X.jsonl (chat vazio). O
#    hook conhece o transcript REAL (transcript_path=Y) e o boot_id X (via ancestralidade /proc do
#    processo claude), gravando o mapa X->Y deterministico que o registry le.
import json
import os
import re
import sys
import time

_STATE = {
    "UserPromptSubmit": "working",
    "PreToolUse": "working",
    "PostToolUse": "working",
    "Notification": "awaiting_input",
    "Stop": "idle",
}

_SID_RE = re.compile(r"--(?:session-id|resume)[ =]([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})")


def _write_marker(base: str, subdir: str, key: str, payload: dict) -> None:
    d = os.path.join(base, subdir)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, key + ".json.tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh)
    os.replace(tmp, os.path.join(d, key + ".json"))  # escrita atomica (leitor nunca pega parcial)


def _boot_session_id(start_pid: int) -> str | None:
    # Sobe a arvore /proc a partir do hook ate achar o claude com --session-id/--resume <uuid> no
    # cmdline = o boot_id que o backend ve. Limite de saltos pra nunca enroscar.
    pid = start_pid
    for _ in range(12):
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as fh:
                cl = fh.read().replace(b"\x00", b" ").decode("utf-8", "replace")
        except OSError:
            return None
        if "claude" in cl:
            m = _SID_RE.search(cl)
            if m:
                return m.group(1)
        try:
            with open(f"/proc/{pid}/stat", encoding="utf-8", errors="replace") as fh:
                ppid = int(fh.read().rsplit(")", 1)[-1].split()[1])
        except (OSError, ValueError, IndexError):
            return None
        if ppid <= 1:
            return None
        pid = ppid
    return None


try:
    o = json.loads(sys.stdin.read())
    event = o.get("hook_event_name")
    sid = o.get("session_id")
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")

    state = _STATE.get(event)
    if state and sid:
        _write_marker(base, ".claude-pocket-state", sid, {"state": state, "ts": time.time()})

    # Transcript ATIVO: gravado em QUALQUER evento (todo hook carrega transcript_path). Crucial: o
    # /resume feito DENTRO de uma sessao ja aberta pode NAO disparar SessionStart -> mas o 1o prompt/
    # tool depois dele dispara UserPromptSubmit/PreToolUse e fixa o transcript certo aqui. SessionStart
    # (instalado tb) so adianta o caso de abrir ja com --resume/clear. boot_id = --session-id do
    # cmdline do claude (via ancestralidade /proc) = a chave que o backend ve.
    tp = o.get("transcript_path")
    if tp:
        boot = _boot_session_id(os.getppid()) or sid  # sem ancestral -> usa o proprio (Y->Y inocuo)
        if boot:
            _write_marker(base, ".claude-pocket-active", boot, {"jsonl": tp, "ts": time.time()})
except Exception:
    pass
sys.exit(0)
