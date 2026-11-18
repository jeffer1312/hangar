#!/usr/bin/env bash
# claude-conta (bash e fish) contra um hangar-conta e um claude falsos: sem argumento lista e
# devolve o código; com nome, só abre o claude com UM caminho de diretório vindo do `--prep`.
#
#   ./scripts/test-claude-conta.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; fail=1; }
expect_eq() { if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 (veio '$2', esperado '$3')"; fi; }

B="$ROOT/bin"; CONTA="$ROOT/conta-a"
mkdir -p "$B" "$CONTA"
# PREP: o que o --prep imprime (\n vira quebra de linha); LIST_RC: código do --list.
cat > "$B/hangar-conta" <<STUB
#!/bin/sh
echo "hangar-conta \$*" >> "$ROOT/calls"
case "\$1" in
  --list) exit "\${LIST_RC:-0}" ;;
  --prep) [ -n "\${PREP_FAIL:-}" ] && exit 1; printf '%b\n' "\$PREP"; exit 0 ;;
esac
STUB
printf '#!/bin/sh\necho "claude CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR $*" >> "%s/calls"\n' "$ROOT" > "$B/claude"
chmod +x "$B/hangar-conta" "$B/claude"

# roda <shell> <args...>: chama a função no shell com o PATH falso; deixa RC e CALLS.
roda() {
  local sh=$1; shift
  : > "$ROOT/calls"
  if [ "$sh" = bash ]; then
    PATH="$B:/usr/bin:/bin" bash -c '. "$0"; claude-conta "$@"' "$REPO/scripts/shell/claude-conta.posix.sh" "$@" 2> "$ROOT/err"
  else
    PATH="$B:/usr/bin:/bin" fish --no-config -c "source '$REPO/scripts/shell/claude-conta.fish'; claude-conta \$argv" "$@" 2> "$ROOT/err"
  fi
  RC=$?; CALLS=$(cat "$ROOT/calls")
}

shells=bash
command -v fish >/dev/null && shells="bash fish" || echo "pulado: fish não instalado"
for sh in $shells; do
  LIST_RC=5 roda "$sh"
  expect_eq "$sh: sem argumento lista" "$CALLS" "hangar-conta --list"
  expect_eq "$sh: devolve o código do --list" "$RC" 5
  PREP="$CONTA" roda "$sh" a --resume x
  expect_eq "$sh: abre na conta" "$CALLS" "hangar-conta --prep a
claude CLAUDE_CONFIG_DIR=$CONTA --resume x"
  expect_eq "$sh: abre sai 0" "$RC" 0
  PREP="$CONTA\\nlinha a mais" roda "$sh" a
  expect_eq "$sh: duas linhas não abrem" "$CALLS" "hangar-conta --prep a"
  expect_eq "$sh: duas linhas saem 1" "$RC" 1
  PREP="$ROOT/nao-existe" roda "$sh" a
  expect_eq "$sh: não diretório não abre" "$CALLS" "hangar-conta --prep a"
  expect_eq "$sh: não diretório sai 1" "$RC" 1
  PREP_FAIL=1 roda "$sh" a
  expect_eq "$sh: --prep que falha não abre na conta padrão" "$CALLS" "hangar-conta --prep a"
  expect_eq "$sh: --prep que falha sai 1" "$RC" 1
done
PREP="$CONTA\\nlinha a mais" roda bash a
expect_eq "bash: frase da saída inesperada" "$(cat "$ROOT/err")" "claude-conta: saída inesperada de hangar-conta --prep"
PREP="$ROOT/nao-existe" roda bash a
expect_eq "bash: frase do não diretório" "$(cat "$ROOT/err")" "claude-conta: $ROOT/nao-existe não é um diretório"

[ $fail = 0 ] && echo "tudo ok"
exit $fail
