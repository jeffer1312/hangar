#!/usr/bin/env bash
# Liga os hooks versionados deste repo (scripts/hooks/) apontando core.hooksPath pra lá.
#
# Por que hooksPath e não symlink em .git/hooks: hooksPath é UMA config e vale pra pasta inteira,
# então um hook novo entra sem ninguém reinstalar nada. E como o caminho é relativo à raiz do
# repo, funciona igual em worktree.
#
# Idempotente. Desligar:  git config --unset core.hooksPath
set -euo pipefail

RAIZ="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
cd "$RAIZ"

chmod +x scripts/hooks/* 2>/dev/null || true
git config core.hooksPath scripts/hooks

echo "hooks ligados: $(git config core.hooksPath)"
# Falha ALTO se algum hook ficou sem +x. O `|| true` do chmod acima é de propósito (FS que não
# guarda o bit, permissão), mas sem este exit o script terminava em 0 dizendo "hooks ligados" com a
# trava de vazamento DESLIGADA — e quem encadeia `install-hooks.sh && ...` só olha o código de saída,
# não o "SEM +x" perdido no meio do stdout.
sem_x=0
for h in scripts/hooks/*; do
    [[ -f "$h" ]] || continue
    if [[ -x "$h" ]]; then
        printf '  ok %s\n' "$(basename "$h")"
    else
        printf '  SEM +x %s\n' "$(basename "$h")"
        sem_x=1
    fi
done
if (( sem_x )); then
    echo
    echo "FALHOU: hook sem permissão de execução — o git NÃO vai rodá-lo. Confira o filesystem." >&2
    exit 1
fi
echo
echo "teste rápido:  echo 'TICKET-0000' >> /tmp/x && git add -f /tmp/x  (deve recusar)"
echo "pular numa emergência:  git commit --no-verify"
