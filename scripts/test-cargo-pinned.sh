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
mkdir -p "$repo/crates" "$repo/scripts" "$tmp/bin" "$comum/debug"
git -C "$repo" init -q
# O cargo-pinned chama os dois pela raiz do checkout: a cópia é a real, a conferência de frescor não entra aqui.
cp "$(dirname "$P")/pin-auth-pair.py" "$repo/scripts/"
printf '#!/usr/bin/env bash\n' > "$repo/scripts/cargo-freshness.sh"

# cargo falso: metadata devolve o target; build grava o executável e emite a mensagem do artefato.
cat > "$tmp/bin/cargo" <<'EOF'
#!/usr/bin/env bash
alvo="${CARGO_TARGET_DIR:-$FAKE_TARGET}"
if [[ "$1" == metadata ]]; then printf '{"target_directory":"%s"}\n' "$alvo"; exit 0; fi
[[ -n "${FAKE_ARGS:-}" ]] && echo "$*" > "$FAKE_ARGS"
[[ -n "${FAKE_FALHA:-}" ]] && { echo "error: falhou" >&2; exit 101; }
mkdir -p "$alvo/debug"
echo "$FAKE_CONTEUDO" > "$alvo/debug/hangar-server"; chmod +x "$alvo/debug/hangar-server"
printf '{"reason":"compiler-artifact","profile":{"test":false},"executable":"%s/debug/hangar-server"}\n' "$alvo"
printf '{"reason":"compiler-artifact","profile":{"test":false},"executable":null}\n'
printf '{"reason":"compiler-artifact","profile":{"test":true},"executable":"%s/debug/deps/it-abc"}\n' "$alvo"
echo "running 1 test"
printf '{"reason":"build-finished","success":true}\n'
EOF
chmod +x "$tmp/bin/cargo"
# systemd-run falso: registra a chamada, falha quando pedido e executa o comando sem criar escopo.
cat > "$tmp/bin/systemd-run" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "${FAKE_ESCOPO_LOG:-/dev/null}"
case "${FAKE_ESCOPO:-}" in
    falha) exit 1 ;;
    so-sonda) [[ " $* " == *" -- "* ]] && exit 1 ;;
esac
while (( $# )); do case "$1" in --) shift; break ;; -p) shift 2 ;; -*) shift ;; *) break ;; esac; done
exec "$@"
EOF
chmod +x "$tmp/bin/systemd-run"
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

# Target já é o do checkout: compila à parte e fixa a cópia nele no fim.
mkdir -p "$repo/crates/target/debug"
(cd "$repo" && PATH="$tmp/bin:$PATH" CARGO_TARGET_DIR="$repo/crates/target" FAKE_CONTEUDO=v4 "$P" build) >/dev/null 2>&1 || \
    { echo "FALHOU build com target próprio saiu com erro"; falhou=1; }
[[ "$(cat "$repo/crates/target/debug/hangar-server")" == v4 ]] || { echo "FALHOU target próprio perdeu o executável"; falhou=1; }

# Jobs e teto saem da memória livre: um job por 9 GB além de 2 GB de folga, entre 1 e o número de
# núcleos; teto = livre - folga, no mínimo 4 GB.
roda_com() {   # $1 = MemAvailable em GiB; o resto vai como ambiente. Deixa args, escopo e erro em $tmp.
    local livre="$1"; shift
    printf 'MemAvailable: %d kB\n' $((livre * 1024 * 1024)) > "$tmp/meminfo"
    rm -f "$tmp/args" "$tmp/escopo"
    (cd "$repo" && env -u CARGO_BUILD_JOBS -u HANGAR_BUILD_GB_POR_JOB "$@" PATH="$tmp/bin:$PATH" \
        FAKE_TARGET="$comum" FAKE_CONTEUDO=v5 FAKE_ARGS="$tmp/args" FAKE_ESCOPO_LOG="$tmp/escopo" \
        HANGAR_MEMINFO="$tmp/meminfo" "$P" build) >/dev/null 2>"$tmp/err"
}
jobs_de() { sed -n 's/.* -j \([0-9]*\) .*/\1/p' "$tmp/args" 2>/dev/null; }
nucleos="$(nproc)"
esperado=$(( nucleos < 4 ? nucleos : 4 ))
roda_com 40; [[ "$(jobs_de)" == "$esperado" ]] || { echo "FALHOU 40 GB livres não deram -j $esperado"; falhou=1; }
grep -q -- "-- bash" "$tmp/escopo" 2>/dev/null && grep -q "MemoryHigh=$((38 * 1024 * 1024))K" "$tmp/escopo" || \
    { echo "FALHOU 40 GB livres não rodaram no escopo com teto de 38 GB"; falhou=1; }
roda_com 3; [[ "$(jobs_de)" == 1 ]] || { echo "FALHOU 3 GB livres não deram -j 1"; falhou=1; }
grep -q "MemoryHigh=$((4 * 1024 * 1024))K" "$tmp/escopo" 2>/dev/null || { echo "FALHOU teto mínimo não é 4 GB"; falhou=1; }
roda_com 3 CARGO_BUILD_JOBS=5; [[ "$(jobs_de)" == 5 ]] || { echo "FALHOU CARGO_BUILD_JOBS explícito não venceu"; falhou=1; }
roda_com 40 HANGAR_BUILD_GB_POR_JOB=0; [[ "$(jobs_de)" == "$esperado" ]] && grep -q "não é um inteiro positivo" "$tmp/err" || \
    { echo "FALHOU HANGAR_BUILD_GB_POR_JOB inválido não voltou para 9 com aviso"; falhou=1; }
# Sem escopo, compila sem teto; escopo que falha depois da sonda compila de novo, sem teto.
roda_com 40 FAKE_ESCOPO=falha && [[ -n "$(jobs_de)" ]] && grep -q "sem teto" "$tmp/err" || \
    { echo "FALHOU sem systemd-run o build não rodou sem teto"; falhou=1; }
roda_com 40 FAKE_ESCOPO=so-sonda && [[ -n "$(jobs_de)" ]] && grep -q "não abriu" "$tmp/err" || \
    { echo "FALHOU escopo que não abriu não recompilou sem teto"; falhou=1; }
FAKE_FALHA=1 roda_com 40 && { echo "FALHOU erro do cargo no escopo saiu 0"; falhou=1; }
grep -q "não abriu" "$tmp/err" && { echo "FALHOU erro do cargo no escopo foi tratado como escopo que não abriu"; falhou=1; }

(( falhou )) && exit 1
echo "ok: cargo-pinned"
