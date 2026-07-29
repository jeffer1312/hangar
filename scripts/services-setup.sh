#!/usr/bin/env bash
# Run claude-cockpit back + front as persistent systemd *user* services.
#
# They keep running after you close the terminal (and across logout/reboot if
# `loginctl enable-linger $USER` is set). The frontend runs `vite` (`npm run dev`)
# so Vite HMR / fast-refresh stays fully live — edits reload in the browser as usual.
#
# Usage:
#   ./scripts/services-setup.sh                 # install + start (idempotent)
#   ./scripts/services-setup.sh --backend-only  # so o backend (frontend roda noutro lugar)
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
BACK="claude-cockpit-backend.service"
FRONT="claude-cockpit-frontend.service"

# Diretório do npm que a unit systemd vai usar. A unit nasce sem o teu PATH de shell, então
# precisa de um caminho ABSOLUTO e ESTÁVEL.
#   1º) o alias `default` do fnm, se existir: sobrevive ao fim do shell, ao contrário do
#       fnm_multishells, que é por-shell e some;
#   2º) senão, o diretório real do NODE (não do npm!), que cobre node de distro, nvm, asdf,
#       homebrew — e contém node E npm lado a lado.
# Antes só existia (1), cravado: quem instalou node pelo gerenciador da distro não conseguia
# usar os serviços — falhava alto, mas falhava, e não era culpa da máquina dele.
# Resolver pelo NPM seria a escolha óbvia e está errada: `readlink -f $(command -v npm)` cai em
# .../lib/node_modules/npm/bin, que tem npm mas NÃO tem node — a unit nasceria com um PATH que
# roda `npm run dev` e quebra no primeiro spawn de node. Medido nesta máquina.
NODE_BIN="$HOME/.local/share/fnm/aliases/default/bin"
if [[ ! -x "$NODE_BIN/npm" ]] && command -v node >/dev/null; then
  NODE_BIN="$(dirname "$(readlink -f "$(command -v node)")")"
fi
UV_BIN="$(command -v uv || true)"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

# --backend-only: nao instala o servico do frontend. Caso real: o PWA roda numa VPS e cada
# maquina so expoe o backend — UM frontend atende varios servidores (ele guarda a lista em
# cp_servers e o usuario adiciona cada backend pela UI). Subir vite aqui seria porta aberta e
# processo node de graca.
BACKEND_ONLY=0
if [[ "${1:-}" == "--backend-only" ]]; then BACKEND_ONLY=1; shift; fi

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
[[ "$BACKEND_ONLY" == 1 ]] || [[ -x "$NODE_BIN/npm" ]] || { echo "npm nao encontrado em $NODE_BIN — instale Node 20+ (ou, se usa fnm, rode: fnm default <ver>)" >&2; exit 1; }
[[ -d "$REPO/frontend/node_modules" ]] || log "WARNING: frontend/node_modules missing — run 'npm install' in frontend first"

mkdir -p "$SD_DIR"

log "Writing $BACK"
cat > "$SD_DIR/$BACK" <<EOF
[Unit]
Description=claude-cockpit backend (FastAPI/uvicorn)
After=network.target

[Service]
WorkingDirectory=$REPO/backend
# Explicit PATH: the backend spawns \`claude\` (~/.local/bin) and \`tmux\` — the user manager's
# PATH may lack ~/.local/bin on a lingering boot with no login session.
Environment=PATH=$NODE_BIN:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$UV_BIN run python -m app.main
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

if [[ "$BACKEND_ONLY" == 1 ]]; then
  # Nao basta nao habilitar: escrever a unit deixaria um arquivo morto no disco, que aparece no
  # `systemctl --user list-unit-files` e confunde quem for diagnosticar depois. Remove uma que
  # tenha sobrado de uma instalacao anterior COM frontend.
  systemctl --user disable --now "$FRONT" 2>/dev/null || true
  rm -f "$SD_DIR/$FRONT"
  log "Skipping $FRONT (--backend-only)"
else
log "Writing $FRONT"
cat > "$SD_DIR/$FRONT" <<EOF
[Unit]
Description=claude-cockpit frontend (Vite dev, HMR)
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
fi

systemctl --user daemon-reload
if [[ "$BACKEND_ONLY" == 1 ]]; then
  systemctl --user enable --now "$BACK"
  log "Done. Backend up (frontend NAO instalado: --backend-only)."
  echo "  Aponte um frontend ja existente para este backend — veja a URL no QR que ele imprime."
else
  systemctl --user enable --now "$BACK" "$FRONT"
  log "Done. Both services up."
fi
echo
echo "  • Status:   ./scripts/services-setup.sh --status"
echo "  • Logs:     ./scripts/services-setup.sh --logs"
echo "  • Restart:  ./scripts/services-setup.sh --restart"
echo "  • Stop all: ./scripts/services-setup.sh --uninstall"
echo
loginctl show-user "$USER" 2>/dev/null | grep -q 'Linger=yes' \
  && echo "Linger=yes → services survive logout/reboot." \
  || echo "Tip: 'sudo loginctl enable-linger $USER' to survive logout/reboot too."
