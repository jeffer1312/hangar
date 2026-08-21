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
import traceback

# Medido no Kimi 0.34.0: TurnStarted/UserPromptSubmit abrem o turno; Stop fecha; Interrupt e o
# Esc do usuario (Stop NAO dispara em interrupt, pela doc); PermissionRequest e a espera de
# aprovacao de tool = awaiting_input. SessionStart NAO mapeia estado: a sessao so e criada no 1o
# prompt ("No session yet" da TUI), entao ele ja vem colado num turno — mas mesmo em resume sem
# turno o bilhete abaixo precisa ser gravado, e ele grava em TODO evento.
#
# LIMITE MEDIDO EM 13/08/2026 (instrumentei o hook e gravei o payload cru de cada evento): turno que
# comeca a partir de um prompt ENFILEIRADO na TUI nao dispara evento NENHUM — nem UserPromptSubmit,
# nem TurnStarted. Zero eventos entre 08:38 e 08:56 com a sessao escrevendo codigo o tempo todo, e o
# marcador congelado em "idle" o intervalo inteiro. Nao ha o que corrigir aqui: o evento nao existe.
# Quem cobre esse buraco e o backend, comparando este marcador com o mtime do wire.jsonl — ver
# app/state.py, corrige_ocioso_kimi.
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
    # O tmp leva o PID: o hook roda como processo solto e dois eventos da MESMA sessao se sobrepoem
    # (PreToolUse/PostToolUse em sequencia). Com nome fixo, as duas escritas usam o mesmo arquivo e o
    # replace promove bytes entrelacados — bilhete torto, que o registry le. Mesmo furo ja corrigido
    # em cp_panel_common.py.
    tmp = os.path.join(d, "%s.json.tmp.%d" % (key, os.getpid()))
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh)
    os.replace(tmp, os.path.join(d, key + ".json"))  # escrita atomica (leitor nunca pega parcial)


# Mesma base do hook do Claude e da extensao do Pi: o HookState vigia <base>/.claude-pocket-state
# e o registry le <base>/.claude-pocket-*. Sem CLAUDE_CONFIG_DIR (padrao) = ~/.claude. FORA do try
# de proposito: o log de falha abaixo precisa dela, e ela nao depende do stdin.
base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")

try:
    # bytes + utf-8 explicito: em modo texto o Windows usa o locale (cp1252). Ver preview_hook.py.
    o = json.loads(sys.stdin.buffer.read().decode("utf-8", "replace"))
    event = o.get("hook_event_name")
    sid = o.get("session_id")

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
    # NUNCA travar o prompt do usuario por causa do hook -> engole a excecao. Mas nao pode ser um
    # `pass` mudo: este hook e a UNICA fonte de estado do Kimi (o spinner dele e fase de lua, fora
    # de SPINNER_GLYPHS, entao o fallback de pane nunca detecta "working") E a unica fonte do
    # bilhete pane->sessao. Falhando calado, a sessao fica congelada em "ociosa/sem transcript" pra
    # sempre e nao ha onde olhar. Deixa UMA linha em disco, best-effort, sem depender de logging.
    try:
        with open(os.path.join(base, "kimi_hook_error.log"), "a", encoding="utf-8") as _fh:
            _fh.write("%s %s\n" % (time.time(), traceback.format_exc().replace("\n", " | ")))
    except Exception:
        pass
sys.exit(0)
