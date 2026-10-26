#!/usr/bin/env bash
# hangar — instalar numa linha só, no Linux/macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.sh | bash
#
# Por padrão instala o PACOTE pronto do canal `latest`, sem git: baixa o `hangar-server` da release
# `server-<canal>`, confere o sha256 pelo `server-latest.json` dela, roda o `install-package` dele
# (que põe o pacote em ~/.hangar/current) e depois o install.sh que vem dentro do pacote.
#
#   --beta   canal de teste (`server-beta`/`package-beta`; no --dev, a branch beta)
#   --dev    instala pelo git como sempre foi (clona e roda o install.sh do checkout); a máquina fica
#            em modo desenvolvedor quando a migração para o pacote chegar
#
# Sistema sem pacote publicado (macOS, hoje) segue pelo git, com o motivo na tela.
#
# Com argumentos: sob `curl | bash` eles vão DEPOIS de `-s --`, senão quem os come é o próprio
# bash, não este script.
#
#   curl -fsSL .../bootstrap.sh | bash -s -- --beta
#   curl -fsSL .../bootstrap.sh | bash -s -- --dev ~/apps/hangar --no-frontend
#
# Uma pasta como PRIMEIRO argumento (que não comece com '-') é o destino do checkout e vale o git,
# como `--dev` (default do checkout: $HOME/hangar). Tudo o que sobra, menos --dev e --beta, vai
# inteiro pro install.sh.
#
# O assistente do app nativo roda este arquivo com a pasta do Hangar que encontrou (quando há uma)
# e --app; aí as falhas daqui também saem marcadas (##HANGAR-ERRO##, ##HANGAR-FIM##) e o install.sh
# nunca recebe o terminal.
#
# Instale num disco LOCAL. Numa pasta compartilhada por rede (Samba/NFS montado de outra
# máquina) o `uv sync` e o `npm ci` recriariam `backend/.venv` e `frontend/node_modules` por
# cima dos da máquina de origem — e esses são dela, não seus: o venv aponta pro
# `/usr/bin/python3.14` de lá e o node_modules traz o binário `@esbuild/linux-x64` dela.
# Instalar de uma segunda máquina quebra a instalação da primeira.
set -euo pipefail

REPO_URL="https://github.com/jeffer1312/hangar.git"
# HANGAR_RELEASE_BASE só existe para o teste servir releases falsas.
RELEASES=${HANGAR_RELEASE_BASE:-https://github.com/jeffer1312/hangar/releases/download}
# Endereço https só baixa por https, nem por redirecionamento; o teste serve as releases por file://.
PROTO=''; case $RELEASES in https://*) PROTO='--proto =https' ;; esac
RAMO="main"
CANAL=latest
APP=0; DEV=0; CHECK=0
ARGS=()
for a in "$@"; do
  case $a in
    --dev)  DEV=1 ;;
    --beta) CANAL=beta; RAMO=beta ;;
    *)      ARGS+=("$a") ;;
  esac
  case $a in --app) APP=1 ;; --check) CHECK=1 ;; esac
done
# O `+` protege o array vazio do `set -u` no bash 3 do macOS.
set -- ${ARGS[@]+"${ARGS[@]}"}
# Senha do celular e auxiliar de administrador do app: só o install.sh as recebe, não o git.
APP_TOKEN=${HANGAR_TOKEN-}; APP_ASKPASS=${HANGAR_ASKPASS-}; APP_ASKPASS_CODE=${HANGAR_ASKPASS_CODE-}
unset HANGAR_TOKEN HANGAR_ASKPASS HANGAR_ASKPASS_CODE
# Só esta execução diz ao install.sh que acabou de criar a instalação: herdada, marcaria passos pendentes.
unset HANGAR_FRESH_PACKAGE

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
nota() { printf '      \033[2m%s\033[0m\n' "$*"; }
fail() {
  printf '  \033[31mX\033[0m   %s\n' "$*" >&2
  # No --app a última linha diz ao assistente que parou aqui, antes do install.sh.
  [ "$APP" = 1 ] && echo "##HANGAR-FIM## falhou"
  exit 1
}
# O código vem logo antes da falha, também no terminal: o app troca o texto pela frase e pelo botão.
fail_with() { # fail_with <código ou vazio> <mensagem>
  [ -n "$1" ] && echo "##HANGAR-ERRO## $1"
  shift; fail "$*"
}
# Vazio quando a internet responde: aí a falha é de outra coisa.
net_code() {
  command -v curl >/dev/null || return 0
  curl --proto '=https' -fsS --max-time 10 -o /dev/null https://github.com 2>/dev/null || echo sem-internet
}

