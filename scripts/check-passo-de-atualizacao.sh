#!/usr/bin/env bash
# Mudança que a máquina de quem já usa não recebe por `git pull` precisa de um passo declarado em
# docs/atualizacoes/ — no MESMO commit. O botão Atualizar não roda mais o instalador inteiro; o
# que roda é o que o passo pedir. Sem o passo, wrapper/tarefa/statusline ficam velhos até alguém
# lembrar, e ninguém lembra.
#
# Uso:  check-passo-de-atualizacao.sh --staged          (pre-commit: o que está em stage)
#       check-passo-de-atualizacao.sh <de>..<para>       (CI: o intervalo do push)
# Saída 0 = ok (ou nada relevante mudou); 1 = mudou algo relevante sem passo novo.
# Escape deliberado, pra mudança que comprovadamente não muda nada na máquina (comentário, teste):
#       HANGAR_SEM_PASSO=1 git commit ...
# e `HANGAR_SEM_PASSO: <por quê>` na mensagem do commit, que é o que o CI enxerga; um commit
# posterior cobre um anterior com `HANGAR_SEM_PASSO=1 <sha>`.
set -uo pipefail

if [[ -n "${HANGAR_SEM_PASSO:-}" ]]; then
    echo "passo de atualização: pulado por HANGAR_SEM_PASSO=1" >&2
    exit 0
fi

# Arquivos dos commits do intervalo sem a marca; merges ficam de fora (trazem o que já foi conferido).
arquivos_sem_marca() {
    local intervalo="$1" filtro="${2:-ACMR}" cobertos c s pula
    cobertos="$(git log --format=%B "$intervalo" | grep -oE 'HANGAR_SEM_PASSO=1( [0-9a-f]{7,40})+' \
        | tr ' ' '\n' | grep -Ex '[0-9a-f]{7,40}' || true)"
    for c in $(git rev-list --no-merges "$intervalo"); do
        git log -1 --format=%B "$c" | grep -qE '(^|[^A-Za-z_])HANGAR_SEM_PASSO(: +[^ ]|=1 [0-9a-f]{7})' && continue
        pula=""
        for s in $cobertos; do [[ "$c" == "$s"* ]] && pula=1; done
        [[ -n "$pula" ]] && continue
        git diff-tree --no-commit-id --name-only -r --diff-filter="$filtro" "$c"
    done | sort -u
}

case "${1:-}" in
    --staged) arquivos="$(git diff --cached --name-only --diff-filter=ACMR)"
              novos="$(git diff --cached --name-only --diff-filter=A)" ;;
    *..*)     git rev-list "$1" >/dev/null || { echo "intervalo inválido: $1" >&2; exit 2; }
              arquivos="$(arquivos_sem_marca "$1"; git diff --name-only --diff-filter=ACMR "$1" | grep -E '^docs/atualizacoes/')"
              novos="$(arquivos_sem_marca "$1" A)" ;;
    *) echo "uso: $0 --staged | <de>..<para>" >&2; exit 2 ;;
esac
[[ -z "$arquivos" ]] && exit 0

# O que muda a MÁQUINA, não o código que o backend serve. Testes e o próprio hook ficam de fora:
# os hooks entram pelo core.hooksPath (mudança vale no próprio checkout, sem instalar nada).
relevantes="$(printf '%s\n' "$arquivos" | grep -E \
    '^(install\.(sh|ps1)|scripts/[^/]+\.(sh|ps1|py|mjs|cjs|js|fish)|hooks/.*|backend/app/[a-z_]*installer\.py)$' \
    | grep -Ev '^scripts/(hooks/|test-|check-passo-de-atualizacao\.sh)' \
    | grep -Ev '\.test\.(mjs|cjs|js)$' || true)"
# Skill nova só chega a quem atualiza pelo link que o instalador cria; editar uma existente não precisa.
skills_novas="$(printf '%s\n' "$novos" | grep -E '^skills/[^/]+/SKILL\.md$' || true)"
relevantes="$(printf '%s\n%s\n' "$relevantes" "$skills_novas" | grep -v '^$' || true)"
[[ -z "$relevantes" ]] && exit 0

passos="$(printf '%s\n' "$arquivos" | grep -E '^docs/atualizacoes/[^/]+\.md$' | grep -v 'README\.md$' || true)"
[[ -n "$passos" ]] && exit 0

cat >&2 <<FIM
  ── falta o passo de atualização ───────────────────────────────────────────
  Este commit muda o que a máquina de quem já usa NÃO recebe por git pull:
$(printf '%s\n' "$relevantes" | sed 's/^/      /')
  O botão Atualizar só roda o que um passo declarado pedir. Crie, no mesmo
  commit, um arquivo docs/atualizacoes/AAAA-MM-DD-<o-que-muda>.md com o
  comando cirúrgico e a prova (modelo e lista de comandos no README de lá).
  Mudança que não altera nada na máquina (comentário, teste):
      HANGAR_SEM_PASSO=1 git commit ...
FIM
exit 1
