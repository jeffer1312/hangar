---
id: 2026-10-24-instrucoes-sessao-filha-ligada
titulo: As sessões passam a saber abrir uma filha ligada, interrompê-la e fechar o navegador ao terminar
comando_posix: ./scripts/install-hangar-send.sh
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update
prova: ~/.local/bin/hangar-send
destrutivo: false
---

As instruções que cada sessão recebe do Hangar agora explicam a sessão filha ligada (abrir com
`link`, interromper com `interrupt`, a resposta final volta sozinha), abrir sessão numa branch
própria e fechar o navegador embutido ao terminar. Pi, omp e Kimi deixam de ser mandados a usar o
que não têm.
