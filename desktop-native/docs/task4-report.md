# Task 4: envio e interrupção

HEAD destacado `55e0d909`. Executor `gpt-6-sol/high`, `CODEX_HOME=/home/jefferson/.codex-jefferson-felizardo`. Step 11 conferido; Step 10 permanece aberto no plano até o fechamento pelo Astra. Fontes congeladas após os dois ajustes finais, sem stage, stash, commit ou push.

## Implementação

- Rascunhos e envios usam a identidade `(servidor, nome, jsonl)`. Um POST em andamento bloqueia outro para a mesma identidade mesmo após trocar de sessão; uma sessão recriada com o mesmo nome não herda o bloqueio nem o resultado antigo. A resposta HTTP atualiza a identidade de origem, preservando o rascunho em falha definida ou entrega incerta. Não há reenvio automático.
- A interface diferencia envio HTTP em andamento, HTTP entregue mas ainda sem prova no transcript, entrada enfileirada (`delivered:false`), falha e entrega incerta. Um evento real novo pode retirar o aviso após HTTP aceito. Recusa e incerteza do HTTP preservam aviso e rascunho mesmo que outro cliente tenha enviado texto igual; um eco sozinho não limpa o compositor enquanto o POST está pendente.
- Enter usa o evento de envio do `TextareaState`; Shift+Enter insere quebra de linha pelo próprio controle, que também recebe a composição IME. O diálogo de conexão usa o `focus_trap` do GPUI Kit, foca seu campo de endereço ao abrir e bloqueia o envio enquanto está aberto.
- Interromper chama `/interrupt?clear=false`, vincula o resultado à identidade selecionada e não força o estado `idle`. O backend pode retornar `detail` como string ou `{code, params, msg}`; o motivo `msg` aparece na UI. Aviso de interrupção é limpo no próximo envio. Pergunta/aprovação pendente continua indicada para resposta no Electron.

Decided alone: regressões de envio em `#[cfg(test)]` de `src/delivery.rs` e `src/api/mod.rs`, pois o pacote binário não possui `lib.rs` para expor módulos a `tests/input_delivery.rs`.

## Sessão real descartável

Criei **somente** `cx-native-task4-jf` pelo `POST /api/sessions` existente, em `/tmp/hangar-native-task4-fixture-jf`, com `provider=codex`, `headless=true`, `codex_account=jefferson-felizardo`, `model=gpt-6-sol`, `effort=high`. O retorno confirmou `codex_home=/home/jefferson/.codex-jefferson-felizardo`; o `turn_context` do primeiro turno confirmou modelo `gpt-6-sol` e esforço `high`. A sessão recebeu instrução de responder apenas em texto, sem ferramentas ou arquivos. Foi preservada viva para a Task 5.

Pelo cliente nativo, Enter enviou o primeiro pedido **uma vez**: `GET /history` mostrou 1 `user_msg` correspondente e 1 resposta (`artifacts/task4-send-pending.png`). Shift+Enter criou duas linhas sem envio (`task4-draft-multiline.png`). O rascunho voltou intacto após selecionar apenas para leitura outra sessão e retornar (`task4-draft-restored.png`). Quatro Tabs circularam o foco dentro do diálogo; Enter nele não enviou o rascunho, e o histórico permaneceu com 1 `user_msg` (`task4-modal-focus.png`).

Durante uma resposta longa da `cx-*`, Interromper foi acionado na UI; ela mostrou “Interrupção solicitada”, a sessão continuou existente e voltou a `idle` (`task4-stream-before-stop.png`, `task4-after-stop.png`). Um novo pedido à mesma sessão respondeu “continued.” (`task4-after-continue.png`). Nenhuma outra sessão recebeu input ou interrupção. A sessão `cx-*` seguia `idle` e presente na lista no fechamento.

## Rede sintética isolada

`desktop-native/tools/task4_fixture.py` foi servido só em loopback com token fictício e dados inventados, depois parado. Com POST atrasado 5 s, a UI mostrou “Enviando…” e botão desabilitado; após trocar para outra sessão sintética e voltar, manteve o mesmo bloqueio e o rascunho (`artifacts/task4-fixture-http-pending.png`, `task4-fixture-return-pending.png`). O segundo clique não gerou POST: a fixture contou **1** pedido para `slow accepted two`. O HTTP entregue mostrou “Enviado. Aguardando confirmação”; após quadro real de transcript, apareceu a bolha e o aviso provisório sumiu (`task4-fixture-confirmed.png`).

Quando a fixture fechou a conexão após receber `uncertain one`, o texto permaneceu no compositor, a UI pediu conferência antes de reenviar e o contador continuou em **1** (`task4-fixture-uncertain.png`). Uma recusa HTTP 409 com `detail` objeto mostrou “Motivo sintético: envio recusado”, com o texto preservado (`task4-fixture-rejected.png`). O 409 de `/interrupt?clear=false` mostrou “Motivo sintético: nenhum turno ativo” e gerou **1** pedido (`task4-fixture-interrupt-rejected.png`). A fixture registrou exatamente um POST para cada um de `slow accepted one`, `slow accepted two`, `uncertain one` e `reject one`.

