# Parte 2B — plano executável do runtime Claude/Codex sem terminal no Rust

> **Para os agentes de execução:** usar `superpowers:subagent-driven-development` ou
> `superpowers:executing-plans`, conforme método aprovado pelo dono. Marcar cada Step concluído
> neste arquivo. Esta entrega é planejamento; aguardar aprovação antes de implementar.

**Goal:** migrar para Rust a conexão do cano, processamento de eventos, fila, entrega, controles
e recuperação de Claude e Codex sem terminal, como uma unidade com um único escritor.

**Architecture:** um ator por chave durável e geração possui o cliente do cano e o diário da
fila. Núcleos puros por provedor produzem efeitos; leitores, escritores e persistência ficam no
ator. Python oferece a API existente, preparação de conta/processo e serviços administrativos
delimitados; assume a reserva apenas após obter a mesma trava usada pelo Rust.

**Tech Stack:** Rust 1.98.1, edition 2024, Tokio/Axum/serde e dependências existentes; Python
3.14 na ponte e na reserva. Sem atualizar CLI ou dependências nesta entrega.

**Spec:** [desenho da 2B](../specs/2026-10-03-hangar-server-parte2b-design-claude-codex.md).
**Análise:** [runtime da 2B](../../analise-hangar-server-parte2b-2026-10-03-claude-codex.md).
Os documentos de 02/10 ficam como referência histórica.

**Base obrigatória:** branch `hangar-server-parte1`, commit **a80a9e45**, já contendo a 2A,
no PR único **#24**. **Base efetiva da sessão de execução:** `93ce2f65`, já com a 2C
integrada e protocolo 3; worktree `hangar-server-parte2b`, branch homônima. A worktree de
planejamento permanece em `f7f797bb`; não houve integração
nem alteração do produto. O plano fixa interfaces, algoritmos que exigem decisão e testes;
nenhum código, fixture, teste, compilação, serviço, commit ou push aqui descrito foi executado.

## Restrições gerais

- Só Claude/Codex sem terminal. Pi, Kimi, omp, orq, lista, quadro, canvas, PTY, voz, nova UI e
  controles de terminal ficam fora. A 2C observa terminais; a 2D trata entrada Claude terminal.
- A 2A já está pronta na base. Preservar seu comportamento e suas correções de rename,
  geração, EOF antigo e isolamento de falhas. Não abrir outra correção Python preliminar.
- Um responsável por `meta.key`, com trava de arquivo durante toda a posse do cliente e da
  fila. Nome, transcript, sid/thread e número de conexões SSE não definem essa identidade.
- Transições bloqueiam mutações. Timeout ou falha de saúde com Rust vivo nunca permitem
  tentar o mesmo input pelo Python. `Unknown` não é reenviado automaticamente.
- Cano v1 vivo continua na reserva até reabertura natural ou ação explícita. Não matar CLI
  saudável para atualizar o cano. Cano v2 e contrato interno do servidor têm versões separadas.
- Snapshot/linha CLI: 16 MiB. Envelope escapado: 32 MiB + 1 KiB. Não truncar input ou pedidos
  pendentes. Prefixo incompleto tem `complete:false` e não é publicado como texto completo.
- Prévia mantém primeira publicação imediata, intervalo de 150 ms e última/limpeza imediatas.
  Estado, pergunta, aprovação, uso e despacho não aguardam o timer da prévia.
- Segredo, endereço e nonce privados ficam na memória do pai e no ambiente do filho Rust;
  nunca no `os.environ` global, no ambiente da CLI, em URL pública, saúde ou log de conteúdo.
  Esta regra sobre endereço/nonce refere-se ao IPC novo da 2B; preservar `terminal_address`
  anunciado pela saúde da 2C, sem publicar segredo ou nonce de autenticação.
- Preservar formatos públicos, autenticação, convidados, IDs de evento e leitura da parte 1.
  Autorização pública continua Python; o responsável pelo runtime independe de quem chamou.
- Identificadores novos em inglês; documentos, comentários e mensagens ao usuário em pt-BR.
  Sem renomear código adjacente, formatar com `--write` ou remover implementações de reserva.
- Testes são escritos primeiro; **rodar apenas quando o dono pedir**, conforme regra do repo.
  Os comandos abaixo são para execução futura autorizada. Verificação real precisa do canal
  acordado com o dono; não iniciar outra cópia do backend, reiniciar serviço ou dirigir CLI viva.
- Não criar/trocar branch ou worktree nesta etapa. Não usar `add -A`, `add .`, force, reset ou
  exclusão de branch. Commit futuro só com caminhos da Task e mensagem descritiva. Sem push
  por este plano; docs ignorados não serão forçados para dentro do commit.

## Foco de revisão

- Reply da CLI chega antes do ACK do cano: resultado definitivo não vira `Unknown` nem causa
  reenvio por ACK tardio/perdido (Tasks 2, 6, 7 e 8).
- Transferência com pergunta e prévia em voo: `peek` não tira o escritor antigo; snapshot
  restaura prefixo e ID tipado antes de liberar comandos (Tasks 2, 5 e 9).
- Dois prompts iguais: uma ocorrência no transcript confirma uma entrada só, mesmo entre
  chamadas e após reinício; cursor é capturado antes do despacho (Tasks 3 e 4).
- Rename, mudança de thread/modo e `/clear` durante espera: comando antigo não chega na vida
  seguinte e fila gerenciada não volta a ter JSONL como única autoridade (Tasks 5, 10 e 11).
- Falha de projeção, lock ou IPC: erro aparece; não há arquivo vazio inventado, histórico velho
  com 304, outro escritor ou outra CLI para contornar a falha (Tasks 3, 5, 8 e 11).

## Ordem e encontro com a 2C

Tasks 1–5 definem contratos, transporte, persistência e posse. Tasks 6–7 portam os provedores
sobre essas interfaces; Tasks 8–11 conectam ator, serviços, fachadas e fluxos públicos. Task 12
fecha a prova. Executar em ordem nesta árvore; eventual trabalho paralelo só em árvores
separadas, preservando um escritor por árvore e as interfaces desta seção.

Antes da primeira edição, após aprovação:

```bash
git status --short
git fetch origin
git show --no-patch --oneline a80a9e45
git log --oneline a80a9e45..origin/hangar-server-parte1
git diff --stat a80a9e45..origin/hangar-server-parte1
```

Conferir a ponta real do PR e da `hangar-server-parte2c` antes de integrar. Em 03/10, a sessão
rust-parte2c esclareceu que `d21a445b` já usa protocolo **2** local com acquire/capture/release/
reduce. Retirar `reduce` e a segunda chamada HTTP do reducer muda esse esquema: a correção
da 2C sobe **Python e Rust para 3**, ainda sem push. Não presumir que a correção terminou.
`terminal_address` continua anunciado na saúde; `side-events` não muda.
A 2B não publicou contrato: continua documental, com base original em protocolo 1. A ordem
acordada é **2C corrigida → 3; primeiro contrato novo da 2B → 4 após integrar a 2C**. Se outro
contrato entrar antes, usar o próximo valor real; nunca reutilizar número para formatos distintos.
Cada mudança interna posterior recebe novo número e sobe as duas constantes juntas. A base
mínima continua a80a9e45 com a 2A, mais a ponta corrigida da 2C antes das Tasks compartilhadas.
Não fazer merge carregando mudanças alheias. Se houver evolução que altere estas interfaces,
ajustar o contrato conjunto antes da edição dependente; mudança de escopo volta ao dono.

