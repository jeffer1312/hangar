#!/usr/bin/env bash
# Mudança que a pessoa vê no app (nativo ou Expo) ou commit `feat` em qualquer pasta pede uma novidade em
# docs/novidades/, que a tela de atualização mostra. Sem ela, o portão (gate.yml) falha.
#
# Uso:  check-novidade.sh <de>..<para>
# Saída 0 = nada a cobrar; 1 = falta a novidade; 2 = intervalo inválido.
# Escape: `HANGAR_SEM_NOVIDADE: <motivo>` na mensagem do commit; vale só para aquele commit.
set -uo pipefail

case "${1:-}" in
    *..*) git rev-list "$1" >/dev/null 2>&1 || { echo "intervalo inválido: $1" >&2; exit 2; } ;;
    *) echo "uso: $0 <de>..<para>" >&2; exit 2 ;;
esac
intervalo="$1"

novidades="$(git diff --name-only --diff-filter=A "$intervalo" -- 'docs/novidades/*.md' | grep -v 'README\.md$' || true)"
[[ -n "$novidades" ]] && exit 0

# Merges ficam de fora: trazem o que já foi conferido na branch de origem.
visiveis=""
features=""
for commit in $(git rev-list --no-merges "$intervalo"); do
    body="$(git log -1 --format=%B "$commit")"
    grep -qE '(^|[^A-Za-z_])HANGAR_SEM_NOVIDADE: *[^ ]' <<<"$body" && continue
    files="$(git diff-tree --no-commit-id --name-only -r "$commit" \
        | grep -E '^(desktop-native/src/|mobile/src/|mobile/app/|messages/[^/]+\.json$)' \
        | grep -Ev '(\.test\.|(^|/)__tests__/|(^|/)tests/)' || true)"
    [[ -n "$files" ]] && visiveis+="$files"$'\n'
    # Recurso novo no servidor também chega à pessoa, mesmo sem tocar nas pastas dos apps.
    subject="$(head -n1 <<<"$body")"
    grep -qE '^feat(\([^)]*\))?!?:' <<<"$subject" && features+="$subject"$'\n'
done
[[ -z "$visiveis" && -z "$features" ]] && exit 0

lista="$(printf '%s%s' "$features" "$(printf '%s' "$visiveis" | sort -u)" | grep -v '^$' | head -10 | sed 's/^/  /')"
msg="Mudança visível ou recurso novo sem novidade em docs/novidades/. Rode a skill hangar-novidade para escrevê-la, ou, se a pessoa não vê nada de novo, ponha 'HANGAR_SEM_NOVIDADE: <motivo>' na mensagem desse commit."
if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    echo "::error title=Falta a novidade::$msg"
    printf '%s\n' "$lista"
else
    printf 'falta: %s\n%s\n' "$msg" "$lista" >&2
fi
exit 1
