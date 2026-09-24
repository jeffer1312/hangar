# Task 3: streaming e recuperação

HEAD destacado: `55e0d909`. Executor `gpt-6-sol/high`, `CODEX_HOME=/home/jefferson/.codex-jefferson-felizardo`. Steps 7–9 implementados e conferidos; suas caixas no plano seguem abertas sob coordenação Astra. Fontes congeladas após os dois ajustes da revisão final, sem stage, stash, commit ou push.

## Implementação

- O parser SSE já instalado (`eventsource-stream` 0.2.3) continua responsável por UTF-8 dividido entre chunks, CRLF, comentários e vários `data:`. O cliente só avança um cursor `<stem>:<offset>` de mensagem real depois que a UI aceita o quadro. IDs de `ChatEvent`, mensagens da fila, `state`, `preview` e `ping` não viram cursor.
- O stream abre antes do GET de histórico. Mensagens, estados e prévias recebidos durante a carga ficam em memória e são aplicados após o snapshot. A costura preserva atualizações ao vivo, IDs já retirados da fila e a prévia corrente; falha de histórico mostra erro sem frase de conversa vazia.
- A fila sintética que chega antes ou depois da mensagem real só perde sua própria entrada ao confirmar. Uma mensagem real não elimina duas entradas iguais, nem dois eventos reais com texto igual são colapsados. `queued_delivered` permanece visível até confirmação. `desistiu`, `held:` e `hook_error` mostram aviso.
- Prévia com `md=true` renderiza Markdown; `md=false` mostra sintaxe literal. Quadros de prévia substituem o texto inteiro. Resposta correspondente, prévia vazia ou saída de `working` limpam a bolha; histórico atrasado não apaga uma prévia nova.
- `reset` aborta o GET antigo, limpa conversa, prévia, posição, buffer, ETag e cursor, então busca o novo histórico. Queda transitória mantém as mensagens com aviso. Reconexão usa `Last-Event-ID`, watchdog de 25 s e espera crescente limitada; 401 interrompe até correção do token e 429 respeita `Retry-After`. `list_error` conserva a lista com indicação de desatualização até novo `sessions`.

## Evidência sintética isolada

`desktop-native/tools/task3_fixture.py` serviu só dados inventados em loopback, com token fictício. Não é uma cópia do backend. O script foi parado ao terminar. A fixture atrasou o histórico em 1 s e enviou SSE com CRLF, comentário, dois `data:` e bytes UTF-8 divididos em chunks de 3 bytes. A janela mostrou o histórico antes da resposta em voo, uma só bolha para a entrada de fila confirmada e duas mensagens reais idênticas separadas (`artifacts/task3-fixture-two-real.png`).

Com o binário da **rodada 1**, compilado em **23/09/2026 17:15:17 −03:00**, as conexões da conversa registraram cursores `null`, `fixture-a:40`, `fixture-b:10`, `fixture-b:10`. O reset substituiu a conversa antiga pela nova (`artifacts/task3-fixture-reset.png`). A terceira conexão retornou 429 com `Retry-After: 3`; a quarta começou 3,01 s depois, pela diferença dos relógios monotônicos da fixture. A prévia Markdown, a prévia literal e a limpeza aparecem em `artifacts/task3-preview-markdown.png`, `task3-preview-plain.png` e `task3-preview-cleared.png`. `list_error` manteve a sessão com aviso e um snapshot posterior retirou o aviso (`artifacts/task3-list-stale-broadcast.png`, `task3-list-recovered-broadcast.png`).

| Captura do binário final | Horário local de 23/09/2026 |
| --- | --- |
| `task3-fixture-list.png` | 17:16:44 |
| `task3-fixture-resumed.png` e `task3-fixture-two-real.png` | 17:17:22 e 17:17:40 |
| `task3-preview-markdown.png` | 17:18:04 |
| `task3-preview-plain.png` | 17:18:28 |
| `task3-preview-cleared.png` | 17:18:53 |
| `task3-list-stale-broadcast.png` | 17:19:17 |
| `task3-list-recovered-broadcast.png` | 17:19:40 |
| `task3-fixture-reset.png` | 17:20:30 |
| `task3-real-restored.png` | 17:23:03 |

A primeira versão da fixture usava um sinal consumido por uma única conexão de lista. Outra conexão podia consumir o pedido de recuperação; isso foi corrigido na própria fixture com geração compartilhada para todas as conexões, antes das duas capturas finais da lista. Não foi falha do cliente.

## Evidência no backend real

