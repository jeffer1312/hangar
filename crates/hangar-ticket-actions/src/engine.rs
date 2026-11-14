use std::{collections::HashMap, future::Future, pin::Pin, sync::{Arc,Mutex}};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use hangar_connectors::host::Host;
use hangar_mission::store::Store;
use crate::{Attempt, Failure, Ledger, Status};

pub type Task<T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send>>;

/// Porta do lançador único e da entrada normal; o servidor conserva suas guardas e defaults.
pub trait Sessions: Send + Sync + 'static {
    fn validate_open(&self, options: Value) -> Result<Value, Failure>;
    fn resolve(&self, name: String, key: Option<String>) -> Task<Option<Session>>;
    fn open(&self, name: String, options: Value) -> Task<()>;
    fn send(&self, session: Session, text: String) -> Task<Delivery>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session { pub name: String, pub key: String, pub conversation: Option<String> }

#[derive(Clone, Copy, Debug)]
pub enum Delivery { Sent, Queued }

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub attempt: String,
    pub connector: String,
    pub ticket_id: String,
    pub stage: String,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub open: Option<Value>,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub confirm_retry: bool,
    #[serde(default="execute_by_default")]
    pub execute: bool,
    #[serde(default)]
    pub start: bool,
}

pub(crate) fn execute_by_default() -> bool {true}

pub struct Engine {
    pub ledger: Ledger,
    pub store: Arc<Store>,
    pub host: Arc<Host>,
    pub sessions: Arc<dyn Sessions>,
    changed: tokio::sync::Notify,
    volatile: Mutex<Volatile>,
}

#[derive(Default)]
struct Volatile {
    attempts: HashMap<String,(Value,Attempt)>,
    latest: HashMap<(String,String),String>,
    executions: HashMap<(String,String),String>,
}

impl Volatile {
    fn remove(&mut self,id: &str) {
        if let Some((_,attempt))=self.attempts.remove(id) {
            let key=(attempt.connector,attempt.ticket_id);
            if self.latest.get(&key).is_some_and(|latest|latest==id) {self.latest.remove(&key);}
            if self.executions.get(&key).is_some_and(|latest|latest==id) {self.executions.remove(&key);}
        }
    }
}

impl Engine {
    pub fn new(ledger: Ledger, store: Arc<Store>, host: Arc<Host>, sessions: Arc<dyn Sessions>) -> Arc<Self> {
        Arc::new(Self { ledger, store, host, sessions, changed: tokio::sync::Notify::new(), volatile: Mutex::default() })
    }

    pub fn read(&self,id: &str) -> Result<Attempt,Failure> {
        if let Some((_,attempt))=self.volatile.lock().unwrap_or_else(|e|e.into_inner()).attempts.get(id) {return Ok(attempt.clone());}
        self.ledger.read(id)
    }

    pub fn latest(&self,connector: &str,ticket: &str) -> Result<Option<Attempt>,Failure> {
        let id=self.volatile.lock().unwrap_or_else(|e|e.into_inner()).latest.get(&(connector.to_owned(),ticket.to_owned())).cloned();
        if let Some(id)=id {return self.read(&id).map(Some);}
        self.ledger.latest(connector,ticket)
    }

    pub fn latest_execution(&self,connector: &str,ticket: &str) -> Result<Option<Attempt>,Failure> {
        let id=self.volatile.lock().unwrap_or_else(|e|e.into_inner()).executions.get(&(connector.to_owned(),ticket.to_owned())).cloned();
        if let Some(id)=id {return self.read(&id).map(Some);}
        self.ledger.latest_execution(connector,ticket)
    }

    fn remember(&self,attempt: &Attempt) {
        if let Some((_,current))=self.volatile.lock().unwrap_or_else(|e|e.into_inner()).attempts.get_mut(&attempt.id) {*current=attempt.clone();}
    }

    fn record(&self,id: &str,status: Status,session: Option<&str>,code: Option<&str>) -> Result<Attempt,Failure> {
        let receipt=self.ledger.record(id,status,session,code).map_err(|_|receipt_unavailable())?;
        self.remember(&receipt);
        Ok(receipt)
    }

