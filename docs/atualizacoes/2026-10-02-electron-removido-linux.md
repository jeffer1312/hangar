---
id: 2026-10-02-electron-removido-linux
titulo: No Linux, o Hangar (Electron) sai da máquina; o app nativo assume o navegador do hangar-preview
comando_posix: ./scripts/remover-electron.sh
prova: ~/.hangar/native/electron-removido
destrutivo: true
tipo: maquina
---

No Linux, o app nativo passa a atender o navegador do `hangar-preview` e a tela remota no celular,
o que só o Electron fazia. O lançador "Hangar (Electron)" sai do menu de apps e as dependências
dele são apagadas. Se o Electron estiver aberto durante a atualização, só o lançador sai. Máquina
sem o app nativo atualizado, ou sem Chrome ou Chromium, continua com o Electron.
