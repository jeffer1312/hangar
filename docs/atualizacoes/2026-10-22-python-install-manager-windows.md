---
id: 2026-10-22-python-install-manager-windows
titulo: No Windows, o Python instalado pelo winget volta a servir aos comandos do Hangar
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-conta.cmd ~/.local/bin/python3.cmd
destrutivo: false
tipo: maquina
---

Com o Python 3.14 instalado pelo winget (Python Install Manager), o instalador não o reconhecia e
deixava de criar o `hangar-conta`, o `hangar-engine` e os comandos do Codex. Agora ele acha o
interpretador e cria os três. Também deixa de aparecer a janela "Como deseja abrir este
arquivo?" pedindo um programa para o `python3`.
