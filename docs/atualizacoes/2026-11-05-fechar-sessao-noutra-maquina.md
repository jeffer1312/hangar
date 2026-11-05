---
id: 2026-11-05-fechar-sessao-noutra-maquina
titulo: Fechar sessão de outra máquina por servidor::sessao
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/CLAUDE.md
destrutivo: false
---

As sessões agora sabem que `hangar-send --close` e a tool `close_session` também fecham sessão de outra máquina sua, pelo endereço `servidor::sessao`.
