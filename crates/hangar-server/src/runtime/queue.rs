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

const CALL_PREFIX: &str = "call::";
const CAP: usize = 1000;
const VERSION: u32 = 2;
/// Janela de chamadas recentes que nunca sai: cobre a repetição da mesma chamada e o ACK atrasado
/// de uma fase. Mesmo valor de `_RECENT_CALLS` em runtime_queue.py.
const RECENT_CALLS: u64 = 256;
const RECEIPT_METADATA: &[&str] = &["native", "message_id", "native_status", "cleanup", "code", "stage",
    "preserve_binding", "queued", "already_confirmed", "disposition", "draft"];
/// Ações cujo resultado é a operação inteira. Mesmo conjunto de `_OPERATION_RECEIPTS` em runtime_queue.py.
const OPERATION_RECEIPTS: &[&str] = &["prepare", "bind_dispatch", "begin_dispatch", "mark_writing", "finish", "late_rpc_resolution"];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    else if action.get("result").is_some() { action["result"] = slim(&action["result"]); }
    action
}

impl State {
    pub fn terminal_write_blocked(&self,conversation:&str)->bool {
        let barrier=&self.runtime_state["terminal_write_barrier"];
        if barrier.is_object() && (barrier["conversation"].is_null() || barrier["conversation"]==conversation) {return true;}
        // A trava gravada é uma só: a incerta de outra conversa também segura a escrita na dela.
        let conversation=Value::from(conversation);
        self.operations.iter().any(|(key,op)|!key.starts_with(CALL_PREFIX) && holds_terminal_write(op,&conversation))
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
        let open_rows: BTreeSet<&str> = self.rows.iter().filter(|r| r["confirmed"] != true).filter_map(|r| r["id"].as_str()).collect();
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
    SetDelivered { entry_id:String, value:bool, steered:bool },
    Abandon { entry_id:String },
    BumpAttempts { entry_id:String },
    EntryDelivered { entry_id:String },
    Confirm { entry_ids:Vec<String> },
    Prune { min_ts:f64 },
    Reconcile { committed:Vec<String>, min_ts:f64, now:f64, grace:f64, max_attempts:u64,
        confirm_only:bool, na_fila_tui:Vec<String> },
    Remove { entry_id:String },
    Clear,
    Rename { name:String },
    Prepare { id:String, payload:Value, entry_id:Option<String> },
    BindDispatch { id:String, cursor:Value },
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
    ReplaceRows { rows:Vec<Value> },
}

pub struct Store {
    state_path: PathBuf,
    projection_dir: PathBuf,
    state: State,
    fenced: bool,
    recover_before_compact: bool,
}

/// Recusa da própria fila: frase fixa, sem caminho nem texto da conversa, então pode ir ao log.
#[derive(Debug)]
pub struct QueueRefusal(pub &'static str);
impl std::fmt::Display for QueueRefusal { fn fmt(&self, f:&mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) } }
impl std::error::Error for QueueRefusal {}

fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, QueueRefusal(message)) }

/// A frase da recusa, se o erro veio da fila; erros do sistema ou do serde ficam só com o tipo.
pub fn refusal(error: &io::Error) -> Option<&'static str> {
    error.get_ref().and_then(|inner|inner.downcast_ref::<QueueRefusal>()).map(|refusal|refusal.0)
}

