---
id: 2026-11-27-filha-ligada-entre-maquinas
titulo: Os agentes sabem que a sessão filha ligada pode abrir noutra máquina
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send ~/.claude/CLAUDE.md
destrutivo: false
---

Com o balanceamento, a sessão que um agente abre ligada a ele pode nascer noutra máquina, e o
trecho "Sessões-irmãs" do seu `~/.claude/CLAUDE.md` deixa de dizer que a ligação vale só nesta.