O retorno `delivered:false` está implementado como “Na fila”, mas não foi exercitado na UI desta Task. Uma tentativa posterior de apontar a janela para a fixture perdeu foco para outra janela do desktop; o contador da fixture permaneceu em zero. Interrompi a automação e retirei a variante não exercitada da fixture, sem afetar o backend real. A composição IME também não foi simulada diretamente; o cliente usa o evento de envio do `TextareaState` em vez de capturar Enter globalmente.

## Build e limite das capturas

`cargo build --manifest-path desktop-native/Cargo.toml --locked` passou no Linux para a prova de fluxo; esse binário tem mtime **23/09/2026 18:54:58 −03:00**. `CARGO_BUILD_JOBS=2 cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run` compilou as regressões, sem executá-las. Os avisos de compilação são campos DTO ainda não lidos.

As capturas reais de envio/rascunho/interrupção foram feitas entre **17:55 e 18:02**, antes das correções posteriores. As capturas sintéticas iniciais de pendência, confirmação, incerteza e recusas são de 18:09–18:16. Elas sustentam os caminhos que não mudaram; as duas provas focadas da correção atual estão abaixo e são posteriores ao build final. A janela do binário final ficou acessível e conectada ao backend real no workspace 9, PID `3428386`, em `artifacts/task4-round2-real-restored.png`; releia a geometria antes de nova captura. A fixture foi parada, e a sessão `cx-native-task4-jf` continua viva para verificar recriação com o mesmo nome na Task 5.

## Correção após a revisão Opus e esclarecimento Astra

O cliente agora guarda os IDs de `user_msg` reais já vistos ao iniciar o POST e não usa esses IDs para confirmar o envio atual. Também recusa envio antes de instalar o histórico inicial. O rastreador mantém recusa/entrega incerta como resultado definitivo do HTTP: mesmo um eco **novo** de texto igual não apaga o aviso nem o rascunho; apenas HTTP aceito e eco novo podem retirar o estado provisório. Os casos de regressão para ID antigo, ID novo, recusa e incerteza foram adicionados a `src/delivery.rs` e compilados com `--no-run`, sem execução.

Na primeira prova focada, a fixture serviu um histórico com `old-real("uncertain one")`. A UI enviou outro “uncertain one”, trocou para outra sessão sintética e voltou durante o POST. Após a queda da conexão, a mensagem antiga seguia na conversa, o compositor mantinha o novo texto e o aviso de entrega incerta aparecia; houve **1** POST (`artifacts/task4-round1-backfill-pending.png`, `task4-round1-backfill-uncertain.png`). Essas duas capturas são anteriores ao ajuste adicional pedido pelo Astra.

No binário da **prova de fluxo** de 18:54:58, repeti somente o cruzamento ampliado: a fixture partiu da mensagem `old-real` e publicou também `other-client-new("uncertain one")` enquanto o novo POST estava pendente. A captura `artifacts/task4-round2-new-echo-pending.png` (18:57:50) mostra duas bolhas antigas/externas e “Enviando…” com o rascunho intacto. Após o HTTP incerto, `task4-round2-new-echo-uncertain.png` (18:58:11) mantém o mesmo rascunho e o aviso; a fixture registrou **1** POST e **2** eventos no histórico.

Após a aprovação do comportamento pelo Astra, fiz apenas os dois ajustes finais pedidos: a assertiva de `uncertainty_remains_visible_until_user_decides` agora espera `Some(Uncertain)` depois de um eco, e o botão Enviar fica visualmente desabilitado enquanto `!history_installed`, como o handler já fazia. `CARGO_BUILD_JOBS=2 cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run` compilou essa versão sem executar nenhum teste. Por instrução do Astra, não refiz capturas nem gerei novo binário de desenvolvimento; a janela acessível ainda usa o build de 18:54:58. A Task 5 deve compilar o código atual antes das provas finais.

## SHA-256 das fontes congeladas

```text
9ed1df2bef7d5b9bf84fed19075bce78dd30fa5c9d379d2fd9d372828832be6e  desktop-native/src/app.rs
c259ab5e88e911acee07b45e2f9fba6ccadd55eaf8a2efe80c290a955e208672  desktop-native/src/api/mod.rs
6c9d269791a6342b76df4ebf8c4811f8f56bcdd4f7aeb8d2d092aefb13fab181  desktop-native/src/delivery.rs
59b0d4f7fa0546fc32e036f34bb7ebdb8e1290ce7995b11eea0a4108b96ab8c3  desktop-native/src/main.rs
df374c2e0c26f0e274432f7bec770f0f368bd60ffd616538a65d51679a5771e1  desktop-native/tools/task4_fixture.py
e76b07b4cd781db33ecac0ae1786ec0000155bab0882098f1aa42b6b485c148c  messages/pt.json
08ae870bcfa49c5487270330116d950b26526649c48f2ddf9ad5882574a74139  messages/en.json
464f3f71950b75462f89bab349eda24d7e840fcf86065a5f91ba3979e3cd53ae  docs/superpowers/plans/2026-09-23-native-desktop-chat.md
```