| Ponto compartilhado | Encontro da 2B com a 2C |
|---|---|
| `rust_server.py`, `lib.rs`, `Config`, `AppState` | Um Supervisor e um processo Rust. Adicionar o listener privado preservando pool, campos e inicialização terminal da 2C. |
| `internal_api.py`, constantes de protocolo, ambiente do filho | Subir `RUST_SERVER_PROTOCOL` e `INTERNAL_PROTOCOL` juntos para o próximo valor da branch integrada, em cada commit que altere o contrato. Nunca reservar o mesmo 2 nas duas branches. |
| `side-events`, `sse.py`, adapters | 2B é fonte headless; 2C é fonte terminal. Selecionar por provedor, modo e geração. Nenhuma publicação duplicada de estado/prévia. |
| `terminal_observer` e `/__hangar_server/terminal` previstos pela 2C | Preservar configure/deactivate no Supervisor. Reserva desativa ambos; a 2B não pede captura de terminal. |
| fila e Codex | Codex terminal mantém app-server/WebSocket e controle Python. Fila já gerenciada conserva seu estado autoritativo se mudar para terminal. |

## Arquivos e interfaces comuns

Criar `crates/hangar-server/src/runtime/{mod,protocol,cano,queue,receipt,claude,codex,actor,gateway}.rs`.
Criar `backend/app/{runtime_coordinator,runtime_queue,runtime_adapter,runtime_policy}.py`.
Não criar outro serviço, banco, lifespan ou processo CLI. O cano durável existente continua dono
do processo CLI. Reutilizar parsers/transcript e escrita atômica já presentes no repo.

Tipos de `protocol.rs`, usados por todas as Tasks:

```rust
// Campos com nomes existentes em português preservam o contrato do cano.
pub struct ClockSample { pub monotonic_s: f64, pub epoch_s: f64 }
pub enum RequestId { Integer(i64), String(String) }
pub enum Disposition { Accepted, Deferred, Rejected, Unknown }
pub enum WriteOutcome { Written, NotWritten, Unknown }
pub struct RuntimeCommand {
    pub operation_id: String,
    pub kind: OperationKind,
    pub payload: serde_json::Value,
}
pub enum EngineInput {
    Line(serde_json::Value),
    WriteAck { operation_id: String, outcome: WriteOutcome },
    PolicyResult { request_id: RequestId, payload: serde_json::Value },
    Tick,
}
pub enum Effect {
    Write { frame: serde_json::Value, operation_id: Option<String> },
    Publish { channel: String, data: serde_json::Value },
    Policy { kind: String, request_id: RequestId, payload: serde_json::Value },
    Reply { operation_id: String, disposition: Disposition, payload: serde_json::Value },
    WakeQueue,
    StateChanged,
    Stop { reason: String },
}
pub struct RuntimeReply {
    pub operation_id: String, pub disposition: Disposition, pub payload: serde_json::Value,
}
pub struct RuntimeEvent {
    pub key: String, pub generation: u64, pub revision: u64,
    pub channel: String, pub data: serde_json::Value,
}
```

`OperationKind` serializa em snake_case: input, steer, interrupt, answer_questions, select,
set_model, set_effort, set_permission_mode, compact, list_models, list_skills, read_rate_limits,
read_settings, set_mode, skip_question, restart, open_terminal, reload, commands, cwd, detach.
`RequestId` é serde untagged; rejeitar bool, não converter número em string. `RuntimeError`
tem `new(code: &str, message: &str)`; Debug/Display não incluem payload de conversa.

`CanoSnapshot` conserva type/versao/pid/init/aberto/pendentes/ultimo_result/rate_limit/stderr_tail/
saiu e acrescenta `inflight`. `pendentes` continua lista de linhas JSON completas.
Nomes canônicos: `State.name`, `used_occurrences`, `CanoBinding.versao`; não criar aliases
queue_name/used_receipts/protocol_version concorrentes entre Tasks.
`RuntimeTarget` contém key/generation/name/provider, metadata, binding do cano
`{pid,escuta,token,versao}`, lease_path/state_path/projection_dir/transcript e created.
Os paths vêm do backend autenticado; nunca do input público.

Contrato HTTP privado, implementado pelas Tasks 5 e 8:

- `POST /runtime/op`, cabeçalhos `x-hangar-internal` e `x-hangar-runtime-instance`.
  Corpo `{protocol,instance,key,generation,operation_id,clock,command}`; resposta
  `{ok:true,result:...}` ou erro explícito sem troca de transporte.
- `command.kind`: `adopt` (descriptor/carry), `detach`, `submit` (text/steer/pre_transcript),
  `control` (control/payload), `queue` (action), `drain`, `confirm`, `snapshot`, `ensure_projection`.
- `GET /runtime/events`: **um SSE agregado por processo**, sem parâmetro de uma sessão.
  Eventos `RuntimeEvent`; primeiro snapshot por chave, depois revisões e heartbeat.
  Evento antigo de instance/key/generation/revision não atualiza o cache Python.
- `adopt`: `{ready:true,state:...}` somente depois de posse, snapshot, projeção e inicialização
  válida. `detach`: `{detached:true}` somente depois de juntar tarefas e liberar a trava.

---

