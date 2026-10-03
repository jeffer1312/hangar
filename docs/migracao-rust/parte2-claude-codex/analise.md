# Parte 2 — Claude e Codex, com e sem terminal

**Data:** 02/10/2026, America/Sao_Paulo. **Base:** `hangar-server-parte2`, `f7f797bb`.
Fetch confirmou `origin/hangar-server-parte1=aa8eec36`, com `2eef31e8` e `aa8eec36` (CI Zig e
teste de processo encerrado). Diff dos módulos desta análise contra o remoto é vazio.
Não houve merge, troca de branch, alteração de produto, teste automatizado ou reinício nosso.

Este documento substitui o alcance do planejamento anterior, que fica preservado como referência.
Só Claude e Codex entram, nos quatro casos de terminal/sem terminal. Pi, Kimi, omp, orq, lista,
quadro e canvas ficam fora. Histórico e transcript ao vivo desses dois provedores já estão em
Rust desde a parte 1: não apresentar isso como entrega nova.

## Recomendação e divisão

Não portar apenas `/input` ou uma escrita de fila. Cano, transporte, pendências, drenagem e
recuperação precisam de um responsável único. Hoje as travas são Python e não coordenam outro
processo; o cano aceita um cliente e expulsa o anterior. Um Rust observador conectado ao mesmo
cano faria os adapters Python religarem e tomarem a conexão de volta.

Recomendo quatro subpartes, nesta ordem, **para aprovação**:

| Subparte | Entrega | Fronteira conservada |
|---|---|---|
| 2A — custo de deltas | Acumular pedaços e publicar prévia/pensamento/input de tool em cadência, sem reparse por pedaço; correção Python pequena, comum às prévias Codex dos dois modos | Python continua único escritor; nenhum protocolo novo |
| 2B — sessões sem terminal | Rust assume runtime Claude/Codex, transporte do cano, reducers, permissões/perguntas, fila, confirmação e retomada como unidade | Python resolve criação, conta/argv/config e políticas da API; adaptadores voláteis têm fronteira explícita |
| 2C — observação do terminal | Rust mantém `tmux -C` e o mesmo capture-pane; depois porta reducer de estado/prévia/statusline com fontes estruturadas. Codex WebSocket reutiliza reducer da 2B | Entrada Claude/plugin/tmux continua Python até a 2D; psmux sem prova usa caminho existente |
| 2D — entrada Claude com terminal | Portar esteira inteira de entrega, socket nativo/plugin/reserva tmux, guards, fila/confirm e opções | Configuração, credenciais e administração geral continuam Python, fora desta parte |

O plano executável desta entrega é **2A**, com código proposto completo e testes primeiro.
2B–2D têm contratos, riscos, ordem e critérios neste desenho; precisam de seus próprios planos
executáveis antes de implementação. Essa divisão é proposta, não uma decisão já aprovada.
Não há justificativa para escrever agora milhares de linhas de integração de CLI ainda sem
contrato de transferência de controle aceito. 2A não é chamada de porte Rust: é preparação barata.

## Linha de base: medido, histórico e não medido

Consultas ao serviço vivo foram GETs autenticados; nenhum prompt, tecla, approval ou resume foi
enviado. Script lê credencial em memória, nunca a imprime. Nomes e conteúdo das conversas não
foram copiados para documento/fixture. Medidas por processo são agregadas: não atribuir CPU do
backend inteiro a uma sessão. Não havia Claude/Codex com terminal no serviço observado.