DEST="$HOME/hangar"
PASTA=0
case "${1-}" in
  ''|-*) ;;               # sem argumento, ou o primeiro já é flag do install.sh
  *)     DEST=$1; PASTA=1; shift ;;
esac
PACOTE="$HOME/.hangar/current"
FRESH_MARK="$HOME/.hangar/fresh-package"

# Roda o install.sh de <pasta> com as opções do usuário; não volta.
roda_instalador() {
  local dir=$1; shift
  say "Instalando: $dir/install.sh $*"
  cd "$dir"
  [ -n "$APP_TOKEN" ] && export HANGAR_TOKEN=$APP_TOKEN
  [ -n "$APP_ASKPASS" ] && export HANGAR_ASKPASS=$APP_ASKPASS
  [ -n "$APP_ASKPASS_CODE" ] && export HANGAR_ASKPASS_CODE=$APP_ASKPASS_CODE
  # Sob `curl | bash` o stdin DESTE script é o cano do curl, e o install.sh herdaria isso: os
  # `read` dele leriam EOF na hora. Aqui entregamos o terminal de verdade. (O install.sh também
  # se defende sozinho — isto é o cinto além do suspensório.) Sem terminal, ele cai no default
  # seguro por conta própria. No --app nunca: quem responde é o assistente.
  if [ "$APP" = 0 ] && { : </dev/tty; } 2>/dev/null; then
    exec ./install.sh "$@" </dev/tty
  else
    exec ./install.sh "$@"
  fi
}

# O instalador do pacote; com o marcador presente, a instalação ainda não marcou os passos de atualização
# (a conferência nunca marca, e uma instalação pode ter parado antes). Não volta.
roda_instalador_do_pacote() {
  [ ! -e "$FRESH_MARK" ] || export HANGAR_FRESH_PACKAGE=1
  roda_instalador "$PACOTE" "$@"
}

# Mesma detecção do `detecta_pkg` do install.sh, e duplicada de propósito: aqui o repositório
# ainda NÃO existe, então não há de onde importá-la. Vazio = gerenciador não reconhecido.
sugere_git() {
  for p in pacman apt-get dnf zypper apk brew; do
    command -v "$p" >/dev/null || continue
    case "$p" in
      pacman)  echo "sudo pacman -S --needed git" ;;
      apt-get) echo "sudo apt-get install -y git" ;;
      dnf)     echo "sudo dnf install -y git" ;;
      zypper)  echo "sudo zypper install -y git" ;;
      apk)     echo "sudo apk add git" ;;
      brew)    echo "brew install git" ;;   # brew nunca com sudo, de propósito
    esac
    return
  done
}

# 0 = o remoto desta pasta é o hangar. Checar o REMOTO, não só a existência da pasta:
# "tem um .git aqui" não quer dizer que é este projeto, e dar pull no repo errado é pior que
# parar.
mesma_origem() {
  local url
  url=$(git -C "$1" remote get-url origin 2>/dev/null) || return 1
  url=${url%/}; url=${url%.git}
  case "$url" in
    *github.com[:/]jeffer1312/hangar) return 0 ;;
  esac
  return 1
}

clona() {
  say "Clonando em $DEST"
  git clone --branch "$RAMO" "$REPO_URL" "$DEST" || fail_with "$(net_code)" "git clone falhou"
  ok "clonado"
}

