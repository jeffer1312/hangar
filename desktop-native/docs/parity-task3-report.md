# Paridade Task 3 — fila, perguntas e aprovações

Base: HEAD `55e0d909` destacado, árvore com as Tasks 1 e 2 aprovadas (baseline `baseline-task3/`). Executor Opus 5.5/medium, conta `claude-200-3`.

## Step 7 — contratos conferidos

Fonte: `backend/app/api.py`, `models.py`, adaptadores (`claude_headless`, `codex/questions.py`, `terminal_input.py`), `packages/core/src/{api,types,proposedPlan}.ts` e os componentes `AskQuestionStepper`, `OptionButtons`, `SessionPlanPreview`, `MessageList`, `Composer`, `Chat.svelte` (lidos como referência; nada em Svelte foi editado; a skill `svelte-code-writer` foi carregada e nenhuma ferramenta dela se aplica a leitura).

| Ação | Contrato real | Nativo |
|---|---|---|
| SSE `ask_question` | Claude com terminal: `{questions}` sem id/provider, reenviado a cada reconexão, sem evento de "limpo" (fecha quando o estado sai de `awaiting_input`). Codex e Claude sem terminal: payload com `provider` e `request_id` (número ou texto), `null` = limpo. | `AskPayload` guarda `request_id` como JSON cru. Pergunta idêntica (mesmo JSON) não refaz o formulário; conteúdo diferente, `null`, troca de sessão, `reset` ou nova conexão refazem. Pergunta não-Codex fecha quando o estado sai de `awaiting_input` (regra do web). |
| `POST /answer` | `{answers, request_id?}`, corpo estrito (`extra="forbid"`). `option`: índices **0-based** + `labels` + `multi`; `text`: `value`, `type_index = nº de opções`, `labels:[value]`; `chat`: `chat_index = nº de opções + 1`; `question_id` só se a pergunta tiver `id`. Resposta `{ok, fallback}`. 409 pergunta mudou; 503 sem confirmação. | `interaction::answer_body` monta o mesmo corpo; só envia com todas as perguntas respondidas; Codex sem "conversar" e com texto só quando `isOther` ou sem opções. `fallback:true` vira aviso visível. |
| Segredo | Vai em claro no corpo; a máscara é só visual. | Campo `masked`, aviso "fica oculta na tela, mas vai ao agente como texto". Nada é registrado em log. |
| `POST /select {option}` | 1-based, `ge=1, le=50`, sem `request_id`: o backend não tem como saber qual pergunta o clique viu. | Botão numerado envia `i+1`. Antes do POST o cliente compara pergunta+opções atuais com as que estavam na tela no clique; diferente → não envia e avisa. **Limite:** essa conferência é local; entre o POST e o pane o backend não protege. |
| `POST /select/submit` | Sem corpo; só múltipla escolha do terminal (rótulo com `[ ]`/`[✔]`). | Caixas de marcar (cada toque é um `/select`), botão "Enviar marcadas (n)" desabilitado com zero. |
| Cancelar | `POST /interrupt` (web: `clear` só com pendente). Sem terminal ele também nega a pergunta. | "Cancelar" do cartão = `/interrupt?clear=false` (o `api.interrupt` existente). |
| Plano Claude sem terminal | `claude_plan_pending {plan, path}` + `question/options` ("Aprovar plano"/"Continuar planejando") respondidos por `/select`. | Plano em Markdown (rolagem própria) + caminho + as opções reais. |
| Plano Codex | `<proposed_plan>` na última resposta (antes de mensagem real do usuário); "Implementar" = `POST /codex/plan/implement` sem corpo, só com estado `idle` e fila vazia; "Continuar planejando" só dispensa na tela. A rota usa o pane (só com terminal). | Porta de `proposedPlan`/`planDisplayText` (marcas fora de cerca de código somem do texto). Barra aparece só para Codex com terminal; "Implementar" habilitado só com `idle` e fila vazia, confere que o plano atual é o do clique. |
| Fila | Bolha `queued-<id>`; `DELETE /queue/{id cru}` só para `desistiu`; `POST /steer` sem corpo: `{promoted, confirmed, queued_ids}` (Codex/sem terminal: `promoted:false` com `queued_ids`; Kimi/Claude terminal: `promoted` pela tecla). 409/502 mantêm a fila. | "Descartar" só em bolha `desistiu`, a bolha sai só após 200. "N na fila · enviar agora" em sessão `working` com fila, para Codex, Kimi ou sem terminal (o web também não oferece ao Claude com terminal). `promoted` retira as bolhas; senão marca `queued_ids` como "entregando" e espera `queue_confirmed`. |

