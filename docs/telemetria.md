# Telemetria anônima do Hangar

Com sua autorização, o Hangar envia ao desenvolvedor contagens de uso e preferências, falhas e
passos anteriores à falha. Envia um retrato por hora e um final ao fechar o dia UTC. Não envia o
diário bruto nem linhas genéricas de log; conversa, texto digitado, nome e endereço ficam de fora.
Os dados recebidos ficam por 400 dias.

A lista abaixo descreve o que o código faz. A fonte é
[`crates/hangar-telemetry/src/events.rs`](../crates/hangar-telemetry/src/events.rs) (eventos e valores
possíveis) e os pontos do servidor que chamam a contagem.

## O que sai

Só o servidor do Hangar (`hangar-server`) de cada máquina envia. O celular e o desktop nunca falam
com o receptor: eles avisam o próprio servidor, que conta.

Cada pacote é um dia (UTC) de uma máquina e leva:

- `schema`, `batch_id` (aleatório, para o receptor não gravar o mesmo pacote duas vezes);
- `machine_id` e `owner_id`: dois números aleatórios gerados na máquina, sem relação com nome,
  conta ou outro identificador do Hangar (veja "Dono de várias máquinas");
- `day`: o dia, sem horário;
- `env`: versão do Hangar, commit (só em pacote de release; fora disso, `dev`), sistema,
  arquitetura e canal de atualização (`latest`, `beta` ou `other`);
- `counters`: a lista de contadores `(evento, dimensões, faixa) → quantidade`;
- `partial`: aparece nos retratos por hora, não no final;
- `records`: até 200 falhas agrupadas por ação, nível, código e etapa, com primeira e última hora UTC,
  quantidade, descrição fixa e duração em milissegundos quando conhecidas, e até 10 passos anteriores;
- `trail`: até 5.000 passos por dia, com hora UTC, ação, nível e tela quando conhecida. Passos iguais
  seguidos têm uma contagem só. Cortes dos registros ou da trilha entram em `diary_dropped`.

As dimensões vêm de listas fixas, booleanos ou versões. Quando a origem foi confirmada, um rótulo
fora da lista vira `other`; origem não confirmada fica fora. Não há texto livre no pacote.

Tempo vai em faixas: `lt100ms`, `lt300ms`, `lt1s`, `lt3s`, `lt10s`, `lt30s`, `ge30s`. Nos
eventos marcados com "(faixa)", cada ocorrência soma 1 na faixa da duração.

### Uso

| Evento | Dimensões | O que conta |
|---|---|---|
| `app_visible_minutes` | `client` (`ios`, `android`) | Minutos com o Expo na tela. Só conta se o início e o fim da medida caem no período ativo e no mesmo dia UTC; no máximo 5 min por aviso ao servidor ativo |
| `message_sent` | `provider`, `surface` (`terminal`, `headless`), `steer` (sim/não), `origin` (`person`, ou `session` quando a mensagem é recado de outra sessão) | Mensagem que você mandou pelo app ou pela API |
| `session_opened` (faixa) | `provider`, `surface`, `model` (lista curta de modelos; motor do `engines.json` sai `engine`, nunca o nome), `effort`, `permission`, e sim/não para `restore`, `jev`, `prewarm`, `worktree` | Sessão que você abriu, com o tempo que levou. Sessão aberta por convidado não conta |
| `feature_used` | `feature` (código da lista `FEATURE_ROUTES`, como `session.rename` ou `search`) | Ação sua que deu certo numa rota da lista. Leituras que os apps repetem sozinhos (lista, conversa, status) não contam |
| `mcp_tool` | `tool` (nome da tool do MCP `hangar`), `error` (sim/não) | Chamada de tool do MCP `hangar` por um agente. Sessão de convidado com login não conta |
| `voice_call` (faixa) | — | Duração da chamada de voz |
| `dictation` (faixa) | `route` (`ditado`, `relimpar`, `sessao`), `status` (`ok`, `warning`, `error`, `cancelled`) | Ditado, com o tempo total |
| `session_placed` | `outcome` (`here`, `elsewhere`, `refused`), `reason` (`same_folder`, `unpublished`, `plain_dir`, `project_rule`, `local_within_limits`, `least_loaded`, `no_candidate`, `not_balanced`), `clone` (sim/não) | Sessão que você pediu com a escolha automática de máquina: onde ela ficou e por quê. A sugestão que a tela mostra antes não conta; máquina e projeto não saem |

`provider` é `claude`, `codex`, `pi`, `omp`, `kimi`, `antigravity` ou `other`.

### Erros

