#!/usr/bin/env bash
# Bootstrap do primeiro servidor; depois da instalação o executor Rust faz as próximas trocas.
set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
run_bootstrap() {
  if [ "${HANGAR_UPDATE_OWNS_RUST:-}" = 1 ]; then
    printf '%s\n' 'binários Rust: quem baixa é o Atualizar, pelo commit escolhido'
    return 0
  fi
  local platform branch tag url branch_tag='' missing='ausente' dest ext='' scratch status manifest name sha asset target data actual failed=0 kept='' installed=''
  case "$(uname -s):$(uname -m)" in
    Linux:x86_64) platform=linux-x86_64 ;;
    Darwin:arm64) platform=macos-aarch64 ;;
    Darwin:x86_64) platform=macos-x86_64 ;;
    *) printf '%s\n' 'binários Rust: a release não tem build para esta máquina'; return 2 ;;
  esac
  branch=$(git -C "$ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null) || branch=main
  case "$branch" in main|master|HEAD|'') tag=server-latest ;; *) tag="server-$(printf '%s' "$branch" | sed 's/[^A-Za-z0-9._-]/-/g')" ;; esac
  if [ -n "${HANGAR_SERVER_RELEASE_URL:-}" ]; then url=${HANGAR_SERVER_RELEASE_URL%/}; tag=''
  else url="https://github.com/jeffer1312/hangar/releases/download/$tag"; fi
  dest=${HANGAR_SERVER_BIN_DIR:-"$HOME/.hangar/bin"}
  scratch=$(mktemp -d) || return 1
  manifest="$scratch/manifest.json"
  status=$(curl --silent --show-error --location --max-time 30 --output "$manifest" --write-out '%{http_code}' "$url/server-latest.json") || status=000
  if [ "$status" = 404 ] && [ -n "$tag" ] && [ "$tag" != server-latest ]; then branch_tag=$tag
  elif [ "$status" != 200 ]; then
    printf 'binários Rust não baixados: não consegui ler o manifesto da release (HTTP %s)\n' "$status"
    rm -rf -- "$scratch"; return 1
  elif ! node -e 'try {const m=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));process.exit(m&&!Array.isArray(m)&&typeof m==="object"?0:1)} catch {process.exit(1)}' "$manifest"; then
    printf '%s\n' 'binários Rust não baixados: não consegui ler o manifesto da release'; rm -rf -- "$scratch"; return 1
  elif [ -n "$tag" ] && [ "$tag" != server-latest ] && ! node -e 'const fs=require("fs");try { const m=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); const p=process.argv[2]; const object=v=>v!==null&&typeof v==="object"&&!Array.isArray(v);process.exit(object(m.files)&&["hangar-server","hangar-cano"].every(n=>object(m.files[p+"/"+n]))?0:1); } catch { process.exit(1); }' "$manifest" "$platform"; then
    branch_tag=$tag; missing="sem build para $platform"
  fi
  if [ -n "$branch_tag" ]; then
    url='https://github.com/jeffer1312/hangar/releases/download/server-latest'
    status=$(curl --silent --show-error --location --max-time 30 --output "$manifest" --write-out '%{http_code}' "$url/server-latest.json") || status=000
  fi
  if [ "$status" != 200 ] || ! node -e 'const m=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));process.exit(m&&!Array.isArray(m)&&typeof m==="object"&&Object.hasOwn(m,"files")?0:1)' "$manifest" 2>/dev/null; then
    printf '%s\n' 'binários Rust não baixados: não consegui ler o manifesto da release'; rm -rf -- "$scratch"; return 1
  fi
  mkdir -p -- "$dest" || { rm -rf -- "$scratch"; return 1; }
  for name in hangar-server hangar-cano; do
    target="$dest/$name"
    node -e 'const fs=require("fs"),path=require("path"),dir=process.argv[1],prefix=(process.argv[2]+".old").toLowerCase();try{for(const n of fs.readdirSync(dir)){if(n.toLowerCase().startsWith(prefix)){try{fs.unlinkSync(path.join(dir,n))}catch{}}}}catch{}' "$dest" "$name"
    if [ -n "$branch_tag" ] && { [ -e "$target" ] || [ -L "$target" ]; }; then kept="${kept:+$kept, }$name"; continue; fi
    asset=$(node -e 'const m=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));const e=m.files?.[process.argv[2]+"/"+process.argv[3]];if(typeof e?.name!=="string"||typeof e?.sha256!=="string")process.exit(1);process.stdout.write(e.name)' "$manifest" "$platform" "$name") || { printf '%s: a release não traz build para %s\n' "$name" "$platform"; failed=1; continue; }
    sha=$(node -e 'const m=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));process.stdout.write(m.files[process.argv[2]+"/"+process.argv[3]].sha256.toLowerCase())' "$manifest" "$platform" "$name") || { failed=1; continue; }
    actual=$(node -e 'try{process.stdout.write(require("crypto").createHash("sha256").update(require("fs").readFileSync(process.argv[1])).digest("hex"))}catch{process.exit(1)}' "$target" 2>/dev/null) || actual=''
    if [ "$actual" != "$sha" ]; then
      data="$scratch/$name"
      if ! curl --fail --silent --show-error --location --max-time 200 --output "$data" "$url/$asset"; then printf '%s: o download falhou\n' "$name"; failed=1; continue; fi
      actual=$(node -e 'process.stdout.write(require("crypto").createHash("sha256").update(require("fs").readFileSync(process.argv[1])).digest("hex"))' "$data") || actual=''
      if [ "$actual" != "$sha" ]; then printf '%s: o sha256 não confere com a release; o binário anterior ficou\n' "$name"; failed=1; continue; fi
      data=$(mktemp "$dest/.$name.XXXXXXXX.tmp") || { failed=1; continue; }
      if ! cp -- "$scratch/$name" "$data" || ! chmod 755 "$data" || ! node -e 'require("fs").renameSync(process.argv[1],process.argv[2])' "$data" "$target"; then rm -f -- "$data"; printf '%s: não consegui gravar o binário\n' "$name"; failed=1; continue; fi
    fi
    if [ -n "$branch_tag" ]; then installed="${installed:+$installed, }$name"; fi
  done
  rm -rf -- "$scratch"
  if [ -n "$kept" ]; then printf 'release %s %s; mantive os binários instalados (%s)\n' "$branch_tag" "$missing" "$kept"; failed=1; fi
  if [ -n "$installed" ]; then printf 'release %s %s; instalei os da main (%s)\n' "$branch_tag" "$missing" "$installed"; failed=1; fi
  return "$failed"
}
run_bootstrap
result=$?
if [ "${1:-}" = --never-fail ]; then exit 0; fi
if [ "${1:-}" = --fetch ] && [ "$result" != 2 ]; then exit 0; fi
exit "$result"
