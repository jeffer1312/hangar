#!/usr/bin/env bash
# Tira o painel do Quickshell (e o ícone de bandeja dele) das máquinas que o instalaram com o antigo
# install-hangar-panel.sh. Os scripts saíram do repositório: sem este passo, os dois serviços ficariam
# reiniciando para sempre contra links quebrados. Roda pelo passo de atualização.
#
# Nunca falha: passo que falha derruba o Atualizar inteiro. O ~/.config/hangar-panel.json e o
# backend/projects.json ficam (são do usuário). Uso: scripts/remover-painel.sh
set -uo pipefail
[ -n "${HOME:-}" ] || { echo "painel: sem HOME, nada removido"; exit 0; }

MARCA="$HOME/.hangar/painel-removido"
marca() { mkdir -p "$(dirname "$MARCA")"; printf '%s\n' "$1" > "$MARCA"; }

if [ "$(uname -s)" != Linux ]; then
  marca "nao e Linux: nada removido"; exit 0
fi

# Sem a marca o passo fica pendente e roda de novo na próxima atualização.
falhou=0
SD="$HOME/.config/systemd/user"
for u in hangar-panel.service hangar-panel-tray.service cp-panel.service cp-panel-tray.service; do
  [ -f "$SD/$u" ] || continue
  # A unit só sai depois do disable: apagada antes, a próxima rodada não a acharia para desligar.
  if systemctl --user disable --now "$u" >/dev/null 2>&1; then
    rm -f "$SD/$u"
    echo "painel: unit $u removida"
  else
    falhou=1
    echo "painel: não consegui desligar $u"
  fi
done
command -v systemctl >/dev/null && systemctl --user daemon-reload >/dev/null 2>&1 || true
# Âncora no início: sem ela o padrão casa qualquer shell cuja linha de comando cite o texto.
pkill -f '^qs -n -c hangar( |$)' 2>/dev/null || true

for p in hangar cp; do
  for s in data open tray action; do
    l="$HOME/.local/bin/$p-panel-$s"
    [ -L "$l" ] && rm -f "$l" && echo "painel: link $l removido"
  done
done
[ -L "$HOME/.config/quickshell/hangar" ] && rm -f "$HOME/.config/quickshell/hangar" && echo "painel: link do quickshell removido"

# Blocos que o instalador gravou entre marcadores no Hyprland; o resto do arquivo fica intacto.
tira_bloco() {
  local file=$1 tag=$2
  [ -f "$file" ] && grep -qF -- "-- hangar:${tag}:start" "$file" || return 0
  TAG="$tag" python3 - "$file" <<'PYEOF' && echo "painel: bloco $tag removido de $file"
import os, re, sys
path, tag = sys.argv[1], os.environ["TAG"]
text = open(path, encoding="utf-8").read()
start, end = f"-- hangar:{tag}:start", f"-- hangar:{tag}:end"
pattern = re.compile(r"\n*" + re.escape(start) + r".*?" + re.escape(end) + r"\n*", re.S)
open(path, "w", encoding="utf-8").write(pattern.sub("\n", text))
PYEOF
}
tira_bloco "$HOME/.config/hypr/custom/keybinds.lua" panel-bind || falhou=1
tira_bloco "$HOME/.config/hypr/custom/execs.lua" panel-exec || falhou=1

if [ "$falhou" = 1 ]; then
  echo "painel: remoção incompleta, tenta de novo na próxima atualização"; exit 0
fi
marca "painel removido"
echo "painel: concluído"
exit 0
