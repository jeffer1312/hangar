#!/usr/bin/env bash
# Os portões do CI (.github/workflows/ci.yml), na mesma ordem, pra rodar ANTES do push.
#
# Uso:  scripts/ci-gates.sh              # roda tudo
#       scripts/ci-gates.sh <de> <ate>   # só o que o intervalo de commits tocou
#
# Com intervalo, backend só roda se `backend/` mudou e front só se `frontend/` mudou — um push só de
# docs não paga 4 minutos de teste. O pre-push chama com o intervalo que vai sair.
set -uo pipefail

RAIZ="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
cd "$RAIZ" || exit 1

roda_back=1; roda_front=1
if (( $# == 2 )); then
    # --no-renames: um arquivo movido pra fora de backend/ ou frontend/ tem que contar como mudança lá.
    mudados="$(git diff --name-only --no-renames "$1" "$2" -- . 2>/dev/null)" || { echo "ci-gates: intervalo $1..$2 ilegível" >&2; exit 1; }
    grep -q '^backend/' <<< "$mudados" || roda_back=0
    grep -q '^frontend/' <<< "$mudados" || roda_front=0
    grep -q '^\.github/workflows/' <<< "$mudados" && { roda_back=1; roda_front=1; }
fi

falhou=0
portao() {
    local nome="$1"; shift
    printf '\n\033[36m── %s\033[0m\n' "$nome"
    if "$@"; then printf '\033[32m   ok %s\033[0m\n' "$nome"; else printf '\033[31m   FALHOU %s\033[0m\n' "$nome"; falhou=1; fi
}

if (( roda_back )); then
    # O CI instala ripgrep de propósito: sem `rg`, a busca entre sessões volta zero e os testes falham só lá.
    command -v rg >/dev/null || { echo "ci-gates: ripgrep (rg) não está no PATH — o CI tem, e os testes de busca dependem dele" >&2; falhou=1; }
    portao "backend: pytest" bash -c 'cd backend && uv run pytest -q'
else
    echo "backend: nada em backend/ mudou, pulando"
fi

if (( roda_front )); then
    portao "frontend: check (svelte-check + tsc)" npm --prefix frontend run check
    portao "frontend: test (vitest)" npm --prefix frontend test
    portao "frontend: build" npm --prefix frontend run build
else
    echo "frontend: nada em frontend/ mudou, pulando"
fi

if (( falhou )); then
    printf '\n\033[31mci-gates: algum portão falhou — o CI vai falhar igual e o dist-latest não atualiza.\033[0m\n' >&2
    exit 1
fi
printf '\n\033[32mci-gates: tudo verde.\033[0m\n'
