use super::protocol::{ClockSample, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};

mod batch;
pub(super) const BATCH_TOO_LARGE: &str = "queue_batch_too_large";
pub(super) const BATCH_SIZE_MESSAGE: &str = "texto do lote ultrapassa o limite do transporte";

pub(super) fn queue_problem(state: &mut hangar_api::state::StateEvent, detail: Option<String>) -> bool {
    if state.problema.as_deref().is_some_and(|code| code != BATCH_TOO_LARGE) { return false; }
    let code = detail.as_ref().map(|_| BATCH_TOO_LARGE.to_owned());
    if state.problema == code && state.problema_detalhe == detail { return false; }
    state.problema = code;
    state.problema_detalhe = detail;
    true
}

const CALL_PREFIX: &str = "call::";
pub(crate) const CAP: usize = 1000;
const VERSION: u32 = 2;
/// Janela de chamadas recentes que nunca sai: cobre a repetição da mesma chamada e o ACK atrasado
/// de uma fase. Mesmo valor de `_RECENT_CALLS` em runtime_queue.py.
const RECENT_CALLS: u64 = 256;
const RECEIPT_METADATA: &[&str] = &["native", "message_id", "native_status", "cleanup", "code", "stage", "batch_id",
    "preserve_binding", "queued", "already_confirmed", "disposition", "draft",
    // A repetição da reabertura em bypass só se reconhece por estes campos (`reopen_claude`).
    "reopened", "mode", "current", "previous_non_plan"];
/// Ações cujo resultado é a operação inteira. Mesmo conjunto de `_OPERATION_RECEIPTS` em runtime_queue.py.
const OPERATION_RECEIPTS: &[&str] = &["prepare", "bind_dispatch", "bind_dispatch_if_prepared", "begin_dispatch", "mark_writing", "finish", "late_rpc_resolution"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Prepared, Dispatching, Accepted, Deferred, Rejected, Unknown, Confirmed }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub id: String,
    pub payload: Value,
    pub entry_id: Option<String>,
    pub status: Status,
    pub result: Value,
    pub dispatch_cursor: Value,
    pub wire_attempts: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal_finalized: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub entry_materialized: bool,
    /// Última chamada que tocou a operação; ausente na v1 = antiga.
    #[serde(default)]
    pub seq: u64,
}

impl Operation {
    fn new(id: &str, payload: Value, entry_id: Option<String>) -> Self {
        Self { id:id.into(), payload, entry_id, status:Status::Prepared,
            result:Value::Null, dispatch_cursor:Value::Null, wire_attempts:BTreeMap::new(), seq:0, terminal_finalized:false, entry_materialized:false }
    }
    fn slim(&mut self) {
        self.result = slim(&self.result);
        for attempt in self.wire_attempts.values_mut() {
            if attempt.get("result").is_some() { attempt["result"] = slim(&attempt["result"]); }
        }
    }
    fn group<'a>(&'a self, key: &'a str) -> &'a str { self.payload["logical_id"].as_str().unwrap_or(key) }
    /// A ocorrência ainda pode confirmar esta operação? Na dúvida, sim.
    fn may_confirm(&self, record: &Value) -> bool {
        if self.entry_id.is_none() || self.status == Status::Confirmed { return false; }
        // Preparada ainda pode ligar cursor. Todo bind captura depois de a operação existir, então
        // quem nasce depois da poda nasce com cursor depois da ocorrência.
        if self.status == Status::Prepared { return true; }
        let cursor = &self.dispatch_cursor;
        if !cursor.is_object() { return false; }
        let (Some(offset),Some(cursor_offset)) = (record["offset"].as_i64(),cursor["offset"].as_i64()) else { return true };
        // O recibo compacto não guarda o timestamp; um cursor anterior ao arquivo ainda pode casar.
        if cursor["file_identity"].is_null() && cursor["absent_since"].as_f64().is_some() {
            return cursor["conversation"]==record["conversation"] && offset>=cursor_offset;
        }
        let same_file = cursor["file_identity"] == record["file_identity"] || cursor["file_identity"].is_null() && cursor_offset == 0;
        cursor["conversation"] == record["conversation"] && same_file && offset >= cursor_offset
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u32,
    pub owner_key: String,
    pub generation: u64,
    pub name: String,
    pub rows: Vec<Value>,
    pub operations: BTreeMap<String, Operation>,
    pub used_occurrences: BTreeMap<String, Value>,
    pub runtime_state: Value,
    #[serde(default)]
    pub next_seq: u64,
}

/// Os recibos terminal ainda conferem transporte e limpeza; o conteúdo da resposta sai.
fn slim(result: &Value) -> Value {
    let mut result = result.clone();
    if let Some(payload) = result.as_object_mut().and_then(|object| object.get_mut("payload")) {
        let small:serde_json::Map<String,Value> = payload.as_object().into_iter().flat_map(|body|body.iter())
            .filter(|(key,value)|RECEIPT_METADATA.contains(&key.as_str()) && (value.is_boolean()
                || value.as_str().is_some_and(|text|text.chars().count()<=200)))
            .map(|(key,value)|(key.clone(),value.clone())).collect();
        *payload=if small.is_empty(){Value::Null}else{Value::Object(small)};
    }
    result
}

/// O recibo guarda a ação sem o volume: basta para detectar reuso do mesmo identificador.
fn receipt_payload(mut action: Value) -> Value {
    if action["kind"] == "set_runtime_state" { action["state"] = Value::Null; }
    // A repetição do avanço ainda precisa distinguir outro estado; a vista inteira (comandos incluídos) não.
    else if action["kind"] == "advance_lifecycle" && !action["state"].is_string() { action["state"] = Value::String(fingerprint(&action["state"])); }
    else if action.get("result").is_some() { action["result"] = slim(&action["result"]); }
    action
}

/// sha256 do JSON com as chaves ordenadas, como a igualdade de `Value`, que ignora a ordem.
fn fingerprint(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(entries.into_iter().map(|(key, value)| (key.clone(), sorted(value))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let bytes = serde_json::to_vec(&sorted(value)).expect("Value tem chaves String e números finitos");
    ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

impl State {
    pub fn terminal_write_blocked(&self,conversation:&str)->bool {
        let barrier=&self.runtime_state["terminal_write_barrier"];
        if barrier.is_object() && (barrier["conversation"].is_null() || barrier["conversation"]==conversation) {return true;}
        // A trava gravada é uma só: a incerta de outra conversa também segura a escrita na dela.
        let conversation=Value::from(conversation);
        self.operations.iter().any(|(key,op)|!key.starts_with(CALL_PREFIX) && holds_terminal_write(op,&conversation))
    }
    /// Linha mais antiga que a drenagem ainda entregaria (mesmo filtro do `Claim`): a nova espera atrás dela.
    pub fn pending_ahead(&self,min_ts:f64)->bool {
        self.rows.iter().any(|r|r["delivered"]==false && current(r,min_ts) && !self.protected(row_id(r)))
    }
    pub fn new(key: &str, generation: u64, name: &str, rows: Vec<Value>) -> Self {
        Self { version:VERSION, owner_key:key.into(), generation, name:name.into(), rows,
            operations:BTreeMap::new(), used_occurrences:BTreeMap::new(), runtime_state:json!({}), next_seq:1 }
    }

    /// Lê v1 ou v2 já podado; devolve se precisa regravar (veio da v1 ou confirmou comando
    /// local antigo). Sem ordem gravada, tudo o que veio da v1 conta como antigo: fora da janela já na leitura.
    pub fn load(bytes: &[u8]) -> io::Result<(Self, bool)> {
        let mut state: State = serde_json::from_slice(bytes).map_err(|_| invalid("estado da fila inválido"))?;
        let mut migrated = state.version == 1;
        if migrated { state.version = VERSION; state.next_seq = RECENT_CALLS + 1; }
        if state.version != VERSION || state.next_seq == 0 || !state.rows.iter().all(Value::is_object)
            || !state.runtime_state.is_object() { return Err(invalid("estado da fila incompatível")); }
        // Recibo gravado antes do corte encolhe já na leitura; o arquivo emagrece na próxima gravação.
        for (_, receipt) in state.operations.iter_mut().filter(|(key, _)| key.starts_with(CALL_PREFIX)) {
            receipt.payload = receipt_payload(std::mem::take(&mut receipt.payload));
        }
        if !needs_terminal_recovery(&state) {
            migrated |= confirm_answered_commands(&mut state);
            migrated |= confirm_side_questions(&mut state);
            state.compact();
        }
        Ok((state, migrated))
    }

    /// Poda o que nada mais lê; mesma regra do `compact` em runtime_queue.py.
    /// Fica: as últimas RECENT_CALLS chamadas (recibo e operação tocada), operação não final,
    /// operação de entrada ainda não confirmada e o grupo inteiro (raiz + fases por `logical_id`)
    /// de quem ficou. Operação final que fica perde o conteúdo da resposta (`slim`). Ocorrência
    /// usada só sai quando nenhuma operação restante pode casá-la.
    pub fn compact(&mut self) {
        let cutoff = self.next_seq.saturating_sub(RECENT_CALLS);
        let open_rows: BTreeSet<&str> = self.rows.iter().filter(|r| r["confirmed"] != true)
            .flat_map(|row|std::iter::once(row_id(row)).chain(row["batch_entries"].as_array().into_iter().flatten().map(row_id))).collect();
        let held = |key: &str, op: &Operation| op.seq >= cutoff || !key.starts_with(CALL_PREFIX)
            && (!matches!(op.status, Status::Accepted | Status::Rejected | Status::Confirmed)
                || op.entry_id.as_deref().is_some_and(|id| open_rows.contains(id)));
        let groups: BTreeSet<String> = self.operations.iter().filter(|(key, op)| !key.starts_with(CALL_PREFIX) && held(key, op))
            .map(|(key, op)| op.group(key).to_owned()).collect();
        let keep: BTreeSet<String> = self.operations.iter().filter(|(key, op)| held(key, op)
            || !key.starts_with(CALL_PREFIX) && groups.contains(op.group(key))).map(|(key, _)| key.clone()).collect();
        self.operations.retain(|key, _| keep.contains(key));
        for (_, op) in self.operations.iter_mut().filter(|(key, op)| !key.starts_with(CALL_PREFIX)
            && matches!(op.status, Status::Accepted | Status::Rejected | Status::Confirmed)) { op.slim(); }
        // A intenção já mora na operação e no recibo do Prepare; quem repete a chamada só lê
        // `status` e `result`. Sem isto cada fase copiava a intenção inteira (anexo incluído).
        for (_, receipt) in self.operations.iter_mut().filter(|(key, receipt)| key.starts_with(CALL_PREFIX)
            && OPERATION_RECEIPTS.contains(&receipt.payload["kind"].as_str().unwrap_or(""))) {
            if let Some(intent) = receipt.result.get_mut("payload") { *intent = Value::Null; }
        }
        // O conteúdo selado mora na linha; recibo de reserva só precisa lembrar quais IDs escolheu.
        for receipt in self.operations.values_mut().filter(|receipt|receipt.payload["kind"]=="claim_batch") {
            if let Some(rows)=receipt.result.as_array_mut() {for row in rows {*row=json!({"id":row["id"]});}}
        }
        // Linha confirmada não volta a confirmar (ConfirmOccurrence recusa), mesmo que a resposta
        // tardia tenha devolvido a operação para `accepted`.
        let confirmed_rows: BTreeSet<&str> = self.rows.iter().filter(|r| r["confirmed"] == true).filter_map(|r| r["id"].as_str()).collect();
        let candidates: Vec<&Operation> = self.operations.iter().filter(|(key, op)| !key.starts_with(CALL_PREFIX)
            && !op.entry_id.as_deref().is_some_and(|id| confirmed_rows.contains(id))).map(|(_, op)| op).collect();
        self.used_occurrences.retain(|_, record| !record.is_object() || candidates.iter().any(|op| op.may_confirm(record)));
    }

    fn protected(&self, entry_id: &str) -> bool {
        self.operations.iter().any(|(key, op)| !key.starts_with(CALL_PREFIX)
            && op.entry_id.as_deref() == Some(entry_id) && matches!(op.status, Status::Dispatching | Status::Unknown))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Load,
    Append { text:String, delivered:bool, ts:Option<f64>, pre_transcript:bool, entry_id:Option<String> },
    /// `confirms`: o comando que a CLI respondeu sozinha, sem linha no transcript.
    AppendLocal { text:String, entry_id:Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] confirms:Option<String> },
    Claim { min_ts:f64, limit:Option<usize>, entry_id:Option<String> },
    ClaimBatch { min_ts:f64 },
    SetDelivered { entry_id:String, value:bool, steered:bool },
    Abandon { entry_id:String },
    GiveUp { entry_id:String },
    /// Entregue há mais de `grace` s e sem prova vira desistida; o prazo conta do despacho (`dispatched_ts`) ou da
    /// reivindicação (`claimed_ts`), campos só do Rust na linha.
    GiveUpUnproved { now:f64, grace:f64 },
    BumpAttempts { entry_id:String },
    EntryDelivered { entry_id:String },
    Confirm { entry_ids:Vec<String> },
    /// Os hooks `UserPromptSubmit` começaram para a entrada já entregue: aceita, falta o registro real.
    MarkConsumed { entry_ids:Vec<String> },
    Prune { min_ts:f64 },
    Reconcile { committed:Vec<String>, min_ts:f64, now:f64, grace:f64, max_attempts:u64,
        confirm_only:bool, na_fila_tui:Vec<String> },
    Remove { entry_id:String },
    /// O X do painel: tira a entrada que ainda espera a vez. `"cancelled"`, `"not_waiting"` ou `"absent"`.
    Cancel { entry_id:String },
    /// Troca o texto da entrada que espera; a intenção adiada guardada na operação leva o texto novo.
    Edit { entry_id:String, text:String },
    /// Põe a entrada antes de `before_id`; `None` é o fim da fila. Só entre as que esperam.
    Reorder { entry_id:String, before_id:Option<String> },
    /// Põe a entrada na frente das que esperam: a próxima drenagem a leva.
    Promote { entry_id:String },
    Clear,
    Rename { name:String },
    Prepare { id:String, payload:Value, entry_id:Option<String> },
    /// Resposta assíncrona reservada: a entrada e a operação de mesmo id nascem juntas, ou nenhuma.
    ReserveAsyncAnswer { entry_id:String, text:String, payload:Value },
    BindDispatch { id:String, cursor:Value },
    /// O passo lógico de várias escritas: a primeira amarra o cursor; a que chega depois do despacho dele
    /// segue sem trocar nada, conferido aqui dentro em vez de numa foto que outra escrita já mudou.
    BindDispatchIfPrepared { id:String, cursor:Value },
    /// `staged`: o despacho começou sem efeito no terminal; `MarkWriting` avisa antes da primeira
    /// escrita. Interrompida antes disso, a tentativa volta à fila em vez de ficar incerta.
    BeginDispatch { id:String, wire_id:String, #[serde(default, skip_serializing_if = "std::ops::Not::not")] staged:bool },
    MarkWriting { id:String, wire_id:String },
    Finish { id:String, status:Status, result:Value },
    ConfirmOccurrence { id:String, proof:super::receipt::ReceiptProof },
    ConfirmLegacy { entry_id:String, occurrence:super::receipt::Occurrence, normalized_text:String },
    LateRpcResolution { id:String, wire_id:String, request_id:RequestId, generation:u64, result:Value },
    Recover,
    EnsureProjection,
    SetRuntimeState { state:Value },
    /// Sob a mesma lease, depois de parar o ator antigo. Não devolve entradas incertas à fila.
    AdvanceLifecycle { generation:u64, state:Value },
    ReplaceRows { rows:Vec<Value> },
}

pub struct Store {
    state_path: PathBuf,
    projection_dir: PathBuf,
    /// Compartilhado com as leituras: a escrita troca o `Arc` inteiro, e a leitura não copia o estado.
    state: Arc<State>,
    fenced: bool,
    recover_before_compact: bool,
    /// Projeção segurada pelo envio direto: a linha só aparece depois do `BeginDispatch`, já entregue.
    held: bool,
}

/// Recusa da própria fila: frase fixa, sem caminho nem texto da conversa, então pode ir ao log.
#[derive(Debug)]
pub struct QueueRefusal(pub &'static str);
impl std::fmt::Display for QueueRefusal { fn fmt(&self, f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) } }
impl std::error::Error for QueueRefusal {}

pub(super) fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, QueueRefusal(message)) }

/// A frase da recusa, se o erro veio da fila; erros do sistema ou do serde ficam só com o tipo.
pub fn refusal(error: &io::Error) -> Option<&'static str> {
    error.get_ref().and_then(|inner|inner.downcast_ref::<QueueRefusal>()).map(|refusal|refusal.0)
}

/// A trava solta por um dono que acabou de sair pode demorar (as tarefas de E/S dele, o `LockFileEx`
/// do Windows). Só "ocupada" espera, até `limit` pelo relógio: contar voltas estendia o prazo quando
/// cada tentativa demorava.
pub fn acquire_lease_within(path: &Path, limit: std::time::Duration) -> io::Result<Arc<Lease>> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match acquire_lease(path) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline =>
                std::thread::sleep(std::time::Duration::from_millis(50).min(deadline.saturating_duration_since(std::time::Instant::now()))),
            outcome => return outcome,
        }
    }
}
/// Trava da fila de uma sessão. Solta por `unlock` no fim: fechar só o descritor a deixava presa
/// enquanto um filho de `fork` ainda antes do `exec` guardava a cópia herdada.
#[derive(Debug)]
pub struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::warn!(code = "queue_lease_unlock_failed", io_kind = ?error.kind(), "trava da fila não foi solta; o descritor será fechado");
        }
    }
}
pub fn acquire_lease(path: &Path) -> io::Result<Arc<Lease>> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let file = options.open(path)?;
    file.try_lock().map_err(io::Error::from)?;
    Ok(Arc::new(Lease(file)))
}
/// TUI e app-server que a vigia adotou nesta fila: outra TUI com a mesma chave é outra sessão.
pub const CODEX_LIFE: &str = "codex_life";

