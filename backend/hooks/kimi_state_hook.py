#!/usr/bin/env python3
# ponytail: hook minimo do Kimi — le o JSON do evento no stdin e grava marcadores. SEM stdout.
# Falha em silencio (nunca trava o prompt). Irmao do state_hook.py (Claude), mas MAIS SIMPLES:
# o payload do Kimi ja traz session_id e cwd em TODO evento (docs oficiais de hooks), entao nao
# ha a danca de ancestralidade /proc que o Claude exige pra achar o boot_id.
#
# Dois marcadores (mesma base do cp-state.ts do Pi — o HookState do backend ja observa este dir):
#  - .claude-pocket-state/<session_id>.json  {state,ts}  -> estado da LISTA sem raspar o pane.
#  - .claude-pocket-kimi/<pane>.json  {session_id,cwd,ts}  -> BILHETE pane->sessao. E a UNICA
#    forma de o backend ligar um pane Kimi ao wire.jsonl: o Kimi nao aceita --session-id escolhido
#    pelo caller (o id nasce dentro, no 1o prompt), entao nao ha o que casar em /proc/cmdline.
#    O hook herda TMUX_PANE do processo kimi (medido: environ do kimi tem TMUX_PANE=%<n>).
#
# Frescor: o ts do bilhete e comparado com o nascimento do processo do pane pelo backend
# (registry.kimi_session_file) — mesmo contrato do bilhete do Pi, que protege contra o tmux reusar
# %pane_id depois de um restart.
import json
import os
import sys
import time

# Medido no Kimi 0.34.0: TurnStarted/UserPromptSubmit abrem o turno; Stop fecha; Interrupt e o
# Esc do usuario (Stop NAO dispara em interrupt, pela doc); PermissionRequest e a espera de
# aprovacao de tool = awaiting_input. SessionStart NAO mapeia estado: a sessao so e criada no 1o
# prompt ("No session yet" da TUI), entao ele ja vem colado num turno — mas mesmo em resume sem
# turno o bilhete abaixo precisa ser gravado, e ele grava em TODO evento.
_STATE = {
    "UserPromptSubmit": "working",
    "TurnStarted": "working",
    "Stop": "idle",
    "Interrupt": "idle",
    "PermissionRequest": "awaiting_input",
}

# AskUserQuestion NAO dispara PermissionRequest (nao e permissao — e a TUI servindo a pergunta) e
# sem estes dois o marcador ficava "working" com o agente parado esperando resposta (medido num
# picker real: PreToolUse com matcher "AskUserQuestion" dispara na abertura, PostToolUse ao
# responder). O installer registra estes eventos SOMENTE com matcher="AskUserQuestion", entao tool
# comum nunca passa por aqui.
_ASKQ = "AskUserQuestion"


def _state_of(event: str, o: dict) -> str | None:
    if event == "PreToolUse" and o.get("tool_name") == _ASKQ:
        return "awaiting_input"
    if event == "PostToolUse" and o.get("tool_name") == _ASKQ:
        return "working"
    return _STATE.get(event)


def _write_marker(base: str, subdir: str, key: str, payload: dict) -> None:
    d = os.path.join(base, subdir)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, key + ".json.tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh)
    os.replace(tmp, os.path.join(d, key + ".json"))  # escrita atomica (leitor nunca pega parcial)


try:
    o = json.loads(sys.stdin.read())
    event = o.get("hook_event_name")
    sid = o.get("session_id")
    # Mesma base do hook do Claude e da extensao do Pi: o HookState vigia <base>/.claude-pocket-state
    # e o registry le <base>/.claude-pocket-*. Sem CLAUDE_CONFIG_DIR (padrao) = ~/.claude.
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")

    state = _state_of(event, o)
    if state and sid:
        _write_marker(base, ".claude-pocket-state", sid, {"state": state, "ts": time.time()})

    # Bilhete pane->sessao em QUALQUER evento: quanto mais cedo o backend liga o pane ao wire,
    # melhor (SessionStart ja basta). Sem TMUX_PANE (kimi fora do tmux) nao ha o que ligar.
    pane = os.environ.get("TMUX_PANE")
    if sid and pane:
        _write_marker(base, ".claude-pocket-kimi", pane.lstrip("%"),
                      {"session_id": sid, "cwd": o.get("cwd"), "ts": time.time()})
except Exception:
    pass
sys.exit(0)
