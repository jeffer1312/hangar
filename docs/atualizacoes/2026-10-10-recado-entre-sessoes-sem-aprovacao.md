---
id: 2026-10-10-recado-entre-sessoes-sem-aprovacao
titulo: Mandar recado para outra sessão deixa de pedir aprovação no Claude
comando_posix: python3 scripts/registrar-mcp.py
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/settings.json
destrutivo: false
---

Sessões do Claude que pedem aprovação para rodar comandos pediam também a cada recado para outra
sessão. Agora o `hangar-send` e as ferramentas do MCP `hangar` ficam liberados no
`~/.claude/settings.json`, e o resto continua pedindo como antes.
