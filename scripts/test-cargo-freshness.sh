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
# As mesmas entradas no layout novo do cargo: <perfil>/build/<pacote>/<hash>/{fingerprint,out}.
unit_local="$build/debug/build/local-package/012abc"
unit_cross="$build/triple/release/build/local-package/012abc"
unit_external="$build/debug/build/serde/012abc"
unit_similar="$build/debug/build/local-package-extra/012abc"
mkdir -p "$local_entry" "$cross" "$dependency" "$external" "$similar" "${artifact%/*}" \
    "$unit_local/fingerprint" "$unit_local/out" "$unit_cross/fingerprint" "$unit_external/fingerprint" "$unit_similar/fingerprint"
printf artifact > "$artifact"
printf artifact > "$unit_local/out/library.rlib"
jq -n --arg workspace "$sandbox/first" --arg target "$target" --arg build "$build" \
    '{workspace_root:$workspace,target_directory:$target,build_directory:$build,packages:[{name:"local-package",source:null,dependencies:[{name:"local-dependency",path:"local"}]}]}' > "$sandbox/metadata.json"
prepare() { bash "$root/scripts/cargo-freshness.sh" "$sandbox/metadata.json" "$target"; }
prepare
[[ ! -e "$local_entry" && ! -e "$cross" && ! -e "$dependency" ]]
[[ -d "$external" && -d "$similar" && $(cat "$artifact") == artifact ]]
[[ ! -e "$unit_local/fingerprint" && ! -e "$unit_cross/fingerprint" ]]
[[ -d "$unit_external/fingerprint" && -d "$unit_similar/fingerprint" && $(cat "$unit_local/out/library.rlib") == artifact ]]
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
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 CARGO_HOME="$temporary/cargo-home" CARGO_TERM_COLOR=never
unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR CARGO_BUILD_BUILD_DIR
for name in first second; do
    checkout="$sandbox/$name"
    mkdir -p "$checkout/scripts" "$checkout/crates/library/src/bin" "$checkout/crates/app/src" "$checkout/crates/app/tests"
    cp "$root/scripts/cargo-pinned" "$root/scripts/pin-auth-pair.py" "$root/scripts/cargo-freshness.sh" \
        "$root/scripts/cargo-worktree-cache.py" "$root/scripts/cargo-worktree-rustc" "$checkout/scripts/"
    # O layout do target muda com a versão do cargo: a prova usa a do repositório.
    cp "$root/crates/rust-toolchain.toml" "$checkout/crates/"
    git init -q "$checkout"
    printf '[workspace]\nmembers=["library","app"]\nresolver="3"\n' > "$checkout/crates/Cargo.toml"
    printf '[package]\nname="cargo-freshness-library"\nversion="0.1.0"\nedition="2024"\n[lib]\ncrate-type=["rlib","cdylib"]\n' > "$checkout/crates/library/Cargo.toml"
    printf "pub fn value()->&'static str{\"%s\"}\n" "$name" > "$checkout/crates/library/src/lib.rs"
    printf 'fn main(){println!("{}",cargo_freshness_library::value());}\n' > "$checkout/crates/library/src/bin/cargo-freshness-helper.rs"
    printf '[package]\nname="cargo-freshness-app"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ncargo-freshness-library={path="../library"}\n' > "$checkout/crates/app/Cargo.toml"
    printf 'fn main(){println!("{}",cargo_freshness_library::value());}\n' > "$checkout/crates/app/src/main.rs"
    cat > "$checkout/crates/app/tests/auxiliary.rs" <<'RS'