pub fn acquire_lease(path: &Path) -> io::Result<Arc<File>> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let file = options.open(path)?;
    file.try_lock().map_err(io::Error::from)?;
    Ok(Arc::new(file))
}
fn row_id(row: &Value) -> &str { row["id"].as_str().unwrap_or("") }
fn is_false(value:&bool)->bool {!*value}
fn current(row: &Value, min_ts: f64) -> bool {
    row["ts"].as_f64().unwrap_or(0.0) >= min_ts - if row["pre_transcript"] == true { 900.0 } else { 0.0 }
}
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c,'_' | '-' | '.') { c } else { '-' }).collect()
}

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
        let mut store = Self { state_path:state_path.into(), projection_dir:projection_dir.into(), state, fenced:false, recover_before_compact };
        store.ensure_projection()?;
        Ok(store)
    }

    pub fn state(&self) -> &State { &self.state }

    pub fn ensure_projection(&mut self) -> io::Result<()> {
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
            self.state = State::load(&std::fs::read(&self.state_path)?)?.0;
            self.recover_before_compact=needs_terminal_recovery(&self.state);
            self.ensure_projection()?;
            self.fenced = false;
        }
        Ok(())
    }

    pub fn exec(&mut self, generation: u64, call_id: &str, clock: ClockSample, action: Action) -> io::Result<Value> {
        self.unfence()?;
        if generation != self.state.generation { return Err(invalid("geração da fila mudou")); }
        if call_id.is_empty() { return Err(invalid("operação da fila sem identificador")); }
        let receipt_id = format!("{CALL_PREFIX}{call_id}");
        let payload = receipt_payload(serde_json::to_value(&action)?);
        if let Some(previous) = self.state.operations.get(&receipt_id) {
            if receipt_payload(previous.payload.clone()) != payload { return Err(invalid("identificador reutilizado com outra operação")); }
            let result = previous.result.clone();
            self.ensure_projection()?;
            return Ok(result);
        }
        let readonly = matches!(&action, Action::Load | Action::EntryDelivered { .. } | Action::EnsureProjection);
        if readonly { self.ensure_projection()?; }
        let target = match &action {
            Action::Prepare { id, .. } | Action::BindDispatch { id, .. } | Action::BeginDispatch { id, .. } | Action::MarkWriting { id, .. }
            | Action::Finish { id, .. } | Action::LateRpcResolution { id, .. } | Action::ConfirmOccurrence { id, .. } => Some(id.clone()),
            _ => None,
        };
        let targeted = target.is_some();
        let mut state = self.state.clone();
        if self.recover_before_compact && !readonly && !matches!(&action,Action::Recover) {
            apply(&mut state,Action::Recover,clock,call_id)?;
        }
        let result = apply(&mut state, action, clock, call_id)?;
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
            self.state = state;
            self.fenced = false;
            self.recover_before_compact=false;
            self.ensure_projection()?;
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
            && receipt.result["id"]==entry || receipt.payload["kind"]=="claim"
            && receipt.result.as_array().is_some_and(|rows|rows.iter().any(|row|row["id"]==entry))
            || receipt.payload["kind"]=="remove" && receipt.payload["entry_id"]==entry && receipt.result==true))
}

