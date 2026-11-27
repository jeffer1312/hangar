# Telemetria anônima do Hangar

O Hangar pode mandar, uma vez por dia, contagens de uso e de erros do próprio Hangar ao
desenvolvedor. Serve para corrigir erro antes de alguém reclamar e saber o que melhorar. Nada é
contado antes de você responder ao aviso, e nunca sai conversa, texto digitado, nome ou endereço.

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
- `counters`: a lista de contadores `(evento, dimensões, faixa) → quantidade`.

Toda dimensão é um valor de uma lista fixa no código, um sim/não ou um número de versão. Valor fora
da lista sai como `other`. Não existe campo de texto livre.

Tempo vai em faixas: `lt100ms`, `lt300ms`, `lt1s`, `lt3s`, `lt10s`, `lt30s`, `ge30s`. Nos
eventos marcados com "(faixa)", cada ocorrência soma 1 na faixa da duração.

### Uso

| Evento | Dimensões | O que conta |
|---|---|---|
| `app_visible_minutes` | `client` (`ios`, `android`, `pwa`, `native`, `other`) | Minutos com o app na tela. O app avisa a cada 5 min, só ao servidor ativo; no máximo 5 min por aviso |
| `message_sent` | `provider`, `surface` (`terminal`, `headless`), `steer` (sim/não), `origin` (`person`, ou `session` quando a mensagem é recado de outra sessão) | Mensagem que você mandou pelo app ou pela API |
| `turn_finished` (faixa) | `provider` | Turno do agente, do começo do trabalho até parar |
| `session_opened` (faixa) | `provider`, `surface`, `model` (lista curta de modelos; motor do `engines.json` sai `engine`, nunca o nome), `effort`, `permission`, e sim/não para `restore`, `jev`, `prewarm`, `worktree` | Sessão que você abriu, com o tempo que levou. Sessão aberta por convidado não conta |
| `feature_used` | `feature` (código da lista `FEATURE_ROUTES`, como `session.rename` ou `search`) | Ação sua que deu certo numa rota da lista. Leituras que os apps repetem sozinhos (lista, conversa, status) não contam |
| `mcp_tool` | `tool` (nome da tool do MCP `hangar`), `error` (sim/não) | Chamada de tool do MCP `hangar` por um agente. Sessão de convidado com login não conta |
| `voice_call` (faixa) | — | Duração da chamada de voz |
| `dictation` (faixa) | `route` (`ditado`, `relimpar`, `sessao`), `status` (`ok`, `warning`, `error`, `cancelled`) | Ditado, com o tempo total |

`provider` é `claude`, `codex`, `pi`, `omp`, `kimi`, `antigravity` ou `other`.

### Erros

| Evento | Dimensões | O que conta |
|---|---|---|
| `problem_shown` | `code` (lista de avisos de tela) | Aviso que apareceu na lista de sessões; conta quando ele aparece, não a cada atualização da lista |
| `problem_hidden` | `code` (lista de avisos só de diagnóstico) | Aviso que o Hangar tirou da tela por ser só diagnóstico; no máximo um por minuto por sessão e código |
| `session_open_failed` | `provider`, `code` | Sessão que você pediu e não abriu, também a recusada antes de começar (nome em uso, pasta fora do permitido, worktree) |
| `send_failed` | `code` | Mensagem recusada |
| `session_fallen` | `cause` (`terminal_out_of_memory`, `terminal_died`) | Sessão com terminal que caiu |
| `server_exit` | `kind` (`crash`, `update`, `requested`), `cause` | Como o servidor anterior parou; contado na subida seguinte |
| `limit_switch` | `code` | Troca de conta pelo limite de uso: trocou, ficou ou falhou |
| `client_fatal` | `client`, `code` (tipo do erro, como `typeerror`, ou `panic` no desktop) | Queda do app. O texto do erro nunca sai |

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

## O que nunca sai

Texto de mensagem e de comando, nome de sessão, pasta, repositório, branch, conta, e-mail, nome da
máquina, endereço e IP, user-agent, detalhe e texto de erro, nome de skill, nome de motor, tokens e
custo, modelo do aparelho, horário dentro do dia, multiplexador.

O que convidado faz (porta de convidado, convite, par, login de convidado) não conta: mensagem,
sessão aberta ou que não abriu, tela, ação na API, evento do app dele e tool do MCP na sessão dele ficam fora.
Fim de turno, erros, quedas e troca por limite contam para todas as sessões da máquina, sem dizer de quem é a sessão.

## Quando sai

- Os contadores do dia ficam na memória e são gravados na fila a cada 15 min e quando o servidor
  desliga, para não perder o dia num reinício.
- Quando o dia (UTC) fecha, o pacote dele sai numa espera aleatória de até 1 h, para as máquinas não
  enviarem todas no mesmo instante.
- Sem rede ou com erro do receptor: tenta de novo depois de 1, 3 e 7 min; não deu, o dia fica na
  fila para a rodada seguinte. Pacote recusado como malformado (400 ou 413) é apagado.
- A fila guarda no máximo 14 dias; passando disso, sai o mais antigo.

## Ver o que está guardado

- `~/.hangar/telemetry/queue/<dia>.json`: um arquivo por dia ainda não enviado, com os contadores
  exatamente como vão sair (o dia de hoje está lá desde a última gravação).
- `~/.hangar/telemetry.json`: sua resposta (`consent`) e os dois números aleatórios.
- `~/.hangar/telemetry/once.json`: marcas do que só conta uma vez (o retrato do dia, cada
  atualização).

No Windows, `~` é a pasta do seu usuário.

## Responder e mudar

- Ao abrir o app (desktop nativo ou celular), o aviso "Ajudar a melhorar o Hangar" pergunta pelas
  suas máquinas que ainda não responderam. "Pode enviar" ou "Não enviar" grava em todas elas de
  uma vez. Fechar sem escolher não grava nada: o aviso volta na próxima abertura.
- Para mudar depois: linha "Contagens anônimas de uso" nas Configurações da máquina.
- "Não enviar" apaga a fila na hora.

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

A mudança no `.env` vale na próxima passada de 15 min ou ao reiniciar o servidor. Com ela, a linha
em Configurações diz que a telemetria foi desligada pelo ambiente da máquina. Se o `.env` não puder
ser lido, a contagem para, mas a fila fica.

## Dono de várias máquinas

Para saber quantas máquinas cada pessoa usa, cada máquina tem um `owner_id` aleatório com a data em
que nasceu. Uma vez por dia ela pergunta às máquinas ligadas do seu `peers.json` qual é o dono delas
e adota o mais antigo. Máquina desligada ou com Hangar antigo não muda nada. Se você cadastrar a
máquina de outra pessoa no `peers.json`, as duas passam a contar como um dono só.

## Para onde vai

`https://telemetry.hangar.dev.br/v1/batch`, atrás do proxy da Cloudflare. O receptor é código aberto,
em [`services/telemetry-ingest`](../services/telemetry-ingest): grava o pacote num Postgres, não lê
IP, recusa só pacote malformado (até 64 KB) e guarda evento ou valor que ainda não conhece em vez de
recusar. Os dados ficam 400 dias. Só o mantenedor do Hangar vê o resumo, na página `/panel` do
mesmo receptor, protegida por senha.
