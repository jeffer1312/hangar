---
id: 2026-11-22-tmux-resume-sessoes-internas
titulo: Terminal de atalho não volta do reboot como sessão Claude falsa
comando_posix: ! grep -qsE '\.hangar/versions/[^/ ]+/scripts/tmux-claude-resume\.sh' ~/.tmux.conf || { perl -pi -e 's#\.hangar/versions/[^/ ]+/scripts/tmux-claude-resume\.sh#.hangar/current/scripts/tmux-claude-resume.sh#g' ~/.tmux.conf && { tmux source-file ~/.tmux.conf 2>/dev/null || true; }; }
prova: scripts/tmux-claude-resume.sh
destrutivo: false
tipo: maquina
---

Depois de reiniciar o computador, terminais de atalho e o shell separado de uma sessão não voltam mais
como uma sessão Claude sem Claude na lista. Vale para quem usa a persistência do tmux
(`tmux-persist-setup.sh`); o `~/.tmux.conf` que apontava para a pasta de uma versão passa a apontar
para a versão em uso.
