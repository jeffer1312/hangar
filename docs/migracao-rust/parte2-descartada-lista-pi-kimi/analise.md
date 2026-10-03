# hangar-server, parte 2 — análise das listas e conversas

**Data:** 02/10/2026. **Base:** `f7f797bbb04a951fdcfd6d8b89a6b7dc64409b1d`, branch
`hangar-server-parte2`. Depois de `git fetch origin`, `origin/hangar-server-parte1` tinha o mesmo
commit. Um novo fetch ao fechar o planejamento encontrou `aa8eec36` (mais `2eef31e8`): download
do Zig direto no CI e teste de processo encerrado em `backend/tests/test_rust_server.py`.
Nenhum arquivo de implementação desta análise mudou; o plano prevê incorporar esses consertos
na execução aprovada. Não foi feito merge nem troca de branch. A parte 1 ainda precisa de uso real e Windows;
o roteiro da parte 1 não é evidência de que essas verificações passaram.

## Recomendação

Portar os parsers, histórico e transmissão compartilhada do Pi/omp e Kimi. Na lista, portar a
distribuição HTTP/SSE para Rust e manter descoberta, decoração e visibilidade no produtor Python
existente. Na `orq`, compartilhar a transmissão no Rust mantendo `orq_timeline.entry` como parser
único Python; o histórico continua repassado. Essas duas fronteiras são decisões propostas,
**ainda não aprovadas**, e significam que esta parte não elimina todo o trabalho Python da lista
nem o custo de montar o histórico `orq`.

Não há medição que justifique prometer redução do custo central por tique da lista nesta máquina.
O serviço observado tem duas sessões sem terminal. A lista já é compartilhada; a descoberta
completa também executa manutenção, não apenas leitura de processos. Reescrevê-la nesta parte
anteciparia adaptadores, estado e pareamento previstos nas partes 4–6. A alternativa integral
deve ser planejada separadamente caso o dono exija remover essas dependências já nesta parte.

## Medições e limites

Medições avulsas, sem teste automatizado, instalador, partida ou reinício de backend. Python 3.14
do ambiente do checkout principal. Nenhuma conversa real foi salva em fixture ou documento.
Os scripts temporários são removidos ao finalizar. Horário: noite de 02/10/2026 (America/Sao_Paulo);
os valores descrevem essa amostra, não o futuro cenário de centenas de sessões.

| Medida | Método / população | Resultado |
|---|---|---|
| `GET /api/sessions` vivo | 1 aquecimento + 20 GETs sequenciais autenticados, leitura integral do corpo, relógio monotônico | mediana **1,043 ms**, mínimo 0,802 ms, máximo 1,312 ms; 2 sessões (Claude e Codex); 2.597 bytes |
| Processo Python vivo | `/proc/914179/status`; filho do `uv` PID 914175 do serviço ativo | RSS 201.756 kB; anônimos 179.492 kB; 24 threads |
| `/proc`, sem cache | 12 chamadas de `procinfo._proc_children_map(max_age=0)`; 399 processos | parede mediana 2,680 ms, máximo 3,751 ms; CPU Python mediana 2,676 ms |
| tmux em lote | 12 `tmux.list_panes_all()`; **zero panes** | parede mediana 1,438 ms, máximo 2,581 ms; CPU Python mediana 0,144 ms; não inclui CPU do tmux |
| Pi/omp, parser sintético | 20.000 linhas, 10.000 mensagens + marcadores de hook; 3.037.780 bytes; `json.loads` + `Stream.feed_events` + flush | parede mediana 91,398 ms; CPU 91,149 ms; máximo 93,171 ms |
| Kimi, parser sintético | 10.000 `tool.result`; 1.858.890 bytes; `json.loads` + `parse_obj` | parede mediana 57,643 ms; CPU 57,487 ms; máximo 59,110 ms |
| `orq`, parser sintético parcial | 10.000 linhas `advance`; 1.278.890 bytes; `json.loads` + `parse_obj`, **sem RunFiles/Jev/triagem de recados** | parede mediana 133,267 ms; CPU 132,966 ms; máximo 136,645 ms |

