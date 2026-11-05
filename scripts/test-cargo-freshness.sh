#!/usr/bin/env bash
# Sem argumento prova a invalidação; --with-cargo também compila duas árvores isoladas.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
sandbox="$temporary/com espaços"
mkdir -p "$sandbox"
build="$sandbox/build"
target="$sandbox/target"
local_entry="$build/debug/.fingerprint/local-package-012abc"
cross="$build/triple/release/.fingerprint/local-package-012abc"
dependency="$build/debug/.fingerprint/local-dependency-012abc"
external="$build/debug/.fingerprint/serde-012abc"
similar="$build/debug/.fingerprint/local-package-extra-012abc"
artifact="$build/debug/deps/library.rlib"
mkdir -p "$local_entry" "$cross" "$dependency" "$external" "$similar" "${artifact%/*}"
printf artifact > "$artifact"
jq -n --arg workspace "$sandbox/first" --arg target "$target" --arg build "$build" \
    '{workspace_root:$workspace,target_directory:$target,build_directory:$build,packages:[{name:"local-package",source:null,dependencies:[{name:"local-dependency",path:"local"}]}]}' > "$sandbox/metadata.json"
prepare() { bash "$root/scripts/cargo-freshness.sh" "$sandbox/metadata.json" "$target"; }
prepare
[[ ! -e "$local_entry" && ! -e "$cross" && ! -e "$dependency" ]]
[[ -d "$external" && -d "$similar" && $(cat "$artifact") == artifact ]]
mkdir -p "$local_entry"
prepare
[[ -d "$local_entry" ]]
jq '.workspace_root += "-other"' "$sandbox/metadata.json" > "$sandbox/next.json"
mv "$sandbox/next.json" "$sandbox/metadata.json"
prepare
[[ ! -e "$local_entry" && -d "$external" && $(cat "$artifact") == artifact ]]
# Um fingerprint que é link perde só o link, nunca o diretório externo.
ln -s "$external" "$local_entry"
printf '{}' > "$build/.hangar-cargo-workspace.json"
prepare
[[ ! -L "$local_entry" && -d "$external" ]]
# Um selo que virou diretório não pode receber o temporário como se a publicação funcionasse.
rm "$build/.hangar-cargo-workspace.json"
mkdir "$build/.hangar-cargo-workspace.json"
if prepare 2>"$sandbox/error"; then exit 1; fi
[[ -s "$sandbox/error" ]]
printf 'Invalidação seletiva, cache, caminhos com espaços e links: OK\n'
[[ ${1:-} == --with-cargo ]] || exit 0
for name in first second; do
    checkout="$sandbox/$name"
    mkdir -p "$checkout/scripts" "$checkout/crates/library/src" "$checkout/crates/app/src"
    cp "$root/scripts/cargo-pinned" "$root/scripts/pin-auth-pair.py" "$root/scripts/cargo-freshness.sh" "$checkout/scripts/"
    git init -q "$checkout"
    printf '[workspace]\nmembers=["library","app"]\nresolver="3"\n' > "$checkout/crates/Cargo.toml"
    printf '[package]\nname="cargo-freshness-library"\nversion="0.1.0"\nedition="2024"\n[lib]\ncrate-type=["rlib","cdylib"]\n' > "$checkout/crates/library/Cargo.toml"
    printf "pub fn value()->&'static str{\"%s\"}\n" "$name" > "$checkout/crates/library/src/lib.rs"
    printf '[package]\nname="cargo-freshness-app"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ncargo-freshness-library={path="../library"}\n' > "$checkout/crates/app/Cargo.toml"
    printf 'fn main(){println!("{}",cargo_freshness_library::value());}\n' > "$checkout/crates/app/src/main.rs"
done
# A segunda árvore é mais velha que os artefatos da primeira, reproduzindo o falso frescor.
find "$sandbox/second/crates" -type f -exec touch -t 200001010000 {} +
build_tree() {
    (cd "$sandbox/$1" && CARGO_TARGET_DIR="$sandbox/shared-target" bash scripts/cargo-pinned build --offline -p cargo-freshness-app) > "$sandbox/build.log" 2>&1
    [[ $("$sandbox/$1/crates/target/debug/cargo-freshness-app") == "$1" ]]
}
build_tree first
cp "$sandbox/first/crates/target/debug/cargo-freshness-app" "$sandbox/first.before"
build_tree second
cmp "$sandbox/first.before" "$sandbox/first/crates/target/debug/cargo-freshness-app"
build_tree second
if grep -q 'Compiling cargo-freshness-library' "$sandbox/build.log"; then
    cat "$sandbox/build.log" >&2
    exit 1
fi
printf 'Duas árvores reais e cache da mesma árvore: OK\n'
