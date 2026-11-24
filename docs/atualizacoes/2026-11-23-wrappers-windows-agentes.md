---
id: 2026-11-23-wrappers-windows-agentes
titulo: pi, omp, kimi e agy digitados no PowerShell abrem a sessão pelo Hangar
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\setup-windows-wrappers.ps1 -Apply
prova: scripts/shell/hangar-open.ps1
tipo: maquina
destrutivo: false
---

No Windows, digitar `pi`, `omp`, `kimi` ou `agy` num terminal do PowerShell agora abre a sessão
pelo Hangar, como já acontecia com `claude` e `codex`. Vale nos terminais abertos depois da
atualização.
