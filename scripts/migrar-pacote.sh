#!/usr/bin/env bash
# Passo 2026-10-24-pacote-por-versao: baixa o hangar-server da release para ~/.hangar/migrate e roda
# a migração por ele. O binário que o motor `git` tem em ~/.hangar/bin é o da versão anterior (o
# Atualizar só o troca depois dos passos), sem o `migrate-package`.
#
# Já migrada (marca em ~/.hangar/migration.json): sai 0 sem fazer nada. Sem binário do Rust para
# este sistema: grava a marca `not_migrated` e sai 0, como o `NoPlatform` da própria migração.
set -uo pipefail
cd "$(dirname "$0")/.."

marca="$HOME/.hangar/migration.json"
if [ -e "$marca" ]; then echo "já migrada: $marca"; exit 0; fi
bin="$HOME/.hangar/migrate/hangar-server"

# 2 = a release não tem build para esta máquina; os avisos do download vão para a saída do passo.
( cd backend && uv run --no-sync python -c "from pathlib import Path; from app import rust_release as r
a = r.fetch(dest=Path.home() / '.hangar' / 'migrate')
print(*(a or []), sep='\n')
raise SystemExit(2 if a is None else 0)" )
rc=$?

if [ "$rc" = 2 ]; then
    json() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
    mkdir -p "$HOME/.hangar"
    printf '{"mode":"not_migrated","date":"%s","checkout":"%s","version":null,"reason":"%s","notes":[],"done":[]}\n' \
        "$(date +%Y-%m-%dT%H:%M:%S%z)" "$(json "$PWD")" \
        "o Hangar não publica o binário do Rust para este sistema; a máquina segue no motor git" > "$marca.tmp" \
        && mv "$marca.tmp" "$marca"
    echo "não migrada: sem binário do Rust para este sistema"
    exit 0
fi
if [ "$rc" != 0 ] || [ ! -x "$bin" ]; then
    echo "##HANGAR-FALHA## o binário da migração não foi baixado para $bin (download saiu com $rc)"
    exit 1
fi
# `exec`: o prazo do passo, se estourar, atinge a própria migração e não só este shell.
exec "$bin" migrate-package
