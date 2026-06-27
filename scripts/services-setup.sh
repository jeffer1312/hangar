#!/usr/bin/env bash
# Run claude-pocket back + front as persistent systemd *user* services.
#
# They keep running after you close the terminal (and across logout/reboot if
# `loginctl enable-linger $USER` is set). The frontend runs `vite` (`npm run dev`)
# so Vite HMR / fast-refresh stays fully live — edits reload in the browser as usual.
#
# Usage:
#   ./scripts/services-setup.sh              # install + start (idempotent)
#   ./scripts/services-setup.sh --status     # show status + recent logs
#   ./scripts/services-setup.sh --logs       # tail both services live
#   ./scripts/services-setup.sh --restart    # restart both
#   ./scripts/services-setup.sh --uninstall  # stop + remove the units
#
# Backend config comes from backend/.env (CP_AUTH_TOKEN, CP_LAN_BIND_IP, ...) — same as
# the manual `uv run python -m app.main`. Safe to re-run.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SD_DIR="$HOME/.config/systemd/user"
BACK="claude-pocket-backend.service"
FRONT="claude-pocket-frontend.service"

# Stable node/npm dir (fnm's `default` alias — survives shell exit, unlike the
# ephemeral fnm_multishells PATH). uvicorn is launched via the system `uv`.
NODE_BIN="$HOME/.local/share/fnm/aliases/default/bin"
UV_BIN="$(command -v uv || true)"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

case "${1:-}" in
  --uninstall)
    systemctl --user disable --now "$BACK" "$FRONT" 2>/dev/null || true
    rm -f "$SD_DIR/$BACK" "$SD_DIR/$FRONT"
    systemctl --user daemon-reload
    log "Removed both services."
    exit 0 ;;
  --status)
    systemctl --user --no-pager status "$BACK" "$FRONT" || true
    exit 0 ;;
  --logs)
    exec journalctl --user -u "$BACK" -u "$FRONT" -f ;;
  --restart)
    systemctl --user restart "$BACK" "$FRONT"
    log "Restarted both."
    exit 0 ;;
esac

[[ -n "$UV_BIN" ]] || { echo "uv not found in PATH" >&2; exit 1; }
[[ -x "$NODE_BIN/npm" ]] || { echo "npm not found at $NODE_BIN (is fnm 'default' set? run: fnm default <ver>)" >&2; exit 1; }
[[ -d "$REPO/frontend/node_modules" ]] || log "WARNING: frontend/node_modules missing — run 'npm install' in frontend first"

mkdir -p "$SD_DIR"

log "Writing $BACK"
cat > "$SD_DIR/$BACK" <<EOF
[Unit]
Description=claude-pocket backend (FastAPI/uvicorn)
After=network.target

[Service]
WorkingDirectory=$REPO/backend
ExecStart=$UV_BIN run python -m app.main
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

log "Writing $FRONT"
cat > "$SD_DIR/$FRONT" <<EOF
[Unit]
Description=claude-pocket frontend (Vite dev, HMR)
After=network.target

[Service]
WorkingDirectory=$REPO/frontend
Environment=PATH=$NODE_BIN:/usr/local/bin:/usr/bin:/bin
ExecStart=$NODE_BIN/npm run dev
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now "$BACK" "$FRONT"

log "Done. Both services up."
echo
echo "  • Status:   ./scripts/services-setup.sh --status"
echo "  • Logs:     ./scripts/services-setup.sh --logs"
echo "  • Restart:  ./scripts/services-setup.sh --restart"
echo "  • Stop all: ./scripts/services-setup.sh --uninstall"
echo
loginctl show-user "$USER" 2>/dev/null | grep -q 'Linger=yes' \
  && echo "Linger=yes → services survive logout/reboot." \
  || echo "Tip: 'sudo loginctl enable-linger $USER' to survive logout/reboot too."
