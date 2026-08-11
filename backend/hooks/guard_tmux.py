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
# ponytail: casa POSICAO DE COMANDO (ver `segmentos`), nao substring. Falar sobre um comando
# proibido — mensagem de commit, doc, grep, recado pra outra sessao — nunca e bloqueio. Nao tenta
# cobrir eval/string montada em runtime nem corpo de heredoc: guarda, nao sandbox.
import json
import os
import shlex
import sys

MULTIPLEXADORES = {"tmux", "psmux"}  # no Windows o psmux publica o alias `tmux`
MATADORES = {"pkill", "killall"}
SOCKET_FLAGS = ("-L", "-S")
# Prefixos que ainda NAO sao o comando: o de verdade vem depois deles.
ENVELOPES = {"sudo", "doas", "env", "nohup", "setsid", "time", "command", "exec", "builtin",
             "xargs", "stdbuf", "nice", "ionice", "systemd-run", "rtk", "proxy", "--"}


def segmentos(comando: str) -> list[str]:
    """Fatia a linha em POSICOES DE COMANDO, respeitando aspas.

    Existe por causa de um falso positivo real (11/08/2026): varrendo todos os tokens, qualquer
    comando que so FALASSE de `tmux kill-server` era recusado — mensagem de commit explicando o
    incidente, doc escrita por heredoc, recado pra outra sessao. O corte e por `;`, `&&`, `||`,
    `|`, `&`, quebra de linha e abertura de subshell/substituicao (`(`, `$(`, crase), que sao
    exatamente os lugares onde o shell comeca um comando novo.

    No `<<` a varredura PARA: dali pra frente e corpo de heredoc, ou seja DADO. Documentar a
    propria trava dentro de um heredoc e legitimo e era o caso que mais pegava."""
    fora: list[str] = []
    buf: list[str] = []
    aspas = ""
    i = 0
    while i < len(comando):
        c = comando[i]
        if aspas:
            buf.append(c)
            if c == aspas:
                aspas = ""
            i += 1
            continue
        if c in "'\"":
            aspas = c
            buf.append(c)
            i += 1
            continue
        if c == "\\" and i + 1 < len(comando):
            buf.append(comando[i:i + 2])
            i += 2
            continue
        if comando.startswith("<<", i):
            break
        if comando.startswith(("&&", "||"), i):
            fora.append("".join(buf))
            buf = []
            i += 2
            continue
        if c in ";|&\n()`":  # `)` fecha subshell: sem ele o token vinha `kill-server)`
            fora.append("".join(buf))
            buf = []
            i += 1
            continue
        if comando.startswith("$(", i):
            fora.append("".join(buf))
            buf = []
            i += 2
            continue
        buf.append(c)
        i += 1
    fora.append("".join(buf))
    return [s for s in fora if s.strip()]


def _tokens(segmento: str) -> list[str]:
    try:
        return shlex.split(segmento)
    except ValueError:  # aspas desbalanceadas (metade de um heredoc, string montada na mao)
        try:
            return shlex.split(segmento, posix=False)
        except ValueError:
            return segmento.split()


def _cabeca(tokens: list[str]) -> tuple[str, list[str]]:
    """(comando de verdade, argumentos dele) — pulando `VAR=x`, `sudo`, `rtk proxy` e afins."""
    for i, t in enumerate(tokens):
        nome = os.path.basename(t.strip("\"'"))
        if "=" in t and not t.startswith("-"):  # VAR=valor antes do comando
            continue
        if nome in ENVELOPES or t.startswith("-"):
            continue
        return nome, tokens[i + 1:]
    return "", []


def perigosos(comando: str) -> str | None:
    """Motivo do bloqueio, ou None se o comando esta liberado."""
    for segmento in segmentos(comando):
        nome, resto = _cabeca(_tokens(segmento))

        if nome in MATADORES:
            # `pkill -f app.main` (reiniciar o backend, documentado no CLAUDE.md) continua liberado:
            # so casa quando o PADRAO fala de tmux — que e o processo com sessoes vivas dentro.
            if any("tmux" in t or "psmux" in t for t in resto):
                return f"`{nome}` casando tmux mata o servidor e todas as sessoes junto"
            continue

        if nome not in MULTIPLEXADORES:
            continue
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
