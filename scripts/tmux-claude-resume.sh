#!/usr/bin/env bash
# hangar — o lado tmux-resurrect do reboot.
#
# As sessões do Hangar (as que têm `CP_SESSION_KEY` no ambiente da sessão tmux) voltam depois do
# reboot pelo servidor do Hangar: ele guarda quais estão abertas e em que conversa, e na subida reabre
# cada uma pelo spec gravado, com o comando direto no `new-session` (session_open/restore.rs). O
# resurrect não as recria: o `save` as tira da foto dele, senão elas voltariam como shell nu com o
# nome tomado. O resto da foto (sessões de shell que não são do Hangar) o resurrect restaura como sempre.
#
# Ligado pelos hooks do resurrect (scripts/tmux-persist-setup.sh):
#   @resurrect-hook-post-save-all     -> tmux-claude-resume.sh save
#   @resurrect-hook-post-restore-all  -> tmux-claude-resume.sh restore   (só registra: o restore é do servidor)
#   run-shell fix-last antes do TPM, e o hook session-closed -> closed
#
# Usage: tmux-claude-resume.sh save | restore | fix-last | closed
set -euo pipefail

DIR="${TMUX_RESURRECT_DIR:-$HOME/.local/share/tmux/resurrect}"
LOG="$HOME/.hangar/logs/privado/claude-resume.log"
if [[ ${OS:-} == Windows_NT ]]; then
  LOG="${LOCALAPPDATA:-${USERPROFILE:-$HOME}/AppData/Local}/hangar/logs/privado/claude-resume.log"
fi
(
  umask 077
  mkdir -p "$(dirname "$LOG")"
  touch "$LOG"
)

# As sessões que o servidor restaura: a chave do spec no ambiente da sessão e o spec gravado. Sem o
# spec (aberta antes do registro) o servidor não tem como reabri-la, e ela fica com o resurrect.
# As internas (`@cp_hidden`: terminal de atalho, shell separado, janela de login) também saem:
# recriadas cruas, perdem a marca e entram na lista como sessão Claude sem Claude.
_hangar_sessions() {
  local name hidden key
  while IFS=$'\t' read -r name hidden; do
    if [ "$hidden" = 1 ]; then
      printf '%s\n' "$name"
      continue
    fi
    key=$(tmux show-environment -t "=$name" CP_SESSION_KEY 2>/dev/null || true)
    if [[ $key =~ ^CP_SESSION_KEY=([0-9a-fA-F]{32})$ ]] && [ -f "$HOME/.hangar/session-specs/${BASH_REMATCH[1]}.json" ]; then
      printf '%s\n' "$name"
    fi
  done < <(tmux list-sessions -F $'#{session_name}\t#{@cp_hidden}' 2>/dev/null || true)
}

# Tira as sessões do Hangar da foto que o resurrect acabou de gravar (linhas `pane`/`window` delas).
save() {
  local last file names tmp
  last="$DIR/last"
  file=$(readlink -f "$last" 2>/dev/null) || return 0
  [ -s "$file" ] || return 0
  names=$(_hangar_sessions)
  [ -n "$names" ] || return 0
  # tmp no MESMO fs: o mv é rename atômico; sync porque o btrfs só grava os dados no commit.
  tmp=$(mktemp "$DIR/.hangar-save.XXXXXX")
  awk -F'\t' -v names="$names" 'BEGIN { n = split(names, list, "\n"); for (i = 1; i <= n; i++) skip[list[i]] = 1 }
    !(($1 == "pane" || $1 == "window") && ($2 in skip))' "$file" > "$tmp"
  sync "$tmp"
  mv "$tmp" "$file"
  sync "$file" 2>/dev/null || true
  echo "$(date '+%F %T') save: $(printf '%s\n' "$names" | wc -l) sessões do Hangar fora da foto" >> "$LOG"
}

restore() {
  echo "$(date '+%F %T') restore: as sessões do Hangar voltam pelo servidor" >> "$LOG"
}

# Antes do restore: `last` apontando pra arquivo vazio (save cortado por crash) = boot sem nenhuma
# sessao. Volta pro save nao-vazio mais recente e apaga os vazios.
fix_last() {
  local last f; last="$DIR/last"
  # A última sessão foi fechada de propósito: este start não é volta de queda, não restaura nada.
  if [ -e "$DIR/closed-on-purpose" ]; then
    rm -f "$DIR/closed-on-purpose"
    tmux set -g @continuum-restore off 2>/dev/null || true
    echo "$(date '+%F %T') fix-last: last session was closed on purpose -> no restore" >> "$LOG"
    return 0
  fi
  [ -s "$last" ] && return 0
  find "$DIR" -maxdepth 1 -name 'tmux_resurrect_2*.txt' -empty -delete
  f=$(ls -t "$DIR"/tmux_resurrect_2*.txt 2>/dev/null | head -1 || true)
  [ -n "$f" ] || return 0
  ln -fs "$(basename "$f")" "$last"
  echo "$(date '+%F %T') fix-last: last was empty -> $(basename "$f")" >> "$LOG"
}

# Hook `session-closed`: sessão fechada não volta no próximo restore. Sobrou alguma, a foto é refeita
# sem ela; não sobrou nenhuma (o servidor vai sair), fica a marca que o fix-last consome no start.
_shutting_down() {
  local s
  for s in "$(systemctl is-system-running 2>/dev/null)" "$(systemctl --user is-system-running 2>/dev/null)"; do
    [ "$s" = stopping ] && return 0
  done
  return 1
}

closed() {
  # Desligar/reiniciar mata as sessões uma a uma: isso é queda, não fechamento, e o boot tem que restaurar.
  if _shutting_down; then return 0; fi
  if [ -n "$(tmux list-sessions -F x 2>/dev/null)" ]; then
    "$HOME/.tmux/plugins/tmux-resurrect/scripts/save.sh" quiet
  else
    mkdir -p "$DIR" && : > "$DIR/closed-on-purpose"
  fi
}

case "${1:-}" in
  save) save ;;
  restore) restore ;;
  fix-last) fix_last ;;
  closed) closed ;;
  *) echo "usage: $0 save|restore|fix-last|closed" >&2; exit 2 ;;
esac