/// Apaga a fila de uma sessão encerrada (o estado e, com `projection`, a projeção por nome). Chamar
/// com a trava; ausente é nada a apagar.
pub fn discard(state_path: &Path, projection_dir: &Path, projection: bool) -> io::Result<()> {
    let bytes = match std::fs::read(state_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let ignore_missing = |result: io::Result<()>| match result {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    if projection && let Ok((state, _)) = State::load(&bytes) {
        ignore_missing(std::fs::remove_file(projection_dir.join(format!("{}.jsonl", sanitize(&state.name)))))?;
    }
    // O ciclo de modos gravado pelo executor terminal morre com a fila da mesma vida.
    ignore_missing(std::fs::remove_file(state_path.with_extension("modes")))?;
    ignore_missing(std::fs::remove_file(state_path))
}

fn row_id(row: &Value) -> &str { row["id"].as_str().unwrap_or("") }
fn is_false(value:&bool)->bool {!*value}
fn current(row: &Value, min_ts: f64) -> bool {
    row["ts"].as_f64().unwrap_or(0.0) >= min_ts - if row["pre_transcript"] == true { 900.0 } else { 0.0 }
}
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c,'_' | '-' | '.') { c } else { '-' }).collect()
}

/// A projeção da fila de `name`: o mesmo `.jsonl` que o `PromptQueue` do Python lê e grava pelo nome.
pub(crate) fn projection_path(projection_dir: &Path, name: &str) -> PathBuf { projection_dir.join(format!("{}.jsonl", sanitize(name))) }

impl Store {
    pub fn open(state_path: &Path, projection_dir: &Path, initial: State) -> io::Result<Self> {
        if let Some(parent) = state_path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::create_dir_all(projection_dir)?;
        let state = match std::fs::read(state_path) {
            Ok(bytes) => {
                let (state, migrated) = State::load(&bytes)?;
                if state.owner_key != initial.owner_key { return Err(invalid("estado da fila incompatível")); }
                if migrated && !needs_terminal_recovery(&state) { atomic_write(state_path, &serde_json::to_vec(&state)?)?; }
                state
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                atomic_write(state_path, &serde_json::to_vec(&initial)?)?;
                initial
            }
            Err(error) => return Err(error),
        };
        let recover_before_compact=needs_terminal_recovery(&state);
        let mut store = Self { state_path:state_path.into(), projection_dir:projection_dir.into(), state:Arc::new(state), fenced:false, recover_before_compact, held:false };
        store.ensure_projection()?;
        Ok(store)
    }

    pub fn state(&self) -> &State { &self.state }