| Evento | Dimensões | O que conta |
|---|---|---|
| `session_open_failed` | `provider`, `code` | Sessão que você pediu e não abriu, também a recusada antes de começar (nome em uso, pasta fora do permitido, worktree) |
| `send_failed` | `code` | Mensagem recusada |
| `client_fatal` | `client`, `code` (tipo do erro, como `typeerror`) | Queda do app com horário comprovado. O texto do erro nunca sai |
| `diary_dropped` | `part` (`records`, `trail`) | Registros e passos fora dos tetos do formato ou do tamanho em bytes |

### Atualização

| Evento | Dimensões | O que conta |
|---|---|---|
| `update_finished` | `result` (`updated`, `updated_manual`, `up_to_date`, `failed`), `from` e `to` (versões), `channel`, `origin` (`manual`, `auto`, `other`) | Fim de uma atualização |
| `update_step` (faixa) | `step` (`download`, `swap`, `steps`, `restart`, `other`), `ok` (sim/não) | Tempo de cada etapa da atualização |
| `update_failed_code` | `code` | Código do erro da atualização que falhou |

### Máquinas, rede e lentidão

| Evento | Dimensões | O que conta |
|---|---|---|
| `peers` | `kind` (`lan`, `tailscale`, `connect`, `public`, `disabled`) | Uma vez por dia: quantas máquinas do `peers.json` há de cada tipo. O tipo é decidido na máquina; o endereço não sai |
| `client_route` (faixa quando há tempo) | `client`, `failed` (sim/não), e `route` (`principal`, `rede_local`, `tailscale`, `connect`) ou `code` da falha | Caminho que o app escolheu para chegar ao servidor |
| `server_startup` (faixa) | — | Tempo de subida do servidor |
| `chat_open` (faixa) | `client` | Tempo para abrir uma conversa |

### Preferências

Uma vez por dia (UTC), com o aceite do escopo 3, o servidor conta as preferências dele, e o app do PC
e o do celular mandam ao servidor ativo (nunca a um convite) o retrato das preferências daquele
aparelho. O mesmo aceite da opção B cobre esse retrato: não há segundo aviso. O servidor confirma o
retrato do app em `POST /api/telemetry`, fora do diário. Só aceita o período de coleta ativo e uma
captura do dia UTC da chegada, com até um minuto de diferença para o futuro. Envio atrasado não ocupa
a vaga do dia nem reabre pacote fechado. Cada par vira um contador `settings` com `client`
(`native`, `ios`, `android` ou `server`), `setting` e `value`. O servidor conta no máximo um
retrato por dia de cada app e aparelho: o app manda junto o id aleatório que já guarda para as
novidades da atualização. A marca do aparelho e os contadores são gravados juntos no arquivo local do
dia antes da confirmação; o id não entra no pacote. Preferência fora da tabela é descartada; valor
fora da lista sai `other`. Nome de fonte, cor escolhida, modelo e qualquer número não saem.

As contagens de preferências são por máquina servidora, não por pessoa nem por aparelho único: cada
servidor manda no próprio pacote os retratos que contou. Tanto o celular quanto o desktop podem
mandar uma vez por dia a cada máquina que ficou ativa naquele aparelho, desde que ela tenha aceitado
o aviso. Se um celular ou PC alternar entre três máquinas no mesmo dia, pode aparecer nas três.

