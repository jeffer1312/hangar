#!/usr/bin/env bash
# Testa o install.sh em modo pacote (a pasta tem manifest.json e não tem .git), sem instalar nada:
# a sandbox do test-install-app.sh, sem uv, sem npm e sem o python3 da máquina; o Python do backend é
# um hangar-python falso que registra as chamadas.
#
#   ./scripts/test-install-pacote.sh
set -uo pipefail

# shellcheck source=scripts/test-install-app.sh
source "$(dirname "${BASH_SOURCE[0]}")/test-install-app.sh" --lib
trap 'rm -rf "$ROOT"' EXIT

# pacote_sandbox <nome>: a sandbox vira uma versão extraída do pacote.
pacote_sandbox() {
  new_sandbox "$1"
  echo '{"version":"0.1.0.42","commit":"c","channel":"latest"}' > "$R/manifest.json"
  rm -rf "$R/frontend" "$B/uv" "$S/sys/python3"
  # O Python embutido: registra a pasta e os argumentos, e sai 0.
  printf '#!/bin/sh\necho "$(pwd) $*" >> "%s/log/python"\nexit 0\n' "$S" > "$R/scripts/hangar-python"
  chmod +x "$R/scripts/hangar-python"
  fake node 'case "$1" in --version) echo v22.0.0 ;; esac; exit 0'
  fake npm 'exit 0'
}

# --- Caso: instalação nova pelo assistente, sem git, uv, npm nem python3 na máquina ---
pacote_sandbox novo
TEST_TOKEN='segredo-do-app-123' run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "novo: sai 0" "$rc" 0
expect_eq "novo: FIM ok" "$(last_line "$S/out")" '##HANGAR-FIM## ok'
expect_line "novo: senha em ~/.hangar/config/.env" "$S/home/.hangar/config/.env" '^CP_AUTH_TOKEN=segredo-do-app-123$'
[ -e "$R/backend/.env" ] && flunk "novo: nada gravado no backend/.env da versão" || pass "novo: nada gravado no backend/.env da versão"
expect_eq "novo: config só do usuário" "$(stat -c %a "$S/home/.hangar/config")" 700
expect_no "novo: nenhum uv" "$S/out" 'uv: (command )?not found|uv sync'
expect_no "novo: nenhum npm" "$S/log/calls" '^npm '
expect_no "novo: nenhum git" "$S/log/calls" '^git '
expect_line "novo: push pelo Python do pacote" "$S/log/python" "^$R/backend -m app.vapid_setup$"
expect_line "novo: fumaça pelo Python do pacote" "$S/log/python" "^$R/backend -c from app import api"
expect_line "novo: tela pronta" "$S/out" 'a tela vem pronta com o pacote'
expect_line "novo: binários do pacote" "$S/out" 'hangar-server e hangar-cano: vieram no pacote'
expect_line "novo: serviço sem --backend-only" "$S/log/scripts" 'services-setup.sh $'
expect_line "novo: hangar-send e skills" "$S/log/scripts" '^\./scripts/install-hangar-send\.sh'
expect_no "novo: passos já marcados pelo install-package" "$S/out" 'marcados como já aplicados|não consegui marcar'
expect_no "novo: sem hook de git pull" "$S/out" 'sem .git|hook de atualização'
SHIM="$S/home/.hangar/cache/install-bin/python3"
expect_line "novo: python3 dos sub-scripts é o do pacote" "$SHIM" "exec \"$R/scripts/hangar-python\""

# --- Caso: --check no pacote confere sem uv e sem gravar ---
pacote_sandbox check
run_install "$S/out" --check --no-frontend; rc=$?
expect_eq "check: sai 0" "$rc" 0
expect_line "check: diagnóstico pelo Python do pacote" "$S/log/python" "^$R/backend -m app.doctor$"
[ -e "$S/home/.hangar/config" ] && flunk "check: não grava nada" || pass "check: não grava nada"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