Não incluído (fica explícito; os três primeiros a árbitra registra na Task 5): aprovação do Codex antes da thread (sessão sem `jsonl`); "Implementar" do Claude sem terminal parado em modo plano (troca `permission-mode` e envia texto, com reversão — muda o modo da sessão); descoberta de plano do Claude com terminal (`/plan-preview`); orientar turno com texto (`/steer {text}`, ação do compositor, Task 4); cor por tipo de permissão (allow/deny) do `OptionButtons`.

## Step 8 — implementação

- `src/interaction.rs` (novo): `Pick`, `Ask` (payload + impressão digital do JSON), `answer_body`, `toggle`, `checkbox` (caixinha do terminal), `proposed_plan`/`plan_display`, `Action` (rota + prazo) e `InFlight` (uma mutação por sessão).
- `src/api/dto.rs`: `AskPayload/AskItem/AskOption`, `PlanPending` no estado, `Steered`.
- `src/api/mod.rs`: `Api::act` — POST/DELETE sem nova tentativa; queda depois de enviar vira incerteza.
- `src/chat.rs`: `ask` no lugar do booleano, `update_ask`, `retire`, `steered`.
- `src/app.rs`: cartões de pergunta, de opções/plano e barra do plano Codex; "Descartar" na bolha desistida; "enviar agora" no rodapé; aviso por sessão. Prazo 60 s para `/answer` (acima dos 35 s do backend no Codex), 30 s nas demais.
- Traduções `native_*` (pt/en) das novas ações; `native_pending_question` agora só aparece quando não há cartão (overlay, login ou seletor sem opções legíveis) e diz para usar o terminal ou o Electron.
- `tools/parity_interactions_fixture.py` (novo): backend sintético com 7 sessões e controle `/control/mode?next=ok|409|503|drop|slow|fallback`, `/control/change`, `/control/withdraw`, `/control/log`.

Proteções: nada é selecionado ou enviado sozinho; o clique confere o pedido atual contra o que estava na tela; uma mutação por identidade de sessão (servidor+nome+jsonl), outras sessões seguem livres; resultado que chega depois de trocar de sessão só grava o aviso daquela sessão; resultado de resposta só fecha a pergunta se ela ainda for a mesma; formulário zera em qualquer troca de pergunta, sessão, conexão ou `reset`; rascunho do compositor não é tocado.

## Step 9 — conferência em fixture sintética

Tudo abaixo é **fixture sintética** (`tools/parity_interactions_fixture.py`, porta 8791 própria), com a janela nativa própria. Nenhuma sessão real foi respondida, nenhuma fila real foi tocada, a demo (PID 4190872) não foi tocada. Contagem de requisições pelo `/control/log` da fixture.