### Task 1: Fixar contratos e oráculos sintéticos da base com a 2A

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/mod.rs`, `runtime/protocol.rs` na pasta Rust acima;
`backend/tests/gen_runtime_golden.py`, `backend/tests/fixtures/headless_runtime/scenarios.json`,
`backend/tests/fixtures/headless_runtime/claude-golden.json`,
`backend/tests/fixtures/headless_runtime/codex-golden.json`,
`crates/hangar-server/tests/runtime_protocol.rs` e `runtime_contract.rs`.
Modificar `crates/hangar-server/src/lib.rs` apenas para expor `runtime`.

**Interfaces:** produz os tipos comuns acima. O gerador
`generate(provider: str, scenarios: list[dict]) -> list[dict]` alimenta adapters Python da base
a80 com relógio, processo, cano, persistência e serviços falsos. Cada cenário contém metadata,
snapshot, comandos/eventos/ACKs/políticas, relógios fixos e saídas públicas/efeitos esperados.
Nunca importa lifespan nem sobe CLI; normaliza só pid, caminhos temporários e UUIDs gerados.

- [ ] **Step 1: Escrever cenários e as verificações do contrato**

`request_ids_keep_type`: `1 != "1"`, bool rejeitado. `clock_domains_stay_separate`: prazo usa
monotônico e reset de cota usa epoch. `command_rejects_unknown_fields`: campo extra falha.
`golden_preserves_public_fields`: comparação integral de StateEvent, preview, pensamento,
ferramenta, pergunta e respostas; não remover nulos/flags para fazer comparação passar.
Incluir os cenários nomeados das Tasks 6–7, inclusive falhas e mensagens desconhecidas.

```rust
#[test]
fn request_ids_keep_type() {
    use hangar_server::runtime::protocol::RequestId;
    let number: RequestId = serde_json::from_str("1").unwrap();
    let string: RequestId = serde_json::from_str("\"1\"").unwrap();
    assert_ne!(number, string);
    assert!(serde_json::from_str::<RequestId>("true").is_err());
}
```

- [ ] **Step 2: Rodar teste vermelho e gerar oráculo, quando solicitado**

Na raiz: `uv run --directory backend python tests/gen_runtime_golden.py`; depois
`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_protocol`.
Esperado inicialmente: falta do módulo/tipos; gerador produz apenas dados sintéticos da base.

- [ ] **Step 3: Definir tipos e gerador com as interfaces fixadas**

Constantes `MAX_FRAME=16*1024*1024`, `MAX_ENVELOPE=2*MAX_FRAME+1024`; serde sem campos extras
em comando/binding. Validar type/versao do snapshot antes de desserializar. Núcleos puros não
recebem relógio do sistema nem abrem arquivos/socket. No gerador, falsificar fronteiras de IO
sem substituir o reducer que serve de oráculo.

- [ ] **Step 4: Conferir contratos e registrar diferença esperada**

Repetir o comando focado autorizado. Diferenciar explicitamente melhoria conservadora de
entrega `Unknown`, readiness por ACK e confirmação por ocorrência; não congelar bug Legacy
como requisito. O restante do contrato público deve coincidir com a base.

- [ ] **Step 5: Commit seletivo da Task**

Stagear somente os caminhos desta Task, com `git add` explícito;
`git commit -m "test(runtime): define headless migration contracts"`.

### Task 2: Cano v2 com leitura de snapshot sem transferência e ACK do filho

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `crates/hangar-server/src/runtime/cano.rs`;
modificar `crates/hangar-cano/src/{main,protocol}.rs`,
`backend/app/adapters/claude_headless/{cano,adapter}.py`,
`backend/app/adapters/codex/{sem_terminal,appserver}.py`;
criar `crates/hangar-cano/tests/cano_v2.rs`,
`crates/hangar-server/tests/runtime_cano.rs`, `backend/tests/test_cano_v2.py`.

**Interfaces:** `peek(binding: &CanoBinding) -> Result<CanoSnapshot, RuntimeError>`;
`connect(binding: &CanoBinding) -> Result<CanoConnection, RuntimeError>`;
`CanoConnection::start(generation: u64, capacity: usize) -> IoTasks` oferece writer limitado,
eventos `Line(Value)`, `WriteAck{operation_id,outcome}`, `End{code}` e handles de reader/writer.
`IoTasks::stop(self)` fecha e junta tarefas, sem matar o filho. Snapshot segue a seção comum.

- [ ] **Step 1: Escrever testes do transporte e dos dois trackers**

`peek_keeps_old_writer`: conexão antiga continua escrevendo e recebendo após peek.
`ack_requires_child_flush`: socket flush sozinho não produz Written; stdin falso falha antes
do write → NotWritten, write parcial → Unknown. `partial_eof_not_forwarded`: zero bytes no filho.
`pending_ids_keep_type`: pedidos 1 e "1" sobrevivem juntos. `cli_cannot_forge_private_ack`:
CLI com type cano_input_ack chega como Line, sem concluir operação do ator.
`snapshot_keeps_full_prefix_and_parent`: texto/Unicode/thinking/tool parcial e Codex por thread
reaparecem; evento result do subagente não fecha o pai. `oversize_keeps_old_connection` recusa
snapshot sem derrubar escritor e não perde pending. Cobrir Unix e TCP, token incorreto e v1.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-cano --test cano_v2` e
`uv run --directory backend pytest tests/test_cano_v2.py`.
Esperado: peek/ACK/snapshot v2 ausentes.

- [ ] **Step 3: Implementar protocolo v2 nos dois canos**

Subir VERSION/VERSAO juntos para 2. Token aleatório obrigatório em Unix e TCP; Unix 0600.
`<token>\n` reivindica writer; `peek <token>\n` serializa snapshot, responde e fecha sem swap.
Validar/serializar snapshot **antes** de substituir cliente. Envelope input contém linha JSON
em `frame`; só encaminhar linha completa e válida. Depois de stdin write+flush, atualizar
tracker e responder ACK. IO parcial/falha após início gera Unknown. CLI output vem em
`cano_output.frame`; ACK é origem do cano. Legacy v2 desembrulha; v1 continua raw.

Tracker Claude: `inflight.claude={complete,text,thinking,tool:{name,input,index}}`.
Codex: `inflight.codex[threadId]={complete,text,itemId,turnId}`.
Usar String.push_str/StringIO; serializar prefixo somente no snapshot. Se prefixo exceder
orçamento, marcar incomplete sem publicar sufixo; se pending não couber, recusar takeover.
Excluir stream/result de subagente da visão do pai, preservando seus controles relevantes.

- [ ] **Step 4: Implementar reader/writer Rust e compatibilidade Legacy**

Scanner por blocos `fill_buf/consume`, sem await por byte. Limites distintos por envelope e
frame. Writer usa wire-id único; ACK do cano tem prazo de 30 s. Nunca considerar flush do
socket Written. Reader separa envelopes, captura stderr/saída e rejeita snapshot incompatível.
Remover a atualização automática de cano v1 ocioso no `_spawn`; elegibilidade vem da versão.

- [ ] **Step 5: Conferir e commitar os caminhos da Task**

Repetir apenas testes focados autorizados; incluir `runtime_cano`.
Commit seletivo: `feat(cano): add authenticated snapshot peek and child acknowledgements`.

### Task 3: Fila autoritativa, diário de operações e projeção JSONL

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/queue.rs`, `backend/app/runtime_queue.py`,
`crates/hangar-server/tests/runtime_queue.rs`, `backend/tests/test_runtime_queue.py`;
modificar `backend/app/pqueue.py` e, para reuso interno, visibilidade das funções existentes
`strip_attach` e `chaves_de_commit` em `crates/hangar-server/src/transcript/history.rs`.

**Interfaces:** `Store::open(state_path, projection_dir, initial) -> io::Result<Store>`;
`Store::exec(generation: u64, call_id: &str, clock: ClockSample, action: Action) -> io::Result<Value>`.
`QueueActor::start(store, lease: Arc<File>)`; `exec(...).await`; `shutdown(self).await`.
Python `QueueCoordinator.queue_gate(name)`, `queue_rpc(route,call_id,clock,action)` e
`commit_python_state(route,state)` são fornecidos pela Task 5. `PromptQueue` mantém suas
assinaturas existentes e delega a elas antes de qualquer escrita.

`State={version,owner_key,generation,name,rows,operations,used_occurrences,runtime_state}`.
`Operation={id,payload,entry_id,status,result,dispatch_cursor,wire_attempts}`.
Status: Prepared, Dispatching, Accepted, Deferred, Rejected, Unknown, Confirmed.
`Action`: Load, Append, AppendLocal, Claim, SetDelivered, Abandon, BumpAttempts,
EntryDelivered, Confirm, Prune, Reconcile, Remove, Clear, Rename, Prepare, BindDispatch,
BeginDispatch, Finish, ConfirmOccurrence, LateRpcResolution, Recover, EnsureProjection,
SetRuntimeState. Campos correspondem às assinaturas existentes de PromptQueue; Prepare
recebe id/payload/entry_id; Finish recebe id/status/result; BindDispatch recebe id/cursor.

- [ ] **Step 1: Escrever testes de persistência e idempotência**

`same_operation_does_not_append_twice`: mesma ID/payload retorna recibo; payload diferente
com mesma ID falha. `state_before_projection`: crash entre os dois conserva uma entrada;
retry mesma ID repara projeção sem append novo. `fsync_after_rename_fences_store`: falha após
substituição bloqueia novas mutações até reler estado comprometido; nunca restaura cópia velha.
`corrupt_state_is_not_empty_queue`: erro
explícito, sem importar JSONL por cima. `unknown_never_unclaims`: Unknown permanece delivered
e não confirmado. `cap_keeps_pending`: CAP 1000 poda apenas confirmados/saídas locais;
desistiu sem confirmação e Unknown são conservados. Cheio dessas entradas recusa input novo.
`rename_keeps_key`: estado/lock iguais e projeção movida uma vez. `late_reply_matches_generation`:
reply de outra geração/tipo não conclui. `unknown_reply_cannot_downgrade_final`: Accepted e
Confirmed permanecem definitivos. Testar paridade dos métodos da fachada em ambos os donos.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_queue`;
`uv run --directory backend pytest tests/test_runtime_queue.py`.