    pub fn ensure_projection(&mut self) -> io::Result<()> {
        self.held = false;
        let mut output = Vec::new();
        for row in &self.state.rows { serde_json::to_writer(&mut output, row)?; output.push(b'\n'); }
        let path = self.projection_dir.join(format!("{}.jsonl", sanitize(&self.state.name)));
        // Mesma regra do Python: as etapas de uma entrega não mudam as mensagens, e regravar igual
        // custava dois fsync por chamada, leituras incluídas. Compara com o arquivo para continuar
        // consertando o que mudou por fora.
        if std::fs::read(&path).ok().as_deref() != Some(output.as_slice()) { atomic_write(&path, &output)?; }
        if let Some(previous) = self.state.runtime_state["_queue_previous_name"].as_str()
            .filter(|p| *p != self.state.name) {
            match std::fs::remove_file(self.projection_dir.join(format!("{}.jsonl", sanitize(previous)))) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    /// Depois de uma gravação que falhou, o disco é a verdade: relê o estado dele. Com o disco ainda
    /// recusando, a próxima gravação falha de novo e bloqueia outra vez.
    pub fn unfence(&mut self) -> io::Result<()> {
        if self.fenced {
            self.state = Arc::new(State::load(&std::fs::read(&self.state_path)?)?.0);
            self.recover_before_compact=needs_terminal_recovery(&self.state);
            self.ensure_projection()?;
            self.fenced = false;
        }
        Ok(())
    }

    pub fn exec(&mut self, generation: u64, call_id: &str, clock: ClockSample, action: Action) -> io::Result<Value> {
        self.exec_held(generation, call_id, clock, action, false)
    }

    /// `hold`: grava com fsync mas não publica a projeção; ela segue segurada só pelo `BindDispatch`
    /// seguinte, e qualquer outra ação (o `BeginDispatch` que marca entregue, o `Finish` que adia) a publica.
    pub fn exec_held(&mut self, generation: u64, call_id: &str, clock: ClockSample, action: Action, hold: bool) -> io::Result<Value> {
        self.exec_validated(generation,call_id,clock,action,hold,None)
    }

    fn exec_validated(&mut self, generation:u64, call_id:&str, clock:ClockSample, action:Action, hold:bool, validate:Option<BatchValidator>) -> io::Result<Value> {
        // Lido sem zerar: ação que falha antes de gravar deixa a linha segurada para a próxima publicar.
        let was_held = self.held;
        let binds = matches!(&action, Action::BindDispatch { .. } | Action::BindDispatchIfPrepared { .. });
        self.unfence()?;
        if call_id.is_empty() { return Err(invalid("operação da fila sem identificador")); }
        let receipt_id = format!("{CALL_PREFIX}{call_id}");
        let payload = receipt_payload(serde_json::to_value(&action)?);
        if generation != self.state.generation {
            let same_advance=matches!(&action,Action::AdvanceLifecycle {generation:next,..}
                if generation.checked_add(1)==Some(*next) && *next==self.state.generation)
                && self.state.operations.get(&receipt_id).is_some_and(|receipt|receipt.payload==payload);
            if !same_advance { return Err(invalid("geração da fila mudou")); }
        }
        if let Some(previous) = self.state.operations.get(&receipt_id) {
            if receipt_payload(previous.payload.clone()) != payload { return Err(invalid("identificador reutilizado com outra operação")); }
            let result = if previous.payload["kind"]=="claim_batch" {
                json!(previous.result.as_array().into_iter().flatten().filter_map(|claimed|
                    self.state.rows.iter().find(|row|row_id(row)==row_id(claimed)).cloned()).collect::<Vec<_>>())
            }else{previous.result.clone()};
            self.ensure_projection()?;
            return Ok(result);
        }
        let readonly = matches!(&action, Action::Load | Action::EntryDelivered { .. } | Action::EnsureProjection);
        if readonly { self.ensure_projection()?; }
        let target = match &action {
            Action::Prepare { id, .. } | Action::BindDispatch { id, .. } | Action::BindDispatchIfPrepared { id, .. } | Action::BeginDispatch { id, .. } | Action::MarkWriting { id, .. }
            | Action::Finish { id, .. } | Action::LateRpcResolution { id, .. } | Action::ConfirmOccurrence { id, .. }
            | Action::ReserveAsyncAnswer { entry_id:id, .. } => Some(id.clone()),
            _ => None,
        };
        let targeted = target.is_some();
        let mut state = (*self.state).clone();
        if self.recover_before_compact && !readonly && !matches!(&action,Action::Recover) {
            apply(&mut state,Action::Recover,clock,call_id)?;
        }
        let mut result = apply(&mut state, action, clock, call_id)?;
        // A recusa acontece antes do diário e da projeção: os membros continuam editáveis.
        if let Some(validate)=validate {
            validate(&mut result)?;
            for row in result.as_array().into_iter().flatten() {
                if row["batch_entries"].is_array() && let Some(saved)=state.rows.iter_mut().find(|saved|row_id(saved)==row_id(row)) {*saved=row.clone();}
            }
        }
        if !readonly {
            let seq = state.next_seq;
            state.next_seq += 1;
            if let Some(op) = target.and_then(|id| state.operations.get_mut(&id)) { op.seq = seq; }
            let mut receipt = Operation::new(&receipt_id, payload, None);
            receipt.status = Status::Accepted;
            receipt.result = result.clone();
            if targeted && receipt.result.is_object() {
                let mut op: Operation = serde_json::from_value(receipt.result.clone())?;
                op.slim();
                receipt.result = serde_json::to_value(op)?;
            }
            receipt.seq = seq;
            state.operations.insert(receipt_id, receipt);
            state.compact();
            self.fenced = true;
            atomic_write(&self.state_path, &serde_json::to_vec(&state)?)?;
            self.state = Arc::new(state);
            self.fenced = false;
            self.recover_before_compact=false;
            self.held = hold || binds && was_held;
            if !self.held { self.ensure_projection()?; }
        }
        Ok(result)
    }
}

fn append_row(state: &mut State, row: Value) -> io::Result<Value> {
    if state.rows.iter().any(|r| row_id(r) == row_id(&row)) { return Err(invalid("entrada da fila já existe")); }
    let overflow = state.rows.len().saturating_add(1).saturating_sub(CAP);
    let candidates: Vec<_> = state.rows.iter().filter(|r| (r["confirmed"] == true || r["papel"] == "assistant")
        && !state.protected(row_id(r))).take(overflow).map(|r| row_id(r).to_owned()).collect();
    if candidates.len() < overflow { return Err(invalid("fila cheia de entradas pendentes")); }
    state.rows.retain(|r| !candidates.iter().any(|id| id == row_id(r)));
    state.rows.push(row.clone());
    mark_materialized(state,row_id(&row));
    Ok(row)
}

fn terminal_input(state:&State,op:&Operation)->bool {
    if op.entry_id.is_none() || op.payload["kind"]!="input" || !op.payload["payload"]["text"].is_string() {return false;}
    if let Some(generation)=op.payload["payload"].get("_terminal_generation") {return generation.as_u64()==Some(state.generation);}
    op.wire_attempts.keys().any(|wire|wire.starts_with(&format!("terminal:{}:",state.generation)))
        || state.operations.iter().any(|(call,receipt)|call.starts_with("call::terminal:queue:") && receipt.payload["kind"]=="prepare"
            && receipt.payload["id"]==op.id && receipt.payload["payload"]==op.payload
            && receipt.payload["entry_id"].as_str()==op.entry_id.as_deref())
}

fn entry_was_materialized(state:&State,entry:&str)->bool {
    state.rows.iter().any(|row|row_id(row)==entry)
        || state.operations.values().any(|op|op.entry_id.as_deref()==Some(entry)
            && (op.entry_materialized || terminal_input(state,op) && !op.wire_attempts.is_empty()))
        || state.operations.iter().any(|(call,receipt)|call.starts_with(CALL_PREFIX) && (matches!(receipt.payload["kind"].as_str(),Some("append"|"append_local"))
            && receipt.result["id"]==entry || matches!(receipt.payload["kind"].as_str(),Some("claim"|"claim_batch"))
            && receipt.result.as_array().is_some_and(|rows|rows.iter().any(|row|row["id"]==entry))
            || receipt.payload["kind"]=="remove" && receipt.payload["entry_id"]==entry && receipt.result==true))
}

fn mark_materialized(state:&mut State,entry:&str) {
    let ids:Vec<_>=state.operations.values().filter(|op|op.entry_id.as_deref()==Some(entry) && terminal_input(state,op))
        .map(|op|op.id.clone()).collect();
    for id in ids {state.operations.get_mut(&id).unwrap().entry_materialized=true;}
}

/// A entrada ainda espera a vez no Hangar, e só ela o painel apaga, edita ou move: nada a reivindicou nem a
/// confirmou, ninguém desistiu dela, não é a resposta reservada de uma pergunta e nenhuma tentativa está em
/// voo. A tentativa terminal preparada e ainda não escrita não está em voo: o ator do terminal não intercala
/// ações da fila com o despacho. Sem terminal, a preparada é o despacho começando.
fn waiting_row(state:&State,row:&Value)->bool {
    let id=row_id(row);
    row_waits(row)
        && !state.operations.iter().any(|(key,op)|!key.starts_with(CALL_PREFIX) && op.entry_id.as_deref()==Some(id)
            && (op.payload["kind"]=="dispatch_async_answer" || matches!(op.status,Status::Dispatching|Status::Unknown)
                || op.status==Status::Prepared && !terminal_input(state,op) && !stale_terminal_intent(op)))
}

/// A parte da regra que a linha sozinha responde, a única que a projeção (sem as operações) permite ler:
/// o painel da fila a usa, e a tentativa em voo que ela não vê o ator recusa como `not_waiting`.
pub(crate) fn row_waits(row:&Value)->bool {
    row["delivered"]==false && row["confirmed"]!=true && row["desistiu"]!=true && row["papel"]!="assistant" && row["reserved"]!=true
}

/// Intenção terminal de uma vida anterior que nunca começou a escrever (queda e reabertura no meio):
/// a linha continua só esperando, e a drenagem desta vida a leva com tentativa nova.
fn stale_terminal_intent(op:&Operation)->bool {
    op.payload["payload"].get("_terminal_generation").is_some() && op.wire_attempts.is_empty()
}

/// Posição da entrada esperando, ou por que ela não pode ser mexida.
fn waiting_index(state:&State,entry:&str)->Result<usize,&'static str> {
    match state.rows.iter().position(|row|row_id(row)==entry) {
        None=>Err("absent"),
        Some(index) if state.rows[index]["batch_entries"].is_array() || !waiting_row(state,&state.rows[index])=>Err("not_waiting"),
        Some(index)=>Ok(index),
    }
}

/// As que esperam, na ordem de entrega, com o mesmo corte de idade do `Claim`: linha de vida anterior que a
/// drenagem nunca levaria não segura comando nem Orientar.
pub fn waiting_ids(state:&State,min_ts:f64)->Vec<String> {
    state.rows.iter().filter(|row|current(row,min_ts) && waiting_row(state,row)).map(|row|row_id(row).to_owned()).collect()
}

/// Texto que a fila aceita como mensagem: comando de barra não tem linha na fila, e controle quebraria a digitação.
/// Uma regra só, para a edição do painel, a rota e a intenção terminal recuperada.
pub(crate) fn queue_text_ok(text:&str)->bool {
    !text.trim().is_empty() && !text.trim_start().starts_with('/') && !text.chars().any(|c|c.is_control() && !matches!(c,'\n'|'\t'))
}

fn terminal_protected(state:&State,entry:&str,except:&str)->bool {
    let protected_status=|status|matches!(status,Status::Accepted|Status::Unknown|Status::Dispatching|Status::Confirmed);
    state.operations.values().any(|op|op.entry_id.as_deref()==Some(entry) && ((op.id!=except && protected_status(op.status))
        || state.operations.values().any(|phase|phase.payload["logical_id"]==op.id && protected_status(phase.status))))
}

fn terminal_finish_sequence(state:&State,id:&str,result:&Value)->Option<u64> {
    state.operations.iter().filter(|(call,receipt)|call.starts_with("call::terminal:queue:") && receipt.payload["kind"]=="finish"
        && receipt.payload["id"]==id && slim(&receipt.payload["result"])==slim(result))
        .filter_map(|(call,_)|call.rsplit(':').next()?.parse().ok()).max()
}

fn needs_terminal_recovery(state:&State)->bool {
    state.operations.iter().any(|(key,op)| {
        if !key.starts_with(CALL_PREFIX) && terminal_input(state,op)
            && (!op.entry_materialized || matches!(op.status,Status::Deferred|Status::Rejected) && !op.terminal_finalized) {return true;}
        key.starts_with("call::terminal:queue:") && matches!(op.payload["kind"].as_str(),Some("claim"|"claim_batch"))
            && op.result.as_array().is_some_and(|claimed|state.rows.iter().any(|row|claimed.iter().any(|item|row_id(item)==row_id(row))
                && row["delivered"]==true && row["confirmed"]!=true && row["desistiu"]!=true && !terminal_protected(state,row_id(row),"")))
    })
}

fn record_terminal_write_barrier(state:&mut State,id:&str) {
    let op=&state.operations[id];
    if op.status!=Status::Unknown || !terminal_input(state,op) || op.result["payload"]["native"]==true {return;}
    if state.runtime_state["terminal_write_barrier"].is_null() {
        state.runtime_state["terminal_write_barrier"]=json!({"operation_id":id,"generation":state.generation,"conversation":op.dispatch_cursor["conversation"]});
    }
}

// Sem exigir a geração atual: a trava atravessa a troca de geração da mesma conversa.
fn holds_terminal_write(op:&Operation,conversation:&Value)->bool {
    op.status==Status::Unknown && op.entry_id.is_some() && op.payload["kind"]=="input" && op.payload["payload"]["text"].is_string()
        && (op.payload["payload"].get("_terminal_generation").is_some() || op.wire_attempts.keys().any(|wire|wire.starts_with("terminal:")))
        && op.result["payload"]["native"]!=true && (conversation.is_null() || op.dispatch_cursor["conversation"]==*conversation)
}

/// Comando respondido pela própria CLI não vira linha no transcript: a resposta dela é a prova de
/// que a entrada mais antiga com aquele texto chegou.
fn confirm_local_command(state:&mut State,source:&str) {
    let source = source.trim();
    if !source.starts_with('/') || source.len() < 2 { return; }
    let Some(id) = state.rows.iter().find(|r|r["confirmed"] != true && r["papel"] != "assistant" && r["delivered"] == true
        && r["text"].as_str().map(str::trim) == Some(source)).map(|r|row_id(r).to_owned()) else { return };
    confirm_row(state,&id);
}

fn confirm_row(state:&mut State,id:&str) {
    if let Some(row) = state.rows.iter_mut().find(|r|row_id(r) == id) {
        row["confirmed"] = json!(true);
        row.as_object_mut().unwrap().remove("desistiu");
    }
    for (_,op) in state.operations.iter_mut().filter(|(key,op)|!key.starts_with(CALL_PREFIX) && op.entry_id.as_deref() == Some(id)) {
        op.status = Status::Confirmed;
    }
    batch::confirm_members(state,id);
    release_terminal_write_barrier(state);
}

/// Pergunta lateral (`/hangar-btw`, `/btw`): o plugin responde no painel e nada entra no transcript,
/// então só a operação ACEITA dela confirma a entrada; despacho em curso ou incerto não confirma nada.
fn confirm_side_question(state:&mut State,op_id:&str)->bool {
    let Some(entry) = state.operations.get(op_id).filter(|op|op.status == Status::Accepted).and_then(|op|op.entry_id.clone())
        else { return false };
    let side = state.rows.iter().find(|row|row_id(row) == entry).is_some_and(|row| {
        row["confirmed"] != true && row["papel"] != "assistant"
            && row["text"].as_str().and_then(|text|text.split_whitespace().next()).is_some_and(|c|c == "/hangar-btw" || c == "/btw")
    });
    if side { confirm_row(state,&entry); }
    side
}

/// Na leitura: perguntas laterais já aceitas que ficaram presas antes desta regra.
fn confirm_side_questions(state:&mut State)->bool {
    let accepted:Vec<String> = state.operations.iter().filter(|(key,op)|!key.starts_with(CALL_PREFIX) && op.status == Status::Accepted)
        .map(|(key,_)|key.clone()).collect();
    accepted.iter().fold(false,|changed,id|confirm_side_question(state,id) || changed)
}

/// Fila gravada antes de a resposta local confirmar o comando: a entrada `/x` entregue seguida
/// logo de uma resposta local que nomeia o mesmo `/x` é a mesma prova, só que já no disco.
fn confirm_answered_commands(state:&mut State)->bool {
    let answered:Vec<String> = state.rows.windows(2).filter(|pair| {
        let (entry,answer) = (&pair[0],&pair[1]);
        let gap = answer["ts"].as_f64().unwrap_or(f64::MAX) - entry["ts"].as_f64().unwrap_or(0.0);
        let Some(command) = entry["text"].as_str().and_then(|text|text.split_whitespace().next()).filter(|c|c.starts_with('/') && c.len() > 1)
            else { return false };
        // Só a resposta que começa pelo próprio comando ("/btw isn't available…"); aviso local qualquer não prova nada.
        let names = answer["text"].as_str().and_then(|text|text.trim_start().strip_prefix(command))
            .is_some_and(|rest|rest.is_empty() || rest.starts_with(char::is_whitespace));
        entry["confirmed"] != true && entry["papel"] != "assistant" && entry["delivered"] == true && entry["desistiu"] != true
            && answer["papel"] == "assistant" && (0.0..=10.0).contains(&gap) && names
    }).map(|pair|row_id(&pair[0]).to_owned()).collect();
    for id in &answered { confirm_row(state,id); }
    !answered.is_empty()
}

/// A trava só sai quando a dona deixou de ser incerta e nenhuma outra da conversa resta.
fn release_terminal_write_barrier(state:&mut State) {
    let barrier=&state.runtime_state["terminal_write_barrier"];
    if !barrier.is_object() {return;}
    if barrier["operation_id"].as_str().and_then(|id|state.operations.get(id)).is_some_and(|op|op.status==Status::Unknown) {return;}
    let conversation=barrier["conversation"].clone();
    let heir=state.operations.iter().find(|(key,op)|!key.starts_with(CALL_PREFIX) && holds_terminal_write(op,&conversation)).map(|(key,_)|key.clone());
    match heir {
        Some(id)=>state.runtime_state["terminal_write_barrier"]=json!({"operation_id":id,"generation":state.generation,"conversation":conversation}),
        None=>{state.runtime_state.as_object_mut().unwrap().remove("terminal_write_barrier");}
    }
}

fn finalize_terminal(state:&mut State,id:&str,clock:ClockSample)->io::Result<()> {
    let op=state.operations.get(id).ok_or_else(||invalid("operação terminal ausente"))?.clone();
    if !terminal_input(state,&op) || op.terminal_finalized || !matches!(op.status,Status::Deferred|Status::Rejected) {return Ok(());}
    let entry=op.entry_id.as_deref().unwrap();
    let protected=terminal_protected(state,entry,id);
    let attempts=state.rows.iter().find(|row|row_id(row)==entry).and_then(|row|row["attempts"].as_u64()).unwrap_or(0);
    // O diário antigo pode já ter gravado o contador antes da queda.
    let counted=terminal_finish_sequence(state,id,&op.result).is_some_and(|finish|state.operations.iter().any(|(call,receipt)|call.starts_with("call::terminal:queue:")
        && call.rsplit(':').next().and_then(|suffix|suffix.parse::<u64>().ok()).is_some_and(|sequence|sequence>finish)
        && receipt.payload["kind"]=="bump_attempts" && receipt.payload["entry_id"]==entry && receipt.result.as_u64()==Some(attempts)));
    let row=state.rows.iter_mut().find(|row|row_id(row)==entry).ok_or_else(||invalid("entrada terminal ausente na finalização"))?;
    if row["confirmed"]!=true && row["desistiu"]!=true && (op.status==Status::Rejected || row["delivered"]!=false) {
        if op.status==Status::Rejected {
            row["delivered"]=json!(true);row["desistiu"]=json!(true);row["desistiu_ts"]=json!(clock.epoch_s);
        }else if !protected {
            let cleanup=op.result["payload"]["cleanup"].as_str().ok_or_else(||invalid("resultado terminal sem prova de limpeza"))?;
            if !matches!(cleanup,"proved"|"not_needed") {return Err(invalid("limpeza incerta não permite reentrega"));}
            if cleanup=="proved" && !counted && attempts>=2 {
                row["delivered"]=json!(true);row["desistiu"]=json!(true);row["desistiu_ts"]=json!(clock.epoch_s);
            }else {
                if cleanup=="proved" && !counted {row["attempts"]=json!(attempts+1);}
                row["delivered"]=json!(false);row.as_object_mut().unwrap().remove("steered");
            }
        }
    }
    state.operations.get_mut(id).unwrap().terminal_finalized=true;Ok(())
}

fn recover_terminal(state:&mut State,clock:ClockSample)->io::Result<()> {
    let legacy:Vec<_>=state.operations.values().filter(|op|terminal_input(state,op)
        && op.payload["payload"].get("_terminal_generation").is_none()).map(|op|op.id.clone()).collect();
    for id in legacy {state.operations.get_mut(&id).unwrap().payload["payload"]["_terminal_generation"]=json!(state.generation);}
    release_terminal_write_barrier(state);
    let unknown:Vec<_>=state.operations.values().filter(|op|op.status==Status::Unknown).map(|op|op.id.clone()).collect();
    for id in unknown {record_terminal_write_barrier(state,&id);}
    let materialized:Vec<_>=state.operations.values().filter(|op|terminal_input(state,op)
        && entry_was_materialized(state,op.entry_id.as_deref().unwrap())).filter_map(|op|op.entry_id.clone()).collect();
    for entry in materialized {mark_materialized(state,&entry);}
    let prepared:Vec<_>=state.operations.values().filter(|op|op.status==Status::Prepared && !op.entry_materialized && op.wire_attempts.is_empty() && terminal_input(state,op)
        && !terminal_protected(state,op.entry_id.as_deref().unwrap(),&op.id)).cloned().collect();
    for op in prepared {
        let entry=op.entry_id.as_deref().unwrap();
        if state.rows.iter().any(|row|row_id(row)==entry) {continue;}
        let text=op.payload["payload"]["text"].as_str().unwrap();
        if !queue_text_ok(text) {return Err(invalid("intenção terminal inválida"));}
        let mut row=json!({"id":entry,"text":text,"ts":clock.epoch_s,"delivered":false});
        if op.payload["payload"]["pre_transcript"]==true {row["pre_transcript"]=json!(true);}
        append_row(state,row)?;
    }
    let mut finished:Vec<_>=state.operations.values().filter(|op|terminal_input(state,op) && matches!(op.status,Status::Deferred|Status::Rejected))
        .map(|op|(terminal_finish_sequence(state,&op.id,&op.result).unwrap_or(0),op.id.clone())).collect();
    finished.sort_by(|left,right|right.0.cmp(&left.0));
    for (_,id) in finished {finalize_terminal(state,&id,clock)?;}
    Ok(())
}

fn apply(state: &mut State, action: Action, clock: ClockSample, call_id: &str) -> io::Result<Value> {
    let protected: BTreeSet<String> = state.rows.iter().filter(|r| state.protected(row_id(r))).map(|r| row_id(r).into()).collect();
    let result = match action {
        Action::Load => json!(state.rows),
        Action::EnsureProjection => Value::Null,
        Action::Append { text, delivered, ts, pre_transcript, entry_id } => {
            let mut row = json!({"id":entry_id.unwrap_or_else(||call_id.into()),"text":text,"ts":ts.unwrap_or(clock.epoch_s),"delivered":delivered});
            if pre_transcript { row["pre_transcript"] = json!(true); }
            append_row(state,row)?
        }
        Action::AppendLocal { text, entry_id, confirms } => {
            let row = append_row(state,json!({"id":entry_id.unwrap_or_else(||call_id.into()),
                "text":text,"ts":clock.epoch_s,"delivered":true,"confirmed":true,"papel":"assistant"}))?;
            if let Some(source) = confirms { confirm_local_command(state,&source); }
            row
        }
        Action::ClaimBatch { min_ts } => batch::claim(state,min_ts,clock,call_id)?,
        Action::Claim { min_ts, limit, entry_id } => {
            let mut claimed = Vec::new();
            for row in &mut state.rows {
                if protected.contains(row_id(row)) || entry_id.as_deref().is_some_and(|id| id != row_id(row)) { continue; }
                if row["delivered"] == false && current(row,min_ts) {
                    row["delivered"] = json!(true); row["claimed_ts"] = json!(clock.epoch_s);
                    claimed.push(row.clone());
                    if limit.is_some_and(|limit| claimed.len() >= limit) { break; }
                }
            }
            json!(claimed)
        }
        Action::SetDelivered { entry_id, value, steered } => {
            if !value && protected.contains(&entry_id) { return Err(invalid("entrega incerta não pode voltar para a fila")); }
            if let Some(row) = state.rows.iter_mut().find(|r| row_id(r) == entry_id && !protected.contains(&entry_id)) {
                row["delivered"] = json!(value);
                if value && steered { row["steered"] = json!(true); }
                else if !value { row.as_object_mut().unwrap().remove("steered"); }
            }
            Value::Null
        }
        Action::Abandon { entry_id } => {
            if let Some(row) = state.rows.iter_mut().find(|r| row_id(r) == entry_id) {
                row["delivered"] = json!(true); row["desistiu"] = json!(true); row["desistiu_ts"] = json!(clock.epoch_s);
            }
            Value::Null
        }
        // A pessoa desiste da entrega que nunca se provou: sem isto a trava de escrita espera para sempre.
        // Nada é reenviado; a linha vira "não chegou" e a fila volta a andar.
        Action::GiveUp { entry_id } => {
            let ids: Vec<String> = state.operations.iter().filter(|(key, op)| !key.starts_with(CALL_PREFIX)
                && op.entry_id.as_deref() == Some(entry_id.as_str()) && holds_terminal_write(op, &Value::Null))
                .map(|(key, _)| key.clone()).collect();
            for id in &ids {
                let op = state.operations.get_mut(id).unwrap();
                let stage = op.result["payload"]["stage"].clone();
                op.status = Status::Rejected;
                op.result = json!({"operation_id":id,"disposition":"rejected","payload":{"code":"delivery_given_up","stage":stage,"cleanup":"not_needed"}});
                op.terminal_finalized = true;
            }
            if !ids.is_empty() {
                if let Some(row) = state.rows.iter_mut().find(|r| row_id(r) == entry_id && r["confirmed"] != true) {
                    row["delivered"] = json!(true); row["desistiu"] = json!(true); row["desistiu_ts"] = json!(clock.epoch_s);
                }
            }
            // Sem incerta desta entrada, a trava pode ter ficado de uma dona que já se resolveu: solta igual.
            release_terminal_write_barrier(state);
            json!(!ids.is_empty())
        }
        // Sem terminal a escrita no stdin é a entrega e nada é redigitado: sem prova no prazo, a entrada vira
        // "não chegou" (descartável) e a incerta deixa de segurar a fila. A prova que chegar depois a resgata.
        Action::GiveUpUnproved { now, grace } => {
            let mut given = Vec::new();
            let mut next_due: Option<f64> = None;
            // Incerta de terminal pode ser tecla já digitada: a finalização dela é do ator do terminal.
            let uncertain: BTreeSet<String> = state.operations.iter()
                .filter(|(key, op)| !key.starts_with(CALL_PREFIX) && op.status == Status::Unknown && !terminal_input(state, op))
                .filter_map(|(_, op)| op.entry_id.clone()).collect();
            for row in &mut state.rows {
                if row["confirmed"] == true || row["desistiu"] == true
                    || row["papel"] == "assistant" || row["steered"] == true { continue; }
                let id = row_id(row).to_owned();
                let ops = || state.operations.iter().filter(|(key, op)| !key.starts_with(CALL_PREFIX) && op.entry_id.as_deref() == Some(id.as_str()));
                // A não marcada como entregue só entra incerta: fora da espera, nada mais a mexeria.
                if row["delivered"] != true && !uncertain.contains(&id) { continue; }
                // Escrita em curso: o desfecho é do ator.
                if ops().any(|(_, op)| op.status == Status::Dispatching) { continue; }
                let since = ["dispatched_ts","claimed_ts","ts"].iter().find_map(|key| row[*key].as_f64()).unwrap_or(0.0);
                if since + grace > now { next_due = Some(next_due.map_or(since + grace, |due: f64| due.min(since + grace))); continue; }
                row["delivered"] = json!(true); row["desistiu"] = json!(true); row["desistiu_ts"] = json!(now);
                given.push(id);
            }
            for op in state.operations.iter_mut().filter(|(key, op)| !key.starts_with(CALL_PREFIX) && op.status == Status::Unknown
                && op.entry_id.as_ref().is_some_and(|entry| given.contains(entry))).map(|(_, op)| op) {
                op.status = Status::Rejected;
                op.result = json!({"operation_id":op.id,"disposition":"rejected","payload":{"code":"delivery_given_up"}});
            }
            json!({"given_up":given,"next_due":next_due})
        }
        Action::BumpAttempts { entry_id } => {
            if let Some(row) = state.rows.iter_mut().find(|r| row_id(r) == entry_id && !protected.contains(&entry_id)) {
                let count = row["attempts"].as_u64().unwrap_or(0) + 1;
                row["attempts"] = json!(count); json!(count)
            } else { json!(0) }
        }
        Action::EntryDelivered { entry_id } => state.rows.iter().find(|r| row_id(r) == entry_id)
            .map(|r|json!(r["delivered"].as_bool().unwrap_or(false))).unwrap_or(Value::Null),
        // Prova direta da CLI (eco da mensagem ou resposta de comando local): vale mesmo com a escrita ainda sem
        // desfecho (o eco e o `result` podem vir antes do `Finish`) e resgata a desistida.
        Action::MarkConsumed { entry_ids } => {
            let mut count = 0;
            for row in &mut state.rows {
                if entry_ids.iter().any(|id| id == row_id(row)) && row["delivered"] == true && row["confirmed"] != true && row["consumed"] != true {
                    row["consumed"] = json!(true); count += 1;
                }
            }
            json!(count)
        }
        Action::Confirm { entry_ids } => {
            let mut count = 0;
            let mut confirmed = Vec::new();
            for row in &mut state.rows {
                if entry_ids.iter().any(|id| id == row_id(row)) && row["delivered"] == true && row["confirmed"] != true {
                    row["confirmed"] = json!(true); count += 1;
                    row.as_object_mut().unwrap().remove("desistiu");
                    confirmed.push(row_id(row).to_owned());
                }
            }
            for id in confirmed {batch::confirm_members(state,&id);}
            json!(count)
        }
        Action::Prune { min_ts } => {
            let before = state.rows.len();
            if min_ts > 0.0 { state.rows.retain(|r| protected.contains(row_id(r)) || current(r,min_ts)); }
            json!(before - state.rows.len())
        }
        Action::Remove { entry_id } => {
            let before = state.rows.len();
            state.rows.retain(|r| row_id(r) != entry_id || r["desistiu"] != true || protected.contains(&entry_id));
            json!(before != state.rows.len())
        }
        Action::Cancel { entry_id } => match waiting_index(state,&entry_id) {
            Err(outcome)=>json!(outcome),
            Ok(index)=>{
                state.rows.remove(index);
                // A intenção adiada desta entrada sai junto: o Recover não a rematerializa como linha nova.
                state.operations.retain(|key,op|key.starts_with(CALL_PREFIX) || op.entry_id.as_deref()!=Some(entry_id.as_str()));
                json!("cancelled")
            }
        },
        Action::Edit { entry_id, text } => {
            if !queue_text_ok(&text) { return Err(invalid("texto da fila inválido")); }
            match waiting_index(state,&entry_id) {
                Err(outcome)=>json!(outcome),
                Ok(index)=>{
                    state.rows[index]["text"]=json!(text);
                    for (_,op) in state.operations.iter_mut().filter(|(key,op)|!key.starts_with(CALL_PREFIX) && op.entry_id.as_deref()==Some(entry_id.as_str())) {
                        if op.payload["payload"]["text"].is_string() { op.payload["payload"]["text"]=json!(text); }
                    }
                    json!("edited")
                }
            }
        }
        Action::Reorder { entry_id, before_id } => {
            let from=match waiting_index(state,&entry_id) { Err(outcome)=>return Ok(json!(outcome)), Ok(index)=>index };
            if let Some(before)=before_id.as_deref().filter(|before|*before!=entry_id) {
                if let Err(outcome)=waiting_index(state,before) { return Ok(json!(outcome)); }
                let row=state.rows.remove(from);
                let at=state.rows.iter().position(|r|row_id(r)==before).expect("conferida acima");
                state.rows.insert(at,row);
            } else if before_id.is_none() {
                let row=state.rows.remove(from);
                state.rows.push(row);
            }
            json!("moved")
        }
        Action::Promote { entry_id } => match waiting_index(state,&entry_id) {
            Err(outcome)=>json!(outcome),
            Ok(from)=>{
                let row=state.rows.remove(from);
                let at=state.rows.iter().position(|r|waiting_row(state,r)).unwrap_or(state.rows.len()).min(from);
                state.rows.insert(at,row);
                json!("moved")
            }
        },
        Action::Clear => { state.rows.clear(); Value::Null }
        Action::Rename { name } => {
            if name.is_empty() { return Err(invalid("nome vazio na fila")); }
            state.runtime_state["_queue_previous_name"] = json!(state.name);
            state.name = name;
            Value::Null
        }
        Action::Prepare { id, payload, entry_id } => {
            if id.starts_with(CALL_PREFIX) { return Err(invalid("identificador reservado")); }
            if let Some(old) = state.operations.get(&id) {
                if old.payload != payload || old.entry_id != entry_id { return Err(invalid("intenção da operação mudou")); }
            } else {
                // Linha já confirmada pela fila: a mesma intenção chegando depois da poda não reenvia.
                let delivered = payload.get("logical_id").is_none() && entry_id.as_deref()
                    .is_some_and(|entry| state.rows.iter().any(|r| row_id(r) == entry && r["confirmed"] == true));
                let mut op = Operation::new(&id,payload,entry_id);
                if delivered {
                    op.status = Status::Accepted;
                    op.result = json!({"operation_id":id,"disposition":"accepted","payload":{"already_confirmed":true}});
                }
                state.operations.insert(id.clone(),op);
            }
            if state.operations[&id].status == Status::Deferred {
                let old = state.operations.get_mut(&id).unwrap(); old.status = Status::Prepared; old.result = Value::Null;old.terminal_finalized=false;
            }
            if let Some(entry)=state.operations[&id].entry_id.clone().filter(|entry|entry_was_materialized(state,entry)) {mark_materialized(state,&entry);}
            serde_json::to_value(&state.operations[&id])?
        }
        Action::ReserveAsyncAnswer { entry_id, text, payload } => {
            if entry_id.is_empty() || entry_id.starts_with(CALL_PREFIX) { return Err(invalid("identificador reservado")); }
            if let Some(old) = state.operations.get(&entry_id) {
                if old.payload != payload || old.entry_id.as_deref() != Some(entry_id.as_str()) { return Err(invalid("intenção da operação mudou")); }
            } else {
                append_row(state,json!({"id":entry_id,"text":text,"ts":clock.epoch_s,"delivered":false,"reserved":true}))?;
                state.operations.insert(entry_id.clone(),Operation::new(&entry_id,payload,Some(entry_id.clone())));
            }
            serde_json::to_value(&state.operations[&entry_id])?
        }
        Action::BindDispatch { id, cursor } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if op.status != Status::Prepared { return Err(invalid("cursor precisa preceder o despacho")); }
            op.dispatch_cursor = cursor;
            serde_json::to_value(op)?
        }
        Action::BindDispatchIfPrepared { id, cursor } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if op.status == Status::Prepared { op.dispatch_cursor = cursor; }
            serde_json::to_value(op)?
        }
        Action::BeginDispatch { id, wire_id, staged } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if !matches!(op.status,Status::Prepared | Status::Dispatching) { return Err(invalid("operação não pode ser reenviada")); }
            op.wire_attempts.entry(wire_id).or_insert_with(||json!({"status":if staged {"staged"} else {"dispatching"},"result":null}));
            op.status = Status::Dispatching;
            for row in &mut state.rows { if Some(row_id(row)) == op.entry_id.as_deref() {
                row["delivered"] = json!(true);
                if row.get("dispatched_ts").is_none() { row["dispatched_ts"] = json!(clock.epoch_s); }
            } }
            serde_json::to_value(op)?
        }
        Action::MarkWriting { id, wire_id } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if op.status != Status::Dispatching { return Err(invalid("escrita fora do despacho")); }
            let attempt = op.wire_attempts.get_mut(&wire_id).ok_or_else(||invalid("tentativa não registrada"))?;
            if attempt["status"] == "staged" { attempt["status"] = json!("dispatching"); }
            serde_json::to_value(op)?
        }
        Action::Finish { id, status, result } => {
            let terminal=state.operations.get(&id).is_some_and(|op|terminal_input(state,op));
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if terminal && status==Status::Deferred && matches!(op.status,Status::Accepted|Status::Unknown|Status::Confirmed|Status::Rejected) {
                return Err(invalid("resultado terminal protegido não permite reentrega"));
            }
            if matches!(op.status,Status::Accepted | Status::Confirmed | Status::Rejected)
                && op.result.get("disposition").is_some() && result.get("write_outcome").is_some() {
                return Ok(serde_json::to_value(op)?);
            }
            if matches!(op.status,Status::Accepted | Status::Confirmed | Status::Rejected) && status == Status::Unknown {
                return Ok(serde_json::to_value(op)?);
            }
            if op.status == Status::Unknown && matches!(status,Status::Prepared | Status::Dispatching | Status::Deferred) {
                return Err(invalid("resultado incerto não permite reenvio"));
            }
            let finalized=op.terminal_finalized && op.status==status;
            let consumed=terminal && status==Status::Accepted && result["payload"]["consumed"]==true;
            let entry=op.entry_id.clone();
            op.status = status; op.result = result;op.terminal_finalized=finalized;
            if consumed && let Some(row)=state.rows.iter_mut().find(|row|Some(row_id(row))==entry.as_deref()) && row["confirmed"]!=true {
                row["consumed"]=json!(true);
            }
            record_terminal_write_barrier(state,&id);
            release_terminal_write_barrier(state);
            if terminal {finalize_terminal(state,&id,clock)?;}
            confirm_side_question(state,&id);
            serde_json::to_value(&state.operations[&id])?
        }
        Action::LateRpcResolution { id, wire_id, request_id, generation, result } => {
            if generation != state.generation { return Err(invalid("resposta de outra geração")); }
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            let attempt = op.wire_attempts.get_mut(&wire_id).ok_or_else(||invalid("tentativa não registrada"))?;
            let expected = op.payload.get("request_id").or_else(||op.payload.get("id"));
            if expected != Some(&serde_json::to_value(&request_id)?) { return Err(invalid("resposta não corresponde ao pedido")); }
            attempt["status"] = json!("accepted"); attempt["result"] = result.clone();
            if op.wire_attempts.values().all(|a|a["status"] == "accepted") && op.status != Status::Confirmed {
                op.status = Status::Accepted; op.result = result;
                release_terminal_write_barrier(state);
                confirm_side_question(state,&id);
            }
            serde_json::to_value(&state.operations[&id])?
        }
        Action::ConfirmOccurrence { id, proof } => {
            if state.used_occurrences.contains_key(&proof.occurrence.id) { return Ok(json!(false)); }
            let operation = state.operations.get(&id).ok_or_else(||invalid("operação não preparada"))?;
            if operation.status == Status::Confirmed { return Ok(json!(false)); }
            let cursor: super::receipt::DispatchCursor = serde_json::from_value(operation.dispatch_cursor.clone())
                .map_err(|_|invalid("operação sem cursor de despacho"))?;
            let row = state.rows.iter_mut().find(|r|Some(row_id(r)) == operation.entry_id.as_deref())
                .ok_or_else(||invalid("entrada da operação não existe"))?;
            if row["confirmed"] == true { return Ok(json!(false)); }
            if !proof.validates(&cursor,row) { return Err(invalid("prova de entrega não corresponde ao despacho")); }
            row["delivered"] = json!(true); row["confirmed"] = json!(true);
            row.as_object_mut().unwrap().remove("desistiu");
            let entry=row_id(row).to_owned();
            state.used_occurrences.insert(proof.occurrence.id.clone(),json!({"operation_id":id,"generation":state.generation,
                "conversation":proof.occurrence.conversation,"file_identity":proof.occurrence.file_identity,"offset":proof.occurrence.offset}));
            state.operations.get_mut(&id).unwrap().status = Status::Confirmed;
            batch::confirm_members(state,&entry);
            release_terminal_write_barrier(state);
            json!(true)
        }
        Action::ConfirmLegacy { entry_id, occurrence, normalized_text } => {
            // Deixou de ser legada desde a leitura (despachada, reivindicada, confirmada, sumiu): nada a fazer.
            if state.used_occurrences.contains_key(&occurrence.id) || !legacy_rows(state).iter().any(|r|row_id(r) == entry_id) {
                return Ok(json!(false));
            }
            let row = state.rows.iter_mut().find(|r|row_id(r) == entry_id).ok_or_else(||invalid("entrada legada não existe"))?;
            let proven = row["ts"].as_f64().is_some_and(|sent|super::receipt::legacy_accepts(&occurrence,sent))
                && entry_lines(row).contains(&normalized_text)
                && crate::transcript::history::chaves_de_commit(&occurrence.text).contains(&normalized_text);
            if !proven { return Err(invalid("prova da entrada legada não corresponde")); }
            row["confirmed"] = json!(true);
            row.as_object_mut().unwrap().remove("desistiu");
            // Texto, não objeto: a compactação só descarta recibo de operação, e este não tem.
            state.used_occurrences.insert(occurrence.id, json!("legacy"));
            json!(true)
        }
        Action::Recover => {
            for op in state.operations.values_mut() {
                // Nenhuma tentativa chegou a escrever: nada pode ter alcançado o terminal.
                if op.status == Status::Dispatching && !op.wire_attempts.is_empty() && op.wire_attempts.values().all(|a|a["status"]=="staged") {
                    for attempt in op.wire_attempts.values_mut() { attempt["status"] = json!("not_written"); }
                    op.status = Status::Deferred; op.terminal_finalized = false;
                    op.result = json!({"operation_id":op.id,"disposition":"deferred","payload":{"code":"interrupted_before_write","queued":op.entry_id.is_some(),"cleanup":"not_needed"}});
                    continue;
                }
                if op.status == Status::Dispatching { op.status = Status::Unknown; }
                // Confirmada pelo transcript, a tentativa chegou: não há o que marcar como incerto.
                if op.status == Status::Confirmed { continue; }
                for attempt in op.wire_attempts.values_mut() {
                    if attempt["status"] == "dispatching" { attempt["status"] = json!("unknown"); }
                }
            }
            recover_terminal(state,clock)?;
            let phases:BTreeSet<_> = state.operations.values().filter(|op|matches!(op.status,Status::Accepted|Status::Unknown|Status::Dispatching|Status::Confirmed))
                .filter_map(|op|op.payload["logical_id"].as_str()).collect();
            let protected:BTreeSet<_> = state.operations.values().filter(|op|matches!(op.status,Status::Accepted|Status::Unknown|Status::Dispatching|Status::Confirmed|Status::Rejected)
                || phases.contains(op.id.as_str())).filter_map(|op|op.entry_id.as_deref()).collect();
            let claimed:BTreeSet<_> = state.operations.iter().filter(|(call,op)|op.payload["kind"]=="claim_batch" || ["call::terminal:queue:","call::claim:","call::steer-claim:"]
                    .iter().any(|prefix|call.starts_with(prefix)) && op.payload["kind"]=="claim")
                .flat_map(|(_,op)|op.result.as_array().into_iter().flatten()).filter_map(|row|row["id"].as_str()).collect();
            // Só o claim do executor (terminal ou drain sem terminal), sem despacho, prova ausência de efeito.
            for row in &mut state.rows {
                let id=row_id(row);
                if row["delivered"]==true && row["confirmed"]!=true && row["desistiu"]!=true && claimed.contains(id) && !protected.contains(id) {
                    row["delivered"]=json!(false);
                }
            }
            Value::Null
        }
        Action::SetRuntimeState { state: runtime_state } => {
            if !runtime_state.is_object() { return Err(invalid("estado privado inválido")); }
            let barrier=state.runtime_state["terminal_write_barrier"].clone();
            let life=state.runtime_state[CODEX_LIFE].clone();
            state.runtime_state = runtime_state;
            if !barrier.is_null() {state.runtime_state["terminal_write_barrier"]=barrier;}
            // A vida da adoção é da vigia, não da vista que o ator grava.
            if !life.is_null() {state.runtime_state[CODEX_LIFE]=life;}
            Value::Null
        }
        Action::AdvanceLifecycle { generation, state:next } => {
            let advanced=generation!=state.generation;
            if advanced && state.generation.checked_add(1)!=Some(generation) || !next.is_object() {return Err(invalid("avanço de vida inválido"));}
            let mut patch=next.as_object().unwrap().clone();
            if let Some(reset)=patch.remove("conversation_reset") {
                let previous=reset["previous"].as_str().filter(|sid|!sid.is_empty()).ok_or_else(||invalid("conversa anterior inválida"))?;
                let next=reset["next"].as_str().filter(|sid|!sid.is_empty()).ok_or_else(||invalid("conversa nova inválida"))?;
                if !advanced || previous==next || reset.as_object().is_none_or(|o|o.len()!=2)
                    || state.runtime_state["view"]["conversation"]!=previous || patch.get("metadata").and_then(|metadata|metadata["session_id"].as_str())!=Some(next) {return Err(invalid("troca de conversa inválida"));}
                state.rows.clear();
            }
            let barrier=state.runtime_state["terminal_write_barrier"].clone();
            if patch.get("terminal_write_barrier").is_some_and(|b|b!=&barrier) {return Err(invalid("avanço removeria uma entrega incerta"));}
            if let Some(name)=patch.remove("name") {
                let name=name.as_str().filter(|n|!n.is_empty()).ok_or_else(||invalid("nome vazio na fila"))?;
                if state.name!=name {state.runtime_state["_queue_previous_name"]=json!(state.name);state.name=name.into();}
            }
            let runtime=state.runtime_state.as_object_mut().ok_or_else(||invalid("estado privado inválido"))?;
            if advanced {
                for key in ["view","channels","preview","thinking","tool","alive","working","deliverable","error","terminal_lifecycle"] {runtime.remove(key);}
            }
            runtime.extend(patch);
            if !barrier.is_null() {state.runtime_state["terminal_write_barrier"]=barrier;}
            state.generation=generation;
            json!({"generation":generation})
        }
        Action::ReplaceRows { rows } => {
            if state.rows.iter().any(|r| protected.contains(row_id(r)) && !rows.contains(r)) {
                return Err(invalid("substituição removeria uma entrada incerta"));
            }
            state.rows = rows;
            Value::Null
        }
        Action::Reconcile { committed, min_ts, now, grace, max_attempts, confirm_only, na_fila_tui } => {
            reconcile(state,&protected,committed,min_ts,now,grace,max_attempts,confirm_only,na_fila_tui)
        }
    };
    Ok(result)
}

