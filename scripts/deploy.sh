#!/usr/bin/env bash
# Auto-deploy do hangar: pull da main -> build -> restart. Disparado pela unit
# 'hangar-deploy.service' (que o webhook do GitHub aciona), ou manual pra testar.
#
#   ./scripts/deploy.sh            # deploy se houver commit novo na origin/main
#   ./scripts/deploy.sh --force    # rebuild+restart mesmo sem commit novo
#
# Garantias:
#   - ff-only: se o repo divergiu de origin/main, ABORTA (nao destroi trabalho local).
#   - backup dist -> build in-place -> se falhar, RESTAURA o dist antigo. Rollback real: build
#     quebrado volta pra versao anterior (nao pode buildar em outDir separado: o vite-plugin-pwa
#     em modo injectManifest quebra com outDir trocado -> swSrc/swDest colidem).
#   - restart só conclui após HTTP válido e PID novo estável; falha restaura código, dist e deps.
set -Eeuo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NODE_BIN="$HOME/.local/share/fnm/aliases/default/bin"
export PATH="$NODE_BIN:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

BACK="hangar-backend.service"
FRONT="hangar-frontend.service"
FORCE="${1:-}"

log() { printf '%s [deploy] %s\n' "$(date '+%F %T')" "$*"; }

cd "$REPO"

exec 9>"$(git rev-parse --git-path hangar-deploy.lock)"
flock -n 9 || { log "ERRO: outro deploy está em execução."; exit 1; }
if ! git diff --quiet || ! git diff --cached --quiet; then
  log "ERRO: há alterações locais versionadas; deploy cancelado para preservá-las."
  exit 1
fi

git fetch --quiet origin main
LOCAL="$(git rev-parse HEAD)"
REMOTE="$(git rev-parse origin/main)"

if [[ "$LOCAL" == "$REMOTE" && "$FORCE" != "--force" ]]; then
  log "sem commit novo ($LOCAL) — nada a fazer."
  exit 0
fi

log "deploy $LOCAL -> $REMOTE"

# A porta como o servidor a lê (CP_PORT do ambiente, senão do .env da VPS, senão 8765), pelo Rust.
. "$REPO/scripts/hangar-server-lookup"
if ! hangar_server_find "$REPO"; then
  log "ERRO: hangar-server não encontrado; não dá para resolver a porta do backend."; exit 1
fi
PORT="$("$HANGAR_SERVER_BIN" deploy-port --backend "$REPO/backend")" || PORT=""
[[ "$PORT" =~ ^[0-9]+$ ]] && (( PORT > 0 && PORT <= 65535 )) || {
  log "ERRO: não foi possível resolver a porta do backend ($HANGAR_SERVER_BIN deploy-port)."; exit 1;
}
HEALTH_URL="http://127.0.0.1:$PORT/api/peers/ping"
UNITS=("$BACK")
if systemctl --user list-unit-files "$FRONT" >/dev/null 2>&1; then
  UNITS+=("$FRONT")
fi
pid() { systemctl --user show -p MainPID --value "$BACK"; }
OLD_PID="$(pid)"
BACKUP="$(mktemp -d -t hangar-deploy.XXXXXX)"
KEEP_BACKUP=0
cleanup() { if (( ! KEEP_BACKUP )); then rm -rf -- "$BACKUP"; fi; }
trap cleanup EXIT
HAD_DIST=0
if [[ -d frontend/dist ]]; then
  cp -a frontend/dist "$BACKUP/dist"
  HAD_DIST=1
fi
NODE_CHANGED=0
git diff --quiet "$LOCAL" "$REMOTE" -- package-lock.json || NODE_CHANGED=1
NODE_TOUCHED=0
RESTART_ATTEMPTED=0
DEPLOY_STARTED="$(date '+%F %T')"

wait_healthy() {
  local previous="$1" current after last="" consecutive=0 code attempt unit active
  for (( attempt=1; attempt<=10; attempt++ )); do
    current="$(pid)" || return 1
    code="$(curl --silent --noproxy '*' --connect-timeout 1 --max-time 2 \
      -o "$BACKUP/health.json" -w '%{http_code}' "$HEALTH_URL")" || code="000"
    after="$(pid)" || return 1
    active=1
    for unit in "${UNITS[@]}"; do
      systemctl --user is-active --quiet "$unit" || active=0
    done
    # Só a saúde do Hangar (`{"hangar": true, …}` no topo) conta: outro programa na porta responde 200
    # também. Pelo node, que o build do front já exige.
    if [[ "$code" == 200 && "$current" =~ ^[1-9][0-9]*$ \
          && "$current" != "$previous" && "$current" == "$after" && "$active" == 1 ]] \
        && node -e 'let j; try { j = JSON.parse(require("fs").readFileSync(0, "utf8")) } catch { process.exit(1) }
                    process.exit(j !== null && typeof j === "object" && !Array.isArray(j) && j.hangar === true ? 0 : 1)' \
          < "$BACKUP/health.json" 2>/dev/null; then
      if [[ "$current" == "$last" ]]; then
        consecutive=$((consecutive + 1))
      else
        consecutive=1
      fi
      last="$current"
      if (( consecutive >= 3 )); then
        log "saúde confirmada: HTTP 200, três respostas consecutivas, PID $current."
        return 0
      fi
    else
      consecutive=0
      last=""
    fi
    log "sonda $attempt/10: HTTP $code, PID $current, respostas estáveis $consecutive/3."
    if (( attempt < 10 )); then sleep 2; fi
  done
  return 1
}

