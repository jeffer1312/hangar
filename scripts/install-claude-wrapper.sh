#!/usr/bin/env bash
# Install the claude-cockpit interactive `claude` and `codex` wrappers.
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
  ensure_block "$1" "source \"$SHELL_DIR/claude.posix.sh\"
source \"$SHELL_DIR/codex.posix.sh\"
source \"$SHELL_DIR/claude-engine.posix.sh\""
}

install_fish() {
  local name dst src
  for name in claude codex claude-engine; do
    src="$SHELL_DIR/$name.fish"
    dst="$HOME/.config/fish/functions/$name.fish"
    mkdir -p "$(dirname "$dst")"
    if [ -e "$dst" ] && ! cmp -s "$src" "$dst"; then
      cp "$dst" "$dst.bak"
      echo "  backed up existing $dst -> $dst.bak"
    fi
    cp "$src" "$dst"
    echo "  installed fish function -> $dst"
  done
}

# Helper chamado pelos wrappers de shell. Symlink absoluto preserva a descoberta do backend/.env
# mesmo quando executado de qualquer cwd e atualiza automaticamente depois de git pull.
mkdir -p "$HOME/.local/bin"
chmod +x "$SCRIPT_DIR/cp-codex"
ln -sfn "$SCRIPT_DIR/cp-codex" "$HOME/.local/bin/cp-codex"
echo "  installed Codex helper -> $HOME/.local/bin/cp-codex"
chmod +x "$SCRIPT_DIR/cp-engine"
ln -sfn "$SCRIPT_DIR/cp-engine" "$HOME/.local/bin/cp-engine"
echo "  installed engine helper -> $HOME/.local/bin/cp-engine"

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
# Window name (inside tmux) = basename of the pane's cwd (not 0/1/2 nor the command name).
set -g allow-rename off
set -g automatic-rename on
set -g automatic-rename-format '#{b:pane_current_path}'
# Terminal/WM title = SESSION NAME, not the cwd basename. Two sessions in the same repo get
# distinct names (the wrapper appends -2, -3) but share a cwd basename, so a cwd title made them
# indistinguishable to the window manager. cp-panel-open needs that title to pick the right window
# whenever the terminal runs single-instance (`kitty -1`), where every window shares one pid and
# the pid->window map stops being a key. Session names are unique by construction; cwd is not.
set -g set-titles on
set -g set-titles-string '#S'
TMUXCONF
)"
  tmux source-file "$HOME/.tmux.conf" 2>/dev/null && echo "  reloaded ~/.tmux.conf" || true
fi

# --- extensao de estado do Pi ------------------------------------------------------------------
# Sem ela a sessao Pi aparece no app sempre "ociosa": o estado vem do marcador, nao do pane.
PI_EXT_DIR="$HOME/.pi/agent/extensions"
if command -v pi >/dev/null 2>&1; then
  mkdir -p "$PI_EXT_DIR"
  ln -sfn "$SCRIPT_DIR/pi/cp-state.ts" "$PI_EXT_DIR/cp-state.ts"
  echo "  linked cp-state.ts into $PI_EXT_DIR"
else
  echo "  pi nao encontrado — pulando a extensao de estado (instale o pi e rode de novo)"
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
echo "  codex         # creates a Cockpit-managed Codex session and attaches its tmux"
echo "Bypass a wrapper anytime with:  command claude ... / command codex ..."
