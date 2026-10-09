---
id: 2026-10-09-instrucoes-das-sessoes-atualizadas
titulo: As instruções que os agentes leem sobre o Hangar batem com o app de hoje
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send ~/.claude/CLAUDE.md
destrutivo: false
---

O trecho "Sessões-irmãs" do seu `~/.claude/CLAUDE.md` foi refeito: os agentes param de responder
ao aviso de pareamento, sabem que fechar uma sessão não avisa o grupo e que o navegador do app
nativo não tem abas, e conhecem a tool `html_render` do MCP.
