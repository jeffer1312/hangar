# Task 2: leitura e ferramentas com detalhes progressivos

Base: `55e0d909` destacado, sobre a entrega congelada da Task 1 (hashes em `baseline-task2.json` do diretório operacional). Só `desktop-native/` e chaves `native_*` de `messages/{pt,en}.json` mudaram. Nenhum stage, stash, commit, push nem teste executado; testes foram só compilados.

## Equivalências com o chat web

| Web (Svelte/core) | Nativo (Rust) | Diferença deliberada |
|---|---|---|
| `agruparConversa` (core `toolGroups.ts`): `tool_result` não vira item; 3+ `tool_use` seguidos viram grupo `g-<1º id>`; pensamentos consecutivos viram bloco `p-<1º id>`; busca entre pensamentos entra no bloco | `conversation::build` com a mesma regra, `GROUP_MIN = 3`, mesmas chaves | Busca = `WebSearch`, `WebFetch`, `ToolSearch` fixo (o web deixa a pessoa escolher em Aparência; não há tela equivalente no nativo). |
| `MessageList` pareia `tool_result` → `tool_use` por `tool_use_id` (Map, o último vence) | `conversation::pair_results`: por `tool_use_id` com trim; cada resultado vai para a **primeira chamada ainda sem par** com o mesmo id | Id repetido não rouba o resultado da outra chamada; resultado sobrando fica visível. |
| Web esconde `tool_result` sem par | Linha própria "Resultado sem chamada", recolhível, com cópia | Pedido da Task: nada some. Exceção: `tool_use_id` `task:<id>` é sinal sintético do backend (`transcript.py`), não saída de ferramenta, e continua fora da lista, como no web. |
| `ToolCard`: linha com nome, resumo da entrada, estado; abre com comando e saída e botão copiar | `render_tool`: botão de disclosure (nome, resumo, estado) e, aberto, "Entrada" (JSON formatado) e "Resultado", cada um com cópia do texto **completo** | Sem diff visual de Edit nem etapas de MCP (`tool-progress`): ficam para um módulo posterior. |
| `summarizeToolInput` / `summarizeToolResult` | `conversation::summarize_input` (Read/Write/Edit, Bash, exec*, Grep/Glob, WebSearch/WebFetch, update_plan, chaves preferidas) e `tool_status` ("N linhas", "pronto", 1ª linha do erro) | Rótulo de resultado simplificado para "N linhas" (sem "carregadas/retornadas"). |
| `ToolGroup`: cabeçalho "Ferramentas: …" | Cabeçalho "Nome · N" (todas iguais) ou "N ferramentas" com os nomes distintos; "N com erro" em aviso; "em execução" se alguma roda | — |
| `ThinkingBlock`: linha recolhida com 1ª frase, conta buscas, abre no lugar | `render_thinking`: "Raciocínio" + 1ª frase (`thought_summary`, < 140), "1 busca"/"N buscas", texto em tom apagado, cópia do raciocínio inteiro | Sem tradução automática do pensamento (web chama `pensamentoEmPt`). |
| SSE `pensamento` / `ferramenta` em voo; some quando o registro durável chega; quadro vazio limpa após 3 s | `ChatUpdate::Thinking` / `LiveTool`, linhas `__thinking__` / `__tool__` no fim; `Chat::apply` limpa ao chegar `thinking` / `tool_use`; vazio ou `idle` agenda limpeza de 3 s por época | Pensamento em voo mostra só os últimos 1.500 caracteres; o durável tem o texto todo. |

## O que mudou

