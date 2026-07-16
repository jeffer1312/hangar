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
    for a in $argv
        switch $a
            case --session-id '--session-id=*' --resume '--resume=*' -c --continue
                command claude $argv
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
        COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 command claude --session-id $id $argv
        return
    end

    set -l base (string replace -ra '[^A-Za-z0-9_-]' '-' (basename "$PWD"))
    test -n "$base"; or set base session
    set -l name $base
    set -l i 2
    while tmux has-session -t "=$name" 2>/dev/null
        set name "$base-$i"
        set i (math $i + 1)
    end

    set -l run
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR
        set run systemd-run --user --scope --collect -q --
    end
    $run tmux new-session -s $name -c "$PWD" \
        -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 \
        claude --session-id $id $argv
end
