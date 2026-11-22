#!/usr/bin/env bash
# Garante a libmpv 2 do player de vídeo do app nativo (crates/hangar-video a carrega em tempo de execução): no Linux
# pelo gerenciador de pacotes da distro, no macOS pelo Homebrew. Nunca falha: sem ela o app abre o vídeo no player do
# sistema. Com terminal o sudo pergunta a senha; sem terminal (Atualizar, passo de atualização) só `sudo -n`, que
# nunca espera ninguém. A marca ~/.hangar/native/libmpv-ok é a prova do passo de atualização e diz o que achou.
# Uso: scripts/install-libmpv.sh             instala se faltar
#      scripts/install-libmpv.sh --comando   só imprime o comando de instalação (sem sudo), vazio se nada falta
# HANGAR_LIBMPV_SEM_SUDO=1: quem chama já tentou com o sudo dele (install.sh); aqui só confere e marca.
# HANGAR_LIBMPV_SEM_INSTALAR=1: o install.sh --update, que não instala dependência; também sem o Homebrew.
set -uo pipefail

[ -n "${HOME:-}" ] || { echo "vídeo do app nativo: sem HOME, nada a fazer"; exit 0; }
MARCA_DIR="$HOME/.hangar/native"
SO_COMANDO=0; [ "${1:-}" = --comando ] && SO_COMANDO=1
COM_TERMINAL=0; [ -t 0 ] && COM_TERMINAL=1

