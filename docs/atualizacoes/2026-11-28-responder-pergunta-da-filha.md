---
id: 2026-11-28-responder-pergunta-da-filha
titulo: As sessões sabem responder a pergunta da filha ligada sem a tool child_answer
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send ~/.claude/CLAUDE.md
destrutivo: false
---

O trecho "Sessões-irmãs" do seu `~/.claude/CLAUDE.md` passa a dizer que a sessão aberta antes da tool
`child_answer` responde a pergunta da filha por `hangar-send --responder`, e que o aviso "a mesma
pergunta já avisada" não pede uma segunda resposta.
