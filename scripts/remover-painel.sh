#!/usr/bin/env bash
# Tira o painel do Quickshell (e o ícone de bandeja dele) das máquinas que o instalaram com o antigo
# install-hangar-panel.sh. Os scripts saíram do repositório: sem este passo, os dois serviços ficariam
# reiniciando para sempre contra links quebrados. Roda pelo passo de atualização.
#
# Nunca falha: passo que falha ou não deixa a prova derruba o Atualizar inteiro. O que não saiu fica
# escrito na prova e no log. O ~/.config/hangar-panel.json e o backend/projects.json ficam (são do
# usuário). Uso: scripts/remover-painel.sh
set -uo pipefail
[ -n "${HOME:-}" ] || { echo "painel: sem HOME, nada removido"; exit 0; }

MARCA="$HOME/.hangar/painel-removido"
marca() { mkdir -p "$(dirname "$MARCA")"; printf '%s\n' "$1" > "$MARCA"; }

if [ "$(uname -s)" != Linux ]; then
  marca "nao e Linux: nada removido"; exit 0
fi

sobrou=()
SD="$HOME/.config/systemd/user"
for u in hangar-panel.service hangar-panel-tray.service cp-panel.service cp-panel-tray.service; do
  [ -f "$SD/$u" ] || continue
  systemctl --user disable --now "$u" >/dev/null 2>&1 || sobrou+=("unit $u ainda ligada até o próximo boot")
  # Sai mesmo sem o disable: no boot seguinte a unit não existe e o serviço não volta.
  rm -f "$SD/$u"
  echo "painel: unit $u removida"
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
# `claude-pocket:` é o marcador de antes da troca de nome.
tira_bloco() {
  local file=$1 mark=$2 tag=$3
  [ -f "$file" ] && grep -qF -- "-- ${mark}:${tag}:start" "$file" || return 0
  MARK="$mark" TAG="$tag" python3 - "$file" <<'PYEOF' && echo "painel: bloco $mark:$tag removido de $file"
import os, re, sys
path, mark, tag = sys.argv[1], os.environ["MARK"], os.environ["TAG"]
text = open(path, encoding="utf-8").read()
start, end = f"-- {mark}:{tag}:start", f"-- {mark}:{tag}:end"
pattern = re.compile(r"\n*" + re.escape(start) + r".*?" + re.escape(end) + r"\n*", re.S)
new, n = pattern.subn("\n", text)
if not n:
    sys.exit(f"painel: {path} tem {start} sem {end}; bloco deixado")
open(path, "w", encoding="utf-8").write(new)
PYEOF
}
H="$HOME/.config/hypr/custom"
for mark in hangar claude-pocket; do
  tira_bloco "$H/keybinds.lua" "$mark" panel-bind || sobrou+=("bloco $mark:panel-bind em $H/keybinds.lua")
  tira_bloco "$H/execs.lua" "$mark" panel-exec || sobrou+=("bloco $mark:panel-exec em $H/execs.lua")
  tira_bloco "$H/rules.lua" "$mark" panel-blur || sobrou+=("bloco $mark:panel-blur em $H/rules.lua")
done

if [ ${#sobrou[@]} -gt 0 ]; then
  marca "painel removido, mas sobrou: $(IFS=';'; echo "${sobrou[*]}")"
  printf 'painel: sobrou %s\n' "${sobrou[@]}"
  exit 0
fi
marca "painel removido"
echo "painel: concluído"
exit 0