- [ ] **Step 3: Implementar transação e journal antes do IO**

Estado privado `<key>.queue-state.json`; `<name>.jsonl` é projeção compatível. Ordem obrigatória:
validar geração/ID → mutar cópia → gravar/fsync temporário exclusivo → substituir estado →
gravar/fsync/substituir projeção → responder. Estado persistido não é revertido porque projeção
falhou. Na próxima mesma operação, reparar e retornar recibo; não recalcular a mutação.
Não engolir falhas de leitura/JSON/rename/fsync. Se rename ocorreu e fsync falhou, bloquear novas
mutações, reler o estado autoritativo e reparar; não repor a cópia antiga em memória e continuar.
Reutilizar atomicidade existente e tratamento
Windows; nunca usar `.tmp` único compartilhado por todos os escritores.

Prepare persiste intenção; capturar cursor e BeginDispatch antes do efeito. Cada fase composta
tem wire-id próprio e parent-id estável. Recover converte Dispatching sem prova em Unknown.
Unknown só muda por prova forte/recibo definitivo correspondente; não volta a Deferred.
QueueActor usa mutex por chave e `spawn_blocking` finito por transação; nenhum worker permanente
`blocking_recv` por sessão. Conservar lease até concluir tarefas de persistência.

- [ ] **Step 4: Implementar a fachada completa e a reserva no mesmo formato**

Encaminhar append, append_saida_local, claim_undelivered, set_delivered, desistir, bump_attempts,
entry_delivered, confirm_delivered, prune_before, reconcile_delivered, remove, clear, rename,
load e `_write_atomic`. Follow continua leitura de projeção. Em Rust, nenhuma escrita Python;
na reserva gerenciada, carregar rows autoritativas, aplicar semântica Legacy em memória e
commit estado+projeção sob a lease existente. Chave nunca gerenciada conserva JSONL Legacy.
Reconcile Legacy pode confirmar entradas comuns; Unknown exige ConfirmOccurrence da Task 4.

- [ ] **Step 5: Conferir e commitar fila como unidade**

Repetir testes focados autorizados. Commit seletivo:
`feat(runtime): persist queue operations with compatible projections`.

### Task 4: Confirmar entrega por ocorrência da conversa atual

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/receipt.rs`, `backend/app/runtime_receipt.py`,
`crates/hangar-server/tests/runtime_receipt.rs`, `backend/tests/test_runtime_receipt.py`;
modificar `runtime/queue.rs`, `backend/app/runtime_queue.py` e
`backend/tests/test_runtime_queue.py`. Reutilizar parsers da parte 1.

**Interfaces:** `ReceiptIndex::new(provider, conversation)`;
`capture(path: &Path) -> io::Result<DispatchCursor>`;
`scan(path: &Path) -> io::Result<Vec<Occurrence>>`;
`match_after(cursor, row, used_occurrences) -> Option<ReceiptProof>`.
Python `ReceiptIndex` em runtime_receipt.py tem as mesmas operações e fixtures para a reserva
sem processo Rust; o coordenador fornece binding e gate. Cursor guarda conversa, identidade do arquivo, offset e anchor; ocorrência guarda identificador
durável, posição, texto normalizado, tipo e timestamp opcional. Prova entra por
`Action::ConfirmOccurrence { id, proof: ReceiptProof }`, com conversa, identidade/posição da
ocorrência, cursor e texto normalizado; texto/timestamp isolados não constituem prova.
JSON do cursor entra por `Action::BindDispatch`.

- [ ] **Step 1: Escrever provas de ausência e duplicidade**

`one_echo_confirms_only_one_identical_prompt`: dois textos iguais, um echo; confirmed=1,
repetir scan/confirm e reiniciar índice continua confirmed=1. `cursor_is_dispatch_not_enqueue`:
segunda mensagem enfileirada durante a primeira exige echo posterior ao seu próprio despacho.
`missing_or_partial_transcript_is_no_proof`: não confirma, não libera retry. `rewritten_file`
e `old_conversation`: nenhum echo velho vale. `enqueue_is_not_consumption`: queue-operation/enqueue
sozinho mantém pendente; dequeue/user correspondente pode provar.
`steer_attachment_is_delivery`: attachment/queued_command posterior ao cursor é prova de
aterrissagem do steer, conforme parser atual; não confundir com queue-operation/enqueue. `attachments_and_unicode`:
preserva as chaves existentes, CRLF/contrabarras Windows e Unicode. Sem timestamp só vale
offset posterior comprovado no mesmo binding/arquivo; caso ambíguo fica Unknown.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_receipt`;
`uv run --directory backend pytest tests/test_runtime_receipt.py`.

- [ ] **Step 3: Implementar cursor e consumo único de provas**

Capturar imediatamente antes de BeginDispatch, não quando append aceita a fila. Ler somente
linhas completas. Mudança de inode/identidade/anchor invalida prova baseada só em offset;
ausência/erro não comprova perda. Usar IDs do provedor quando disponíveis, ou combinação de
conversa/arquivo/posição; registrar occurrence usada na mesma transação da confirmação.
Não usar conjunto de texto do histórico inteiro como prova de Unknown. Não reconciliar duas
entradas com a mesma ocorrência nem eliminar o registro de uso enquanto puder haver ambiguidade.
Aplicar o mesmo algoritmo em runtime_receipt.py usando os parsers existentes; a reserva não
substitui essa prova pelo conjunto de texto do histórico. Fixtures iguais conferem os dois lados.

- [ ] **Step 4: Conferir e commitar as provas de entrega**

Repetir o teste focado autorizado e os casos afetados da fila.
Commit seletivo: `fix(runtime): confirm queued input using distinct transcript occurrences`.

### Task 5: Coordenador de posse, transferência e reserva Python

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `backend/app/runtime_coordinator.py`,
`backend/tests/test_runtime_ownership.py`, `crates/hangar-server/tests/runtime_lease.rs`;
modificar `backend/app/runtime_queue.py`, `runtime/queue.rs`.