O binário da primeira entrega conectou ao backend Hangar existente em `127.0.0.1:8765`, mostrou a lista e abriu o histórico/SSE da própria sessão `hangar-native-exec-jf` em modo leitura (`artifacts/task3-real-list.png`, `task3-real-chat.png`). Depois dos dois ajustes finais, o novo binário voltou ao mesmo backend e ficou acessível (`artifacts/task3-round2-real-restored.png`). O token foi lido somente em memória do `backend/.env` e digitado por stdin na entrada protegida, sem copiá-lo para esta worktree, URL, argv, captura ou log. Nenhum prompt foi enviado e nenhuma sessão foi interrompida. A janela final ficou aberta no workspace 9, PID `3158984` na conferência; releia a geometria antes de nova captura.

`cargo build --locked` passou no Linux. `CARGO_BUILD_JOBS=2 cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run` compilou os dez casos de regressão sem executá-los; os testes automatizados continuam não executados conforme o plano. Os avisos restantes são campos DTO ainda não usados. Reset, fila e 429 foram demonstrados na fixture sintética; não foram provocados nas sessões reais.

## Correções após o parecer da rodada 1

O parecer apontou a ausência dos casos de regressão e capturas anteriores à última edição de produção. **Depois** das capturas antigas, `app.rs` ganhou o aviso explícito de `held:` e o rótulo de carregamento no reset; `chat.rs` refinou a normalização usada para associar prévia e resposta. Após o parecer, só foram adicionados módulos `#[cfg(test)]` em `chat.rs` e `sse.rs`; nenhuma linha de produção mudou. O binário foi recompilado depois dessas adições, e todos os cenários sintéticos citados acima foram repetidos com ele.

Decided alone: casos em `#[cfg(test)]` dentro dos módulos em vez de `tests/*.rs`, porque o pacote é apenas binário e não tem `lib.rs`.

## Dois ajustes da revisão final Astra

- `Chat::apply` não usa novamente um ID real já presente em `claimed_real` para remover outra entrada da fila durante `merge_history`. O novo caso `history_merge_does_not_reuse_a_real_message_for_another_queue_entry` foi compilado, não executado. Na fixture focada, histórico atrasado contendo `real-one("ok")` foi costurado após `queued-one`, `queued-two` e `real-one`: a captura `artifacts/task3-round2-queue.png` (17:33:52) mostra o real e a segunda entrada ainda marcada “Na fila”, enquanto a primeira foi confirmada.
- `forward_stream` agora drena o canal intermediário quando o produtor termina, entregando o `Offline(401)` terminal; se o destino some, o produtor é cancelado. Na fixture focada, selecionar `synthetic-denied` abriu o diálogo “Token recusado” (`artifacts/task3-round2-sse-401.png`, 17:34:52). O contador de pedidos SSE dessa sessão permaneceu em **1** após 2,5 s, além do retry inicial que ocorreria em 1 s.

O binário desta correção foi gerado por `cargo build --manifest-path desktop-native/Cargo.toml --locked` em **17:31:40 −03:00**, depois dos dois arquivos Rust e da fixture. `CARGO_BUILD_JOBS=2 cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run` compilou o novo caso antes do build, sem executá-lo. Somente fila e erro SSE 401 foram repetidos no binário novo; reset, 429 e Markdown mantêm a prova da rodada 1 porque seus caminhos não mudaram. A fixture focada foi parada depois da captura, e a janela foi restaurada para o backend real às 17:37:13.

## SHA-256 das fontes congeladas

```text
c74df78a8a24d5642c165498ea4cc414c9176982de0aae45419139b8774827ce  desktop-native/src/api/sse.rs
72ce358e846ba63f002edb8fcfcbc7a65d3f08738a008f8d8f1450d9f30d6948  desktop-native/src/chat.rs
c931cec7e8b7ad0dc8009f0ab84b214965c42ff7759d34c06cce4012fc32a4df  desktop-native/src/app.rs
d08b4acdde3bf47dd1512a9d411ffd9062bb48a368620713ccc524e6c494cbb2  desktop-native/src/api/mod.rs
2f0fa00ba6f513c6b3452403de26b296db51803af55c99a15013c5d77281fd41  desktop-native/tools/task3_fixture.py
e76b07b4cd781db33ecac0ae1786ec0000155bab0882098f1aa42b6b485c148c  messages/pt.json
08ae870bcfa49c5487270330116d950b26526649c48f2ddf9ad5882574a74139  messages/en.json
e6feef72b1b349cdeb0e0d0169f0d0e9e41df85a0e23a9502b2378590af50489  docs/superpowers/plans/2026-09-23-native-desktop-chat.md
```
