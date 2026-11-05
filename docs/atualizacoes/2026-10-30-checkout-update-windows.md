---
id: 2026-10-30-checkout-update-windows
titulo: Atualização do checkout protegida no Windows
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/AppData/Local/hangar/hangar-backend.vbs
destrutivo: false
tipo: maquina
---

A recuperação automática aguarda o atualizador terminar antes de religar o servidor.
O diagnóstico e a atualização do checkout passam a usar o executável nativo.
