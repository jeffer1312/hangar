---
id: 2026-09-28-tmux-fechar-sem-reabrir
titulo: Sessão fechada de propósito não volta no próximo start do tmux
comando_posix: ! grep -qs tmux-claude-resume.sh ~/.tmux.conf || { ./scripts/tmux-persist-setup.sh && { tmux source-file ~/.tmux.conf 2>/dev/null || true; }; }
prova: scripts/tmux-claude-resume.sh
destrutivo: false
tipo: maquina
---

Fechar sessões (inclusive todas) não faz mais o tmux reabri-las no próximo start: só uma queda de
verdade, como reboot ou falta de memória, restaura as sessões. Vale para quem usa a persistência do
tmux (`tmux-persist-setup.sh`); quem não usa não é tocado.
