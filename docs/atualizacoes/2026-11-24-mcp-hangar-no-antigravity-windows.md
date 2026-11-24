---
id: 2026-11-24-mcp-hangar-no-antigravity-windows
titulo: As sessões do Antigravity passam a ter o MCP do Hangar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-mcp-stdio.cmd
tipo: maquina
destrutivo: false
---

As sessões do Antigravity (`agy`) passam a falar com o Hangar como as do Claude e do Codex: listam e
mandam recado para outras sessões, usam o navegador embutido e o cofre de senhas. Vale para as sessões
abertas depois da atualização.
