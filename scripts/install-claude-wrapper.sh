#!/usr/bin/env bash
# Install the claude-pocket `claude` wrapper into your shell (+ optional tmux config + statusline).
#
# The wrapper makes every interactive `claude` trackable by the app: it injects a unique
# --session-id (so two claudes in the same folder never leak/overwrite each other) and launches
# claude inside a tmux session named after the folder (the app only lists tmux sessions). See
# scripts/shell/claude.fish and scripts/shell/claude.posix.sh.
#
# It also (opt-in) sets the claude-pocket statusline as your Claude Code statusLine, so the app can
# parse model / context / cost / rate-limit reliably (the parser expects that format). See
# scripts/omniroute-statusline.js.
#
# One-time setup. Idempotent — safe to re-run (updates the managed block in place). It only edits
# between `# >>> claude-pocket >>>` / `# <<< claude-pocket <<<` markers and backs up replaced files.
#
# Usage:
#   ./scripts/install-claude-wrapper.sh [fish|bash|zsh|all] [--no-tmux] [--statusline|--no-statusline]
#
#   (no shell arg)   auto-detect from $SHELL
#   all              install for fish + bash + zsh
#   --no-tmux        skip the ~/.tmux.conf truecolor + window-rename block
#   --statusline     set the claude-pocket statusline as your Claude statusLine (no prompt)
#   --no-statusline  skip the statusline step
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SHELL_DIR="$SCRIPT_DIR/shell"
STATUSLINE_JS="$SCRIPT_DIR/omniroute-statusline.js"
BEGIN_MARK="# >>> claude-pocket >>>"
END_MARK="# <<< claude-pocket <<<"

DO_TMUX=1
DO_STATUS=""   # "", 1 or 0 — empty means "ask if interactive, else yes"
TARGET=""
for arg in "$@"; do
  case "$arg" in
    --no-tmux) DO_TMUX=0 ;;
    --statusline) DO_STATUS=1 ;;
    --no-statusline) DO_STATUS=0 ;;
    fish|bash|zsh|all) TARGET="$arg" ;;
    -h|--help) awk 'NR==1{next} /^#/{print;next} {exit}' "$0"; exit 0 ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

if [ -z "$TARGET" ]; then
  case "$(basename "${SHELL:-}")" in
    fish) TARGET=fish ;;
    bash) TARGET=bash ;;
    zsh)  TARGET=zsh ;;
    *) echo "Could not detect shell from \$SHELL. Pass one: fish | bash | zsh | all" >&2; exit 2 ;;
  esac
  echo "Detected shell: $TARGET (override with an arg)"
fi

# Insert/replace the managed block between markers. $1=file  $2=payload
ensure_block() {
  local file="$1" payload="$2" tmp
  touch "$file"
  if grep -qF "$BEGIN_MARK" "$file"; then
    tmp="$(mktemp)"
    awk -v b="$BEGIN_MARK" -v e="$END_MARK" -v p="$payload" '
      $0==b {print; print p; skip=1; next}
      $0==e {skip=0; print; next}
      skip {next}
      {print}
    ' "$file" >"$tmp" && mv "$tmp" "$file"
    echo "  updated managed block in $file"
  else
    printf '\n%s\n%s\n%s\n' "$BEGIN_MARK" "$payload" "$END_MARK" >>"$file"
    echo "  added managed block to $file"
  fi
}

install_posix() {  # $1 = rc file
  ensure_block "$1" "source \"$SHELL_DIR/claude.posix.sh\""
}

install_fish() {
  local dst="$HOME/.config/fish/functions/claude.fish"
  mkdir -p "$(dirname "$dst")"
  if [ -e "$dst" ] && ! cmp -s "$SHELL_DIR/claude.fish" "$dst"; then
    cp "$dst" "$dst.bak"
    echo "  backed up existing $dst -> $dst.bak"
  fi
  cp "$SHELL_DIR/claude.fish" "$dst"
  echo "  installed fish function -> $dst"
}