| Medida real | Método / população | Resultado |
|---|---|---|
| Sessões vivas | GET lista apenas para inventário da amostra | um Claude e um Codex, ambos sem terminal |
| Claude `/history?limit=200` | cinco GETs, mediana; transcript 21.592.245 bytes | 17,848 ms; resposta 170.639 bytes / 200 eventos |
| Codex `/history?limit=200` | cinco GETs, mediana; transcript 4.314.285 bytes | 33,423 ms; resposta 957.409 bytes / 176 eventos |
| Cano Claude Python | `/proc/798007/status`; argv lido só para identificar programa após `--` | RSS 10.992 kB; seis threads |
| Cano Codex Python | `/proc/1206973/status`; mesmo método | RSS 21.780 kB; seis threads |
| Backend Python | `/proc/1239094/status` | RSS 201.620 kB; 22 threads |
| hangar-server vivo | `/proc/1239163/status` | RSS 20.400 kB; 25 threads |
| CPU agregada do processo | ticks user+system após as leituras de histórico, espera nominal700ms mais tempo da varredura; resolução10ms | Python190ms de CPU; Rust990ms de CPU; canos abaixo de10ms |

O Rust pode usar vários núcleos, por isso CPU acumulada pode exceder a espera de700ms de parede.
A amostra mistura trabalho real e GETs do levantamento; não demonstra que uma sessão custa esses
números, nem serve de comparação CPU Python×Rust. Os canos vivos ainda são Python; a presença
do servidor Rust não significa que todas as sessões atuais já usam hangar-cano.

### Transporte de captura — medição nova Linux

Multiplexador **isolado**, socket aleatório próprio, configuração `/dev/null`, pane sintético
200×50, 4.490 bytes. tmux 3.7b. Mesmo comando atual `capture-pane -p -t %0 -S -200`; cinco
aquecimentos e 100 capturas por método. Nenhuma sessão real foi anexada/dirigida/redimensionada.
As 200 respostas temporizadas foram comparadas byte a byte. Servidor e temporários removidos.

| Medida | Subprocesso por captura | Cliente de controle persistente |
|---|---:|---:|
| Mediana por captura | 1,798850 ms | 0,151227 ms |
| CPU Python do lote | 23,668 ms | 9,288 ms |
| CPU de filhos do lote | 140,459 ms | 1,190 ms |
| CPU servidor tmux | 20 ms | abaixo da resolução de 10 ms |

Parede: `perf_counter_ns`, comando até leitura completa. CPU pai/filhos: `resource.getrusage`,
soma user+system; lote controle inclui attach/detach e aquecimento. CPU servidor: `/proc/pid/stat`
do servidor criado. Razão de medianas = 1,7988495 / 0,151227 = **11,895× menos tempo de transporte**.
Não é ganho do backend inteiro; loopback HTTP, classifier, sidecars, SSE e CLIs ficaram fora.
O ensaio anterior sem `-S -200` foi descartado como comparação do caminho atual.

Cliente usado: `tmux -L <socket criado> -C attach-session -E -f read-only,ignore-size,no-output
-t =sample`; dimensões permaneceram 200×50. Fechou somente o servidor nomeado criado, retorno0.
Esse ensaio é medição avulsa permitida pelo início, não teste automatizado do produto.

### Deltas sem terminal — causa e medição

`claude_headless/adapter.py:1423–1431` faz `previa +=`, `pensamento +=`, `tool_json +=`, decode
e extração `_input_parcial` a cada pedaço; Codex `_consumir` faz `buf +=` seguido de push
(`codex/adapter.py:1614`). `PushPreviewSource.text` retém strings publicadas (`preview_push.py:42`),
portanto não pressupor a otimização de concatenação em lugar do CPython: há outra referência.

Medição sintética final: Python3.14.6, mediana de três, script extrai por AST a função atual e
suas duas constantes e carrega somente json/re da stdlib. Campo de texto com200KiB significa
204.800 caracteres ASCII, mais envelope JSON compacto; não é conversa real.

| Input e tamanho do campo | Pedaço | Parser parcial a cada pedaço |
|---|---:|---:|
| Write file_path+content64KiB | 128B | 10,794ms |
| Write file_path+content128KiB | 128B | 38,953ms |
| Write file_path+content200KiB | 128B | 91,206ms |
| Campo-alvo prompt200KiB | 128B | 5.415,474ms |
| Campo-alvo prompt200KiB | 32B | 21.532,751ms |