| Caso | Captura (`../artifacts/`) | Resultado |
|---|---|---|
| Pergunta Claude aberta | `parity-task3-ask-open.png` | 2 perguntas (única com descrição e prévia monoespaçada; múltipla), nenhuma opção marcada, "Enviar respostas" desabilitado, "Responda todas…". |
| Escolha | `parity-task3-ask-picked.png` | SQLite + Lint marcados; nada enviado (log 0). |
| Recusa 409 | `parity-task3-ask-409.png` | 1 POST `{"answers":[{"kind":"option","indices":[1],"multi":false,"labels":["SQLite"]},{"kind":"option","indices":[0],"multi":true,"labels":["Lint"]}]}`; mensagem do servidor visível; escolhas mantidas. |
| Pedido trocado com envio em voo | `parity-task3-ask-inflight.png`, `-ask-changed-inflight.png`, `-ask-changed-after.png` | Botão "Enviando…" desabilitado; a pergunta nova aparece com formulário vazio; resposta tardia (409) só vira aviso; nenhum reenvio (2 POSTs no total, ambos por clique). |
| Texto livre + conversar + entrega por texto | `parity-task3-ask-text-chat.png`, `-ask-fallback.png` | `{"kind":"text","value":…,"type_index":2,"labels":[…]}` e `{"kind":"chat","chat_index":4}`; `fallback:true` mostra "entregues como mensagem de texto"; cartão fecha. |
| Codex | `parity-task3-codex-open.png`, `-codex-secret.png` | Sem "Conversar"; "Digitar resposta" por `isOther`; pergunta sem opções abre o campo direto; segredo mascarado; corpo com `"request_id": 42` numérico e `question_id`. |
| Entrega incerta | `parity-task3-codex-uncertain.png` | Conexão derrubada pela fixture após receber: "Sem confirmação do servidor. Confira a conversa antes de repetir."; nenhuma nova tentativa; formulário mantido. |
| Troca de sessão com envio em voo | `parity-task3-switch-other-session.png`, `-switch-back.png` | Outra sessão com opções clicáveis durante o envio; na volta, "Respostas enviadas." e a conversa atualizada. |
| Permissão (`/select`) | `parity-task3-perm-selected.png` | `{"option":1}`; "Escolha enviada. Aguardando a sessão."; cartão some quando o estado muda. |
| Múltipla do terminal | `parity-task3-multi-open.png`, `-multi-toggled.png`, `-multi-submitted.png` | Toque = `/select {"option":1}`, a caixa reflete o estado que volta do servidor; "Enviar marcadas (2)" = `/select/submit` sem corpo. |
| Plano Claude sem terminal | `parity-task3-plan-pending.png` | Título, Markdown com lista e código, caminho, "1. Aprovar plano"/"2. Continuar planejando", "Cancelar". |
| 503 e cancelar | `parity-task3-plan-503-before-fix.png`, `-plan-cancel.png` | 503 aparecia só como incerteza; **corrigido** para "motivo do servidor + incerteza" (código revisto, não recapturado). Cancelar = `POST …/interrupt` (a rota do `api.interrupt` existente, com `clear=false`). |
| Plano Codex | `parity-task3-codex-plan-markers-before-fix.png`, `-codex-plan.png`, `-codex-plan-implemented.png` | Marcas `<proposed_plan>` apareciam cruas; **corrigido** (porta de `planDisplayText`) e recapturado. "Implementar plano" = 1 POST sem corpo; a barra some quando chega a mensagem real. |
| Fila | `parity-task3-queue-open.png`, `-queue-discarded.png`, `-queue-steered.png` | Bolhas "Na fila" e uma desistida com "Descartar"; `DELETE …/queue/ccc` e a bolha sai após 200; "2 na fila · enviar agora" = `/steer` sem corpo → "2 mensagem(ns) da fila entregue(s) ao turno." e as bolhas saem pelo `queue_confirmed`. |

Correção durante a conferência: o botão "Enviar respostas" dependia de uma repintura casual para refletir o texto digitado; agora cada edição do campo redesenha o cartão.

Segunda etapa, janela própria no workspace 12 (que abriu no monitor DP-3, saída física existente; ao fim o DP-3 voltou ao workspace 9), mesma fixture, binário final, foco/PID/workspace conferidos antes de cada lote:

| Caso | Captura | Resultado |
|---|---|---|
| Teclado | `parity-task3-keyboard-focus.png`, `-keyboard-selected.png` | Tab percorre "Ir para o fim" → "Copiar" das mensagens → Postgres → SQLite → "Digitar resposta" com anel de foco visível; Shift+Tab volta; Espaço marca SQLite (rádio) e Lint (caixa); "Enviar respostas" habilita; 0 requisições. |
| Pergunta recolhida com formulário preenchido e rascunho | `parity-task3-withdrawn-draft-kept.png` | `/control/withdraw` (`ask_question` `null` + estado `idle`): o cartão some com as escolhas, o rascunho "rascunho de teste" continua no compositor, 0 requisições. |

**Limite:** sessão recriada com a mesma pergunta só por teste compilado (o formulário zera em `reset_details`, chamado na troca de `jsonl`). Não houve prova contra backend real (sem sessão descartável com pergunta pendente). Enter no botão focado não foi exercitado.

Revisores automáticos (`ecc:code-reviewer`, `ecc:silent-failure-hunter`, modelo herdado da sessão) foram disparados e **parados antes de concluir** por ordem da árbitra (limite de duas revisões por Task); nenhum achado deles foi usado.

**Incidente de ambiente:** a janela de teste rodou numa saída virtual `T3NATIVE` criada com `hyprctl output create headless`. Isso fez o `displaylayout.sh` do usuário gravar uma linha `T3NATIVE` em `~/.config/hypr/monitors.lua`, e a saída ficou sobreposta ao DP-3 a partir de y=971: três cliques nessa faixa caíram na barra do DP-3 e abriram o painel lateral do quickshell. Desmontagem: painel fechado (`quickshell:sidebarRightToggle`), janela e fixture paradas, saída removida; o próprio gerador reescreveu o `monitors.lua` sem a linha (conferido por diff contra o estado registrado antes: só a linha `T3NATIVE` saiu). Sem reload. A árbitra vetou repetir esse método.