# O caminho pelo git: clona (ou atualiza) o checkout e roda o install.sh dele. Não volta.
pelo_git() {
  if [ -f "$PACOTE/manifest.json" ]; then
    # O serviço segue chamando o pacote: o install.sh de um checkout por cima misturaria as versões.
    fail "esta máquina já roda o Hangar pelo pacote; para usar um checkout: hangar-server dev-mode on <checkout>"
  fi
  if ! command -v git >/dev/null; then
    cmd=$(sugere_git)
    if [ -n "$cmd" ]; then nota "instale com:  $cmd"
    else nota "instale o git pelo gerenciador de pacotes do teu sistema"; fi
    fail "git é obrigatório — sem ele não há como clonar o repositório"
  fi
  ok "git $(git --version | awk '{print $3}')"

  if [ ! -e "$DEST" ]; then
    clona
  elif [ ! -d "$DEST" ]; then
    fail "$DEST existe e não é uma pasta — escolha outro destino"
  elif mesma_origem "$DEST"; then
    ok "$DEST já é este repositório — atualizando em vez de clonar"
    # No --app o pull não roda o hook post-merge: ele faria um --update inteiro, sem marcas, antes do
    # ##HANGAR-PROTOCOLO##; o install.sh --app logo abaixo já faz o mesmo trabalho.
    sem_hooks=''; [ "$APP" = 1 ] && sem_hooks='-c core.hooksPath=/dev/null'
    # shellcheck disable=SC2086  # de propósito: vazio some, cheio vira dois argumentos
    if ! git $sem_hooks -C "$DEST" pull --ff-only origin "$RAMO"; then
      # Mudança local é o caso comum, e tem frase e botão próprios no app.
      if [ -n "$(git -C "$DEST" status --porcelain --untracked-files=no 2>/dev/null)" ]; then
        fail_with checkout-sujo "git pull falhou em $DEST: há mudança local — resolva na mão e rode de novo"
      fi
      fail_with "$(net_code)" "git pull falhou em $DEST — resolva na mão e rode de novo"
    fi
    ok "atualizado"
  elif [ -n "$(ls -A "$DEST" 2>/dev/null)" ]; then
    nota "outro destino: bootstrap.sh ~/apps/hangar"
    fail "$DEST já existe e NÃO é o hangar — não vou mexer no que é seu"
  else
    clona   # pasta existe mas está vazia: o git clona pra dentro dela
  fi
  # A migração para o pacote lê esta marca e deixa a máquina em modo desenvolvedor.
  if [ "$DEV" = 1 ]; then mkdir -p "$HOME/.hangar" && : > "$HOME/.hangar/dev-requested"; fi
  roda_instalador "$DEST" "$@"
}

# Sistema sem pacote: o motivo na tela e o git.
sem_pacote() { # sem_pacote <motivo> [opções]
  local motivo=$1; shift
  say "Sem pacote: $motivo"
  nota "instalando pelo git"
  pelo_git "$@"
}

plataforma() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64|Linux-amd64) echo linux-x86_64 ;;
    Darwin-arm64) echo macos-aarch64 ;;
  esac
}

sha256_de() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

JA_INSTALADO="já há uma instalação do Hangar nesta máquina: para atualizar, use o Atualizar do app; para seguir pelo git, rode com --dev"

TMP=''
limpa_tmp() { [ -z "$TMP" ] || rm -rf "$TMP"; TMP=''; }

