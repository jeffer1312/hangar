# Correções finais da 2C — prova da entrega

Cinco itens corrigidos e aprovados pela revisão independente. Branch `hangar-server-parte2c`, base `e0c44443`, HEAD `ed02b9ff`; árvore limpa, sem push ou merge.

- Falhas, pausa e reserva por sessão; uma sessão saudável não reinicia a outra, e uma sessão ruim não bloqueia todas. Diário registra mudanças de estado com sessão/código; `limite_ms` informa pausa agendada, enquanto `ms` informa duração observada. Polls durante a mesma pausa não repetem registro. Warn Rust limitado por sessão/causa, com expiração e limite de memória.
- Falha inicial é retentável no mesmo produtor/SSE. Fechamento explícito e cancelamento permanecem definitivos.
- Acquire não apaga histórico Rust; somente captura válida reinicia a pausa.
- Limpeza da prévia propaga cancelamento externo; o cancelamento normal de seu heartbeat continua esperado.
- Redutor temporal Rust, memória e fatos identificados como referência para a 2B. Produção 2C usa `analyze` no Rust e o processamento temporal no Python, sem RPC adicional.
- Correção adicional de identidade: operações antigas conferem nome, consumidor, geração e referência do registro antes de alterar falha/diário/sucesso; fechamento antigo não remove sucessor que reutilizou o nome.

Regressões falharam antes de cada correção. Conferência focada final: 116 casos do observador Python aprovados (um teste HTTP existente fora desse filtro); 72 de limpeza/deduplicação da prévia aprovados; 40 Rust distintos aprovados (28 controle, 11 contratos/rotas/diagnóstico, 1 limite de avisos). Não somar reexecuções. Sem suíte inteira, serviço/provedor/app vivo ou Windows; fakes e tmux privado com guardas antes do I/O.

O parecer do código e da correção final está em `.superpowers/sdd/2026-10-03-hangar-server-parte2c-final-corrections/review-final.md`; relatos dos executores e regressões na mesma pasta. Protocolo3, captura/HTTP únicos, opener reutilizado e quatro vagas reais de I/O preservados. A medição anterior de CPU não foi repetida nesta correção; nenhuma promessa de desempenho atual em produção.

```text
7b94f84a fix(preview): preserve cancellation during heartbeat cleanup
f46ff1cd fix(terminal): throttle observer route warnings by session and cause
73cc3ed1 fix(terminal): isolate observer failures by session
3bf358d2 fix(terminal): reset observer retries only after valid capture
0a8f78a6 docs(terminal): identify temporal reducer as Part 2B reference
87e5b098 fix(terminal): retry leases after transient startup failures
6e65200b fix(terminal): record per-session circuit pause duration
ed02b9ff fix(terminal): discard failures from retired lease attempts
```
