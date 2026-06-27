# claude-pocket — `claude` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
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
function claude
    if string match -qr -- '--session-id|--resume' "$argv"
        command claude $argv
        return
    end

    set -l id (uuidgen)

    if set -q TMUX; or contains -- -p $argv; or contains -- --print $argv; or not isatty stdin
        command claude --session-id $id $argv
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

    tmux new-session -s $name -c "$PWD" claude --session-id $id $argv
end
