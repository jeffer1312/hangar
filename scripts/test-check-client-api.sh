#!/usr/bin/env bash
# Confere o scripts/check-client-api.sh num repositório descartável: quando a trava dispara, quando
# o CLIENT_API subiu o bastante e o escape.
# Usage: bash scripts/test-check-client-api.sh
set -uo pipefail

RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
CHECK="$RAIZ/scripts/check-client-api.sh"
falhou=0

# Nada da máquina entra: nem config global (hooksPath, assinatura), nem a do sistema.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset HANGAR_SEM_CLIENT_API HANGAR_SEM_PASSO

d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
cd "$d" || exit 1
git init -q -b main .

api() {   # $1 = valor da constante; $2 = linha extra (opcional)
    mkdir -p crates/hangar-api/src
    printf '//! api\npub const CLIENT_API: u32 = %s;\n%s\n' "$1" "${2:-}" > crates/hangar-api/src/lib.rs
}
commit() { git add -A && git commit -q --no-verify -m "$1"; }

confere() {   # $1 = rótulo, $2 = saída esperada, resto = argumentos do check
    local rotulo="$1" esperado="$2" obtido
    shift 2
    bash "$CHECK" "$@" >/dev/null 2>&1; obtido=$?
    if [[ "$obtido" != "$esperado" ]]; then
        printf 'FALHOU %s: saiu %s, esperado %s\n' "$rotulo" "$obtido" "$esperado"
        falhou=1
    else
        printf 'ok %s\n' "$rotulo"
    fi
}

api 2
mkdir -p desktop-native/src/api
echo 'fn a() {}' > desktop-native/src/update.rs
echo 'fn b() {}' > desktop-native/src/api/mod.rs
echo 'x' > README.md
commit base
base="$(git rev-parse HEAD)"

# ── intervalo (CI) ────────────────────────────────────────────────────────────────────────────
git checkout -q -b sem-subir "$base"
api 2 'pub fn nova() {}'; commit 'hangar-api sem subir'
confere "hangar-api sem subir → 1" 1 "$base..HEAD"

git checkout -q -b subindo "$base"
api 3 'pub fn nova() {}'; commit 'hangar-api subindo'
confere "hangar-api subindo para mais que a base → 0" 0 "$base..HEAD"

git checkout -q -b nativo "$base"
echo 'const R: &str = "/api/x";' >> desktop-native/src/update.rs; commit 'nativo chama rota nova'
confere "nativo acrescenta \"/api/ sem subir → 1" 1 "$base..HEAD"

git checkout -q -b nativo-mod "$base"
echo 'fn c() {}' >> desktop-native/src/api/mod.rs; commit 'mexe no módulo de api do nativo'
confere "desktop-native/src/api sem subir → 1" 1 "$base..HEAD"

git checkout -q -b nativo-outro "$base"
echo 'fn c() {}' >> desktop-native/src/update.rs; echo y >> README.md; commit 'nada de api'
confere "mudança fora da trava → 0" 0 "$base..HEAD"

git checkout -q -b escape "$base"
echo 'const R: &str = "/api/x";' >> desktop-native/src/update.rs
commit $'nativo usa rota que já existe\n\nHANGAR_SEM_CLIENT_API: a rota já existe desde a api 1'
confere "escape na mensagem do commit → 0" 0 "$base..HEAD"

# A linha vale para o PR inteiro: num commit posterior, cobre o anterior que mexeu na api.
git checkout -q -b escape-depois "$base"
echo 'fn c() {}' >> desktop-native/src/api/mod.rs; commit 'mexe no módulo de api do nativo'
echo y >> README.md
commit $'ajuste\n\nHANGAR_SEM_CLIENT_API: nada novo é pedido ao servidor'
confere "escape num commit posterior do intervalo → 0" 0 "$base..HEAD"

# PR paralelo: a base já subiu para 3 (outro PR mesclado) e esta branch também parou em 3.
git checkout -q -b base-nova "$base"
api 3; commit 'outro PR subiu'
base3="$(git rev-parse HEAD)"
git checkout -q -b paralelo "$base3"
api 3 'pub fn outra() {}'; commit 'PR paralelo em 3'
confere "PR com o mesmo número da base → 1" 1 "$base3..HEAD"

git checkout -q -b ilegivel "$base"
printf 'pub fn sem_constante() {}\n' > crates/hangar-api/src/lib.rs; commit 'constante sumiu'
confere "constante ilegível na ponta → 2" 2 "$base..HEAD"

# Base sem a constante conta como api 1.
git checkout -q --orphan antiga
git rm -rq --cached . && rm -rf crates desktop-native README.md
mkdir -p crates/hangar-api/src && echo '//! sem contrato' > crates/hangar-api/src/lib.rs; commit antiga
antiga="$(git rev-parse HEAD)"
api 2; commit 'primeiro contrato'
confere "base sem a constante vale 1 → 0" 0 "$antiga..HEAD"

# ── stage (pre-commit): compara com a origin/main ─────────────────────────────────────────────
git checkout -q -f main
git update-ref refs/remotes/origin/main "$base"
api 2 'pub fn nova() {}'; git add -A
confere "stage sem subir → 1" 1 --staged
HANGAR_SEM_CLIENT_API=1 confere "stage com HANGAR_SEM_CLIENT_API=1 → 0" 0 --staged
api 3 'pub fn nova() {}'; git add -A
confere "stage subindo → 0" 0 --staged
git commit -q --no-verify -m 'subiu'
api 3 'pub fn mais() {}'; git add -A
confere "segundo commit local acima da origin/main → 0" 0 --staged
# Merge da main em andamento: o stage traz a mudança já conferida lá, no mesmo número da origin/main.
git update-ref refs/remotes/origin/main HEAD
api 3 'pub fn da_main() {}'; git add -A
confere "stage no mesmo número da origin/main → 1" 1 --staged
git rev-parse HEAD > "$(git rev-parse --git-path MERGE_HEAD)"
confere "commit de merge → 0" 0 --staged
rm -f "$(git rev-parse --git-path MERGE_HEAD)"
git reset -q --hard

# ── o pre-commit chama a trava ────────────────────────────────────────────────────────────────
mkdir -p scripts/hooks
cp "$RAIZ/scripts/hooks/pre-commit" scripts/hooks/
cp "$RAIZ/scripts/check-client-api.sh" "$RAIZ/scripts/check-passo-de-atualizacao.sh" scripts/ 2>/dev/null
git add -A && HANGAR_SEM_PASSO=1 git commit -q --no-verify -m 'hooks'
git config core.hooksPath scripts/hooks
git update-ref refs/remotes/origin/main HEAD
echo 'const R: &str = "/api/y";' >> desktop-native/src/update.rs; git add -A
if git commit -q -m 'pelo hook' >/dev/null 2>&1; then
    echo 'FALHOU pre-commit deixou passar "/api/ sem subir'; falhou=1
else
    echo 'ok pre-commit recusa "/api/ sem subir'
fi
if HANGAR_SEM_CLIENT_API=1 git commit -q -m 'pelo hook com escape' >/dev/null 2>&1; then
    echo 'ok pre-commit aceita HANGAR_SEM_CLIENT_API=1'
else
    echo 'FALHOU pre-commit recusou com HANGAR_SEM_CLIENT_API=1'; falhou=1
fi

(( falhou )) && { echo "test-check-client-api: FALHOU"; exit 1; }
echo "test-check-client-api: ok"
