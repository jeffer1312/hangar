#!/bin/sh
# headersHelper do MCP `hangar` no Claude Code: roda a cada conexão, herda o ambiente do `claude`
# e imprime os cabeçalhos em JSON (`hangar-server mcp-headers`). O token sai do `.env`, nunca do
# ambiente da sessão: `~/.hangar/config/.env` quando existe, senão o da `backend/` desta cópia.
set -u
self=$0
while [ -L "$self" ]; do
    dir=$(CDPATH= cd -- "$(dirname -- "$self")" && pwd)
    link=$(readlink -- "$self")
    case $link in /*) self=$link ;; *) self=$dir/$link ;; esac
done
root=$(CDPATH= cd -P -- "$(dirname -- "$self")/.." && pwd -P)
. "$root/scripts/hangar-server-lookup"
if ! hangar_server_find "$root"; then
    # Sem cabeçalho o servidor dá 401 e o Claude cai no OAuth: falha alto.
    echo 'hangar-mcp-headers: hangar-server não encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN' >&2
    exit 1
fi
exec "$HANGAR_SERVER_BIN" mcp-headers --root "$root"
