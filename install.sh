#!/usr/bin/env bash
# hangar — instalação completa no Linux/macOS.
#
#   ./install.sh              # interativo
#   ./install.sh --yes        # aceita tudo (não pergunta nada)
#   ./install.sh --check      # só diz o que falta e sai, sem instalar nada
#   ./install.sh --avancado   # pergunta cada extra (o padrão instala o recomendado sem perguntar)
#   ./install.sh --agentes=codex,pi   # agentes de código a instalar (claude, codex, pi, omp, kimi)
#   ./install.sh --update        # re-aplica só o que um `git pull` não atualiza sozinho
#   ./install.sh --no-frontend   # só o backend (o PWA já roda noutro lugar)
#   ./install.sh --no-wrapper --no-services --no-hangar-send --no-panel   # pula partes
#   ./install.sh --app --tailscale=sim|nao --sem-nativo   # assistente do app nativo: sem perguntas, marcas ##HANGAR-*
#
# Os sub-scripts (services-setup.sh, lan-setup.sh, install-hangar-send.sh, ...) continuam
# rodáveis sozinhos; este aqui só faz com que um comando baste.
set -euo pipefail
cd "$(dirname "$0")"
REPO=$(pwd)

YES=0; CHECK=0; UPDATE=0; WRAPPER=1; SERVICES=1; CPSEND=1; PANEL=1; FRONTEND=1; AVANCADO=0; AGENTES=''
APP=0; TAILSCALE=''; SEM_NATIVO=0
for arg in "$@"; do
  case "$arg" in
    --yes|-y)      YES=1 ;;
    --check)       CHECK=1 ;;
    --avancado)    AVANCADO=1 ;;
    --agentes=*)   AGENTES=${arg#--agentes=} ;;
    # --app: modo do assistente do app nativo. A pessoa já respondeu nas telas dele.
    --app)         APP=1 ;;
    --tailscale=sim|--tailscale=nao) TAILSCALE=${arg#--tailscale=} ;;
    --tailscale=*) echo "--tailscale aceita sim ou nao"; exit 1 ;;
    --sem-nativo)  SEM_NATIVO=1 ;;
    # --update: modo do hook post-merge. Re-aplica o que o `git pull` NÃO atualiza (units com
    # caminho cravado, o bloco de protocolo no ~/.claude/CLAUDE.md, deps do backend, build do
    # front) e NÃO toca em nada que peça senha ou decisão: sem instalar dependência, sem token,
    # sem firewall, sem Tailscale. Um hook que para pedindo sudo no meio de um pull é pior que
    # hook nenhum.
    --update)      UPDATE=1; YES=1 ;;
    --no-wrapper)  WRAPPER=0 ;;
    --no-services) SERVICES=0 ;;
    # O nome antigo do flag segue aceito: está no histórico de comando de quem já instala assim.
    --no-hangar-send|--no-cp-send)  CPSEND=0 ;;
    --no-panel)    PANEL=0 ;;
    --no-frontend) FRONTEND=0 ;;
    *) echo "flag desconhecida: $arg"; exit 1 ;;
  esac
done

# O Hangar pilota agentes de código e precisa de pelo menos um; o Claude Code é o padrão, não o único.
TODOS_AGENTES="claude codex pi omp kimi"
nome_agente() {
  case $1 in claude) echo "Claude Code" ;; codex) echo "Codex" ;; pi) echo "Pi" ;; omp) echo "omp" ;; kimi) echo "Kimi Code" ;; esac
}
agentes_validos() { # agentes_validos <lista separada por vírgula>: 0 = todos conhecidos e não vazia
  local a lista=${1//,/ }
  [ -n "${lista// /}" ] || return 1
  for a in $lista; do case " $TODOS_AGENTES " in *" $a "*) ;; *) return 1 ;; esac; done
}
if [ -n "$AGENTES" ] && ! agentes_validos "$AGENTES"; then
  echo "--agentes aceita, separados por vírgula: ${TODOS_AGENTES// /, }"; exit 1
fi

say() {
  # Título numerado ("3/8 ...") ganha a barra de progresso; os demais seguem só em negrito.
  local t="$*"
  if [[ $t =~ ^([0-9]+)/8\  ]]; then
    local n=${BASH_REMATCH[1]}
    printf '\n  \033[1;36m[%s%s] %s\033[0m\n' \
      "$(printf '%*s' "$n" '' | tr ' ' '#')" "$(printf '%*s' "$((8-n))" '' | tr ' ' '-')" "$t"
  else
    printf '\n\033[1m%s\033[0m\n' "$t"
  fi
}
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
nota() { printf '      \033[2m%s\033[0m\n' "$*"; }
falta(){ printf '  \033[33m--\033[0m  %s\n' "$*"; }
erro() { printf '  \033[31mX\033[0m   %s\n' "$*"; }
# A marca é o que a tela do Atualizar mostra como motivo (o app só vê as últimas linhas).
fail() { erro "$*"; echo "##HANGAR-FALHA## $*"; exit 1; }

# Marcas do assistente do app nativo; contrato na spec
# docs/superpowers/specs/2026-10-06-instalador-grafico-design.md. Só saem no --app.
HANGAR_PROTOCOL=1
CURRENT_STEP=''
FINAL_STATE=falhou
mark_step() {
  [ "$APP" = 1 ] || return 0
  [ "$2" = fazendo ] && CURRENT_STEP=$1
  echo "##HANGAR-PASSO## $1 $2"
}
# Última linha no --app: sem ela o app sabe que a instalação foi interrompida.
finish_app() {
  [ "$FINAL_STATE" = falhou ] && [ -n "$CURRENT_STEP" ] && echo "##HANGAR-PASSO## $CURRENT_STEP falhou"
  echo "##HANGAR-FIM## $FINAL_STATE"
}
if [ "$APP" = 1 ]; then
  echo "##HANGAR-PROTOCOLO## $HANGAR_PROTOCOL"
  trap finish_app EXIT
fi
# A senha do celular chega pelo ambiente, nunca pelo argv (que o `ps` mostra), e sai dele já aqui
# para nenhum instalador de terceiro chamado depois herdá-la.
APP_TOKEN=''
[ "$APP" = 1 ] && APP_TOKEN=${HANGAR_TOKEN-}
unset HANGAR_TOKEN
# O auxiliar da senha de administrador (posto pelo app) e o código dele também saem do ambiente:
# só a app_sudo os repassa, e só ao próprio auxiliar.
APP_ASKPASS=${HANGAR_ASKPASS-}
APP_ASKPASS_CODE=${HANGAR_ASKPASS_CODE-}
unset HANGAR_ASKPASS HANGAR_ASKPASS_CODE
mark_item() { [ "$APP" = 1 ] || return 0; echo "##HANGAR-ITEM## $1 $2 $3"; }
# Item de uma parte inteira: pendente quando ela somou problema desde a contagem `antes`.
mark_item_since() { # mark_item_since <id> <antes> <texto>
  local st=ok
  [ "${#PROBLEMAS[@]}" -gt "$2" ] && st=pendente
  mark_item "$1" "$st" "$3"
}
# O código vem logo antes da falha, também no terminal: o app troca o texto pela frase e pelo botão.
fail_with() { # fail_with <código ou vazio> <mensagem>
  [ -n "$1" ] && echo "##HANGAR-ERRO## $1"
  shift; fail "$*"
}
# Vazio quando a internet responde: aí a falha é de outra coisa.
net_code() {
  command -v curl >/dev/null || return 0
  curl -fsS --max-time 10 -o /dev/null https://github.com 2>/dev/null || echo sem-internet
}
# Textos que dizem a causa de uma falha com sudo: sudo clássico, sudo-rs e apt, medidos em
# docs/decisoes/instalacao.md ("O `sudo` sem terminal: `sudo -S`, não `SUDO_ASKPASS`").
SUDO_DENIED_RE='not in the sudoers|not allowed to (run|execute)|may not run sudo|afraid I can.t do that'
# Só linhas do próprio sudo: um comando que imprime "Authentication failed" não é senha errada.
SUDO_WRONG_RE='^Sorry, try again|^sudo(-rs)?: .*(incorrect (password|authentication) attempt|Authentication failed)'
PKG_STALE_RE='Unable to locate package|Unable to fetch some archives|Failed to fetch'
# Escrita pela app_sudo quando a pessoa não deu a senha (fechou a janela ou errou três vezes).
SUDO_NO_PASSWORD='hangar: a senha de administrador não foi informada'
# Escrita pela app_sudo quando o sudo aceita a senha mas não a guarda para o comando seguinte.
SUDO_NO_CACHE='hangar: o sudo aceitou a senha mas não a guarda entre comandos (timestamp_timeout=0)'