**Interfaces:** `WriterLease(path).close()`; `Binding.descriptor()` com
name/key/provider/headless/meta/jsonl/projection_dir/state_path/lock_path/generation;
`RuntimeCoordinator.register(binding)`, `managed_runtime(name)`, `managed_queue(name)`,
`legacy_allowed(key,generation)`, `queue_gate`, `queue_rpc`, `commit_python_state`;
async `adopt(name)`, `detach(name)`, `recover(name, confirmed_dead: bool)`,
`freeze(name)` como contexto de lifecycle e `op(name, command, operation_id)`.
O gateway da Task 8 converte descriptor explicitamente para RuntimeTarget.

- [ ] **Step 1: Escrever testes de exclusão e recuperação**

`two_processes_one_lease`: Rust e Python em processos falsos separados, um só adquire.
`failed_peek_keeps_python`: lease/cliente atuais intactos. `quiesce_precedes_release`:
reader, warm/religador, drain e controle encerram antes de release. `timeout_is_not_fallback`:
Rust vivo não chama Legacy. `detach_and_lock_before_reserve`: resposta sozinha não basta.
`recover_dispatch_as_unknown`: morte após write mantém entrada sem resend.
`managed_queue_survives_terminal_mode`: fila não volta para JSONL-only.
`late_ready_rejected`: instance/key/generation antigos não liberam operações.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`uv run --directory backend pytest tests/test_runtime_ownership.py`;
`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_lease`.

- [ ] **Step 3: Implementar lease e fases do coordenador**

Fases Python/PreparingRust/Rust/RecoveringPython. Lockfile privado por meta.key, nunca
substituído/deletado/renomeado. Rust `File::try_lock`, Python flock/locking; abrir read+write.
Python reutiliza sua lease nas transações, sem abrir outra FD para relock. Prova Windows usa
processos separados; documentar indisponibilidade se ambiente não estiver acessível.
ManagedRuntime exige headless e key válida; ManagedQueue exige key já cadastrada, inclusive
reserva/terminal. Fase intermediária recusa mutação antes de aceitar mensagem.

- [ ] **Step 4: Implementar transferência e recuperação na ordem fixada**

Adopt: peek v2 válido → bloquear novas mutações → aguardar/finalizar operações Legacy →
desarmar reconectores → fechar/juntar reader/writer → liberar lease → claim Rust/snapshot fresco/
importação/projeção/hydrate → ready da mesma vida. Não matar cano. `carry` transporta
metadados de correlação de controles/intenções e o último estado privado; nunca Futures/tasks
Python. Operações em voo terminam ou ficam Unknown no diário antes de release. Pendência de
aprovação do servidor pode atravessar a transferência; controle cliente incerto não é repetido. Falha após release requer
detach Rust confirmado e aquisição da lease antes de restaurar Legacy.
Recover: confirmar morte do processo → adquirir lease → ler diário → Dispatching vira Unknown →
reparar projeção → abrir um cliente Legacy no mesmo cano → hydrate → liberar novos comandos.
Se Rust continua vivo e indisponível, manter bloqueio e expor erro. Não iniciar segunda CLI.

- [ ] **Step 5: Conferir e commitar coordenação**

Repetir os testes focados autorizados. Commit seletivo:
`feat(runtime): coordinate exclusive ownership and safe Python recovery`.

### Task 6: Núcleo Claude nativo, incluindo perguntas e esforço diferido

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/claude.rs`, `crates/hangar-server/tests/runtime_claude.rs`;
modificar fixtures Claude e `runtime_contract.rs` da Task 1.
**Interfaces:** `ClaudeEngine::new(metadata: Value,generation: u64,clock: ClockSample)`;
`hydrate(snapshot: CanoSnapshot)`, `start_initialize(operation_id: String)`,
`apply(input: EngineInput,clock)`, `command(command: RuntimeCommand,clock)` retornam
`Result<Vec<Effect>,RuntimeError>`; `view() -> Value`, `next_deadline() -> Option<f64>`.
View contém public_state e campos privados alive/iniciando/in_progress/pending/question/model/
effort/permission_mode/previous_non_plan/commands/terminal_commands; não serializar estes últimos
como novo formato público. Políticas da Task 9 e fila da Task 3 são consumidas por Effect.

- [ ] **Step 1: Escrever testes do reducer e controles**

`init_timeout_not_deliverable`: timeout não equivale a ACK; ACK tardio libera a mesma vida.
`pending_plan_rules`: Read em bypass pode autoaprovar; ExitPlanMode/Edit/Write/MultiEdit/
NotebookEdit nunca aprovados sozinhos. `question_validates_current_request`: multiSelect,
texto e conversar, ID velho rejeitado. `interrupt_denies_all_pending_first`: ordem dos efeitos.
`subagent_does_not_close_parent`; `usage_last_call_is_context`: total não vira janela de contexto
e zero não apaga uso válido. `first_150ms_last_and_clear`: exatamente o texto da 2A, callback
antigo não ressuscita prefixo. `unknown_control_is_neutral`: success vazio + nota privada.
`effort_is_journaled_before_write`: intenção pai e tentativa filho `${parent}:effort` persistem;
esforço ocupado aguarda fronteira segura; só confirmação local correspondente aplica valor.
`late_ack_does_not_override_result`: Accepted anterior permanece definitivo.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_claude`.

- [ ] **Step 3: Portar reducer e buffers, mantendo as regras da base**

Portar `_on_event`, `_on_system`, `_on_stream`, `_on_control_request`, `_aplicar_uso`,
`_recalcular_estado` e estado da `_Sessao`; ler a seção Regras vigentes de harnesses antes.
Buffer String por canal/bloco; materializar prefixo/JSON parcial só ao publicar. Preservar
schema StateEvent/PreviewEvent, ferramenta.text JSON e limpeza por geração. Snapshot incomplete
suprime sufixo até fronteira segura. Cota/formatação são dados de política; estado é decisão Rust.

- [ ] **Step 4: Portar controles, readiness e esforço com recibos separados**

Encode initialize, permissions, select/answer, interrupt, modelo/effort, comandos/compact.
Preservar ID tipado e ordem das pendências; plan restaura base anterior; default vira manual.
Timeouts atuais: initialize 60 s, disponibilidade 180 s, controle 15 s; waiter initialize
tardio continua rastreado. Nenhum `finally` torna initialized=true sem prova.
Persistir EffortIntent no runtime_state; antes do write diferido, Prepare da tentativa filha.
Após restart, falta de ACK/confirm local mantém Unknown, sem reaplicar por adivinhação.
`next_deadline` inclui publicação, controle e label; não usar tick global de 150 ms que atrasa
publicação até 300 ms. Result interrupt não vira erro de turno; `/clear` atualiza sid e reset.

- [ ] **Step 5: Comparar oráculo e commitar núcleo Claude**

Quando solicitado, executar `runtime_contract` e o teste focado; comparar todas as saídas,
com diferenças conservadoras registradas na Task 1.
Commit seletivo: `feat(runtime): port Claude headless state and controls to Rust`.

