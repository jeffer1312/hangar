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
#  - already passed --session-id/--resume/--continue -> respected, untouched.
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
    for a in $argv
        switch $a
            case --session-id '--session-id=*' --resume '--resume=*' --continue
                command pi $argv
                return
        end
    end

    set -l id (uuidgen)

    # TMUX herdado pode estar MORTO (mesmo caso do wrapper claude). Valida o pane; stale -> limpa.
    if set -q TMUX
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    if set -q TMUX; or not isatty stdin
        set -x CP_PI_SESSION $id
        command pi --session-id $id $argv
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

    set -l run
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
        set run systemd-run --user --scope --collect -q --
    end
    # `sh -c` exporta CP_PI_SESSION DENTRO do próprio pane antes do exec pi — ver o comentário de
    # cabeçalho pra o porquê. $0 vira "_" (placeholder), $1 o uuid, o resto ($@) os args originais.
    $run tmux new-session -s $name -c "$PWD" \
        sh -c 'export CP_PI_SESSION="$1"; shift; exec pi --session-id "$CP_PI_SESSION" "$@"' _ $id $argv
end
