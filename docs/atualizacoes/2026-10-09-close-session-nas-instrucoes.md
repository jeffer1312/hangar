---
id: 2026-10-09-close-session-nas-instrucoes
titulo: Os agentes sabem fechar sessões pelo MCP do Hangar
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send ~/.claude/CLAUDE.md
destrutivo: false
---

O MCP do Hangar ganhou a ferramenta `close_session`, e o trecho "Sessões-irmãs" do seu
`~/.claude/CLAUDE.md` passa a citá-la: um agente fecha a sessão auxiliar que abriu sem precisar do
terminal.
