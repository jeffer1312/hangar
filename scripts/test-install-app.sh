#!/usr/bin/env bash
# Testa o install.sh no modo --app (assistente do app nativo) sem instalar nada: o install.sh é
# copiado para um repositório falso, o HOME é temporário e o PATH só tem programas falsos e o mínimo
# do sistema. Nunca toca o checkout de verdade nem o HOME de quem roda.
#
#   ./scripts/test-install-app.sh              # roda todos os casos
#   source scripts/test-install-app.sh --lib   # só define as funções e mantém a pasta (uso real)
set -uo pipefail

LIB_ONLY=0
[ "${1:-}" = --lib ] && LIB_ONLY=1
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(mktemp -d)"
[ "$LIB_ONLY" = 1 ] || trap 'rm -rf "$ROOT"' EXIT
BASH_BIN=$(command -v bash)
fail=0

# Ferramentas reais que o install.sh usa. Fora delas não há tmux, git, sudo, gerenciador de pacotes
# nem tailscale: cada caso põe em $B os falsos que quiser.
SYS_TOOLS="bash sh env cat grep sed tr cut tail head date mkdir rm mv cp mktemp tee dirname basename uname id sleep seq awk find timeout setsid openssl python3 wc sort ls chmod ln touch readlink"

# fake <comando> <corpo sh>: programa falso que registra a chamada em $S/log/calls e roda o corpo.
fake() {
  printf '#!/bin/sh\necho "%s $*" >> "%s/log/calls"\n%s\n' "$1" "$S" "$2" > "$B/$1"
  chmod +x "$B/$1"
}

# new_sandbox <nome>: define S (raiz do caso), R (repositório falso) e B (programas falsos).
new_sandbox() {
  S="$ROOT/$1"; R="$S/repo"; B="$S/bin"
  mkdir -p "$R/backend" "$R/scripts" "$R/frontend" "$S/home" "$B" "$S/sys" "$S/log"
  cp "$REPO/install.sh" "$R/install.sh"
  local t p
  for t in $SYS_TOOLS; do
    p=$(command -v "$t" 2>/dev/null) && [ -x "$p" ] && ln -sf "$p" "$S/sys/$t"
  done
  # Sub-scripts do repositório: só registram que foram chamados.
  for t in install-native.sh install-claude-wrapper.sh services-setup.sh install-hangar-send.sh \
           install-skills-bridge.sh tmux-persist-setup.sh lan-setup.sh install-hangar-panel.sh; do
    printf '#!/bin/sh\necho "$0 $*" >> "%s/log/scripts"\nexit 0\n' "$S" > "$R/scripts/$t"
    chmod +x "$R/scripts/$t"
  done
  fake tmux 'exit 0'
  fake claude 'exit 0'
  fake uv 'exit 0'
  fake hostname 'echo 192.168.0.10'
  fake systemctl 'case "$*" in *is-active*) echo active ;; *list-unit-files*|*" cat "*) exit 1 ;; esac; exit 0'
}

# run_install <saída> [args]: sem terminal (setsid), stdin nulo, teto de tempo; TEST_TOKEN vira
# HANGAR_TOKEN e TEST_ASKPASS vira HANGAR_ASKPASS (vazio = rodado à mão, sem o app). O `| cat` só
# termina quando o tee do log do install.sh fecha a saída.
run_install() {
  local out=$1; shift
  ( cd "$R" && env -i HOME="$S/home" PATH="$B:$S/sys" HANGAR_TOKEN="${TEST_TOKEN-}" \
      HANGAR_ASKPASS="${TEST_ASKPASS-}" HANGAR_ASKPASS_CODE=c0de-do-teste \
      timeout 120 setsid -w "$BASH_BIN" ./install.sh "$@" </dev/null 2>&1 ) | cat > "$out"
  return "${PIPESTATUS[0]}"
}