# Todo sudo do instalador passa por aqui. No --app a senha vem da janela do app: o auxiliar do
# HANGAR_ASKPASS a imprime e ela vai pelo cano direto ao `sudo -S`, nunca para variável, argv ou
# saída (o sudo-rs do Ubuntu não aceita -A nem lê SUDO_ASKPASS). Fora do --app, o sudo de sempre.
app_sudo() { # app_sudo <motivo> <comando…>
  local reason=$1 retry='' err try
  local -a st
  shift
  [ "$APP" = 1 ] || { sudo "$@"; return; }
  # Credencial que o sudo guardou num pedido anterior deste processo: não pergunta de novo.
  if sudo -n true 2>/dev/null; then sudo -n "$@"; return; fi
  # --app rodado à mão: sem o app ninguém dá a senha, e isso não é senha cancelada nem falta de sudo.
  if [ ! -x "$APP_ASKPASS" ]; then
    echo "hangar: sem HANGAR_ASKPASS, o --app não tem como pedir a senha de administrador" >&2
    return 1
  fi
  err=$(mktemp)
  # Sudo recusado é resposta, não erro do instalador: sem isto o `set -e` sairia antes de ler a causa
  # quando a app_sudo roda dentro de um cano (subshell). O `local -` devolve o -e na saída.
  local -; set +e
  # A senha só autentica (-v); o comando roda depois pelo -n. Com `sudo -S <comando>` uma regra
  # NOPASSWD deixaria a senha na entrada do comando, e a saída dele se confundiria com a do sudo.
  for try in 1 2 3; do
    # A resposta do sudo vai à saída e ao arquivo: é dela que sai a causa da falha.
    { HANGAR_ASKPASS_CODE=$APP_ASKPASS_CODE "$APP_ASKPASS" "$reason" ${retry:+"$retry"} \
        | sudo -S -p '' -v 2>&1 | tee "$err" >&2; st=("${PIPESTATUS[@]}"); }
    [ "${st[1]}" = 0 ] && break
    # O auxiliar saiu sem senha: a pessoa fechou a janela.
    [ "${st[0]}" = 0 ] || { echo "$SUDO_NO_PASSWORD" >&2; break; }
    grep -qiE "$SUDO_DENIED_RE" "$err" && break
    # Só senha recusada pede de novo; o --retry faz o app descartar a guardada.
    grep -qiE "$SUDO_WRONG_RE" "$err" || break
    [ "$try" = 3 ] && echo "$SUDO_NO_PASSWORD" >&2
    retry=--retry
  done
  rm -f "$err"
  [ "${st[1]}" = 0 ] || return "${st[1]}"
  # Sem credencial guardada, a única via seria a senha no stdin do comando: para aqui.
  sudo -n true 2>/dev/null || { echo "$SUDO_NO_CACHE" >&2; return 1; }
  sudo -n "$@"
}
# Causa de uma falha com sudo, pela saída do comando; vazio = desconhecida.
sudo_error_code() { # sudo_error_code <arquivo com a saída>
  # Sudo que não guarda a credencial também não deixa este usuário instalar pelo app.
  if grep -qiE "$SUDO_DENIED_RE" "$1" || grep -qF "$SUDO_NO_CACHE" "$1"; then
    echo sem-sudo
  elif grep -qF "$SUDO_NO_PASSWORD" "$1"; then
    echo senha-cancelada
  elif grep -qiE "$PKG_STALE_RE" "$1"; then
    echo pacotes-desatualizados
  elif grep -qiE "$SUDO_WRONG_RE" "$1"; then
    echo senha-cancelada
  fi
}

# Duas gravidades, e a diferença é o que acontece com os passos seguintes:
#  - ESSENCIAL falhou -> para na hora (fail): backend, token e frontend sustentam todos os
#    passos seguintes, e seguir adiante só enterrava a causa;
#  - EXTRA que a pessoa PEDIU falhou -> anota_problema: o app funciona sem ele, mas a falha
#    entra na lista do fim, que diz "terminou com pendências" em vez de "Pronto". Falha que só
#    imprime amarelo e some foi como instalações inteiras saíram com o Tailscale sem publicar
#    e ninguém soube na hora.
PROBLEMAS=()
# No --app cada problema vira pendência com código; sem código conhecido, `outro`.
anota_problema() { # anota_problema <texto> [código]
  erro "$1"; PROBLEMAS+=("$1")
  [ "$APP" = 1 ] && echo "##HANGAR-PENDENCIA## ${2:-outro} $1"
  return 0
}

gira() { # gira <rótulo> <comando...>: spinner enquanto roda; sem TTY (ou --update), saída direta
  local rotulo=$1; shift
  if [ "$TEM_TTY" = 0 ] || [ "$UPDATE" = 1 ]; then "$@"; return $?; fi
  local tmp; tmp=$(mktemp)
  "$@" >"$tmp" 2>&1 &
  local pid=$! quadros='-\|/' i=0
  while kill -0 "$pid" 2>/dev/null; do
    printf '\r  %s %s ' "${quadros:$i:1}" "$rotulo"
    i=$(( (i+1) % 4 ))
    sleep 0.2
  done
  local rc=0; wait "$pid" || rc=$?
  printf '\r\033[K'
  if [ "$rc" != 0 ]; then cat "$tmp"; fi   # falhou: devolve a saída que o spinner escondeu
  rm -f "$tmp"
  return "$rc"
}

# Toda pergunta lê do TERMINAL, nunca do stdin do script. Sob `curl … | bash` (e sob o
# bootstrap.sh) o stdin é o cano do curl: um `read` normal recebia EOF na hora, devolvia string
# vazia — e vazio aqui vale como "sim". O instalador aceitaria firewall, serviços, Tailscale e
# hangar-send sem ninguém ter respondido nada. Sem terminal (CI, cron), a resposta é NÃO.
# --app: quem responde é o app; o terminal de onde ele foi aberto nunca é lido.
if [ "$APP" = 1 ]; then TEM_TTY=0
elif { exec 3</dev/tty; } 2>/dev/null; then TEM_TTY=1; else TEM_TTY=0; fi

# Log em arquivo, nunca no --update: o app lê a saída CRUA pra pegar ##HANGAR-AVISO##,
# e o `tee` quebraria esse parse. E nunca no --check: ele promete não escrever nada no disco,
# e criar o próprio log já era escrita.
LOG="$HOME/.hangar/logs/privado/install.log"
if [ "$UPDATE" = 0 ] && [ "$CHECK" = 0 ]; then
  mkdir -p "$(dirname "$LOG")"
  printf '\n===== %s  %s =====\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$0 $*" >> "$LOG"
  exec > >(tee -a "$LOG") 2>&1
fi

# Quatro sabores, e a diferença é quem decide:
#  ask       -> padrão SIM sem perguntar (--avancado pergunta). Sem terminal: NÃO.
#  ask_senha -> vai pedir sudo: SEMPRE pergunta (com terminal), mesmo no padrão. --yes: sim.
#  ask_extra -> extra de terceiro/ambiente: padrão NÃO; só --avancado pergunta. --yes: sim.
_pergunta() {
  if [ "$TEM_TTY" = 0 ]; then
    printf '  \033[2m%s [S/n] -> sem terminal para responder, assumindo NÃO\033[0m\n' "$1"
    return 1
  fi
  local r=''
  printf '  %s [S/n] ' "$1"
  read -r r <&3 || r=''
  [ -z "$r" ] || [[ "$r" =~ ^[SsYy] ]]
}
# --app responde pela tabela do assistente: ask e ask_senha sim (a senha de administrador vem da
# janela do app), ask_extra não. É a exceção declarada à regra "sem terminal, tudo é NÃO".
ask() {
  [ "$APP" = 1 ] && { nota "$1 -> sim (assistente)"; return 0; }
  [ "$YES" = 1 ] && return 0
  [ "$TEM_TTY" = 0 ] && { _pergunta "$1"; return; }
  [ "$AVANCADO" = 0 ] && { nota "$1 -> sim (padrão; --avancado pergunta)"; return 0; }
  _pergunta "$1"
}
ask_senha() { [ "$APP" = 1 ] && { nota "$1 -> sim (assistente)"; return 0; }; [ "$YES" = 1 ] && return 0; _pergunta "$1"; }
ask_extra() { [ "$APP" = 1 ] && return 1; [ "$YES" = 1 ] && return 0; [ "$AVANCADO" = 0 ] && return 1; _pergunta "$1"; }

PENDENTE=()
DEPS_ERROR=''

# Gerenciador de pacotes do sistema, pro único dep que precisa de root (tmux).
detecta_pkg() {
  for p in pacman apt-get dnf zypper apk brew; do
    command -v "$p" >/dev/null || continue
    case "$p" in
      # No --app ninguém responde ao [S/n] do pacman.
      pacman)  if [ "$APP" = 1 ]; then echo "sudo pacman -S --needed --noconfirm"; else echo "sudo pacman -S --needed"; fi ;;
      apt-get) echo "sudo apt-get install -y" ;;
      dnf)     echo "sudo dnf install -y" ;;
      zypper)  echo "sudo zypper install -y" ;;
      apk)     echo "sudo apk add" ;;
      brew)    echo "brew install" ;;   # brew nunca com sudo, de propósito
    esac
    return
  done
}
PKG=$(detecta_pkg)

