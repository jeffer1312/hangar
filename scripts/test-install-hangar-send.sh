#!/usr/bin/env bash
# O install-hangar-send.sh rodado de dentro de um pacote (~/.hangar/current -> versions/<v>): os links
# que ele grava fora do Hangar (atalhos, skills, agentes) apontam para o `current`, nunca para a
# pasta da versão, que a poda apaga na atualização seguinte. HOME temporário; os scripts são os reais.
#
#   ./scripts/test-install-hangar-send.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; [ -n "${2:-}" ] && sed 's/^/     | /' "$2" | tail -20; fail=1; }
expect_eq() { if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 (veio '$2', esperado '$3')" "${4:-}"; fi; }

H="$ROOT/home"; V="$H/.hangar/versions/0.1.0.42"
mkdir -p "$V/scripts" "$V/skills/teste/agents" "$H/.hangar/config" "$H/.local/bin"
# O que o pacote leva (o install-hooks.sh fica fora dele: é de quem desenvolve).
for f in install-hangar-send.sh registrar-mcp.py instalar-agentes.py hangar-send hangar-python hangar-preview hangar-doctor folha \
         hangar-mcp-headers.sh; do
  cp "$REPO/scripts/$f" "$V/scripts/$f"
done
printf -- '---\nname: teste\ndescription: x\n---\nskill\n' > "$V/skills/teste/SKILL.md"
printf -- '---\nname: agente-teste\ndescription: x\n---\ncorpo\n' > "$V/skills/teste/agents/agente-teste.md"
echo '{}' > "$V/manifest.json"
printf 'CP_AUTH_TOKEN=abc12345\n' > "$H/.hangar/config/.env"
ln -s "$V" "$H/.hangar/current"

( env -i HOME="$H" PATH="$(dirname "$(command -v python3)"):/usr/bin:/bin" \
    bash "$H/.hangar/current/scripts/install-hangar-send.sh" > "$ROOT/out" 2>&1 ); rc=$?
expect_eq "sai 0 sem o install-hooks.sh do checkout" "$rc" 0 "$ROOT/out"
C="$H/.hangar/current"
expect_eq "hangar-send pelo current" "$(readlink "$H/.local/bin/hangar-send")" "$C/scripts/hangar-send" "$ROOT/out"
expect_eq "hangar-python pelo current" "$(readlink "$H/.local/bin/hangar-python")" "$C/scripts/hangar-python"
expect_eq "skill pelo current" "$(readlink "$H/.claude/skills/teste")" "$C/skills/teste"
expect_eq "agente pelo current" "$(readlink "$H/.claude/agents/agente-teste.md")" "$C/skills/teste/agents/agente-teste.md" "$ROOT/out"

# Máquina já migrada e os instaladores rodando do checkout de antes, como o motor git faz com os
# passos que vêm depois da migração na mesma rodada: os links continuam no `current`.
K="$ROOT/checkout"
mkdir -p "$K"
cp -r "$REPO/scripts" "$K/scripts"
cp -r "$V/skills" "$K/skills"
cp -r "$REPO/scripts/." "$V/scripts/"
printf '{\n  "mode": "package",\n  "checkout": "%s"\n}\n' "$K" > "$H/.hangar/migration.json"
( env -i HOME="$H" PATH="$(dirname "$(command -v python3)"):/usr/bin:/bin" \
    bash "$K/scripts/install-hangar-send.sh" > "$ROOT/out" 2>&1 ); rc=$?
expect_eq "checkout: sai 0" "$rc" 0 "$ROOT/out"
expect_eq "checkout: hangar-send pelo current" "$(readlink "$H/.local/bin/hangar-send")" "$C/scripts/hangar-send" "$ROOT/out"
expect_eq "checkout: skill pelo current" "$(readlink "$H/.claude/skills/teste")" "$C/skills/teste"
( env -i HOME="$H" SHELL=/bin/bash PATH="$(dirname "$(command -v python3)"):/usr/bin:/bin" \
    bash "$K/scripts/install-claude-wrapper.sh" bash --no-tmux --no-statusline > "$ROOT/out" 2>&1 ); rc=$?
expect_eq "checkout: wrapper sai 0" "$rc" 0 "$ROOT/out"
expect_eq "checkout: lançador do Codex pelo current" "$(readlink "$H/.local/bin/hangar-codex-tui")" "$C/scripts/hangar-codex-tui" "$ROOT/out"
expect_eq "checkout: hangar-open pelo current" "$(readlink "$H/.local/bin/hangar-open")" "$C/scripts/hangar-open"
grep -q "source \"$C/scripts/shell/" "$H/.bashrc" 2>/dev/null && pass "checkout: bloco do shell pelo current" \
    || flunk "checkout: bloco do shell pelo current" "$H/.bashrc"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
