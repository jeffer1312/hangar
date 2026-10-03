# hangar-server, parte 2 — lista compartilhada e conversas dos demais provedores

**Data:** 2026-10-02. **Status:** proposta para aprovação; nenhuma implementação autorizada.
**Base:** `hangar-server-parte2`, commit `f7f797bb`. Primeiro fetch confirmou essa base; fetch final
encontrou `aa8eec36` e `2eef31e8` (teste de processo morto e CI Zig). Incorporar na execução
aprovada; não houve merge/troca de branch nesta etapa. Código de implementação analisado não mudou.
**Análise:** [custos, referências e limitações](../../analise-hangar-server-parte2-2026-10-02.md).

## O que muda

O objetivo é tirar do Python a leitura e distribuição que pode ser compartilhada sem reescrever
agora os adaptadores e o estado. Os clientes continuam nas mesmas portas, rotas e formatos.
Não haverá telas novas nem trabalho específico de quadro/canvas.

| Rota / fonte | Proposta desta parte | Dependência Python conservada |
|---|---|---|
| Dono: `GET /api/sessions` | handler Rust consulta lista interna fresca e devolve a resposta | descoberta, decoração, filtro de visibilidade, invalidação |
| Dono: `GET /api/sessions/events` | um stream interno da lista para N clientes Rust | `_ListRefresher`, leitura de atalhos, marcadores nav, presença do dono |
| Pi, omp e Kimi: `/history` | Rust lê arquivo e junta fila existente | resolução do transcript e fila durável produzida pelo Python |
| Pi, omp e Kimi: `/events` | Rust lê e distribui transcript por sessão | side-events: estado, prévia, perguntas, sugestões, fila e drain |
| `orq`: `/events` | um stream Python materializado por sessão, distribuído pelo Rust; replay finito para entrada/retomada | parser único `orq_timeline`, estado/triagem e arquivos da execução |
| `orq`: `/history` | repasse existente ao Python | leitura e enriquecimento integrais |
| Convidado / demais rotas | repasse existente | autenticação e políticas atuais |

**Mudança de fronteira em relação ao início:** descoberta/decoração da lista e histórico/parser
`orq` não ficam integralmente Rust nesta parte. É recomendação, depende de aprovação. Medições:
GET vivo aquecido 1,043 ms com duas sessões; lista já tem produtor único; parser orq sintético
parcial 133,267 ms para 10.000 linhas, sem triagem real. Não há amostra que prove economia por
tique com centenas de sessões. Ganho esperado aqui: distribuição e parsers compartilhados;
não declarar redução percentual de CPU ou eliminação do Python.

## Fora do escopo

Quadro/canvas, layout/recursos de desktop web, novas telas, geração TS, instalação/distribuição,
toolchain, cano, custos, comando tmux em modo controle, terminal PTY, envio/spawn/drive de picker,
pareamento, ciclo de vida orq, triagem Jev, painel orq, portas 8766/8768 e retirada do Python.
Esses fluxos seguem funcionando por repasse ou pela mesma fonte Python.

## Linha de base

A análise registra método e população de cada número. Serviço vivo Python PID 914179, RSS
201.756 kB, 24 threads, duas sessões sem terminal. Pi/Kimi/omp/orq reais não estavam disponíveis.
Microbenchmarks sintéticos Python: Pi/omp 91,398 ms / 20.000 linhas; Kimi 57,643 ms / 10.000;
orq parcial 133,267 ms / 10.000. São populações diferentes, não comparação de provedores.
Não foi feito build ou medição Rust desta parte, nem teste automatizado.

## Como funciona

### 1. Gate e contrato interno

Reusar `routes::gate`, autenticação, IP confiável, repasse e CORS/gzip da parte 1. Somente GET
com token reconhecido como dono usa atalhos Rust; outros métodos e tokens seguem ao Python.
No HTTPS, cookie simples continua inválido; cabeçalho Bearer vence query/cookie. O segredo
interno só em memória Python/ambiente do filho, nunca em log ou ambiente das sessões.

Novas rotas sob `require_internal` (loopback **e** segredo):

- `GET /internal/sessions`: lista fresca do dono, com a mesma semântica de `api.list_sessions`.
- `GET /internal/list-events`: `list_events(internal_nav=True)`; o conjunto de eventos públicos
  continua igual, mas internamente `nav_snapshot` substitui `nav`.
- `GET /internal/list-nav`: retrato fresco dos marcadores nav para anexação de cliente novo.
- `GET /internal/sessions/{name}/orq-events`: conversa inteira com `info`, frames de transcript
  com seus ids SSE e side-events; retoma via query `last_event_id`.
