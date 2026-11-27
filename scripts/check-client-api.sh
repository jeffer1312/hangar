#!/usr/bin/env bash
# Mudança na API que o app nativo consome exige subir o CLIENT_API (crates/hangar-api/src/lib.rs):
# é por ele que o app sabe se o servidor desta máquina já atende o que ele chama.
#
# Dispara quando o diff toca crates/hangar-api/src/** ou desktop-native/src/api/**, ou acrescenta em
# desktop-native/src/** uma linha com o literal "/api/ (o app passando a chamar rota). Disparada,
# exige o CLIENT_API MAIOR que o da base: dois PRs que sobem 2→3 em paralelo deixam o segundo
# vermelho até ele subir para 4.
#
# Uso:  check-client-api.sh --staged          (pre-commit: stage contra origin/main, ou HEAD)
#       check-client-api.sh <de>..<para>      (CI: no PR a base, no push o anterior)
# Saída 0 = ok; 1 = disparou sem subir; 2 = constante ilegível ou uso errado.
# Escape deliberado, para mudança que não muda o que o app exige do servidor:
#       HANGAR_SEM_CLIENT_API=1 git commit ...
# e `HANGAR_SEM_CLIENT_API: <por quê>` na mensagem de um commit do intervalo, que é o que o CI enxerga:
# vale para o PR inteiro, como o número, que também é conferido pela ponta.
set -uo pipefail

LIB=crates/hangar-api/src/lib.rs
GATILHO='^(crates/hangar-api/src/|desktop-native/src/api/)'

if [[ -n "${HANGAR_SEM_CLIENT_API:-}" ]]; then
    echo "CLIENT_API: pulado por HANGAR_SEM_CLIENT_API=1" >&2
    exit 0
fi

valor() {   # $1 = objeto git (rev:caminho ou :caminho); vazio quando não há a constante
    git show "$1" 2>/dev/null | sed -n 's/^pub const CLIENT_API: u32 = \([0-9][0-9]*\);.*/\1/p' | head -1
}

# $1 = lista de arquivos, $2 = diff de desktop-native/src; sai 0 quando a trava dispara.
disparou() {
    grep -Eq "$GATILHO" <<<"$1" && return 0
    grep '^+' <<<"$2" | grep -v '^+++' | grep -qF '"/api/'
}

case "${1:-}" in
    --staged)
        git rev-parse -q --verify HEAD >/dev/null || exit 0
        # Commit de merge traz o que já foi conferido do outro lado, como no modo intervalo.
        [[ -e "$(git rev-parse --git-path MERGE_HEAD)" ]] && exit 0
        disparou "$(git diff --cached --name-only)" \
                 "$(git diff --cached -U0 --no-color -- desktop-native/src)" || exit 0
        base_rev=HEAD
        git rev-parse -q --verify refs/remotes/origin/main >/dev/null && base_rev=origin/main
        base="$(valor "$base_rev:$LIB")"; atual="$(valor ":$LIB")"; onde="o stage" ;;
    *..*)
        git rev-list "$1" >/dev/null 2>&1 || { echo "intervalo inválido: $1" >&2; exit 2; }
        # Sem pipe: com pipefail, o `grep -q` sai cedo e o SIGPIPE do `git log` daria falha mesmo achando o escape.
        mensagens="$(git log --no-merges --format=%B "$1")"
        grep -qE '(^|[^A-Za-z_])HANGAR_SEM_CLIENT_API: +[^ ]' <<<"$mensagens" && exit 0
        algum=""
        # Merges ficam de fora: trazem o que já foi conferido.
        for c in $(git rev-list --no-merges "$1"); do
            if disparou "$(git diff-tree --no-commit-id --name-only -r "$c")" \
                        "$(git diff-tree --no-commit-id -p -U0 --no-color -r "$c" -- desktop-native/src)"; then
                algum=1; break
            fi
        done
        [[ -z "$algum" ]] && exit 0
        base_rev="${1%%..*}"
        base="$(valor "$base_rev:$LIB")"; atual="$(valor "${1##*..}:$LIB")"; onde="${1##*..}" ;;
    *) echo "uso: $0 --staged | <de>..<para>" >&2; exit 2 ;;
esac

if [[ -z "$atual" ]]; then
    echo "CLIENT_API ilegível em $LIB ($onde): esperado 'pub const CLIENT_API: u32 = N;'" >&2
    exit 2
fi
base="${base:-1}"   # base de antes do contrato fala a api 1
(( atual > base )) && exit 0

cat >&2 <<FIM
  ── falta subir o CLIENT_API ───────────────────────────────────────────────
  Esta mudança mexe na API que o app nativo consome (crates/hangar-api/src,
  desktop-native/src/api ou um "/api/ novo no desktop-native/src), mas o
  CLIENT_API em $LIB está em $atual e o de $base_rev em $base.
  Suba para $((base + 1)): é por ele que o app sabe que precisa de servidor novo.
  Mudança que não muda o que o app exige do servidor:
      HANGAR_SEM_CLIENT_API=1 git commit ...
  e, para o CI, "HANGAR_SEM_CLIENT_API: <por quê>" na mensagem de um commit do PR.
FIM
exit 1
