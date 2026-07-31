# claude-cockpit — `pi` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Makes every interactive `pi` trackable by the claude-cockpit app:
#  1. injects a unique --session-id  -> CP_PI_SESSION carries the SAME uuid, exported into pi's own
#     environment. pi rewrites its own argv, so --session-id disappears from /proc/<pid>/cmdline —
#     the backend reads CP_PI_SESSION from /proc/<pid>/environ instead (registry.py:_pi_sid_of).
#  2. runs INSIDE tmux               -> the app only lists tmux sessions; a pi started outside tmux
#     is invisible to the app.
#
# Rules:
#  - already passed a flag that manages its own session state -> no --session-id injection, but
#    STILL wrapped in tmux when interactive+outside tmux. Per `pi --help`: --session-id,
#    --resume/-r, --continue/-c, --session (path|id), --fork (path|id), --no-session. Injecting
#    --session-id alongside any of these either errors or silently overrides what the user asked
#    for (e.g. `pi -c` would start a FRESH random session instead of continuing the last one).
#    The tmux wrap keeps resumed sessions visible in the app; state tracking still works because
#    cp-state.ts publishes the real session file from inside pi (it never depended on the id).
#  - not an interactive session at all -> raw, exactly like the codex wrapper leaves its subcommands
#    alone. Two shapes: (a) the FIRST argument is one of pi's subcommands (install/remove/uninstall/
#    update/list/config) — matched on the first argument only, so `pi "remove the dead code"` stays an
#    interactive launch with an initial prompt; (b) a flag that makes pi print and exit anywhere in the
#    args (-p/--print, --mode, --list-models, --export, --help/-h, --version/-v). Wrapping these was
#    the bug: `pi remove npm:foo` opened the TUI and never removed anything.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> only inject the id + export the var.
#  - outside tmux + interactive -> create a tmux session named after the folder BASENAME (suffix
#     -2/-3 if it already exists) and run pi (with the id) inside it. Quitting pi ends the command,
#     so the tmux session dies and disappears from the app.
#
# CP_PI_SESSION is EXPORTED, never passed via `tmux -e`: a tmux server that is already running does
# NOT hand its client's environment to a brand-new pane (measured — an `export`/`set -x` before
# `tmux new-session` never reaches the pane when a server is already up), so the export has to
# happen INSIDE the pane's own process. That's why the "create a new tmux session" branch below runs
# a tiny `sh -c` that exports CP_PI_SESSION and only then execs pi, instead of relying on ambient
# inheritance or on `-e` (same reasoning as CP_ENGINE in CLAUDE.md — keeps one shape for both
# wrappers).
#
# Escape hatch: `command pi ...` runs the raw binary, bypassing this wrapper.
function pi
    # subcomando: só vale como PRIMEIRO argumento (`pi remove x` é subcomando; `pi "remove x"` é prompt).
    if test (count $argv) -gt 0
        switch $argv[1]
            case install remove uninstall update list config
                command pi $argv
                return
        end
    end

    # usos não interativos primeiro: o pi imprime e sai, não tem TUI pra envolver em tmux.
    # Ordem indiferente hoje: o loop de resume só marca a flag (não retorna) e este retorna
    # sempre — `pi -c -p "x"` sai cru nas duas ordens (provado em review, 31/07).
    for a in $argv
        switch $a
            case -p --print --mode '--mode=*' --list-models '--list-models=*' --export '--export=*' --help -h --version -v
                command pi $argv
                return
        end
    end

    # Flags que gerenciam a própria sessão: NUNCA injetar --session-id (um id novo por cima de
    # `pi -c` abriria sessão FRESCA em vez de continuar). Mas o tmux continua valendo — antes
    # essas flags passavam cruas e a sessão retomada ficava INVISÍVEL pro app. O rastreio não
    # depende do id injetado: cp-state.ts publica o arquivo real da sessão por dentro do pi.
    set -l own_session 0
    for a in $argv
        switch $a
            case --session-id '--session-id=*' --resume '--resume=*' -r --continue -c --session '--session=*' --fork '--fork=*' --no-session
                set own_session 1
                break
        end
    end

    # Substituição que falha deixa a variável como LISTA VAZIA no fish (não como string vazia): sem o
    # fallback — o mesmo do pi.posix.sh — uma máquina sem uuidgen fazia `pi --session-id $id $argv`
    # colapsar pra `pi --session-id <primeiro argumento do usuário>`, comendo o argumento e exportando
    # CP_PI_SESSION vazio. As aspas em "$id" abaixo fecham o resto do buraco: mesmo que os dois jeitos
    # falhem, o pi recebe um --session-id vazio (erro visível) em vez de engolir o argv do usuário.
    set -l id (uuidgen 2>/dev/null)
    test -n "$id"; or set id (cat /proc/sys/kernel/random/uuid 2>/dev/null)

    # TMUX herdado pode estar MORTO (mesmo caso do wrapper claude). Valida o pane; stale -> limpa.
    if set -q TMUX
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    if set -q TMUX; or not isatty stdin
        if test $own_session -eq 1
            command pi $argv
            return
        end
        set -x CP_PI_SESSION $id
        command pi --session-id "$id" $argv
        return
    end

    # Mesma regra do backend (app/names.py) e do wrapper claude: acento -> ASCII antes do filtro.
    set -l base (basename "$PWD")
    if command -v iconv >/dev/null 2>&1
        set -l ascii (printf '%s' "$base" | iconv -f UTF-8 -t ASCII//TRANSLIT 2>/dev/null)
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

    # NAO usar `$run tmux ...` com $run possivelmente vazio: em fish, variavel vazia na posicao
    # de COMANDO e erro fatal ("O comando expandido estava vazio"), ao contrario de bash/zsh, onde
    # ela some e o comando seguinte roda. Como o probe acima FALHA nesta maquina (5/5, e o
    # comentario acima ja dizia isso), o caminho vazio e o NORMAL aqui, nao a excecao — o `claude`
    # simplesmente parava de abrir. Por isso a chamada e duplicada nos dois ramos, igual ao
    # claude.posix.sh, que documenta a mesma escolha.
    # `sh -c` exporta CP_PI_SESSION DENTRO do próprio pane antes do exec pi — ver o comentário de
    # cabeçalho pra o porquê. $0 vira "_" (placeholder), $1 o uuid, o resto ($@) os args originais.
    # CP_SESSION_NAME: carimbo de identidade pro cp-send de dentro do pane (ver claude.fish).
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
        if test $own_session -eq 1
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec pi "$@"' _ $argv
        else
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'export CP_PI_SESSION="$1"; shift; exec pi --session-id "$CP_PI_SESSION" "$@"' _ "$id" $argv
        end
    else
        if test $own_session -eq 1
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec pi "$@"' _ $argv
        else
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'export CP_PI_SESSION="$1"; shift; exec pi --session-id "$CP_PI_SESSION" "$@"' _ "$id" $argv
        end
    end
end
