#!/usr/bin/env bash
# O install-hangar-send.sh rodado de dentro de um pacote (~/.hangar/current -> versions/<v>): os links
# que ele grava fora do Hangar (atalhos, skills) apontam para o `current`, nunca para a pasta da versão,
# que a poda apaga na atualização seguinte; o registro, os agentes e o bloco do CLAUDE.md vão ao
# `hangar-server` com a raiz pelo `current`. HOME temporário; os scripts são os reais; o `hangar-server`
# é falso (anota os argumentos e a entrada) e nenhum Python é chamado.
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
mkdir -p "$V/scripts" "$V/skills/teste/agents" "$H/.hangar/config" "$H/.local/bin" "$ROOT/stubs"
cp -r "$REPO/scripts/." "$V/scripts/"
printf -- '---\nname: teste\ndescription: x\n---\nskill\n' > "$V/skills/teste/SKILL.md"
printf -- '---\nname: agente-teste\ndescription: x\n---\ncorpo\n' > "$V/skills/teste/agents/agente-teste.md"
echo '{}' > "$V/manifest.json"
printf 'CP_AUTH_TOKEN=abc12345\n' > "$H/.hangar/config/.env"
ln -s "$V" "$H/.hangar/current"

# hangar-server falso: uma linha por chamada com os argumentos; a entrada do claude-md-block vai para um arquivo.
SERVER="$ROOT/fake-hangar-server"
cat > "$SERVER" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "$ROOT/calls"
if [ "\$1" = claude-md-block ]; then cat > "$ROOT/block"; fi
exit 0
STUB
chmod +x "$SERVER"
# Sem python3 nem hangar-python no PATH: chamar qualquer um deles fica anotado e falha.
for py in python3 python hangar-python; do
  printf '#!/bin/sh\necho "%s chamado: $*" >> "%s/python-calls"\nexit 97\n' "$py" "$ROOT" > "$ROOT/stubs/$py"
  chmod +x "$ROOT/stubs/$py"
done
run_install() {
  : > "$ROOT/calls"
  ( env -i HOME="$H" CP_RUST_SERVER_BIN="$SERVER" PATH="$ROOT/stubs:/usr/bin:/bin" \
      bash "$1" > "$ROOT/out" 2>&1 )
}

run_install "$H/.hangar/current/scripts/install-hangar-send.sh"; rc=$?
expect_eq "sai 0 sem o install-hooks.sh do checkout" "$rc" 0 "$ROOT/out"
C="$H/.hangar/current"
expect_eq "hangar-send pelo current" "$(readlink "$H/.local/bin/hangar-send")" "$C/scripts/hangar-send" "$ROOT/out"
if [ -e "$H/.local/bin/hangar-python" ] || [ -L "$H/.local/bin/hangar-python" ]; then
  flunk "hangar-python não é mais instalado"
else
  pass "hangar-python não é mais instalado"
fi
expect_eq "skill pelo current" "$(readlink "$H/.claude/skills/teste")" "$C/skills/teste"
expect_eq "registro, agentes e bloco pelo hangar-server, com a raiz pelo current" "$(cat "$ROOT/calls")" \
    "$(printf 'mcp-register --root %s\nagents-install --root %s\nclaude-md-block' "$C" "$C")" "$ROOT/out"
if head -1 "$ROOT/block" | grep -qx '<!-- claude-pocket:sessoes-irmas:start -->' \
    && tail -1 "$ROOT/block" | grep -qx '<!-- claude-pocket:sessoes-irmas:end -->'; then
  pass "o bloco chega inteiro pela entrada"
else
  flunk "o bloco chega inteiro pela entrada" "$ROOT/block"
fi
if [ -e "$ROOT/python-calls" ]; then flunk "nenhum Python chamado" "$ROOT/python-calls"; else pass "nenhum Python chamado"; fi

# Máquina já migrada e os instaladores rodando do checkout de antes, como o motor git faz com os
# passos que vêm depois da migração na mesma rodada: os links continuam no `current`.
K="$ROOT/checkout"
mkdir -p "$K"
cp -r "$REPO/scripts" "$K/scripts"
cp -r "$V/skills" "$K/skills"
printf '{\n  "mode": "package",\n  "checkout": "%s"\n}\n' "$K" > "$H/.hangar/migration.json"
run_install "$K/scripts/install-hangar-send.sh"; rc=$?
expect_eq "checkout: sai 0" "$rc" 0 "$ROOT/out"
expect_eq "checkout: hangar-send pelo current" "$(readlink "$H/.local/bin/hangar-send")" "$C/scripts/hangar-send" "$ROOT/out"
expect_eq "checkout: skill pelo current" "$(readlink "$H/.claude/skills/teste")" "$C/skills/teste"
expect_eq "checkout: registro com a raiz pelo current" "$(head -1 "$ROOT/calls")" "mcp-register --root $C" "$ROOT/out"
( env -i HOME="$H" SHELL=/bin/bash CP_RUST_SERVER_BIN="$SERVER" PATH="$ROOT/stubs:/usr/bin:/bin" \
    bash "$K/scripts/install-claude-wrapper.sh" bash --no-tmux --no-statusline > "$ROOT/out" 2>&1 ); rc=$?
expect_eq "checkout: wrapper sai 0" "$rc" 0 "$ROOT/out"
expect_eq "checkout: hangar-codex pelo current" "$(readlink "$H/.local/bin/hangar-codex")" "$C/scripts/hangar-codex" "$ROOT/out"
expect_eq "checkout: hangar-open pelo current" "$(readlink "$H/.local/bin/hangar-open")" "$C/scripts/hangar-open"
grep -q "source \"$C/scripts/shell/" "$H/.bashrc" 2>/dev/null && pass "checkout: bloco do shell pelo current" \
    || flunk "checkout: bloco do shell pelo current" "$H/.bashrc"

# Um passo que falha para o instalador (set -e): depois do registro que falhou, nada roda calado.
cat > "$SERVER" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "$ROOT/calls"
[ "\$1" = mcp-register ] && { echo "erro: falso" >&2; exit 1; }
exit 0
STUB
run_install "$H/.hangar/current/scripts/install-hangar-send.sh"; rc=$?
expect_eq "registro que falha para o instalador" "$rc" 1 "$ROOT/out"
expect_eq "nada depois do registro que falhou" "$(cat "$ROOT/calls")" "mcp-register --root $C" "$ROOT/out"

# Binário anterior ao código no --update do checkout: o resto roda, e a saída 3 diz que algo foi pulado.
cat > "$SERVER" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "$ROOT/calls"
[ "\$1" = agents-install ] && { echo "hangar-server: subcomando desconhecido: agents-install" >&2; exit 2; }
if [ "\$1" = claude-md-block ]; then cat > "$ROOT/block"; fi
exit 0
STUB
: > "$ROOT/calls"; rm -f "$ROOT/block"
( env -i HOME="$H" CP_RUST_SERVER_BIN="$SERVER" PATH="$ROOT/stubs:/usr/bin:/bin" HANGAR_SRV_ANTIGO_AVISA=1 \
    bash "$H/.hangar/current/scripts/install-hangar-send.sh" > "$ROOT/out" 2>&1 ); rc=$?
expect_eq "binário antigo: sai 3, não 0" "$rc" 3 "$ROOT/out"
grep -q '^claude-md-block' "$ROOT/calls" && [ -s "$ROOT/block" ] && pass "binário antigo: o bloco do CLAUDE.md ainda é gravado" \
    || flunk "binário antigo: o bloco do CLAUDE.md ainda é gravado" "$ROOT/out"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