- `src/conversation.rs` (novo): montagem pura da lista (`Item::{Event, Tool, Group, Thinking, Orphan}`), pareamento, resumos, cerca de código segura (`fenced`) e corte por caracteres (`clip`). Os itens guardam **índices de `Chat::events`**, nunca a posição visual; a identidade de cada linha é o id do evento ou `g-`/`p-` do primeiro evento.
- `src/app.rs`: `sync_rows` monta `items`; linhas `Event` e a prévia seguem o caminho da Task 1 (`push_str` na prévia) sem mudança. Linhas de ferramenta/pensamento recalculam altura quando a assinatura muda (resultado chegou, grupo cresceu). Estado aberto/fechado em `expanded: HashSet<String>` por id; limpo em conexão nova, troca de sessão, sessão que sumiu e `reset` (`reset_details`, que também invalida os temporizadores das linhas em voo). Coluna de leitura centralizada com `max_w(820px)`. Detalhes grandes mostram 20.000 caracteres com aviso "Mostrando X de Y caracteres. Copiar leva o texto completo."; a cópia usa o texto original. Removido o `compact_tool` (cortava ferramentas de forma irreversível na tela).
- `src/chat.rs`: campos `live_thinking` / `live_tool`; `apply` limpa o item em voo quando o durável chega. Reconciliação de fila, prévia e entrega incerta intocadas.
- `src/main.rs`: registra `gpui_kit::assets::Assets`. Sem isso nenhum ícone aparecia (seta e copiar em branco).
- `tools/parity_tools_fixture.py` (novo): backend sintético com duas sessões, histórico misto, turno ao vivo (`/control/start`) e eventos extras (`/control/more`).
- Traduções `native_*`: entrada/resultado, copiar entrada/resultado, estados da ferramenta, grupo, erros, pensamento em voo, buscas e aviso de corte.

## Provas de interface (fixture sintética, binário final)

Janela própria no workspace 12 do monitor do notebook; a demo (PID 4190872) não foi tocada. Capturas em `../artifacts/`:

| Caso | Arquivo | Resultado |
|---|---|---|
| Conversa mista recolhida, erro visível | `parity-task2-collapsed-error.png` | "Bash cargo build" com o erro em laranja na linha fechada; grupo "4 ferramentas … 1 com erro". |
| Grupo aberto | `parity-task2-group-open.png` | Abre no lugar; o conteúdo acima não se move. |
| Detalhe com resultado longo | `parity-task2-tool-detail.png`, `parity-task2-clipped.png` | Entrada formatada, resultado em bloco monoespaçado, aviso "Mostrando 20000 de 98399 caracteres". |
| Copiar completo | — | "Copiar resultado" colocou 98.399 caracteres / 1.200 linhas no clipboard, idênticos à fonte (comparação byte a byte). O clipboard anterior foi restaurado. |
| Teclado | `parity-task2-keyboard-focus.png`, `parity-task2-keyboard-sheet.png` | Tab chega ao cabeçalho com anel de foco; Espaço abre, Enter fecha; foco e posição de leitura mantidos. |
| Pensamento | `parity-task2-thinking-open.png` / `parity-task2-final.png` | Linha "Raciocínio" + 1ª frase + "1 busca"; aberto mostra os dois pensamentos, a busca entre eles e "Copiar". |
| Em voo → durável | `parity-task2-live-sheet.png`, `parity-task2-live-final-sheet.png`, `parity-task2-live-times.txt` | "Pensando" cresce; ao chegar o `thinking` vira "Raciocínio" sem duplicar; "Bash … em execução" em voo vira a chamada registrada e depois "2 linhas"; prévia vira resposta final uma vez. |
| Leitura acima enquanto chega evento | `parity-task2-hold-sheet.png`, `parity-task2-hold-end.png` | Captura antes/depois de `/control/more` **idêntica pixel a pixel**; "Ir para o fim" mostra a mensagem nova e a ferramenta já pareada. |
| Trocar e voltar | `parity-task2-switch-sheet.png` | A outra sessão mostra só as mensagens dela; na volta o histórico recarrega com tudo recolhido. |
| Honestidade do estado | (nas capturas acima) | `Bash sleep 999` sem resultado aparece "sem resultado" (há mensagem depois dela); só a chamada da cauda em sessão `working` aparece "em execução". |

