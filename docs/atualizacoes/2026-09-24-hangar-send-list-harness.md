---
id: 2026-09-24-hangar-send-list-harness
titulo: hangar-send --list mostra o harness de cada sessão
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

`hangar-send --list` ganhou a coluna do harness (`codex/headless`, `claude/tmux`,
`claude:<motor>/tmux`) entre o estado e a pasta. O bloco "Sessões-irmãs" do CLAUDE.md global
passa a citar essa coluna, por isso o instalador do hangar-send roda de novo.
