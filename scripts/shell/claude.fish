# claude-pocket — `claude` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Makes every interactive `claude` trackable by the claude-pocket app:
#  1. injects a unique --session-id  -> the backend binds the exact transcript (.jsonl), so two
#     claudes in the SAME folder never leak into / overwrite each other.
#  2. runs INSIDE tmux               -> the app only lists tmux sessions; a claude started outside
#     tmux is invisible to the app.
#
# Rules:
#  - already passed --session-id/--resume/-c/--continue -> respected, untouched (the CLI rejects
#     --session-id combined with --resume/--continue).
#  - already in tmux ($TMUX) / -p / --print / stdin not a tty (pipe/script) -> only inject the id.
#  - outside tmux + interactive            -> create a tmux session named after the folder BASENAME
#     (suffix -2/-3 if it already exists) and run claude (with the id) inside it. Quitting claude
#     ends the command, so the tmux session dies and disappears from the app.
#
# COLORTERM + CLAUDE_CODE_TMUX_TRUECOLOR keep Claude's theme 24-bit inside tmux (see
# docs/tmux-truecolor-setup.md). The tmux server goes in its own systemd scope so closing the
# terminal that spawned it doesn't kill every session (same fix as backend/app/tmux.py).
#
# Escape hatch: `command claude ...` runs the raw binary, bypassing this wrapper.
function claude
    # Motor de modelo (claude-engine setou CP_ENGINE): o env é aplicado DENTRO do pane pelo
    # `cp-engine --exec`, não por `tmux -e`. Dois motivos: (1) tmux não herda o env de quem chama, e
    # (2) `-e ANTHROPIC_AUTH_TOKEN=…` deixaria a key visível em /proc/<pid>/cmdline para qualquer
    # usuário da máquina. Depois do exec, o cmdline é o do claude — sem segredo. Construído ANTES do
    # scan de flags abaixo porque -c/--resume/--session-id saem por um early return e também
    # precisam do prefixo — senão o motor é silenciosamente ignorado e a sessão sobe na conta
    # Anthropic.
    set -l pre
    if set -q CP_ENGINE; and test -n "$CP_ENGINE"
        set pre cp-engine --exec $CP_ENGINE --
    end

    for a in $argv
        switch $a
            case --session-id '--session-id=*' --resume '--resume=*' -c --continue
                # mesmo bypass de `command` do caminho "só injeta o id" abaixo: sem motor, chama o
                # binário direto (evita recursão na função); com motor, quem executa é o cp-engine
                # (processo à parte achado no PATH) — `command` não existiria pra ele executar.
                if test (count $pre) -eq 0
                    command claude $argv
                else
                    $pre claude $argv
                end
                return
        end
    end

    set -l id (uuidgen)

    # TMUX herdado pode estar MORTO (ex: kitty single-instance cujo mestre nasceu dentro de um pane
    # que já fechou -> todo terminal novo herda TMUX/TMUX_PANE stale). Sem esta guarda, o wrapper
    # achava que "já está em tmux", só injetava o id e o claude abria CRU (invisível no app).
    if set -q TMUX
        # list-panes -t <pane>: exit 1 se o pane não existe (display-message devolve 0 até pra pane morto).
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    if set -q TMUX; or contains -- -p $argv; or contains -- --print $argv; or not isatty stdin
        # `command` só faz sentido quando o QUEM roda é este próprio shell: bypassa a função `claude`
        # para não recursar nela mesma. Com $pre setado, quem executa é o cp-engine (um processo
        # separado, achado no PATH) — o execvpe dele nunca vê função de shell, então "command" viraria
        # só uma string a mais no argv (e um alvo inexistente pro execvpe procurar).
        if test (count $pre) -eq 0
            COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 command claude --session-id $id $argv
        else
            COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 $pre claude --session-id $id $argv
        end
        return
    end

    # Mesma regra do backend (app/names.py): acento -> ASCII equivalente ANTES do filtro, senao
    # "Área de Trabalho" vira "-rea-de-Trabalho" (a 1a letra some e sobra traco na frente — nome de
    # sessao tmux comecando com "-" e pedir encrenca). Sem iconv, segue com o nome cru.
    set -l base (basename "$PWD")
    if command -v iconv >/dev/null 2>&1
        set -l ascii (printf '%s' "$base" | iconv -f UTF-8 -t ASCII//TRANSLIT 2>/dev/null)
        # Checar o STATUS, nao so "veio algo": num basename com byte UTF-8 invalido (pasta vinda de
        # samba/zip com codepage errada) o iconv imprime o prefixo que ja converteu, falha no byte
        # ruim e sai 1. So testar `-n` aceitava esse pedaco truncado e o nome perdia tudo depois do
        # byte — "bad-<0xff>-name" virava "bad", calado. O posix ja acertava por encadear com &&.
        test $status -eq 0; and test -n "$ascii"; and set base $ascii
    end
    set base (string replace -ra '[^A-Za-z0-9_-]' '-' -- $base)
    set base (string replace -ra '^-+|-+$' '' -- $base)
    test -n "$base"; or set base session
    set -l name $base
    set -l i 2
    while tmux has-session -t "=$name" 2>/dev/null
        set name "$base-$i"
        set i (math $i + 1)
    end

    set -l run
    # `command -q systemd-run` diz que o BINARIO existe, nao que ele FUNCIONA: o gerenciador systemd
    # do usuario pode recusar criar scope transiente ("Failed to start transient scope unit"), e ai
    # o `claude` simplesmente nao abre. Medido nesta maquina: 5/5 falhas com binario e gerenciador na
    # mesma versao. O probe custa um fork e transforma "nao abre" em "abre sem scope proprio".
    # As tres condicoes ficam na MESMA linha logica: em fish, um `and` em linha nova encerra a
    # condicao do if e vira o primeiro comando do corpo — o probe seria ignorado, calado.
    # NAO usar `$run tmux ...` com $run possivelmente vazio: em fish, variavel vazia na posicao
    # de COMANDO e erro fatal ("O comando expandido estava vazio"), ao contrario de bash/zsh, onde
    # ela some e o comando seguinte roda. Como o probe acima FALHA nesta maquina (5/5, e o
    # comentario acima ja dizia isso), o caminho vazio e o NORMAL aqui, nao a excecao — o `claude`
    # simplesmente parava de abrir. Por isso a chamada e duplicada nos dois ramos, igual ao
    # claude.posix.sh, que documenta a mesma escolha.
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
        systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 \
            $pre claude --session-id $id $argv
    else
        tmux new-session -s $name -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 \
            $pre claude --session-id $id $argv
    end
end
