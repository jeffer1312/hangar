---
id: 2026-11-17-atalhos-windows-sem-python
titulo: No Windows, o hangar-conta e os lançadores do Codex passam a rodar sem Python
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update && powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-atalhos-sem-python.ps1
prova: ~/.hangar/passos/2026-11-17-atalhos-windows-sem-python
tipo: maquina
destrutivo: false
---

No Windows, o `hangar-conta` (usado pelo `claude-conta`) e os lançadores do Codex passavam pelo
Python. Eles passam a chamar o próprio servidor do Hangar; sem esta atualização, o `claude-conta`
pararia de abrir.