Cada benchmark de parser usou um aquecimento e cinco repetições, com dados inventados em memória.
As três populações são diferentes: os tempos não são comparação entre provedores. Não incluem
disco, histórico/fila, HTTP ou transmissão. Nenhum Rust da parte 2 foi implementado, compilado ou
medido. Não existem Pi, Kimi, omp ou `orq` vivos no serviço para medir seus `/history` reais.

O GET aquecido pode usar o retrato já decorado de até 2 s (`api.py:1780`); não mede o custo do
produtor por tique. Não foi medido esse custo com muitas sessões ou no Windows. A linha de base
antiga de 2,2 ms com três sessões é de outra amostra, não uma melhora atribuível à parte 2.

## Lista: fluxo e trabalho que continua necessário

1. `api.list_sessions` (`backend/app/api.py:1756`) consulta `recent_list(2.0)` ou a descoberta
   `_guardar_snap` (`:970`). Esta usa TTL de 1 s e single-flight; cada decoração recebe cópias.
2. `SessionRegistry.list` (`backend/app/registry.py:1199`) junta panes e árvores de processos,
   registros Claude/Codex sem terminal, tickets Pi/Kimi/omp, vínculos de grupo e linhas `orq`.
   A varredura de pares mortos (`:1199–1381`) pode arquivar contratos e avisar sessões: não
   executar `registry.list()` como se fosse microbenchmark puro.
3. `list_with_state` (`:1405`) consulta adapters, estado/approvals, perguntas pendentes,
   statusline, última resposta, git, plano, loop, compartilhamento e dono. Muitos campos têm
   origem dinâmica Python, não são simples desserialização.
4. `_ListRefresher` (`backend/app/sse.py:396`) já é produtor único. Poll de 1,5 s **depois** do
   trabalho; duração do ciclo não é prazo garantido. Falha mantém retrato e emite `list_error`;
   recuperação reemite `sessions` mesmo sem mudança da assinatura. Última conexão libera o
   produtor (`:527–545`).
5. `list_events` (`:565`) emite `sessions`, `list_error`, `shortcut_terminals`, `nav` e `ping`.
   Ping de aplicação a cada 8 s; comentário SSE do EventSourceResponse a cada 15 s. Sem ids de
   retomada. `_list_sig` (`:335`) ignora `last_activity` e reduz statusline para evitar emissão
   a cada escrita. `GET` usa o retrato recente completo, **não** o último JSON estável do SSE.
6. `stall_watch.py:108–114` também decora a lista. Não é eliminado pelo fanout Rust.

Rust ganha distribuição de um quadro serializado para vários clientes e mantém rede/cancelamento
fora do Python. Python continua pagando a descoberta/decoração uma vez por ciclo e os GETs que
precisem de dados frescos. Não há ganho percentual medido para essa fronteira. Portar só
`last_reply` tem benefício incerto: `_reply_cache` (`registry.py:1764`) já evita reparsing parado.

### Visibilidade, atalhos e navegação

- **Dono não significa lista sem filtro.** `guest_users.visible_to(None, name)`
  (`backend/app/guest_users.py:240`) esconde a sessão de convidado com `owner_sees=false`.
  `filter_visible` (`:249`) continua no Python também na ponte interna do dono.
- `guest_safe` e `Share.sees` mantêm recortes de convidados e removem referências a sessões
  alheias. Convidados não entram no hub Rust; continuam pelas mesmas rotas Python.
- Atalhos são uma lista independente (`sse.py:391`, `shortcut_terminals.list_all`), com leitura
  single-flight que não bloqueia sessões. Falha guarda anterior; lista vazia é dado válido.
- Navegação é marcador durável por sessão, com `{url, ts}`, TTL 600 s e confirmação por
  `DELETE /nav` (`sse.py:193–276`). Cada cliente recebe uma vez por timestamp, incluindo cliente
  que entra depois. Um cache de eventos `nav` antigos reabriria navegador já confirmado.