Limites: na volta à sessão os eventos que chegaram só pelo SSE somem porque a fixture devolve histórico fixo; isso é da fixture, não do cliente. Paginação de histórico antigo ("Mais antigas") e evento `reset` não foram exercitados na janela; o código limpa expansão e temporizadores nesses caminhos. Não houve prova contra sessão real: só leitura de código do backend (`transcript.py`, `rollout.py`, adaptadores Pi/Kimi) para confirmar que `tool_use_id` é o identificador da ferramenta e que `id` do evento é outro.

## Rodada 2 (conferência final da árbitra)

Dois ajustes pedidos em `pareceres/task2-r2-astra.md`:

1. **Registro antigo não apaga item em voo de outro.** `Chat::apply` só consolida o que está em voo quando o evento é **novo** (id desconhecido) e **corresponde** ao item: pensamento durável cujo texto contém o texto em voo (espaços normalizados); chamada com o mesmo nome e a mesma entrada (ou entrada em voo vazia). Replay de id conhecido não mexe em nada. `merge_history` guarda prévia, pensamento/ferramenta em voo e o último registro consolidado antes de reaplicar o histórico e restaura depois. Quadro atrasado igual ao pensamento já registrado é ignorado (`update_live_thinking` → false); o da ferramenta também, até chegar o resultado dela (depois disso uma chamada idêntica volta a aparecer em voo). Nenhum evento durável é deduplicado por texto; entrega de mensagens intocada.
2. **Resultado nunca liga a chamada posterior.** `pair_results` percorre em ordem e cada resultado pega a chamada **anterior** mais antiga ainda sem par com o mesmo `tool_use_id`; sem candidata anterior, fica órfão e a chamada futura continua livre para o próximo resultado.

Provas na mesma fixture (`/control/r2`, histórico com 410 eventos para o botão "Carregar anteriores"), binário final:

| Caso | Arquivo | Resultado |
|---|---|---|
| `result(old,x), call(new,x), result(new,x)` | `parity-task2-r2-orphan-before-call.png` | "Resultado sem chamada: saída antiga, anterior à chamada"; "Bash echo repetido" com "1 linha" (pareado com o resultado seguinte). |
| B em voo + replay de A + "Carregar anteriores" | `parity-task2-r2-live-sheet.png` (quadros 1-3) | "Pensando / Pensamento B em andamento" e "Bash echo B em execução" continuam após o replay de `t1`/`evt-read` e após o clique, que reaplicou o histórico (o botão some porque 410 < 800). |
| B consolidado + quadros atrasados | `parity-task2-r2-after-late-frames.png`, quadros 4-6 | "Raciocínio: Pensamento B em andamento, concluído." e uma única linha "Bash echo B"; o `pensamento` atrasado (0,3 s após o registro) e a `ferramenta` atrasada não recriaram linha em voo; ao fim "1 linha". Ordem dos quadros enviados em `parity-task2-r2-frames.txt`. |

Par que só completa após paginação: coberto por teste compilado (`page_that_starts_mid_pair_pairs_after_older_page_arrives`); a fixture devolve sempre o histórico inteiro, então não há página parcial para exibir na janela. Teclado, cópia e layout não mudaram nesta rodada e não foram repetidos.

## Compilação

- `cargo build --manifest-path desktop-native/Cargo.toml --locked` (rodada 2, após a última edição): `Finished dev profile [unoptimized] target(s) in 2.41s`; os mesmos três avisos de campos DTO não lidos da Task 1.
- `cargo test --manifest-path desktop-native/Cargo.toml --locked --no-run` (rodada 2): `Finished test profile [unoptimized] target(s) in 2.14s`. Casos novos compilados, **não executados**: pareamento por `tool_use_id` com espaços, id repetido + resultado sobrando, resultado anterior não liga a chamada posterior, par completo após página antiga, grupo com id estável, busca dentro do pensamento, sinal `task:` fora da lista, resumos, cerca/corte com acentos, durável substituindo em voo, replay/histórico preservando item em voo de outro, quadros atrasados ignorados.
