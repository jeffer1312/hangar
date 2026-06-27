# claude-pocket — `claude` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/claude.fish
#
# Makes every interactive `claude` trackable by the claude-pocket app:
#  1. injects a unique --session-id  -> the backend binds the exact transcript (.jsonl), so two
#     claudes in the SAME folder never leak into / overwrite each other.
#  2. runs INSIDE tmux               -> the app only lists tmux sessions; a claude started outside
#     tmux is invisible to the app.
#
# Rules:
#  - already passed --session-id/--resume  -> respected, untouched.
#  - already in tmux ($TMUX) / -p / --print / stdin not a tty (pipe/script) -> only inject the id.
#  - outside tmux + interactive            -> create a tmux session named after the folder BASENAME
#     (suffix -2/-3 if it already exists) and run claude (with the id) inside it. Quitting claude
#     ends the command, so the tmux session dies and disappears from the app.
#
# Escape hatch: `command claude ...` runs the raw binary, bypassing this wrapper.
claude() {
    local a
    # respect an explicit --session-id / --resume
    for a in "$@"; do
        case "$a" in
            --session-id|--session-id=*|--resume|--resume=*) command claude "$@"; return ;;
        esac
    done

    local id
    id=$(uuidgen 2>/dev/null) || id=$(cat /proc/sys/kernel/random/uuid)

    # only inject the id (no tmux) when: already in tmux, print mode, or stdin not a tty
    local print=0
    for a in "$@"; do case "$a" in -p|--print) print=1 ;; esac; done
    if [ -n "${TMUX:-}" ] || [ "$print" = 1 ] || [ ! -t 0 ]; then
        command claude --session-id "$id" "$@"
        return
    fi

    # outside tmux + interactive: tmux session named after the folder basename, unique.
    local base name i
    base=$(basename "$PWD" | tr -c 'A-Za-z0-9_-' '-')
    base=${base%-}; base=${base#-}
    [ -n "$base" ] || base=session
    name=$base; i=2
    while tmux has-session -t "=$name" 2>/dev/null; do
        name="$base-$i"; i=$((i + 1))
    done

    tmux new-session -s "$name" -c "$PWD" claude --session-id "$id" "$@"
}
