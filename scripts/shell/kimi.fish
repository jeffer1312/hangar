# hangar — `kimi` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Makes every interactive `kimi` trackable by the hangar app by running it INSIDE tmux (the app
# only lists tmux sessions). SIMPLER than the pi wrapper on purpose: the Kimi CLI has no
# `--session-id` flag (the session id is born inside, at the first prompt), so there is no id to
# inject and no env var to export. The pane<->session link is the ticket the backend's kimi hook
# writes (it inherits TMUX_PANE from the pane) — see backend/hooks/kimi_state_hook.py.
#
# Rules (same as the posix version):
#  - subcommand as the FIRST argument -> raw (export/provider/acp/web/server/login/doctor/vis/
#    migrate/upgrade/update); `kimi "export the function"` stays an interactive launch.
#  - print-and-exit flags anywhere in the args -> raw: -p/--prompt, --output-format, --help/-h,
#    --version/-V.
#  - resume flags (-S/--session, -c/--continue) are NOT special: nothing to inject, normal path.
#  - already in tmux / stdin not a tty -> raw.
#  - outside tmux + interactive -> tmux session named after the folder BASENAME (-2/-3 on clash).
#
# Escape hatch: `command kimi ...` runs the raw binary, bypassing this wrapper.
function kimi
    # subcomando: só vale como PRIMEIRO argumento.
    if test (count $argv) -gt 0
        switch $argv[1]
            case export provider acp web server login doctor vis migrate upgrade update
                command kimi $argv
                return
        end
    end

    # usos não interativos primeiro: o kimi imprime e sai, não tem TUI pra envolver em tmux.
    for a in $argv
        switch $a
            case -p --prompt '--prompt=*' --output-format '--output-format=*' --help -h --version -V
                command kimi $argv
                return
        end
    end

    # TMUX herdado pode estar MORTO (mesmo caso dos outros wrappers). Valida o pane; stale -> limpa.
    if set -q TMUX
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    # já dentro de tmux, ou stdin não é um tty: nada a fazer (o bilhete do hook liga pane->sessão).
    if set -q TMUX; or not isatty stdin
        command kimi $argv
        return
    end

    # Mesma regra do backend (app/names.py) e dos outros wrappers: acento -> ASCII antes do filtro.
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

    # NAO usar `$run tmux ...` com $run possivelmente vazia: em fish, variavel vazia na posicao de
    # COMANDO e erro fatal (ver pi.fish, que documenta o caso). Por isso a chamada e duplicada nos
    # dois ramos. `sh -c` + exec mantem a MESMA forma dos outros wrappers. CP_SESSION_NAME: carimbo
    # de identidade pro hangar-send de dentro do pane (ver claude.fish).
    # KIMI_CODE_TUI_FULL_SCREEN=1: fullscreen TUI experimental do Kimi 0.36+ (scroll proprio,
    # composer fixo). O -e do backend (app/tmux.py) cobre sessoes criadas pelo app; este cobre as
    # abertas pelo terminal. Caminho "ja dentro de tmux -> raw" nao passa aqui: quem cobre e o
    # `set -gx` no config.fish.
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
        systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
            -e "KIMI_CODE_TUI_FULL_SCREEN=1" \
            sh -c 'exec kimi "$@"' _ $argv
    else
        tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
            -e "KIMI_CODE_TUI_FULL_SCREEN=1" \
            sh -c 'exec kimi "$@"' _ $argv
    end
end