#[test]
fn reads_the_helper_from_the_same_target() {
    let app = std::path::Path::new(env!("CARGO_BIN_EXE_cargo-freshness-app"));
    let output = std::process::Command::new(app.with_file_name("cargo-freshness-helper"))
        .output().expect("executável auxiliar do mesmo target");
    assert!(output.status.success());
    let marker = String::from_utf8(output.stdout).expect("saída UTF-8");
    assert_eq!(marker.trim(), std::env::var("WORKTREE_CACHE_EXPECTED").unwrap());
}
RS
done
# A segunda árvore é mais velha que os artefatos da primeira, reproduzindo o falso frescor.
find "$sandbox/second/crates" -type f -exec touch -t 200001010000 {} +
build_tree() {
    local name=$1; shift
    (cd "$sandbox/$name" && env "$@" bash scripts/cargo-pinned build --offline \
        -p cargo-freshness-app -p cargo-freshness-library) \
        > "$sandbox/build.log" 2>&1 || { cat "$sandbox/build.log" >&2; return 1; }
    [[ $("$sandbox/$name/crates/target/debug/cargo-freshness-app") == "$name" ]]
}
assert_reused() {
    if grep -Eq 'Compiling cargo-freshness-(app|library)' "$sandbox/build.log"; then
        cat "$sandbox/build.log" >&2
        return 1
    fi
}
shared_target="CARGO_TARGET_DIR=$sandbox/shared-target"
build_tree first "$shared_target"
cp "$sandbox/first/crates/target/debug/cargo-freshness-app" "$sandbox/first.before"
build_tree second "$shared_target"
cmp "$sandbox/first.before" "$sandbox/first/crates/target/debug/cargo-freshness-app"
build_tree second "$shared_target"
assert_reused
printf 'Duas árvores reais e cache da mesma árvore: OK\n'
[[ "$OSTYPE" == linux* || "$OSTYPE" == darwin* ]] || exit 0
build_tree first "$shared_target"
assert_reused
printf 'Cache compartilhado preservado na volta: OK\n'

# Sem configuração de target, as worktrees do mesmo repositório devem usar o cache comum.
git -C "$sandbox/first" add scripts crates/Cargo.toml crates/Cargo.lock crates/rust-toolchain.toml crates/app crates/library
git -C "$sandbox/first" -c user.name=Teste -c user.email=teste@example.invalid -c core.hooksPath=/dev/null commit -qm teste
git -C "$sandbox/first" -c core.hooksPath=/dev/null worktree add --detach --quiet "$sandbox/linked"
printf "pub fn value()->&'static str{\"linked\"}\n" > "$sandbox/linked/crates/library/src/lib.rs"
build_tree first
build_tree linked
build_tree first
[[ -d "$sandbox/first/crates/target/.cargo-pinned-shared/debug" ]]
[[ ! -e "$sandbox/first/crates/target/.cargo-pinned-shared/debug/cargo-freshness-helper" ]]
assert_reused
printf 'Cache padrão compartilhado entre worktrees do mesmo repositório: OK\n'

for name in first linked; do
    (cd "$sandbox/$name" && WORKTREE_CACHE_EXPECTED="$name" bash scripts/cargo-pinned \
        test --offline -p cargo-freshness-app --test auxiliary) > "$sandbox/build.log" 2>&1 || {
        cat "$sandbox/build.log" >&2
        exit 1
    }
    grep -Fxq 'test reads_the_helper_from_the_same_target ... ok' "$sandbox/build.log"
done
printf 'Teste de integração usou o executável auxiliar da própria worktree: OK\n'

mkdir -p "$CARGO_HOME"
printf '[build]\nrustc-workspace-wrapper = "configured-rustc"\n' > "$CARGO_HOME/config.toml"
cat > "$CARGO_HOME/configured-rustc" <<'SH'
#!/usr/bin/env sh
touch "$CARGO_CACHE_TEST_MARK"
exec "$@"
SH
chmod +x "$CARGO_HOME/configured-rustc"
build_tree first "PATH=$CARGO_HOME:$PATH" "CARGO_CACHE_TEST_MARK=$sandbox/custom-wrapper-used"
[[ -f "$sandbox/custom-wrapper-used" ]]
printf 'Wrapper configurado pelo usuário preservado: OK\n'
