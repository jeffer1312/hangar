#!/usr/bin/env bash
# Trava onde os scripts leem os dados do usuário: `~/.hangar/config/<arquivo>` quando existe, senão o
# da `backend/` da cópia (máquina ainda não migrada), arquivo a arquivo. Cobre o atalho
# hangar-mcp-headers.sh (pelo `hangar-server mcp-headers` compilado desta árvore) e o vigia.sh da
# orquestrar. O `hangar-send` tem os mesmos casos nos testes do crate `hangar-send` (`.env`) e do
# `send_cli` do servidor (`peers.json`). HOME e cópia em pasta temporária, só com valores falsos.
#
# Uso: ./scripts/test-user-data.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
falhas=0
INST="$TMP/inst"
CONFIG="$TMP/home/.hangar/config"
export CP_RUST_SERVER_BIN="$REPO/crates/target/debug/hangar-server"
[[ -x "$CP_RUST_SERVER_BIN" ]] || { echo "compile antes: scripts/cargo-pinned build -p hangar-server"; exit 1; }

mkdir -p "$INST/scripts" "$INST/backend" "$TMP/home"
cp "$REPO/scripts/hangar-mcp-headers.sh" "$REPO/scripts/hangar-send" "$REPO/scripts/hangar-server-lookup" "$INST/scripts/"
# Executável e PATH sem as pastas do usuário: o `command -v hangar-send` do vigia nunca acha o
# instalado na máquina (que leria o `.env` real).
chmod +x "$INST/scripts/hangar-send"
# O trecho que resolve os arquivos sai do script de verdade: uma cópia colada aqui passaria com o
# arquivo real quebrado.
sed -n '/^ENVFILE=/,/^T=/p' "$REPO/skills/orquestrar/scripts/vigia.sh" > "$TMP/vigia.sh"
[[ -s "$TMP/vigia.sh" ]] || { echo "FALHA: trecho de resolução não encontrado"; exit 1; }

headers() { HOME="$TMP/home" sh "$INST/scripts/hangar-mcp-headers.sh" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["Authorization"])' 2>/dev/null; }
vigia() { HOME="$TMP/home" PATH="$INST/scripts:/usr/bin:/bin" bash -c "$(cat "$TMP/vigia.sh")"$'\nprintf "%s\\n" "$T"' vigia 2>/dev/null; }

checa() { # <caso> <esperado> <obtido> — o obtido nunca é impresso: pode ser um token de verdade.
    if [[ "$2" == "$3" ]]; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FALHA %s -> esperava %q, veio outro valor\n' "$1" "$2"; falhas=$((falhas + 1))
    fi
}

printf 'CP_AUTH_TOKEN=da-backend\n' > "$INST/backend/.env"
checa "sem o lugar novo: headers" "Bearer da-backend" "$(headers)"
checa "sem o lugar novo: vigia" "da-backend" "$(vigia)"
[[ ! -e "$TMP/home/.hangar" ]] || { echo "FALHA a leitura criou ~/.hangar"; falhas=$((falhas + 1)); }

mkdir -p "$CONFIG"
printf 'CP_AUTH_TOKEN=do-config\n' > "$CONFIG/.env"
checa "nos dois lugares: headers" "Bearer do-config" "$(headers)"
checa "nos dois lugares: vigia" "do-config" "$(vigia)"
printf 'CP_AUTH_TOKEN=do-cp-env\n' > "$TMP/cp-env"
checa "vigia: CP_ENV vence" "do-cp-env" "$(CP_ENV="$TMP/cp-env" vigia)"

rm "$INST/backend/.env"
checa "só no lugar novo: headers" "Bearer do-config" "$(headers)"
checa "só no lugar novo: vigia" "do-config" "$(vigia)"

exit "$falhas"
