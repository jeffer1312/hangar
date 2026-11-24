#!/usr/bin/env bash
# A prova `--ativo` do LaunchAgent do macOS bate na porta do mesmo `.env` que o servidor lê:
# ~/.hangar/config/.env numa máquina migrada, backend/.env na que não migrou. launchctl e curl
# falsos; roda em qualquer sistema e nunca toca o launchd.
#
#   ./scripts/test-services-launchd.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; fail=1; }

# caso <nome> <migrada 0|1> <porta esperada>
caso() {
  local nome=$1 migrada=$2 esperada=$3
  local S="$ROOT/$nome" R="$ROOT/$nome/repo" B="$ROOT/$nome/bin" H="$ROOT/$nome/home"
  mkdir -p "$R/scripts" "$R/backend" "$B" "$H"
  cp "$REPO/scripts/services-launchd.sh" "$R/scripts/"
  printf 'CP_PORT=7001\n' > "$R/backend/.env"
  if [ "$migrada" = 1 ]; then mkdir -p "$H/.hangar/config"; printf 'CP_PORT=7002\n' > "$H/.hangar/config/.env"; fi
  printf '#!/bin/sh\n[ "$1" = print ] && echo "pid = 42"\nexit 0\n' > "$B/launchctl"
  printf '#!/bin/sh\nfor a; do last=$a; done\necho "$last" >> "%s/curl"\nexit 0\n' "$S" > "$B/curl"
  chmod +x "$B/launchctl" "$B/curl"
  ( cd "$R" && HOME="$H" PATH="$B:$PATH" bash scripts/services-launchd.sh --ativo >"$S/out" 2>&1 ) \
    || { flunk "$nome: --ativo falhou"; sed 's/^/     | /' "$S/out"; return; }
  if grep -Fqx "http://127.0.0.1:$esperada/" "$S/curl"; then pass "$nome: porta $esperada"
  else flunk "$nome: esperava a porta $esperada"; sed 's/^/     | /' "$S/curl"; fi
}

caso nao-migrada 0 7001
caso migrada 1 7002

exit $fail