### Task 7: Núcleo Codex nativo, incluindo RPC e perguntas assíncronas

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/codex.rs`, `crates/hangar-server/tests/runtime_codex.rs`;
modificar fixtures Codex e `runtime_contract.rs`.
**Interfaces:** `codex::Engine::new(metadata,generation,clock)`, `hydrate(snapshot)`,
`bootstrap(reconnect: bool,operation_id: String)`, `apply`, `command`, `next_deadline` como Claude.
`view() -> Value` contém StateEvent público; `control_view() -> Value` contém campos privados.
O ator normaliza ambos em RuntimeView. RPC usa `RequestId`, Effect e diário comuns.

- [ ] **Step 1: Escrever testes do protocolo Codex**

`initialize_then_resume`: initialize→initialized→thread/resume ou start; Already initialized
aceito apenas no contexto correspondente. `reply_ids_and_generations`: números e strings
distintos, resposta antiga não conclui controle novo. `rpc_timeout_keeps_pending`: Unknown
uma vez, reply tardio resolve, sem turn/start adicional. `server_request_once`: aprovação e
pergunta respondidas só no request corrente; método desconhecido recebe -32601.
`question_hydrate_merges_notifications`: revisão protege notification nova durante leitura;
echo local/título igual não consome pergunta. `turn_end_idle_before_drain`: ordem explícita.
`preview_full_prefix_on_takeover` e `thread_switch_clears_old_preview`.
`sandbox_requires_idle` e `terminal_adapter_untouched`.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_codex`.

- [ ] **Step 3: Portar reducer, bootstrap e correlação de RPC**

Portar consumo headless de appserver/adapter/async_questions, sem usar WebSocket ou tmux.
IDs cliente `hangar:<generation>:<counter>`; mapas distintos para pedidos do servidor e RPC
cliente. Bootstrap em etapas por efeitos; nunca aguardar reply dentro de command/apply.
Restaurar turn/thread/pending e prefixo por snapshot; só evento da thread atual altera estado.
Timers monotônicos; rate-limit/reset usa epoch. Buffers adotam cadência da 2A.

- [ ] **Step 4: Portar catálogo de controles e guardas de lifecycle**

turn/start, steer, interrupt, compact, models/list, skills/list, rateLimits/read, settings/read,
modelo/effort/modo, approvals e questions. Preservar `thread/settings/update` usado pela base;
conferir schema/documentação da versão instalada se houver dúvida, sem atualizar CLI.
Sandbox/restart/open_terminal passam por barreira e prova idle, sem pending/write/RPC ativo.
Mudança sandbox reinicia cliente sob a mesma posse, junta tarefas anteriores e conserva contador
RPC; não é licença para outra CLI concorrente. Dados de skills/formatos administrativos podem
vir da política Python, evitando dependência nova só para reproduzir hash de nome.

- [ ] **Step 5: Comparar oráculo e commitar núcleo Codex**

Quando solicitado, executar testes focados e `runtime_contract`.
Commit seletivo: `feat(runtime): port Codex headless RPC and questions to Rust`.

### Task 8: Ator Rust e gateway privado no processo existente

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `runtime/actor.rs`, `runtime/gateway.rs`,
`crates/hangar-server/tests/runtime_actor.rs` e `runtime_gateway.rs`;
modificar `runtime/mod.rs`, `crates/hangar-server/src/{lib,config}.rs` e
`backend/app/rust_server.py`, `backend/tests/test_rust_server.py`.

**Interfaces:** `RuntimeActor::spawn(target,queue,connection,engine) -> RuntimeHandle`;
handle async `command`, `drain`, `confirm`, `snapshot`, `ensure_projection`, `stop`.
`RuntimeRegistry::adopt(target,carry)`, `detach(key,generation)` e `subscribe()`;
`gateway::serve(listener,registry,secret,instance,protocol)` implementa o contrato comum.
Supervisor `configure_runtime(ready,secret,instance)` e `deactivate_runtime(confirmed_dead)`
configuram o coordenador da Task 5 e preservam configure/deactivate da 2C.

- [ ] **Step 1: Escrever testes de concorrência e API privada**

