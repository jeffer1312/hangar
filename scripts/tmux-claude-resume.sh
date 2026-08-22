#!/usr/bin/env bash
# claude-pocket — persist & restore the live agent conversation per tmux session across reboot.
#
# Why this exists: tmux-resurrect restores panes + cwd but NOT the agent CLI (it isn't in resurrect's
# restore whitelist), and when an MCP server runs as a child it mis-captures that child (e.g.
# `npm exec chrome-devtools-mcp`) as the pane command. So resurrect alone brings sessions back as
# bare shells = the app shows "no id" and the conversation is unreachable.
#
# Fix: keep our OWN map  session-name -> provider + session id (+ account + engine), refreshed on
# every resurrect save (while the agent is alive). On restore we relaunch the RIGHT resume command in
# each bare pane, so the exact transcript comes back instead of a blank shell.
#
# Providers (same detection as backend registry.provider_of_pane — argv0 of the pane's descendants):
#   claude  id = uuid on the cmdline (--session-id/--resume)      -> claude --resume <uuid>
#   kimi    id = ticket .claude-pocket-kimi/<pane>.json           -> kimi -S <session_id>
#   pi      id = ticket .claude-pocket-pi/<pane>.json             -> pi --session <uuid>
# Kimi has no caller-chosen id (no --session-id; -S only resumes) and pi rewrites its argv, so for
# both the per-pane ticket written by the app's hooks is the ONLY link pane -> session.
# Codex is deliberately out: its session lives in the app-server sidecar, not in a pane command.
#
# Account and engine ride along because a resumed agent that lands on the DEFAULT account can't find
# the transcript at all: `--conta` sessions carry CLAUDE_CONFIG_DIR and `--engine` ones CP_ENGINE,
# both read from the live process env and re-applied on the relaunch.
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
LOG="${TMUX_RESURRECT_DIR:-$HOME/.local/share/tmux/resurrect}/claude-resume.log"
# session-id (uuid) on claude's command line: --session-id <uuid> / --resume <uuid> (= the .jsonl).
SID_RE='--(session-id|resume)[ =]([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})'

_cmdline() { tr '\0' ' ' < "/proc/$1/cmdline" 2>/dev/null || true; }
_env_of()  { tr '\0' '\n' < "/proc/$1/environ" 2>/dev/null | sed -n "s/^$2=//p" | head -1; }
# Epoch second the process started (+-1s, from ps etimes). Used for the ticket freshness test.
_start_of() {
  local e; e=$(ps -o etimes= -p "$1" 2>/dev/null | tr -d ' ') || return 1
  [ -n "$e" ] || return 1
  echo $(( $(date +%s) - e ))
}
_config_dir_of() { local d; d=$(_env_of "$1" CLAUDE_CONFIG_DIR); echo "${d:-$HOME/.claude}"; }

# Which agent runs under a pane pid, and its pid/cmdline (search the pid + all its descendants). The
# agent may be a direct child of the pane shell (manual/wrapper) or the pane pid itself (app-created).
# Sets AGENT_PROV / AGENT_PID / AGENT_CMD.
AGENT_PROV=""; AGENT_PID=""; AGENT_CMD=""
scan_pane() {
  local stack=("$1") p k cl argv0 prov
  AGENT_PROV=""; AGENT_PID=""; AGENT_CMD=""
  while [ ${#stack[@]} -gt 0 ]; do
    p=${stack[-1]}; unset 'stack[-1]'
    cl=$(_cmdline "$p")
    for k in $(ps -o pid= --ppid "$p" 2>/dev/null); do stack+=("$k"); done
    # Same exclusion as registry.provider_of_pane: a daemon/subagent child is not the REPL owner.
    case "$cl" in *daemon*|*--bg-*|*--agent*) continue ;; esac
    argv0=${cl%% *}
    case "${argv0##*/}" in
      claude)         prov=claude ;;
      kimi|kimi-code) prov=kimi ;;
      pi)             prov=pi ;;
      *)              continue ;;
    esac
    AGENT_PROV=$prov; AGENT_PID=$p; AGENT_CMD=$cl
    return 0
  done
  return 1
}

