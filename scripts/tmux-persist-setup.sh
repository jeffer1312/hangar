#!/usr/bin/env bash
# Make your claude-pocket tmux sessions survive a reboot or OOM kill.
#
# One-time setup. Installs tmux-resurrect + tmux-continuum (via TPM) and a systemd
# *user* timer that auto-saves the tmux layout every 15 min. On the next fresh tmux
# start (after reboot/OOM), continuum restores the last layout, panes, working dirs
# and the programs that were running (incl. `claude`).
#
# Why a systemd timer instead of continuum's own auto-save?
#   continuum hooks its periodic save into the tmux status line. The claude-pocket
#   reference conf runs with `status off`, which disables that hook — so continuum
#   alone would NEVER auto-save. The timer drives the save from outside; continuum is
#   kept only for restore-on-start. If you run with the status bar ON you don't need
#   this script — just set `@continuum-save-interval '15'` in your conf.
#
# Usage:
#   ./scripts/tmux-persist-setup.sh            # install / update (idempotent)
#   ./scripts/tmux-persist-setup.sh --uninstall
#
# Safe to re-run. Does NOT need root. Edits ~/.tmux.conf only to append a clearly
# marked plugin block if it is missing (backs the file up first).
set -euo pipefail

PLUGIN_DIR="$HOME/.tmux/plugins"
CONF="$HOME/.tmux.conf"
# Persist/restore the claude conversation per session (resurrect alone restores only a bare shell).
RESUME_SH="$(cd "$(dirname "$0")" && pwd)/tmux-claude-resume.sh"
SD_DIR="$HOME/.config/systemd/user"
SERVICE="tmux-resurrect-save.service"
TIMER="tmux-resurrect-save.timer"
SAVE_SCRIPT="$PLUGIN_DIR/tmux-resurrect/scripts/save.sh"
# Default tmux server socket: ${TMUX_TMPDIR:-/tmp}/tmux-<uid>/default
SOCKET="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/default"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

uninstall() {
  log "Disabling timer"
  systemctl --user disable --now "$TIMER" 2>/dev/null || true
  rm -f "$SD_DIR/$SERVICE" "$SD_DIR/$TIMER"
  systemctl --user daemon-reload
  echo "Removed systemd units. Plugins in $PLUGIN_DIR and conf lines left untouched."
  echo "To fully remove: delete the marked block in $CONF and rm -rf $PLUGIN_DIR/{tpm,tmux-resurrect,tmux-continuum}"
}

[[ "${1:-}" == "--uninstall" ]] && { uninstall; exit 0; }

command -v git  >/dev/null || { echo "git not found" >&2; exit 1; }
command -v tmux >/dev/null || { echo "tmux not found" >&2; exit 1; }

# 1. Plugins (clone or update)
log "Installing plugins into $PLUGIN_DIR"
mkdir -p "$PLUGIN_DIR"
for repo in tmux-plugins/tpm tmux-plugins/tmux-resurrect tmux-plugins/tmux-continuum; do
  dest="$PLUGIN_DIR/${repo##*/}"
  if [[ -d "$dest/.git" ]]; then
    git -C "$dest" pull --quiet --ff-only || true
  else
    git clone --quiet "https://github.com/$repo" "$dest"
  fi
done

# 2. Ensure ~/.tmux.conf carries the plugin block (append if missing)
MARK="# >>> claude-pocket tmux-persist >>>"
if [[ -f "$CONF" ]] && grep -qF "tmux-plugins/tmux-resurrect" "$CONF"; then
  log "Plugin lines already present in $CONF — leaving as is"
else
  log "Appending plugin block to $CONF (backup: $CONF.bak)"
  [[ -f "$CONF" ]] && cp "$CONF" "$CONF.bak"
  cat >> "$CONF" <<EOF

$MARK
set -g @plugin 'tmux-plugins/tpm'
set -g @plugin 'tmux-plugins/tmux-resurrect'
set -g @plugin 'tmux-plugins/tmux-continuum'
set -g @resurrect-capture-pane-contents 'on'
set -g @continuum-restore 'on'
set -g @continuum-save-interval '0'   # save driven by systemd timer (status bar is off)
# Bring the claude conversation back (not a bare shell): save name->uuid, restore via --resume.
set -g @resurrect-hook-post-save-all    '$RESUME_SH save'
set -g @resurrect-hook-post-restore-all '$RESUME_SH restore'
run '$PLUGIN_DIR/tpm/tpm'             # keep TPM init at the very end
# <<< claude-pocket tmux-persist <<<
EOF
fi

# 2b. Resume hooks: a conf that already had the plugin lines (e.g. copied from
# docs/tmux.conf.example) skipped the block above and has NO hooks — without them the
# name->uuid map is never saved and restore brings back bare shells. Append just the hooks.
if ! grep -qF "tmux-claude-resume.sh" "$CONF"; then
  log "Appending claude-resume hooks to $CONF"
  cat >> "$CONF" <<EOF

# >>> claude-pocket resume hooks >>>
# Bring the claude conversation back (not a bare shell): save name->uuid, restore via --resume.
set -g @resurrect-hook-post-save-all    '$RESUME_SH save'
set -g @resurrect-hook-post-restore-all '$RESUME_SH restore'
# <<< claude-pocket resume hooks <<<
EOF
fi

# 3. systemd user timer + service (auto-save every 15 min)
log "Installing systemd user timer (socket: $SOCKET)"
mkdir -p "$SD_DIR"
cat > "$SD_DIR/$SERVICE" <<EOF
[Unit]
Description=Save tmux state (resurrect) for claude-pocket
# Only run when a default tmux server is actually up.
ConditionPathExists=$SOCKET

[Service]
Type=oneshot
ExecStart=$SAVE_SCRIPT quiet
EOF

cat > "$SD_DIR/$TIMER" <<EOF
[Unit]
Description=Auto-save tmux state every 15 min

[Timer]
OnBootSec=5min
OnUnitActiveSec=15min
Persistent=true

[Install]
WantedBy=timers.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now "$TIMER"

log "Done."
echo
echo "  • Reload tmux config:   tmux source-file $CONF"
echo "  • Load plugins now:     open tmux, press  prefix + I   (capital i)"
echo "  • Manual save/restore:  prefix + Ctrl-s  /  prefix + Ctrl-r"
echo "  • Timer status:         systemctl --user list-timers $TIMER"
echo
echo "Note: a save file only appears when the layout CHANGES — resurrect dedups"
echo "identical consecutive saves (no new file != failure)."
echo "Saves live in ~/.local/share/tmux/resurrect/ ."
echo
echo "Optional (survive logout too): sudo loginctl enable-linger $USER"