marca() { [ "$SO_COMANDO" = 1 ] && return 0; mkdir -p "$MARCA_DIR"; printf '%s\n' "$1" > "$MARCA_DIR/libmpv-ok"; }
# O passo de atualização fica feito mesmo com falha (sem terminal não há senha a pedir), então o motivo vai para a
# marca e para o log da instalação; quem o lê roda este script de novo.
aviso() { # aviso <motivo> [comando]
  [ "$SO_COMANDO" = 1 ] && exit 0
  echo "vídeo do app nativo: aviso — $1. O app abre os vídeos no player do sistema."
  [ -n "${2:-}" ] && echo "  Para ter o vídeo dentro do app: $2"
  marca "falta: $1${2:+ (comando: $2)}"
  { mkdir -p "$HOME/.hangar/logs/privado" \
      && printf '%s install-libmpv: falta: %s%s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" "${2:+ (comando: $2)}" \
        >> "$HOME/.hangar/logs/privado/install.log"; } 2>/dev/null || true
  exit 0
}
# Com root quando precisa: com terminal o sudo pergunta a senha, sem terminal só `sudo -n`.
roda() {
  if [ "$(id -u)" = 0 ]; then "$@"
  elif [ "$COM_TERMINAL" = 1 ]; then sudo "$@"
  else sudo -n "$@"
  fi
}

# Só a API 2 serve (o app recusa a 1): o nome `libmpv.so.2` é o que garante isso, não o `libmpv.so` do pacote -dev.
linux_tem() {
  local ld d
  ld=$(command -v ldconfig 2>/dev/null || { [ -x /sbin/ldconfig ] && echo /sbin/ldconfig; } || true)
  if [ -n "$ld" ] && "$ld" -p 2>/dev/null | grep -q 'libmpv\.so\.2 '; then return 0; fi
  for d in /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /usr/local/lib; do
    compgen -G "$d/libmpv.so.2*" >/dev/null && return 0
  done
  return 1
}
# As pastas onde o app procura no macOS (library.rs): o Frameworks do pacote e as do Homebrew.
mac_tem() {
  local d
  for d in "$HOME/Applications/Hangar.app/Contents/Frameworks" /Applications/Hangar.app/Contents/Frameworks /opt/homebrew/lib /usr/local/lib; do
    [ -e "$d/libmpv.2.dylib" ] && return 0
  done
  return 1
}

if [ -n "${HANGAR_LIBMPV:-}" ] && [ -e "$HANGAR_LIBMPV" ]; then
  marca "HANGAR_LIBMPV: $HANGAR_LIBMPV"; exit 0
fi

case "$(uname -s)" in
  Linux)
    if linux_tem; then marca "sistema: libmpv.so.2"; exit 0; fi
    # Pacote que traz a libmpv.so.2 em cada família: no Debian/Ubuntu é o libmpv2 (o libmpv1 é a API 1);
    # no Fedora o mpv-libs vem do RPM Fusion, que nem toda máquina tem.
    APT=0
    if command -v pacman >/dev/null 2>&1; then
      # Sem --noconfirm o [S/n] do pacman espera quem não está lá (Atualizar, --app); a senha do sudo já é a confirmação.
      CMD="pacman -S --needed --noconfirm mpv"
    elif command -v apt-get >/dev/null 2>&1; then
      # O install.sh só pergunta a senha por um pacote que a lista conhece.
      [ "$SO_COMANDO" = 1 ] && ! apt-cache show libmpv2 >/dev/null 2>&1 && exit 0
      # Sem o frontend não interativo, uma pergunta do debconf espera quem não está lá.
      CMD="env DEBIAN_FRONTEND=noninteractive apt-get install -y libmpv2"; APT=1
    elif command -v dnf >/dev/null 2>&1; then
      CMD="dnf install -y mpv-libs"
    else
      aviso "não sei o pacote da libmpv 2 nesta distro" "instale a libmpv.so.2 (pacote do mpv) pelo gerenciador da distro"
    fi
    [ "$SO_COMANDO" = 1 ] && { echo "$CMD"; exit 0; }
    { [ "${HANGAR_LIBMPV_SEM_SUDO:-}" = 1 ] || [ "${HANGAR_LIBMPV_SEM_INSTALAR:-}" = 1 ]; } \
      && aviso "a libmpv 2 não está instalada" "sudo $CMD"
    [ "$(id -u)" = 0 ] || command -v sudo >/dev/null 2>&1 || aviso "sem sudo para instalar a libmpv 2" "$CMD (como root)"
    echo "vídeo do app nativo: instalando a libmpv 2 ($CMD)"
    # $CMD sem aspas de propósito: só palavras fixas, sem espaço dentro de nenhuma.
    roda $CMD; rc=$?
    # A lista de pacotes velha não conhece o libmpv2: refaz a lista uma vez, só nesse caso, e tenta de novo.
    if [ "$rc" != 0 ] && [ "$APT" = 1 ] && ! apt-cache show libmpv2 >/dev/null 2>&1; then
      echo "vídeo do app nativo: o apt não conhece o libmpv2; atualizando a lista de pacotes"
      roda env DEBIAN_FRONTEND=noninteractive apt-get update && { roda $CMD; rc=$?; }
      apt-cache show libmpv2 >/dev/null 2>&1 \
        || aviso "esta distro não tem o pacote libmpv2 (o libmpv1 é a API 1, que o app não usa)"
    fi
    linux_tem && { marca "instalada: $CMD"; echo "vídeo do app nativo: libmpv 2 instalada"; exit 0; }
    case "$CMD" in
      dnf*) aviso "o dnf não instalou o mpv-libs (código $rc; ele vem do RPM Fusion)" "habilite o RPM Fusion e rode: sudo $CMD" ;;
      # Lista de pacotes velha dá 404 numa dependência; só -Sy seria atualização parcial, que o Arch não suporta.
      pacman*) aviso "o pacman não instalou o mpv (código $rc; com a lista de pacotes velha é preciso atualizar o sistema)" "sudo pacman -Syu mpv" ;;
      *) aviso "a instalação da libmpv 2 não terminou (código $rc)" "sudo $CMD" ;;
    esac
    ;;
  Darwin)
    [ "$SO_COMANDO" = 1 ] && exit 0
    if mac_tem; then marca "sistema: libmpv.2.dylib"; exit 0; fi
    BREW=$(command -v brew 2>/dev/null || true)
    for b in /opt/homebrew/bin/brew /usr/local/bin/brew; do [ -z "$BREW" ] && [ -x "$b" ] && BREW=$b; done
    [ -n "$BREW" ] || aviso "a libmpv 2 não está instalada e não há Homebrew" "instale o Homebrew e rode: brew install mpv"
    [ "${HANGAR_LIBMPV_SEM_INSTALAR:-}" = 1 ] && aviso "a libmpv 2 não está instalada" "brew install mpv"
    echo "vídeo do app nativo: instalando o mpv pelo Homebrew"
    # brew nunca com sudo. Sem a atualização automática ele não para minutos baixando o índice inteiro antes.
    HOMEBREW_NO_AUTO_UPDATE=1 "$BREW" install mpv; rc=$?
    mac_tem && { marca "instalada: brew install mpv"; echo "vídeo do app nativo: libmpv 2 instalada"; exit 0; }
    aviso "o brew install mpv não terminou (código $rc)" "brew install mpv"
    ;;
  *)
    marca "sem libmpv para $(uname -s)"; exit 0
    ;;
esac
