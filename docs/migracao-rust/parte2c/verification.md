# Parte 2C — entrega e conferência

Esta entrega foi corrigida posteriormente; o estado vigente e a medição estão em [revisão corretiva](2026-10-03-hangar-server-parte2c-revision-verification.md).

Implementada e aprovada pelas revisões independentes por Task e pela revisão final. Branch `hangar-server-parte2c`, base `a80a9e45`, HEAD `d21a445b`; árvore versionada limpa. Sem push, merge ou publicação. Plano/spec/análise continuam ignorados, como pedido.

## Entrega

- Um cliente `tmux -C` contínuo por sessão observada, compartilhado por consumidores, com grade `alacritty_terminal` e encerramento/liberação/expiração definidos. A captura tmux permanece referência para histórico, wrap, ANSI e linhas vazias. Nenhuma tecla/resize/resposta ANSI do observador.
- Processamento completo Claude de estado/prévia no Rust, com fatos nativos/plugin/hook coletados no Python, reserva com memória/fatos iguais, guarda de vínculo/época e prioridade dos sidecars. Codex conserva estado e prévia nativos e observa o terminal durante seu monitor, sem adquirir tmux em headless.
- Listener privado efêmero de loopback no mesmo processo Rust, separado do bind público. Saúde anuncia o endereço; Supervisor valida antes da ponte. Contrato Python/Rust **2** no mesmo commit; `side-events` preservado. 2B precisa reconciliar o protocolo ao integrar.

## Provas e metodologia

A contagem considera casos dos arquivos focados, incluindo parametrizações, sem somar reexecuções. Python: 381 casos únicos da integração original, mais 17 da correção HTTP e 2 da regressão da prévia = **400 casos únicos**. Último arquivo alterado (`test_terminal_observer.py`): **56/56** verdes após o último ajuste. Rust: **28/28** na última rodada dos três arquivos, sem mudança Rust posterior (1 contrato golden, 20 controle, 7 rotas). Não rodou suíte inteira.

O golden compara 58 capturas estáticas e 16 sequências/65 quadros com o Python original. Controle real foi conferido exclusivamente em sockets tmux privados, com panes sintéticos: PID compartilhado, último release/reap, ambiente/dimensões, histórico/ANSI/join/UTF-8/linhas vazias/%literal, cursor na margem, resize externo, falhas e expiração. HTTP real de loopback contra servidor sintético conferiu a ponte, sem tmux; Rust em bind público específico `127.0.0.2` e privado `127.0.0.1` conferiu resposta e fechamento das duas portas.

Revisões corrigiram fronteira/casefold Unicode, cursor com wrap pendente, correlação de frames fragmentados, reserva em `HTTPException`, recusa de redirects e getter dinâmico da prévia após `/clear` com múltiplos SSE. Todos tiveram reprodução vermelha e prova verde; nenhuma pendência de revisão ficou aberta.

Comandos focados executados:

```sh
(cd backend && uv run pytest tests/test_terminal_observer.py tests/test_shared_capture.py tests/test_state_classifier.py tests/test_preview_dedup.py tests/test_rust_server.py tests/test_internal_api.py tests/test_plugin_bridge.py tests/test_codex_adapter.py tests/test_sse.py -q)
(cd backend && uv run pytest tests/test_terminal_observer.py -q)
cargo test --manifest-path crates/Cargo.toml -p hangar-server --test contract_terminal --test terminal_control --test terminal_routes
git diff --check
```

## Limites e incidente

App/backend vivo e VM Windows não foram conferidos. Nenhum serviço foi iniciado/reiniciado/parado e nenhum instalador rodou. Windows permanece Python; sem afirmação de desempenho/carga em produção. A migração total, 2B, entrada de teclas/2D, demais providers e lista continuam fora desta entrega.

Um teste inicial sem dublê tentou `tmux capture-pane -p -t %8 -S -200` no tmux real; falhou `can't find pane: %8`, sem leitura de conversa nem escrita. A tentativa contrariou o isolamento solicitado e foi informada ao dono. O dublê e as guardas antes do I/O foram corrigidos; teardown detecta até chamada proibida engolida pela reserva. Nenhuma outra chamada real ocorreu nos novos testes.

## Commits

```text
d23d59fd feat(server): port terminal state and preview reducers
3c14bd0e fix(server): match Python terminal Unicode rules
1f67623e feat(server): observe terminal screens through persistent tmux control
962fbe11 fix(server): correlate control responses and preserve pending terminal wrap
a258733f feat(server): integrate terminal observation with Python fallback
24ad5ea0 fix(server): reject terminal bridge redirects and HTTP protocol failures
d21a445b fix(server): follow the current preview transcript binding
```

## Decisões registradas

- Seguir o kickoff atual que exige `alacritty_terminal` e manter a captura tmux como referência de histórico/wrap. Custo: consultas continuam pelo cliente persistente, sem processo por leitura.
- Executar por implementadores delegados com revisão independente por Task, como o roteiro/kickoff; nenhuma aprovação intermediária foi pedida. Custo: contextos separados de implementação/revisão.
- Rodar somente os testes focados autorizados e preservar o serviço vivo. Limite: a suíte inteira e o uso em produção não estão conferidos.
- Acrescentar listener interno efêmero de loopback no mesmo processo para suportar bind público em IP específico da LAN. Custo: um socket local adicional; nenhum processo, serviço ou lifespan adicional.
- Manter Windows no Python enquanto não houver prova da versão/protocolo instalado. Limite: sem afirmação de uso real do caminho Rust no Windows ou no app vivo.
- Não afirmar desempenho/carga em produção a partir dos ensaios funcionais isolados.
- Preservar 2B, demais providers, sessões sem terminal, entrada de teclas e lista fora da 2C. Limite: esta entrega é parte da migração; o contrato deverá ser reconciliado com a 2B.

Resumo entregue à sessão `hangar` pelo MCP tipado, com confirmação `delivered: true`, em 03/10/2026. Branch/worktree preservados; somente o diretório temporário desta execução foi removido após arquivar relatórios.