Decode apenas final é referência inferior, não correção equivalente de UI: prompt200KiB foi
0,141/0,202ms nos dois tamanhos de pedaço. Concatenar atributo de texto200KiB/128B custou3,039ms
contra StringIO+leitura final0,041ms, sem alias da fonte SSE. As medidas excluem push, SSE,
cano/CLI, IO e recuperação. O0,73s do início é histórico de outra população, não reproduzido
como tal. Campos-alvo do regex custam mais que content; não generalizar o resultado para toda tool.

Uma primeira tentativa com uv criou um `.venv` ignorado nesta worktree. Ele foi removido após
confirmar que não era symlink nem estava em uso; ambiente principal preservado. Os números finais
acima vieram do interpretador/stdlib, sem carregar app, lifespan ou CLI e sem instalar serviço.

Correção menor: armazenar pedaços sem juntar/parsear a cada delta, publicar primeiro valor,
valores intermediários em150 ms e último antes de fechar bloco; limpar imediatamente quando
transcript/turno torna preview obsoleta. Input parcial permanece visível; só decodificar no fim
apagaria uma função existente. Não escrever parser JSON incremental próprio nem adicionar biblioteca.

Cadência reduz reconstruções/decodes redundantes; o formato público full-replace ainda custa
o tamanho do texto em cada publicação. Não prometer complexidade linear de toda a rede para
qualquer duração de stream. Limite é explicado, medido e testado por contagem de publicações,
conteúdo exato e ausência de callback antigo após fechamento/troca.

## Caminho sem terminal: dono do processo e do controle

### Claude

`adapters/claude_headless/adapter.py:172` reúne estado, pending, question, usage, texto em voo,
futures, spawn/drain/ocioso. `sessions.py` guarda identidade/config/sidecar. `cano.py` ou
hangar-cano é dono do processo CLI, independente do backend. `stream-json` é a saída; request/
response de controle carrega ids e pending; pensamento pede `--thinking-display summarized`.

- `send_prompt:351` marca turno antes de esperar notificação; `delivery_lock:321`, `drain:382`
  e fim de turno coordenam uma entrada por vez. Não drenar lote de prompts porque o estado antigo
  ainda está idle. Prévia, pensamento e ferramenta têm fontes separadas (`preview_push.py`).
- `_aplicar_snapshot:988` recupera init, turno, permissões/perguntas, último result/rate; não
  pressupor que ele contém texto/thinking/tool parcial: snapshotv1 não guarda esses buffers.
- `_ler:1034` distingue erro de transporte de erro de mensagem. EOF com cano vivo agenda
  religação (`:1072`), não cria outro Claude. Parada65min, base de permissão/plano, reload ocioso,
  teto de subidas e hooks continuam parte do comportamento, em qualquer linguagem.
- `_on_control_request:1455` e respostas mantêm request corrente, permissão humana/auto-policy,
  AskUserQuestion e cancelamento. Resposta de request antigo não fecha pergunta nova.

### Codex

`adapters/codex/sem_terminal.py:115` sobe app-server stdio como filho do cano; `:127` conecta e
reinjeta pending do snapshot. Initialize repetido já inicializado é sucesso; thread vazia sem
rollout não pode ser retomada como thread com conversa. CODEX_HOME é da conta escolhida.

`appserver.py:161` lê JSON-RPC, separa resposta de pedido bidirecional mesmo com ids iguais;
`respond:270` responde uma vez; `request:290` correlaciona future e timeout; erro de leitura encerra
loop. `adapter.py:577` associa client/thread, `_consumir:1545` transforma notificações e drena
quando turno termina, mesmo sem chat aberto. Prévia/state/usage/approval/question/settings vivem
nesse runtime; não substituí-los por mtime ou capture do pane.

O mesmo reducer serve Codex terminal e sem terminal. Nativo com TUI `codex --remote` usa
WebSocket no app-server do pane; sem terminal usa stdio no cano. Não subir outro app-server
para observar a thread. Aprovações TUI continuam TUI; cartões headless preservam tipos/ids.

