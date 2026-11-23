---
id: 2026-11-22-mcp-na-conta-padrao-com-config-dir
titulo: Com CLAUDE_CONFIG_DIR exportado, as sessões da conta padrão recebem o MCP do Hangar
comando: scripts/hangar-server-command mcp-register --root .
comando_windows: if exist bin\hangar-server.exe (bin\hangar-server.exe mcp-register --root .) else ("%USERPROFILE%\.hangar\bin\hangar-server.exe" mcp-register --root .)
prova: ~/.claude.json
tipo: maquina
destrutivo: false
---

Quem exporta `CLAUDE_CONFIG_DIR` no ambiente (mesmo apontando para `~/.claude`) tem as sessões da
conta padrão lendo o `.claude.json` de dentro dessa pasta. Ele passa a receber o MCP do Hangar com o
prazo longo do cofre de senhas, e as outras contas passam a espelhar os MCP desse arquivo.