- A ponte proposta manda internamente `nav_snapshot` com **todos os marcadores ainda vivos**,
  inclusive mapa vazio após confirmação/expiração. O envelope leva `observed_at` monotônico da
  leitura Python. Nova anexação consulta `/internal/list-nav` fresco e ignora retratos anteriores
  a essa leitura, mesmo que cheguem atrasados. Retratos internos periódicos de1s fecham a corrida
  entre leitura e assinatura; Rust compara timestamp por cliente e produz o `nav` público
  existente `{name,url}`. Não altera o contrato do cliente nem repete nav público a cada tique.

## Conversas: formatos e invariantes

### Pi e omp

`backend/app/adapters/pi/transcript.py:65–138` transforma `message`, usando `message.role`.
`OmpAdapter(PiAdapter)` (`backend/app/adapters/omp/adapter.py:27`) herda a mesma leitura;
diferenças de raiz, spawn, tools `question`/`ask` e status ficam nos adapters Python.

- Uma linha com vários blocos usa id do nó e `:<índice original>`; não renumerar após filtros.
- Timestamp de mensagem é ms → segundos. Texto remove CSI ANSI e scrub de surrogate; imagem
  em resultado vira `[imagem <mime>]`, sem base64. Pensamento continua `thinking` recolhido.
- `Stream` (`pi/transcript.py:213–295`) segura usuário até a próxima linha, pois o hook escreve
  seu marcador depois. Só corta prefixo quando `parentId` casa; não usar heurística textual.
- No SSE, mensagem liberada por `feed` leva o início da linha que a libera. No flush, o código
  usa a posição da última tentativa de leitura: pode ser EOF, início de linha parcial ou início
  da última linha completa da releitura (`backend/app/transcript.py:841–895`), embora o comentário
  diga “última linha lida”. Preservar o código, não essa descrição. O atraso de 200 ms só existe quando retém
  algo. Na leitura de histórico, flush imediato e `ev.ts` original preservam a ordem.
- Parser/histórico atuais seguem ordem física; não acrescentar seleção de ramo/fork como parte
  da migração. Nova instância do Stream por tentativa de janela e por geração do arquivo.
- **Buraco de retomada encontrado por leitura do código:** se uma fala Pi com dois blocos sai
  junto da linha seguinte, seu id SSE aponta para essa linha seguinte. Recomeçar nela sem o
  estado retido não recompõe os blocos. O flush em EOF tem o mesmo problema. O plano propõe
  preparar o parser com o último objeto válido anterior ao cursor, descartando sua saída:
  preserva ids/offsets e recupera irmãos. É reforço necessário da retomada, não paridade com
  o defeito Python. Testes de soltura por metadata, hook e EOF devem comprovar na execução.

### Kimi

`backend/app/adapters/kimi/transcript.py:60–140`: envelope `time` em ms. Usuário só quando
`context.append_message`, role user e `origin.kind=user`; injection/steer não viram fala.
`content.part` separa texto e `think`. ToolCall args que não são objeto viram `{}`.
Resultado sem uuid usa `res:<toolCallId ou parentUuid ou uuid>`; sem todos eles, id vazio tem
aviso sem conteúdo de conversa. Resultado objeto sem `output` textual usa `json.dumps` Python
com `ensure_ascii=False` e separadores normais; usar os helpers `pyjson` já portados.

Estado continua por fronteira de turno no adapter/hook, não mtime. Pergunta/aprovação, drive do
picker e confirmação de resposta continuam no Python; portar o parser não os substitui.

### orq

É provider só de leitura, sem pane/processo. `runs.timeline_path` (`orq/runs.py:27`) aponta para
`timeline-<pasta resolvida>.jsonl`; **não** `eventos.jsonl` ou painel. A API mantém 409 de entrada,
interrupção, rename e pareamento. O estado vem de `advance.lock` e atividade (`runs.py:139`).

`orq.adapter.parse_obj` (`:22–37`) produz `notice`, texto cru e `ChatEvent.orq` enriquecido.
`orq_timeline.event_id` (`:264`) calcula `orq:` + primeiros 16 caracteres de SHA1 do
`json.dumps(sort_keys=True)`; o SSE usa nome do transcript + offset, não esse hash.
`entry` (`orq_timeline.py:181–270`) depende de `RunFiles`, `jev-shadow.jsonl`, casamento por texto
até 45 s e `skills/orquestrar/scripts/orq_triage.py`. Regex de perguntas/código inclui lookaround.
O painel e o chat usam o mesmo parser; duplicá-lo em Rust sem migrar painel contradiz a regra.