/// Entregue antes de o Rust assumir a sessão: nenhuma operação a liga a um cursor de despacho.
/// A reivindicada pelo próprio drain ainda não foi escrita, e a desistida não chegou: nenhuma entra.
/// A mais recente vem primeiro, para a perdida mais antiga não levar a linha da que chegou.
pub(crate) fn legacy_rows(state: &State) -> Vec<Value> {
    let dispatched: BTreeSet<&str> = state.operations.values().filter_map(|op|op.entry_id.as_deref()).collect();
    let claimed: BTreeSet<&str> = state.operations.iter().filter(|(key,op)|key.starts_with(CALL_PREFIX) && matches!(op.payload["kind"].as_str(),Some("claim"|"claim_batch")))
        .flat_map(|(_,op)|op.result.as_array().into_iter().flatten()).filter_map(|row|row["id"].as_str()).collect();
    let mut rows: Vec<Value> = state.rows.iter().filter(|r|r["delivered"] == true && r["confirmed"] != true && r["desistiu"] != true
        && r["papel"] != "assistant" && !r["batch_entries"].is_array() && !dispatched.contains(row_id(r)) && !claimed.contains(row_id(r))).cloned().collect();
    rows.sort_by(|a,b|b["ts"].as_f64().unwrap_or(0.0).total_cmp(&a["ts"].as_f64().unwrap_or(0.0)));
    rows
}