pass()  { echo "ok   $1"; }
flunk() { echo "FAIL $1"; if [ -n "${2:-}" ] && [ -f "$2" ]; then tail -40 "$2" | sed 's/^/     | /'; fi; fail=1; }
expect_line() { if grep -Eq -- "$3" "$2" 2>/dev/null; then pass "$1"; else flunk "$1 (sem /$3/)" "$2"; fi; }
expect_no()   { if grep -Eq -- "$3" "$2" 2>/dev/null; then flunk "$1 (achou /$3/)" "$2"; else pass "$1"; fi; }
expect_eq()   { if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 (veio '$2', esperado '$3')"; fi; }
last_line()   { grep -av '^[[:space:]]*$' "$1" | tail -1; }
first_mark()  { grep -a -m1 '^##HANGAR-' "$1"; }
steps()       { grep -a '^##HANGAR-PASSO## ' "$1" | cut -d' ' -f2- | paste -sd'|'; }
# before <arquivo> <regex A> <regex B>: a primeira linha com A vem antes da primeira com B
before() {
  local a b
  a=$(grep -nE -m1 -- "$2" "$1" | cut -d: -f1); b=$(grep -nE -m1 -- "$3" "$1" | cut -d: -f1)
  [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}

# sudo falso que aceita tudo, inclusive -n (credencial já guardada), e executa o comando. Sem
# `exec`: o `true` do `sudo -n true` é o builtin do sh (a sandbox não tem /usr/bin/true no PATH).
SUDO_EXEC='while [ $# -gt 0 ]; do case $1 in -n|-S|-v|-k) shift ;; -p) shift 2 ;; *) break ;; esac; done; [ $# -eq 0 ] && exit 0; "$@"'
# sudo falso do --app: começa sem credencial guardada (-n falha); com -S lê a senha da entrada,
# responde como o sudo clássico quando ela vem errada e, com "s3nha-certa", guarda a credencial
# (o -n passa a valer, como no sudo de verdade logo depois de um -S)
SUDO_APP='n=0; s=0; c="$(dirname "$0")/.sudo-cred"
while [ $# -gt 0 ]; do case $1 in -n) n=1; shift ;; -S) s=1; shift ;; -v) shift ;; -p) shift 2 ;; *) break ;; esac; done
if [ $n = 1 ] && [ ! -f "$c" ]; then echo "sudo: a password is required" >&2; exit 1; fi
if [ $s = 1 ]; then
  IFS= read -r pw || pw=
  if [ "$pw" != s3nha-certa ]; then echo "Sorry, try again." >&2; echo "sudo: 1 incorrect password attempt" >&2; exit 1; fi
  : > "$c"
fi
[ $# -eq 0 ] && exit 0
"$@"'
# apt_installs_tmux: apt-get falso que "instala" o tmux falso
apt_installs_tmux() {
  cat > "$B/apt-get" <<EOF
#!/bin/sh
echo "apt-get \$*" >> "$S/log/calls"
printf '#!/bin/sh\nexit 0\n' > "$B/tmux"; chmod +x "$B/tmux"
EOF
  chmod +x "$B/apt-get"
}
# askpass_says <corpo sh>: auxiliar falso do app em $B/askpass; sem o código desta instalação no
# ambiente ele sai 3, como o app que recusa o pedido
askpass_says() {
  fake askpass "[ \"\$HANGAR_ASKPASS_CODE\" = c0de-do-teste ] || exit 3
$1"
}

[ "$LIB_ONLY" = 1 ] && return 0

APP_ARGS=(--app --tailscale=nao --sem-nativo --no-frontend)

# --- Caso: caminho feliz do assistente, e rodar de novo ---
new_sandbox feliz
TEST_TOKEN='segredo-do-app-123' run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
expect_eq "feliz: sai 0" "$rc" 0
expect_eq "feliz: PROTOCOLO é a primeira marca" "$(first_mark "$S/out")" '##HANGAR-PROTOCOLO## 1'
expect_eq "feliz: FIM ok é a última linha" "$(last_line "$S/out")" '##HANGAR-FIM## ok'
expect_eq "feliz: etapas na ordem" "$(steps "$S/out")" \
  'preparar fazendo|preparar ok|instalar fazendo|celular fazendo|celular ok|instalar fazendo|instalar ok|final fazendo|final ok'
expect_line "feliz: senha do app gravada" "$R/backend/.env" '^CP_AUTH_TOKEN=segredo-do-app-123$'
expect_no "feliz: senha fora da saída" "$S/out" 'segredo-do-app-123'
expect_no "feliz: senha fora do log" "$S/home/.hangar/logs/privado/install.log" 'segredo-do-app-123'
expect_no "feliz: nenhuma pergunta ficou sem resposta" "$S/out" 'assumindo NÃO'
expect_line "feliz: ask responde sim (serviços)" "$S/log/scripts" 'services-setup.sh --backend-only'
expect_no "feliz: ask_extra responde não (persistência)" "$S/log/scripts" 'tmux-persist-setup'
expect_no "feliz: --sem-nativo não roda o install-native" "$S/log/scripts" 'install-native'
expect_no "feliz: --tailscale=nao não cria etapa" "$S/out" '^##HANGAR-PASSO## tailscale'
TEST_TOKEN='outra-senha-456' run_install "$S/out2" "${APP_ARGS[@]}"
expect_eq "de novo: FIM ok" "$(last_line "$S/out2")" '##HANGAR-FIM## ok'
expect_eq "de novo: uma linha de senha só" "$(grep -c '^CP_AUTH_TOKEN=' "$R/backend/.env")" 1
expect_line "de novo: a senha atual é mantida" "$R/backend/.env" '^CP_AUTH_TOKEN=segredo-do-app-123$'
expect_no "de novo: a senha nova não vaza" "$S/out2" 'outra-senha-456'

# --- Caso: máquina migrada para o pacote lê e grava os dados em ~/.hangar/config ---
new_sandbox migrada
mkdir -p "$S/home/.hangar/config"
printf 'CP_AUTH_TOKEN=do-pacote-123\nCP_SERVER_ID=casa\nCP_PORT=8799\n' > "$S/home/.hangar/config/.env"
TEST_TOKEN='outra-senha-456' run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
expect_eq "migrada: sai 0" "$rc" 0
expect_eq "migrada: nenhum token novo no backend/.env" "$(cat "$R/backend/.env" 2>/dev/null | grep -c '^CP_AUTH_TOKEN=')" 0
expect_eq "migrada: uma linha de senha só" "$(grep -c '^CP_AUTH_TOKEN=' "$S/home/.hangar/config/.env")" 1
expect_line "migrada: a senha do pacote fica" "$S/home/.hangar/config/.env" '^CP_AUTH_TOKEN=do-pacote-123$'
expect_eq "migrada: nome da máquina não repete" "$(grep -c '^CP_SERVER_ID=' "$S/home/.hangar/config/.env")" 1

# --- Caso: senha com espaço no meio chega ao backend igual ---
new_sandbox especial
TEST_TOKEN='minha senha 123' run_install "$S/out" "${APP_ARGS[@]}"
# O backend lê o .env pelo python-dotenv; sem o venv aqui, confere a linha gravada.
PY="$REPO/backend/.venv/bin/python"
if [ -x "$PY" ] && "$PY" -c 'import dotenv' 2>/dev/null; then
  expect_eq "especial: o backend lê igual" \
    "$("$PY" -c 'import sys; from dotenv import dotenv_values; print(dotenv_values(sys.argv[1])["CP_AUTH_TOKEN"])' "$R/backend/.env")" 'minha senha 123'
else
  expect_eq "especial: gravada literal" "$(grep '^CP_AUTH_TOKEN=' "$R/backend/.env")" 'CP_AUTH_TOKEN=minha senha 123'
fi

# --- Caso: senha que o .env não guarda inteira é recusada ---
for t in 'abc #def' 'x${HOME}y' '"com-aspas"' ' espaco-inicial' 'minha senha ção'; do
  new_sandbox recusada
  TEST_TOKEN=$t run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
  expect_eq "recusada [$t]: falha" "$rc" 1
  expect_line "recusada [$t]: diz quais caracteres" "$S/out" '^##HANGAR-FALHA## .*ASCII sem acento.*nem espaço no começo ou no fim$'
  expect_eq "recusada [$t]: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'
  if grep -qF -- "$t" "$S/out"; then flunk "recusada [$t]: senha na saída" "$S/out"; else pass "recusada [$t]: senha fora da saída"; fi
  rm -rf "$S"
done

# --- Caso: sem senha do app, gera uma aleatória e não mostra ---
new_sandbox aleatoria
TEST_TOKEN='' run_install "$S/out" "${APP_ARGS[@]}"
tok=$(grep '^CP_AUTH_TOKEN=' "$R/backend/.env" | cut -d= -f2-)
expect_eq "aleatória: 48 caracteres" "${#tok}" 48
expect_no "aleatória: fora da saída" "$S/out" "${tok:-nada-gravado}"

# --- Caso: senha curta do app para a instalação ---
new_sandbox curta
TEST_TOKEN='1234' run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
expect_eq "curta: falha" "$rc" 1
expect_eq "curta: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Caso: --tailscale=nao vale mesmo com a Tailscale instalada ---
new_sandbox ts-nao
fake tailscale 'exit 0'
run_install "$S/out" "${APP_ARGS[@]}"
expect_no "ts-nao: tailscale nunca é chamada" "$S/log/calls" '^tailscale '
expect_no "ts-nao: sem etapa tailscale" "$S/out" '^##HANGAR-PASSO## tailscale'

# --- Caso: falha essencial termina com PASSO falhou e FIM falhou ---
new_sandbox uv-falha
fake uv '[ "$1" = sync ] && exit 1; exit 0'
run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
expect_eq "uv-falha: sai com erro" "$rc" 1
expect_line "uv-falha: FALHA com o texto de sempre" "$S/out" '^##HANGAR-FALHA## uv sync falhou — o backend ficou sem as dependências$'
expect_line "uv-falha: etapa instalar falhou" "$S/out" '^##HANGAR-PASSO## instalar falhou$'
expect_eq "uv-falha: FIM falhou é a última linha" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Caso: pendência termina com FIM pendente (e sai 1, como hoje) ---
new_sandbox pendente
fake systemctl 'case "$*" in *is-active*) echo inactive ;; *list-unit-files*|*" cat "*) exit 1 ;; esac; exit 0'
run_install "$S/out" "${APP_ARGS[@]}"; rc=$?
expect_eq "pendente: sai 1" "$rc" 1
expect_line "pendente: etapa final pendente" "$S/out" '^##HANGAR-PASSO## final pendente$'
expect_eq "pendente: FIM pendente" "$(last_line "$S/out")" '##HANGAR-FIM## pendente'

# --- Caso: --check --app só confere ---
new_sandbox check
run_install "$S/out" --check --app --no-frontend; rc=$?
expect_eq "check: sai 0" "$rc" 0
expect_eq "check: PROTOCOLO primeiro" "$(first_mark "$S/out")" '##HANGAR-PROTOCOLO## 1'
expect_eq "check: etapas" "$(steps "$S/out")" 'preparar fazendo|preparar ok'
expect_eq "check: FIM ok" "$(last_line "$S/out")" '##HANGAR-FIM## ok'
if [ ! -e "$R/backend/.env" ] && [ ! -e "$S/home/.hangar" ]; then pass "check: nada escrito"; else flunk "check: nada escrito"; fi

# --- Caso: sem --app a saída não ganha marcas do assistente ---
new_sandbox terminal
run_install "$S/out" --check --no-frontend
expect_no "terminal: sem marcas do assistente" "$S/out" '^##HANGAR-(PROTOCOLO|PASSO|ITEM|LINK|PENDENCIA|FIM)'

# --- Caso: --tailscale com valor inválido ---
new_sandbox ts-invalido
run_install "$S/out" --app --tailscale=talvez; rc=$?
expect_eq "ts-inválido: recusa" "$rc" 1
expect_line "ts-inválido: diz o que aceita" "$S/out" '--tailscale aceita sim ou nao'

# --- Caso: --check --app lista os itens ---
new_sandbox itens
rm -f "$B/tmux"; fake apt-get 'exit 0'
run_install "$S/out" --check --app --no-frontend
expect_line "itens: tmux na fila" "$S/out" '^##HANGAR-ITEM## tmux fila tmux$'
expect_line "itens: Claude encontrado" "$S/out" '^##HANGAR-ITEM## claude ok Claude Code$'
expect_line "itens: uv encontrado" "$S/out" '^##HANGAR-ITEM## uv ok uv$'
expect_line "itens: git opcional pendente" "$S/out" '^##HANGAR-ITEM## git pendente git$'
expect_eq "itens: preparar pendente" "$(steps "$S/out")" 'preparar fazendo|preparar pendente'
expect_eq "itens: FIM pendente" "$(last_line "$S/out")" '##HANGAR-FIM## pendente'

# --- Caso: itens da instalação no caminho feliz ---
new_sandbox itens-feliz
run_install "$S/out" "${APP_ARGS[@]}"
for i in 'backend ok' 'token ok' 'nativo ok' 'rust ok' 'wrappers ok' 'rede-local ok' 'servicos ok' 'hangar-send ok' 'backend-importa ok' 'multiplexador ok'; do
  expect_line "itens-feliz: $i" "$S/out" "^##HANGAR-ITEM## $i "
done

# --- Caso: nenhum agente ficou instalado ---
new_sandbox sem-agente
rm -f "$B/claude"
run_install "$S/out" --app --agentes=codex --tailscale=nao --sem-nativo --no-frontend
expect_line "sem-agente: item do agente na fila" "$S/out" '^##HANGAR-ITEM## codex fila Codex$'
expect_line "sem-agente: pendência do agente" "$S/out" '^##HANGAR-PENDENCIA## agente-nao-instalou Codex instalou'
if before "$S/out" '^##HANGAR-ERRO## sem-agente$' '^##HANGAR-FALHA## nenhum agente'; then pass "sem-agente: ERRO antes da FALHA"; else flunk "sem-agente: ERRO antes da FALHA" "$S/out"; fi
expect_eq "sem-agente: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Caso: Claude Code não instalou e era o único agente ---
new_sandbox claude-falhou
rm -f "$B/claude"; fake curl 'exit 22'
run_install "$S/out" "${APP_ARGS[@]}"
expect_line "claude-falhou: item falhou" "$S/out" '^##HANGAR-ITEM## claude falhou Claude Code$'
if before "$S/out" '^##HANGAR-ERRO## agente-nao-instalou$' '^##HANGAR-FALHA## faltam: Claude Code'; then pass "claude-falhou: ERRO antes da FALHA"; else flunk "claude-falhou: ERRO antes da FALHA" "$S/out"; fi

# --- Caso: sem internet no uv sync ---
new_sandbox sem-internet
fake uv '[ "$1" = sync ] && exit 1; exit 0'
fake curl 'exit 7'
run_install "$S/out" "${APP_ARGS[@]}"
if before "$S/out" '^##HANGAR-ERRO## sem-internet$' '^##HANGAR-FALHA## uv sync falhou'; then pass "sem-internet: ERRO antes da FALHA"; else flunk "sem-internet: ERRO antes da FALHA" "$S/out"; fi

# --- Caso: uv sync falha com a internet no ar: nenhum código inventado ---
new_sandbox uv-com-rede
fake uv '[ "$1" = sync ] && exit 1; exit 0'
fake curl 'exit 0'
run_install "$S/out" "${APP_ARGS[@]}"
expect_no "uv-com-rede: sem código" "$S/out" '^##HANGAR-ERRO##'

# --- Caso: o código de erro também sai no terminal ---
new_sandbox erro-terminal
fake uv '[ "$1" = sync ] && exit 1; exit 0'
fake curl 'exit 7'
run_install "$S/out" --yes --tailscale=nao --no-frontend
expect_line "erro-terminal: código sem --app" "$S/out" '^##HANGAR-ERRO## sem-internet$'
expect_no "erro-terminal: sem FIM fora do --app" "$S/out" '^##HANGAR-FIM##'

# --- Caso: sem systemd de usuário o assistente não termina ---
new_sandbox sem-systemd
rm -f "$B/systemctl"
run_install "$S/out" "${APP_ARGS[@]}"
expect_line "sem-systemd: código" "$S/out" '^##HANGAR-ERRO## sem-systemd$'
expect_eq "sem-systemd: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Caso: no --app a senha vai do auxiliar do app ao sudo -S ---
new_sandbox sudo-app
rm -f "$B/tmux"; apt_installs_tmux; fake sudo "$SUDO_APP"; askpass_says 'echo s3nha-certa'
fake uv "env | grep '^HANGAR_ASKPASS' >> '$S/log/vazou'; exit 0"
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_line "sudo-app: tenta a credencial guardada antes" "$S/log/calls" '^sudo -n true$'
expect_line "sudo-app: o auxiliar recebe o motivo" "$S/log/calls" '^askpass instalar o tmux$'
expect_line "sudo-app: a senha só autentica" "$S/log/calls" '^sudo -S -p +-v$'
expect_line "sudo-app: pacote pelo sudo -n" "$S/log/calls" '^sudo -n apt-get install -y tmux$'
expect_line "sudo-app: tmux instalado" "$S/out" '^##HANGAR-ITEM## tmux ok tmux$'
expect_no "sudo-app: a senha fica fora da saída" "$S/out" 's3nha-certa'
expect_no "sudo-app: auxiliar e código fora do ambiente dos outros programas" "$S/log/vazou" 'HANGAR_ASKPASS'
expect_eq "sudo-app: FIM ok" "$(last_line "$S/out")" '##HANGAR-FIM## ok'

# --- Caso: senha errada → o auxiliar é chamado de novo com --retry ---
new_sandbox senha-errada
rm -f "$B/tmux"; apt_installs_tmux; fake sudo "$SUDO_APP"
askpass_says 'case "$*" in *--retry*) echo s3nha-certa ;; *) echo errada ;; esac'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_line "senha-errada: pede de novo marcando a recusa" "$S/log/calls" '^askpass instalar o tmux --retry$'
expect_line "senha-errada: tmux instalado na segunda" "$S/out" '^##HANGAR-ITEM## tmux ok tmux$'

# --- Caso: três senhas erradas ---
new_sandbox tres-erradas
rm -f "$B/tmux"; fake apt-get 'exit 0'; fake sudo "$SUDO_APP"; askpass_says 'echo errada'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_eq "tres-erradas: três pedidos" "$(grep -c '^askpass ' "$S/log/calls")" 3
if before "$S/out" '^##HANGAR-ERRO## senha-cancelada$' '^##HANGAR-FALHA## faltam: tmux'; then pass "tres-erradas: ERRO antes da FALHA"; else flunk "tres-erradas: ERRO antes da FALHA" "$S/out"; fi

# --- Caso: a pessoa fechou a janela de senha ---
new_sandbox senha-cancelada
rm -f "$B/tmux"; fake apt-get 'exit 0'; fake sudo "$SUDO_APP"; askpass_says 'exit 1'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_eq "senha-cancelada: um pedido só" "$(grep -c '^askpass ' "$S/log/calls")" 1
expect_line "senha-cancelada: código" "$S/out" '^##HANGAR-ERRO## senha-cancelada$'

# --- Caso: usuário sem permissão de sudo ---
new_sandbox sem-sudo
rm -f "$B/tmux"; fake apt-get 'exit 0'; askpass_says 'echo s3nha-certa'
fake sudo '[ "$1" = -n ] && exit 1; echo "maria is not in the sudoers file." >&2; exit 1'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_eq "sem-sudo: não pede a senha de novo" "$(grep -c '^askpass ' "$S/log/calls")" 1
if before "$S/out" '^##HANGAR-ERRO## sem-sudo$' '^##HANGAR-FALHA## faltam: tmux'; then pass "sem-sudo: ERRO antes da FALHA"; else flunk "sem-sudo: ERRO antes da FALHA" "$S/out"; fi

# --- Caso: lista de pacotes velha ---
new_sandbox pacotes
rm -f "$B/tmux"; fake sudo "$SUDO_APP"; askpass_says 'echo s3nha-certa'
fake apt-get 'echo "E: Unable to locate package tmux" >&2; exit 100'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_line "pacotes: código" "$S/out" '^##HANGAR-ERRO## pacotes-desatualizados$'

# --- Caso: o comando autenticado imprime "Authentication failed": não é senha errada ---
new_sandbox auth-do-comando
rm -f "$B/tmux"; fake sudo "$SUDO_APP"; askpass_says 'echo s3nha-certa'
fake apt-get 'echo "E: Authentication failed for repo mirror" >&2; exit 100'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_eq "auth-do-comando: um pedido de senha" "$(grep -c '^askpass ' "$S/log/calls")" 1
expect_eq "auth-do-comando: o comando roda uma vez" "$(grep -c '^apt-get ' "$S/log/calls")" 1
expect_no "auth-do-comando: não vira senha cancelada" "$S/out" '^##HANGAR-ERRO## senha-cancelada$'

# --- Caso: regra NOPASSWD para o comando: a senha não chega à entrada dele ---
new_sandbox nopasswd
rm -f "$B/tmux"; askpass_says 'echo s3nha-certa'
# sudo falso com NOPASSWD para tudo menos o `true`: o comando roda sem ler a senha, mesmo com -S
fake sudo 'v=0; c="$(dirname "$0")/.sudo-cred"
while [ $# -gt 0 ]; do case $1 in -n|-S) shift ;; -v) v=1; shift ;; -p) shift 2 ;; *) break ;; esac; done
if [ $v = 1 ]; then IFS= read -r pw || pw=; [ "$pw" = s3nha-certa ] || { echo "Sorry, try again." >&2; exit 1; }; : > "$c"; exit 0; fi
if [ "$1" = true ] && [ ! -f "$c" ]; then echo "sudo: a password is required" >&2; exit 1; fi
"$@"'
cat > "$B/apt-get" <<EOF
#!/bin/sh
echo "apt-get \$*" >> "$S/log/calls"
cat > "$S/log/stdin"
printf '#!/bin/sh\nexit 0\n' > "$B/tmux"; chmod +x "$B/tmux"
EOF
chmod +x "$B/apt-get"
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_line "nopasswd: tmux instalado" "$S/out" '^##HANGAR-ITEM## tmux ok tmux$'
if [ -f "$S/log/stdin" ] && [ ! -s "$S/log/stdin" ]; then pass "nopasswd: o comando não recebe a senha"; else flunk "nopasswd: o comando não recebe a senha" "$S/log/stdin"; fi

# --- Caso: sudo que aceita a senha mas não a guarda: não manda a senha ao comando ---
new_sandbox sem-cache
rm -f "$B/tmux"; fake apt-get 'exit 0'; askpass_says 'echo s3nha-certa'
fake sudo 'if [ "$1" = -n ]; then echo "sudo: a password is required" >&2; exit 1; fi
IFS= read -r pw || pw=; [ "$pw" = s3nha-certa ] || exit 1; [ "$4" = -v ] && exit 0; shift 3; "$@"'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_no "sem-cache: o comando não roda" "$S/log/calls" '^apt-get '
expect_line "sem-cache: diz a causa" "$S/out" 'não a guarda entre comandos'
if before "$S/out" '^##HANGAR-ERRO## sem-sudo$' '^##HANGAR-FALHA## faltam: tmux'; then pass "sem-cache: ERRO antes da FALHA"; else flunk "sem-cache: ERRO antes da FALHA" "$S/out"; fi

# --- Caso: --app rodado à mão, sem HANGAR_ASKPASS: não é senha cancelada ---
new_sandbox sem-askpass
rm -f "$B/tmux"; fake apt-get 'exit 0'; fake sudo "$SUDO_APP"
run_install "$S/out" "${APP_ARGS[@]}"
expect_no "sem-askpass: o sudo -S nem é chamado" "$S/log/calls" '^sudo -S'
expect_no "sem-askpass: nenhum código de senha" "$S/out" '^##HANGAR-ERRO## (senha-cancelada|sem-sudo)'
expect_eq "sem-askpass: FIM falhou" "$(last_line "$S/out")" '##HANGAR-FIM## falhou'

# --- Caso: credencial já guardada: o auxiliar nem é chamado ---
new_sandbox credencial
rm -f "$B/tmux"; apt_installs_tmux; fake sudo "$SUDO_EXEC"; askpass_says 'echo s3nha-certa'
TEST_ASKPASS="$B/askpass" run_install "$S/out" "${APP_ARGS[@]}"
expect_line "credencial: o pacote vai pelo sudo -n" "$S/log/calls" '^sudo -n apt-get install -y tmux$'
expect_no "credencial: sem pedido de senha" "$S/log/calls" '^askpass '

# --- Caso: fora do --app o sudo é o de sempre ---
new_sandbox sudo-terminal
rm -f "$B/tmux"; apt_installs_tmux; fake sudo "$SUDO_EXEC"; askpass_says 'echo s3nha-certa'
TEST_ASKPASS="$B/askpass" run_install "$S/out" --yes --tailscale=nao --no-frontend
expect_line "sudo-terminal: sudo direto" "$S/log/calls" '^sudo apt-get install -y tmux$'
expect_no "sudo-terminal: sem auxiliar" "$S/log/calls" '^askpass '

# --- Caso: Tailscale — login pela app_sudo, link do login e HTTPS desligado ---
new_sandbox tailscale
fake sudo "$SUDO_APP"; askpass_says 'echo s3nha-certa'
cat > "$B/tailscale" <<EOF
#!/bin/sh
echo "tailscale \$*" >> "$S/log/calls"
case "\$1" in
  status)
    if [ "\$2" = --json ]; then echo '{"Self":{"DNSName":"pc.tail1.ts.net."}}'; exit 0; fi
    [ -f "$S/ts-logado" ] && exit 0
    exit 1 ;;
  up)
    echo 'To authenticate, visit:' >&2
    printf '\n\thttps://login.tailscale.com/a/abc123\n\n' >&2
    touch "$S/ts-logado"; exit 0 ;;
  serve)
    echo 'Serve is not enabled on your tailnet. HTTPS certificates are disabled.' >&2
    exit 1 ;;
esac
exit 0
EOF
chmod +x "$B/tailscale"
TEST_ASKPASS="$B/askpass" run_install "$S/out" --app --tailscale=sim --sem-nativo --no-frontend
expect_line "tailscale: link do login" "$S/out" '^##HANGAR-LINK## tailscale-login https://login\.tailscale\.com/a/abc123$'
expect_line "tailscale: motivo do login" "$S/log/calls" '^askpass entrar na conta Tailscale$'
expect_line "tailscale: login pelo sudo -n" "$S/log/calls" '^sudo -n timeout 300 tailscale up$'
expect_line "tailscale: conta conectada" "$S/out" '^##HANGAR-ITEM## tailscale-conta ok '
expect_line "tailscale: HTTPS vira pendência" "$S/out" '^##HANGAR-PENDENCIA## tailscale-https '
expect_line "tailscale: etapa pendente" "$S/out" '^##HANGAR-PASSO## tailscale pendente$'
expect_eq "tailscale: FIM pendente" "$(last_line "$S/out")" '##HANGAR-FIM## pendente'

# --- Caso: o script da Tailscale roda inteiro como administrador ---
new_sandbox ts-instala
fake sudo "$SUDO_APP"; askpass_says 'echo s3nha-certa'
fake curl 'echo "echo instalador-da-tailscale"'
TEST_ASKPASS="$B/askpass" run_install "$S/out" --app --tailscale=sim --sem-nativo --no-frontend
expect_line "ts-instala: sh -c pelo sudo -n" "$S/log/calls" '^sudo -n sh -c curl -fsSL https://tailscale\.com/install\.sh \| sh$'
expect_line "ts-instala: o script rodou" "$S/out" '^instalador-da-tailscale$'
expect_line "ts-instala: sem o comando, item falhou" "$S/out" '^##HANGAR-ITEM## tailscale falhou Tailscale$'

# --- fim dos casos ---
if [ "$fail" = 0 ]; then echo "tudo ok"; else echo "houve falha"; fi
exit "$fail"
