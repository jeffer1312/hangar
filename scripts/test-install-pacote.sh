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
KEY=$(printf 'a%.0s' $(seq 64))
pacote_sandbox() {
  new_sandbox "$1"
  printf '{\n  "channel": "latest",\n  "commit": "c",\n  "dist_key": "%s",\n  "version": "0.1.0.42"\n}\n' "$KEY" > "$R/manifest.json"
  rm -rf "$R/frontend" "$B/uv" "$S/sys/python3"
  # A tela da chave, como o install-package a deixa.
  mkdir -p "$S/home/.hangar/ui/$KEY" && : > "$S/home/.hangar/ui/$KEY/index.html"
  # O Python embutido: registra a pasta e os argumentos, e sai 0.
  printf '#!/bin/sh\necho "$(pwd) $*" >> "%s/log/python"\nexit 0\n' "$S" > "$R/scripts/hangar-python"
  chmod +x "$R/scripts/hangar-python"
  # O hangar-server do pacote: registra os argumentos; falha com $S/mark-fails.
  mkdir -p "$R/bin"
  printf '#!/bin/sh\necho "$*" >> "%s/log/server"\necho "aviso: passo malformado ignorado: torto.md"\n[ ! -e "%s/mark-fails" ]\n' "$S" "$S" > "$R/bin/hangar-server"
  chmod +x "$R/bin/hangar-server"
  fake node 'case "$1" in --version) echo v22.0.0 ;; esac; exit 0'
  fake npm 'exit 0'
}

# --- Caso: instalação nova pelo assistente, sem git, uv, npm nem python3 na máquina ---
pacote_sandbox novo
TEST_FRESH=1 TEST_TOKEN='segredo-do-app-123' run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
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
expect_eq "novo: passos marcados no fim, pelo hangar-server do pacote" "$(cat "$S/log/server" 2>/dev/null)" 'install-package --mark-steps'
expect_line "novo: passo malformado aparece" "$S/out" 'passo malformado ignorado: torto.md'
before "$S/out" 'o backend importa' 'passo malformado' && pass "novo: marca depois da fumaça" || flunk "novo: marca depois da fumaça" "$S/out"
expect_no "novo: sem hook de git pull" "$S/out" 'sem .git|hook de atualização'
SHIM="$S/home/.hangar/cache/install-bin/python3"
expect_line "novo: python3 dos sub-scripts é o do pacote" "$SHIM" "exec \"$R/scripts/hangar-python\""

# --- Caso: rodou de novo numa instalação que já existia: nenhum passo marcado, e diz por quê ---
pacote_sandbox de-novo
run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "de-novo: sai 0" "$rc" 0
[ -e "$S/log/server" ] && flunk "de-novo: nada marcado" "$S/out" || pass "de-novo: nada marcado"
expect_line "de-novo: diz que os pendentes ficam para o Atualizar" "$S/out" 'nenhum passo de atualização marcado.*Atualizar'

# --- Caso: a tela da chave do pacote não veio: pendência, não "vem pronta" ---
pacote_sandbox sem-tela
rm -rf "$S/home/.hangar/ui"
run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "sem-tela: sai 1" "$rc" 1
expect_no "sem-tela: não diz que vem pronta" "$S/out" 'a tela vem pronta'
expect_line "sem-tela: pendência com a chave" "$S/out" "^##HANGAR-PENDENCIA## .*tela web.*$KEY"
expect_eq "sem-tela: FIM pendente" "$(last_line "$S/out")" '##HANGAR-FIM## pendente'

# --- Caso: tela web desligada nesta máquina: a falta da pasta não é problema ---
pacote_sandbox tela-desligada
rm -rf "$S/home/.hangar/ui"; echo '{"web_ui": false}' > "$S/home/.hangar/web-ui.json"
run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "tela-desligada: sai 0" "$rc" 0
expect_line "tela-desligada: diz que está desligada" "$S/out" 'tela web desligada'
expect_no "tela-desligada: sem pendência" "$S/out" '^##HANGAR-PENDENCIA##'

# --- Caso: a marca dos passos falhou: pendência ---
pacote_sandbox marca-falha
: > "$S/mark-fails"
TEST_FRESH=1 run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "marca-falha: sai 1" "$rc" 1
expect_line "marca-falha: pendência" "$S/out" '^##HANGAR-PENDENCIA## .*passos de atualização'

