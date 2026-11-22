---
id: 2026-11-22-skill-whatsapp
titulo: Nova skill whatsapp, que lê as conversas pelo arquivador só leitura do Hangar
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/skills/whatsapp/SKILL.md
destrutivo: false
---

Chega a skill `whatsapp`. Com o WhatsApp ligado em Configurações > WhatsApp, as sessões leem,
buscam e pedem mídia antiga das suas conversas pelas ferramentas do Hangar. Nada é enviado em seu
nome.
