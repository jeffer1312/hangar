#!/usr/bin/env bash
# claude-pocket — persist & restore the live `claude` conversation per tmux session across reboot.
#
# Why this exists: tmux-resurrect restores panes + cwd but NOT `claude` (it isn't in resurrect's
# restore whitelist), and when an MCP server runs as a child it mis-captures that child (e.g.
# `npm exec chrome-devtools-mcp`) as the pane command. So resurrect alone brings sessions back as
# bare shells = the app shows "no id" and the conversation is unreachable.
#
# Fix: keep our OWN map  session-name -> claude session-id (uuid), refreshed on every resurrect
# save (while claude is alive). On restore we relaunch `claude --resume <uuid>` in each bare pane,
# so the exact transcript (<uuid>.jsonl) comes back instead of a blank shell. The app then tracks
# it again (registry matches `--resume <uuid>` the same as `--session-id`).
#
# Wired via resurrect hooks (installed by scripts/tmux-persist-setup.sh):
#   @resurrect-hook-post-save-all     -> tmux-claude-resume.sh save
#   @resurrect-hook-post-restore-all  -> tmux-claude-resume.sh restore
#
# ponytail: map is keyed by tmux session name (unique even when many sessions share one cwd, where
# newest-by-mtime would collide). Ceiling: a /clear mid-session rolls a NEW uuid while the cmdline
# keeps the boot uuid -> we resume the boot transcript, not the post-clear one. Acceptable: the boot
# thread is the main conversation. Upgrade path: read the live fd / newest-after-clear like the
# backend's registry does, if post-clear loss ever bites.
#
# Usage: tmux-claude-resume.sh save | restore
set -euo pipefail

MAP="${TMUX_RESURRECT_DIR:-$HOME/.local/share/tmux/resurrect}/claude-sessions.tsv"
ACTIVE="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.claude-pocket-active"
LOG="${TMUX_RESURRECT_DIR:-$HOME/.local/share/tmux/resurrect}/claude-resume.log"
# session-id (uuid) on claude's command line: --session-id <uuid> / --resume <uuid> (= the .jsonl).
SID_RE='--(session-id|resume)[ =]([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})'

# uuid of the claude running under a pane pid (search the pid + all its descendants). claude may be
# a direct child of the pane shell (manual/wrapper) or the pane pid itself (app-created).
claude_uuid() {
  local stack=("$1") p k cl
  while [ ${#stack[@]} -gt 0 ]; do
    p=${stack[-1]}; unset 'stack[-1]'
    cl=$(tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null) || cl=""
    if [[ $cl == *claude* && $cl =~ $SID_RE ]]; then
      printf '%s\n' "${BASH_REMATCH[2]}"; return 0
    fi
    for k in $(ps -o pid= --ppid "$p" 2>/dev/null); do stack+=("$k"); done
  done
  return 1
}

# The cmdline uuid can be a GHOST: resume via the TUI picker (or /clear) keeps the wrapper's
# throwaway --session-id on the cmdline while claude writes to another <uuid>.jsonl. The backend's
# state_hook already records the REAL transcript per boot-id in <config>/.claude-pocket-active/
# <boot_id>.json = {"jsonl": <path>, ...} — trust that marker when it points at a live file.
real_uuid() {
  local j
  j=$(sed -n 's/.*"jsonl": *"\([^"]*\)".*/\1/p' "$ACTIVE/$1.json" 2>/dev/null)
  if [ -n "$j" ] && [ -f "$j" ]; then
    j=${j##*/}; printf '%s\n' "${j%.jsonl}"
  else
    printf '%s\n' "$1"
  fi
}

save() {
  mkdir -p "$(dirname "$MAP")"
  local tmp name pid u; tmp=$(mktemp)
  # One pid per session (active pane); these sessions are single-pane by design.
  while read -r name pid; do
    [ -n "${pid:-}" ] || continue
    u=$(claude_uuid "$pid") || continue
    printf '%s\t%s\n' "$name" "$(real_uuid "$u")" >> "$tmp"
  done < <(tmux list-sessions -F '#{session_name} #{pane_pid}' 2>/dev/null)
  mv "$tmp" "$MAP"
}

restore() {
  # Audit trail: the 2026-07-17 boot restored sessions but injected nothing and left no trace to
  # debug — log every decision so the next failure is diagnosable.
  echo "$(date '+%F %T') restore: start (map: $(wc -l < "$MAP" 2>/dev/null || echo 0) entries)" >> "$LOG"
  [ -f "$MAP" ] || return 0
  local name uuid cur
  while IFS=$'\t' read -r name uuid; do
    [ -n "${name:-}" ] && [ -n "${uuid:-}" ] || continue
    # "=" = exact-match target: without it tmux prefix-matches, and a dead "api" entry
    # would resolve to a live "api-2" — resuming the wrong transcript in the wrong session.
    tmux has-session -t "=$name" 2>/dev/null || { echo "  $name: no session" >> "$LOG"; continue; }
    # ":." (active window/pane) is REQUIRED: on tmux 3.7 a bare "=name" resolves for has-session but
    # NOT for pane-targeting commands (send-keys dies with "can't find pane") — and under `set -e`
    # that single failure silently aborted the whole restore (the 2026-07-17 boot).
    cur=$(tmux display -t "=$name:." -p '#{pane_current_command}' 2>/dev/null) || cur=""
    # Only inject into a bare shell — never clobber a claude that's already running.
    case "$cur" in
      fish|bash|zsh|sh|"") tmux send-keys -t "=$name:." "claude --resume $uuid" Enter \
                             && echo "  $name: injected $uuid" >> "$LOG" \
                             || echo "  $name: send-keys FAILED" >> "$LOG" ;;
      *)                   echo "  $name: skipped (pane runs '$cur')" >> "$LOG" ;;
    esac
    # Note: an untrusted cwd (e.g. $HOME) makes claude show its "trust this folder?" prompt and
    # wait — answer it once on that session; trusted project dirs resume unattended.
  done < "$MAP"
}

case "${1:-}" in
  save) save ;;
  restore) restore ;;
  *) echo "usage: $0 save|restore" >&2; exit 2 ;;
esac