# Point Claude Code's statusLine at scripts/omniroute-statusline.js so the app parses it reliably.
install_statusline() {
  local node settings
  node="$(command -v node || true)"
  # Resolve symlinks (fnm/nvm shims live in volatile per-shell dirs) -> stable real binary path.
  [ -n "$node" ] && node="$(readlink -f "$node" 2>/dev/null || echo "$node")"
  if [ -z "$node" ]; then
    echo "  node not found in PATH — skipping statusline (install Node 20+ and re-run with --statusline)"
    return
  fi
  settings="$HOME/.claude/settings.json"
  mkdir -p "$(dirname "$settings")"
  [ -f "$settings" ] || echo '{}' >"$settings"
  cp "$settings" "$settings.bak"
  SP_NODE="$node" SP_SCRIPT="$STATUSLINE_JS" SP_FILE="$settings" "$node" -e '
    const fs = require("fs");
    const p = process.env.SP_FILE;
    let d = {}; try { d = JSON.parse(fs.readFileSync(p, "utf8")); } catch {}
    d.statusLine = { type: "command", command: process.env.SP_NODE + " " + process.env.SP_SCRIPT };
    fs.writeFileSync(p, JSON.stringify(d, null, 2));
  '
  echo "  set Claude statusLine -> $node $STATUSLINE_JS (backup: $settings.bak)"
  case "$node" in
    *fnm*|*nvm*|*node-versions*)
      echo "  note: statusLine is pinned to this exact node version path — re-run this installer after upgrading node" ;;
  esac
}

case "$TARGET" in
  fish) install_fish ;;
  bash) install_posix "$HOME/.bashrc" ;;
  zsh)  install_posix "$HOME/.zshrc" ;;
  all)  install_fish; install_posix "$HOME/.bashrc"; install_posix "$HOME/.zshrc" ;;
esac

if [ "$DO_TMUX" = 1 ]; then
  echo "tmux config (~/.tmux.conf):"
  ensure_block "$HOME/.tmux.conf" "$(cat <<'TMUXCONF'
# Truecolor for Claude Code inside tmux (TERM must NOT start with tmux/screen, or colors break).
set -g default-terminal "xterm-256color"
set -ga terminal-overrides ",xterm-kitty:Tc,xterm-256color:Tc"
set -ga terminal-features ",xterm-kitty:RGB,xterm-256color:RGB"
set-environment -g COLORTERM truecolor
set-environment -g CLAUDE_CODE_TMUX_TRUECOLOR 1
# Clipboard de imagem (wl-paste) dentro do Claude Code: sessao criada por um client anexado
# herda o WAYLAND_DISPLAY dele mesmo quando o server tmux nasceu sem a var (ex: via backend).
set -ga update-environment "WAYLAND_DISPLAY"
# Window/title name = basename of the pane's cwd (not 0/1/2 nor the command name).
set -g allow-rename off
set -g automatic-rename on
set -g automatic-rename-format '#{b:pane_current_path}'
set -g set-titles on
set -g set-titles-string '#{b:pane_current_path}'
TMUXCONF
)"
  tmux source-file "$HOME/.tmux.conf" 2>/dev/null && echo "  reloaded ~/.tmux.conf" || true
fi

# Statusline: ask if undecided and interactive; default yes otherwise.
if [ -z "$DO_STATUS" ]; then
  if [ -t 0 ]; then
    printf "Set the claude-pocket statusline as your Claude statusLine? (recommended) [Y/n] "
    read -r ans </dev/tty || ans=""
    case "$ans" in [Nn]*) DO_STATUS=0 ;; *) DO_STATUS=1 ;; esac
  else
    DO_STATUS=1
  fi
fi
if [ "$DO_STATUS" = 1 ]; then
  echo "statusline (~/.claude/settings.json):"
  install_statusline
fi

# Sessões-irmãs: instala o cp-send junto (symlink + bloco no CLAUDE.md global). Idempotente —
# era passo manual separado e máquina nova ficava sem o protocolo de pareamento sem ninguém avisar.
CP_SEND_INSTALLER="$(dirname "$0")/install-cp-send.sh"
if [ -x "$CP_SEND_INSTALLER" ]; then
  echo
  echo "cp-send (sessões-irmãs):"
  # Não-fatal: o trabalho principal (wrapper/tmux/statusline) já foi feito — falha aqui avisa e segue.
  "$CP_SEND_INSTALLER" || echo "aviso: install-cp-send falhou (não-fatal; wrapper já instalado)"
else
  echo "aviso: install-cp-send.sh não encontrado em $CP_SEND_INSTALLER — setup de sessões-irmãs PULADO"
fi

echo
echo "Done. Open a NEW terminal (or reload your rc) so the wrapper loads, then run:"
echo "  claude        # creates a tmux session named after the folder, with a --session-id"
echo "Bypass the wrapper anytime with:  command claude ..."