# --- Caso: a instalação parou antes do fim: nenhum passo marcado ---
pacote_sandbox falha-cedo
fake tmux 'exit 1'
TEST_FRESH=1 run_install "$S/out" --app --tailscale=nao --sem-nativo; rc=$?
expect_eq "falha-cedo: sai 1" "$rc" 1
[ -e "$S/log/server" ] && flunk "falha-cedo: nada marcado" "$S/out" || pass "falha-cedo: nada marcado"

# --- install-native.sh: a primeira conexão do app leva a senha de ~/.hangar/config/.env ---
# native_box <nome>: N (cópia do Hangar com o install-native.sh de verdade), H (HOME) e NREL (a release falsa).
native_box() {
  N="$ROOT/$1/hangar"; H="$ROOT/$1/home"; NREL="$ROOT/$1/rel"
  mkdir -p "$N/scripts" "$N/backend" "$H" "$NREL/pkg"
  cp "$REPO/scripts/install-native.sh" "$N/scripts/"
  printf '#!/bin/sh\nexit 0\n' > "$N/scripts/install-chromium.sh"; chmod +x "$N/scripts/install-chromium.sh"
  printf '#!/bin/sh\nmkdir -p "$HOME/.local/bin" && printf "#!/bin/sh\\n" > "$HOME/.local/bin/hangar-native" && chmod +x "$HOME/.local/bin/hangar-native"\n' > "$NREL/pkg/install-linux.sh"
  chmod +x "$NREL/pkg/install-linux.sh"
  tar -czf "$NREL/Hangar-linux-x86_64.tar.gz" -C "$NREL/pkg" install-linux.sh
  printf '{"version": "1.2.3", "files": {"Hangar-linux-x86_64.tar.gz": "%s"}}\n' \
    "$(sha256sum "$NREL/Hangar-linux-x86_64.tar.gz" | cut -d' ' -f1)" > "$NREL/native-latest.json"
}
run_native() { env -i HOME="$H" PATH=/usr/bin:/bin HANGAR_NATIVE_UPDATE_URL="file://$NREL" "$N/scripts/install-native.sh" > "$1" 2>&1; }
if [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ]; then
  native_box nativo
  mkdir -p "$H/.hangar/config"; printf 'CP_AUTH_TOKEN=tok-do-pacote\nCP_PORT=9876\n' > "$H/.hangar/config/.env"
  run_native "$ROOT/nativo/out"
  expect_line "nativo: conexão com a senha do config" "$H/.config/hangar-native/connection.json" '"address": "http://127.0.0.1:9876", "token": "tok-do-pacote"'

  native_box nativo-sem-senha
  run_native "$ROOT/nativo-sem-senha/out"
  expect_line "nativo-sem-senha: aviso visível" "$ROOT/nativo-sem-senha/out" 'sem a senha do backend'
  [ -e "$H/.config/hangar-native/connection.json" ] && flunk "nativo-sem-senha: sem conexão gravada" || pass "nativo-sem-senha: sem conexão gravada"
else
  echo "pulado: install-native.sh só tem pacote de teste para Linux x86_64"
fi

# --- hangar-doctor e show-qr.sh pelo Python do pacote, sem uv ---
pacote_sandbox doctor
cp "$REPO/scripts/hangar-doctor" "$REPO/scripts/show-qr.sh" "$R/scripts/"
env -i HOME="$S/home" PATH="$S/sys" "$R/scripts/hangar-doctor" --qr > "$S/out" 2>&1
env -i HOME="$S/home" PATH="$S/sys" "$R/scripts/show-qr.sh" >> "$S/out" 2>&1
expect_line "doctor: pelo hangar-python" "$S/log/python" "^$R/backend -m app.doctor --qr$"
expect_line "show-qr: pelo hangar-python" "$S/log/python" "^$R/backend -c from app.config import settings"

# --- Caso: --check no pacote confere sem uv e sem gravar ---
pacote_sandbox check
run_install "$S/out" --check --no-frontend; rc=$?
expect_eq "check: sai 0" "$rc" 0
expect_line "check: diagnóstico pelo Python do pacote" "$S/log/python" "^$R/backend -m app.doctor$"
[ -e "$S/home/.hangar/config" ] && flunk "check: não grava nada" || pass "check: não grava nada"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
