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

# run <caso> <rc do download> <cria o binário 0|1> [marca já existe] [ro|velho]: roda o passo no sandbox.
# `ro`: ~/.hangar sem escrita; `velho`: um binário de uma rodada anterior já em ~/.hangar/migrate.
run() {
  S="$ROOT/$1"; R="$S/repo"; H="$S/home"; B="$S/bin"
  mkdir -p "$R/scripts" "$R/backend" "$H" "$B"
  cp "$REPO/scripts/migrar-pacote.sh" "$R/scripts/"
  [ "${4:-0}" = 1 ] && mkdir -p "$H/.hangar" && echo '{}' > "$H/.hangar/migration.json"
  # 6º argumento 0: o aviso não foi aceito na tela (o padrão é aceito).
  [ "${6:-1}" = 1 ] && mkdir -p "$H/.hangar" && echo '["2026-10-24-pacote-por-versao"]' > "$H/.hangar/update-accepted.json"
  if [ "${5:-}" = velho ]; then
    mkdir -p "$H/.hangar/migrate"
    printf '#!/bin/sh\necho "velho $*" >> "%s/calls"\n' "$S" > "$H/.hangar/migrate/hangar-server"
    chmod +x "$H/.hangar/migrate/hangar-server"
  fi
  [ "${5:-}" = ro ] && mkdir -p "$H/.hangar" && chmod 555 "$H/.hangar"
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
  [ -d "$H/.hangar" ] && chmod 755 "$H/.hangar"
}

run marcada 0 0 1
expect_eq "marcada: sai 0" "$RC" 0 "$S/out"
expect_eq "marcada: não baixa nada" "$(cat "$S/calls" 2>/dev/null)" ""
expect_eq "marcada: a prova do passo existe" "$([ -s "$H/.hangar/migration-offered" ] && echo sim || echo nao)" sim

# Motor ou app antigo atualizando, sem o aviso aceito: só a marca de pendente, nada baixado nem migrado.
run sem-aceite 0 1 0 "" 0
expect_eq "sem-aceite: sai 0" "$RC" 0 "$S/out"
expect_eq "sem-aceite: não baixa nem migra" "$(cat "$S/calls" 2>/dev/null)" ""
expect_eq "sem-aceite: marca a migração pendente" "$([ -s "$H/.hangar/migration-offered" ] && echo sim || echo nao)" sim
expect_eq "sem-aceite: sem marca de migrada" "$([ -e "$H/.hangar/migration.json" ] && echo sim || echo nao)" nao

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

run marca-sem-escrita 2 0 0 ro
expect_eq "marca-sem-escrita: falha" "$RC" 1 "$S/out"
expect_eq "marca-sem-escrita: motivo marcado" "$(grep -c '##HANGAR-FALHA##' "$S/out")" 1 "$S/out"

# O download não trouxe binário novo (aviso de manifesto ou sha): o da rodada anterior não roda.
run binario-velho 0 0 0 velho
expect_eq "binario-velho: falha" "$RC" 1 "$S/out"
expect_eq "binario-velho: o velho não roda" "$(grep -c '^velho' "$S/calls" 2>/dev/null || true)" 0 "$S/out"
expect_eq "binario-velho: motivo marcado" "$(grep -c '##HANGAR-FALHA##' "$S/out")" 1 "$S/out"

run baixou 0 1
expect_eq "baixou: sai com o código da migração" "$RC" 7 "$S/out"
expect_eq "baixou: migra da raiz do checkout" "$(grep '^migrate' "$S/calls")" "migrate migrate-package em $R"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
