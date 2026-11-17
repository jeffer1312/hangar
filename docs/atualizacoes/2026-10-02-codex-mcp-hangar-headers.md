---
id: 2026-10-02-codex-mcp-hangar-headers
titulo: O MCP hangar no Codex volta a saber de qual sessão vem cada chamada
comando_posix: scripts/hangar-server-command mcp-register --root .
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/settings.json
destrutivo: false
---

Se o app desktop do Codex regravou o `config.toml` sem os dados que identificam a sessão, o
Hangar deixava o registro do MCP `hangar` como estava e as ferramentas dele não sabiam quem
chamava. Agora o registro confere todos os campos e refaz o bloco quando falta algum.
