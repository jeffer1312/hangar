#!/bin/sh
# O MCP `hangar` por stdio, para o `agy` (que só manda cabeçalho fixo): `hangar-server mcp-stdio` repassa ao
# `/mcp/` local com a identidade do ambiente da sessão. O token sai do `.env`, nunca do ambiente da sessão.
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
    echo 'hangar-mcp-stdio: hangar-server não encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN' >&2
    exit 1
fi
exec "$HANGAR_SERVER_BIN" mcp-stdio --root "$root"
