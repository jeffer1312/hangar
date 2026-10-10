---
id: 2026-10-10-recado-entre-sessoes-sem-aprovacao
titulo: Mandar recado para outra sessão deixa de pedir aprovação no Claude
comando_posix: python3 scripts/registrar-mcp.py
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/settings.json
destrutivo: false
---

Sessões do Claude que pedem aprovação para rodar comandos pediam também a cada recado para outra
sessão. Agora as ferramentas de recado do MCP `hangar` (mandar, avisar o grupo, listar sessões)
ficam liberadas no `~/.claude/settings.json`. Criar ou fechar sessão e o `hangar-send` pelo
terminal continuam pedindo aprovação.
