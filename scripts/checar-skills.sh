#!/usr/bin/env bash
# Confere que as skills de que uma sessao depende estao MESMO instaladas, antes de ela
# comecar a trabalhar.
#
# Por que existe: em 15/08/2026 seis plugins (superpowers e example entre eles) estavam
# habilitados no settings.json mas com o installPath apontando pra ~/.claude-work, que nao
# existe. O Claude Code carregava o plugin vazio, sem erro nenhum — a skill simplesmente nao
# aparecia. Uma sessao so descobria isso ao tentar invocar, no meio da Task. E o Pi tem o
# problema irmao: a ponte ~/.pi/agent/skills-bridge cobria so plugins, nenhuma skill pessoal.
#
# Uso:
#   scripts/checar-skills.sh                      # confere o conjunto padrao
#   scripts/checar-skills.sh writing-plans verify # confere uma lista sua
#
# Sai 0 se tudo estiver la; 1 se faltar alguma (e diz o que fazer).

set -uo pipefail

CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
PONTE_PI="$HOME/.pi/agent/skills-bridge"

# O conjunto que a skill orchestrating-idea-to-push usa de ponta a ponta.
PADRAO=(
  orchestrating-idea-to-push
  writing-plans
  executing-plans
  subagent-driven-development
  test-driven-development
  using-git-worktrees
  verification-before-completion
  systematic-debugging
)

ALVOS=("$@")
[ ${#ALVOS[@]} -eq 0 ] && ALVOS=("${PADRAO[@]}")

# Onde uma skill pode morar. A busca e por SKILL.md, nao por diretorio: plugin com
# installPath quebrado deixa o diretorio "existindo" no registro e vazio no disco.
achar() {
  local nome="$1" alvo
  for alvo in \
    "$CLAUDE_DIR/skills/$nome/SKILL.md" \
    "$HOME/.claude/skills/$nome/SKILL.md" \
    "$PONTE_PI/$nome/SKILL.md"
  do
    [ -f "$alvo" ] && { echo "$alvo"; return 0; }
  done
  # Plugins: qualquer versao em cache serve, desde que o SKILL.md esteja mesmo la.
  alvo=$(find "$HOME/.claude/plugins/cache" -maxdepth 5 -path "*/skills/$nome/SKILL.md" \
         -print -quit 2>/dev/null)
  [ -n "$alvo" ] && { echo "$alvo"; return 0; }
  return 1
}

faltando=()
echo "Conferindo ${#ALVOS[@]} skills…"
for s in "${ALVOS[@]}"; do
  if onde=$(achar "$s"); then
    printf '  ok     %-32s %s\n' "$s" "${onde/#$HOME/\~}"
  else
    printf '  FALTA  %s\n' "$s"
    faltando+=("$s")
  fi
done

# Registro de plugin apontando pra caminho inexistente e o defeito silencioso que originou
# este script: vale avisar mesmo quando todas as skills pedidas foram achadas.
reg="$HOME/.claude/plugins/installed_plugins.json"
if [ -f "$reg" ] && command -v python3 >/dev/null 2>&1; then
  quebrados=$(python3 - "$reg" <<'PY'
import json, os, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
ruins = {k for k, v in d.get("plugins", {}).items()
         for i in v if not os.path.isdir(i.get("installPath") or "")}
print("\n".join(sorted(ruins)))
PY
)
  if [ -n "$quebrados" ]; then
    echo
    echo "AVISO: plugins registrados com caminho que nao existe (carregam vazios, sem erro):"
    echo "$quebrados" | sed 's/^/  /'
    echo "  Conserto: apontar o installPath pra pasta real em ~/.claude/plugins/cache/."
  fi
fi

if [ ${#faltando[@]} -gt 0 ]; then
  echo
  echo "PARE: ${#faltando[@]} skill(s) faltando. Nao comece a Task sem elas — avise o arbitro."
  echo "  Claude: rode /reload-plugins; se nao voltar, o installPath esta quebrado (acima)."
  echo "  Pi:     falta o symlink em $PONTE_PI (as pessoais de ~/.claude/skills nao entram sozinhas)."
  exit 1
fi

echo
echo "Tudo no lugar."