# The cmdline uuid can be a GHOST: resume via the TUI picker (or /clear) keeps the wrapper's
# throwaway --session-id on the cmdline while claude writes to another <uuid>.jsonl. The backend's
# state_hook already records the REAL transcript per boot-id in <config>/.claude-pocket-active/
# <boot_id>.json = {"jsonl": <path>, ...} — trust that marker when it points at a live file.
# The marker lives in the pane's OWN config dir: a session on another account writes it under
# ~/.claude-<conta>, and reading ~/.claude there would silently fall back to the boot uuid.
real_uuid() {  # <boot uuid> <config dir>
  local j
  j=$(sed -n 's/.*"jsonl": *"\([^"]*\)".*/\1/p' "$2/.claude-pocket-active/$1.json" 2>/dev/null)
  if [ -n "$j" ] && [ -f "$j" ]; then
    j=${j##*/}; printf '%s\n' "${j%.jsonl}"
  else
    printf '%s\n' "$1"
  fi
}

# Per-pane ticket written by the kimi/pi hooks (mirrors registry.kimi_session_file / pi_session_file),
# freshness test included: tmux reuses %pane_id after a server restart, so a ticket older than the
# agent process belongs to the pane's PREVIOUS incarnation and would resume someone else's session.
ticket_field() {  # <kimi|pi> <pane id> <agent pid> <json key>
  local tick data ts born
  tick="$(_config_dir_of "$3")/.claude-pocket-$1/${2#%}.json"
  data=$(cat "$tick" 2>/dev/null) || return 1
  ts=$(sed -n 's/.*"ts" *: *\([0-9.]*\).*/\1/p' <<<"$data")
  born=$(_start_of "$3") || return 1
  [ -n "$ts" ] || return 1
  # 2s of slack: the ticket ts comes from the hook's clock, the birth from btime+ticks of the kernel.
  awk -v a="$ts" -v b="$born" 'BEGIN{exit !(a >= b - 2)}' || return 1
  sed -n "s/.*\"$4\" *: *\"\([^\"]*\)\".*/\1/p" <<<"$data" | head -1
}

save() {
  mkdir -p "$(dirname "$MAP")"
  local tmp name pane pid id cfg engine; tmp=$(mktemp)
  # One pid per session (active pane); these sessions are single-pane by design.
  while read -r name pane pid; do
    [ -n "${pid:-}" ] || continue
    scan_pane "$pid" || continue
    id=""
    case "$AGENT_PROV" in
      claude) if [[ $AGENT_CMD =~ $SID_RE ]]; then
                id=$(real_uuid "${BASH_REMATCH[2]}" "$(_config_dir_of "$AGENT_PID")")
              fi ;;
      kimi)   id=$(ticket_field kimi "$pane" "$AGENT_PID" session_id) || id="" ;;
      pi)     id=$(ticket_field pi "$pane" "$AGENT_PID" id) || id="" ;;
    esac
    [ -n "$id" ] || continue
    # `|| cfg=""`: o processo pode morrer ENTRE o scan_pane e esta leitura — e o instante mais
    # provavel disso e o desligamento, que e justamente quando o resurrect salva. Sem a guarda o
    # `tr` falha, o pipefail contamina a atribuicao e o `set -e` mata o save() aqui, calado (todo
    # log deste arquivo vive no restore()). Como o MAP so e gravado no `mv` do fim, uma sessao que
    # racea derruba a atualizacao de TODAS as outras daquele ciclo. Mesma guarda do _cmdline.
    cfg=$(_env_of "$AGENT_PID" CLAUDE_CONFIG_DIR) || cfg=""
    engine=$(_env_of "$AGENT_PID" CP_ENGINE) || engine=""
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$AGENT_PROV" "$id" "$cfg" "$engine" >> "$tmp"
  done < <(tmux list-sessions -F '#{session_name} #{pane_id} #{pane_pid}' 2>/dev/null)
  mv "$tmp" "$MAP"
}

restore() {
  # Audit trail: the 2026-07-17 boot restored sessions but injected nothing and left no trace to
  # debug — log every decision so the next failure is diagnosable.
  echo "$(date '+%F %T') restore: start (map: $(wc -l < "$MAP" 2>/dev/null || echo 0) entries)" >> "$LOG"
  [ -f "$MAP" ] || return 0
  local name prov id cfg engine cur cmd
  while IFS=$'\t' read -r name prov id cfg engine; do
    # Map written before providers existed: <name>\t<uuid>, claude on the default account.
    if [ -z "${id:-}" ] && [ -n "${prov:-}" ]; then id=$prov; prov=claude; fi
    [ -n "${name:-}" ] && [ -n "${id:-}" ] || continue
    # "=" = exact-match target: without it tmux prefix-matches, and a dead "api" entry
    # would resolve to a live "api-2" — resuming the wrong transcript in the wrong session.
    tmux has-session -t "=$name" 2>/dev/null || { echo "  $name: no session" >> "$LOG"; continue; }
    # ":." (active window/pane) is REQUIRED: on tmux 3.7 a bare "=name" resolves for has-session but
    # NOT for pane-targeting commands (send-keys dies with "can't find pane") — and under `set -e`
    # that single failure silently aborted the whole restore (the 2026-07-17 boot).
    cur=$(tmux display -t "=$name:." -p '#{pane_current_command}' 2>/dev/null) || cur=""
    # Only inject into a bare shell — never clobber an agent that's already running.
    case "$cur" in
      fish|bash|zsh|sh|"") ;;
      *) echo "  $name: skipped (pane runs '$cur')" >> "$LOG"; continue ;;
    esac
    case "$prov" in
      claude) cmd="claude --resume $id" ;;
      kimi)   cmd="kimi -S $id" ;;
      pi)     cmd="pi --session $id" ;;
      *)      echo "  $name: unknown provider '$prov'" >> "$LOG"; continue ;;
    esac
    # Engine env is applied INSIDE the pane by cp-engine (os.execvpe), same as registry does when it
    # spawns the session; the account is a plain env prefix because the pane already exists (the
    # `tmux new-session -e` the app uses is not available on a restored pane).
    if [ -n "${engine:-}" ]; then cmd="cp-engine --exec $engine -- $cmd"; fi
    if [ -n "${cfg:-}" ]; then cmd="env CLAUDE_CONFIG_DIR=$(printf '%q' "$cfg") $cmd"; fi
    if tmux send-keys -t "=$name:." "$cmd" Enter; then
      echo "  $name: injected [$prov] $id${engine:+ engine=$engine}${cfg:+ conta=$cfg}" >> "$LOG"
    else
      echo "  $name: send-keys FAILED" >> "$LOG"
    fi
    # Note: an untrusted cwd (e.g. $HOME) makes the agent show its "trust this folder?" prompt and
    # wait — answer it once on that session; trusted project dirs resume unattended.
  done < "$MAP"
}

case "${1:-}" in
  save) save ;;
  restore) restore ;;
  *) echo "usage: $0 save|restore" >&2; exit 2 ;;
esac
