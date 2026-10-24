#!/usr/bin/env bash
# Confere o scripts/cargo-pinned contra um cargo falso: copia o executável do target compartilhado
# para crates/target do checkout, não copia quando o target já é o do checkout, e falha do cargo
# sai como falha.
# Usage: ./scripts/test-cargo-pinned.sh
set -uo pipefail

P="$(realpath "$(dirname "$0")/cargo-pinned")"
falhou=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 HANGAR_BUILD_LOCK="$tmp/lock"
repo="$tmp/repo" comum="$tmp/comum"
mkdir -p "$repo/crates" "$tmp/bin" "$comum/debug"
git -C "$repo" init -q

# cargo falso: metadata devolve o target; build grava o executável e emite a mensagem do artefato.
cat > "$tmp/bin/cargo" <<'EOF'
#!/usr/bin/env bash
alvo="${CARGO_TARGET_DIR:-$FAKE_TARGET}"
if [[ "$1" == metadata ]]; then printf '{"target_directory":"%s"}\n' "$alvo"; exit 0; fi
[[ -n "${FAKE_FALHA:-}" ]] && { echo "error: falhou" >&2; exit 101; }
echo "$FAKE_CONTEUDO" > "$alvo/debug/hangar-server"; chmod +x "$alvo/debug/hangar-server"
printf '{"reason":"compiler-artifact","executable":"%s/debug/hangar-server"}\n' "$alvo"
printf '{"reason":"compiler-artifact","executable":null}\n'
printf '{"reason":"compiler-artifact","profile":{"test":true},"executable":"%s/debug/deps/it-abc"}\n' "$alvo"
echo "running 1 test"
printf '{"reason":"build-finished","success":true}\n'
EOF
chmod +x "$tmp/bin/cargo"
roda() { (cd "$repo" && PATH="$tmp/bin:$PATH" FAKE_TARGET="$comum" "$P" build -p hangar-server); }

saida="$(FAKE_CONTEUDO=v1 roda 2>/dev/null)" || { echo "FALHOU build saiu com erro"; falhou=1; }
# A saída dos testes passa; as mensagens JSON do cargo, não.
[[ "$saida" == "running 1 test" ]] || { echo "FALHOU saída repassada: \"$saida\""; falhou=1; }
[[ "$(cat "$repo/crates/target/debug/hangar-server" 2>/dev/null)" == v1 ]] || \
    { echo "FALHOU executável não foi fixado no checkout"; falhou=1; }
[[ ! -e "$repo/crates/target/debug/deps/it-abc" ]] || { echo "FALHOU copiou executável de teste"; falhou=1; }
[[ -x "$repo/crates/target/debug/hangar-server" ]] || { echo "FALHOU cópia sem permissão de execução"; falhou=1; }

# Outro checkout recompila no target comum: a cópia deste não muda.
echo v2 > "$comum/debug/hangar-server"
[[ "$(cat "$repo/crates/target/debug/hangar-server")" == v1 ]] || { echo "FALHOU cópia acompanhou o target comum"; falhou=1; }

FAKE_FALHA=1 FAKE_CONTEUDO=v3 roda >/dev/null 2>&1 && { echo "FALHOU erro do cargo saiu 0"; falhou=1; }
[[ "$(cat "$repo/crates/target/debug/hangar-server")" == v1 ]] || { echo "FALHOU build com erro trocou a cópia"; falhou=1; }

# Target já é o do checkout: nada a copiar, e o arquivo não é apagado.
mkdir -p "$repo/crates/target/debug"
(cd "$repo" && PATH="$tmp/bin:$PATH" CARGO_TARGET_DIR="$repo/crates/target" FAKE_CONTEUDO=v4 "$P" build) >/dev/null 2>&1 || \
    { echo "FALHOU build com target próprio saiu com erro"; falhou=1; }
[[ "$(cat "$repo/crates/target/debug/hangar-server")" == v4 ]] || { echo "FALHOU target próprio perdeu o executável"; falhou=1; }

(( falhou )) && exit 1
echo "ok: cargo-pinned"
