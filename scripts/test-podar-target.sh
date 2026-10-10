#!/usr/bin/env bash
# Confere o scripts/podar-target num target falso: o que sai e o que fica em cada modo.
# Usage: ./scripts/test-podar-target.sh
set -uo pipefail

P="$(cd "$(dirname "$0")" && pwd)/podar-target"
falhou=0

# Data de "$1 horas atrás" no formato do `touch -t`, com o `date` do GNU ou do BSD.
horas_atras() {
    local t=$(( $(date +%s) - $1 * 3600 ))
    date -d "@$t" +%Y%m%d%H%M.%S 2>/dev/null || date -r "$t" +%Y%m%d%H%M.%S
}

arquivo() {   # $1 = caminho, $2 = idade em horas ("" = agora), $3 = x para executável
    mkdir -p "$(dirname "$1")"
    head -c 4096 /dev/urandom > "$1"
    [[ -z "${2:-}" ]] || touch -t "$(horas_atras "$2")" "$1"
    [[ "${3:-}" != x ]] || chmod +x "$1"
}

sobrou() {   # $1 = raiz; imprime os arquivos restantes, relativos, numa linha
    (cd "$1" && find . -type f | sed 's|^\./||' | LC_ALL=C sort | tr '\n' ' ')
}

confere() {   # $1 = rótulo, $2 = obtido, $3 = esperado
    [[ "$2" == "$3" ]] || { printf 'FALHOU %s\n  sobrou:   "%s"\n  esperado: "%s"\n' "$1" "$2" "$3"; falhou=1; }
}

# ── máquina: por nome, a cópia mais nova e as do mesmo build; as velhas saem ──────────────────
d="$(mktemp -d)"
t="$d/target/debug"
arquivo "$t/deps/it-aaaaaaaaaaaaaaaa" "" x                       # executável de teste atual
arquivo "$t/deps/it-aaaaaaaaaaaaaaaa.d"
arquivo "$t/deps/it-bbbbbbbbbbbbbbbb" 72 x             # cópia velha do mesmo teste
arquivo "$t/deps/it-bbbbbbbbbbbbbbbb.d" 72
arquivo "$t/deps/hangar_server-cccccccccccccccc" "" x            # bin e teste unitário do mesmo build
arquivo "$t/deps/hangar_server-dddddddddddddddd" 2 x
arquivo "$t/deps/hangar_server-eeeeeeeeeeeeeeee" 120 x
arquivo "$t/deps/proxy-ffffffffffffffff" 216 x          # sozinho: fica, mesmo velho
arquivo "$t/deps/libserde-1111111111111111.rlib" 216    # biblioteca nunca sai
arquivo "$t/deps/libserde-2222222222222222.rlib"
arquivo "$t/deps/libserde_derive-3333333333333333.so" 216 x
arquivo "$t/deps/libserde_derive-4444444444444444.so" "" x
arquivo "$t/incremental/hangar_server-1abc/s-novo/a"
arquivo "$t/incremental/hangar_server-2def/s-velho/a" 96
touch -t "$(horas_atras 96)" "$t/incremental/hangar_server-2def"
arquivo "$t/incremental/it-3ghi/s/a" 96
touch -t "$(horas_atras 96)" "$t/incremental/it-3ghi"
"$P" "$d/target" >/dev/null || { echo "FALHOU podar-target saiu com erro (máquina)"; falhou=1; }
confere "máquina" "$(sobrou "$t")" "deps/hangar_server-cccccccccccccccc deps/hangar_server-dddddddddddddddd deps/it-aaaaaaaaaaaaaaaa deps/it-aaaaaaaaaaaaaaaa.d deps/libserde-1111111111111111.rlib deps/libserde-2222222222222222.rlib deps/libserde_derive-3333333333333333.so deps/libserde_derive-4444444444444444.so deps/proxy-ffffffffffffffff incremental/hangar_server-1abc/s-novo/a incremental/it-3ghi/s/a "
rm -rf "$d"

# ── cache do CI: sai tudo do workspace, ficam as dependências de terceiros ────────────────────
d="$(mktemp -d)"
mkdir -p "$d/ws/membro/src"
cat > "$d/ws/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["membro"]
EOF
cat > "$d/ws/membro/Cargo.toml" <<'EOF'
[package]
name = "hangar-membro"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "hangar-membro"
path = "src/main.rs"

[[test]]
name = "it"
path = "src/it.rs"
EOF
touch "$d/ws/membro/src/main.rs" "$d/ws/membro/src/it.rs"
for perfil in debug dist x86_64-unknown-linux-gnu/dist; do
    t="$d/ws/target/$perfil"
    arquivo "$t/hangar-membro" "" x
    arquivo "$t/hangar-membro.d"
    arquivo "$t/deps/hangar_membro-aaaaaaaaaaaaaaaa" "" x
    arquivo "$t/deps/it-bbbbbbbbbbbbbbbb" "" x
    arquivo "$t/deps/it-bbbbbbbbbbbbbbbb.d"
    arquivo "$t/deps/libserde-1111111111111111.rlib"
    arquivo "$t/.fingerprint/hangar-membro-aaaaaaaaaaaaaaaa/x"
    arquivo "$t/.fingerprint/serde-1111111111111111/x"
    arquivo "$t/build/hangar-membro-aaaaaaaaaaaaaaaa/out"
    arquivo "$t/build/ring-2222222222222222/out"
    arquivo "$t/incremental/hangar_membro-1abc/s/a"
    arquivo "$t/examples/exemplo-cccccccccccccccc" "" x
done
"$P" --cache "$d/ws/target" >/dev/null || { echo "FALHOU podar-target saiu com erro (cache)"; falhou=1; }
for perfil in debug dist x86_64-unknown-linux-gnu/dist; do
    confere "cache $perfil" "$(sobrou "$d/ws/target/$perfil")" ".fingerprint/serde-1111111111111111/x build/ring-2222222222222222/out deps/libserde-1111111111111111.rlib "
done
rm -rf "$d"

(( falhou )) && exit 1
echo "ok: podar-target"
