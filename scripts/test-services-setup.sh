#!/usr/bin/env bash
# Trava da unit do backend numa máquina migrada para o pacote: o services-setup.sh (que o
# `install.sh --update` roda) mantém a unit no lançador estável e lê/grava o `.env` de
# ~/.hangar/config, em vez de voltar ao `uv run` do checkout. HOME e programas falsos; nunca toca
# o systemd de verdade.
#
#   ./scripts/test-services-setup.sh            # o script deste checkout
#   SCRIPT=/outro/services-setup.sh ./scripts/test-services-setup.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${SCRIPT:-$REPO/scripts/services-setup.sh}"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
fail=0
pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; fail=1; }
expect_line() { if grep -Fqx -- "$3" "$2" 2>/dev/null; then pass "$1"; else flunk "$1 (sem a linha '$3')"; sed 's/^/     | /' "$2" 2>/dev/null; fi; }
expect_no()   { if grep -Eq -- "$3" "$2" 2>/dev/null; then flunk "$1 (achou /$3/)"; else pass "$1"; fi; }

# sandbox <nome> <migrada 0|1> [opções; padrão --backend-only, SEM_OPCOES=1 roda sem nenhuma]:
# repositório falso com o script, HOME e programas falsos. PACOTE_REPO=1 faz o repositório ser um
# pacote extraído (manifest.json, sem .git).
sandbox() {
  local nome=$1 migrada=$2; shift 2
  [ $# -gt 0 ] || [ "${SEM_OPCOES:-0}" = 1 ] || set -- --backend-only
  S="$ROOT/$nome"; R="$S/repo"; B="$S/bin"; H="$S/home"
  mkdir -p "$R/scripts" "$R/backend/app" "$B" "$H/.config/systemd/user" "$S/sys"
  [ "${PACOTE_REPO:-0}" = 1 ] && echo '{}' > "$R/manifest.json"
  # Checkout que ainda sobe pelo Python; COM_PYTHON=0 é o checkout depois que ele saiu.
  [ "${COM_PYTHON:-1}" = 1 ] && : > "$R/backend/app/main.py"
  cp "$SCRIPT" "$R/scripts/services-setup.sh"
  printf 'CP_AUTH_TOKEN=velho\n' > "$R/backend/.env"
  for t in bash sh env cat grep sed tr cut tail head dirname readlink printf mkdir rm cmp cp chmod mv; do
    p=$(command -v "$t") && ln -sf "$p" "$S/sys/$t"
  done
  local fakes="systemctl loginctl uv"
  [ "${PACOTE_REPO:-0}" = 1 ] && fakes="systemctl loginctl"   # o pacote não tem uv, npm nem node
  for t in $fakes; do printf '#!/bin/sh\necho "%s $*" >> "%s/calls"\nexit 0\n' "$t" "$S" > "$B/$t"; chmod +x "$B/$t"; done
  # FAKE_SYSTEMD: a versão que o `systemctl --version` falso diz (padrão 255).
  printf '#!/bin/sh\necho "systemctl $*" >> "%s/calls"\n[ "$1" = --version ] && echo "systemd %s (%s.1-1)"\nexit 0\n' \
    "$S" "${FAKE_SYSTEMD:-255}" "${FAKE_SYSTEMD:-255}" > "$B/systemctl"; chmod +x "$B/systemctl"
  if [ "$migrada" = 1 ]; then
    mkdir -p "$H/.hangar/config" "$H/.hangar/bin" "$H/.hangar/versions/0.1.0.5"
    ln -s "$H/.hangar/versions/0.1.0.5" "$H/.hangar/current"
    printf '#!/bin/sh\n' > "$H/.hangar/bin/hangar-launcher"; chmod +x "$H/.hangar/bin/hangar-launcher"
    if [ "${VERSAO_COM_BIN:-0}" = 1 ]; then
      mkdir -p "$H/.hangar/versions/0.1.0.5/bin"
      printf '#!/bin/sh\n# novo\n' > "$H/.hangar/versions/0.1.0.5/bin/hangar-server"; chmod +x "$H/.hangar/versions/0.1.0.5/bin/hangar-server"
    fi
    # BOOT_DIRECT=1: a versão que sobe o hangar-server sem o Python (manifesto com `boot: direct`).
    [ "${BOOT_DIRECT:-0}" = 1 ] && printf '{\n  "boot": "direct",\n  "version": "0.1.0.5"\n}\n' > "$H/.hangar/versions/0.1.0.5/manifest.json"
    printf 'CP_AUTH_TOKEN=novo\nCP_FRONT_PORT=5173\n' > "$H/.hangar/config/.env"
  fi
  RC=0
  ( cd "$R" && env -i HOME="$H" USER=teste PATH="$B:$S/sys" bash scripts/services-setup.sh "$@" >"$S/out" 2>&1 ) || RC=$?
  [ "${ESPERA_FALHA:-0}" = 1 ] || [ "$RC" = 0 ] || { flunk "$nome: o script falhou"; sed 's/^/     | /' "$S/out"; }
  UNIT="$H/.config/systemd/user/hangar-backend.service"
}

sandbox migrada 1
expect_line "migrada: unit chama o lançador" "$UNIT" "ExecStart=\"$H/.hangar/bin/hangar-launcher\" launch backend"
expect_line "migrada: fora do checkout" "$UNIT" "WorkingDirectory=$H/.hangar"
expect_no "migrada: sem uv run" "$UNIT" 'uv run'
# P1, decisão 2: o systemd nunca desiste e espera a parada protegida. Lançador que não dá para
# conferir com a versão (aqui, versão sem bin/) não avisa o systemd: Type=simple, sem watchdog.
expect_line "migrada: lançador não conferido fica simple" "$UNIT" "Type=simple"
expect_no "migrada: sem watchdog" "$UNIT" 'WatchdogSec'
expect_line "migrada: nunca desiste" "$UNIT" "StartLimitIntervalSec=0"
expect_line "migrada: espera crescente até 10 min" "$UNIT" "RestartMaxDelaySec=10min"
expect_line "migrada: parada protegida" "$UNIT" "TimeoutStopSec=120"
expect_line "migrada: o cgroup sai junto" "$UNIT" "KillMode=control-group"
expect_line "migrada: lançador velho fica sem versão com bin/" "$H/.hangar/bin/hangar-launcher" '#!/bin/sh'
expect_no "migrada: CP_FRONT_PORT saiu do .env novo" "$H/.hangar/config/.env" 'CP_FRONT_PORT'
expect_line "migrada: o .env velho não foi tocado" "$R/backend/.env" 'CP_AUTH_TOKEN=velho'

sandbox antiga 0
expect_line "antiga: unit no uv do checkout" "$UNIT" "ExecStart=$B/uv run python -m app.main"
expect_line "antiga: no backend do checkout" "$UNIT" "WorkingDirectory=$R/backend"
expect_line "antiga: o Python não avisa o systemd" "$UNIT" "Type=simple"
expect_no "antiga: sem watchdog" "$UNIT" 'WatchdogSec'

# Decisão 7: checkout sem o app/main.py sobe o hangar-server pela mesma escolha de binário do CLI.
COM_PYTHON=0 sandbox checkout-rust 0
expect_line "checkout-rust: unit no hangar-server" "$UNIT" "ExecStart=\"$R/scripts/hangar-server-command\""
# Checkout: o binário pode ser de antes do aviso ao systemd; fica simple.
expect_line "checkout-rust: Type=simple" "$UNIT" "Type=simple"

# Decisão 1: o lançador estável é renovado com o hangar-server da versão em uso.
VERSAO_COM_BIN=1 BOOT_DIRECT=1 sandbox renova 1
expect_line "renova: lançador é o da versão" "$H/.hangar/bin/hangar-launcher" '# novo'
# P2: versão com `boot: direct` e lançador igual ao dela avisa o systemd, que vigia o laço.
expect_line "renova: Type=notify" "$UNIT" "Type=notify"
expect_line "renova: só o processo principal avisa" "$UNIT" "NotifyAccess=main"
expect_line "renova: watchdog" "$UNIT" "WatchdogSec=60"
expect_line "renova: subida lenta mas viva não é morta" "$UNIT" "TimeoutStartSec=300"

# Instalação nova pelo pacote: sem npm, sem --backend-only, sem pergunta; o backend serve a tela.
PACOTE_REPO=1 SEM_OPCOES=1 sandbox pacote 1 </dev/null
expect_line "pacote: unit chama o lançador" "$UNIT" "ExecStart=\"$H/.hangar/bin/hangar-launcher\" launch backend"
expect_no "pacote: sem unit do front" "$H/.config/systemd/user/hangar-frontend.service" '.'
expect_no "pacote: não pede npm" "$S/out" 'npm nao encontrado|node_modules'
[ -e "$H/.config/systemd/user/hangar-frontend.service" ] && flunk "pacote: unit do front criada" || pass "pacote: unit do front não criada"
expect_no "pacote: sem dica do npm run dev" "$S/out" 'npm --prefix frontend'
expect_no "pacote: sem caminho relativo do checkout" "$S/out" '\./scripts/services-setup\.sh'
expect_line "pacote: dica pelo current" "$S/out" '  • Status:   ~/.hangar/current/scripts/services-setup.sh --status'

# Pacote sem o lançador (install-package não rodou): recusa, em vez de gravar uma unit com uv.
PACOTE_REPO=1 SEM_OPCOES=1 ESPERA_FALHA=1 sandbox pacote-sem-lancador 0
[ "$RC" != 0 ] && pass "pacote-sem-lancador: falha" || flunk "pacote-sem-lancador: falha"
expect_no "pacote-sem-lancador: nenhuma unit" "$UNIT" '.'
grep -q 'install-package' "$S/out" && pass "pacote-sem-lancador: diz o que rodar" || flunk "pacote-sem-lancador: diz o que rodar"

# Versão que ainda sobe pelo Python (sem `boot: direct`): lançador renovado, mas Type=simple.
VERSAO_COM_BIN=1 sandbox renova-python 1
expect_line "renova-python: Type=simple" "$UNIT" "Type=simple"
expect_no "renova-python: sem watchdog" "$UNIT" 'WatchdogSec|TimeoutStartSec'

# systemd < 254 ignora RestartSteps/RestartMaxDelaySec: espera fixa de 10 s, nunca o laço de 2 s.
FAKE_SYSTEMD=252 sandbox systemd-velho 1
expect_line "systemd-velho: espera fixa de 10 s" "$UNIT" "RestartSec=10"
expect_no "systemd-velho: sem RestartSteps" "$UNIT" 'RestartSteps|RestartMaxDelaySec|RestartSec=2$'
expect_line "systemd-novo: espera crescente" "$ROOT/migrada/home/.config/systemd/user/hangar-backend.service" "RestartSteps=8"

if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