| `setting` | Valores | De onde |
|---|---|---|
| `theme` | `auto`, `light`, `dark`, `desktop` | PC, celular (`system` do celular sai `auto`) |
| `panels` | `attached`, `floating` | PC |
| `palette` | `classic`, `neutral` | PC, celular |
| `background` | `plain`, `texture`, `aurora`, `image`, `desktop` | PC, celular (o liso do celular sai `plain`; o fundo com luz do PC sai `aurora`) |
| `background_effect` | `none`, `dither`, `ascii`, `halftone`, `scanlines` | PC, celular |
| `wallpaper` | `window`, `glass` | PC |
| `surface_material` | `glass`, `opaque` | PC |
| `reading` | `auto`, `none`, `text`, `sheet` | PC, celular |
| `font` | `system`, `mono`, `custom` (outra fonte instalada; o nome não sai) | PC |
| `code_font` | `jetbrains_mono`, `system`, `custom` | PC |
| `text_size` | `small`, `normal`, `large`: menor, igual ou maior que o tamanho de fábrica do app | PC, celular |
| `compact_style` | `on`, `off`: a aparência é a do atalho "Estilo compacto" | PC |
| `tool_look` | `classic`, `chips`, `tree`, `terminal`, `claude_desktop` | PC, celular |
| `thinking_tools` | `none`, `search`, `all` | PC, celular |
| `thinking_collapsed`, `task_list`, `table_chart` | `on`, `off` | PC (`task_list` e `table_chart` também no celular) |
| `ask_highlight` | `accent`, `amber` | PC, celular |
| `delivery` | `queue`, `direct`, `jev`, `conversation` | PC, celular |
| `hands_free` | `on`, `off` | PC, celular |
| `navigation` | `tabs`, `bottom_tabs`, `conversations`, `sidebar` | PC |
| `sidebar_compact`, `tabs_full` | `on`, `off` | PC |
| `sidebar_height` | `full`, `content` | PC |
| `sidebar_group` | `none`, `project`, `machine` | PC |
| `list_group` | `none`, `server`, `project` | celular |
| `list_organize` | `sessions`, `conversations` | celular |
| `home_screen` | `sessions`, `new_conversation` | celular |
| `tablet_bar` | `left`, `bottom`, `top` | só tablet |
| `language` | `system`, `pt`, `en` | PC, celular |
| `tips` | `on`, `off` | PC |
| `tour` | `done`, `skipped`, `none` | PC |
| `conversation_beta`, `code_mode` | `on`, `off` (opções da tela Beta, guardadas no servidor) | servidor |
| `default_provider` | `claude`, `codex`, `pi`, `omp`, `kimi`, `antigravity` | servidor |
| `default_effort` | `unset`, `off`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra` (o padrão salvo do agente padrão) | servidor |
| `auto_report` | `on`, `off`, `unanswered` | servidor |
| `dictation_org` | `none`, `harness`, `external_api` | servidor |
| `dictation_provider` | `openai`, `groq`, `elevenlabs`, `whisper_cpp`, `none` (o primeiro serviço da lista; `groq` é a chave Groq antiga, sem lista) | servidor |
| `connect`, `computer_use` | `on`, `off` | servidor |
| `whatsapp` | `installed`, `no` | servidor |
| `jev` | `configured`, `no` (só se há chave; a chave não sai) | servidor |

O canal de atualização não entra aqui: ele já vai em `env` em todo pacote.

### Falhas e trilha

- Tela: só eventos recebidos com autenticação do dono e horário original dentro da coleta. O
  servidor marca a origem em disco; um campo no corpo do cliente não muda essa decisão. O dia é
  calculado pelo horário UTC completo da linha, não pelo nome do arquivo local.
- HTTP: ação do catálogo de rotas e código fixo do servidor, sem URL, parâmetros nem resposta.
- MCP: só chamadas de uma sessão identificada pela chave, comprovadamente do dono e sem convidado,
  convite ou par de fora. Nome de ferramenta fora do catálogo vira `other`; argumentos e respostas
  não são lidos para a coleta.

O pacote não inclui texto de conversa, erro livre, saída de comando, pilha, caminhos, nomes de sessão,
conta ou máquina, credenciais, e-mails, endereços nem conteúdo de convidado. Turnos, avisos de sessão,
quedas sem autoria provada e ocorrências antigas sem horário comprovado ficam fora. O diário local,
os logs e o envio manual de relatório continuam separados. O receptor preserva os lotes históricos de
versões anteriores, que podiam trazer `logs`.

## Quando sai

- A cada 15 min e no desligamento, o servidor grava os contadores e a projeção segura. Uma vez por
  hora envia o retrato parcial do dia UTC. Um dia com só falhas ou passos também gera pacote.
- Ao fechar o dia UTC, congela a projeção final em disco, antes do envio e da poda habitual do diário.
  O envio final espera até 1 h de forma aleatória. Os parciais e o final usam o mesmo `batch_id`;
  substituem o que chegou antes, sem somar, e um parcial atrasado não substitui o final.
- Sem rede ou com erro do receptor, o final tenta de novo após 1, 3 e 7 min e segue na fila se não
  for aceito. `400` dá baixa no lote rejeitado; `413` fica na fila com falha registrada. O corpo
  JSON é limitado a 2 MiB, cortando detalhes antes dos contadores.
- A fila guarda no máximo 14 arquivos de dias; passando disso, sai o mais antigo.

## Ver o que está guardado

- `~/.hangar/telemetry-v3/queue/<dia>.json`: contadores, projeção e ID de cada dia do escopo 3, e as
  marcas privadas dos retratos do dia (máquinas e preferências do servidor e de cada app), gravadas
  junto com as contagens deles. As marcas não saem no pacote.
- `~/.hangar/telemetry.json`: escolha de B em `option_b`, período de coleta e dois números aleatórios.
  O booleano legado `consent` permanece `false` ao aceitar B: uma reversão não autoriza logs antigos.
  Um "sim" sem escopo ou do escopo 2 sai do arquivo na subida, e a máquina fica sem resposta.
- `~/.hangar/telemetry-v3/once.json`: marcas do que só conta uma vez na coleta atual, como cada
  atualização. O emissor de B nunca importa a fila nem as marcas de versões anteriores.

No Windows, `~` é a pasta do seu usuário.

## Responder e mudar

- Na abertura do desktop nativo e do Expo, máquinas que conhecem o mesmo aviso respondem juntas.
  Fechar sem escolher não grava nada. O ajuste fica em "Contagens anônimas de uso" nas Configurações;
  o "Não enviar" do aviso aberto ali também grava a recusa da máquina ainda sem resposta.
- `GET /api/telemetry` anuncia `consent_scope: 3` e, ligada, o `snapshot_period` que os apps
  devolvem com o retrato. Para ligar: `PUT {"consent":true,"consent_scope":3}`, que vale para a
  opção B e para o retrato de preferências. Um sim antigo (sem escopo ou do escopo 2) não liga nada;
  o não antigo vale. Com servidor que anuncia só o escopo 2 ou nenhum escopo, o app mostra o aviso
  daquela versão (veja "Servidores legados"). Não há segunda tentativa sem escopo depois de uma
  recusa. A PWA não oferece o ajuste.
- `PUT {"consent":false}` revoga, mesmo de um cliente antigo, e apaga a fila de B e as das versões
  anteriores (`~/.hangar/telemetry/` e `~/.hangar/telemetry-scope-2/`). O que o receptor já recebeu
  não é apagado pela revogação.
- Se você recusar, o servidor manda uma única vez um aviso anônimo de que recusou (só o tipo da
  recusa, a versão, o sistema e o canal), e nada mais: `{"kind", "version", "os", "channel"}` em `POST /v1/declined`, com `kind`
  `declined` (a primeira resposta foi não) ou `revoked` (tinha aceitado e desligou). Sem
  `machine_id`, `owner_id`, lote nem dia; o receptor só soma 1 no dia em que recebe. A marca
  `refusal_reported` no `telemetry.json` faz o aviso sair uma vez por máquina, mesmo mudando de ideia
  depois. Não sai com `HANGAR_TELEMETRY=0`, `DO_NOT_TRACK=1`, CI ou `.env` ilegível, nem pedido de
  convidado. São até quatro tentativas em segundo plano; se nenhuma chegar, o código vai ao diário de
  uso e o aviso não é tentado de novo.

Sem resposta, nenhum contador é contado nem gravado: só existe o `telemetry.json`, com `consent: null` e
os dois números aleatórios.

## Desligar pela máquina

Estas opções vencem a resposta dada no app e apagam a fila:

- `HANGAR_TELEMETRY=0` ou `DO_NOT_TRACK=1` no `.env` do Hangar: `~/.hangar/config/.env`, ou
  `backend/.env` em máquina ainda não migrada. O nome da variável vale em maiúsculas ou minúsculas.
- As mesmas variáveis no ambiente do processo do servidor. Exportar no seu shell **não alcança** o
  serviço do systemd nem a tarefa do Windows, que sobem o servidor sozinhos: use o `.env`.
- Em CI desliga sozinho: `CI`, `GITHUB_ACTIONS`, `GITLAB_CI` ou `JENKINS_URL` definidas (valor
  diferente de vazio, `0` ou `false`).

O servidor relê o `.env` antes de cada envio, na gravação periódica (15 min) e ao reiniciar. A linha de
Configurações informa se o ambiente desligou a coleta. Se o `.env` estiver ilegível, a coleta para e o
período é invalidado: os arquivos antigos podem ficar no disco, mas não voltam a ser enviados.

## Dono de várias máquinas

Para saber quantas máquinas cada pessoa usa, cada máquina tem um `owner_id` aleatório com a data em
que nasceu. Uma vez por dia ela pergunta às máquinas ligadas do seu `peers.json` qual é o dono delas
e adota o mais antigo. Máquina desligada ou com Hangar antigo não muda nada. Se você cadastrar a
máquina de outra pessoa no `peers.json`, as duas passam a contar como um dono só.

## Para onde vai

`https://telemetry.hangar.dev.br/v1/batch` (o aviso de recusa, `/v1/declined` do mesmo endereço), atrás do proxy da Cloudflare. O receptor é código aberto,
em [`services/telemetry-ingest`](../services/telemetry-ingest): grava o pacote num Postgres, não lê
IP, limita o corpo a 2 MiB e guarda evento ou valor que ainda não conhece em vez de
recusar. Os dados ficam 400 dias. Só o mantenedor do Hangar e quem ele convidar veem o resumo, na
página `/panel` do mesmo receptor, com conta e senha.

## Servidores legados

Servidores que não anunciam `consent_scope` usam o aviso antigo: mandavam um pacote por dia com
contagens e linhas de aviso e erro do servidor. A limpeza antiga tirava caminhos, nomes de sessão,
contas, máquinas, e-mails, endereços, trechos entre aspas e identificadores; mesmo assim, parte do
texto do erro podia passar. Servidores que anunciam só `consent_scope: 2` usam o aviso de preferências
daquela versão: o mesmo pacote diário com as linhas limpas do log, mais o retrato de preferências do
servidor e dos apps. Nenhum dos dois é o comportamento da opção B descrito acima.