rollback() {
  local code="$1" rejected_pid
  trap - ERR
  KEEP_BACKUP=1
  log "ERRO: SHA $REMOTE rejeitado na etapa '$STAGE' (código $code). Rollback para $LOCAL."
  if (( RESTART_ATTEMPTED )); then
    if ! journalctl --user -u "$BACK" --since "$DEPLOY_STARTED" -n 100 --no-pager \
        > "$BACKUP/backend-rejeitado.log" 2>&1; then
      log "AVISO: não foi possível capturar o journal do backend."
    fi
  fi
  log "backup e diagnóstico privado: $BACKUP"
  rejected_pid="$(pid)" || rejected_pid=0
  if (( RESTART_ATTEMPTED )) && ! systemctl --user stop "${UNITS[@]}"; then
    log "CRITICO: rollback não conseguiu parar os serviços; recuperação manual necessária."
    exit 2
  fi
  # --keep recusa sobrescrever alterações concorrentes; não apaga trabalho local.
  if ! git -C "$REPO" reset --keep "$LOCAL"; then
    log "CRITICO: rollback do Git falhou; backup preservado, sem nova tentativa."
    exit 2
  fi
  if ! rm -rf -- "$REPO/frontend/dist"; then
    log "CRITICO: rollback não conseguiu remover o dist rejeitado."; exit 2
  fi
  if (( HAD_DIST )) && ! cp -a "$BACKUP/dist" "$REPO/frontend/dist"; then
    log "CRITICO: rollback não conseguiu restaurar o dist."; exit 2
  fi
  if (( NODE_TOUCHED )) && ! (cd "$REPO" && npm ci --workspace=@hangar/core --workspace=frontend); then
    log "CRITICO: rollback das dependências do front falhou."; exit 2
  fi
  if (( RESTART_ATTEMPTED )); then
    systemctl --user reset-failed "$BACK" || log "AVISO: reset-failed não pôde ser aplicado."
    if ! systemctl --user restart "${UNITS[@]}"; then
      systemctl --user stop "${UNITS[@]}" || log "CRITICO: não foi possível interromper o crash-loop."
      log "CRITICO: restart do rollback falhou; sem nova tentativa."; exit 2
    fi
  else
    rejected_pid=0
  fi
  if ! wait_healthy "$rejected_pid"; then
    if ! journalctl --user -u "$BACK" --since "$DEPLOY_STARTED" -n 100 --no-pager \
        > "$BACKUP/backend-rollback.log" 2>&1; then
      log "AVISO: não foi possível capturar o journal do backend."
    fi
    if (( RESTART_ATTEMPTED )); then
      systemctl --user stop "${UNITS[@]}" || log "CRITICO: não foi possível interromper o crash-loop."
    fi
    log "CRITICO: rollback também falhou na sonda; recuperação manual necessária, sem novas tentativas."
    exit 2
  fi
  log "rollback concluído: $LOCAL novamente saudável; deploy rejeitado."
  exit 1
}

# A função é lida inteira antes do merge: trocar deploy.sh no checkout não troca comandos em voo.
deploy() {
  # ff-only: se divergiu, falha aqui e mantém o checkout intacto.
  if ! git merge --ff-only origin/main; then
    log "ERRO: repo divergiu de origin/main (nao e fast-forward). Abortando sem tocar em nada."
    exit 1
  fi

  STAGE="preparação"
  trap 'rollback "$?"' ERR

  # --- Frontend ---
  # O `npm ci` roda na RAIZ (`frontend` não tem lockfile proprio, e o `@hangar/core` so existe como
  # link criado por instalacao na raiz) e é SELETIVO: o app nativo tambem e workspace — precisa ser,
  # senao o EAS Build nao detecta o monorepo —, e sem os dois `--workspace` este passo baixaria o
  # toolchain do React Native numa VPS que nunca vai compilar o app. O build continua so do frontend.
  cd "$REPO"

  # node_modules em dia so quando o lock mudou (ci e lento; roda so quando precisa).
  STAGE="dependências do front"
  if (( NODE_CHANGED )); then
    NODE_TOUCHED=1
    log "package-lock.json mudou -> npm ci"
    npm ci --workspace=@hangar/core --workspace=frontend
  fi

  # Backup do dist atual ANTES do build. O vite esvazia o dist no inicio (emptyOutDir), entao um
  # build que falha no meio deixaria o dist parcial -> restauramos do backup nesse caso.
  # Caminhos com `frontend/` na frente: o cwd agora e a RAIZ, por causa do workspace.
  STAGE="build"
  log "build"
  npm run build -w frontend

  # --- Restart (so chega aqui com build ok) ---
  # O FRONT so existe quando a instalacao escolheu servico separado no 5173; com o backend servindo a
  # UI (o padrao novo) a unit nao existe, e um restart nela derruba o deploy inteiro no set -e — build
  # feito, servico velho no ar, deploy marcado como failed. Duas topologias sao validas: pergunta.
  STAGE="restart"
  RESTART_ATTEMPTED=1
  log "restart ${UNITS[*]}"
  systemctl --user restart "${UNITS[@]}"

  STAGE="saúde pós-restart"
  wait_healthy "$OLD_PID"
  trap - ERR
  log "deploy concluido: $(git rev-parse --short HEAD)"
}

deploy
