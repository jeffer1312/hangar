---
id: 2026-11-17-vigia-no-lancador-windows
titulo: No Windows, o próprio Hangar religa o servidor travado e a tarefa hangar-vigia sai
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update && powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; New-Item -ItemType Directory -Force (Join-Path $HOME '.hangar\passos') | Out-Null; New-Item -ItemType File -Force (Join-Path $HOME '.hangar\passos\2026-11-17-vigia-no-lancador-windows') | Out-Null"
prova: ~/.hangar/passos/2026-11-17-vigia-no-lancador-windows
destrutivo: true
tipo: maquina
---

O Hangar passa a perceber sozinho quando o servidor trava e o reinicia. A tarefa separada que
fazia isso a cada 5 minutos sai, e com ela o aviso "Não é possível encontrar o arquivo de script
hangar-vigia.vbs" que aparecia em algumas máquinas.

A conferência começa 3 minutos depois de o servidor ficar pronto. O servidor que trava antes
disso, e uma versão anterior que ainda sobe pelo Python, só são reabertos quando caem.
