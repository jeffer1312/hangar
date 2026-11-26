---
id: 2026-11-26-navegador-embutido-instrucoes
titulo: As sessões passam a usar o navegador embutido também em subagente, clicando por texto e sem esperas fixas
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

As instruções do navegador embutido ficaram mais curtas e deixaram de depender do aviso do Hangar no
prompt: subagentes também passam a usá-lo. Elas dizem que o clique aceita o texto do botão e que
`click`, `press` e `eval` já esperam a página responder.