## Rodada 2 (correção do parecer `pareceres/task3-r1.md`)

Janela própria no workspace 12 (monitor DP-3, devolvido ao workspace 9 no fim), fixture sintética, binário final, foco/PID/workspace conferidos antes de cada lote. Sem saída virtual, sem revisores automáticos.

| Bloqueio | Correção | Prova (`../artifacts/`) |
|---|---|---|
| 1. Fila contava entregue ao processo | `Chat::waiting` (porta de `queuedMessages`): no Codex e sem terminal, `queued_delivered` não espera mais; desistida nunca conta. `queued_count` usa ela (botão, rótulo e liberação do plano Codex). Fixture: `/steer` só promove não entregues, como o `claim_undelivered`, e `queue_confirmed` chega 4 s depois. | `parity-task3-r2-queue-one-waiting.png` ("1 na fila · enviar agora" com uma entregue + uma pendente + uma desistida); `-queue-after-200.png` (botão some, a pendente vira "Entregue ao agente"); `-queue-confirmed.png` (sai com o `queue_confirmed`). 1 POST `/steer`. |
| 2. Perguntas de Pi/omp/Kimi | `interaction::ask_from_events`: último `tool_use` (`question`/`ask`/`AskUserQuestion`) sem `tool_result`; omp só a 1ª pergunta; Kimi `multi_select`; Pi pergunta única; formato inesperado → nada (cartão cru de opções segue). `Ask.tool_use_id`; impressão `tool:<id>:<entrada>`. Derivada no fim de `sync_rows`; não fecha pelo estado, fecha pelo `tool_result`; resposta aceita guarda o id em `answered_tools` (zera na troca de sessão) para não reabrir antes do `tool_result`. | `parity-task3-r2-ask-pi.png`, `-ask-omp.png` (só "Qual formato?"), `-ask-kimi.png` (múltipla); `-kimi-picked.png`; POST `{"answers":[{"kind":"option","indices":[0],"multi":true,"labels":["linux"]}]}` sem `request_id` e sem `question_id`; `-kimi-after-200.png` (cartão fechado, "Respostas enviadas.", `tool_use` ainda "sem resultado"); `-kimi-after-result.png` (resultado chegou, cartão não voltou). |
| 3. Cartão cortava opções sem sinal | A receita literal (`overflow_y_scrollbar`) **não serviu**: sem altura fixa (só `max_h`) a área de rolagem do `Scrollable` cresce com o conteúdo, a barra não aparece (modo padrão `Scrolling`) e a roda deixa de rolar (`parity-task3-r2-recipe-scrollable-no-bar.png`, `-recipe-scrollable-no-scroll.png`). Alternativa aprovada pela árbitra: o `overflow_y_scroll` que já rolava + `ScrollHandle` guardado na view (novo só quando muda a pergunta/plano) + `Scrollbar::vertical(..).mode(ScrollbarMode::Always)` sobreposto num contêiner relativo, com recuo à direita no conteúdo para a barra não cobrir controles. Só nesses dois cartões; tema global intocado. | `parity-task3-r2-ask-scrollbar.png` (barra à direita com o cartão cortado em "Lint"); `-ask-scrolled-wheel.png` (roda leva a "Tipos"/"Testes", polegar embaixo); `-plan-long-scrollbar.png` e `-plan-long-scrolled.png` (plano de 20 passos com barra e rolagem). |

Pedidos extras do parecer:

- 503 recapturado depois da correção: `parity-task3-r2-503-reason-uncertain.png` — "Não foi possível confirmar o envio da resposta ao Codex. Sem confirmação do servidor. Confira a conversa antes de repetir." (o texto do motivo é o da fixture). A captura `plan-503-before-fix` fica só como histórico.
- Digitação única: `parity-task3-r2-typed-once.png` mostra "MariaDB" uma vez; o POST saiu com `"value":"MariaDB"`. O "MariaDBMariaDB" da rodada 1 foi digitação minha repetida (a primeira não aparecera na captura), não defeito do campo.