# Cedo porque o 6/8 também precisa dela (o `tailscale serve` publica ESTA porta), não só o resumo.
PORTA_FIM=$(grep '^CP_PORT=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
PORTA_FIM=${PORTA_FIM:-8765}

if [ "$UPDATE" = 0 ] && [ "$CHECK" = 0 ]; then
  echo
  echo "  +--------------------------------------------------+"
  echo "  |  hangar — instalacao                             |"
  echo "  |  cada etapa prova o que fez antes da proxima;    |"
  echo "  |  se algo falhar, eu paro ali e digo o conserto   |"
  echo "  +--------------------------------------------------+"
fi

# ── 0/8 Antes de começar ─────────────────────────────────────────────────────
# As DUAS decisões que são da pessoa (token e "vou usar fora de casa?") vêm juntas, na frente:
# depois disto o instalador segue sozinho, e o que restar de senha já foi anunciado aqui.
# O --check não entra: ele não pode gravar nada no disco, token incluído.
gera_token() { openssl rand -hex 24 2>/dev/null || python3 -c 'import secrets; print(secrets.token_hex(24))'; }
# O backend lê o .env pelo python-dotenv, que corta o valor em " #" e expande "${VAR}": sem aspas
# no .env, a senha com esses caracteres não chega inteira e o celular nunca entra.
# Só ASCII visível: o app nativo monta o cabeçalho Authorization com a senha, e acento não autentica.
TOKEN_PROIBIDO="só vale ASCII sem acento; não vale # \$ ' \" \\ nem espaço no começo ou no fim"
token_proibido() {
  local LC_ALL=C
  case $1 in *[!\ -~]*|*'#'*|*'$'*|*\'*|*'"'*|*'\'*|[[:blank:]]*|*[[:blank:]]) return 0 ;; esac
  return 1
}
if [ "$UPDATE" = 0 ] && [ "$CHECK" = 0 ]; then
say "0/8 Antes de começar"
echo "  No máximo duas perguntas agora, e depois o instalador segue sozinho até o fim."
echo "  Ele pode pedir sua senha de administrador (pergunta antes, cada vez) para:"
echo "  instalar o programa que mantém as sessões abertas (tmux), liberar a porta do"
echo "  Wi-Fi e, se você quiser, o Tailscale."
if [ -f backend/.env ] && grep -q '^CP_AUTH_TOKEN=' backend/.env; then
  ok "backend/.env já tem CP_AUTH_TOKEN (mantido)"
elif [ "$APP" = 1 ]; then
  # Mesmo piso da pergunta do terminal; vazio = o app pediu a senha gerada.
  case $APP_TOKEN in *$'\n'*|*$'\r'*) fail "a senha do celular veio do app com quebra de linha" ;; esac
  if [ -z "$APP_TOKEN" ]; then
    APP_TOKEN=$(gera_token); ok "senha aleatória gerada (o app a lê de backend/.env)"
  elif [ ${#APP_TOKEN} -lt 8 ] || [ "$APP_TOKEN" = change-me ]; then
    fail "a senha do celular veio do app com menos de 8 caracteres"
  elif token_proibido "$APP_TOKEN"; then
    fail "a senha do celular veio do app com caractere que o backend não lê inteiro: $TOKEN_PROIBIDO"
  else
    ok "senha do celular escolhida no app"
  fi
  printf 'CP_AUTH_TOKEN=%s\n' "$APP_TOKEN" >> backend/.env
elif [ "$YES" = 1 ]; then
  printf 'CP_AUTH_TOKEN=%s\n' "$(gera_token)" >> backend/.env
  ok "token aleatório gerado (--yes não pergunta)"
elif [ "$TEM_TTY" = 0 ]; then
  # Sem terminal não dá pra perguntar, mas token é a ÚNICA tranca do app: gerar um aleatório é
  # o lado seguro (o inseguro seria seguir sem token). O que não pode é fazer isso calado —
  # quem rodou precisa saber que existe um token e onde ele está.
  TOKEN=$(gera_token)
  printf 'CP_AUTH_TOKEN=%s\n' "$TOKEN" >> backend/.env
  # NÃO ecoar o valor: sem tty isto roda em provisionamento automatizado (CI, Ansible, ssh
  # não-interativo) e o stdout vira log. Pior aqui do que em qualquer projeto: o próprio app lê
  # `tmux capture-pane`, então uma instalação dentro de um pane monitorado gravaria o token no
  # histórico da ferramenta que ele protege. O PowerShell já não imprimia — era assimetria.
  falta "sem terminal para perguntar — token ALEATÓRIO gerado (valor em backend/.env)"
  nota "está em backend/.env; é ele que você digita no celular. Pra trocar, edite o arquivo."
else
  echo "  Você vai DIGITAR este token no celular, então escolha algo que lembre."
  echo "  Enter em branco = gera um aleatório de 48 caracteres (seguro, chato de digitar)."
  nota "Com Tailscale a rede já é fechada e o token é a segunda tranca. No Wi-Fi de casa ele"
  nota "é a ÚNICA: quem estiver na rede e acertar a senha roda comando como você."
  while :; do
    printf '  Token: '
    read -r TOKEN <&3 || TOKEN=''
    [ -z "$TOKEN" ] && { TOKEN=$(gera_token); ok "aleatório gerado"; break; }
    # O backend só recusa o literal 'change-me'; o piso de 8 é daqui, pra senha curta não passar.
    [ ${#TOKEN} -lt 8 ] && { erro "curto demais — no mínimo 8 caracteres"; continue; }
    [ "$TOKEN" = "change-me" ] && { erro "esse valor o backend recusa de propósito"; continue; }
    token_proibido "$TOKEN" && { erro "$TOKEN_PROIBIDO"; continue; }
    break
  done
  printf 'CP_AUTH_TOKEN=%s\n' "$TOKEN" >> backend/.env
  ok "CP_AUTH_TOKEN gravado em backend/.env"
fi
# Prova = o que o backend exige: valor presente e diferente do literal 'change-me'. O piso de
# 8 caracteres é da PERGUNTA interativa, não da prova — um .env antigo com token curto é válido
# pro backend e não pode derrubar uma reinstalação boa.
grep -q '^CP_AUTH_TOKEN=.\+' backend/.env 2>/dev/null \
  && ! grep -q '^CP_AUTH_TOKEN=change-me[[:space:]]*$' backend/.env \
  || fail "o token não foi gravado em backend/.env — sem ele o celular não entra"
nota "É esse token que você digita no celular na primeira conexão."

QUER_TAILSCALE=0
if [ "$TAILSCALE" = nao ]; then
  nota "Tailscale: não usar (--tailscale=nao), mesmo se já estiver instalado"
elif [ "$TAILSCALE" = sim ]; then
  QUER_TAILSCALE=1; ok "Tailscale: usar fora de casa (--tailscale=sim)"
elif command -v tailscale >/dev/null; then
  QUER_TAILSCALE=1; ok "Tailscale já instalado — vai ser usado"
else
  echo "  Você vai usar o Hangar fora de casa (celular fora do Wi-Fi do PC)?"
  nota "Sim = instala o Tailscale, uma rede privada entre o PC e o celular, sem abrir"
  nota "nada para a internet. Não = só no mesmo Wi-Fi. Dá pra mudar depois: ./install.sh"
  # `|| true`: um "não" aqui é resposta, não falha — sem ele o set -e derrubaria o instalador.
  _pergunta "Usar fora de casa (instalar Tailscale)?" && QUER_TAILSCALE=1 || true
fi
fi
# Sem passo 0 (--update, --check) ninguém respondeu nada, e o padrão é NÃO mexer no Tailscale:
# o --update é o hook do `git pull`, onde um `sudo tailscale up` esperaria senha por 5 minutos.
QUER_TAILSCALE=${QUER_TAILSCALE:-0}
# Estado da etapa "tailscale" do assistente; vazio = a etapa não existe nesta instalação.
TS_STATE=''
[ "$QUER_TAILSCALE" = 1 ] && TS_STATE=ok

# Instala o que cai no $HOME sem root. Separado de propósito do tier que precisa de sudo:
# um instalador que pede senha sem avisar é como se perde a confiança de quem está rodando.
precisa_home() { # precisa_home <rótulo> <cmd> <comando de instalação> <pra quê>
  local rotulo=$1 cmd=$2 instalacao=$3 porque=$4
  if command -v "$cmd" >/dev/null; then ok "$rotulo"; mark_item "$cmd" ok "$rotulo"; return 0; fi
  if [ "$CHECK" = 1 ]; then falta "$rotulo — $porque"; nota "$instalacao"; mark_item "$cmd" fila "$rotulo"; PENDENTE+=("$rotulo"); return 1; fi
  echo "  .. $rotulo não encontrado ($porque)"
  nota "$instalacao"
  if [ "$UPDATE" = 1 ]; then erro "$rotulo faltando (--update não instala dependência)"; PENDENTE+=("$rotulo"); return 1; fi
  if ask "Instalar agora? (vai pro teu \$HOME, sem sudo)"; then
    mark_item "$cmd" fazendo "$rotulo"
    # O erro vai ao log: o relatório de falha precisa da causa. O resultado é conferido logo abaixo.
    eval "$instalacao" >/dev/null 2>>"$LOG" || true
    # O instalador põe em ~/.local/bin, que pode não estar no PATH DESTE shell.
    export PATH="$HOME/.local/bin:$HOME/.local/share/fnm:$PATH"
    hash -r 2>/dev/null || true
    if command -v "$cmd" >/dev/null; then ok "$rotulo instalado"; mark_item "$cmd" ok "$rotulo"; return 0; fi
  fi
  erro "$rotulo continua faltando"; mark_item "$cmd" falhou "$rotulo"
  # Código do primeiro que faltou: agente tem frase própria; o resto quase sempre é download que não veio.
  if [ "$cmd" = claude ]; then DEPS_ERROR=${DEPS_ERROR:-agente-nao-instalou}
  else DEPS_ERROR=${DEPS_ERROR:-$(net_code)}; fi
  PENDENTE+=("$rotulo"); return 1
}

precisa_root() { # precisa_root <rótulo> <cmd> <pacote> <pra quê>
  local rotulo=$1 cmd=$2 pacote=$3 porque=$4
  if command -v "$cmd" >/dev/null; then ok "$rotulo"; mark_item "$cmd" ok "$rotulo"; return 0; fi
  if [ -z "$PKG" ]; then
    erro "$rotulo faltando e não reconheci o gerenciador de pacotes — instale $pacote na mão"
    mark_item "$cmd" falhou "$rotulo"
    PENDENTE+=("$rotulo"); return 1
  fi
  if [ "$CHECK" = 1 ]; then falta "$rotulo — $porque"; nota "$PKG $pacote"; mark_item "$cmd" fila "$rotulo"; PENDENTE+=("$rotulo"); return 1; fi
  echo "  .. $rotulo não encontrado ($porque)"
  nota "$PKG $pacote     <- precisa de senha de administrador"
  if [ "$UPDATE" = 1 ]; then erro "$rotulo faltando (--update não instala dependência)"; PENDENTE+=("$rotulo"); return 1; fi
  if ask_senha "Rodar esse comando?"; then
    mark_item "$cmd" fazendo "$rotulo"
    # O sudo do gerenciador passa pela app_sudo. A saída também vai para um arquivo: é dela que sai
    # o código da falha para o app.
    local instala=$PKG saida
    [ "${PKG%% *}" = sudo ] && instala="app_sudo 'instalar o $rotulo' ${PKG#sudo }"
    saida=$(mktemp)
    if eval "$instala $pacote" 2>&1 | tee "$saida"; then
      rm -f "$saida"; ok "$rotulo instalado"; mark_item "$cmd" ok "$rotulo"; return 0
    fi
    DEPS_ERROR=${DEPS_ERROR:-$(sudo_error_code "$saida")}
    rm -f "$saida"
  fi
  erro "$rotulo continua faltando"; mark_item "$cmd" falhou "$rotulo"; PENDENTE+=("$rotulo"); return 1
}

# ── 1/8 Dependências ─────────────────────────────────────────────────────────
[ "$UPDATE" = 1 ] && say "Modo --update: só o que um git pull não atualiza sozinho" || true
mark_step preparar fazendo
say "1/8 Dependências"
precisa_root "tmux"        tmux   tmux 'sem ele não existe sessão' || true
CLAUDE_INSTALL='curl -fsSL https://claude.ai/install.sh | bash'
ACHADOS=''
for a in $TODOS_AGENTES; do command -v "$a" >/dev/null && ACHADOS="$ACHADOS $a"; done
# Só o --avancado pergunta: o guiado tem duas perguntas e decide o resto pelo que já está no disco.
if [ -z "$AGENTES" ] && [ "$AVANCADO" = 1 ] && [ "$TEM_TTY" = 1 ] && [ "$UPDATE" = 0 ] && [ "$CHECK" = 0 ]; then
  PADRAO_AGENTES=$(echo ${ACHADOS:-claude} | tr ' ' ',')
  echo "  Quais agentes de código você vai usar? (${TODOS_AGENTES// /, }; separe por vírgula)"
  while :; do
    printf '  Agentes [%s]: ' "$PADRAO_AGENTES"
    read -r AGENTES <&3 || AGENTES=''
    AGENTES=${AGENTES// /}; AGENTES=${AGENTES:-$PADRAO_AGENTES}
    agentes_validos "$AGENTES" && break
    erro "não conheço algum desses — use: ${TODOS_AGENTES// /, }"
  done
fi
# Escolhidos que faltam e não são o Claude: o comando oficial deles roda no 2/8, com o backend
# pronto, pela mesma tabela do painel de Harnesses (backend/app/harness_commands.py).
AGENTES_NOVOS=''
if [ -n "$AGENTES" ]; then
  for a in ${AGENTES//,/ }; do
    if [ "$a" = claude ]; then
      precisa_home "Claude Code" claude "$CLAUDE_INSTALL" 'agente escolhido' || true
    elif command -v "$a" >/dev/null; then
      ok "$(nome_agente "$a")"; mark_item "$a" ok "$(nome_agente "$a")"
    elif [ "$CHECK" = 1 ] || [ "$UPDATE" = 1 ]; then
      falta "$(nome_agente "$a") — agente escolhido"; mark_item "$a" fila "$(nome_agente "$a")"; PENDENTE+=("$(nome_agente "$a")")
    else
      AGENTES_NOVOS="$AGENTES_NOVOS $a"; mark_item "$a" fila "$(nome_agente "$a")"; nota "$(nome_agente "$a"): instalado no passo 2/8"
    fi
  done
elif [ -n "$ACHADOS" ]; then
  for a in $ACHADOS; do ok "$(nome_agente "$a")"; mark_item "$a" ok "$(nome_agente "$a")"; done
  command -v claude >/dev/null || nota "Claude Code não é obrigatório; para instalar: ./install.sh --agentes=claude"
else
  precisa_home "Claude Code" claude "$CLAUDE_INSTALL" 'nenhum agente de código encontrado; o Claude Code é o padrão' || true
fi
precisa_home "uv"          uv     'curl -LsSf https://astral.sh/uv/install.sh | sh' 'gerencia o venv do backend' || true
if [ "$FRONTEND" = 0 ]; then
  ok "Node: dispensado (--no-frontend)"
elif ! command -v npm >/dev/null; then
  precisa_home "Node 20+" node \
    'curl -fsSL https://fnm.vercel.app/install | bash && "$HOME/.local/share/fnm/fnm" install 22 && "$HOME/.local/share/fnm/fnm" default 22' \
    'o frontend é Svelte' || true
elif ! node -e 'process.exit(parseInt(process.versions.node) >= 20 ? 0 : 1)' 2>/dev/null; then
  erro "Node 20+ é necessário (atual: $(node --version))"; mark_item node falhou "Node 20+"; PENDENTE+=("Node 20+")
else
  ok "node $(node --version)"; mark_item node ok "Node $(node --version)"
fi

if command -v git >/dev/null; then ok "git"; mark_item git ok git
else falta "git ausente — o painel de git e o chip de branch ficam vazios"; mark_item git pendente git; fi

# Tailscale entra aqui, junto das outras dependências: a decisão já foi tomada no passo 0, e o
# 6/8 só publica. Falhar aqui não derruba a instalação — o app ainda funciona no Wi-Fi de casa.
# `UPDATE = 0` nos dois: o --update é o hook do `git pull` e nada ali pode parar pedindo senha.
# O app liga cada item à última etapa que começou: a Tailscale abre a dela e devolve a vez.
[ -n "$TS_STATE" ] && mark_step tailscale fazendo
# No --app o script da Tailscale, que chama sudo por dentro, roda inteiro como administrador: o
# sudo de dentro já é root e não pede nada. No terminal, como sempre.
ts_install() {
  if [ "$APP" = 1 ]; then app_sudo "instalar a Tailscale" sh -c 'curl -fsSL https://tailscale.com/install.sh | sh'
  else curl -fsSL https://tailscale.com/install.sh | sh; fi
}
# O link do login sai no stderr do `tailscale up`; no --app ele vira marca e o app abre o navegador.
tailscale_up() {
  app_sudo "entrar na conta Tailscale" timeout 300 tailscale up 2>&1 | while IFS= read -r linha; do
    printf '%s\n' "$linha"
    case $linha in
      *https://login.tailscale.com/*)
        if [ "$APP" = 1 ]; then
          echo "##HANGAR-LINK## tailscale-login $(printf '%s\n' "$linha" | grep -o 'https://login\.tailscale\.com/[^[:space:]]*')"
        fi ;;
    esac
  done
  return "${PIPESTATUS[0]}"
}
if [ "$UPDATE" = 0 ] && [ "$QUER_TAILSCALE" = 1 ] && command -v tailscale >/dev/null; then
  mark_item tailscale ok Tailscale
fi
if [ "$UPDATE" = 0 ] && [ "$QUER_TAILSCALE" = 1 ] && ! command -v tailscale >/dev/null; then
  nota "Tailscale: instalação do sistema."
  if ask_senha "Instalar o Tailscale agora (vai pedir a senha)?"; then
    mark_item tailscale fazendo Tailscale
    if ts_install && command -v tailscale >/dev/null; then
      ok "Tailscale instalado"; mark_item tailscale ok Tailscale
    else
      anota_problema "instalação do Tailscale falhou"; QUER_TAILSCALE=0; TS_STATE=pendente
      mark_item tailscale falhou Tailscale
    fi
  else
    QUER_TAILSCALE=0; nota "pulado — o celular entra só pelo Wi-Fi do PC"
  fi
fi
if [ "$UPDATE" = 0 ] && [ "$QUER_TAILSCALE" = 1 ]; then
  if tailscale status >/dev/null 2>&1; then
    mark_item tailscale-conta ok "conta Tailscale"
  else
    echo "  Falta entrar no Tailscale: vai abrir um link, faça login no navegador (até 5 min)."
    if ask_senha "Entrar agora (vai pedir a senha)?"; then
      mark_item tailscale-conta fazendo "conta Tailscale"
      if tailscale_up; then
        mark_item tailscale-conta ok "conta Tailscale"
      else
        anota_problema "login no Tailscale não concluiu — depois: sudo tailscale up e ./install.sh" tailscale-login
        TS_STATE=pendente; mark_item tailscale-conta pendente "conta Tailscale"
      fi
    fi
  fi
fi
[ -n "$TS_STATE" ] && mark_step preparar fazendo

if [ "$CHECK" = 1 ]; then
  if [ -x backend/.venv/bin/python ]; then
    say "Diagnóstico"; (cd backend && uv run --quiet --no-sync python -m app.doctor) || PENDENTE+=("doctor")
  fi
  if [ ${#PENDENTE[@]} -eq 0 ]; then mark_step preparar ok; FINAL_STATE=ok; say "Nada faltando."; exit 0; fi
  mark_step preparar pendente; FINAL_STATE=pendente
  say "Faltam: ${PENDENTE[*]}"; exit 1
fi
[ ${#PENDENTE[@]} -eq 0 ] || fail_with "$DEPS_ERROR" "faltam: ${PENDENTE[*]}"
mark_step preparar ok

# ── 2/8 Backend ──────────────────────────────────────────────────────────────
mark_step instalar fazendo
say "2/8 Backend"
(cd backend && uv sync --quiet) || fail_with "$(net_code)" "uv sync falhou — o backend ficou sem as dependências"
ok "dependências instaladas"
mark_item backend ok "servidor do Hangar"
# Sem o par VAPID o push para o celular sai calado e o celular nem se inscreve.
(cd backend && uv run --quiet --no-sync python -m app.vapid_setup) \
  || anota_problema "chaves do push para o celular não geradas — rode: cd backend && uv run python -m app.vapid_setup" push
nota "psutil NÃO entra aqui: no Linux existe /proc e ele é mais rápido (ver app/procinfo.py)"

for a in $AGENTES_NOVOS; do
  mark_item "$a" fazendo "$(nome_agente "$a")"
  if (cd backend && gira "instalando $(nome_agente "$a")" uv run --quiet --no-sync python -m app.harness_commands "$a"); then
    export PATH="$HOME/.local/bin:$PATH"; hash -r 2>/dev/null || true
    if command -v "$a" >/dev/null; then ok "$(nome_agente "$a") instalado"; mark_item "$a" ok "$(nome_agente "$a")"
    else
      anota_problema "$(nome_agente "$a") instalou, mas o comando $a não aparece no PATH — abra outro terminal e rode ./install.sh de novo" agente-nao-instalou
      mark_item "$a" falhou "$(nome_agente "$a")"
    fi
  else
    anota_problema "$(nome_agente "$a") não instalou — instale pelo painel Harnesses do app ou veja a saída acima" agente-nao-instalou
    mark_item "$a" falhou "$(nome_agente "$a")"
  fi
done
# Prova do mínimo: sem nenhum agente o app abre, mas não tem o que pilotar.
TEM_AGENTE=0
for a in $TODOS_AGENTES; do command -v "$a" >/dev/null && TEM_AGENTE=1; done
[ "$TEM_AGENTE" = 1 ] || fail_with sem-agente "nenhum agente de código instalado — o Hangar precisa de pelo menos um (./install.sh --agentes=claude)"

# ── 3/8 Token de acesso ──────────────────────────────────────────────────────
# O trabalho foi feito no passo 0: token e Tailscale são as DUAS decisões da pessoa, e elas
# vêm antes de qualquer instalação — depois o instalador segue sozinho.
say "3/8 Token de acesso"
# Confere o que o passo 0 gravou, em vez de anunciar sucesso de memória: sem TTY o passo 0 não
# pergunta nada, e um `.env` sem token deixa o celular de fora sem ninguém ver.
if grep -q '^CP_AUTH_TOKEN=.\+' backend/.env 2>/dev/null \
   && ! grep -q '^CP_AUTH_TOKEN=change-me[[:space:]]*$' backend/.env; then
  ok "definido no passo 0"
  mark_item token ok "senha do celular"
elif [ "$UPDATE" = 1 ]; then
  anota_problema "backend/.env sem CP_AUTH_TOKEN — rode ./install.sh sem --update para definir"
else
  fail "o token não foi gravado em backend/.env — sem ele o celular não entra"
fi

# ── 4/8 Frontend ─────────────────────────────────────────────────────────────
# O CI compila o front a cada push na main e publica o resultado na release `dist-latest`. Baixar
# de lá evita o passo mais lento e mais frágil da instalação (o `npm ci` + build local).
DIST_URL=https://github.com/jeffer1312/hangar/releases/download/dist-latest
# Cada desistência diz o MOTIVO: só o sucesso falava, então quem via o `npm ci` de um minuto e meio
# rodando não tinha como saber se o download nem foi tentado, se o CI ainda não publicou aquele
# commit, ou se foi a própria árvore que o desqualificou.
baixar_dist() { # 0 = frontend/dist agora tem o build publicado pelo CI
  command -v curl >/dev/null 2>&1 && command -v tar >/dev/null 2>&1 \
    || { nota "dist do CI não usado: falta curl ou tar"; return 1; }
  local sha_local sha_remoto tmp
  sha_local=$(git rev-parse HEAD 2>/dev/null) \
    || { nota "dist do CI não usado: sem git, não dá pra saber de que commit ele é"; return 1; }
  # Árvore suja no front = quem está editando quer o SEU código na tela, não o do CI. `packages`
  # junto porque metade das fontes da tela mora no `@hangar/core`: olhando só `frontend`, uma
  # edição em api.ts/format.ts seria apagada por um dist do CI que não a contém.
  [ -z "$(git status --porcelain -- frontend packages 2>/dev/null)" ] \
    || { nota "dist do CI não usado: frontend/ ou packages/ tem mudança local — ele apagaria ela da tela"; return 1; }
  sha_remoto=$(curl -fsSL --max-time 15 "$DIST_URL/frontend-dist.sha" 2>/dev/null) \
    || { nota "dist do CI não usado: não consegui ler o frontend-dist.sha (rede ou release fora)"; return 1; }
  # Commit diferente NÃO cancela mais o download: o dist mais recente do CI é melhor que um build
  # local, que é o passo mais frágil da instalação. Só dizemos de que commit ele é.
  [ "$sha_remoto" = "$sha_local" ] \
    || nota "o dist do CI é do commit ${sha_remoto:0:8} e este checkout está em ${sha_local:0:8}"
  tmp=$(mktemp -d "frontend/.dist-baixado.XXXXXX") \
    || { nota "dist do CI não usado: não consegui criar a pasta temporária em frontend/ (permissão ou disco cheio)"; return 1; }
  # Extrai ao LADO do dist e só então troca: um download interrompido no meio não pode deixar a
  # máquina sem front nenhum — o build local depois nem roda, porque este caminho já disse "ok".
  if curl -fsSL --max-time 180 "$DIST_URL/frontend-dist.tar.gz" 2>/dev/null | tar -xzf - -C "$tmp" \
     && [ -f "$tmp/index.html" ]; then
    rm -rf frontend/dist && mv "$tmp" frontend/dist && return 0
  fi
  rm -rf "$tmp"
  nota "dist do CI não usado: o frontend-dist.tar.gz não baixou ou veio incompleto"
  return 1
}
say "4/8 Frontend"
if [ "$FRONTEND" = 0 ]; then
  ok "pulado (--no-frontend)"
  nota "UM frontend atende VÁRIOS backends: ele guarda a lista de servidores no próprio"
  nota "navegador (chave cp_servers) e você adiciona cada máquina pelo menu de conta."
  nota "Então dá pra ter o PWA numa VPS e só o backend em cada máquina de trabalho —"
  nota "menos porta aberta e nenhum processo node de graça por aqui."
  nota "Pra ligar este backend ao front que você já usa: abra o PWA, adicione o servidor"
  nota "com a URL que o backend imprime no QR, e cole o token de backend/.env."
else
# Só rebuilda se houver motivo: dist ausente, ou alguma fonte/lockfile mais novo que ele. Num
# re-run logo após um `git pull` sem mudança de front, isso economiza o `npm ci` inteiro.
# `packages/core/src` na lista porque a tela é buildada a partir das DUAS árvores: sem ele, um pull
# que mexe só no `@hangar/core` responde "nada mudou" e serve o dist velho, calado.
DIST=frontend/dist/index.html
if [ -f "$DIST" ] && [ -z "$(find frontend/src packages/core/src package-lock.json \
                              frontend/index.html \
                              frontend/vite.config.* -newer "$DIST" -print -quit 2>/dev/null)" ]; then
  ok "frontend já buildado e atualizado (nada mudou desde o último build)"
elif baixar_dist; then
  ok "dist baixado do CI (não precisou compilar aqui)"
elif [ "$UPDATE" = 1 ]; then
  # Atualização NUNCA compila o front. O build local é o passo mais frágil que existe aqui — na VM
  # Windows ele nem roda, porque o `npm ci` de um lock feito no Linux não traz a dependência nativa
  # do rollup e o build morre. Sem o dist do CI, a tela fica na versão anterior e o resto segue.
  anota_problema "frontend não atualizado: o dist do CI não pôde ser baixado — a tela continua na versão anterior"
else
  # Só o modo interativo chega aqui (o --update parou no ramo acima), então o --silent é sempre bom:
  # ele existe pra não poluir o terminal de quem instala.
  QUIETO=--silent
  # A flag vai ANTES do nome do script: no npm 11 `npm run build --silent` não é mais consumida
  # pelo npm, ela é repassada ao script e chega no `vite build`, que morre com CACError.
  # `npm ci` na RAIZ (`frontend` não tem lockfile próprio, e o `@hangar/core` só existe como link
  # criado por instalação na raiz) e SELETIVO: o app nativo também é workspace — precisa ser, senão
  # o EAS Build não detecta o monorepo —, e sem os dois `--workspace` isto baixaria o toolchain do
  # React Native na máquina de quem só quer usar o Hangar. O build dele é no Expo.
  build_front() { npm ci --workspace=@hangar/core --workspace=frontend $QUIETO && npm run $QUIETO build -w frontend; }
  gira "npm ci + build do frontend" build_front \
    && [ -f "$DIST" ] && ok "buildado em frontend/dist/" \
    || fail "o build do frontend falhou — corrige o erro acima e re-roda (ele continua de onde parou)"
fi
fi
[ "$FRONTEND" = 1 ] && mark_item frontend ok "tela do Hangar"

# ── App nativo (desktop-native, release native-latest) ───────────────────────
# É a única janela de desktop instalada. Falhar aqui não derruba a instalação: o Hangar segue no navegador.
say "App nativo"
if [ "$SEM_NATIVO" = 1 ]; then
  # O próprio app (assistente) se copia para o lugar de sempre; o install-native.sh baixaria a
  # native-latest por cima do binário aberto.
  ok "pulado (--sem-nativo): o app já está nesta máquina"
  mark_item nativo ok "app do Hangar"
else
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) NATIVO_APP="$HOME/.local/bin/hangar-native" ;;
  Darwin-arm64) NATIVO_APP="$HOME/Applications/Hangar.app" ;;
  *) NATIVO_APP= ;;
esac
if [ -z "$NATIVO_APP" ]; then
  ./scripts/install-native.sh || true
  falta "sem app nativo para esta máquina — use o Hangar pelo navegador"
elif ./scripts/install-native.sh && [ -e "$NATIVO_APP" ]; then
  ok "app nativo instalado (lançador \"Hangar\")"
  mark_item nativo ok "app do Hangar"
  if [ -x "$HOME/.local/bin/hangar-native" ] && [ "$TEM_TTY" = 1 ] && [ "$UPDATE" = 0 ] && { [ -n "${WAYLAND_DISPLAY:-}" ] || [ -n "${DISPLAY:-}" ]; }; then
    ABRIR_NATIVO=1
  fi
else
  anota_problema "o app nativo não instalou — use o Hangar pelo navegador (tente: ./scripts/install-native.sh)"
  mark_item nativo pendente "app do Hangar"
fi
fi

# ── Binários Rust (hangar-server e hangar-cano, release server-latest) ───────
# Sem eles o backend em Python atende sozinho: falha aqui nunca para a instalação.
say "Binários Rust"
RUST_RC=0
(cd backend && uv run --quiet --no-sync python -m app.rust_release) || RUST_RC=$?
case "$RUST_RC" in
  0) ok "hangar-server e hangar-cano em ~/.hangar/bin"; mark_item rust ok "servidor rápido" ;;
  2) falta "sem binários Rust para esta máquina — o backend em Python atende sozinho"; mark_item rust ok "servidor rápido (não há para esta máquina)" ;;
  *) anota_problema "binários Rust não baixaram — o backend em Python atende sozinho (tente: cd backend && uv run python -m app.rust_release)"; mark_item rust pendente "servidor rápido" ;;
esac

# ── 5/8 Wrappers do claude e do codex ────────────────────────────────────────
# Sem eles um `claude` que VOCÊ abre no terminal é invisível pro app: sem --session-id o backend
# não sabe qual transcript é daquela sessão, e fora do tmux não há pane pra ler estado nem
# receber input. Sessão criada PELO app funciona de qualquer jeito; isto é a outra direção.
WRAP_N=${#PROBLEMAS[@]}
say "5/8 Wrappers do claude e do codex"
# Já instalado -> nem pergunta. Re-rodar o install.sh depois de um `git pull` deve pegar só o
# que falta, sem obrigar a responder S/n pro que já está de pé.
# Já instalado -> RE-RODA sem perguntar, em vez de pular. "Instalado" não é "atualizado": os
# sub-scripts geram conteúdo (blocos de rc, units, o texto do protocolo no ~/.claude/CLAUDE.md)
# que um `git pull` sozinho não atualiza. Eles são idempotentes, então re-rodar é barato; o que
# não pode voltar é perguntar S/n pro que já está de pé.
if [ -e "$HOME/.local/bin/hangar-engine" ]; then
  ./scripts/install-claude-wrapper.sh --statusline >/dev/null && ok "wrappers atualizados" || anota_problema "wrappers do claude/codex falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$WRAPPER" = 1 ] && ask "Instalar (recomendado)?"; then
  ./scripts/install-claude-wrapper.sh || anota_problema "wrappers do claude/codex não instalaram"
else
  nota "pulado — sessão aberta no terminal não vai aparecer no app"
  nota "depois: ./scripts/install-claude-wrapper.sh"
fi
mark_item_since wrappers "$WRAP_N" "sessão aberta no terminal aparece no app"

# ── 6/8 Acesso pelo celular ──────────────────────────────────────────────────
mark_step celular fazendo
CEL_N=${#PROBLEMAS[@]}
say "6/8 Acesso pelo celular"
# Escutar em todas as interfaces é o que deixa os clientes usarem a rede local antes do Tailscale.
# 0.0.0.0 e nunca 'auto': 'auto' tira o loopback, e o `tailscale serve` fala com localhost. Valor
# escolhido à mão (um IP específico) fica; só o padrão só-local é trocado. Vale no --update.
BIND_ATUAL=$(grep -E '^CP_LAN_BIND_IP=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- | cut -d'#' -f1 | tr -d " \"'" || true)
if [ "$CHECK" = 0 ] && { [ -z "$BIND_ATUAL" ] || [ "$BIND_ATUAL" = 127.0.0.1 ] || [ "$BIND_ATUAL" = localhost ] || [ "$BIND_ATUAL" = ::1 ]; }; then
  grep -q '^CP_LAN_BIND_IP=' backend/.env 2>/dev/null && sed -i.bak '/^CP_LAN_BIND_IP=/d' backend/.env && rm -f backend/.env.bak
  [ -n "$(tail -c1 backend/.env 2>/dev/null)" ] && echo >> backend/.env   # .env sem quebra no fim
  printf 'CP_LAN_BIND_IP=0.0.0.0\n' >> backend/.env
  ok "CP_LAN_BIND_IP=0.0.0.0 gravado — a rede local alcança o backend (o token continua exigido)"
fi
grep -qE '^CP_LAN_BIND_IP=' backend/.env 2>/dev/null || [ "$CHECK" = 1 ] \
  || fail "CP_LAN_BIND_IP não foi gravado em backend/.env"
mark_item rede-local ok "rede de casa"
if [ "$UPDATE" = 1 ]; then
  ok "pulado no --update (firewall e Tailscale pedem senha; nada aqui muda com git pull)"
else
echo "  Duas formas, e elas não competem:"
echo "    LAN       — celular no mesmo Wi-Fi. Precisa liberar as portas no firewall."
echo "    Tailscale — VPN pessoal. Funciona de QUALQUER lugar sem expor nada pra internet."
nota "Fora de casa, use Tailscale. NUNCA abra porta pra internet pública: o app roda o"
nota "claude como VOCÊ, então um host exposto é execução remota na sua máquina."

porta_liberada() { # 0 = já tem regra pra esta porta
  if command -v ufw >/dev/null; then
    sudo -n ufw status 2>/dev/null | grep -q "^$1" && return 0
  fi
  if command -v firewall-cmd >/dev/null; then
    firewall-cmd --list-ports 2>/dev/null | grep -q "$1/tcp" && return 0
  fi
  return 1
}
if command -v ufw >/dev/null || command -v firewall-cmd >/dev/null; then
  # A 5173 só entra quando ESTA máquina tem serviço de front (instalação antiga que ficou no
  # `vite preview`). Sem ele quem serve a interface é o backend, na 8765, e abrir uma porta que
  # ninguém escuta é furo aberto de graça. Mesma pergunta que o services-setup.sh faz depois.
  PORTAS=(8765)
  [ "$FRONTEND" = 1 ] && [ -f "$HOME/.config/systemd/user/hangar-frontend.service" ] && PORTAS+=(5173)
  LISTA="${PORTAS[*]}"
  FALTA=0
  for p in "${PORTAS[@]}"; do porta_liberada "$p" || FALTA=1; done
  if [ "$FALTA" = 0 ]; then
    ok "porta(s) $LISTA já liberada(s) no firewall"
    mark_item firewall ok "porta do Wi-Fi liberada"
  else
    nota "Liberar precisa de senha de administrador. Por fora seria:"
    for p in "${PORTAS[@]}"; do nota "    sudo ./scripts/lan-setup.sh $p"; done
    if ask_senha "Liberar a(s) porta(s) $LISTA agora (vai pedir a senha)?"; then
      OK_FW=1
      for p in "${PORTAS[@]}"; do app_sudo "liberar a porta $p no firewall" ./scripts/lan-setup.sh "$p" || OK_FW=0; done
      if [ "$OK_FW" = 1 ]; then ok "portas liberadas"; mark_item firewall ok "porta do Wi-Fi liberada"
      else anota_problema "liberar portas no firewall falhou"; mark_item firewall pendente "porta do Wi-Fi liberada"; fi
    fi
  fi
else
  nota "sem ufw/firewalld — provavelmente não há firewall bloqueando (padrão de Arch/CachyOS)"
fi
[ "${#PROBLEMAS[@]}" -gt "$CEL_N" ] && mark_step celular pendente || mark_step celular ok

publica_tailscale() { # grava CP_PUBLIC_URL com o https do tailnet; o serve precisa de root (ou operator)
  local nome saida_serve teto='' codigo=outro
  nome=$(tailscale status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))' 2>/dev/null || true)
  [ -n "$nome" ] || { falta "Tailscale sem login — sudo tailscale up e ./install.sh de novo"; TS_STATE=pendente; return 1; }
  nota "Publicar o Hangar no Tailscale precisa da senha (tailscale serve)."
  ask_senha "Publicar agora (vai pedir a senha)?" || { nota "pulado — depois: ./install.sh"; return 1; }
  # No --app ninguém responde ao convite que o serve imprime com o HTTPS desligado: o teto devolve a
  # vez ao app, que mostra a pendência com "Conferir de novo".
  [ "$APP" = 1 ] && teto='timeout 60'
  if saida_serve=$(app_sudo "publicar o Hangar na Tailscale" $teto tailscale serve --bg "$PORTA_FIM" 2>&1) \
     && tailscale serve status --json 2>/dev/null | grep -q '"443"'; then
    grep -q '^CP_PUBLIC_URL=' backend/.env 2>/dev/null && sed -i.bak '/^CP_PUBLIC_URL=/d' backend/.env && rm -f backend/.env.bak
    printf 'CP_PUBLIC_URL=https://%s\n' "$nome" >> backend/.env
    ok "publicado no Tailscale: https://$nome"
    mark_item tailscale-https ok "endereço seguro (HTTPS)"
  else
    printf '%s\n' "$saida_serve" | grep -qiE 'https|not enabled' && codigo=tailscale-https
    anota_problema "tailscale serve falhou — HTTPS do tailnet desligado? Habilite em https://login.tailscale.com/admin/dns (HTTPS Certificates) e rode ./install.sh de novo" "$codigo"
    TS_STATE=pendente; mark_item tailscale-https pendente "endereço seguro (HTTPS)"
    return 1
  fi
}
# Compartilhar sessão liga o Funnel pelo backend, que roda como este usuário: sem operador o tailscaled recusa.
# Não depende do `serve` ter passado; só a instalação nova chega aqui (o --update não pode pedir senha).
operador_tailscale() {
  local u="${USER:-$(id -un)}"
  command -v tailscale >/dev/null || return 0
  sudo -n true 2>/dev/null || ask_senha "Liberar o Funnel para compartilhar sessão (vai pedir a senha)?" \
    || { nota "pulado — para compartilhar sessão: sudo tailscale set --operator=$u"; return 0; }
  if app_sudo "liberar o compartilhamento de sessão" tailscale set --operator="$u" >/dev/null 2>&1; then ok "Tailscale: $u pode ligar o Funnel (compartilhar sessão)"
  else anota_problema "tailscale set --operator falhou — compartilhar sessão vai pedir: sudo tailscale set --operator=$u"; TS_STATE=pendente; fi
}
if [ -n "$TS_STATE" ]; then
  mark_step tailscale fazendo
  if [ "$QUER_TAILSCALE" = 1 ]; then publica_tailscale || true; operador_tailscale; fi
  mark_step tailscale "$TS_STATE"
else
  nota "sem Tailscale: o celular entra pelo Wi-Fi do PC. Fora de casa? ./install.sh e responda Sim."
fi

fi

# ── 7/8 Rodar sozinho + sessões-irmãs + painel ───────────────────────────────
mark_step instalar fazendo
say "7/8 Serviços, hangar-send e painel"
# No --app ninguém vai subir o backend na mão: sem systemd de usuário o assistente não termina.
if [ "$APP" = 1 ] && ! systemctl --user show-environment >/dev/null 2>&1; then
  fail_with sem-systemd "sem systemd de usuário nesta máquina — o Hangar não tem como iniciar sozinho"
fi
SERV_N=${#PROBLEMAS[@]}
if ! command -v systemctl >/dev/null; then
  nota "serviços: sem systemd nesta máquina — rode backend e frontend na mão"
elif systemctl --user list-unit-files hangar-backend.service >/dev/null 2>&1 &&
     systemctl --user cat hangar-backend.service >/dev/null 2>&1; then
  # O caminho do node e o WorkingDirectory ficam CRAVADOS dentro da unit — git pull não os
  # muda. O próprio services-setup.sh só reinicia o que mudou de verdade, então re-rodar aqui
  # não derruba a conexão SSE do celular à toa.
  # O rc do setup e o is-active cobrem furos diferentes: o rc pega "setup falhou com o serviço
  # velho ainda de pé" (pareceria ok), e o is-active pega "setup saiu 0 mas o serviço não subiu".
  SETUP_RC=0
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only >/dev/null || SETUP_RC=$?
  else ./scripts/services-setup.sh >/dev/null || SETUP_RC=$?; fi
  [ "$SETUP_RC" != 0 ] && anota_problema "services-setup.sh falhou (exit $SETUP_RC)"
  [ "$(systemctl --user is-active hangar-backend.service 2>/dev/null)" = active ] \
    && ok "serviços atualizados (active)" \
    || anota_problema "serviços atualizados mas o hangar-backend não está active"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$SERVICES" = 1 ] && ask "Rodar backend+frontend como serviços de usuário (sobrevivem a fechar o terminal)?"; then
  SETUP_RC=0
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only || SETUP_RC=$?
  else ./scripts/services-setup.sh || SETUP_RC=$?; fi
  [ "$SETUP_RC" != 0 ] && anota_problema "services-setup.sh falhou (exit $SETUP_RC)"
  [ "$(systemctl --user is-active hangar-backend.service 2>/dev/null)" = active ] \
    || anota_problema "serviços instalados mas o hangar-backend não está active"
  nota "Pra sobreviver a logout/reboot também: loginctl enable-linger \$USER"
else
  nota "pulado — rodando na mão, fechar o terminal derruba o backend"
fi
mark_item_since servicos "$SERV_N" "início automático"
SEND_N=${#PROBLEMAS[@]}

if [ -e "$HOME/.local/bin/hangar-send" ]; then
  # O binário é symlink (atualiza sozinho), mas o bloco "Sessões-irmãs" do ~/.claude/CLAUDE.md
  # sai de um heredoc deste script: sem re-rodar, as sessões novas leem o protocolo VELHO.
  ./scripts/install-hangar-send.sh >/dev/null && ok "hangar-send + skills atualizados" || anota_problema "hangar-send + skills falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$CPSEND" = 1 ] && ask "Instalar hangar-send + skills (sessões conversam entre si e se pareiam)?"; then
  ./scripts/install-hangar-send.sh || anota_problema "hangar-send + skills não instalaram"
else
  nota "pulado — depois: ./scripts/install-hangar-send.sh"
fi
mark_item_since hangar-send "$SEND_N" "sessões conversam entre si"

# Ponte de skills pro Pi e pro Kimi. Roda SEMPRE (inclusive no --update): quem cria sessão nesses
# dois agentes é este app, e uma sessão nascida assim não enxerga as skills do Claude sem a ponte.
# Sai 0 e não faz nada quando nenhum dos dois está instalado.
./scripts/install-skills-bridge.sh >/dev/null 2>&1 \
  && ok "ponte de skills (Pi/Kimi) atualizada" \
  || nota "ponte de skills pulada — depois: ./scripts/install-skills-bridge.sh"

# Sessões sobrevivendo a reboot: TPM + resurrect + continuum + um timer systemd que salva.
# Fica DEPOIS dos serviços de propósito — é o único passo aqui que clona repositório de
# terceiro (os plugins do tmux), então merece pergunta própria mesmo com --yes já dito.
if [ -d "$HOME/.tmux/plugins/tmux-resurrect" ]; then
  ./scripts/tmux-persist-setup.sh >/dev/null && ok "persistência de sessões atualizada" || erro "persistência de sessões falhou ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
else
  nota "Opcional: fazer as sessões voltarem depois de um reboot/OOM, com a conversa junto."
  nota "Clona 3 plugins de tmux de terceiros (tpm, resurrect, continuum) no teu ~/.tmux."
  ask_extra "Instalar a persistência de sessões?" && { ./scripts/tmux-persist-setup.sh || anota_problema "persistência de sessões não instalou"; }
fi

# Painel flutuante + tray. Só Hyprland com Quickshell (testado no rice end-4/dots-hyprland).
if ! { command -v qs >/dev/null && pgrep -x Hyprland >/dev/null; }; then
  nota "painel do desktop: pulado (requer Hyprland + Quickshell)"
# Detecta pelo SYMLINK, nao pela unit systemd: medido nesta maquina, o painel roda sob `flock`
# direto (`qs -n -c hangar`) e nao existe hangar-panel.service — checar a unit dava "ausente"
# num painel instalado e vivo.
elif [ -e "$HOME/.local/bin/hangar-panel-open" ]; then
  ok "painel + tray já instalados"
  # Único passo que NÃO re-roda sozinho: o painel é a única coisa aqui que está VISIVELMENTE
  # em execução no teu desktop, e a forma como ele sobe pode divergir da que o script gera
  # (medido: rodando sob flock, sem unit systemd). Re-instalar por conta própria mudaria algo
  # que funciona, sem pedir. Os arquivos são symlink, então QML e scripts já vêm do git pull.
  nota "atualizar de propósito (muda como o painel sobe): ./scripts/install-hangar-panel.sh"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$PANEL" = 1 ] && ask_extra "Instalar painel flutuante + tray (SUPER+SHIFT+U)?"; then
  ./scripts/install-hangar-panel.sh || anota_problema "painel do desktop não instalou"
fi

# ── Passos de atualização: marcar como já feitos ─────────────────────────────
# Uma instalação do ZERO já satisfaz todo passo de `docs/atualizacoes/` — eles existem pra levar
# uma máquina ANTIGA até aqui. Sem esta marca, a primeira vez que essa máquina apertasse Atualizar
# no app, ela rodaria a história inteira de passos, todos já cumpridos por este instalador.
# No --update NÃO se marca nada: ali a máquina é justamente a antiga, e os passos precisam rodar.
if [ "$UPDATE" = 1 ]; then
  :
else
  say "Passos de atualização"
  if uv run --directory backend python -c "from app import atualizacoes; atualizacoes.marcar_todos()" 2>/dev/null; then
    ok "marcados como já aplicados (instalação nova)"
  else
    nota "não consegui marcar agora — o app resolve no primeiro Atualizar"
  fi
fi

# ── Atualizar sozinho no próximo git pull (opcional) ─────────────────────────
# Hook post-merge: roda depois de todo `git pull` bem-sucedido. A escolha de atualizar continua
# sendo tua — o pull é que dispara, e o pull você deu. Sem isto, um pull te deixa com código
# novo e units/protocolo velhos, e nada avisa.
HOOK=".git/hooks/post-merge"
if [ "$UPDATE" = 1 ]; then
  :   # o próprio hook está rodando; não se re-instala no meio da própria execução
elif [ ! -d .git ]; then
  nota "sem .git (cópia sem histórico?) — hook de atualização indisponível"
elif [ -f "$HOOK" ] && grep -q 'install.sh --update' "$HOOK"; then
  ok "hook de atualização já instalado"
  nota "remover: rm $HOOK"
elif [ -f "$HOOK" ]; then
  falta "já existe um $HOOK que não é nosso — não vou mexer nele"
  nota "pra somar, acrescente a linha:  ./install.sh --update"
else
  echo "  Quando alguém atualizar o Hangar por 'git pull', isto reaplica sozinho o que o pull"
  echo "  não cobre. Não pede senha. Pelo botão Atualizar do app não precisa de nada disto."
  if ask "Deixar o próximo 'git pull' já se atualizar sozinho?"; then
  # Corpo vem de scripts/post-merge.hook, FONTE ÚNICA compartilhada com o install.ps1 (que passou a
  # instalar o mesmo hook no Windows, onde ninguém o instalava). Era um heredoc aqui; com dois
  # instaladores, duas cópias inline divergiriam — e divergência de cópia duplicada é a família de
  # bug mais caro deste projeto.
    if [ -f scripts/post-merge.hook ]; then
      cp scripts/post-merge.hook "$HOOK"
      chmod +x "$HOOK"
      ok "hook instalado — o próximo 'git pull' já se atualiza sozinho"
    else
      falta "scripts/post-merge.hook não encontrado — hook não instalado"
    fi
  else
    nota "pulado — depois de um git pull, rode ./install.sh --update na mão"
  fi
fi

# ── 8/8 Checagem de fumaça ───────────────────────────────────────────────────
# Até aqui foi tudo instalação. Este passo separa "instalou" de "funciona" — e é o mesmo passo
# que, do lado Windows, pegou o backend não importando por causa de um `import fcntl`.
mark_step instalar ok
mark_step final fazendo
say "8/8 Checagem de fumaça"
(cd backend && uv run python -c "from app import api, registry, procinfo, projects" ) \
  && { ok "o backend importa"; mark_item backend-importa ok "o servidor abre"; } \
  || fail "o backend não importa nesta máquina"

(cd backend && uv run python -c "from app import procinfo; assert procinfo._TEM_PROC, 'sem /proc'") \
  && ok "leitura de processo via /proc funcionando" \
  || nota "sem /proc (macOS?) — o backend usa psutil, confira se ele foi instalado"

S="cp-fumaca-$$"
if tmux new-session -d -s "$S" -c /tmp 'sh' 2>/dev/null; then
  tmux kill-session -t "=$S" 2>/dev/null
  ok "o multiplexador cria e mata sessão"
  mark_item multiplexador ok "sessões abrem e fecham"
else
  fail "o tmux não criou uma sessão de teste — o app não vai abrir sessão"
fi

# ── Portão final: extra que a pessoa pediu e falhou NÃO passa em branco ────────────────────
if [ ${#PROBLEMAS[@]} -gt 0 ]; then
  say "Terminou com pendências"
  for p in "${PROBLEMAS[@]}"; do falta "$p"; done
  echo "  O que já estava no ar continua no ar — e é por isso que isto precisa ser dito alto:"
  echo "  a tela pode seguir funcionando e a instalação PARECER boa. Resolve a lista e re-roda;"
  echo "  o instalador é idempotente e continua de onde parou."
  if [ "$UPDATE" = 1 ]; then
    # Sai 0 (derrubar a atualização inteira por um extra opcional seria pior), mas a tela do
    # Atualizar só enxerga o ##HANGAR-AVISO## — sem a marca, o app mostraria "Atualizado" com
    # extra quebrado. E NÃO cai no "Pronto": a última palavra não pode ser sucesso.
    echo "##HANGAR-AVISO## terminou com pendencias: ${PROBLEMAS[*]}"
  else
    mark_step final pendente; FINAL_STATE=pendente
    exit 1
  fi
else
  mark_step final ok
  say "Pronto"
fi
if [ "${ABRIR_NATIVO:-0}" = 1 ]; then
  # O passo 7/8 acabou de reiniciar o backend; abrir antes da porta voltar mostra a tela de
  # "não consegui carregar a interface" (medido: serviço active às :53, janela aberta no mesmo
  # segundo, porta ainda fechada). Espera até 20s; sem serviço a porta nunca abre e aí a janela
  # mesmo mostra o que falta.
  for _ in $(seq 1 40); do
    (exec 3<>"/dev/tcp/127.0.0.1/$PORTA_FIM") 2>/dev/null && break
    sleep 0.5
  done
  # Destacado do instalador (setsid + sem stdio): fechar o terminal não leva a janela junto.
  setsid "$HOME/.local/bin/hangar-native" </dev/null >/dev/null 2>&1 &
  PID_NATIVO=$!
  # Lançar não prova que abriu: um crash na largada sai depois. Com um Hangar já aberto, o novo
  # pode entregar a chamada a ele e sair, então vale qualquer instância viva.
  sleep 2
  if kill -0 "$PID_NATIVO" 2>/dev/null || pgrep -x hangar-native >/dev/null 2>&1; then
    ok "janela do Hangar aberta"
  else
    falta "o app Hangar fechou logo ao abrir — use o Hangar pelo navegador (endereço no resumo abaixo)"
  fi
fi
URL_FIM=$(grep '^CP_PUBLIC_URL=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
# O valor do token só aparece com terminal: sem TTY isto roda em provisionamento e o stdout
# vira log — mesma regra do passo 3/8.
TOKEN_FIM="(está em backend/.env)"
[ "$TEM_TTY" = 1 ] && TOKEN_REAL=$(grep '^CP_AUTH_TOKEN=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
echo
echo "  +---------------------------------------------------------------"
echo "   RESUMO"
# O token de verdade só vai pra TELA — o `tee` grava esta saída no log, e o token não pode ir lá.
if [ "$TEM_TTY" = 1 ]; then printf '   token   : %s\n' "$TOKEN_REAL" > /dev/tty; fi
echo "   token   : $TOKEN_FIM"
echo "   local   : http://127.0.0.1:$PORTA_FIM"
if [ -n "$URL_FIM" ]; then echo "   celular : $URL_FIM"
else echo "   celular : não publicado no Tailscale"; fi
echo "  +---------------------------------------------------------------"
[ -n "$URL_FIM" ] || URL_FIM="http://$(hostname -I 2>/dev/null | awk '{print $1}'):$PORTA_FIM"
QR_MOSTRADO=0
if [ "$TEM_TTY" = 1 ] && [ "$UPDATE" = 0 ]; then
  echo
  # Só no terminal: a URL do QR carrega o token, e o log não pode tê-lo.
  # rc 0 é a prova de que o desenho saiu (2 = sem tty); sem ela a tela mandaria ler um QR que
  # não existe.
  if (cd backend && uv run --quiet --no-sync python -m app.doctor --qr) > /dev/tty 2>/dev/null; then
    QR_MOSTRADO=1
  fi
fi
cat <<EOF

  O QUE FAZER AGORA
EOF
PRIMEIRO_AGENTE=''
for a in $TODOS_AGENTES; do command -v "$a" >/dev/null && { PRIMEIRO_AGENTE=$a; break; }; done
echo "   1. No PC: abra um terminal, digite  ${PRIMEIRO_AGENTE:-claude}  e faça o login (só na primeira vez)."
if [ "$QR_MOSTRADO" = 1 ]; then
  echo "   2. No celular: leia o QR acima (ou abra $URL_FIM e digite o token)."
else
  echo "   2. No celular: abra $URL_FIM e digite o token."
fi
[ "$QUER_TAILSCALE" = 1 ] && echo "   3. No celular: instale o app Tailscale e entre com a MESMA conta do PC."
command -v loginctl >/dev/null && echo "   4. Para o Hangar subir mesmo sem você logar no PC:  sudo loginctl enable-linger \$USER"
cat <<EOF
   Algo não abriu?  hangar-doctor   (diz o que falta e como consertar)
   Log desta instalação: $LOG
   Guia completo: docs/USAGE.md
EOF
FINAL_STATE=ok
