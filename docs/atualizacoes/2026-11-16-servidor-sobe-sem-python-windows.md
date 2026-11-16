---
id: 2026-11-16-servidor-sobe-sem-python-windows
titulo: No Windows, o servidor do Hangar sobe sem o Python e a tarefa nunca desiste de religar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/AppData/Local/hangar/hangar-backend.vbs
destrutivo: true
tipo: maquina
---

A tarefa do Hangar passa a abrir o servidor direto, sem o Python no meio, e volta sozinha sempre que
ele cair. A atualização reescreve a tarefa e reinicia o servidor: as conexões do celular caem por
alguns segundos.
