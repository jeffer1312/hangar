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
# Cada bloco sai com as linhas vazias em volta, que viram uma quebra só. Bloco sem o fim deixa o
# arquivo como estava.
tira_bloco() {
  local file=$1 mark=$2 tag=$3 tmp
  [ -f "$file" ] && grep -qF -- "-- ${mark}:${tag}:start" "$file" || return 0
  tmp=$(mktemp "$file.XXXXXX") || return 1
  if ! START="-- ${mark}:${tag}:start" END="-- ${mark}:${tag}:end" awk '
    BEGIN { start = ENVIRON["START"]; end = ENVIRON["END"] }
    inside { if (index($0, end)) { inside = 0; after = 1 }; next }
    index($0, start) { inside = 1; blanks = 0; if (!printed) lead = 1; next }
    $0 == "" { if (!after) blanks++; next }
    {
      if (lead) { print ""; lead = 0 }
      for (; blanks > 0; blanks--) print ""
      print; printed = 1; after = 0
    }
    END {
      if (inside) exit 3
      if (lead) print ""
      if (!after) for (; blanks > 0; blanks--) print ""
    }' "$file" > "$tmp"; then
    rm -f "$tmp"
    echo "painel: $file tem -- ${mark}:${tag}:start sem -- ${mark}:${tag}:end; bloco deixado" >&2
    return 1
  fi
  # `cat >` e não `mv`: o arquivo continua o mesmo (link, dono e modo ficam).
  cat "$tmp" > "$file" && rm -f "$tmp" && echo "painel: bloco $mark:$tag removido de $file"
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