Achado e corrigido nesta rodada: depois de responder uma pergunta de Pi/omp/Kimi, até o `tool_result` chegar, aparecia "Há um pedido pendente que esta janela ainda não consegue responder", falso. Agora esse aviso não aparece enquanto a pergunta respondida espera o resultado (`-kimi-after-200.png`).

### Ajuste da conferência final (`pareceres/task3-r2-astra.md`)

A marca de pergunta do transcript respondida era gravada só se a sessão estivesse aberta quando o 200 chegasse, e apagada em toda troca de sessão, então a pergunta respondida podia reabrir antes do `tool_result`. Agora:

- `answered_tools` guarda **(SessionKey, id da ferramenta)** e sobrevive à troca de seleção. Troca de `jsonl` (sessão recriada) já é outra chave; `reset` do transcript apaga as marcas e o envio pendente daquela sessão.
- O id da ferramenta é capturado **no clique** (`answering[SessionKey]`), e o 200 grava a marca com esse id, mesmo com a sessão em segundo plano. O formulário aberto só é fechado se for a mesma pergunta daquela sessão.

Prova (fixture, `/control/hold` retém o `tool_result`, `/answer` lento):

| Passo | Captura | Resultado |
|---|---|---|
| Kimi: marca "linux", envia, troca para omp com envio em voo | `parity-task3-r2b-kimi-inflight.png`, `-other-session-after-200.png` | "Enviando…"; a omp mostra o próprio cartão, intacto, quando o 200 do Kimi chega em segundo plano. |
| Volta ao Kimi depois do 200, resultado retido | `parity-task3-r2b-back-no-reopen.png` | Cartão não reaparece; `tool_use` "sem resultado"; "Respostas enviadas.". |
| Sai e volta de novo antes do resultado | `parity-task3-r2b-again-no-reopen.png` | Continua sem cartão. |
| Libera o resultado; nova pergunta (outro id) | `parity-task3-r2b-result-arrived.png`, `-new-question.png` | Resultado "1 linha"; a pergunta nova abre com formulário vazio. |

Uma requisição no total: `POST /api/sessions/ask-kimi/answer {"answers":[{"kind":"option","indices":[0],"multi":true,"labels":["linux"]}]}`. Build e `cargo test --no-run`: `Finished`; nada executado.

**Limite registrado:** Tab alcança "Tipos" e "Testes" (o anel some da área visível), mas o foco **não rola o cartão** até eles; roda do mouse e arraste da barra alcançam (`parity-task3-r2-ask-tab-focus-hidden.png`). Escolher por teclado uma opção fora da área visível exige rolar antes.

## Decidido sem o plano dizer

- Perguntas num cartão único com todas as perguntas visíveis (o web usa passo a passo com tela de revisão); o envio continua sendo um botão explícito, habilitado só com todas respondidas.
- Erro de `/answer` mantém o cartão com as escolhas para qualquer provider (o web fecha a pergunta nos não-Codex).
- 5xx mostra o motivo do servidor junto do aviso de incerteza, sem nova tentativa.
- "Implementar plano" oculto para Codex sem terminal (a rota lê o pane); o web não barra.
- Descarte de plano Codex ("Continuar planejando") é só local, por id da mensagem, como no web.

## Compilação

- `cargo build --manifest-path desktop-native/Cargo.toml --locked` (após a última edição): `Finished dev profile [unoptimized] target(s)`; os mesmos 3 avisos de campos DTO não lidos das Tasks anteriores.
- Rodada 2: mesmos comandos após a última edição, `Finished dev profile` e `Finished test profile`, mesmos 3 avisos. Novos casos compilados, não executados: fila entregue × pendente × desistida por provider; perguntas de Pi/omp/Kimi (omp só a 1ª, Kimi múltipla, Pi sem opções → nada, `tool_result` do mesmo id → nada, provider sem pergunta de transcript → nada).
- `cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run`: `Finished test profile [unoptimized] target(s)`. Casos novos compilados, **não executados**: corpo de `/answer` igual ao web (índices 0-based, `request_id` cru, texto, conversar), respostas incompletas/indevidas não saem, escolha única × múltipla, caixinha do terminal, `proposed_plan`/`plan_display` com cerca, uma mutação por sessão e resultado atrasado de outra ação ignorado, retrato idêntico de pergunta não zera escolhas e pergunta do Claude fecha ao sair do aguardo, `/steer` marca só os ids listados ou retira tudo com `promoted`.
- `messages/{pt,en}.json` válidos; fixture compila (`py_compile`).
