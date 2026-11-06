---
id: 2026-11-06-skill-iphone-wda-sidestore
titulo: Nova skill iphone-wda-sidestore, para instalar o SideStore e manter o WebDriverAgent no iPhone pelo Linux
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.claude/skills/iphone-wda-sidestore/SKILL.md
destrutivo: false
---

Chega a skill `iphone-wda-sidestore`. Ela tem o passo a passo para pôr o SideStore e o
WebDriverAgent num iPhone a partir do Linux, com conta Apple gratuita. Também renova o
WebDriverAgent antes de vencer os 7 dias, e o `start-wda.sh` já faz essa renovação ao subir.