Recomendação: Rust divide **um** streaming completo materializado pelo Python por sessão orq;
replay autenticado por aparelho cobre a entrada e retomada. `/history` permanece no repasse.
Não guardar transcript em `SideCache.queue`: atualmente ela retém toda `message` sem teto
(`crates/hangar-server/src/side.rs:105–122`). Frames de conversa devem carregar id SSE e posição
separados, com canal limitado e reconexão ao atrasar. O benefício é parse ao vivo compartilhado;
o benchmark parcial não estima custo de triagem real nem prova ganho no histórico.

## Infraestrutura já portável da parte 1

- `routes.rs:140–151`: gate resolve IP e token; proxy conserva status, streaming, websocket e
  remove segredo interno (`proxy.rs:77–94`). CORS/ETag/gzip existentes devem ser reutilizados.
- `transcript/mod.rs:24–46`: Provider contém só Claude/ClaudeHeadless/Codex. Acrescentar Pi/Omp/Kimi
  exige dispatch explícito em **histórico e tail**; não deixar `!= Codex` virar parser Claude.
- `history.rs:182–248`: rewrite atual é escolhido por `!= Codex`; expandir whitelist antes de
  habilitar novos providers. Reusar a mesma junção de fila e crescimento 256 KiB × 4.
- `tail.rs:68–119`: leitor de linhas completas, geração, watcher por pasta e broadcast por
  sessão. SSE id continua `<session_key>:<início da linha>`, query vence cabeçalho, retomada
  relê a linha inteira. `LineParser` precisa feed/flush/held para Pi, sem mudar Claude/Codex.
- `internal_api.py:53–70`: info e side-events compartilham payload; `side_events` hoje exige
  jsonl. Protocol atual Python/Rust = **1**; lista interna sobe para **2** e a ponte orq para
  **3**, nos dois lados e no commit de cada alteração interna.
- `rust_server.py`: reserva no mesmo processo/lifespan, binário inexistente/desligado/incompatível
  ou três quedas em 60 s. Parte 2 não modifica serviço, instalador, cano ou toolchain 1.98.1.

## Uma exigência que o protocolo atual não identifica

O início pede “peer sempre repassado”. O `hangar-send` remoto envia GET com Bearer do peer
(`scripts/hangar-send:238`) sem marcador de origem; se o valor é o token do dono remoto, o gate
atual o reconhece como dono. É indistinguível de celular/nativo daquele dono.

Convidado, par externo com token Share, rotas de peers e pareamento continuam no Python. Não
prometer que toda chamada de um peer que usa **token do dono** fica no Python. Para garantir
isso seria preciso alterar o protocolo dos chamadores ou manter essa rota inteira repassada.
A proposta conserva o gate da parte 1 e registra esta distinção para aprovação; não acrescenta
flag “peer” não autenticada que alegadamente ofereça isolamento.

## Prova e riscos da execução futura

Golden sintético gerado pelos parsers Python: Pi/omp hooks casando e não casando, usuário no EOF,
ANSI/surrogates, imagens e índices; Kimi injection/think/result sem uuid; histórico com fila,
timestamps, limites e múltiplas janelas. Transmissão com linha parcial, truncamento, troca de
provider, recriação com mesmo nome, atraso de receptor e mais de 200 linhas durante desconexão.

Lista: fresco GET versus SSE estável, visibilidade dono/convidado, reset de claims, erro e
recuperação com assinatura igual, nav confirmado antes da nova assinatura, shortcuts vazios,
refcount até zero. Orq: triagem/jev enriquecidos iguais ao Python, ids/offsets preservados,
replay simultâneo ao append e religação sem buraco.

No uso real autorizado: celular/PWA/nativo e servidores conectados, cada provider vivo, queda
de rede e retomada, opção de reserva Python e incompatibilidade protocol. Sem reiniciar serviço
ou criar sessões reais nesta análise. Esta etapa entregou documentos; testes automatizados e
uso real da implementação futura não ocorreram.
