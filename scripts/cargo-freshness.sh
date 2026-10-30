#!/usr/bin/env bash
# Chamado sob a trava do cargo-pinned; não compila nem altera artefatos publicados.
set -euo pipefail
metadata=$1
target=$(realpath -m "$2")
workspace=$(realpath -m "$(jq -er .workspace_root "$metadata")")
original=$(realpath -m "$(jq -er .target_directory "$metadata")")
build=$(realpath -m "$(jq -er '.build_directory // .target_directory' "$metadata")")
[[ "$build" != "$original" ]] || build=$target
state=$(jq -cS --arg workspace "$workspace" '{workspace:$workspace,packages:([
    .packages[] | select(.source == null) | .name
] + [.packages[].dependencies[]? | select(.path != null) | .name] | unique)}' "$metadata")
mkdir -p -- "$build"
stamp="$build/.hangar-cargo-workspace.json"
if [[ -f "$stamp" ]] && previous=$(jq -cS . "$stamp" 2>/dev/null) && [[ "$previous" == "$state" ]]; then
    exit 0
fi
mapfile -t names < <(jq -r '.packages[]' <<< "$state")
removed=0
shopt -s nullglob
for fingerprints in "$build"/*/.fingerprint "$build"/*/*/.fingerprint; do
    for entry in "$fingerprints"/*; do
        name=${entry##*/}
        [[ "$name" =~ ^(.+)-[0-9a-f]+$ ]] || continue
        package=${BASH_REMATCH[1]}
        for local_name in "${names[@]}"; do
            [[ "$package" == "$local_name" ]] || continue
            if [[ -L "$entry" ]]; then
                rm -- "$entry"
            elif [[ -d "$entry" ]]; then
                rm -r -- "$entry"
            else
                break
            fi
            removed=$((removed+1))
            break
        done
    done
done
# Uma compilação falha também pode deixar artefatos desta árvore no target comum.
temporary=$(mktemp "$build/.hangar-cargo-workspace.XXXXXX")
trap 'rm -f -- "$temporary"' EXIT
printf '%s\n' "$state" > "$temporary"
mv -fT -- "$temporary" "$stamp"
if (( removed > 0 )); then
    printf 'cargo-pinned: %s fingerprints locais invalidados na troca de árvore\n' "$removed" >&2
fi