- `GET /internal/sessions/{name}/orq-replay`: corpo finito com `info`, `start`, `cut` e eventos
  `{offset,event}` gerados pelo mesmo parser. Offset além do arquivo é inválido; sem id válido
  usa a cauda de 200 linhas. Arquivo/geração mudados obrigam cliente a receber `reset`.

Protocol sobe de **1 para 2** na Task de lista e de **2 para 3** na Task da ponte orq em
`backend/app/rust_server.py:RUST_SERVER_PROTOCOL` e
`crates/hangar-server/src/lib.rs:INTERNAL_PROTOCOL`, no mesmo commit das respectivas rotas. Se parte 1
subir esse número antes da execução, usar o próximo número comum após conferir ambos; nunca
rebaixar protocolo. Sem binário, desligado, incompatível ou com quedas, mesma reserva Python
no mesmo processo e sem segundo lifespan. Serviço e instalador não mudam.

**Peer:** protocolo atual não distingue um peer que envia token do dono de um aparelho do dono
(`scripts/hangar-send:238`, `routes.rs:140`). A proposta conserva esse gate. Tokens de convidados
e pares externos, e rotas de peers/pareamento, seguem ao Python. Garantir “qualquer peer sempre
repassado” exigiria mudar chamadores ou abandonar o atalho da rota inteira; não alegar essa
garantia com os cabeçalhos atuais. Essa distinção também precisa da aprovação do dono.

### 2. Lista

Preservar `_ListRefresher` único e a descoberta single-flight. O GET Rust **não** serve o último
`sessions` do SSE, pois `_list_sig` ignora campos de atividade. Consulta interna executa a lógica
atual de frescor (até 2 s, invalidação de criação/rename) e filtro do dono. Falha interna volta
ao repasse original, conservando status e diagnóstico; não devolve lista vazia fictícia.

Um `ListHub` com referência por cliente mantém uma conexão interna. Última referência fecha
fonte e tarefas; o Python executa `app_saiu`/release. Primeiro cliente liga, sem produtor órfão.
Snapshot de entrada e assinatura do canal são obtidos sob a mesma trava: nenhuma mudança pode
cair entre as duas operações. Cache separado de `sessions`, `shortcut_terminals` e estado de
erro; falha produz `list_error`, guarda anterior; recuperação reemite `sessions` mesmo igual.

`nav_snapshot` interno é `{observed_at, pending:{name:{url,ts}}}`, com `observed_at` do
`time.monotonic_ns()` Python após copiar marcadores vivos. Nunca aparece no SSE público. Cada
cliente consulta `/internal/list-nav` antes da anexação, entrega os pendentes frescos e guarda
timestamps vistos. Retrato observado antes desse GET é ignorado mesmo se chegar atrasado.
O stream interno publica esse retrato a cada1s, inclusive vazio/igual: a próxima leitura corrige
a corrida entre GET e assinatura. Só a mudança de timestamp gera `nav` público. Não reter nav
confirmado para novo cliente; falha interna limpa o retrato nav até nova confirmação da fonte.

Lista pública mantém ping de 8 s, comentário de 15 s, `Cache-Control: no-store`,
`X-Accel-Buffering: no`, CRLF e sem `retry:`/id SSE. Cliente lento que perde broadcast fecha e
reconecta; não inventar `reset` da lista, que os clientes não tratam. Falha da fonte interna
emite `list_error` uma vez, religa 1,2,4,8,16,30 s, preserva lista anterior e mantém ping.

### 3. Pi/omp e Kimi

Acrescentar `Provider::Pi`, `Omp`, `Kimi` e parsers isolados. Omp usa o parser Pi existente, com
Stream por arquivo, sem abstração de adapter nova. Reutilizar helpers `py`, `pyjson`, types e
junção de fila. Não portar funções de pergunta/estado/spawn desses módulos.

Pi conserva índice original de bloco, timestamp ms/1000, ANSI removido, surrogate substituído,
thinking, tool ids e placeholders de imagem. Só o marcador de hook com parentId correto corta
prefixo de usuário. Stream retém uma linha, flush imediato no histórico e espera 200 ms no tail
quando houver retido. A posição SSE precisa seguir o código Python: feed libera com offset da
linha corrente; flush usa posição de EOF/parcial/última linha da releitura. Não “corrigir” esse
contrato junto com o porte. A espera pode segurar somente o estado do leitor daquela sessão
no `spawn_blocking`, para o corte e o flush serem consistentes; nunca o mapa de hubs/watchers
ou o laço de eventos. Um attach da mesma sessão pode esperar esses 200 ms.

