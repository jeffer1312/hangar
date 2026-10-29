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
# A prova do passo: ele rodou, migrando agora ou deixando a migração para a tela.
if ! { mkdir -p "$HOME/.hangar" && date +%Y-%m-%dT%H:%M:%S%z > "$HOME/.hangar/migration-offered"; } 2>/dev/null; then
    echo "##HANGAR-FALHA## não consegui gravar $HOME/.hangar/migration-offered"
    exit 1
fi
if [ -e "$marca" ]; then echo "já migrada: $marca"; exit 0; fi
# Sem o aviso aceito na tela (o motor ou o app antigo atualizando), nada muda: a versão nova mostra o
# aviso com a escolha, e a migração roda pela tela.
# Mesma leitura do servidor (`update::accepted`): só uma lista de textos vale; outro formato é nada aceito.
# O Node já é pré-requisito do instalador; a leitura conserva a validação de lista de textos.
if ! command -v node >/dev/null 2>&1; then
    echo "##HANGAR-FALHA## sem Node para ler o aceite em $HOME/.hangar/update-accepted.json"
    exit 1
fi
if ! node -e 'const fs=require("fs");try{const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));process.exit(Array.isArray(d)&&d.every(v=>typeof v==="string")&&d.includes("2026-10-24-pacote-por-versao")?0:1)}catch{process.exit(1)}' "$HOME/.hangar/update-accepted.json" 2>/dev/null; then
    echo "migração pendente: espera o aviso ser aceito na tela de atualização"
    exit 0
fi
pasta="$HOME/.hangar/migrate"
bin="$pasta/hangar-server"

# O binário de uma rodada anterior não pode rodar no lugar de um download que falhou.
if ! rm -rf "$pasta"; then
    echo "##HANGAR-FALHA## não consegui tirar o binário anterior de $pasta"
    exit 1
fi

# 2 = a release não tem build para esta máquina; os avisos do download vão para a saída do passo.
avisos="$( HANGAR_UPDATE_OWNS_RUST= HANGAR_SERVER_BIN_DIR="$pasta" bash scripts/install-server-binaries.sh --fetch 2>&1 )"
rc=$?
[ -n "$avisos" ] && printf '%s\n' "$avisos"

if [ "$rc" = 2 ]; then
    json() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
    if ! { mkdir -p "$HOME/.hangar" \
        && printf '{"mode":"not_migrated","date":"%s","checkout":"%s","version":null,"reason":"%s","notes":[],"done":[]}\n' \
            "$(date +%Y-%m-%dT%H:%M:%S%z)" "$(json "$PWD")" \
            "o Hangar não publica o binário do Rust para este sistema; a máquina segue no motor git" > "$marca.tmp" \
        && mv "$marca.tmp" "$marca"; } 2>/dev/null; then
        rm -f "$marca.tmp" 2>/dev/null
        echo "##HANGAR-FALHA## sem binário do Rust para este sistema, e não consegui gravar a marca $marca"
        exit 1
    fi
    echo "não migrada: sem binário do Rust para este sistema"
    exit 0
fi
if [ "$rc" != 0 ] || [ ! -x "$bin" ]; then
    echo "##HANGAR-FALHA## o binário da migração não foi baixado para $bin (download saiu com $rc)${avisos:+: $(printf '%s' "$avisos" | tr '\n' ' ')}"
    exit 1
fi
# `exec`: o prazo do passo, se estourar, atinge a própria migração e não só este shell.
exec "$bin" migrate-package