fn mark_materialized(state:&mut State,entry:&str) {
    let ids:Vec<_>=state.operations.values().filter(|op|op.entry_id.as_deref()==Some(entry) && terminal_input(state,op))
        .map(|op|op.id.clone()).collect();
    for id in ids {state.operations.get_mut(&id).unwrap().entry_materialized=true;}
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
        key.starts_with("call::terminal:queue:") && op.payload["kind"]=="claim"
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
        if text.trim().is_empty() || text.trim_start().starts_with('/') || text.chars().any(|c|c.is_control() && !matches!(c,'\n'|'\t')) {return Err(invalid("intenção terminal inválida"));}
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
        Action::Claim { min_ts, limit, entry_id } => {
            let mut claimed = Vec::new();
            for row in &mut state.rows {
                if protected.contains(row_id(row)) || entry_id.as_deref().is_some_and(|id| id != row_id(row)) { continue; }
                if row["delivered"] == false && current(row,min_ts) {
                    row["delivered"] = json!(true);
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
        Action::BumpAttempts { entry_id } => {
            if let Some(row) = state.rows.iter_mut().find(|r| row_id(r) == entry_id && !protected.contains(&entry_id)) {
                let count = row["attempts"].as_u64().unwrap_or(0) + 1;
                row["attempts"] = json!(count); json!(count)
            } else { json!(0) }
        }
        Action::EntryDelivered { entry_id } => state.rows.iter().find(|r| row_id(r) == entry_id)
            .map(|r|json!(r["delivered"].as_bool().unwrap_or(false))).unwrap_or(Value::Null),
        Action::Confirm { entry_ids } => {
            let mut count = 0;
            for row in &mut state.rows {
                if !protected.contains(row_id(row)) && entry_ids.iter().any(|id| id == row_id(row)) && row["delivered"] == true && row["confirmed"] != true {
                    row["confirmed"] = json!(true); count += 1;
                }
            }
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
        Action::BindDispatch { id, cursor } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if op.status != Status::Prepared { return Err(invalid("cursor precisa preceder o despacho")); }
            op.dispatch_cursor = cursor;
            serde_json::to_value(op)?
        }
        Action::BeginDispatch { id, wire_id, staged } => {
            let op = state.operations.get_mut(&id).ok_or_else(||invalid("operação não preparada"))?;
            if !matches!(op.status,Status::Prepared | Status::Dispatching) { return Err(invalid("operação não pode ser reenviada")); }
            op.wire_attempts.entry(wire_id).or_insert_with(||json!({"status":if staged {"staged"} else {"dispatching"},"result":null}));
            op.status = Status::Dispatching;
            for row in &mut state.rows { if Some(row_id(row)) == op.entry_id.as_deref() { row["delivered"] = json!(true); } }
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
            op.status = status; op.result = result;op.terminal_finalized=finalized;
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
            state.used_occurrences.insert(proof.occurrence.id.clone(),json!({"operation_id":id,"generation":state.generation,
                "conversation":proof.occurrence.conversation,"file_identity":proof.occurrence.file_identity,"offset":proof.occurrence.offset}));
            state.operations.get_mut(&id).unwrap().status = Status::Confirmed;
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
            let claimed:BTreeSet<_> = state.operations.iter().filter(|(call,op)|call.starts_with("call::terminal:queue:") && op.payload["kind"]=="claim")
                .flat_map(|(_,op)|op.result.as_array().into_iter().flatten()).filter_map(|row|row["id"].as_str()).collect();
            // Só o claim do executor terminal, sem despacho, prova ausência de efeito na TUI.
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
            state.runtime_state = runtime_state;
            if !barrier.is_null() {state.runtime_state["terminal_write_barrier"]=barrier;}
            Value::Null
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
    let claimed: BTreeSet<&str> = state.operations.iter().filter(|(key,op)|key.starts_with(CALL_PREFIX) && op.payload["kind"] == "claim")
        .flat_map(|(_,op)|op.result.as_array().into_iter().flatten()).filter_map(|row|row["id"].as_str()).collect();
    let mut rows: Vec<Value> = state.rows.iter().filter(|r|r["delivered"] == true && r["confirmed"] != true && r["desistiu"] != true
        && r["papel"] != "assistant" && !dispatched.contains(row_id(r)) && !claimed.contains(row_id(r))).cloned().collect();
    rows.sort_by(|a,b|b["ts"].as_f64().unwrap_or(0.0).total_cmp(&a["ts"].as_f64().unwrap_or(0.0)));
    rows
}

pub(crate) fn entry_lines(row: &Value) -> BTreeSet<String> {
    let raw = row["text"].as_str().unwrap_or("").trim();
    let stripped = crate::transcript::history::strip_attach(raw);
    let mut lines: BTreeSet<_> = [raw,stripped.trim()].into_iter()
        .flat_map(|text|std::iter::once(text).chain(text.split('\n')))
        .map(str::trim).filter(|text|!text.is_empty()).map(str::to_owned).collect();
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

enum QueueMessage {
    Exec { generation:u64, call_id:String, clock:ClockSample, action:Action, reply:oneshot::Sender<io::Result<Value>> },
    Stop,
    Snapshot(oneshot::Sender<io::Result<State>>),
}

pub struct QueueActor {
    sender: mpsc::Sender<QueueMessage>,
    task: tokio::task::JoinHandle<()>,
    initial: State,
    lease: Arc<File>,
}

impl QueueActor {
    pub fn start(store: Store, lease: Arc<File>) -> Self {
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
                    QueueMessage::Exec { generation,call_id,clock,action,reply } => {
                        let lease = lease.clone();
                        let job = tokio::task::spawn_blocking(move || {
                            let _lease = lease;
                            let result = store.exec(generation,&call_id,clock,action);
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
    pub fn lease(&self) -> Arc<File> { self.lease.clone() }

    pub async fn exec(&self, generation:u64, call_id:&str, clock:ClockSample, action:Action) -> io::Result<Value> {
        let (reply,response) = oneshot::channel();
        self.sender.send(QueueMessage::Exec { generation,call_id:call_id.into(),clock,action,reply }).await
            .map_err(|_|invalid("fila encerrada"))?;
        response.await.map_err(|_|invalid("fila encerrada sem recibo"))?
    }

    pub async fn snapshot(&self) -> io::Result<State> {
        let (reply,response) = oneshot::channel();
        self.sender.send(QueueMessage::Snapshot(reply)).await.map_err(|_|invalid("fila encerrada"))?;
        response.await.map_err(|_|invalid("fila encerrada sem estado"))?
    }

    pub async fn shutdown(self) -> io::Result<()> {
        self.sender.send(QueueMessage::Stop).await.map_err(|_|invalid("fila já encerrada"))?;
        self.task.await.map_err(|_|invalid("persistência da fila interrompida"))
    }
}
