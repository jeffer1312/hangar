---
id: 2026-10-30-codex-mcp-hangar-prazo-longo
titulo: As sessões Codex esperam até 10 minutos pela sua resposta no cofre de senhas
comando_posix: python3 scripts/registrar-mcp.py
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: scripts/registrar-mcp.py
destrutivo: false
---

Quando uma sessão Codex pede uma senha do cofre, ela espera você aprovar no celular por até 10
minutos, em vez de desistir depois de 1 minuto. Vale para as sessões Codex abertas depois da
atualização.
