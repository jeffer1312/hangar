# Migração do backend para Rust — roteiro e estado

Atualizado em 2026-10-07. Ponto de partida para cada parte nova: ler este arquivo, a análise
inicial e a spec da parte 1. Tudo desta pasta vive na branch do PR #24 (`hangar-server-parte1`):
um pull nela traz a documentação em qualquer máquina.

## O que tem nesta pasta

| Arquivo | O que é |
|---|---|
| `analise-inicial.md` | Medições e motivos da migração, as 7 partes |
| `parte1/spec.md`, `parte1/plano.md` | Conversas do Claude/Codex no Rust, `hangar-cano`, tipos compartilhados |
| `parte1/registro-da-execucao.md` | Decisões tomadas na execução, detalhes adiados, o que ficou para conferir |
| `parte2-claude-codex/` | Análise e spec da parte 2 (2A–2D) e o plano da 2A executado |
| `parte2b/` | Análise, spec e plano da 2B (sem terminal no Rust) — aguardando aprovação |
| `parte2c/` | Análise, desenho, planos, revisões e provas da 2C (com terminal no Rust) |
| `parte3/` | Análise, spec e plano da parte 3 (custos e uso no Rust) |
| `git-arquivos/` | Núcleo compartilhado de Git/arquivos, ponte privada, integração e evidências de validação |
| `dono-unico/` | Inventário das passagens Python↔Rust com o Rust vivo, desenho do dono único, plano e o registro de cada Task — executado na `feat/rust-single-owner` (Tasks 1–10; falta a prova de uso real, Task 11) |
| `parte2-descartada-lista-pi-kimi/` | Primeiro escopo da parte 2 (lista + Pi/Kimi/omp/orq), trocado pelo dono |
| `lista-estado/` | Lista de sessões no Rust: medição (o repouso de 4% era o Supervisor), inventário, desenho, plano e provas. Fases 0–B executadas (lista do dono no `ListHub`); a Fase C e a Task 24 foram substituídas pela `parte4/` |
| `parte4/` | Estado, prévia, pergunta nativa e terminal real no Rust: inventário, desenho, plano, medições (`medicao.md`), prova isolada (`prova-real.md`) e achados sem conserto (`achados-pendentes.md`) |
| `pedidos/` | Pedidos enviados às sessões Codex e os achados de revisão da 2C |
| `parte5-codex/` | Parte 5, todo o Codex no Rust: análise (`analise.md`) e spec em subpartes 5A–5I (`spec.md`, aprovada em 07/10), planos da 5A (`plano-5a.md`) e da 5B (`plano-5b.md`) |
| `../../backend/tests/fixtures/accounts_contract/` | #115: inventário público de rotas, consumidores e autoria de arquivos; goldens Python e harness isolado para a migração de contas Claude/Codex |

Os caminhos absolutos dentro de `pedidos/` e dos documentos das sessões apontam para as pastas de
trabalho da máquina de origem; nesta pasta os arquivos equivalentes são os da tabela acima.

## Como a migração funciona

- **Estrangulamento.** O `hangar-server` (Rust) fica na porta 8765. Ele atende sozinho o que já foi
  portado e repassa o resto ao Python, que escuta numa porta interna. O Python sobe e vigia o Rust
  e assume a porta sozinho se ele faltar ou cair. A cada parte, mais rotas passam para o Rust; no fim
  o Python sai e o instalador leva um binário só.
