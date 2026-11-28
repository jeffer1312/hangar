#!/usr/bin/env bash
# Confere o scripts/check-novidade.sh num repositório descartável: quando cobra a novidade, quando ela
# basta e o escape.
# Usage: bash scripts/test-check-novidade.sh
set -uo pipefail

RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
CHECK="$RAIZ/scripts/check-novidade.sh"
falhou=0

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset GITHUB_ACTIONS

d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
cd "$d" || exit 1
git init -q -b main .

commit() { git add -A && git commit -q --no-verify -m "$1"; }

confere() {   # $1 = rótulo, $2 = saída esperada
    local rotulo="$1" esperado="$2" obtido
    bash "$CHECK" "$base..HEAD" >/dev/null 2>&1; obtido=$?
    if [[ "$obtido" != "$esperado" ]]; then
        printf 'FALHOU %s: saiu %s, esperado %s\n' "$rotulo" "$obtido" "$esperado"
        falhou=1
    else
        printf 'ok %s\n' "$rotulo"
    fi
}

mkdir -p desktop-native/src crates/x/src docs/novidades
echo 'fn a() {}' > desktop-native/src/app.rs
echo 'fn b() {}' > crates/x/src/lib.rs
echo 'x' > README.md
commit base
base="$(git rev-parse HEAD)"

git checkout -q -b tela "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs; commit 'fix(native): tela'
confere "tela sem novidade → 1" 1

git checkout -q -b tela-com "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs
printf -- '---\ntitulo: t\ntipo: correcao\nonde: desktop\n---\n\nx\n' > docs/novidades/2026-01-01-x.md
commit 'fix(native): tela'
confere "tela com novidade → 0" 0

git checkout -q -b escape "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs; commit $'fix(native): tela\n\nHANGAR_SEM_NOVIDADE: só espaçamento interno'
confere "tela com HANGAR_SEM_NOVIDADE → 0" 0

git checkout -q -b recurso-servidor "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'feat(groups): resposta da filha chega ao pai'
confere "feat só no servidor sem novidade → 1" 1

git checkout -q -b conserto-servidor "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'fix(groups): conserto'
confere "fix só no servidor → 0" 0

git checkout -q -b docs "$base"
echo 'y' >> README.md; commit 'docs: readme'
confere "só docs → 0" 0

(( falhou )) && { echo "test-check-novidade: FALHOU"; exit 1; }
echo "test-check-novidade: ok"
