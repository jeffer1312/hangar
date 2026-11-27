# Telemetria anônima do Hangar

O Hangar pode mandar, uma vez por dia, contagens de uso e de erros do próprio Hangar ao
desenvolvedor. Serve para corrigir erro antes de alguém reclamar e saber o que melhorar. Nada é
contado antes de você responder ao aviso. As contagens não levam conversa, texto digitado, nome nem
endereço. As linhas de aviso e erro do log do servidor também vão, limpas por regras (veja "Linhas de
aviso e erro do servidor"): a limpeza tira caminho, nomes, e-mail, endereço, trecho entre aspas e ids,
mas um pedaço do texto do erro ainda pode passar. O Hangar não lê conversa nem texto digitado para
isso e não os escreve no log.

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
- `logs`: as linhas de aviso e erro do log do servidor naquele dia, já limpas (abaixo).

Toda dimensão é um valor de uma lista fixa no código, um sim/não ou um número de versão. Valor fora
da lista sai como `other`. O único texto livre é a mensagem das linhas de `logs`, e só depois da
limpeza descrita em "Linhas de aviso e erro do servidor".

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
| `log_dropped` | — | Linha de aviso ou erro que não entrou no pacote: o dia já tinha 100 linhas diferentes, o servidor estava gerando linhas demais no momento, ou os nomes a tirar não puderam ser lidos |

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

Depois do aceite do aviso que inclui preferências, o app do PC e o do celular mandam ao servidor
ativo (nunca a um convite) o retrato das preferências daquele aparelho, e o servidor conta as dele.
O servidor confirma o retrato em `POST /api/telemetry`, fora do diário. Só aceita o período de coleta
ativo e uma captura do dia UTC da chegada, com até um minuto de diferença para o futuro. Envio
atrasado não ocupa a vaga do dia nem reabre pacote fechado. Cada par vira um contador
`settings` com `client` (`native`, `ios`, `android` ou `server`), `setting` e `value`. O
servidor conta no máximo um retrato por dia de cada app e aparelho: o app manda junto o id aleatório
que já guarda para as novidades da atualização, e esse id fica no `once.json` da máquina, fora do
pacote. Preferência fora da tabela é descartada; valor fora da lista sai `other`. Nome de fonte, cor
escolhida, modelo e qualquer número não saem.

As contagens de preferências são por máquina servidora, não por pessoa nem por aparelho único: cada
servidor manda no próprio pacote os retratos que contou. Tanto o celular quanto o desktop podem
mandar uma vez por dia a cada máquina que ficou ativa naquele aparelho, desde que ela tenha aceitado
esse aviso. Se um celular ou PC alternar entre três máquinas no mesmo dia, pode aparecer nas três.

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

### Linhas de aviso e erro do servidor

Cada linha de nível aviso (`warn`) ou erro (`error`) que o `hangar-server` escreve no log dele
vai também ao pacote do dia como `{level, target, message, count}`: `target` é o módulo do código que
escreveu (como `hangar_server::list`), `message` é o texto da linha, e `count` quantas vezes a mesma
linha, já limpa, apareceu no dia. As linhas do nível informativo, do celular e do desktop não vão.
Dos campos da linha só entram `code`, `codigo`, `status`, `kind`, `cause`, `step`, `provider`,
`surface`, `error`, `ok`, `result`, `attempt`, `ms`, `http_status` e `method`, e só
com valor de código (minúsculas, dígitos e `_ . : -`); os outros ficam fora.

Antes de contar, o servidor limpa o texto na própria máquina:

- todo trecho entre aspas duplas, simples ou crases (também com aspas escapadas, `\"`) vira `"…"`:
  é dado, não a frase do código;
- tira credenciais (tokens, `Bearer`, senhas em URL e em JSON, chaves conhecidas), IPs, nomes do
  Tailscale (`*.ts.net`) e trechos longos em hex ou base64, com as mesmas regras do "Reportar
  problema";
- todo caminho de arquivo (absoluto, com `~/` ou `%LOCALAPPDATA%`, de unidade do Windows ou de rede,
  também com espaço no meio, como `C:\Program Files\…`, até o último trecho com barra antes de `; , )`
  ou aspas) vira `<caminho>`, sem guardar nem o nome do arquivo;
- endereço `http(s)` vira `<url>`, e-mail vira `<email>`, `host:porta` e nome com ponto (`a.rs`,
  `casa.exemplo.com`) viram `<nome>`; número de versão (`0.42.0`) fica;
- nomes da pessoa viram marcador: sessão (toda sessão vista hoje e ontem, aberta ou não, e as dos
  registros de abertura) vira `<sessão>`, sem diferença de maiúscula, também com acento; conta do Claude e do Codex, rótulo de conta e usuário do
  sistema viram `<conta>`; esta máquina, o nome dela e as do `peers.json` viram `<máquina>`. Esses
  nomes são relidos a cada 30 s; sem uma leitura boa deles a linha não sai;
- id (UUID, hex de 16 ou mais) vira `<id>` e número com 6 dígitos ou mais (pid, porta alta) vira `<n>`;
- caractere invisível (controle, formatação como U+200B e U+202E, separador de linha) vira espaço;
- a mensagem é cortada em 300 caracteres.

A limpeza é por regras, não por entender o texto: o que não casa com nenhuma delas (uma palavra
solta que a pessoa escreveu e o código repetiu no erro, por exemplo) passa. Linha que esperava para
ser contada quando a pessoa respondeu "Pode enviar" é de antes da resposta e não vai.

Por pacote vão no máximo 100 linhas diferentes; passando disso, a linha nova só soma `log_dropped`,
que conta também a linha descartada por excesso no momento ou por falta dos nomes.
Sem resposta ao aviso, ou com "Não enviar", nenhuma linha é guardada.

## O que nunca sai

Texto de mensagem e de comando, nome de sessão, pasta, repositório, branch, conta, e-mail, nome da
máquina, endereço e IP, user-agent, texto de erro do app, nome de skill, nome de motor, tokens e
custo, modelo do aparelho, horário dentro do dia, multiplexador. O texto de erro do servidor sai só
nas linhas de `logs`, depois da limpeza acima, que é por regras e pode deixar passar um pedaço dele.

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

- `~/.hangar/telemetry-scope-2/queue/<dia>.json`: um arquivo por dia ainda não enviado, com os contadores
  exatamente como vão sair (o dia de hoje está lá desde a última gravação).
- `~/.hangar/telemetry.json`: sua resposta e os dois números aleatórios. O aceite do aviso de
  preferências fica em `consent_scope: 2`; a recusa continua em `consent: false`.
- `~/.hangar/telemetry-scope-2/once.json`: marcas do que só conta uma vez (o retrato do dia das máquinas
  e das preferências, cada atualização). As marcas com dia somem depois de ontem.

No Windows, `~` é a pasta do seu usuário.

## Responder e mudar

- Ao abrir o app (desktop nativo ou celular), o aviso "Ajudar a melhorar o Hangar" pergunta pelas
  máquinas que ainda não responderam e identificam o aviso de preferências. Sem essa identificação,
  só é possível revogar o aceite existente. Fechar sem escolher não grava nada.
- O "Pode enviar" antigo não inclui preferências. Ao atualizar, a máquina deixa de enviar e pede
  o novo aceite, sem importar a fila anterior. O "Não enviar" continua valendo.
- Para mudar depois: linha "Contagens anônimas de uso" nas Configurações da máquina.
- "Não enviar" apaga a fila na hora, sem apagar o que o receptor já recebeu.

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
IP, recusa só pacote malformado (até 256 KB) e guarda evento ou valor que ainda não conhece em vez de
recusar. Os dados ficam 400 dias. Só o mantenedor do Hangar e quem ele convidar veem o resumo, na
página `/panel` do mesmo receptor, com conta e senha.
