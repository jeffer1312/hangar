#!/usr/bin/env bash
# Testa o bootstrap.sh no modo pacote (o padrão) sem rede: releases falsas por file://, um
# hangar-server falso publicado nelas (grava os argumentos e monta um `current` com um install.sh
# falso), git e uname falsos no PATH.
#
#   ./scripts/test-bootstrap-pacote.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
BASH_BIN=$(command -v bash)
fail=0
SYS_TOOLS="bash sh env cat mkdir cp printf awk dirname basename ls timeout setsid rm mv ln curl sha256sum tr grep sed mktemp chmod tee tail head cut date"

# new_box <nome>: S (raiz do caso), B (programas falsos), REL (as releases)
new_box() {
  S="$ROOT/$1"; B="$S/bin"; REL="$S/rel"
  mkdir -p "$B" "$S/sys" "$S/home" "$REL"
  local t p
  for t in $SYS_TOOLS; do
    p=$(command -v "$t" 2>/dev/null) && [ -x "$p" ] && ln -sf "$p" "$S/sys/$t"
  done
  # install.sh falso: grava argumentos, se recebeu a senha e onde rodou.
  cat > "$S/install-fake.sh" <<'EOF'
#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$@" > "$d/args"
printf '%s' "${HANGAR_TOKEN-}" > "$d/tok"
pwd > "$d/cwd"
exit 0
EOF
  chmod +x "$S/install-fake.sh"
  cat > "$B/git" <<'EOF'
#!/bin/sh
echo "git $*" >> "$FAKE_LOG"
case "$*" in
  --version) echo "git version 2.50.0" ;;
  "-C "*" remote get-url origin") cat "$2/.git/origin" 2>/dev/null || exit 2 ;;
  clone*) for a in "$@"; do dest=$a; done; mkdir -p "$dest/.git" && cp "$FAKE_INSTALL" "$dest/install.sh" ;;
esac
exit 0
EOF
  printf '#!/bin/sh\ncase "$1" in -s) echo "${FAKE_OS:-Linux}" ;; -m) echo "${FAKE_ARCH:-x86_64}" ;; esac\n' > "$B/uname"
  chmod +x "$B/git" "$B/uname"
  # O hangar-server falso: o `install-package` sai com FAKE_RC; com 0 monta o `current`.
  cat > "$S/server-fake" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$S/server.log"
[ -z "\${HANGAR_TOKEN-}" ] || echo "servidor viu a senha" >> "$S/server.log"
case "\${FAKE_RC:-0}" in
  0) v="\$HOME/.hangar/versions/0.1.0.42"; mkdir -p "\$v"
     echo '{"version":"0.1.0.42","commit":"c","channel":"latest"}' > "\$v/manifest.json"
     cp "$S/install-fake.sh" "\$v/install.sh"; ln -s "\$v" "\$HOME/.hangar/current"
     echo "instalado: versão 0.1.0.42" ;;
  3) echo "o índice não tem pacote para linux-x86_64; a máquina segue no motor git" >&2 ;;
  4) echo "o Hangar já está instalado pelo git em /x; ele passa para o pacote pelo Atualizar do app" >&2 ;;
  *) echo "a instalação falhou: rede" >&2 ;;
esac
exit "\${FAKE_RC:-0}"
EOF
}

# publish <canal> [plataforma] [sha]: o hangar-server falso na release server-<canal>, com o manifesto no formato do CI.
publish() {
  local canal=$1 plat=${2:-linux-x86_64} sha=${3:-} dir="$REL/server-$1"
  mkdir -p "$dir"
  cp "$S/server-fake" "$dir/hangar-server-$plat"
  [ -n "$sha" ] || sha=$(sha256sum "$dir/hangar-server-$plat" | awk '{print $1}')
  cat > "$dir/server-latest.json" <<EOF
{
  "commit": "c",
  "files": {
    "$plat/hangar-cano": {
      "name": "hangar-cano-$plat",
      "sha256": "$(printf '0%.0s' $(seq 64))"
    },
    "$plat/hangar-server": {
      "name": "hangar-server-$plat",
      "sha256": "$sha"
    }
  },
  "protocol": 89
}
EOF
}

# run_boot <saída> [args]
run_boot() {
  local out=$1; shift
  ( env -i HOME="$S/home" PATH="$B:$S/sys" FAKE_LOG="$S/git.log" FAKE_INSTALL="$S/install-fake.sh" \
      HANGAR_RELEASE_BASE="${TEST_RELEASE_BASE-file://$REL}" ${TEST_TOKEN:+HANGAR_TOKEN=$TEST_TOKEN} \
      ${FAKE_RC:+FAKE_RC=$FAKE_RC} ${FAKE_OS:+FAKE_OS=$FAKE_OS} ${FAKE_ARCH:+FAKE_ARCH=$FAKE_ARCH} \
      timeout 60 setsid -w "$BASH_BIN" "$REPO/bootstrap.sh" "$@" </dev/null 2>&1 ) > "$out"
}

pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; if [ -n "${2:-}" ] && [ -f "$2" ]; then sed 's/^/     | /' "$2" | tail -30; fi; fail=1; }
expect_line() { if grep -Eq -- "$3" "$2" 2>/dev/null; then pass "$1"; else flunk "$1 (sem /$3/)" "$2"; fi; }
expect_no()   { if grep -Eq -- "$3" "$2" 2>/dev/null; then flunk "$1 (achou /$3/)" "$2"; else pass "$1"; fi; }
expect_eq()   { if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 (veio '$2', esperado '$3')" "${4:-}"; fi; }
expect_absent() { if [ -e "$2" ]; then flunk "$1 ($2 existe)" "${3:-}"; else pass "$1"; fi; }
last_line()   { grep -av '^[[:space:]]*$' "$1" | tail -1; }
V="$ROOT/x"   # pasta da versão do caso, preenchida depois do run_boot
version_dir() { V="$S/home/.hangar/versions/0.1.0.42"; }

# --- Padrão: pacote do canal latest, opções repassadas ao install.sh do pacote, senha só para ele ---
new_box padrao; publish latest
TEST_TOKEN=segredo-123 run_boot "$S/out" --app --tailscale=nao --agentes=codex
version_dir
expect_eq "padrao: install-package do latest" "$(head -1 "$S/server.log" 2>/dev/null)" "install-package --channel latest --release-base file://$REL" "$S/out"
expect_eq "padrao: opções chegam ao install.sh do pacote" "$(paste -sd' ' "$V/args" 2>/dev/null)" '--app --tailscale=nao --agentes=codex' "$S/out"
expect_eq "padrao: install.sh roda do current" "$(cat "$V/cwd" 2>/dev/null)" "$S/home/.hangar/current"
expect_eq "padrao: install.sh recebe a senha" "$(cat "$V/tok" 2>/dev/null)" segredo-123
expect_no "padrao: o servidor não vê a senha" "$S/server.log" 'viu a senha'
expect_absent "padrao: nada de git" "$S/git.log"
expect_eq "padrao: temporário do download apagado" "$(ls "$S/home/.hangar/cache" 2>/dev/null | grep -c bootstrap)" 0

# --- --beta: release server-beta e canal beta; a flag não vai ao install.sh ---
new_box beta; publish beta
run_boot "$S/out" --beta --yes
version_dir
expect_eq "beta: install-package do beta" "$(head -1 "$S/server.log" 2>/dev/null)" "install-package --channel beta --release-base file://$REL" "$S/out"
expect_eq "beta: só --yes chega ao install.sh" "$(paste -sd' ' "$V/args" 2>/dev/null)" '--yes'

# --- sha256 que não confere: o binário não roda ---
new_box sha; publish latest linux-x86_64 "$(printf 'a%.0s' $(seq 64))"
run_boot "$S/out" --app
expect_absent "sha: hangar-server não rodou" "$S/server.log" "$S/out"
expect_line "sha: motivo na tela" "$S/out" 'sha256'
expect_eq "sha: FIM falhou é a última linha" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'
expect_absent "sha: nada de git" "$S/git.log"

# --- Sem pacote para este sistema (install-package sai com 3): segue pelo git, com o motivo ---
new_box sem-pacote; publish latest
FAKE_RC=3 run_boot "$S/out" --yes
expect_line "sem-pacote: motivo na tela" "$S/out" 'não tem pacote para linux-x86_64'
expect_line "sem-pacote: clona" "$S/git.log" "clone --branch main .* $S/home/hangar"
expect_eq "sem-pacote: install.sh do checkout" "$(paste -sd' ' "$S/home/hangar/args" 2>/dev/null)" '--yes' "$S/out"
expect_absent "sem-pacote: sem marca de --dev" "$S/home/.hangar/dev-requested"

# --- Release sem o hangar-server deste sistema: git, sem baixar nada ---
new_box sem-build; publish latest macos-aarch64
run_boot "$S/out" --yes
expect_line "sem-build: motivo na tela" "$S/out" 'não publica o hangar-server para linux-x86_64'
expect_absent "sem-build: hangar-server não rodou" "$S/server.log"
expect_eq "sem-build: install.sh do checkout" "$(paste -sd' ' "$S/home/hangar/args" 2>/dev/null)" '--yes' "$S/out"

# --- Entrada deste sistema ilegível: falha visível, sem cair no git com motivo falso ---
new_box entrada-torta; publish latest linux-x86_64 nao-e-um-sha
run_boot "$S/out" --app
expect_line "entrada-torta: diz que não leu a entrada" "$S/out" 'não consegui ler o nome e o sha256'
expect_no "entrada-torta: sem o motivo de sistema sem pacote" "$S/out" 'não publica o hangar-server'
expect_absent "entrada-torta: nada de git" "$S/git.log"
expect_eq "entrada-torta: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Sistema sem pacote (Linux arm): git direto ---
new_box arm; publish latest
FAKE_ARCH=aarch64 run_boot "$S/out" --yes
expect_line "arm: motivo na tela" "$S/out" 'sem pacote para Linux aarch64'
expect_absent "arm: hangar-server não rodou" "$S/server.log"
expect_line "arm: clona" "$S/git.log" 'clone'

# --- --dev: o caminho de hoje, e a migração vai deixar a máquina em modo desenvolvedor ---
new_box dev; publish latest
run_boot "$S/out" --dev --yes
expect_absent "dev: hangar-server não rodou" "$S/server.log" "$S/out"
expect_line "dev: clona a main" "$S/git.log" "clone --branch main .* $S/home/hangar"
expect_eq "dev: só --yes chega ao install.sh" "$(paste -sd' ' "$S/home/hangar/args" 2>/dev/null)" '--yes' "$S/out"
[ -e "$S/home/.hangar/dev-requested" ] && pass "dev: marca de --dev gravada" || flunk "dev: marca de --dev gravada" "$S/out"
new_box dev-beta
run_boot "$S/out" --dev --beta --yes
expect_line "dev-beta: clona a beta" "$S/git.log" 'clone --branch beta '

# --- --dev numa máquina já no pacote: recusa, o caminho é o dev-mode ---
new_box dev-pacote
mkdir -p "$S/home/.hangar/current"; echo '{}' > "$S/home/.hangar/current/manifest.json"
run_boot "$S/out" --dev --app
expect_line "dev-pacote: aponta o dev-mode" "$S/out" 'dev-mode on'
expect_absent "dev-pacote: nada de git clone" "$S/home/hangar"
expect_eq "dev-pacote: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Pasta explícita numa máquina já no pacote: a mesma recusa, sem clonar por cima ---
new_box pasta-pacote
mkdir -p "$S/home/.hangar/current"; echo '{}' > "$S/home/.hangar/current/manifest.json"
run_boot "$S/out" "$S/home/apps/hangar" --app
expect_line "pasta-pacote: aponta o dev-mode" "$S/out" 'dev-mode on'
expect_absent "pasta-pacote: nada de git" "$S/git.log"
expect_absent "pasta-pacote: nada clonado" "$S/home/apps/hangar"
expect_eq "pasta-pacote: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Já instalado pelo pacote: roda o install.sh dele de novo, sem baixar ---
new_box de-novo; publish latest
mkdir -p "$S/home/.hangar/current"; echo '{}' > "$S/home/.hangar/current/manifest.json"
cp "$S/install-fake.sh" "$S/home/.hangar/current/install.sh"
run_boot "$S/out" --yes
expect_absent "de-novo: nada baixado" "$S/server.log"
expect_eq "de-novo: install.sh do current" "$(paste -sd' ' "$S/home/.hangar/current/args" 2>/dev/null)" '--yes' "$S/out"

# --- install-package recusa (instalação existente): para com o motivo ---
new_box recusa; publish latest
FAKE_RC=4 run_boot "$S/out" --app
expect_line "recusa: motivo na tela" "$S/out" 'já está instalado pelo git'
expect_eq "recusa: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'
expect_absent "recusa: nada de git" "$S/git.log"
expect_line "recusa: aponta o Atualizar e o --dev" "$S/out" 'use o Atualizar do app.*--dev'
expect_no "recusa: não é 'parou'" "$S/out" 'instalação do pacote parou'

# --- Checkout do Hangar na pasta padrão (instalado com --no-services): para antes do install-package ---
new_box checkout; publish latest
mkdir -p "$S/home/hangar/.git"; echo "https://github.com/jeffer1312/hangar.git" > "$S/home/hangar/.git/origin"
run_boot "$S/out" --app
expect_absent "checkout: hangar-server não rodou" "$S/server.log" "$S/out"
expect_line "checkout: aponta o Atualizar e o --dev" "$S/out" "$S/home/hangar.*use o Atualizar do app.*--dev"
expect_eq "checkout: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Release por https: o curl só aceita https ---
new_box https
printf '#!/bin/sh\necho "curl $*" >> "%s/curl.log"\nexit 22\n' "$S" > "$B/curl"; chmod +x "$B/curl"
TEST_RELEASE_BASE='' run_boot "$S/out" --app
expect_line "https: manifesto com --proto =https" "$S/curl.log" "^curl --proto =https .*https://github.com/jeffer1312/hangar/releases/download/server-latest/server-latest.json"

# --- --check sem instalação pelo pacote: não instala nada ---
new_box check; publish latest
run_boot "$S/out" --check
expect_absent "check: nada baixado" "$S/server.log"
expect_line "check: diz o que fazer" "$S/out" 'rode sem --check'

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