- **O Rust é o único dono do que já migrou (decisão do dono, 04/10/2026).** Fica só a reserva do
  processo inteiro: Rust ausente, incompatível ou caindo → o Python assume a porta e tudo. Com o
  Rust de pé, falha numa operação migrada vira erro com código e motivo (diário e
  `hangar-server.log`), nunca a passagem daquela sessão ou operação ao Python. O código Python das
  partes migradas fica só de referência e sai na parte 7. Substitui a regra anterior ("3 tentativas
  + 1 e só aquela sessão vai para o Python"): o código de passagem entre os dois donos foi a origem
  da maioria dos defeitos de 04/10 (primeira mensagem sumindo, "sessão em transferência", entrega
  marcada sem chegar, reserva circular de Git). Plano da mudança em `dono-unico/`; o que saiu está
  em `docs/decisoes/superado.md`.
- **Estado do dono único (04/10/2026, branch `feat/rust-single-owner`, contrato interno 17).** O
  processo tem um modo só: `pending` (Rust esperado ou voltando de uma queda 1–2; operações esperam
  até 30 s), `rust` ou `python` (sem binário, `CP_RUST_SERVER=0`, `--reload` ou desistência do
  Supervisor). Sessão Claude sem terminal e com terminal nasce no Rust; a administração fecha e
  reabre nele; o terminal empresta o teclado ao Python por uma operação (desde a 5-0 não mais nas escritas de chat, resposta e seleção); histórico, eventos,
  Git/arquivos e a observação do terminal respondem erro com código em vez de repassar. Codex sem
  terminal, Pi, Kimi, omp e orq seguem no Python (provedores não migrados).
- **Contrato interno versionado à mão.** Mudou rota `/internal`, evento do `side-events` ou variável
  passada ao filho → subir `RUST_SERVER_PROTOCOL` (Python) e `INTERNAL_PROTOCOL` (Rust) juntos.
  Atual: **37** (parte 5-0; a 35 era a parte 4 com as junções da `hangar-server-parte1`).
  O `versao` do snapshot do `hangar-cano` acompanha o `VERSAO` do `cano.py`.
- **Paridade provada por golden.** Formato que o cliente lê sai igual ao do Python, conferido por
  fixtures sintéticas geradas pelos parsers Python (`backend/tests/fixtures/contract/`).

## Partes

| # | Parte | Estado |
|---|---|---|
| 1 | Conversas do Claude e do Codex (`/history` e `/events`) + `hangar-cano` + tipos compartilhados (`crates/hangar-api`) | **Feita, no PR #24.** Rodando na máquina do dono pelo canal de testes. Falta teste no uso real e VM Windows |
| 2 | Sessões do Claude e do Codex, com e sem terminal — Pi, Kimi, omp, orq, lista, quadro e canvas ficam para depois (dono não usa agora) | Dividida em 2A, 2B, 2C e 2D. **2A** (fim da travada da prévia, no Python): feita, no PR. **2C** (com terminal: tmux `-C` no Rust, contrato versão 3): feita, no PR; testes verdes em Linux, Windows e macOS desde `16f45cad` (no Windows a 2C fica desligada de propósito e o Python atende). **2B** (sem terminal no Rust: cano, fila e controle; contrato interno versão 7, cano versão 2): feita, no PR (`75f4b003`); CI verde em Linux, Windows e macOS. Falta uso real com o dono, teste opcional do Codex, medições e interoperação Python/Rust no Windows. Plano em `parte2b/`. Fila sem terminal no formato v2 (contrato 8, `fix-queue-compaction`): o arquivo v1 é podado e regravado no lugar na primeira abertura, sem cópia; voltar o app para uma versão de contrato 7 não é suportado para a fila das sessões sem terminal. **2D** (envio ao Claude com terminal; contrato versão 10 na junção): em planejamento e execução na sessão Codex `rust-parte2d`, branch `hangar-server-parte2d`, documentos em `parte2d/` |
| 3 | Custos e uso: `/api/costs`, `/api/uso`, `/api/cotacao` e custo de sessão Codex no Rust com índice SQLite próprio; cotas e stats ficam no Python | Feita na branch `hangar-server-parte3` (contrato 8) e juntada ao dono único em `feat/parte3-custos-rust`, contrato **20** (dono único + worktrees no Rust, 19, entraram antes): falha vira 503 com código, sem passagem ao Python. Paridade Python/Rust comprovada; coleta fria 3,270 s, incremental 0,038 s e pico completo 93 MiB. Falta uso real com o dono. [Medidas e isolamento](../decisoes/plataforma.md#custos-e-uso-no-hangar-server) |
| 4 | Estado e prévia (tmux em modo controle, cópia da tela por sessão) + terminal real (`portable-pty`) | **Feita na `feat/parte4`** (Tasks 1–12, contrato 35). Com o Rust de pé, o `Monitor` do Rust é o único dono do estado ao vivo, da prévia, da pergunta nativa, da sugestão e do `problema` de Claude com terminal em qualquer porta (psmux avulso no Windows), e a lista lê o estado dele; todo PTY do terminal real é do Rust, Windows incluído (ConPTY), e o Python só faz a porta de entrada da 8766/8768. A ponte do observador terminal ficou sem consumidor. Medidas em `parte4/medicao.md` (20 chats trabalhando: Python 176,5 → 29 ms/s; terminal: CPU por MB pela metade), prova isolada em `parte4/prova-real.md`. Falta o uso real com o dono no celular/app/nativo (Task 13, Step 30) e a VM no estado do chat (Step 31; terminal na VM já conferido). [Estado](../decisoes/plataforma.md#estado-ao-vivo-de-claude-com-terminal-no-monitor-do-rust) · [terminal](../decisoes/plataforma.md#hangar-server-a-porta-pública-em-rust-o-python-atrás) |
| 5 | Adaptadores dos provedores e envio de mensagens (mais mudam com as CLIs; tipos gerados do schema do app-server do Codex) | Metade Codex, **5A feita** (PR #107: tipos do protocolo em `crates/hangar-codex`, motor tipado, aviso de versão, cliente stdio/WebSocket); 5B–5I na `parte5-codex/spec.md`, com o módulo comum de processo do cano na 5B. Metade Claude, **5-0 feita** na branch `hangar-server-parte5-claude` (contrato 37): as escritas do dono em sessão Claude (`/input`, `/steer`, `/interrupt`, `/keys`, `/select`, `/answer`, fila), com e sem terminal, são do Rust, com porta de entrada por sessão que o Python fecha ao congelar ou transferir. Falta uso real com o dono; [roteiro de medição](parte5-claude/medicao-5-0.md) sem números. Resto da metade Claude e metade Codex: em `parte5-claude/` e `parte5-codex/`. [Decisões](../decisoes/plataforma.md#escritas-do-claude-no-hangar-server) |
| #115 | Contas, autenticação, login, identidade e cotas Claude/Codex | Frente separada da 5E e da parte 6; modelos ficam na 5E, integração geral na 5F e transferência na 5G. Contratos e referência Python em `backend/tests/fixtures/accounts_contract/`. Preparação fica no Python por gancho restrito. A implementação e a prova real têm aceite próprio; não estão concluídas pela captura dos goldens |
| 6 | Resto da API: criação/troca de conta da sessão, convidados/8766, pareamento, MCP (`rmcp`), push (`web-push`), atualização e outros provedores | — |
| 7 | Remover o Python: binário único no instalador | — |

Cada parte: spec própria → plano → execução por subagente com revisor por Task → revisão final →
PR em rascunho → teste no uso real pelo canal de testes (abaixo) → merge.

Lista de sessões (fora da numeração, `lista-estado/`): Fases 0–B feitas, contrato 27; a lista do
dono sai do `ListHub` do Rust ([medidas](../decisoes/plataforma.md#lista-do-dono-no-hangar-server)).

## Ainda no Python

Com o Rust de pé (07/10/2026, depois da 5-0):

- **Escritas que nascem no Python:** broadcast, grupo, par, MCP, `/then`, `/loop`, criar sessão e
  `/model-effort` seguem por `coordinator.op`/`_send_one`, cobertos só pelo `freeze`, sem a porta de
  entrada do Rust. Convidado e Connect (8766/8768) também.

- **Provedores não migrados (parte 5):** Codex sem terminal e o envio do Codex com terminal; estado
  do Codex (eventos do app-server); estado e prévia de Pi, omp e Kimi (`StateMonitor` e
  `PreviewBroker` Python); orq; fatos de entrada do executor (`runtime_terminal.facts`).
- **Fatos e serviços que o `Monitor` e a lista pedem:** plugin (estado recente, pergunta segurada,
  sugestão, presença do app), `em_troca`, modo de permissão (`permission.observe`), `session.dead`,
  e a entrega (`session.deliverable` → `adapter.drain`).
- **Portas 8766 e 8768:** porta de entrada do convidado e do Connect (estado, prévia, pergunta e
  sugestão pelo canal privado do hub; transcript e fila lidos pelo Python; terminal ligado a
  `/__hangar_server/term`), a Origin do terminal (`/internal/term/origin`) e
  o 409 de painel aberto (pergunta `term.active` ao Rust).
- **#115:** contas, login e cotas Claude/Codex; com a migração ativa, o Rust mantém autoria
  exclusiva e os reconciliadores Python ficam limitados à preparação de configuração.
- **Parte 6:** convidados, pareamento, MCP, push, atualização, uploads, ditado; contas/cotas
  dos outros provedores, stats, criação/troca de conta da sessão e mutações de worktree.
- **Parte 7:** o Supervisor que sobe o Rust e a reserva do processo inteiro (modo `python`).

## Testar uma branch no app (canal de testes)

Já na `main` (`c1751412`): `CP_UPDATE_BRANCH=<branch>` no `backend/.env` faz o "Atualizar" puxar a
branch em vez da `main` (com resgate, stash e rollback iguais). Fora da `main` a tela é compilada na
máquina. Esvaziar o campo volta para a `main` no próximo Atualizar manual. O backend lê o `.env` só
ao subir: depois de editar, reiniciar.

Na `hangar-server-parte1`, o `server.yml` compila a branch a cada push e publica em
`server-<branch>`; o download dos binários escolhe a release pela branch do checkout.

## Medição Python × Rust (parte 1, 02/10/2026)

Mesmos arquivos reais (lidos só na máquina, saída idêntica nos dois lados); mediana de 3 rodadas,
processo novo por rodada, tempo só da chamada; i5-13400F com outras sessões rodando (ruído 5–10%);
memória = pico do processo (o Python tem ~46 MB só de imports). "8 juntos" = 8 threads num processo,
como o backend faz. Rust = build glibc (o zigbuild 2.28 mediu igual).

| Arquivo | Leitura | Python | Rust | Ganho | Pico de memória Python → Rust |
|---|---|---|---|---|---|
| Claude 286 MB | histórico inteiro | 378 ms | 268 ms | 1,4× | 54–92 MB → 11 MB |
| Claude 286 MB | últimas 200 | 53 ms | 36 ms | 1,5× | → 9 MB |
| Claude 286 MB | 8 juntos | 3572 ms | 543 ms | 6,6× | → 49 MB |
| Claude 286 MB | chat ao vivo (arquivo todo) | 657 ms | 363 ms | 1,8× | → 11 MB |
| Codex 66 MB | histórico inteiro | 380 ms | 229 ms | 1,7× | 67–108 MB → 18 MB |
| Codex 66 MB | últimas 200 | 188 ms | 25 ms | 7,5× | → 8 MB |
| Codex 66 MB | 8 juntos | 2312 ms | 379 ms | 6,1× | → 90 MB |
| Codex 66 MB | chat ao vivo | 248 ms | 214 ms | 1,2× | → 14 MB |
| Claude 3,5 MB | histórico inteiro | 16 ms | 11 ms | 1,5× | 47–53 MB → 6 MB |
| Claude 3,5 MB | últimas 200 | 24 ms | 12,5 ms | 1,9× | → 6 MB |
| Claude 3,5 MB | 8 juntos | 154 ms | 41 ms | 3,8× | → 11 MB |
| Claude 3,5 MB | chat ao vivo | 18 ms | 12 ms | 1,5× | → 5 MB |

No histórico do Claude grande, os dois lados pulam a maior parte do arquivo por um atalho de regex
(`silent_attachment_timestamp`); o chat ao vivo lê tudo. O ganho grande é em leituras simultâneas,
onde o GIL serializa o Python.

## Lições da parte 1 (valem para as próximas)

- **Medir antes de afirmar.** O ganho real do Rust aqui é concorrência (6–9× em leituras
  simultâneas) e memória (10 MB contra 54–92 MB numa leitura grande); uma leitura sozinha ganha
  1,2–1,8×, porque o Python já usava atalhos.
- **Build Linux:** glibc 2.28 via `cargo zigbuild` (roda em Debian 10+, Ubuntu 20.04+, RHEL 8+).
  musl foi testado e descartado: o alocador dele custou até 5× mais memória em leituras simultâneas.
- **Todo binário novo tem reserva.** O `hangar-cano` é testado uma vez antes de usar e cai no
  `cano.py` se não rodar; o `hangar-server` cai no Python. Nada novo pode deixar a porta ou a sessão
  sem dono.
- **Log nunca leva texto de conversa:** pânico registra só arquivo:linha; erro de serde não vai cru.
- **Execução:** o código de port vindo pronto no plano (compilado antes numa cópia) fez as Tasks
  passarem de primeira; as revisões acharam o que importava em segurança, concorrência e paridade.

## Desempenho: erros que já custaram (pedido do dono, 05/10/2026)

Toda parte nova é escrita pensando em desempenho e conferida contra esta lista antes de juntar.
Cada item foi um defeito real, achado depois de ir para a máquina do dono.

- **Nada de laço apertado.** Varrer ou ler de novo só quando a entrada muda, com teto de
  frequência. O Supervisor varria ~560 processos 4×/s e regravava o registro com fsync (4,33% de um
  núcleo parado; desce a árvore do Rust desde `55d26bec3`: 0,73%); o git esperava girando a cada
  1 ms (`905fc74e2`).
- **Chamada ao Python é cara: só quando a entrada muda ou com prazo.** A linha de status era
  pedida ao Python a cada mudança de estado, ~7 chamadas/s com uma sessão sem terminal
  transmitindo; agora só quando as entradas mudam ou a cada 30 s (`2a0d07d89`).
- **Não republicar o que não mudou.** Saíam 451 eventos de estado por minuto por aparelho, só 100
  diferentes (`2a0d07d89`).
- **fsync fora de trava e fora do laço de eventos; leitura nunca grava.** `terminal_facts` fazia 3
  gravações com fsync por leitura (`905fc74e2`); fsync sob trava congelou o notebook por 6 s na 2B.
- **Processo filho custa, sobretudo no Windows (~25 ms cada).** Não abrir `git` para o que pode
  ser pulado (`2148edf56`).
- **Estado em disco pequeno e podado.** O estado da 2B chegou a 88 MB.
- **Medir antes e depois, em release**, com backend isolado e sem cliente; debug infla a análise
  ~30×. Medir pico de memória (RSS) junto com CPU; heaptrack ou dhat quando o pico subir.
- **Memória e recursos no Rust (pedido do dono, 05/10).** O objetivo é velocidade; memória se
  economiza só quando não custa tempo (cache que acelera vale a memória, com teto). Dado compartilhado por `Arc`/referência,
  sem clone por tique; buffers reaproveitados e `with_capacity`; todo cache e toda fila com teto e
  invalidação por mudança; arquivo em fluxo ou só a cauda, serde emprestado onde o dado não
  sobrevive à leitura; I/O bloqueante em `spawn_blocking` e paralelismo com teto (`Semaphore`);
  canais com limite, nunca `unbounded`; dado grande solto assim que termina o uso.
- **Compilação na máquina:** no máximo 2 `cargo` ao mesmo tempo, `CARGO_BUILD_JOBS=4`, `target/`
  por worktree e apagado no fim; plugin `rust-analyzer-lsp` desligado nas worktrees de Task (um
  por sessão, ~3 GB cada, levou a máquina a carga 62 e 25 GB de swap). Task testa o Linux na
  máquina e não sobe a própria branch; Windows e macOS saem do CI no push do lote.

## Pendências da parte 1

- **Uso real** (roteiro em `docs/decisoes/plataforma.md`, entrada "hangar-server"): abrir chats do
  Claude e do Codex no celular, web e nativo; derrubar a rede e reconectar sem perder nem duplicar;
  reiniciar o backend com sessão sem terminal viva; `CP_RUST_SERVER=0`; medidas de "depois".
- **VM Windows:** cano abrindo sessão com `hangar-engine.CMD`; `scripts/windows-tasks.ps1`;
  `install.ps1` (sem `pwsh` aqui, nunca rodou); volta do Python para a 8765 depois de queda.
- **CI:** primeira execução do `server.yml` no GitHub (Zig, cargo-zigbuild, release por branch).
- **29 detalhes menores** anotados no registro da execução
  (`parte1/registro-da-execucao.md`),
  nenhum bloqueante. Os mais prováveis de aparecer: sem checagem de vida do Rust depois que sobe;
  binário presente mas incompatível só é detectado no `hangar-cano`.
- **Achados fora da migração:** na `main`, `cargo test` do `desktop-native` não compila
  (`src/app.rs:5790` usa `HashSet` sem import); 3 bugs do Python que derrubam o chat de uma sessão
  (surrogate solto com timestamp no `RewriteFilter`, linha JSON que não é objeto, U+2028 na fila).
- **Rust 1.99.0** saiu; a atualização (crates + nativo juntos) fica para um trabalho separado.
