---
id: 2026-09-25-mcp-hangar-conecta
titulo: MCP do Hangar conecta no Windows e no Claude aberto pelo terminal
comando_posix: ./scripts/install-claude-wrapper.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-mcp-headers
destrutivo: false
---

No Windows o MCP `hangar` aparecia como "not authenticated" ("Dynamic Client Registration
rejected"): o Claude Code não conseguia executar o script que envia o token. Agora ele usa um
`hangar-mcp-headers.cmd` e conecta. No Linux, o `claude` aberto pelo terminal na conta padrão
lia `~/.claude/.claude.json` em vez de `~/.claude.json` e ficava sem o MCP, sem as pastas
confiáveis e sem o histórico; agora usa o mesmo arquivo das sessões abertas pelo app. Sessão já
aberta só vê a mudança depois de reconectar (`/mcp`) ou reabrir.