Para retomar no cursor da linha que liberou o usuário ou no EOF, preparar o parser Pi com o
último objeto válido anterior ao cursor, descartando sua saída. Isso recompõe os irmãos da
mesma mensagem sem trocar ids/offsets. Corrige uma perda possível do leitor Python atual e
precisa de testes próprios de metadata, hook e EOF; não ocultar essa diferença na alegação
de paridade.

Kimi preserva filtro `origin.kind=user`, think, `res:<cid>` e output de ferramenta com a
serialização Python quando é objeto sem output textual. Estado não usa mtime do wire.

`history::parse_from` passa a distinguir explicitamente cada provider; RewriteFilter só nos
Claude. Cada tentativa de janela Pi tem Stream novo. Fila, dedup com relógio, poda de sessão
anterior e corte final por limit seguem a parte 1. SSE conserva query sobre Last-Event-ID,
overlap da linha com vários blocos, geração, reset, linhas parciais e recurso por sessão/pasta.

### 4. orq sem duplicar triagem

Um hub específico consome streaming completo Python, incluindo `info` e posição de cada frame.
Não usa `LineParser` JSONL Rust nem `SideCache.queue` para a conversa. Reusa cache dos últimos
eventos de estado; transcript passa pelo canal limitado e **nunca** vira histórico em memória.
Na ponte, `bridge=True` difere de `side=True`: manda info nas trocas, mas continua mandando
transcript com ids, usando `merged_events` e `orq_timeline.entry` atuais.

Ao ligar, upstream inicia em offset 0; ninguém recebe automaticamente essa varredura. Cada
cliente primeiro assina o canal, depois busca replay finito. Durante replay, frames entram no
canal limitado. Após obtê-lo, entrega replay e só frames novos da mesma geração a partir do corte.
Geração/info divergentes ou receptor atrasado → reset e reconexão, nunca seguir com buraco.
Flush/linha parcial/truncamento devem ser testados com o id público real. Na religação upstream,
retoma com último id SSE; retransmissão pode repetir, o cliente deduplica por ChatEvent.id.
Não limitar recuperação interna às últimas 200 linhas: o intervalo da queda pode ser maior.

`/history` orq conserva o repasse atual e sua montagem dinâmica. Campo `orq` continua enriquecido
na história e no ao vivo, mesmo com Jev/triagem indisponível (fallback e aviso existentes).
Provider de leitura mantém recusa 409 de entrada; não cria pane, fila entregável ou processo.

## Como provar

Testes primeiro no plano; **rodar só quando o dono pedir**. Fixtures sempre sintéticas, geradas
pelos parsers e rotas Python atuais; código proposto nos documentos não foi compilado.

- Golden Pi/omp/Kimi: todas as variantes de texto/tool/result/think, hook correto/errado/EOF,
  ANSI, surrogate, múltiplos blocos, timestamps, fila, limites e expansão de janela.
- Tail Pi: feed e flush, EOF/parcial, marcador chegando durante os 200 ms, id/offset exatos;
  linha parcial, rebind, truncamento, reconexão no meio de multi-bloco e troca de provider.
- Lista: GET fresco separado de SSE estável, claims que escondem sessão do dono, guest proxy,
  shortcuts vazios, erro/recuperação mesma assinatura, nav confirmado e expiração, vários
  clientes e refcounts até zero, atraso do receptor, falha de conexão interna.
- Orq: campo enriquecido igual, ids/hash/offset, append durante replay, troca de execução,
  queda com mais de 200 linhas, receptor lento, entrada continua recusada.
- Protocol/reserva: valores coincidem; binário incompatível não assume porta; não introduzir
  lifespan adicional. Checks da parte 1 tocados continuam exigidos quando autorizados.

No uso real, com o dono após execução: lista e chats em celular/PWA/nativo, provedores reais,
abrir segundo aparelho, perda de rede e retomada; convidados e servidor remoto; medir novamente
o mesmo cenário. Restart/CP_RUST_SERVER=0/Windows exigem autorização da sessão de teste — não
executar no serviço vivo por conta. Registrar população, bytes, tempos e o que não pôde conferir.

## Pronto quando

Os quatro streams extras seguem o contrato; Pi/omp/Kimi history usam Rust; lista do dono tem
fanout Rust com um produtor Python; orq ao vivo usa um parser por sessão e história Python
preservada; convidados e demais endpoints funcionam pelo repasse. Protocol/reserva conferidos,
fixtures/checks autorizados passam e uso real é registrado sem perda/reabertura de nav.

Não declarar que a lista é integralmente Rust ou que o histórico orq ganhou desempenho. Esses
limites permanecem documentados para a próxima parte. Antes de executar, o dono aprova esta
fronteira e a interpretação de peers; depois seleciona o fluxo de execução do Superpowers.