    /// Aguarda uma mudança sem observar o disco em laço. O consumidor impõe seu prazo de espera.
    pub async fn completed(&self, id: &str) -> Result<Attempt, Failure> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let attempt = self.read(id)?;
            if !matches!(attempt.status, Status::Preparing | Status::Opening | Status::Sending) { return Ok(attempt); }
            changed.await;
        }
    }

    pub fn start(self: &Arc<Self>, request: Request) -> Result<Attempt, Failure> {
        let value = serde_json::to_value(&request).expect("pedido serializável");
        let mut cache=self.volatile.lock().unwrap_or_else(|e|e.into_inner());
        if let Some((original,attempt))=cache.attempts.get(&request.attempt) {
            if original!=&value {return Err(Failure::new(409,"ticket_action_conflict","a tentativa já pertence a outro pedido"));}
            return Ok(attempt.clone());
        }
        match self.ledger.read(&request.attempt) {
            Ok(_) => return Ok(self.ledger.reserve(&request.attempt,&value)?.attempt),
            Err(error) if error.status==404 => {}
            Err(error) => return Err(error),
        }
        let key=(request.connector.clone(),request.ticket_id.clone());
        let previous = if !request.execute {None} else {match cache.executions.get(&key).and_then(|id|cache.attempts.get(id)) {
            Some((_,attempt))=>Some(attempt.clone()),
            None=>self.ledger.latest_execution(&request.connector,&request.ticket_id)?,
        }};
        if previous.as_ref().is_some_and(|a| a.id!=request.attempt && a.status==Status::Uncertain) && !request.confirm_retry {
            return Err(Failure::new(409,"ticket_action_check_required","confira o envio anterior antes de tentar novamente"));
        }
        if cache.attempts.len()>=128 {return Err(Failure::new(503,"ticket_action_unavailable","há resultados aguardando recuperação do armazenamento; confira as tentativas e reinicie o servidor"));}
        let reservation = self.ledger.reserve(&request.attempt, &value)?;
        if reservation.fresh {
            if request.execute {cache.executions.insert(key.clone(),request.attempt.clone());}
            cache.latest.insert(key,request.attempt.clone());
            cache.attempts.insert(request.attempt.clone(),(value,reservation.attempt.clone()));
        }
        drop(cache);
        if reservation.fresh {
            let engine = self.clone();
            tokio::spawn(async move {
                let recovery = previous.filter(|a|a.status==Status::Uncertain && request.confirm_retry);
                let result = tokio::time::timeout(std::time::Duration::from_secs(90), engine.run(&request,recovery)).await
                    .unwrap_or_else(|_| Err(Failure::new(504, "ticket_action_timeout", "a ação não respondeu no prazo")));
                if let Err(error) = result {
                    let active = engine.read(&request.attempt).ok();
                    let uncertain = error.status >= 500 && active.is_some_and(|a| matches!(a.status,Status::Opening|Status::Sending));
                    let status=if uncertain {Status::Uncertain} else {Status::Failed};
                    let stored=engine.ledger.fail(&request.attempt,status,&error);
                    let mut cache=engine.volatile.lock().unwrap_or_else(|e|e.into_inner());
                    if stored.is_ok() {cache.remove(&request.attempt);}
                    else if let Some((_,attempt))=cache.attempts.get_mut(&request.attempt) {
                        attempt.status=status;attempt.code=Some(error.code);attempt.message=Some(error.message);
                    }
                } else {
                    engine.volatile.lock().unwrap_or_else(|e|e.into_inner()).remove(&request.attempt);
                }
                engine.changed.notify_waiters();
            });
        }
        Ok(reservation.attempt)
    }

    async fn run(&self, request: &Request, recovery: Option<Attempt>) -> Result<(), Failure> {
        let guard = self.store.begin_action(&request.connector, &request.ticket_id).map_err(mission_error)?;
        let (stage, previous) = self.store.action_stage(&guard, &request.stage).map_err(mission_error)?;
        if !request.repeat && previous.as_ref().and_then(|item|item.stage.as_deref())==Some(&request.stage) {
            self.record(&request.attempt,Status::Recorded,previous.as_ref().and_then(|item|item.session.as_deref()),None)?;
            return Ok(());
        }
        let ticket = self.host.get_ticket(&request.connector, hangar_connectors::work_tracking::GetParams::new(&request.ticket_id)
            .map_err(|error| Failure::new(422,error.code.as_str(),error.message))?).await
            .map_err(|error| Failure::new(424,error.code(),error.message()))?.summary;
        hangar_mission::store::TicketMetadata {key:ticket.key.clone(),title:ticket.title.clone(),url:ticket.url.clone()}.validate().map_err(mission_error)?;
        if !request.execute {
            self.store.track_action(&guard,hangar_mission::store::TicketMetadata {key:ticket.key,title:ticket.title,url:ticket.url}).map_err(mission_error)?;
            self.store.move_action(&guard,&request.stage).map_err(mission_error)?;
            self.record(&request.attempt,Status::Recorded,previous.as_ref().and_then(|item|item.session.as_deref()),None)?;
            return Ok(());
        }
        let entry = self.host.entry(&request.connector).map_err(|e|Failure::new(424,e.code(),e.message.clone()))?;
        let configured=stage.actions.as_ref().and_then(|map|map.get(&request.connector));
        let action_prompt = if entry.capabilities.contains_key(hangar_connectors::work_actions::CAPABILITY) {
            let actions = self.host.list_actions(&request.connector, hangar_connectors::work_tracking::GetParams::new(&request.ticket_id)
                .map_err(|e|Failure::new(422,e.code.as_str(),e.message))?).await.map_err(|e|Failure::new(424,e.code(),e.message()))?;
            if configured.is_some_and(|ids|ids.iter().any(|id|!actions.iter().any(|action|&action.action.id==id))) {
                return Err(Failure::new(409,"ticket_action_stale","uma ação configurada não está mais disponível; revise as ações desta etapa"));
            }
            let available: Vec<_> = actions.into_iter().filter(|a|configured.map_or_else(||a.action.stage.as_deref()==Some(&stage.id),|ids|ids.contains(&a.action.id))).collect();
            if let Some(id) = &request.action {
                let selected = available.iter().find(|a|&a.action.id==id).ok_or_else(||Failure::new(409,"ticket_action_stale","a ação já não está disponível nesta etapa"))?;
                if request.fingerprint.as_deref()!=Some(&selected.fingerprint) { return Err(Failure::new(409,"ticket_action_stale","as opções mudaram; escolha a ação novamente")); }
                Some(selected.prompt.clone())
            } else if !available.is_empty() {
                return Err(Failure::new(409,"ticket_action_choice_required","escolha a ação da etapa"));
            } else { None }
        } else {
            if request.action.is_some() || configured.is_some_and(|ids|!ids.is_empty()) { return Err(Failure::new(409,"unsupported_capability","o conector não declarou as ações configuradas; revise esta etapa")); }
            if previous.as_ref().and_then(|item|item.session.as_ref()).is_none() && self.store.snapshot().map_err(mission_error)?.stages.first().is_some_and(|s|s.id==stage.id) {
                entry.start_prompt_for(&ticket).map_err(|e|Failure::new(422,e.code.as_str(),e.message))?
            } else {None}
        };
        let history = previous.as_ref().map_or(0,|item|item.history.len());
        let prompt = stage.prompt.as_deref().unwrap_or_default().trim();
        if action_prompt.as_deref().is_none_or(|p|p.trim().is_empty()) && prompt.is_empty() {
            self.store.track_action(&guard, hangar_mission::store::TicketMetadata {key:ticket.key,title:ticket.title,url:ticket.url}).map_err(mission_error)?;
            self.store.commit_action(&guard, &request.stage, history, "Etapa registrada pelo board.").map_err(mission_error)?;
            self.record(&request.attempt, Status::Recorded, None, None)?;
            return Ok(());
        }
        let linked = previous.as_ref().and_then(|item|item.session.clone()).or_else(||recovery.as_ref().and_then(|a|a.session.clone()));
        let key = previous.as_ref().and_then(|item|item.session_key.clone().or_else(||item.session.as_ref().and_then(|name|item.session_keys.get(name).cloned())))
            .or_else(||recovery.as_ref().and_then(|a|a.session_key.clone()));
        let weak=previous.as_ref().is_some_and(|item|item.session.is_some()) && key.is_none();
        if weak && !request.start {return Err(Failure::new(409,"ticket_session_identity_required","confirme a associação da sessão no ticket antes de executar a etapa"));}
        let resolved = if weak {None} else {match linked {Some(name)=>self.sessions.resolve(name,key.clone()).await?,None=>None}};
        let open = request.open.clone().or_else(||previous.as_ref().and_then(|item|item.repository.as_ref()).map(|cwd|serde_json::json!({"cwd":cwd})));
        if key.is_some() && resolved.is_none() && open.is_none() {return Err(Failure::new(409,"ticket_session_changed","a sessão vinculada foi encerrada ou recriada"));}
        let session = match resolved {
            Some(session) => session,
            None => {
                let options = self.sessions.validate_open(open.ok_or_else(||Failure::new(409,"ticket_action_folder_required","escolha a pasta para abrir a sessão"))?)?;
                let key: String = ticket.key.chars().filter(|c|c.is_ascii_alphanumeric()||*c=='-').take(32).collect();
                let name = format!("ticket-{key}-{}",&request.attempt[..8]);
                self.record(&request.attempt,Status::Opening,Some(&name),None)?;
                self.sessions.open(name.clone(),options).await?;
                self.sessions.resolve(name,None).await?.ok_or_else(||Failure::new(503,"ticket_session_unavailable","a sessão aberta ainda não pôde ser conferida"))?
            }
        };
        let recovering=previous.as_ref().and_then(|item|item.session.as_ref()).is_none();
        let conversation=recovery.as_ref().filter(|attempt|recovering && attempt.session_key.as_deref()==Some(&session.key))
            .and_then(|attempt|attempt.conversation.as_deref());
        if let Some(expected)=conversation {
            match session.conversation.as_deref() {
                Some(found) if found==expected=>{},
                Some(_)=>return Err(Failure::new(409,"ticket_session_changed","a sessão da tentativa anterior começou outra conversa")),
                None=>return Err(Failure::new(503,"ticket_session_unavailable","a conversa da tentativa anterior ainda não pôde ser conferida")),
            }
        }
        self.remember(&self.ledger.bind(&request.attempt,&session).map_err(|_|receipt_unavailable())?);
        self.store.link_bound_action(&guard,hangar_mission::store::Link {key:ticket.key.clone(),title:ticket.title,url:ticket.url,session:session.name.clone()},&session.key,session.conversation.as_deref()).map_err(mission_error)?;
        let mut text = format!("Esta sessão trabalha o ticket {}. Execute a ação abaixo pelo fluxo do plugin, mantendo suas perguntas e aprovações. Informe o andamento pela tool `mission` do MCP hangar; anexe as evidências na validação e aguarde o dono quando houver portão.\n\n",ticket.key);
        if let Some(action) = action_prompt {text.push_str(&action);}
        if !prompt.is_empty() {text.push_str("\n\nInstruções da etapa:\n");text.push_str(prompt);}
        self.record(&request.attempt,Status::Sending,Some(&session.name),None)?;
        let delivery = self.sessions.send(session.clone(),text).await?;
        let (status,note) = match delivery {
            Delivery::Sent=>(Status::Sent,"Ação enviada à sessão pelo board."),
            Delivery::Queued=>(Status::Queued,"Ação colocada na fila da sessão pelo board."),
        };
        self.store.commit_action(&guard,&request.stage,history,note).map_err(|_|Failure::new(503,"ticket_action_commit_failed","o envio foi aceito, mas a etapa não pôde ser registrada"))?;
        self.record(&request.attempt,status,Some(&session.name),None)?;
        Ok(())
    }
}

fn mission_error(error: hangar_mission::store::Error) -> Failure {
    use hangar_mission::store::Kind;
    let status = match error.kind { Kind::Invalid=>422, Kind::Gate=>409, Kind::NotFound=>404, Kind::Unavailable=>503 };
    Failure::new(status,error.code,error.message)
}

fn receipt_unavailable() -> Failure {
    Failure::new(503,"ticket_action_unavailable","não foi possível registrar o resultado; confira a sessão antes de repetir")
}
