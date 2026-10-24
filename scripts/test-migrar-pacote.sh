#!/usr/bin/env bash
# Trava do passo 2026-10-24-pacote-por-versao (scripts/migrar-pacote.sh): sai na hora com a marca;
# sem binário do Rust para o sistema, grava a marca `not_migrated` e sai 0; binário não baixado é
# falha; com ele, a migração roda da raiz do checkout. `uv` e o binário são falsos; HOME temporário.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; [ -f "${2:-}" ] && sed 's/^/     | /' "$2"; fail=1; }
expect_eq() { if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 (veio '$2', esperado '$3')" "${4:-}"; fi; }

# run <caso> <rc do download> <cria o binário 0|1> [marca já existe]: roda o passo no sandbox.
run() {
  S="$ROOT/$1"; R="$S/repo"; H="$S/home"; B="$S/bin"
  mkdir -p "$R/scripts" "$R/backend" "$H" "$B"
  cp "$REPO/scripts/migrar-pacote.sh" "$R/scripts/"
  [ "${4:-0}" = 1 ] && mkdir -p "$H/.hangar" && echo '{}' > "$H/.hangar/migration.json"
  # O download falso: anota, cria (ou não) o binário e sai com o código pedido.
  cat > "$B/uv" <<EOF
#!/bin/sh
echo "uv \$*" >> "$S/calls"
if [ "$3" = 1 ]; then
  mkdir -p "\$HOME/.hangar/migrate"
  printf '#!/bin/sh\necho "migrate \$* em \$(pwd)" >> "$S/calls"\nexit 7\n' > "\$HOME/.hangar/migrate/hangar-server"
  chmod +x "\$HOME/.hangar/migrate/hangar-server"
fi
exit $2
EOF
  chmod +x "$B/uv"
  ( cd "$R" && env -i HOME="$H" PATH="$B:/usr/bin:/bin" bash scripts/migrar-pacote.sh >"$S/out" 2>&1 ); RC=$?
}

run marcada 0 0 1
expect_eq "marcada: sai 0" "$RC" 0 "$S/out"
expect_eq "marcada: não baixa nada" "$(cat "$S/calls" 2>/dev/null)" ""

run sem-build 2 0
expect_eq "sem-build: sai 0" "$RC" 0 "$S/out"
expect_eq "sem-build: marca not_migrated" \
  "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["mode"], bool(d["reason"]), d["checkout"])' "$H/.hangar/migration.json" 2>&1)" \
  "not_migrated True $R"

run sem-binario 0 0
expect_eq "sem-binario: falha" "$RC" 1 "$S/out"
expect_eq "sem-binario: motivo marcado" "$(grep -c '##HANGAR-FALHA##' "$S/out")" 1 "$S/out"
expect_eq "sem-binario: sem marca" "$([ -e "$H/.hangar/migration.json" ] && echo sim || echo nao)" nao

run download-falhou 1 1
expect_eq "download-falhou: falha" "$RC" 1 "$S/out"

run baixou 0 1
expect_eq "baixou: sai com o código da migração" "$RC" 7 "$S/out"
expect_eq "baixou: migra da raiz do checkout" "$(grep '^migrate' "$S/calls")" "migrate migrate-package em $R"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
