---
id: 2026-10-01-atualizar-windows-front-no-ar
titulo: No Windows, o Atualizar deixa de falhar quando as dependências da tela mudam
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -Command "try { . .\scripts\windows-tasks.ps1; Stop-HangarFrontend (Get-Location).Path } catch { Write-Host $_ }"
prova: scripts/windows-tasks.ps1
destrutivo: true
tipo: maquina
---

No Windows, uma atualização que trazia dependências novas da tela parava em "as dependências do
front não instalaram" e passava a falhar em toda tentativa seguinte. Agora ela termina sozinha.
