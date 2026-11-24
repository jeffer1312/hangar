---
id: 2026-11-24-instrucoes-forcar-conta
titulo: As sessões passam a saber abrir uma sessão na conta acima de 95% quando pedido
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

As instruções que cada sessão recebe do Hangar agora dizem como abrir uma sessão numa conta com 95%
ou mais de uso quando você pedir (`forcar_conta` / `--forcar-conta`), em vez de ela ir sempre para a
de mais folga.