pub(crate) fn entry_lines(row: &Value) -> BTreeSet<String> {
    let raw = row["text"].as_str().unwrap_or("").trim();
    let stripped = crate::transcript::history::strip_attach(raw);
    let mut lines: BTreeSet<_> = if row["batch_entries"].is_array() && row["batch_image_receipt"].is_string() {
        // Lote do terminal (nunca guarda `batch_prompt`): a TUI pode deixar o path como texto, sem anexar a imagem,
        // e aí só o texto inteiro aparece. Sem terminal, a imagem enviada pelo próprio servidor tem de constar.
        let text = (!row["batch_prompt"].is_object() && !raw.is_empty()).then(|| raw.to_owned());
        ["batch_image_receipt","batch_image_source_receipt"].into_iter()
            .filter_map(|key|row[key].as_str().map(str::to_owned)).chain(text).collect()
    } else if row["batch_entries"].is_array() {
        // Só o pedido completo confirma o lote, mesmo se uma parte isolada já apareceu antes.
        std::iter::once(raw.to_owned()).filter(|text|!text.is_empty()).collect()
    } else {
        [raw,stripped.trim()].into_iter().flat_map(|text|std::iter::once(text).chain(text.split('\n')))
            .map(str::trim).filter(|text|!text.is_empty()).map(str::to_owned).collect()
    };
    let shrunk: Vec<_> = lines.iter().map(|text| {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' { out.push(c); continue; }
            let mut count: usize = 1;
            while chars.peek() == Some(&'\\') { chars.next(); count += 1; }
            out.extend(std::iter::repeat_n('\\',count.div_ceil(2)));
        }
        out
    }).collect();
    lines.extend(shrunk);
    lines
}

