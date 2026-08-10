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
for h in scripts/hooks/*; do
    [[ -f "$h" ]] || continue
    printf '  %s %s\n' "$([[ -x "$h" ]] && echo ok || echo 'SEM +x')" "$(basename "$h")"
done
echo
echo "teste rápido:  echo 'TICKET-0000' >> /tmp/x && git add -f /tmp/x  (deve recusar)"
echo "pular numa emergência:  git commit --no-verify"
