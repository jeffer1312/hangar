#!/usr/bin/env python3
# PreToolUse/Bash: recusa comando tmux que derruba TODAS as sessoes da maquina.
#
# Por que existe: `tmux kill-server` sem `-L`/`-S` vai no socket default e mata todas as sessoes
# de trabalho de uma vez. Aconteceu 2x em 11/08/2026 nesta maquina — as duas por agente querendo
# "bancada limpa" antes de um teste, achando que estava num servidor isolado (a primeira via
# TMUX_TMPDIR, que $TMUX ignora; a segunda direto). Nenhum erro, nenhum OOM: as sessoes somem.
#
# Bloqueia (exit 2 = o agente le o stderr e nao roda):
#   - `tmux kill-server`         sem -L/-S  -> mata o servidor inteiro
#   - `tmux kill-session -a`     sem -L/-S  -> mata todas as OUTRAS sessoes
#   - `tmux kill-session` sem -t sem -L/-S  -> mata a sessao ATUAL (o pane de quem chamou)
#   - `pkill`/`killall` casando tmux        -> mesmo estrago, por fora do tmux
# Libera com socket explicito (`tmux -L probe-x kill-server`) e alvo exato
# (`tmux kill-session -t '=nome:'`).
#
# ponytail: casa por token, nao por regex no comando cru. Nao tenta cobrir heredoc/eval/string
# montada em runtime — guarda, nao sandbox: pega o jeito que o agente escreve de verdade.
import json
import os
import shlex
import sys

SEPARADORES = {"&&", "||", ";", "|", "\n"}
MULTIPLEXADORES = {"tmux", "psmux"}  # no Windows o psmux publica o alias `tmux`
MATADORES = {"pkill", "killall"}
SOCKET_FLAGS = ("-L", "-S")


def _invocacao(tokens: list[str], i: int) -> list[str]:
    """Argumentos da invocacao que comeca em tokens[i]: para no proximo comando da linha."""
    resto: list[str] = []
    for t in tokens[i + 1:]:
        if t in SEPARADORES:
            break
        resto.append(t)
    return resto


def perigosos(comando: str) -> str | None:
    """Motivo do bloqueio, ou None se o comando esta liberado."""
    try:
        tokens = shlex.split(comando)
    except ValueError:  # aspas desbalanceadas -> melhor olhar cru do que desistir
        tokens = comando.split()

    for i, tok in enumerate(tokens):
        nome = os.path.basename(tok.strip("\"'"))

        if nome in MATADORES:
            # `pkill -f app.main` (reiniciar o backend, documentado no CLAUDE.md) continua liberado:
            # so casa quando o PADRAO fala de tmux — que e o processo com sessoes vivas dentro.
            if any("tmux" in t or "psmux" in t for t in _invocacao(tokens, i)):
                return f"`{nome}` casando tmux mata o servidor e todas as sessoes junto"
            continue

        if nome not in MULTIPLEXADORES:
            continue
        resto = _invocacao(tokens, i)
        # `-L nome` / `-Lnome` / `-S /caminho`: servidor proprio, entao a explosao e contida
        if any(t.startswith(SOCKET_FLAGS) for t in resto):
            continue
        if "kill-server" in resto:
            return "`tmux kill-server` sem -L/-S derruba TODAS as sessoes do socket default"
        if "kill-session" in resto:
            if "-a" in resto:
                return "`tmux kill-session -a` sem -L/-S mata todas as outras sessoes do default"
            if "-t" not in resto and not any(t.startswith("-t") for t in resto):
                return "`tmux kill-session` sem -t mata a sessao ATUAL (o pane de quem chamou)"
    return None


try:
    entrada = json.loads(sys.stdin.read())
    if entrada.get("tool_name") == "Bash":
        motivo = perigosos(entrada.get("tool_input", {}).get("command", "") or "")
        if motivo:
            texto = (
                f"BLOQUEADO: {motivo}.\n"
                "Precisa de tmux limpo pra uma sonda? Suba um SERVIDOR SEPARADO com socket "
                "proprio e derrube ele inteiro a vontade:\n"
                "  tmux -L probe-x new-session -d -s alvo ...\n"
                "  tmux -L probe-x kill-server\n"
                "Nunca no servidor default — e onde vivem as sessoes de trabalho do usuario "
                "(TMUX_TMPDIR NAO isola: dentro do tmux quem manda e $TMUX).\n"
                "Pra matar UMA sessao: `tmux kill-session -t '=nome:'`."
            )
            # DOIS canais porque os tres agentes bloqueiam de jeitos diferentes (todos medidos
            # nesta maquina em 11/08/2026): Claude Code e Kimi Code param a ferramenta no
            # **exit 2** e mostram o stderr (kimi: `resultFromExitCode`, exitCode === 2 ->
            # action "block"); o adaptador do pi (claude-hooks-adapter.ts:138) SO bloqueia com
            # `permissionDecision: "deny"` no **stdout** — exit != 0 la vira um aviso na UI e a
            # ferramenta roda assim mesmo. Emitir so um dos dois deixa um dos motores desprotegido.
            print(json.dumps({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": texto,
            }}))
            print(texto, file=sys.stderr)
            sys.exit(2)
except Exception:
    pass  # guarda nunca trava a sessao: falhou, libera
sys.exit(0)
