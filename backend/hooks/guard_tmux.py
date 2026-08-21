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
# proibido — mensagem de commit, doc, grep, recado pra outra sessao — nunca e bloqueio.
#
# O QUE ESTA TRAVA NAO PEGA (guarda, nao sandbox — quem quiser burlar, burla):
#   - Expansao de variavel: `CMD=tmux; ${CMD} kill-server` passa. Saber o valor de ${CMD} exigiria
#     rodar um shell aqui dentro, que e caro e perigoso num hook que roda a cada Bash.
#   - Corpo de heredoc e string montada em runtime (`printf ... > x.sh && ./x.sh`).
#   - Recursao em interprete so vai ate FUNDO niveis.
# Tudo isso e aceito de propos ito: o alvo e o agente distraido querendo bancada limpa, nao um
# atacante. Falso positivo custa mais que furo — a trava vive no caminho de TODO comando Bash.
import json
import os
import re
import shlex
import sys
import time

MULTIPLEXADORES = {"tmux", "psmux"}  # no Windows o psmux publica o alias `tmux`
MATADORES = {"pkill", "killall"}
SOCKET_FLAGS = ("-L", "-S")
# Prefixos que ainda NAO sao o comando: o de verdade vem depois deles.
ENVELOPES = {"sudo", "doas", "env", "nohup", "setsid", "time", "timeout", "command", "exec",
             "builtin", "stdbuf", "nice", "ionice", "systemd-run", "rtk", "proxy", "--"}
# Rodam um comando que vem como ARGUMENTO — precisam ser reprocessados, senao `bash -c "..."`
# entrega a linha inteira num token so e a trava nao ve nada dentro.
INTERPRETES = {"sh", "bash", "zsh", "dash", "ksh", "ash", "busybox", "eval"}
DURACAO = re.compile(r"^\d+(\.\d+)?[smhd]?$")  # o `5` do `timeout 5 tmux ...`
FUNDO = 3  # teto de recursao: `bash -c "bash -c ..."` nao vira loop


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
        if nome in ENVELOPES or t.startswith("-") or DURACAO.match(nome):
            continue
        return nome, tokens[i + 1:]
    return "", []


def _mata_tmux(nome: str, args: list[str]) -> bool:
    """O pkill/killall alcanca um processo tmux?

    Com `-f` o argumento e REGEX contra a linha de comando inteira, entao comparar substring
    deixava passar `pkill -f 't*mux'` e `pkill -f 'tm.x'` — os dois casam um tmux de verdade sem
    conter as letras 'tmux'. Aqui a gente pergunta o contrario: esse padrao casaria `tmux`?"""
    regex = any(t.startswith("-") and not t.startswith("--") and "f" in t for t in args)
    for t in args:
        if t.startswith("-"):
            continue
        if not regex:
            if "tmux" in t or "psmux" in t:
                return True
            continue
        try:
            if re.search(t, "tmux server") or re.search(t, "psmux"):
                return True
        except re.error:  # padrao invalido: cai no criterio antigo em vez de estourar
            if "tmux" in t:
                return True
    return False


def _script_do_interprete(nome: str, args: list[str]) -> str | None:
    """O comando embutido em `eval "..."` / `bash -c "..."`, ou None se nao ha."""
    if nome == "eval":
        return " ".join(args) if args else None
    for i, t in enumerate(args):
        # cobre `-c`, `-lc`, `-euc` — qualquer flag curta que contenha o c
        if t.startswith("-") and not t.startswith("--") and "c" in t:
            return args[i + 1] if i + 1 < len(args) else None
    return None


def perigosos(comando: str, fundo: int = 0) -> str | None:
    """Motivo do bloqueio, ou None se o comando esta liberado."""
    for segmento in segmentos(comando):
        nome, resto = _cabeca(_tokens(segmento))

        # `bash -c "tmux kill-server"` / `eval "..."`: o comando de verdade e o ARGUMENTO. Sem
        # reprocessar, a trava so via `bash` e liberava — e encadear com `bash -c` e coisa que
        # agente escreve sem malicia nenhuma.
        if nome in INTERPRETES and fundo < FUNDO:
            dentro = _script_do_interprete(nome, resto)
            if dentro:
                motivo = perigosos(dentro, fundo + 1)
                if motivo:
                    return motivo
            continue

        # `printf 'kill-server' | xargs tmux`: os argumentos vem da ENTRADA, nao da linha — nao da
        # pra saber o que o tmux vai receber. Alvo tmux/pkill via xargs e recusa por padrao.
        if nome == "xargs":
            alvo, _ = _cabeca(resto)
            if alvo in MULTIPLEXADORES or alvo in MATADORES:
                return (f"`xargs {alvo}` monta os argumentos pela entrada — o comando real nao "
                        "aparece na linha, entao pode ser um kill-server")
            continue

        if nome in MATADORES:
            # `pkill -f app.main` (reiniciar o backend, documentado no CLAUDE.md) continua liberado:
            # so casa quando o PADRAO alcanca tmux — que e o processo com sessoes vivas dentro.
            if _mata_tmux(nome, resto):
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


def _registrar(motivo: str) -> None:
    """Anota no disco quando a trava LIBERA por nao ter conseguido decidir.

    Ela falha aberta de proposito (um hook que estoura nao pode travar a sessao), mas falhar
    aberta E muda deixaria o furo invisivel pra sempre: ninguem descobre que a trava parou de
    valer olhando pra tela. Mesmo raciocinio do log em disco do kimi_state_hook. Best-effort:
    se nem logar der, engole — o comando do usuario nao paga por isso."""
    try:
        base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
        with open(os.path.join(base, "guard_tmux-falhas.log"), "a", encoding="utf-8") as fh:
            fh.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {motivo}\n")
    except Exception:
        pass


try:
    # bytes + utf-8 explicito (ver preview_hook.py). Aqui vale por um motivo a mais: este hook
    # falha ABERTO, entao um decode que estoura vira comando liberado. Com "replace" o comando
    # segue parseavel e `perigosos()` ainda casa os padroes, que sao ASCII.
    entrada = json.loads(sys.stdin.buffer.read().decode("utf-8", "replace"))
    if entrada.get("tool_name") == "Bash":
        bruto = entrada.get("tool_input", {}).get("command", "")
        if bruto and not isinstance(bruto, str):
            # Nao da pra parsear o que nao e texto — libera, mas deixa rastro.
            _registrar(f"tool_input.command nao e string: {type(bruto).__name__}")
            sys.exit(0)
        motivo = perigosos(bruto or "")
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
except SystemExit:
    raise  # o exit(2)/exit(0) de cima passa reto
except Exception as erro:
    _registrar(f"{type(erro).__name__}: {erro}")  # falhou -> libera, mas NAO em silencio
sys.exit(0)
