#!/usr/bin/env bash
# Confere o scripts/check-novidade.sh num repositório descartável: quando cobra a novidade, quando ela
# basta e o escape por commit.
# Usage: bash scripts/test-check-novidade.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK="$ROOT/scripts/check-novidade.sh"
failed=0

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset GITHUB_ACTIONS

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cd "$dir" || exit 1
git init -q -b main .

commit() { git add -A && git commit -q --no-verify -m "$1"; }
news() { mkdir -p docs/novidades; printf -- '---\ntitulo: t\ntipo: correcao\nonde: todos\n---\n\nx\n' > "docs/novidades/2026-01-01-$1.md"; }

check_case() {   # $1 = rótulo, $2 = saída esperada
    local label="$1" expected="$2" got
    bash "$CHECK" "$base..HEAD" >/dev/null 2>&1; got=$?
    if [[ "$got" != "$expected" ]]; then
        printf 'FALHOU %s: saiu %s, esperado %s\n' "$label" "$got" "$expected"
        failed=1
    else
        printf 'ok %s\n' "$label"
    fi
}

mkdir -p desktop-native/src mobile/src crates/x/src docs/novidades messages
echo 'fn a() {}' > desktop-native/src/app.rs
echo 'export {}' > mobile/src/a.ts
echo 'fn b() {}' > crates/x/src/lib.rs
echo '{}' > messages/pt.json
echo 'x' > README.md
commit base
base="$(git rev-parse HEAD)"

git checkout -q -b screen "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs; commit 'fix(native): tela'
check_case "tela sem novidade → 1" 1

git checkout -q -b screen-news "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs; news a; commit 'fix(native): tela'
check_case "tela com novidade → 0" 0

git checkout -q -b escape "$base"
echo 'fn c() {}' >> desktop-native/src/app.rs; commit $'fix(native): tela\n\nHANGAR_SEM_NOVIDADE: só espaçamento interno'
check_case "tela com escape no próprio commit → 0" 0

git checkout -q -b escape-elsewhere "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'feat(groups): resposta da filha chega ao pai'
echo 'y' >> README.md; commit $'test: fixture\n\nHANGAR_SEM_NOVIDADE: só teste'
check_case "escape noutro commit não libera o feat → 1" 1

git checkout -q -b server-feature "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'feat(groups): resposta da filha chega ao pai'
check_case "feat só no servidor sem novidade → 1" 1

git checkout -q -b breaking-feature "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'feat!: troca de formato'
check_case "feat! sem novidade → 1" 1

git checkout -q -b feature-news "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; news b; commit 'feat(groups): resposta da filha chega ao pai'
check_case "feat com novidade → 0" 0

git checkout -q -b server-fix "$base"
echo 'fn c() {}' >> crates/x/src/lib.rs; commit 'fix(groups): conserto'
check_case "fix só no servidor → 0" 0

git checkout -q -b messages "$base"
echo '{"a":"b"}' > messages/pt.json; commit 'fix: texto'
check_case "só messages/*.json → 1" 1

git checkout -q -b mobile-test "$base"
echo 'test()' > mobile/src/a.test.ts; commit 'test(mobile): caso'
check_case "só teste do Expo → 0" 0

git checkout -q -b docs "$base"
echo 'y' >> README.md; commit 'docs: readme'
check_case "só docs → 0" 0

(( failed )) && { echo "test-check-novidade: FALHOU"; exit 1; }
echo "test-check-novidade: ok"
