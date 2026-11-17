---
id: 2026-11-16-servidor-sobe-sem-python-windows
titulo: No Windows, o servidor do Hangar sobe sem o Python e a tarefa nunca desiste de religar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update && powershell -NoProfile -ExecutionPolicy Bypass -Command ". .\scripts\windows-tasks.ps1; if (-not (Test-HangarBackendTaskDirect (Join-Path $env:LOCALAPPDATA 'hangar\hangar-backend.vbs'))) { Write-Output '##HANGAR-FALHA## a tarefa hangar-backend nao ficou com os reinicios automaticos e o launch backend'; exit 1 }; New-Item -ItemType Directory -Force (Join-Path $HOME '.hangar\passos') | Out-Null; New-Item -ItemType File -Force (Join-Path $HOME '.hangar\passos\2026-11-16-servidor-sobe-sem-python-windows') | Out-Null"
prova: ~/.hangar/passos/2026-11-16-servidor-sobe-sem-python-windows
destrutivo: true
tipo: maquina
---

A tarefa do Hangar passa a abrir o servidor direto, sem o Python no meio, e volta sozinha sempre que
ele cair. A atualização reescreve a tarefa e reinicia o servidor: as conexões do celular caem por
alguns segundos.
