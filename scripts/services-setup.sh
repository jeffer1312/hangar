#!/usr/bin/env bash
# Run claude-cockpit back + front as persistent systemd *user* services.
#
# They keep running after you close the terminal (and across logout/reboot if
# `loginctl enable-linger $USER` is set). The frontend serves the BUILD (`npm run preview`),
# not the dev server: o install.sh ja gera o frontend/dist e ninguem o servia — a instalacao
# produzia um artefato de producao e servia desenvolvimento. O bloco `preview` do vite.config.ts
# ja estava pronto pra isso (mesma porta 5173, mesmo proxy /api, mesmos allowedHosts do tailnet),
# entao a origem NAO muda: quem ja usa nao perde localStorage (cp_servers, tema, layout...).
# Pra mexer no layout com recarga ao vivo: pare o servico e rode `npm run dev` na mao.
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

# Quem serve a interface. Duas formas, e a diferenca NAO e cosmetica:
#   backend  — o 8765 serve o dist E a API. Um servico so, um endereco so. O servico do front
#              nem e instalado; quem for mexer no layout roda `npm run dev` na mao.
#   preview  — servico separado no 5173 servindo o BUILD (`vite preview`), API por proxy.
# O `dev` deixou de ser opcao de INSTALACAO: o install.sh sempre buildou o dist e ninguem o
# servia, entao a instalacao produzia artefato de producao e servia servidor de desenvolvimento.
#
# Padrao 'backend' vale so pra instalacao NOVA. Quem ja tem a unit do front no disco mantem o
# que tem: trocar a porta muda a ORIGEM, e origem nova = localStorage vazio (cp_servers com os
# tokens, tema, layout do canvas). Ninguem perde configuracao por causa de um `git pull`.
SERVE="${CP_SERVE:-}"
FRONT_JA_EXISTE=0
[[ -f "$SD_DIR/$FRONT" ]] && FRONT_JA_EXISTE=1

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

if [[ "$BACKEND_ONLY" == 0 && -z "$SERVE" ]]; then
  if [[ "$FRONT_JA_EXISTE" == 1 ]]; then
    # Instalacao existente: NAO pergunta e NAO muda. Mexer aqui trocaria a porta de quem ja usa.
    SERVE=preview
    log "Frontend ja instalado — mantendo o servico no 5173 (use CP_SERVE=backend pra trocar)."
  elif [[ -t 0 ]]; then
    echo
    echo "  Quem serve a interface?"
    echo "    1) o backend, no 8765 — um servico so, um endereco so (recomendado)"
    echo "    2) servico separado no 5173, servindo o build"
    read -rp "  [1/2, Enter = 1]: " _r
    case "${_r:-1}" in 2) SERVE=preview ;; *) SERVE=backend ;; esac
  else
    SERVE=backend   # nao-interativo (CI, curl|bash): o padrao da instalacao nova
  fi
fi
[[ "$BACKEND_ONLY" == 1 ]] && SERVE=backend

mkdir -p "$SD_DIR"

# Escreve a unit e diz se ela MUDOU. Re-rodar o script e o jeito de aplicar uma atualizacao
# (o caminho do node e o WorkingDirectory ficam cravados dentro da unit, entao `git pull`
# sozinho nao muda nada), mas reiniciar o backend sem necessidade derruba as conexoes SSE do
# celular. Reinicia so o que de fato mudou.
MUDOU=()
escreve_unit() { # escreve_unit <nome> <conteudo>
  local nome=$1 conteudo=$2 destino="$SD_DIR/$1"
  if [[ -f "$destino" ]] && [[ "$(cat "$destino")" == "$conteudo" ]]; then
    log "$nome sem mudanca"
  else
    printf '%s' "$conteudo" > "$destino"
    MUDOU+=("$nome")
    log "Writing $nome"
  fi
}

escreve_unit "$BACK" "$(cat <<EOF
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
)"

if [[ "$BACKEND_ONLY" == 1 || "$SERVE" == backend ]]; then
  # Nao basta nao habilitar: escrever a unit deixaria um arquivo morto no disco, que aparece no
  # `systemctl --user list-unit-files` e confunde quem for diagnosticar depois. Remove uma que
  # tenha sobrado de uma instalacao anterior COM frontend.
  systemctl --user disable --now "$FRONT" 2>/dev/null || true
  rm -f "$SD_DIR/$FRONT"
  if [[ "$BACKEND_ONLY" == 1 ]]; then log "Skipping $FRONT (--backend-only)"; else log "Skipping $FRONT (o backend serve a interface no 8765)"; fi
else
escreve_unit "$FRONT" "$(cat <<EOF
[Unit]
Description=claude-cockpit frontend (Vite preview, serve o build)
After=network.target

[Service]
WorkingDirectory=$REPO/frontend
Environment=PATH=$NODE_BIN:/usr/local/bin:/usr/bin:/bin
ExecStart=$NODE_BIN/npm run preview
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
)"
fi

systemctl --user daemon-reload
if [[ ${#MUDOU[@]} -gt 0 ]]; then
  systemctl --user restart "${MUDOU[@]}"
  log "Reiniciado (unit mudou): ${MUDOU[*]}"
fi
if [[ "$BACKEND_ONLY" == 1 || "$SERVE" == backend ]]; then
  systemctl --user enable --now "$BACK"
  if [[ "$SERVE" == backend && "$BACKEND_ONLY" == 0 ]]; then
    log "Done. Backend up, servindo a interface em http://127.0.0.1:8765/"
    echo "  Pra desenvolver com recarga ao vivo: npm --prefix frontend run dev (porta 5173)"
  else
  log "Done. Backend up (frontend NAO instalado: --backend-only)."
  echo "  Aponte um frontend ja existente para este backend — veja a URL no QR que ele imprime."
  fi
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
