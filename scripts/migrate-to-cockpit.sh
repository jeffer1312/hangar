#!/usr/bin/env bash
# migrate-to-cockpit.sh — migra ESTA máquina do nome claude-pocket pro claude-cockpit.
#
# Roda do clone local (qualquer nome de pasta):  ./scripts/migrate-to-cockpit.sh
# Idempotente: re-rodar numa máquina já migrada só re-verifica.
#
# O que faz:
#   1. renomeia a pasta do clone pra "claude-cockpit" (irmã, mesmo pai);
#   2. aponta o remote origin pra github.com/jeffer1312/claude-cockpit;
#   3. corrige paths absolutos antigos em ~/.claude/settings.json (hooks/statusline)
#      e ~/.tmux.conf (hooks do resurrect);
#   4. troca as units systemd claude-pocket-* pelas claude-cockpit-* (services-setup.sh)
#      e migra a claude-pocket-deploy.service se existir (servidores com webhook);
#   5. re-roda install-cp-send.sh (cp-send + skills) e, se Hyprland+Quickshell,
#      install-cp-panel.sh (painel/tray).
#
# NÃO muda (de propósito — são dados/ids internos): ~/.claude/.claude-pocket-pair/,
# .claude-pocket-uploads/, instância quickshell "claude-pocket", cp-*/CP_*.
set -euo pipefail

OLD="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
NEW="$(dirname "$OLD")/claude-cockpit"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

# ── 1. Renomear a pasta ──────────────────────────────────────────────────────
if [[ "$OLD" == "$NEW" ]]; then
    log "pasta já se chama claude-cockpit"
else
    if command -v tmux >/dev/null && tmux list-panes -a -F '#{pane_current_path}' 2>/dev/null | grep -q "^$OLD"; then
        echo "AVISO: há painéis tmux com cwd dentro de $OLD — os shells deles vão ficar órfãos após o rename." >&2
    fi
    [[ -e "$NEW" ]] && { echo "erro: $NEW já existe — resolva antes." >&2; exit 1; }
    mv "$OLD" "$NEW"
    log "pasta: $OLD -> $NEW"
fi
cd "$NEW"

# ── 2. Remote ────────────────────────────────────────────────────────────────
if git remote get-url origin | grep -q "claude-pocket"; then
    git remote set-url origin "$(git remote get-url origin | sed 's/claude-pocket/claude-cockpit/')"
fi
log "remote: $(git remote get-url origin)"

# ── 3. Paths absolutos antigos em configs do usuário ─────────────────────────
for f in "$HOME/.claude/settings.json" "$HOME/.tmux.conf"; do
    if [[ -f "$f" ]] && grep -qF "$OLD" "$f" 2>/dev/null && [[ "$OLD" != "$NEW" ]]; then
        sed -i "s|$OLD|$NEW|g" "$f"
        log "paths corrigidos em $f"
    fi
done

# ── 4. Units systemd ─────────────────────────────────────────────────────────
SD="$HOME/.config/systemd/user"
if command -v systemctl >/dev/null && [[ -f "$SD/claude-pocket-backend.service" ]]; then
    log "trocando units claude-pocket-* por claude-cockpit-*"
    systemctl --user disable --now claude-pocket-backend.service claude-pocket-frontend.service 2>/dev/null || true
    rm -f "$SD/claude-pocket-backend.service" "$SD/claude-pocket-frontend.service"
    ./scripts/services-setup.sh        # escreve/sobe as claude-cockpit-* a partir do path novo
fi
if [[ -f "$SD/claude-pocket-deploy.service" ]]; then
    systemctl --user disable claude-pocket-deploy.service 2>/dev/null || true
    sed "s|$OLD|$NEW|g" "$SD/claude-pocket-deploy.service" > "$SD/claude-cockpit-deploy.service"
    rm -f "$SD/claude-pocket-deploy.service"
    systemctl --user daemon-reload
    log "unit de deploy migrada pra claude-cockpit-deploy.service"
fi

# ── 5. Re-instalar symlinks (cp-send, skills, painel) ────────────────────────
./scripts/install-cp-send.sh
if command -v qs >/dev/null && [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]]; then
    ./scripts/install-cp-panel.sh
fi

# ── Verificação ──────────────────────────────────────────────────────────────
echo
if command -v systemctl >/dev/null && [[ -f "$SD/claude-cockpit-backend.service" ]]; then
    systemctl --user is-active claude-cockpit-backend.service claude-cockpit-frontend.service \
        && log "serviços claude-cockpit-* ativos" \
        || echo "AVISO: serviço não subiu — ver: journalctl --user -u claude-cockpit-backend.service" >&2
fi
command -v cp-send >/dev/null && cp-send --list >/dev/null 2>&1 && log "cp-send ok (backend respondendo)" \
    || echo "AVISO: cp-send --list falhou — backend fora?" >&2
log "migração concluída. Clone em: $NEW"