#[allow(clippy::too_many_arguments)]
fn reconcile(state: &mut State, protected: &BTreeSet<String>, committed: Vec<String>, min_ts: f64,
    now: f64, grace: f64, max_attempts: u64, confirm_only: bool, pending: Vec<String>) -> Value {
    let mut available: BTreeSet<String> = committed.into_iter().collect();
    let rows_before = state.rows.clone();
    let mut reserved = BTreeSet::new();
    let mut owners: BTreeMap<String,String> = BTreeMap::new();
    for row in &state.rows {
        if row["delivered"] != true || row["confirmed"] == true || protected.contains(row_id(row)) { continue; }
        let lines = entry_lines(row);
        reserved.extend(lines.intersection(&available).cloned());
        for line in lines.iter().filter(|l|l.chars().count() >= 8) {
            for echo in available.iter().filter(|e|e.starts_with(line.as_str())) {
                if owners.get(echo).is_none_or(|old|old.chars().count() < line.chars().count()) { owners.insert(echo.clone(),line.clone()); }
            }
        }
    }
    let mut requeued = Vec::new();
    let mut removed = BTreeSet::new();
    for row in &mut state.rows {
        if row["delivered"] != true || row["confirmed"] == true || protected.contains(row_id(row)) { continue; }
        if row["desistiu"] == true {
            if !current(row,min_ts) { continue; }
            let cutoff = row["desistiu_ts"].as_f64().unwrap_or(row["ts"].as_f64().unwrap_or(0.0)+60.0);
            if rows_before.iter().any(|other|row_id(other) != row_id(row) && other["text"] == row["text"] && other["ts"].as_f64().unwrap_or(0.0) > cutoff) {
                removed.insert(row_id(row).to_owned()); continue;
            }
        } else {
            if !current(row,min_ts) { row["confirmed"] = json!(true); continue; }
            if now - row["ts"].as_f64().unwrap_or(0.0) < grace { continue; }
        }
        let lines = entry_lines(row);
        if lines.iter().any(|l|pending.contains(l)) { continue; }
        let mut consumed = BTreeSet::new();
        for line in lines {
            if available.contains(&line) { consumed.insert(line); }
            else if line.chars().count() >= 8 {
                if let Some(echo) = available.iter().filter(|e|e.starts_with(&line) && !reserved.contains(*e)
                    && owners.get(*e) == Some(&line)).min_by_key(|e|e.chars().count()) { consumed.insert(echo.clone()); }
            }
        }
        if !consumed.is_empty() || (row["desistiu"] != true && row["text"].as_str().unwrap_or("").trim().is_empty()) {
            for echo in consumed { available.remove(&echo); }
            row["confirmed"] = json!(true); row.as_object_mut().unwrap().remove("desistiu");
        } else if row["desistiu"] == true || confirm_only || row["steered"] == true { continue; }
        else if row["attempts"].as_u64().unwrap_or(0) >= max_attempts {
            row["desistiu"] = json!(true); row["desistiu_ts"] = json!(now);
        } else {
            row["delivered"] = json!(false);
            row["attempts"] = json!(row["attempts"].as_u64().unwrap_or(0)+1);
            requeued.push(row.clone());
        }
    }
    state.rows.retain(|r|!removed.contains(row_id(r)));
    json!(requeued)
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Lê um arquivo que outro escritor troca por `atomic_write`. No Windows quem abre no instante da
/// troca recebe NotFound ou acesso negado, inclusive na troca POSIX: ausente ou recusado de verdade é o
/// que continua assim depois da espera curta. As primeiras tentativas não dependem do relógio (a
/// thread preemptada perderia a espera inteira entre duas leituras) e só cedem a vez, porque o sono
/// de 1 ms dura um tique (~15 ms); depois dorme, para o arquivo ausente de verdade não girar a CPU.
pub fn read_replaced(path: &Path) -> io::Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match std::fs::read(path) {
            Err(error) if cfg!(windows) && matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied)
                && (attempts <= 100 || std::time::Instant::now() < deadline) => if attempts <= 100 {
                    std::thread::yield_now()
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                },
            outcome => return outcome,
        }
    }
}
pub fn read_replaced_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read_replaced(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// `rename` que sobrevive ao Windows: lá um leitor aberto sem FILE_SHARE_DELETE (o CLI lendo
/// o JSON) recusa a troca por um instante. No POSIX a troca nunca falha por leitor aberto.
pub(crate) fn replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    for wait in [20,40,60,80,100,120] {
        match std::fs::rename(from,to) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => std::thread::sleep(std::time::Duration::from_millis(wait)),
            Err(error) => return Err(error),
        }
    }
    std::fs::rename(from,to)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(||invalid("arquivo sem diretório"))?;
    let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_err(|_|invalid("relógio do arquivo temporário inválido"))?.as_nanos();
    let temp = parent.join(format!(".queue-{}-{tick}-{}.tmp",std::process::id(),TEMP_ID.fetch_add(1,Ordering::Relaxed)));
    let mut created = false;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp)?;
        created = true;
        file.write_all(bytes)?; file.sync_all()?; drop(file);
        replace(&temp,path)?;
        #[cfg(unix)] { File::open(parent)?.sync_all()?; }
        Ok(())
    })();
    if created {
        match std::fs::remove_file(&temp) {
            Err(error) if error.kind() != io::ErrorKind::NotFound && result.is_ok() => return Err(error),
            _ => {}
        }
    }
    result
}

pub(super) type BatchValidator = Box<dyn FnOnce(&mut Value)->io::Result<()> + Send>;
enum QueueMessage {
    Exec { generation:u64, call_id:String, clock:ClockSample, action:Action, hold:bool, validate:Option<BatchValidator>, reply:oneshot::Sender<io::Result<Value>> },
    Stop,
    Snapshot(oneshot::Sender<io::Result<Arc<State>>>),
}

pub struct QueueActor {
    sender: mpsc::Sender<QueueMessage>,
    task: tokio::task::JoinHandle<()>,
    initial: State,
    lease: Arc<Lease>,
}

impl QueueActor {
    pub fn start(store: Store, lease: Arc<Lease>) -> Self {
        let initial = store.state().clone();
        let actor_lease = lease.clone();
        let (sender,mut receiver) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            let mut store = store;
            while let Some(message) = receiver.recv().await {
                match message {
                    QueueMessage::Stop => break,
                    QueueMessage::Snapshot(reply) => {
                        if store.fenced {
                            let job = tokio::task::spawn_blocking(move || { let result = store.unfence(); (store,result) }).await;
                            match job {
                                Ok((next,result)) => { store = next; if let Err(error) = result { let _ = reply.send(Err(error)); continue; } }
                                Err(_) => { let _ = reply.send(Err(invalid("persistência da fila interrompida"))); break; }
                            }
                        }
                        let _ = reply.send(Ok(store.state.clone()));
                    }
                    QueueMessage::Exec { generation,call_id,clock,action,hold,validate,reply } => {
                        let lease = lease.clone();
                        let job = tokio::task::spawn_blocking(move || {
                            let _lease = lease;
                            let result = store.exec_validated(generation,&call_id,clock,action,hold,validate);
                            (store,result)
                        }).await;
                        match job {
                            Ok((next,result)) => { store = next; let _ = reply.send(result); }
                            Err(_) => { let _ = reply.send(Err(invalid("persistência da fila interrompida"))); break; }
                        }
                    }
                }
            }
        });
        Self { sender,task,initial,lease:actor_lease }
    }

    pub fn initial_state(&self) -> &State { &self.initial }
    pub fn lease(&self) -> Arc<Lease> { self.lease.clone() }

    pub async fn exec(&self, generation:u64, call_id:&str, clock:ClockSample, action:Action) -> io::Result<Value> {
        self.exec_held(generation,call_id,clock,action,false).await
    }

    pub async fn exec_held(&self, generation:u64, call_id:&str, clock:ClockSample, action:Action, hold:bool) -> io::Result<Value> {
        self.exec_validated(generation,call_id,clock,action,hold,None).await
    }

    pub(super) async fn exec_validated(&self,generation:u64,call_id:&str,clock:ClockSample,action:Action,hold:bool,validate:Option<BatchValidator>) -> io::Result<Value> {
        let (reply,response) = oneshot::channel();
        self.sender.send(QueueMessage::Exec { generation,call_id:call_id.into(),clock,action,hold,validate,reply }).await
            .map_err(|_|invalid("fila encerrada"))?;
        response.await.map_err(|_|invalid("fila encerrada sem recibo"))?
    }

    pub async fn snapshot(&self) -> io::Result<Arc<State>> {
        let (reply,response) = oneshot::channel();
        self.sender.send(QueueMessage::Snapshot(reply)).await.map_err(|_|invalid("fila encerrada"))?;
        response.await.map_err(|_|invalid("fila encerrada sem estado"))?
    }

    pub async fn shutdown(self) -> io::Result<()> {
        self.sender.send(QueueMessage::Stop).await.map_err(|_|invalid("fila já encerrada"))?;
        self.task.await.map_err(|_|invalid("persistência da fila interrompida"))
    }
}

