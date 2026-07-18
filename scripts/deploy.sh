#!/usr/bin/env bash
# Auto-deploy do claude-cockpit: pull da main -> build -> restart. Disparado pela unit
# 'claude-cockpit-deploy.service' (que o webhook do GitHub aciona), ou manual pra testar.
#
#   ./scripts/deploy.sh            # deploy se houver commit novo na origin/main
#   ./scripts/deploy.sh --force    # rebuild+restart mesmo sem commit novo
#
# Garantias:
#   - ff-only: se o repo divergiu de origin/main, ABORTA (nao destroi trabalho local).
#   - backup dist -> build in-place -> se falhar, RESTAURA o dist antigo. Rollback real: build
#     quebrado volta pra versao anterior (nao pode buildar em outDir separado: o vite-plugin-pwa
#     em modo injectManifest quebra com outDir trocado -> swSrc/swDest colidem).
#   - restart SO acontece depois de build ok.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NODE_BIN="$HOME/.local/share/fnm/aliases/default/bin"
export PATH="$NODE_BIN:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

BACK="claude-cockpit-backend.service"
FRONT="claude-cockpit-frontend.service"
FORCE="${1:-}"

log() { printf '%s [deploy] %s\n' "$(date '+%F %T')" "$*"; }

cd "$REPO"

git fetch --quiet origin main
LOCAL="$(git rev-parse HEAD)"
REMOTE="$(git rev-parse origin/main)"

if [[ "$LOCAL" == "$REMOTE" && "$FORCE" != "--force" ]]; then
  log "sem commit novo ($LOCAL) — nada a fazer."
  exit 0
fi

log "deploy $LOCAL -> $REMOTE"

# ff-only: nao faz merge nem reset --hard. Se divergiu, falha aqui e mantem tudo intacto.
if ! git merge --ff-only origin/main; then
  log "ERRO: repo divergiu de origin/main (nao e fast-forward). Abortando sem tocar em nada."
  exit 1
fi

# --- Frontend: build isolado + swap ---
cd "$REPO/frontend"

# node_modules em dia so quando o lock mudou (ci e lento; roda so quando precisa).
if ! git diff --quiet "$LOCAL" HEAD -- package-lock.json 2>/dev/null; then
  log "package-lock.json mudou -> npm ci"
  npm ci
fi

# Backup do dist atual ANTES do build. O vite esvazia o dist no inicio (emptyOutDir), entao um
# build que falha no meio deixaria o dist parcial -> restauramos do backup nesse caso.
log "backup dist"
rm -rf dist.bak
[[ -d dist ]] && cp -a dist dist.bak

log "build"
if ! npm run build; then
  log "ERRO: build falhou -> restaurando dist anterior, sem restart."
  rm -rf dist
  [[ -d dist.bak ]] && mv dist.bak dist
  exit 1
fi
rm -rf dist.bak

# --- Backend: deps so quando o lock mudou ---
cd "$REPO/backend"
if ! git diff --quiet "$LOCAL" HEAD -- uv.lock pyproject.toml 2>/dev/null; then
  log "uv.lock/pyproject mudou -> uv sync"
  uv sync --quiet || log "AVISO: uv sync falhou (segue com deps atuais)."
fi

# --- Restart (so chega aqui com build ok) ---
log "restart $BACK + $FRONT"
systemctl --user restart "$BACK" "$FRONT"

log "deploy concluido: $(git rev-parse --short HEAD)"