# O caminho pelo pacote. Não volta.
pelo_pacote() {
  local plat url manifesto entrada nome sha recebido rc indice
  # Re-rodar continua de onde parou: o pacote já instalado roda o instalador dele de novo.
  if [ -f "$PACOTE/manifest.json" ] && [ ! -e "$PACOTE/.git" ]; then
    ok "o Hangar já está instalado pelo pacote — rodando o instalador dele de novo (atualizar é pelo app)"
    local instalado
    instalado=$(tr -d ' \t\r\n' < "$PACOTE/manifest.json" | sed -n 's/.*"channel":"\([A-Za-z0-9._-]*\)".*/\1/p')
    # Não há troca de canal: o atualizador segue o canal do manifesto instalado.
    [ -z "$instalado" ] || [ "$instalado" = "$CANAL" ] \
      || nota "instalado no canal $instalado (pedido: $CANAL); o canal instalado é mantido"
    roda_instalador_do_pacote "$@"
  fi
  # Checkout instalado e nunca iniciado (--no-services) não deixa unit nem anotação para o install-package ver.
  if [ -d "$DEST" ] && mesma_origem "$DEST"; then fail "o Hangar está em $DEST, pelo git; $JA_INSTALADO"; fi
  plat=$(plataforma)
  [ -n "$plat" ] || sem_pacote "sem pacote para $(uname -s) $(uname -m)" "$@"
  command -v curl >/dev/null || fail "curl é obrigatório para baixar o pacote"

  # O índice antes do hangar-server: num canal sem pacote, a release server-<canal> pode ter um binário
  # de antes do `install-package`, que subiria como servidor em vez de dizer o motivo.
  url="$RELEASES/package-$CANAL/package-$CANAL.json"
  rc=0
  # shellcheck disable=SC2086
  indice=$(curl $PROTO -fsSL --max-time 60 -w '\n%{http_code}' "$url" 2>/dev/null) || rc=$?
  if [ "$rc" = 22 ] && [ "${indice##*$'\n'}" = 404 ]; then
    sem_pacote "o canal $CANAL ainda não publica pacote ($url não existe)" "$@"
  fi
  [ "$rc" = 0 ] || fail_with "$(net_code)" "não consegui ler $url (curl $rc, HTTP ${indice##*$'\n'})"
  printf '%s' "${indice%$'\n'*}" | tr -d ' \t\r\n' | grep -q "\"$plat\":{" \
    || sem_pacote "o canal $CANAL não tem pacote para $plat" "$@"

  say "Baixando o Hangar (canal $CANAL)"
  url="$RELEASES/server-$CANAL"
  # shellcheck disable=SC2086  # PROTO vazio some, cheio vira dois argumentos
  manifesto=$(curl $PROTO -fsSL --max-time 60 "$url/server-latest.json") \
    || fail_with "$(net_code)" "não consegui ler $url/server-latest.json"
  # O manifesto é JSON indentado com objetos rasos por arquivo: sem espaço e quebra, cada entrada é `"<sistema>/<nome>":{…}`.
  entrada=$(printf '%s' "$manifesto" | tr -d ' \t\r\n' | grep -o "\"$plat/hangar-server\":{[^}]*}" || true)
  nome=$(printf '%s' "$entrada" | sed -n 's/.*"name":"\([A-Za-z0-9._-]*\)".*/\1/p')
  sha=$(printf '%s' "$entrada" | sed -n 's/.*"sha256":"\([0-9a-fA-F]\{64\}\)".*/\1/p' | tr 'A-F' 'a-f')
  [ -n "$entrada" ] || sem_pacote "a release server-$CANAL não publica o hangar-server para $plat" "$@"
  if [ -z "$nome" ] || [ -z "$sha" ]; then
    fail "a release server-$CANAL tem o hangar-server para $plat, mas não consegui ler o nome e o sha256 da entrada: $entrada"
  fi

  # Em ~/.hangar e não no /tmp: o /tmp montado com noexec não roda o binário.
  mkdir -p "$HOME/.hangar/cache"
  TMP=$(mktemp -d "$HOME/.hangar/cache/bootstrap.XXXXXX")
  trap limpa_tmp EXIT
  # shellcheck disable=SC2086
  curl $PROTO -fsSL --max-time 600 -o "$TMP/hangar-server" "$url/$nome" || fail_with "$(net_code)" "o download de $nome falhou"
  recebido=$(sha256_de "$TMP/hangar-server")
  [ "$recebido" = "$sha" ] || fail "o hangar-server baixado não confere com o publicado (sha256 $recebido, esperado $sha)"
  chmod +x "$TMP/hangar-server"
  ok "hangar-server conferido (sha256)"

  nota "baixando e conferindo o pacote (pode levar alguns minutos)"
  set +e
  "$TMP/hangar-server" install-package --channel "$CANAL" --release-base "$RELEASES" 2>&1 | tee "$TMP/saida"
  rc=${PIPESTATUS[0]}
  set -e
  case $rc in
    0) ;;
    # 3 = o índice não tem pacote para este sistema; a última linha é o motivo.
    3) local motivo; motivo=$(tail -n 1 "$TMP/saida"); limpa_tmp; sem_pacote "$motivo" "$@" ;;
    4) fail "$JA_INSTALADO" ;;
    *) fail "a instalação do pacote parou (motivo acima)" ;;
  esac
  limpa_tmp
  # Instalação criada agora: o marcador fica até o install.sh marcar os passos como já aplicados.
  : > "$FRESH_MARK"
  roda_instalador_do_pacote "$@"
}

say "hangar — instalação em uma linha"
if [ "$DEV" = 1 ] || [ "$PASTA" = 1 ]; then
  pelo_git "$@"
else
  pelo_pacote "$@"
fi