#[cfg(test)]
mod headless_delivery_tests {
    use super::*;
    fn store(dir:&tempfile::TempDir) -> Store {
        Store::open(&dir.path().join("state.json"),dir.path(),State::new("key",1,"session",vec![])).unwrap()
    }
    fn clock(epoch_s:f64) -> ClockSample { ClockSample { monotonic_s:0.0,epoch_s } }
    fn append(store:&mut Store,id:&str,ts:f64) {
        store.exec(1,&format!("append:{id}"),clock(ts),Action::Append { text:id.into(),delivered:false,ts:Some(ts),pre_transcript:false,entry_id:Some(id.into()) }).unwrap();
    }
    fn row(store:&Store,id:&str) -> Value { store.state().rows.iter().find(|r|r["id"] == id).unwrap().clone() }

    /// A leitura da fila não copia o estado: duas leituras sem escrita entre elas são o mesmo; a escrita troca.
    #[tokio::test]
    async fn a_snapshot_shares_the_state_until_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let queue = QueueActor::start(store(&dir),acquire_lease(&dir.path().join("lease")).unwrap());
        let (a,b) = (queue.snapshot().await.unwrap(),queue.snapshot().await.unwrap());
        assert!(std::sync::Arc::ptr_eq(&a,&b),"sem escrita, o mesmo estado");
        queue.exec(1,"append:x",clock(1.0),Action::Append { text:"x".into(),delivered:false,ts:Some(1.0),pre_transcript:false,entry_id:Some("x".into()) }).await.unwrap();
        let c = queue.snapshot().await.unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a,&c) && c.rows.len() == a.rows.len() + 1,"a escrita troca, e a leitura velha fica como era");
        queue.shutdown().await.unwrap();
    }

    #[test]
    fn headless_claim_that_never_dispatched_goes_back_to_the_queue_on_recovery() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a",10.0);
        store.exec(1,"claim:x",clock(11.0),Action::Claim { min_ts:0.0,limit:Some(1),entry_id:None }).unwrap();
        store.exec(1,"stop-recover:x",clock(12.0),Action::Recover).unwrap();
        assert_eq!(row(&store,"a")["delivered"],false,"reivindicada e nunca despachada ficou como entregue");
    }

    #[test]
    fn steered_entry_never_gives_up() {
        // Decisão do usuário (paridade com o Python): orientação sem prova não vira "não chegou".
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"steer",10.0);
        store.exec(1,"steered",clock(11.0),Action::SetDelivered { entry_id:"steer".into(),value:true,steered:true }).unwrap();
        let outcome=store.exec(1,"give-up",clock(500.0),Action::GiveUpUnproved { now:500.0,grace:60.0 }).unwrap();
        assert_eq!(outcome["given_up"],json!([]));
        assert!(row(&store,"steer")["desistiu"].is_null());
    }

    #[test]
    fn prompt_hooks_started_mark_only_a_delivered_unconfirmed_row_as_consumed() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"sent",10.0); append(&mut store,"waiting",10.5);
        store.exec(1,"prepare",clock(11.0),Action::Prepare { id:"sent".into(),payload:json!({"kind":"input"}),entry_id:Some("sent".into()) }).unwrap();
        store.exec(1,"begin",clock(11.0),Action::BeginDispatch { id:"sent".into(),wire_id:"w".into(),staged:false }).unwrap();
        assert_eq!(store.exec(1,"started",clock(11.2),Action::MarkConsumed { entry_ids:vec!["sent".into(),"waiting".into()] }).unwrap(),json!(1));
        assert_eq!(row(&store,"sent")["consumed"],true);
        assert!(row(&store,"waiting")["consumed"].is_null(),"na fila do Hangar não começou hook nenhum");
        store.exec(1,"confirm",clock(12.0),Action::Confirm { entry_ids:vec!["sent".into()] }).unwrap();
        assert_eq!(store.exec(1,"again",clock(12.1),Action::MarkConsumed { entry_ids:vec!["sent".into()] }).unwrap(),json!(0));
    }

    #[test]
    fn local_command_answered_before_the_write_finished_is_confirmed() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"/cost",10.0);
        store.exec(1,"prepare",clock(11.0),Action::Prepare { id:"/cost".into(),payload:json!({"kind":"input"}),entry_id:Some("/cost".into()) }).unwrap();
        store.exec(1,"begin",clock(11.0),Action::BeginDispatch { id:"/cost".into(),wire_id:"w".into(),staged:false }).unwrap();
        // O `result` local chegou antes do `Finish` da escrita.
        assert_eq!(store.exec(1,"local-confirm",clock(12.0),Action::Confirm { entry_ids:vec!["/cost".into()] }).unwrap(),json!(1));
        assert_eq!(row(&store,"/cost")["confirmed"],true);
    }

    #[test]
    fn give_up_counts_from_the_dispatch_and_frees_the_uncertain_entry() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        // Esperou na fila muito tempo e foi despachada agora: o prazo conta do despacho.
        append(&mut store,"late",10.0);
        append(&mut store,"lost",10.0);
        append(&mut store,"writing",10.0);
        for id in ["late","lost","writing"] {
            store.exec(1,&format!("prepare:{id}"),clock(100.0),Action::Prepare { id:id.into(),payload:json!({"kind":"input"}),entry_id:Some(id.into()) }).unwrap();
        }
        store.exec(1,"begin:lost",clock(100.0),Action::BeginDispatch { id:"lost".into(),wire_id:"w-lost".into(),staged:false }).unwrap();
        store.exec(1,"finish:lost",clock(100.0),Action::Finish { id:"lost".into(),status:Status::Unknown,result:json!({}) }).unwrap();
        store.exec(1,"begin:writing",clock(100.0),Action::BeginDispatch { id:"writing".into(),wire_id:"w-writing".into(),staged:false }).unwrap();
        store.exec(1,"begin:late",clock(150.0),Action::BeginDispatch { id:"late".into(),wire_id:"w-late".into(),staged:false }).unwrap();
        store.exec(1,"finish:late",clock(150.0),Action::Finish { id:"late".into(),status:Status::Accepted,result:json!({}) }).unwrap();
        let outcome=store.exec(1,"give-up",clock(170.0),Action::GiveUpUnproved { now:170.0,grace:60.0 }).unwrap();
        assert_eq!(outcome,json!({"given_up":["lost"],"next_due":210.0}));
        assert_eq!(row(&store,"lost")["desistiu"],true);
        assert!(row(&store,"late")["desistiu"].is_null() && row(&store,"writing")["desistiu"].is_null());
        assert_eq!(store.state().operations["lost"].status,Status::Rejected,"a incerta desistida não segura mais o descarte");
        assert_eq!(store.exec(1,"remove",clock(171.0),Action::Remove { entry_id:"lost".into() }).unwrap(),json!(true));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn sid_commit_clears_a_and_advances_b_in_one_store_write() {
        let dir=tempfile::tempdir().unwrap();
        let mut initial=State::new("key",4,"session",vec![json!({"id":"a","text":"only-a","delivered":false,"ts":1.0})]);
        initial.runtime_state["view"]=json!({"conversation":"sid-a","deliverable":true});
        let mut store=Store::open(&dir.path().join("state.json"),dir.path(),initial).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:10.0};
        let action=||Action::AdvanceLifecycle {generation:5,state:json!({"conversation_reset":{"previous":"sid-a","next":"sid-b"},"metadata":{"key":"key","session_id":"sid-b"}})};
        store.exec(4,"sid-commit",clock,action()).unwrap();
        assert_eq!(store.state().generation,5);assert!(store.state().rows.is_empty());
        assert_eq!(store.state().runtime_state["metadata"]["session_id"],"sid-b");
        assert!(store.state().runtime_state["view"].is_null());
        assert!(store.state().runtime_state.get("conversation_reset").is_none());
        assert!(store.exec(4,"old-claim",clock,Action::Claim {min_ts:0.0,limit:None,entry_id:None}).is_err());
        assert!(store.exec(5,"b-claim",clock,Action::Claim {min_ts:0.0,limit:None,entry_id:None}).unwrap().as_array().unwrap().is_empty());
        assert_eq!(store.exec(4,"sid-commit",clock,action()).unwrap(),json!({"generation":5}));
    }
    #[test]
    fn advance_preserves_unknown_and_only_repeats_the_same_transition() {
        let dir=tempfile::tempdir().unwrap();
        let mut initial=State::new("key",4,"session",vec![json!({"id":"row","text":"pending","delivered":true,"confirmed":false,"desistiu":false,"ts":1000.0})]);
        let mut unknown=Operation::new("input",json!({"kind":"input"}),Some("row".into()));
        unknown.status=Status::Unknown;
        initial.operations.insert("input".into(),unknown);
        initial.runtime_state["terminal_write_barrier"]=json!({"conversation":"sid","generation":4,"operation_id":"input"});
        initial.runtime_state["view"]=json!({"working":true});
        let before=initial.runtime_state["terminal_write_barrier"].clone();
        let mut store=Store::open(&dir.path().join("state.json"),dir.path(),initial).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1000.0};
        assert_eq!(store.exec(4,"same-life",clock,Action::AdvanceLifecycle {generation:4,state:json!({"name":"renamed","metadata":{"setting":true}})}).unwrap(),json!({"generation":4}));
        assert_eq!(store.state().name,"renamed");
        assert_eq!(store.state().runtime_state["_queue_previous_name"],"session");
        assert_eq!(store.state().runtime_state["view"],json!({"working":true}));
        assert!(store.state().runtime_state.get("name").is_none());
        let advance=||Action::AdvanceLifecycle {generation:5,state:json!({"terminal_lifecycle":{"conversation":"sid"}})};
        assert_eq!(store.exec(4,"advance",clock,advance()).unwrap(),json!({"generation":5}));
        assert_eq!(store.exec(4,"advance",clock,advance()).unwrap(),json!({"generation":5}));
        assert!(store.exec(4,"another",clock,advance()).is_err());
        assert!(store.exec(4,"advance",clock,Action::AdvanceLifecycle {generation:5,state:json!({"other":true})}).is_err());
        assert!(store.exec(5,"skip",clock,Action::AdvanceLifecycle {generation:7,state:json!({})}).is_err());
        assert_eq!(store.state().runtime_state["terminal_write_barrier"],before);
        assert!(store.state().runtime_state["view"].is_null());
        assert_eq!(store.state().runtime_state["metadata"],json!({"setting":true}));
        assert_eq!(store.state().name,"renamed");
        assert_eq!(store.state().operations["input"].status,Status::Unknown);
        assert_eq!(store.state().rows.len(),1);
        assert!(store.exec(5,"erase",clock,Action::AdvanceLifecycle {generation:6,state:json!({"terminal_write_barrier":null})}).is_err());
        assert!(store.exec(5,"claim",clock,Action::Claim {min_ts:0.0,limit:None,entry_id:None}).unwrap().as_array().unwrap().is_empty());
    }
    #[test]
    fn advance_receipt_keeps_a_fingerprint_of_the_view_not_the_view() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("state.json");
        let mut store=Store::open(&path,dir.path(),State::new("key",4,"session",vec![])).unwrap();
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1000.0};
        let commands:Vec<Value>=(0..200).map(|i|json!({"name":format!("/command-{i}"),"description":"x".repeat(400)})).collect();
        let advance=|state:Value|Action::AdvanceLifecycle {generation:5,state};
        store.exec(4,"advance",clock,advance(json!({"view":{"commands":commands},"metadata":{"a":1,"b":2}}))).unwrap();
        let stored=&store.state().operations["call::advance"].payload["state"];
        assert_eq!(stored.as_str().map(str::len),Some(64));
        assert!(!serde_json::to_string(&store.state().operations["call::advance"]).unwrap().contains("/command-0"));
        assert_eq!(String::from_utf8(std::fs::read(&path).unwrap()).unwrap().matches("\"/command-0\"").count(),1);
        assert_eq!(store.exec(4,"advance",clock,advance(json!({"metadata":{"b":2,"a":1},"view":{"commands":commands}}))).unwrap(),json!({"generation":5}));
        assert!(store.exec(4,"advance",clock,advance(json!({"view":{"commands":[]},"metadata":{"a":1,"b":2}}))).is_err());
    }
    #[test]
    fn advance_receipt_written_with_the_whole_view_shrinks_on_load_and_still_repeats() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("state.json");
        let view=json!({"view":{"commands":[{"name":"/old","description":"y".repeat(400)}]}});
        let mut old=State::new("key",5,"session",vec![]);
        let mut receipt=Operation::new("call::advance",json!({"kind":"advance_lifecycle","generation":5,"state":view}),None);
        receipt.status=Status::Accepted; receipt.result=json!({"generation":5}); receipt.seq=1; old.next_seq=2;
        old.operations.insert("call::advance".into(),receipt);
        std::fs::write(&path,serde_json::to_vec(&old).unwrap()).unwrap();
        let mut store=Store::open(&path,dir.path(),State::new("key",5,"session",vec![])).unwrap();
        assert!(store.state().operations["call::advance"].payload["state"].is_string());
        let clock=ClockSample {monotonic_s:0.0,epoch_s:1000.0};
        assert_eq!(store.exec(4,"advance",clock,Action::AdvanceLifecycle {generation:5,state:view}).unwrap(),json!({"generation":5}));
    }
}

