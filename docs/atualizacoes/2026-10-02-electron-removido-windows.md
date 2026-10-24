---
id: 2026-10-02-electron-removido-windows
titulo: No Windows, o Hangar (Electron) sai da máquina; o app nativo assume a tela remota do navegador
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\remover-electron.ps1
prova: ~/.hangar/native/electron-removido
destrutivo: true
tipo: maquina
---

No Windows, o app nativo passa a atender a tela remota do navegador no celular, a última coisa que
só o Electron fazia. Os atalhos "Hangar (Electron)" saem do menu Iniciar e da área de trabalho, e as
dependências dele são apagadas. Se o Electron estiver aberto durante a atualização, só os atalhos
saem. Máquina sem o app nativo continua com o Electron.
