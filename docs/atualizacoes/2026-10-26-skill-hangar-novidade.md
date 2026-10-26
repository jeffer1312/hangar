---
id: 2026-10-26-skill-hangar-novidade
titulo: Nova skill hangar-novidade, que escreve as novidades mostradas na atualização
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/skills/hangar-novidade/SKILL.md
destrutivo: false
---

Chega a skill `hangar-novidade`: ela escreve o texto da novidade que aparece na tela de
atualização e, quando é recurso novo, a animação que mostra como usar. Ela é lembrada ao abrir um
PR que muda alguma tela.