`blocked_rpc_does_not_block_interrupt_or_state`; `prepare_before_every_write`;
`wire_ids_distinguish_compound_control`; `cli_reply_before_ack_is_final`;
`concurrent_same_id_has_one_dispatch`; `late_wire_reply_resolves_parent`;
`unknown_preparation_never_starts_new_mutable_phase`;
`ack_timeout_is_unknown_without_resend`; `queue_failure_prevents_write`;
`stop_joins_io_and_persistence_before_unlock`; `one_sse_reader_multiple_keys`;
`wrong_secret_instance_or_generation_denied`; `private_port_loopback_only`;
`startup_one_line_no_secret`; `terminal_and_headless_sources_do_not_overlap`.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_actor --test runtime_gateway`;
`uv run --directory backend pytest tests/test_rust_server.py`.

- [ ] **Step 3: Implementar mailbox e executor de efeitos**

Mailboxes limitadas; reader/writer e persistência separados. Processar evento/controle/timer
enquanto RPC aguarda. O ator amostra seu próprio Instant/SystemTime para ClockSample; o clock
recebido na ponte não define deadlines locais. Reserva usa seu próprio relógio. Não comparar
monotônico persistido em processos diferentes; conservar apenas estado/prova, reconstruindo
prazos locais na recuperação. Antes de cada Effect::Write/serviço com IO: Prepare da fase, cursor,
BeginDispatch durável, então envio. Correlacionar wire-id com parent-id/request-id/geração.
Reply CLI pode resolver antes do ACK; manter recibo definitivo. Reply tardio atualiza a fase
e resolve a operação agregada quando todas as fases necessárias têm prova; não deixar o pai
Unknown depois de concluir a fase final. Validar ID tipado, geração, método e conversa.
Se uma fase preparatória ficou Unknown, uma resposta tardia pode resolver a leitura, mas não
iniciar nova fase mutável de uma operação já encerrada por timeout; requer nova ação explícita.
Reservar ID de operação na mailbox antes de aguardar persistência, evitando dois Prepare
concorrentes da mesma ID. Falha de persistência impede
IO ou retorno de sucesso. WakeQueue busca/claim por autoridade, não recursão Legacy.
Políticas retornam por mensagem ao ator; Apply nunca espera chamada de rede. Stop cancela
operações de rede, junta tarefas finitas de persistência e reader/writer, só então libera lease.

- [ ] **Step 4: Implementar gateway e descoberta da porta privada**

Mesmo processo Rust abre 127.0.0.1:0. Emite uma única linha stdout
`{type:"runtime_ready",protocol,instance,port}`; stdout não recebe outro conteúdo.
Python `_spawn` usa stdout PIPE; lê até 4096 bytes em até 10 s, confere nonce novo
`HANGAR_RUNTIME_INSTANCE`, protocolo e porta. Segredo interno já existente fica só no ambiente
do filho; endereço/instance privados da 2B não entram em health. Preservar `terminal_address`
da 2C na saúde e suas guardas de acesso. Parent stdin EOF continua encerrando
Rust. Recusa startup inválido; antes de reserva, confirmar filho encerrado.
Autenticar loopback+segredo+instance antes de ler corpo; limitar JSON. Agregar eventos em um SSE
com snapshot/revisão/heartbeat; sem publicar queued/local/confirmed (follow Python já faz isso).
Config/lib/AppState recebem alterações pontuais que preservam a 2C. Subir protocolo conjunto.

- [ ] **Step 5: Conferir e commitar ator/gateway**

Repetir checks focados autorizados. Commit seletivo incluindo constantes Python/Rust no mesmo
commit: `feat(runtime): host exclusive headless actors behind private IPC`.

### Task 9: Serviços administrativos e fachadas sem leitor Python concorrente

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** criar `backend/app/runtime_policy.py`, `backend/app/runtime_adapter.py`,
`backend/tests/test_runtime_policy.py`, `backend/tests/test_runtime_adapter.py`;
modificar `backend/app/internal_api.py`, `backend/app/adapters/claude_headless/adapter.py`,
`backend/app/adapters/codex/{adapter,appserver}.py`, `backend/app/runtime_coordinator.py`.

**Interfaces:** `runtime_policy.run(kind: str,payload: dict,metadata: dict) -> dict`;
rota privada `/internal/runtime/policy` valida responsável/key/generation/request_id antes de
executar no pool; resposta `{ok:true,data}` ou `{ok:false,error_type}` sem conteúdo em log.
`RuntimeAdapter` conserva assinaturas públicas dos adapters e encaminha comandos ao coordenador;
snapshot/escolhas/comandos/problema são cache RuntimeView; state_monitor assina condição do cache.
`LegacyIO.prepare_wire(operation_id,phase,payload)` e `finish_wire(...,outcome)` usam o mesmo
journal na reserva gerenciada, antes/depois de write/RPC/UDS.

- [ ] **Step 1: Escrever testes dos limites da fachada e serviços**

`owned_runtime_never_opens_legacy_reader`: falhar o teste se ensure_running/conectar/_write/
_ctrl/_on_event forem chamados. `sync_endpoint_uses_server_loop`: sem deadlock do event loop.
`policy_failure_is_visible_and_redacted`: nenhum raw input/segredo em diário exportado.
`native_message_has_journal_before_uds`: falha incerta não tenta cano depois.
`legacy_reserve_journals_controls`: envio na reserva tem Prepare/BeginDispatch e Unknown.
`getter_uses_same_generation`: picker/comandos não leem `_sessions` como autoridade.
`invalid_event_or_gap_requests_snapshot`: estado bom preservado, comandos bloqueados até
reposição válida. Getter/close_sync/rename não devolvem None genérico para esconder falha;
encaminham ou expõem indisponibilidade conforme seu contrato.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`uv run --directory backend pytest tests/test_runtime_policy.py tests/test_runtime_adapter.py`.

- [ ] **Step 3: Implementar catálogo estreito de serviços**

Claude: prepare_prompt, answer_body, format_status, local_output, last_usage, reload_stamp,
quota, native_message, unknown_private. Compartilhados: session.patch_meta, session.marker,
diag.error. Codex: preparação de imagens/input e catálogo/formatos de skills quando usados.
Reutilizar helpers puros atuais; patch aceita só campos de sidecar previstos, com binding
revalidado. Serviços não chamam cliente/reducer/PromptQueue; notas locais, confirmações de slash
e receipts são ações nativas da fila. Quota retorna model_dump, sem inventar estrutura.

UDS é serviço administrativo autorizado pelo ator após journal; msg_id deriva de key+operationId.
Nenhum retry de POST/UDS mutável. Exceção de envio sem prova do ponto de falha retorna Unknown.
Recibo posterior correlaciona msg_id/opId e atualiza o journal uma vez, sem gravar fila por fora.
Unknown raw da CLI vai só ao log privado limitado existente, nunca ao diário exportável.
Env/argv/preparação de conta/lançamento continuam em helpers existentes; não usar _Sessao com
reader oculto para facilitar a migração. Subir o protocolo interno nos dois lados.

- [ ] **Step 4: Implementar fachadas, cache e hooks da reserva**

Um leitor SSE privado por instância Rust distribui eventos a slots com condição/cache;
rejeitar instance/geração/revisão antigos. Evento inválido ou lacuna de revisão marca cache
indisponível para mutações e pede snapshot fresco; não apagar o último estado bom da interface.
Campos privados da view suportam getters/lifecycle;
não expor como evento público novo. Endpoints sync usam loop_servidor sem bloquear seu próprio
loop. Em posse Rust, startup/warm/reconnect/parking/drain Legacy estão desarmados.
Na reserva gerenciada, instrumentar os pontos finais `_write`/request RPC/UDS: journal antes
de IO, resultado depois; exception após write não se disfarça de Deferred/NotWritten.

- [ ] **Step 5: Conferir e commitar fachadas/serviços**

Repetir testes focados autorizados. Commit seletivo:
`refactor(runtime): delegate headless adapters without duplicate CLI readers`.

### Task 10: Encaminhar todos os produtores de input, controle e confirmação

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** modificar `backend/app/{api,registry,sse,stall_watch}.py`, `pqueue.py`,
`runtime_adapter.py`, `runtime_coordinator.py`; criar `backend/tests/test_runtime_routing.py`.

**Interfaces:** `RuntimeCoordinator.op(name,command,operation_id)` usa o gateway;
`submit` usa UUID da entrada como operation_id. Métodos externos conservam contratos de
resposta existentes; Deferred só significa fila persistida, Unknown mantém pendência visível.
ManagedRuntime e ManagedQueue são decisões separadas da Task 5.

- [ ] **Step 1: Escrever teste parametrizado de todos os caminhos**

Parametrizar POST input/steer, recado grupo/par/peer/convidado, hook, _drenar/_drain_session,
_confirm_and_drain/_confirm_codex_queue, _maybe_chain, loop/bastão, nota local e receipt UDS.
Cada caso headless Rust: exatamente uma operação nativa e zero append/claim/write/UDS Legacy.
Casos terminal/outros providers seguem caminho atual. `post_hook_and_turn_end_once` dispara
fontes simultâneas e confirma um despacho. `unknown_not_marked_unsent` conserva fila após erro.

- [ ] **Step 2: Rodar teste vermelho, quando solicitado**

`uv run --directory backend pytest tests/test_runtime_routing.py`.

- [ ] **Step 3: Encaminhar antes de qualquer efeito Legacy**

API `_enviar`, `_send_one_headless`, `_send_one_codex`, input/steer/select/answer/interrupt,
modelo/effort/permissão/compact e getters usam fachada. Timers/hooks/SSE/fim de turno enviam
drain/confirm ao mesmo ator; appends de encadeamento/loop passam por PromptQueue gerenciada.
_ao_recibo_nativo deixa de decidir entrega/gravar fila fora do journal. Em API rename direto,
substituir atomico sobre caminhos da fila por `PromptQueue(name).rename(new)`.
Preservar guest/auth/contexto do remetente e mensagens de erro; sem novo endpoint público.

- [ ] **Step 4: Conferir ausência de atalhos e commitar roteamento**

Busca por `_sessions`, `_write_atomic`, `_append_lock`, `PromptQueue`, `conectar_cano`,
`send_prompt`, `steer`, `_drenar` nos arquivos da Task: cada acesso de escrita headless tem
gate/fachada, cada getter usa RuntimeView, e toda exceção é justificada no teste.
Repetir teste focado autorizado. Commit seletivo:
`refactor(runtime): route all headless producers through the active owner`.

### Task 11: Lifecycle e fonte pública, incluindo histórico e integração com a 2C

Status: ready-for-agent (aguarda aprovação)
Risk: high
**Files:** modificar `backend/app/{api,registry,sse,internal_api,rust_server}.py`,
`runtime_coordinator.py`, adapters afetados e `crates/hangar-server/src/{lib,routes,config}.rs`;
criar `backend/tests/test_runtime_lifecycle.py`,
`crates/hangar-server/tests/runtime_side_events.rs`. Preservar arquivos/campos da 2C integrados.

**Interfaces:** freeze da Task 5 é barreira para clear/rename/kill/reload/conta/modo/thread;
`ensure_projection(key,generation)` é chamado antes de calcular ETag ou servir fila em history.
fonte pública é escolhida uma vez por binding; decoder do side-events mantém evento público
existente. A fonte headless vem de RuntimeView; terminal vem da 2C/Legacy correspondente.

- [ ] **Step 1: Escrever testes das trocas e leituras**

`rename_while_command_waits`: key/lease iguais, nome e projeção novos, callback velho descartado.
`clear_changes_conversation`: cursor/evento anterior não confirma turno novo.
`terminal_switch_keeps_managed_queue`: proprietário Python assume mesmo estado depois do detach.
`stop_failure_keeps_files`: não apagar fila/sidecar/cano após falha de parada.
`history_projection_error_not_304`: projeção falhou → 503, não ETag velho/vazio.
`two_clients_one_source` e `no_sse_runtime_still_drains`: quantidade de viewers não controla ator.
`provider_switch_no_double_state`: reset e seleção de fonte preservam regra existente.
`combined_supervisor_deactivates_b_and_c`: nenhuma perda do observador terminal integrado.

- [ ] **Step 2: Rodar testes vermelhos, quando solicitado**

`uv run --directory backend pytest tests/test_runtime_lifecycle.py`;
`cargo test --manifest-path crates/Cargo.toml -p hangar-server --test runtime_side_events`.

- [ ] **Step 3: Integrar lifecycle com barreira e sem exclusão prematura**

Entrar freeze → barrar produtores → aguardar/juntar operações → detach e lease → mudança
administrativa → novo binding/geração → reabrir no responsável escolhido. Em rename apenas nome
muda, key/lease não. Em clear/thread/mode novo cursor/vida invalida comandos antigos. Não abrir
cliente novo antes de encerrar o anterior; falha mantém arquivos e expõe problema.
Python só inicia CLI por lançamento explícito existente, nunca como reparo silencioso do cano.

- [ ] **Step 4: Integrar history, side-events e contrato conjunto**

Garantir projeção antes de history/ETag; falha explícita 503, sem fallback stale. Follow Python
é fonte única de queued/local/confirmed. Runtime publica estado/prévia/pensamento/ferramenta,
sem segundo reducer no side-events. Preservar reset/__reprovider__, IDs e decoração comum de
loop/background. Conferir diff atual da 2C; preservar campos, routers, terminal pool e
configure/deactivate. Protocol bump para próximo número real em ambos os lados no mesmo commit.

- [ ] **Step 5: Conferir e commitar integração de lifecycle**

Repetir testes focados autorizados. Commit seletivo:
`feat(runtime): integrate headless lifecycle and public streams safely`.

### Task 12: Provar falhas, uso real e limites da migração

Status: ready-for-human (uso real depende do dono)
Risk: high
**Files:** criar `backend/tests/test_runtime_failure_matrix.py`,
`crates/hangar-server/tests/runtime_recovery.rs`;
modificar `docs/decisoes/plataforma.md`, `docs/decisoes/harnesses.md` e `CLAUDE.md`
somente com regras medidas desta entrega; manter registro de execução no próprio plano.
**Interfaces:** fakes/processos isolados das Tasks anteriores; nenhuma ferramenta de teste
inicia serviços ou CLI reais. Produz registro do que foi provado e do que ficou sem conferência.

- [ ] **Step 1: Escrever matriz de falhas e invariantes**

Injetar queda em before_prepare/after_prepare/before_write/partial_write/after_write/
before_reply/between_state_and_projection/after_detach. Para cada ponto afirmar: no máximo
um writer; nenhuma entrada perdida; Unknown não reenvia; resultado não publicado como sucesso
sem persistência; reserva usa mesmo cano e diário. Cobrir sem Rust, desligado, protocolo
incompatível e três quedas em 60 s sem segundo lifespan. Windows: lock interoperável, replace,
CRLF/Unicode e paths; não declarar prova remota se conexão não foi possível.

- [ ] **Step 2: Rodar verificações pedidas pelo dono**

Pedido focado: testes dos arquivos tocados em um comando por ferramenta, repetindo só falhas.
Pedido completo: `npm run check` na raiz, Vitest e pytest completos, além dos testes Cargo das
crates alteradas. Build só para servir o dist local, conforme regra do projeto. Não rodar estes
comandos por simples conclusão de planejamento ou implementação. Registrar comando/resultado.

- [ ] **Step 3: Verificação manual — Claude e Codex sem terminal no canal autorizado**

Com o dono no ambiente acordado: input idle/ocupado, steer/cancel, aprovação, AskUserQuestion,
modelo/effort/modo, slash local, recado, fila e confirmação; dois viewers e nenhum viewer.
Conferir troca de owner, reconexão e prefixo com pergunta em voo usando sessões de prova
autorizadas. Não reiniciar serviço da máquina nem encerrar sessões de trabalho para simular falha.
Conferir uma sessão terminal para provar preservação do contrato junto à 2C.

- [ ] **Step 4: Registrar medidas e limitações, sem concluir além da prova**

Registrar CPU/parede por cenário sintético, tamanho/chunks/cadência/número de publicações,
latência de input/controle, RSS/threads e recuperação; comparar mesmas entradas e base a80.
Não atribuir ganho total a transporte sintético ou final-only. Cano v1 remanescente é reserva
documentada até reabertura, não migração forçada de todas as sessões antigas. Checks/uso real
não autorizados ou ambiente Windows indisponível aparecem como não conferidos.

- [ ] **Step 5: Revisar diff final e registrar estado para o dono**

Conferir cobertura da spec e fontes por modo; nenhuma mudança de Pi/Kimi/omp/orq/lista/quadro/
canvas, nenhuma dependência atualizada nem serviço duplicado. Revisão local do código e checks
do repo, sem MR novo. Commit seletivo dos testes/regras medidos:
`test(runtime): prove recovery and document headless ownership`.
Reportar hash, branch e estado real da árvore após commits. Não publicar/push por este plano.

## Critério de aprovação e término

Esta entrega termina com o plano salvo e revisado documentalmente, aguardando aprovação do
dono; **não autoriza implementação**. Na execução, 2B está pronta quando Claude/Codex headless
usam reducer, cano, controles e fila Rust como unidade, todos os produtores estão encaminhados,
reserva usa a mesma autoridade e exclusão, e as provas solicitadas e uso real estão registrados.
Uma falha pendente ou check não conferido não vira sucesso pela existência de um commit.

Revisão adversarial de arquitetura e conferência independente de paths/símbolos podem ser
solicitadas antes da execução. Não criam uma etapa extra de implementação nem substituem a
aprovação pedida para este plano.
