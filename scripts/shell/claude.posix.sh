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
claude() {
    local a
    # respect flags that manage their own session (injecting --session-id alongside them errors)
    for a in "$@"; do
        case "$a" in
            --session-id|--session-id=*|--resume|--resume=*|-c|--continue) command claude "$@"; return ;;
        esac
    done

    local id
    id=$(uuidgen 2>/dev/null) || id=$(cat /proc/sys/kernel/random/uuid)

    # only inject the id (no tmux) when: already in tmux, print mode, or stdin not a tty
    local print=0
    for a in "$@"; do case "$a" in -p|--print) print=1 ;; esac; done
    # TMUX herdado pode estar MORTO (ex: kitty single-instance cujo mestre nasceu dentro de um pane
    # que já fechou). Valida o pane; stale -> limpa e segue pro caminho "fora do tmux" (cria sessão).
    if [ -n "${TMUX:-}" ]; then
        # list-panes -t <pane>: exit 1 se o pane não existe (display-message devolve 0 até pra pane morto).
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    if [ -n "${TMUX:-}" ] || [ "$print" = 1 ] || [ ! -t 0 ]; then
        COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 command claude --session-id "$id" "$@"
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

    # duplicated call: zsh doesn't word-split an unquoted prefix var, so no $run trick here
    if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        systemd-run --user --scope --collect -q -- tmux new-session -s "$name" -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 claude --session-id "$id" "$@"
    else
        tmux new-session -s "$name" -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 claude --session-id "$id" "$@"
    fi
}
