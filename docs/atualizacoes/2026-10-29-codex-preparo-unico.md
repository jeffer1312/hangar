---
id: 2026-10-29-codex-preparo-unico
titulo: O Codex prepara a conta uma vez e reutiliza nas próximas aberturas
comando: scripts/hangar-server-command harness-setup repair-codex
comando_windows: if exist bin\hangar-server.exe (bin\hangar-server.exe harness-setup repair-codex) else ("%USERPROFILE%\.hangar\bin\hangar-server.exe" harness-setup repair-codex)
prova: ~/.hangar/codex-prepare-once-v1.json
tipo: maquina
destrutivo: false
---

A primeira abertura aguarda a conta ficar pronta. As seguintes reutilizam a instalação;
atualizações ficam para a sincronização manual. As contas existentes recebem um reparo único
das dependências do ECC e dos caminhos antigos de Python, preservando os demais hooks.