## Com terminal: precedência antes da captura

`state.py:782` usa poll0,75s, quadros compartilhados com TTL/voo (`:698–758`), e classify
(`:388`) com guards/debounce temporal. Claude usa registro nativo quando válido, plugin/hook,
depois pane; spinner igual não prova atividade. Pergunta/overlay real exige decisão imediata.
`preview.py:605–679` prefere sidecar, distingue `None` de `""`, usa0,15s trabalhando e0,75s parado,
e descarta leitura iniciada antes de /clear. `statusline.py` preserva sidecar inteiro antes de pane.
Codex usa snapshot nativo saudável/assinado da **thread atual**, com pane para seletor pré-thread
e fallback, não fonte primária de estado/prévia.

`tmux.py:1202` resolve pane autoritativo e executa captura; isso não é ler chat pelo terminal:
conversa continua transcript. Uma conexão -C por sessão observada atende todos os aparelhos.
Parser enquadra `%begin`/`%end`/`%error` e notificações sem confundir uma linha do conteúdo
iniciada por `%` com protocolo. Timeout/EOF/erro de comando não viram captura vazia nem sessão morta.
[Protocolo oficial tmux](https://github.com/tmux/tmux/wiki/Control-Mode).

O observador não pode parecer pessoa presente para `plugin_bridge.terminal_preso:414`, nem
alterar tamanho/ambiente/pane ativo. Flags ignore-size/no-output e attach-E foram medidos;
excluir explicitamente `client_control_mode` dos guards de pessoa no terminal. Falha permanece
conservadora: se não sabe, não segura permissão escondendo-a de uma pessoa.
[Flags oficiais](https://man.openbsd.org/tmux.1).

### Emulador e Windows

`desktop-native/Cargo.toml:30` já usa alacritty_terminal0.26.0; `src/term_view.rs:88–154` mantém
Term/Processor e responde PtyWrite. O servidor observador **não envia** essas respostas ao CLI.
Reconstrução inicial, alternate screen, scrollback, wraps/resize e retorno após queda ainda não
foram provados. Capture via-C resolve o fork sem exigir emulador. Só considerar emulação depois
de medir o custo residual e provar checkpoint fiel; não adicionar agora outra representação da tela.

O upstream psmux documenta-C/-CC; isso não prova a versão instalada. A consulta SSH somente
leitura a delphi-02 falhou em verificação da chave; não foi contornada e Windows não foi medido.
Preservar subprocesso Python até capability e captura equivalente serem provados na instalação.
Alvo psmux continua `=<sessão>:<janela>.<pane>`, sem assumir que `%1` é global.
[Compatibilidade psmux](https://github.com/psmux/psmux/blob/master/docs/compatibility.md),
[argumentos psmux](https://github.com/psmux/psmux/blob/master/docs/tmux_args_reference.md).

## Envio, fila e prova de entrega

`api.py:3687–4070` ramifica quatro casos, mas grupos/recados/hooks/loop/reconexão/fim de turno
também chegam à esteira. Migrar só a rota POST deixa outros escritores Python ativos.

- Claude terminal: socket nativo só para recados identificados; fala do dono vai como prompt.
  Plugin principal, tmux na ausência; `INCERTO` não autoriza fallback que digita novamente.
  Composer é esvaziado e paste/Enter/limpeza são comprovados; partial não vira “enviado”.
- Claude headless: uma delivery_lock, cano vivo e initialized, user stream-json; acordar ocorre
  em background quando subindo/parado. Interromper/selecionar/responder valida pending corrente.
- **Codex nos dois modos usa `turn/start` JSON-RPC**, não digitação da TUI; `turn/interrupt` e
  expectedTurnId no steer, sem sobrescrever settings recém-escolhidos na TUI.
- `pqueue.py:547–737`: append/claim/confirm/reconcile sob trava intraprocesso, arquivo atômico
  não coordena dois processos. IDs, delivered, confirmed e desistiu têm funções distintas.
  Transcrição é prova; RPC aceito não prova que rollout já gravou, nem ausência autoriza reenviar.
- `api._agendar_confirmacao:1133` e `_confirm_and_drain:1209` distinguem mid-turn/ilegível/idle;
  timestamps e ocorrência por prompt preservam duas mensagens iguais. Fila nunca é descartada
  porque dono mudou de processo ou mesmo nome foi recriado.
- Timeout `turn/start` depois de aceite é resultado **incerto**; contrato atual não fornece
  idempotência fim a fim. Rust não repete POST pelo Python após despacho sem prova de não envio.
  Não declarar “exatamente uma vez” por existir UUID ou arquivo atômico.

Guards de pane interativo, teclas allowlist, multiline/clipboard/escape psmux e seleção por
adapter continuam valendo. Reduzir forks não elimina os tempos de acomodação do CLI.
Rotas de convidado continuam Python e respeitam share versus par só leitura; owner gate preservado.

## Transferência para Rust e reserva: contrato necessário na 2B

O cano expulsa cliente anterior (`cano.py:249`, `hangar-cano/main.rs:293`) e os adapters Python
religam (`claude_headless:1072`, `codex:1158`). Antes do Rust conectar é obrigatório interromper
reconectores/bombas/drains daquela vida, suspender operações e congelar escrita da fila no Python.
Rust só fica entregável após ligar ao cano existente, aplicar snapshot e reconciliar identidade.
Todas as entradas, inclusive convidado via Python, encaminham para o mesmo responsável.

Proposta de identidade interna: session_key durável, provider/transport, sid/thread_id atual,
runtime_generation e runtime_owner. A geração acompanha /clear/recriação/mudança de modo/thread;
não usar só nome nem TTL de lock como autorização. ACK de handoff exige antigo cliente fechado,
fila sem operação em voo e novo snapshot aplicado. Nenhuma segunda CLI é criada para fallback.

Rust morreu: primeiro suspender envio, confirmar processo/client antigo encerrado, reabrir o
cliente Python do mesmo cano, aplicar snapshot/pendências e só então drenar. Rust apenas sem
resposta ainda vivo não libera segundo escritor: devolver indisponibilidade/incerteza e resolver
a propriedade. Resposta perdida após write conserva entrada visível aguardando prova, sem retry
cego. Estado/prévia podem reconectar por leitura; operação com efeito não tem a mesma reserva.

A reserva HTTP da parte1 continua no mesmo processo/lifespan. Protocol interno atual1; 2A não
muda wire/protocol. Cada entrega 2B–2D que mudar rotas/eventos/ambiente sobe ambos os protocolos
no mesmo commit, a partir do valor então vigente. Se snapshot do cano precisar de texto/pending
novos, VERSAO/versão Rust do cano também sobem juntos; não assumir snapshotv1 suficiente para
prévia em voo. Isso exige desenho próprio da 2B antes de executar, não um writer paralelo agora.

## Verificação futura

2A: conteúdo exato, decode/publicação por janela, primeira/última atualização, limpeza imediata,
callback cancelado após result/clear/close/rebind, Ask/permissão/drain sem150ms de atraso; code
completo no plano. Medir trabalho total do cenário e atraso do event loop, distinguindo CPU de IO.

2B–2D: fake CLI/cano/app-server com requests/ids/pending; crash após claim e antes/depois write;
POST/drain/hook concorrentes; dois aparelhos e nenhum SSE; retorno de mesma sessão após restart;
mensagens iguais, anexo, same-name/thread/provider/mode change; plugin incerto e composer ocupado;
guest pair sem escrita; controle tmux sem tamanho/presença humana alterados; psmux provado na VM.

Checks automatizados só quando o dono pedir. Uso real requer dono e canal de testes, sem subir
outro backend ou dirigir sessão de trabalho para fabricar medição. Esta análise não conferiu
entrega por prompt real, Windows instalado nem o código novo de buffers/Rust.
