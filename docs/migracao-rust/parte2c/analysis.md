# Parte 2C — análise

A árvore está limpa em `hangar-server-parte2c`, base `a80a9e45`. O envio, a lista e os processos sem terminal ficam fora.

`state.shared_capture` já junta consumidores e capturas em voo. `StateMonitor.stream` decide perguntas, congelamento de spinner, quatro quadros sem spinner, plugin e registro/hook. `PreviewBroker` prefere sidecar e conserva a geração de `/clear`. O Codex já usa app-server para estado/prévia: o terminal não deve vencê-lo.

O kickoff atual pede `alacritty_terminal`; prevalece sobre a spec anterior, que o adiava. A captura pelo cliente de controle permanece referência para histórico, ANSI, linhas vazias e wrap. A grade em memória acompanha saída/checkpoints; divergência não entrega texto errado. Há uma conexão por sessão observada, compartilhada pelos consumidores, nunca por aparelho.

`plugin_bridge.terminal_preso` conta qualquer cliente como pessoa; excluir control-mode é pré-condição. Windows continua Python: documentação upstream de psmux não prova o protocolo instalado. Não acessar VM nem alterar serviço.

O Python conserva aquisição dos fatos de plugin/hook, permissões, loop e shells e os efeitos do SSE. A decisão visual, os contadores temporais e a extração da prévia são Rust; toda falha volta à implementação Python. O estado nativo do Codex conserva sua precedência.

Ponto de encontro 2B: `rust_server.py`, `routes.rs`, `lib.rs` e a versão do contrato. Não alterar `side-events`, dono do app-server, cano ou fila. As partes precisam reconciliar o número do protocolo ao integrar, nunca copiar versão incompatível.

## Conferência do psmux (03/10/2026)

O upstream `psmux/psmux` em `6fb5d8c3a55f48abc4eccfe638d344c79f26bf28` reconhece `-CC`/`-C` em `src/main.rs:916–921`; `src/control.rs` implementa escapes octais, `%begin`, `%end` e `%error`. Fonte: [argumentos](https://github.com/psmux/psmux/blob/6fb5d8c3a55f48abc4eccfe638d344c79f26bf28/src/main.rs#L916) e [protocolo](https://github.com/psmux/psmux/blob/6fb5d8c3a55f48abc4eccfe638d344c79f26bf28/src/control.rs). Isso confirma implementação upstream, não a versão/protocolo instalados no Windows. Não houve ensaio de VM; permanece a reserva Python.