#[cfg(all(test, windows))]
mod replaced_read_tests {
    use super::*;
    #[test]
    fn a_file_being_replaced_never_reads_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.queue-state.json");
        atomic_write(&path, b"{}").unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = { let (path, stop) = (path.clone(), stop.clone()); std::thread::spawn(move || {
            // A troca de produção sem o fsync do `atomic_write`: o que está à prova é o leitor, e mais trocas por segundo.
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                let temp = path.with_extension(format!("{n}.tmp"));
                std::fs::write(&temp, format!("{{\"n\":{n}}}")).unwrap();
                replace(&temp, &path).unwrap();
            }
        }) };
        let (mut reads, mut missing) = (0u32, 0u32);
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(3) {
            match read_replaced(&path) {
                Ok(_) => reads += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => missing += 1,
                Err(error) => panic!("leitura recusada: {error}"),
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert!(reads > 100, "poucas leituras para provar algo: {reads}");
        assert_eq!(missing, 0, "o arquivo trocado sumiu em {missing} de {} leituras", reads + missing);
    }

    /// Leitor aberto que compartilha a exclusão (o `atomico.read` do Python, o `std::fs::read`) não faz a
    /// troca falhar: a outra metade da #145, agora que o Rust grava o sidecar do Claude.
    #[test]
    fn a_reader_that_shares_deletion_does_not_block_the_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (path, temp) = (dir.path().join("cl.json"), dir.path().join("cl.tmp"));
        std::fs::write(&path, b"velho").unwrap();
        std::fs::write(&temp, b"novo").unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        replace(&temp, &path).unwrap();
        let mut old = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut old).unwrap();
        assert_eq!(old, b"velho");
        assert_eq!(std::fs::read(&path).unwrap(), b"novo");
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;
    fn store(dir:&tempfile::TempDir) -> Store {
        Store::open(&dir.path().join("state.json"),dir.path(),State::new("key",1,"session",vec![])).unwrap()
    }
    fn clock(epoch_s:f64) -> ClockSample { ClockSample { monotonic_s:0.0,epoch_s } }
    fn append(store:&mut Store,id:&str) {
        store.exec(1,&format!("append:{id}"),clock(10.0),Action::Append { text:format!("texto {id}"),delivered:false,ts:Some(10.0),pre_transcript:false,entry_id:Some(id.into()) }).unwrap();
    }
    fn exec(store:&mut Store,call:&str,action:Action) -> Value { store.exec(1,call,clock(20.0),action).unwrap() }

    /// Duas escritas do mesmo passo lógico: a segunda chega depois do despacho da primeira e não derruba o
    /// ator; o cursor que vale é o da primeira. O amarrar estrito continua recusando.
    #[test]
    fn a_late_conditional_bind_keeps_the_first_cursor() {
        let dir=tempfile::tempdir().unwrap();let mut store=store(&dir);
        exec(&mut store,"prepare",Action::Prepare { id:"phase".into(),payload:json!({"kind":"phase"}),entry_id:None });
        exec(&mut store,"bind-a",Action::BindDispatchIfPrepared { id:"phase".into(),cursor:json!({"from":"a"}) });
        exec(&mut store,"dispatch-a",Action::BeginDispatch { id:"phase".into(),wire_id:"a".into(),staged:false });
        let late=exec(&mut store,"bind-b",Action::BindDispatchIfPrepared { id:"phase".into(),cursor:json!({"from":"b"}) });
        assert_eq!(late["dispatch_cursor"],json!({"from":"a"}),"o cursor da primeira escrita fica");
        assert!(store.exec(1,"bind-c",clock(20.0),Action::BindDispatch { id:"phase".into(),cursor:json!({}) }).is_err(),"o estrito segue recusando");
    }

    #[test]
    fn cancel_removes_only_a_waiting_entry() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a"); append(&mut store,"b");
        assert_eq!(exec(&mut store,"cancel-a",Action::Cancel { entry_id:"a".into() }),json!("cancelled"));
        assert_eq!(waiting_ids(store.state(),0.0),vec!["b".to_string()]);
        assert_eq!(exec(&mut store,"cancel-x",Action::Cancel { entry_id:"x".into() }),json!("absent"));
    }

    #[test]
    fn waiting_ids_cut_rows_older_than_the_life_like_the_claim() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"old");
        assert_eq!(waiting_ids(store.state(),0.0),vec!["old".to_string()]);
        // A vida começou depois dela (ts 10): a drenagem nunca a levaria, e ela não segura comando nem Orientar.
        assert!(waiting_ids(store.state(),50.0).is_empty());
    }

    #[test]
    fn a_never_written_terminal_intent_of_an_older_life_still_waits() {
        // Queda e reabertura numa vida nova: a intenção preparada antes não escreveu nada.
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a");
        let command=json!({"operation_id":"a","kind":"input","payload":{"text":"texto a","pre_transcript":false,"_terminal_generation":0}});
        exec(&mut store,"prepare",Action::Prepare { id:"a".into(),payload:command,entry_id:Some("a".into()) });
        assert_eq!(waiting_ids(store.state(),0.0),vec!["a".to_string()]);
        assert_eq!(exec(&mut store,"move",Action::Promote { entry_id:"a".into() }),json!("moved"));
    }

    #[test]
    fn edit_and_cancel_refuse_a_claimed_entry() {
        // A drenagem reivindicou a entrada no meio: o painel recusa em vez de mexer no que já saiu.
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a");
        exec(&mut store,"claim",Action::Claim { min_ts:0.0,limit:Some(1),entry_id:None });
        assert_eq!(exec(&mut store,"edit",Action::Edit { entry_id:"a".into(),text:"novo".into() }),json!("not_waiting"));
        assert_eq!(exec(&mut store,"cancel",Action::Cancel { entry_id:"a".into() }),json!("not_waiting"));
        assert_eq!(exec(&mut store,"move",Action::Promote { entry_id:"a".into() }),json!("not_waiting"));
        assert_eq!(store.state().rows[0]["text"],"texto a");
    }

    #[test]
    fn edit_rewrites_the_deferred_intent_so_the_drain_sends_the_new_text() {
        // Sem terminal o ator reenvia o comando gravado na operação: ele precisa levar o texto novo.
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a");
        let command=json!({"operation_id":"a","kind":"input","payload":{"text":"texto a","pre_transcript":false}});
        exec(&mut store,"prepare",Action::Prepare { id:"a".into(),payload:command,entry_id:Some("a".into()) });
        exec(&mut store,"finish",Action::Finish { id:"a".into(),status:Status::Deferred,result:json!({"operation_id":"a","disposition":"deferred","payload":{}}) });
        assert_eq!(exec(&mut store,"edit",Action::Edit { entry_id:"a".into(),text:"texto novo".into() }),json!("edited"));
        assert_eq!(store.state().rows[0]["text"],"texto novo");
        assert_eq!(store.state().operations["a"].payload["payload"]["text"],"texto novo");
        // A reentrega prepara a mesma intenção, agora com o texto novo, sem "intenção da operação mudou".
        let again=json!({"operation_id":"a","kind":"input","payload":{"text":"texto novo","pre_transcript":false}});
        assert!(store.exec(1,"prepare-again",clock(21.0),Action::Prepare { id:"a".into(),payload:again,entry_id:Some("a".into()) }).is_ok());
    }

    #[test]
    fn edit_refuses_empty_slash_and_control_text() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a");
        for (call,text) in [("e1","   "),("e2","/clear"),("e3","a\u{1b}b")] {
            let error=store.exec(1,call,clock(20.0),Action::Edit { entry_id:"a".into(),text:text.into() }).unwrap_err();
            assert_eq!(refusal(&error),Some("texto da fila inválido"),"{text:?}");
        }
        assert_eq!(store.state().rows[0]["text"],"texto a");
    }

    #[test]
    fn reorder_moves_among_waiting_and_end_is_none() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        for id in ["a","b","c"] { append(&mut store,id); }
        assert_eq!(exec(&mut store,"r1",Action::Reorder { entry_id:"c".into(),before_id:Some("a".into()) }),json!("moved"));
        assert_eq!(waiting_ids(store.state(),0.0),vec!["c","a","b"]);
        assert_eq!(exec(&mut store,"r2",Action::Reorder { entry_id:"c".into(),before_id:None }),json!("moved"));
        assert_eq!(waiting_ids(store.state(),0.0),vec!["a","b","c"]);
        assert_eq!(exec(&mut store,"r3",Action::Promote { entry_id:"b".into() }),json!("moved"));
        assert_eq!(waiting_ids(store.state(),0.0),vec!["b","a","c"]);
        // A drenagem segue a ordem nova.
        let claimed=exec(&mut store,"claim",Action::Claim { min_ts:0.0,limit:Some(1),entry_id:None });
        assert_eq!(claimed[0]["id"],"b");
    }

    #[test]
    fn reorder_before_a_delivered_entry_is_not_waiting() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"a"); append(&mut store,"b");
        exec(&mut store,"claim",Action::Claim { min_ts:0.0,limit:Some(1),entry_id:Some("a".into()) });
        assert_eq!(exec(&mut store,"r",Action::Reorder { entry_id:"b".into(),before_id:Some("a".into()) }),json!("not_waiting"));
    }

    #[test]
    fn reserved_answer_is_not_a_waiting_entry() {
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        exec(&mut store,"reserve",Action::ReserveAsyncAnswer { entry_id:"ans".into(),text:"resposta".into(),payload:json!({"kind":"dispatch_async_answer"}) });
        assert_eq!(store.state().rows[0]["reserved"],true);
        assert!(waiting_ids(store.state(),0.0).is_empty());
        assert_eq!(exec(&mut store,"cancel",Action::Cancel { entry_id:"ans".into() }),json!("not_waiting"));
    }

    #[test]
    fn cancel_never_touches_an_uncertain_delivery() {
        // O X do painel não é o "Descartar" da bolha perdida: a incerta continua na fila, sem desistência.
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        append(&mut store,"u");
        exec(&mut store,"prepare",Action::Prepare { id:"u".into(),payload:json!({"kind":"input","payload":{"text":"texto u"}}),entry_id:Some("u".into()) });
        exec(&mut store,"begin",Action::BeginDispatch { id:"u".into(),wire_id:"w".into(),staged:false });
        exec(&mut store,"finish",Action::Finish { id:"u".into(),status:Status::Unknown,result:json!({}) });
        assert_eq!(exec(&mut store,"cancel",Action::Cancel { entry_id:"u".into() }),json!("not_waiting"));
        assert_eq!(store.state().rows.len(),1);
        assert!(store.state().rows[0]["desistiu"].is_null());
    }

    #[test]
    fn cancelled_terminal_intent_does_not_come_back_on_recovery() {
        // A intenção terminal preparada e adiada sai junto: o Recover não rematerializa a linha.
        let dir=tempfile::tempdir().unwrap(); let mut store=store(&dir);
        let intent=json!({"operation_id":"t","kind":"input","payload":{"text":"oi","pre_transcript":false,"_terminal_generation":1}});
        exec(&mut store,"prepare",Action::Prepare { id:"t".into(),payload:intent,entry_id:Some("t".into()) });
        store.exec(1,"append",clock(10.0),Action::Append { text:"oi".into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some("t".into()) }).unwrap();
        // Adiada como a mensagem do turno em curso, antes de ser apagada no painel.
        exec(&mut store,"finish",Action::Finish { id:"t".into(),status:Status::Deferred,result:json!({"operation_id":"t","disposition":"deferred","payload":{"code":"queue_ahead"}}) });
        assert_eq!(exec(&mut store,"cancel",Action::Cancel { entry_id:"t".into() }),json!("cancelled"));
        assert!(store.exec(1,"recover",clock(20.0),Action::Recover).is_ok());
        assert!(store.state().rows.is_empty());
    }
}
