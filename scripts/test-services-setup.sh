#!/usr/bin/env bash
# Trava da unit do backend numa máquina migrada para o pacote: o services-setup.sh (que o
# `install.sh --update` roda) mantém a unit no lançador estável e lê/grava o `.env` de
# ~/.hangar/config, em vez de voltar ao `uv run` do checkout. HOME e programas falsos; nunca toca
# o systemd de verdade.
#
#   ./scripts/test-services-setup.sh            # o script deste checkout
#   SCRIPT=/outro/services-setup.sh ./scripts/test-services-setup.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${SCRIPT:-$REPO/scripts/services-setup.sh}"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; fail=1; }
expect_line() { if grep -Fqx -- "$3" "$2" 2>/dev/null; then pass "$1"; else flunk "$1 (sem a linha '$3')"; sed 's/^/     | /' "$2" 2>/dev/null; fi; }
expect_no()   { if grep -Eq -- "$3" "$2" 2>/dev/null; then flunk "$1 (achou /$3/)"; else pass "$1"; fi; }

# sandbox <nome> <migrada 0|1> [opções; padrão --backend-only, SEM_OPCOES=1 roda sem nenhuma]:
# repositório falso com o script, HOME e programas falsos. PACOTE_REPO=1 faz o repositório ser um
# pacote extraído (manifest.json, sem .git).
sandbox() {
  local nome=$1 migrada=$2; shift 2
  [ $# -gt 0 ] || [ "${SEM_OPCOES:-0}" = 1 ] || set -- --backend-only
  S="$ROOT/$nome"; R="$S/repo"; B="$S/bin"; H="$S/home"
  mkdir -p "$R/scripts" "$R/backend" "$B" "$H/.config/systemd/user" "$S/sys"
  [ "${PACOTE_REPO:-0}" = 1 ] && echo '{}' > "$R/manifest.json"
  cp "$SCRIPT" "$R/scripts/services-setup.sh"
  printf 'CP_AUTH_TOKEN=velho\n' > "$R/backend/.env"
  for t in bash sh env cat grep sed tr cut tail head dirname readlink printf mkdir rm; do
    p=$(command -v "$t") && ln -sf "$p" "$S/sys/$t"
  done
  local fakes="systemctl loginctl uv"
  [ "${PACOTE_REPO:-0}" = 1 ] && fakes="systemctl loginctl"   # o pacote não tem uv, npm nem node
  for t in $fakes; do printf '#!/bin/sh\necho "%s $*" >> "%s/calls"\nexit 0\n' "$t" "$S" > "$B/$t"; chmod +x "$B/$t"; done
  if [ "$migrada" = 1 ]; then
    mkdir -p "$H/.hangar/config" "$H/.hangar/bin" "$H/.hangar/versions/0.1.0.5"
    ln -s "$H/.hangar/versions/0.1.0.5" "$H/.hangar/current"
    printf '#!/bin/sh\n' > "$H/.hangar/bin/hangar-launcher"; chmod +x "$H/.hangar/bin/hangar-launcher"
    printf 'CP_AUTH_TOKEN=novo\nCP_FRONT_PORT=5173\n' > "$H/.hangar/config/.env"
  fi
  RC=0
  ( cd "$R" && env -i HOME="$H" USER=teste PATH="$B:$S/sys" bash scripts/services-setup.sh "$@" >"$S/out" 2>&1 ) || RC=$?
  [ "${ESPERA_FALHA:-0}" = 1 ] || [ "$RC" = 0 ] || { flunk "$nome: o script falhou"; sed 's/^/     | /' "$S/out"; }
  UNIT="$H/.config/systemd/user/hangar-backend.service"
}

sandbox migrada 1
expect_line "migrada: unit chama o lançador" "$UNIT" "ExecStart=\"$H/.hangar/bin/hangar-launcher\" launch backend"
expect_line "migrada: fora do checkout" "$UNIT" "WorkingDirectory=$H/.hangar"
expect_no "migrada: sem uv run" "$UNIT" 'uv run'
expect_no "migrada: CP_FRONT_PORT saiu do .env novo" "$H/.hangar/config/.env" 'CP_FRONT_PORT'
expect_line "migrada: o .env velho não foi tocado" "$R/backend/.env" 'CP_AUTH_TOKEN=velho'

sandbox antiga 0
expect_line "antiga: unit no uv do checkout" "$UNIT" "ExecStart=$B/uv run python -m app.main"
expect_line "antiga: no backend do checkout" "$UNIT" "WorkingDirectory=$R/backend"

# Instalação nova pelo pacote: sem npm, sem --backend-only, sem pergunta; o backend serve a tela.
PACOTE_REPO=1 SEM_OPCOES=1 sandbox pacote 1 </dev/null
expect_line "pacote: unit chama o lançador" "$UNIT" "ExecStart=\"$H/.hangar/bin/hangar-launcher\" launch backend"
expect_no "pacote: sem unit do front" "$H/.config/systemd/user/hangar-frontend.service" '.'
expect_no "pacote: não pede npm" "$S/out" 'npm nao encontrado|node_modules'
[ -e "$H/.config/systemd/user/hangar-frontend.service" ] && flunk "pacote: unit do front criada" || pass "pacote: unit do front não criada"

# Pacote sem o lançador (install-package não rodou): recusa, em vez de gravar uma unit com uv.
PACOTE_REPO=1 SEM_OPCOES=1 ESPERA_FALHA=1 sandbox pacote-sem-lancador 0
[ "$RC" != 0 ] && pass "pacote-sem-lancador: falha" || flunk "pacote-sem-lancador: falha"
expect_no "pacote-sem-lancador: nenhuma unit" "$UNIT" '.'
grep -q 'install-package' "$S/out" && pass "pacote-sem-lancador: diz o que rodar" || flunk "pacote-sem-lancador: diz o que rodar"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
