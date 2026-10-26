#!/usr/bin/env bash
# Mudança que a pessoa vê no app (nativo ou Expo) costuma merecer uma novidade em docs/novidades/,
# que a tela de atualização mostra. Este check só AVISA: nunca bloqueia, porque correção pequena
# e ajuste interno também tocam essas pastas.
#
# Uso:  check-novidade.sh <de>..<para>
# Saída 0 sempre (com ou sem aviso); 2 = intervalo inválido.
# Escape: `HANGAR_SEM_NOVIDADE: <motivo>` numa mensagem de commit do intervalo.
set -uo pipefail

case "${1:-}" in
    *..*) git rev-list "$1" >/dev/null 2>&1 || { echo "intervalo inválido: $1" >&2; exit 2; } ;;
    *) echo "uso: $0 <de>..<para>" >&2; exit 2 ;;
esac
intervalo="$1"

git log --no-merges --format=%B "$intervalo" | grep -qE '(^|[^A-Za-z_])HANGAR_SEM_NOVIDADE: *[^ ]' && exit 0

# Merges ficam de fora: trazem o que já foi conferido na branch de origem.
arquivos="$(git log --no-merges --name-only --format= "$intervalo" | sort -u)"

visiveis="$(printf '%s\n' "$arquivos" \
    | grep -E '^(desktop-native/src/|mobile/src/|mobile/app/|messages/[^/]+\.json$)' \
    | grep -Ev '(\.test\.|(^|/)__tests__/|(^|/)tests/)' || true)"
[[ -z "$visiveis" ]] && exit 0

novidades="$(git diff --name-only --diff-filter=A "$intervalo" -- 'docs/novidades/*.md' | grep -v 'README\.md$' || true)"
[[ -n "$novidades" ]] && exit 0

lista="$(printf '%s\n' "$visiveis" | head -10 | sed 's/^/  /')"
msg="Mudança visível no app sem novidade em docs/novidades/. Rode a skill hangar-novidade para escrevê-la, ou, se a pessoa não vê nada de novo, ponha 'HANGAR_SEM_NOVIDADE: <motivo>' na mensagem de um commit."
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    echo "::warning title=Falta a novidade::$msg"
    printf '%s\n' "$lista"
else
    printf 'aviso: %s\n%s\n' "$msg" "$lista" >&2
fi
exit 0
