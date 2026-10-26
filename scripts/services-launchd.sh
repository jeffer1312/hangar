#!/usr/bin/env bash
# Backend do Hangar como LaunchAgent do macOS: o par do services-setup.sh, que é só do Linux.
#
# Sobe no login, volta sozinho quando cai e não leva as sessões junto quando reinicia. Só o
# backend: no macOS ele mesmo serve a interface (frontend/dist) no 8765.
#
# Uso:
#   ./scripts/services-launchd.sh             # instala e sobe (idempotente; só reinicia se o plist mudou)
#   ./scripts/services-launchd.sh --restart   # reinicia o backend
#   ./scripts/services-launchd.sh --status    # estado e pid
#   ./scripts/services-launchd.sh --ativo     # sai 0 só com o backend de pé (prova do instalador)
#   ./scripts/services-launchd.sh --uninstall # para e remove o serviço
#
# A configuração continua no backend/.env, como no `uv run python -m app.main` na mão.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL=br.dev.hangar.backend
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMINIO="gui/$(id -u)"
ALVO="$DOMINIO/$LABEL"
LOG_DIR="$HOME/.hangar/logs"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

carregado() { launchctl print "$ALVO" >/dev/null 2>&1; }
pid_atual() { launchctl print "$ALVO" 2>/dev/null | awk '$1 == "pid" && $2 == "=" {print $3; exit}'; }
PORTA=$(grep '^CP_PORT=' "$REPO/backend/.env" 2>/dev/null | tail -1 | cut -d= -f2- || true)
PORTA=${PORTA:-8765}
# Pid sozinho não prova nada: num ciclo de queda o launchd sempre tem um processo novo subindo.
# De pé = o serviço tem processo E o backend responde, por até 60 s (a subida leva uns segundos).
ativo() {
  local _
  for _ in $(seq 1 60); do
    [ -n "$(pid_atual)" ] && curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$PORTA/" && return 0
    sleep 1
  done
  return 1
}

case "${1:-}" in
  --restart) launchctl kickstart -k "$ALVO"; exit ;;
  --ativo) ativo; exit ;;
  --status)
    if carregado; then
      launchctl print "$ALVO" | awk '$1 == "state" || $1 == "pid" || /last exit code/'
    else
      echo "serviço $LABEL não carregado"
    fi
    exit 0 ;;
  --uninstall)
    launchctl bootout "$ALVO" 2>/dev/null || true
    rm -f "$PLIST"
    log "serviço $LABEL removido"
    exit 0 ;;
  "") ;;
  *) echo "uso: $0 [--restart|--status|--ativo|--uninstall]" >&2; exit 2 ;;
esac

# O Python do venv direto, sem `uv run` no meio: o sinal de parar do launchd chega ao próprio
# backend, que encerra o hangar-server filho. Com o uv no meio, matar o uv deixava o backend órfão.
PY_BIN="$REPO/backend/.venv/bin/python"
[ -x "$PY_BIN" ] || { echo "venv do backend não encontrado — rode: cd backend && uv sync" >&2; exit 1; }
# O agente nasce sem o PATH do shell: o backend abre `claude`, `tmux`, `node` e `tailscale`.
PATH_SERVICO="$HOME/.local/bin"
if command -v node >/dev/null; then PATH_SERVICO="$PATH_SERVICO:$(dirname "$(command -v node)")"; fi
PATH_SERVICO="$PATH_SERVICO:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
# Sem locale o tmux e o Claude desenham acento e símbolo como `_`.
LOCALE="${LC_ALL:-${LANG:-en_US.UTF-8}}"

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

plist() {
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml "$PY_BIN")</string><string>-m</string><string>app.main</string>
  </array>
  <key>WorkingDirectory</key><string>$(xml "$REPO/backend")</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>$(xml "$PATH_SERVICO")</string>
    <key>LANG</key><string>$(xml "$LOCALE")</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <!-- Volta só quando sai com erro, como o Restart=on-failure da unit do Linux. -->
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>5</integer>
  <!-- Sem AbandonProcessGroup de propósito: o launchd mata o grupo do backend ao pará-lo, e
       nenhum Python órfão fica segurando a porta. O tmux e o hangar-server têm grupo próprio
       e não vão junto. -->
  <key>StandardOutPath</key><string>$(xml "$LOG_DIR/backend-launchd.log")</string>
  <key>StandardErrorPath</key><string>$(xml "$LOG_DIR/backend-launchd.log")</string>
</dict>
</plist>
EOF
}

mkdir -p "$(dirname "$PLIST")" "$LOG_DIR"
NOVO="$(plist)"
plutil -lint - <<<"$NOVO" >/dev/null || { echo "plist gerado inválido" >&2; exit 1; }

# Reiniciar sem necessidade derruba as conexões do celular: só mexe no que mudou.
if [ -f "$PLIST" ] && [ "$(cat "$PLIST")" = "$NOVO" ]; then
  if carregado; then log "$LABEL sem mudança"; exit 0; fi
else
  printf '%s\n' "$NOVO" > "$PLIST"
  log "Gravado $PLIST"
  launchctl bootout "$ALVO" 2>/dev/null || true
fi

# Backend rodando na mão segura a porta: o serviço entraria em ciclo de queda sem dizer por quê.
# A espera cobre o backend que o bootout acabou de parar, que ainda solta a porta.
DONO=''
for _ in 1 2 3 4 5 6 7 8 9 10; do
  DONO="$(lsof -nP -tiTCP:"$PORTA" -sTCP:LISTEN 2>/dev/null | head -1 || true)"
  [ -z "$DONO" ] && break
  sleep 1
done
if [ -n "$DONO" ]; then
  echo "a porta $PORTA já está em uso (pid $DONO) — pare o backend aberto na mão e rode de novo" >&2
  exit 1
fi
launchctl bootstrap "$DOMINIO" "$PLIST"
log "$LABEL carregado"
