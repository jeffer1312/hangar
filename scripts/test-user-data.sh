#!/usr/bin/env bash
# Trava onde os scripts leem os dados do usuário: `~/.hangar/config/<arquivo>` quando existe, senão o
# da `backend/` da cópia (máquina ainda não migrada), arquivo a arquivo. Cobre hangar-mcp-headers.sh,
# hangar-send (ENV_FILE e PEERS_FILE) e o vigia.sh da orquestrar. HOME e cópia em pasta temporária,
# só com valores falsos.
#
# Uso: ./scripts/test-user-data.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
falhas=0
INST="$TMP/inst"
CONFIG="$TMP/home/.hangar/config"

mkdir -p "$INST/scripts" "$INST/backend" "$TMP/home"
cp "$REPO/scripts/hangar-mcp-headers.sh" "$REPO/scripts/hangar-send" "$INST/scripts/"
# Executável e PATH sem as pastas do usuário: o `command -v hangar-send` do vigia nunca acha o
# instalado na máquina (que leria o `.env` real).
chmod +x "$INST/scripts/hangar-send"
# O trecho que resolve os arquivos sai dos scripts de verdade: uma cópia colada aqui passaria com o
# arquivo real quebrado.
sed -n '/^\(BACKEND_DIR\|ENV_FILE\)=/,/^SERVER_ID=/p' "$REPO/scripts/hangar-send" > "$TMP/send.sh"
sed -n '/^ENVFILE=/,/^T=/p' "$REPO/skills/orquestrar/scripts/vigia.sh" > "$TMP/vigia.sh"
[[ -s "$TMP/send.sh" && -s "$TMP/vigia.sh" ]] || { echo "FALHA: trecho de resolução não encontrado"; exit 1; }

headers() { HOME="$TMP/home" bash "$INST/scripts/hangar-mcp-headers.sh" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["Authorization"])' 2>/dev/null; }
# `$0` do trecho é a cópia do hangar-send, como quando ele roda de verdade.
send() { HOME="$TMP/home" bash -c "$(cat "$TMP/send.sh")"$'\nprintf "%s|%s\\n" "$TOKEN" "$(cat "$PEERS_FILE" 2>/dev/null)"' "$INST/scripts/hangar-send" 2>/dev/null; }
vigia() { HOME="$TMP/home" PATH="$INST/scripts:/usr/bin:/bin" bash -c "$(cat "$TMP/vigia.sh")"$'\nprintf "%s\\n" "$T"' vigia 2>/dev/null; }

checa() { # <caso> <esperado> <obtido> — o obtido nunca é impresso: pode ser um token de verdade.
    if [[ "$2" == "$3" ]]; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FALHA %s -> esperava %q, veio outro valor\n' "$1" "$2"; falhas=$((falhas + 1))
    fi
}

printf 'CP_AUTH_TOKEN=da-backend\n' > "$INST/backend/.env"
printf 'peers-da-backend\n' > "$INST/backend/peers.json"
checa "sem o lugar novo: headers" "Bearer da-backend" "$(headers)"
checa "sem o lugar novo: hangar-send" "da-backend|peers-da-backend" "$(send)"
checa "sem o lugar novo: vigia" "da-backend" "$(vigia)"
[[ ! -e "$TMP/home/.hangar" ]] || { echo "FALHA a leitura criou ~/.hangar"; falhas=$((falhas + 1)); }

mkdir -p "$CONFIG"
printf 'CP_AUTH_TOKEN=do-config\n' > "$CONFIG/.env"
# Arquivo a arquivo: o `.env` migrou, o peers.json ainda não.
checa "só o .env migrou: hangar-send" "do-config|peers-da-backend" "$(send)"
printf 'peers-do-config\n' > "$CONFIG/peers.json"
checa "nos dois lugares: headers" "Bearer do-config" "$(headers)"
checa "nos dois lugares: hangar-send" "do-config|peers-do-config" "$(send)"
checa "nos dois lugares: vigia" "do-config" "$(vigia)"
printf 'CP_AUTH_TOKEN=do-cp-env\n' > "$TMP/cp-env"
checa "vigia: CP_ENV vence" "do-cp-env" "$(CP_ENV="$TMP/cp-env" vigia)"

rm "$INST/backend/.env" "$INST/backend/peers.json"
checa "só no lugar novo: headers" "Bearer do-config" "$(headers)"
checa "só no lugar novo: hangar-send" "do-config|peers-do-config" "$(send)"
checa "só no lugar novo: vigia" "do-config" "$(vigia)"

exit "$falhas"
