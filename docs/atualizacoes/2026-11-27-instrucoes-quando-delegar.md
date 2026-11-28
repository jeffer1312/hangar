---
id: 2026-11-27-instrucoes-quando-delegar
titulo: As sessões passam a saber quando abrir uma filha ligada e como acompanhá-la
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

As instruções que cada sessão recebe do Hangar agora dizem quando vale abrir uma filha ligada em vez
do subagente nativo, e que não é preciso esperar com `sleep` nem pedir a resposta por recado: ela
volta sozinha.
