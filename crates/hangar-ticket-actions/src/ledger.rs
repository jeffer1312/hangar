use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Failure;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Preparing, Opening, Sending, Sent, Queued, Recorded, Uncertain, Failed }

impl Status {
    fn active(self) -> bool { matches!(self, Self::Preparing | Self::Opening | Self::Sending) }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Attempt {
    pub id: String,
    pub connector: String,
    pub ticket_id: String,
    pub stage: String,
    #[serde(default="crate::engine::execute_by_default")]
    pub execute: bool,
    pub session: Option<String>,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub conversation: Option<String>,
    pub status: Status,
    pub code: Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub message: Option<String>,
}

pub struct Reservation {
    pub attempt: Attempt,
    pub fresh: bool,
}

pub struct Ledger { root: PathBuf, epoch: String }

#[derive(Deserialize, Serialize)]
struct Record {
    version: u32,
    request_hash: String,
    #[serde(default)]
    epoch: String,
    attempt: Attempt,
}

impl Ledger {
    pub fn new(root: PathBuf) -> Self { Self { root, epoch: hangar_core::ids::random_hex(16) } }

    pub fn bind(&self,id: &str,session: &crate::Session) -> Result<Attempt,Failure> {
        if !hangar_core::ids::valid_session_name(&session.name) || session.key.is_empty() || session.key.len()>1024 || session.key.contains('\0')
            || session.conversation.as_ref().is_some_and(|value|value.is_empty()||value.len()>1024||value.contains('\0')) {
            return Err(Failure::new(422,"ticket_action_invalid","identidade da sessão inválida"));
        }
        self.update(id,|attempt| {
            attempt.session=Some(session.name.clone());
            attempt.session_key=Some(session.key.clone());
            attempt.conversation=session.conversation.clone();
        })
    }

    pub fn latest(&self, connector: &str, ticket: &str) -> Result<Option<Attempt>, Failure> {
        self.latest_mode(connector,ticket,false)
    }

    pub fn latest_execution(&self,connector: &str,ticket: &str) -> Result<Option<Attempt>,Failure> {
        self.latest_mode(connector,ticket,true)
    }

    fn latest_mode(&self,connector: &str,ticket: &str,execution: bool) -> Result<Option<Attempt>,Failure> {
        let ids = self.read_index(connector, ticket,execution)?;
        for id in ids {
            match self.read(&id) {
                Ok(attempt) if execution && !attempt.execute=>{},
                Ok(attempt) => return Ok(Some(attempt)),
                Err(error) if error.status == 404 => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    pub fn read(&self, id: &str) -> Result<Attempt, Failure> {
        valid_id(id)?;
        Ok(self.visible(self.read_record(id)?))
    }

    pub fn record(&self, id: &str, status: Status, session: Option<&str>, code: Option<&str>) -> Result<Attempt, Failure> {
        valid_id(id)?;
        if session.is_some_and(|name| !hangar_core::ids::valid_session_name(name)) {
            return Err(Failure::new(422, "ticket_action_invalid", "nome da sessão inválido"));
        }
        self.update(id,|attempt| {
            attempt.status=status;
            if let Some(name)=session {attempt.session=Some(name.to_owned());}
            attempt.code=code.map(str::to_owned);
        })
    }

    pub fn fail(&self,id: &str,status: Status,error: &Failure) -> Result<Attempt,Failure> {
        self.update(id,|attempt| {
            attempt.status=status;
            attempt.code=Some(error.code.clone());
            attempt.message=Some(error.message.clone());
        })
    }

    fn update(&self,id: &str,change: impl FnOnce(&mut Attempt)) -> Result<Attempt,Failure> {
        valid_id(id)?;
        let _lock = hangar_core::fs::lock_file(&self.root.join("ledger.lock")).map_err(|_| unavailable())?;
        let mut record = self.read_record(id)?;
        if !record.attempt.status.active() || record.epoch != self.epoch { return Ok(self.visible(record)); }
        change(&mut record.attempt);
        let bytes = serde_json::to_vec(&record).map_err(|_| unavailable())?;
        self.index(&record.attempt)?;
        hangar_core::fs::atomic_write(&self.root.join(format!("{id}.json")), &bytes).map_err(|_| unavailable())?;
        Ok(record.attempt)
    }

    pub fn reserve(&self, id: &str, request: &Value) -> Result<Reservation, Failure> {
        valid_id(id)?;
        let field = |name: &str| request.get(name).and_then(Value::as_str)
            .filter(|value| !value.is_empty() && !value.contains('\0')).map(str::to_owned)
            .ok_or_else(|| Failure::new(422, "ticket_action_invalid", format!("campo {name} inválido")));
        let (connector, ticket_id, stage) = (field("connector")?, field("ticket_id")?, field("stage")?);
        let execute=match request.get("execute") {
            Some(Value::Bool(execute))=>*execute,None=>true,
            _=>return Err(Failure::new(422,"ticket_action_invalid","execute precisa ser booleano")),
        };
        if !hangar_core::ids::valid_slug(&connector, 64) || !hangar_core::ids::valid_slug(&stage, 32) || ticket_id.len() > 1024 {
            return Err(Failure::new(422, "ticket_action_invalid", "identidade do ticket ou da etapa inválida"));
        }
        let mut canonical = request.clone();
        canonical.sort_all_objects();
        let raw = serde_json::to_vec(&canonical).map_err(|_| unavailable())?;
        let request_hash = hangar_core::credentials::hash(&raw);
        std::fs::create_dir_all(&self.root).map_err(|_| unavailable())?;
        let path = self.root.join(format!("{id}.json"));
        let _lock = hangar_core::fs::lock_file(&self.root.join("ledger.lock")).map_err(|_| unavailable())?;
        match hangar_core::fs::read_replaced(&path) {
            Ok(bytes) => {
                let record: Record = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
                if record.version != 1 { return Err(unavailable()); }
                if record.request_hash != request_hash { return Err(Failure::new(409, "ticket_action_conflict", "a tentativa já pertence a outro pedido")); }
                Ok(Reservation { attempt: self.visible(record), fresh: false })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let attempt = Attempt { id: id.into(), connector, ticket_id, stage, execute, session: None, session_key: None, conversation: None, status: Status::Preparing, code: None, message: None };
                let record = Record { version: 1, request_hash, epoch: self.epoch.clone(), attempt: attempt.clone() };
                let bytes = serde_json::to_vec(&record).map_err(|_| unavailable())?;
                self.index(&attempt)?;
                hangar_core::fs::atomic_write(&path, &bytes).map_err(|_| unavailable())?;
                Ok(Reservation { attempt, fresh: true })
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn visible(&self, mut record: Record) -> Attempt {
        if record.epoch != self.epoch && record.attempt.status.active() {
            record.attempt.status = Status::Uncertain;
            record.attempt.code = Some("ticket_action_uncertain".into());
        }
        record.attempt
    }

    fn read_record(&self, id: &str) -> Result<Record, Failure> {
        let bytes = hangar_core::fs::read_replaced(&self.root.join(format!("{id}.json"))).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound { Failure::new(404, "ticket_action_not_found", "tentativa não encontrada") }
            else { unavailable() }
        })?;
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if record.version != 1 { return Err(unavailable()); }
        Ok(record)
    }

    fn index_path(&self, connector: &str, ticket: &str,execution: bool) -> Result<PathBuf, Failure> {
        if !hangar_core::ids::valid_slug(connector, 64) || ticket.is_empty() || ticket.len() > 1024 || ticket.contains('\0') {
            return Err(Failure::new(422, "ticket_action_invalid", "identidade do ticket inválida"));
        }
        let key = serde_json::to_vec(&(connector, ticket)).map_err(|_| unavailable())?;
        Ok(self.root.join(format!("{}-{}.json",if execution {"execution"} else {"index"},hangar_core::credentials::hash(&key))))
    }

    fn read_index(&self, connector: &str, ticket: &str,execution: bool) -> Result<Vec<String>, Failure> {
        match hangar_core::fs::read_replaced(&self.index_path(connector, ticket,execution)?) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| unavailable()),
            Err(error) if error.kind()==std::io::ErrorKind::NotFound && execution=>self.read_index(connector,ticket,false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err(unavailable()),
        }
    }

    fn index(&self, attempt: &Attempt) -> Result<(), Failure> {
        for execution in [false,true] {
            if execution && !attempt.execute {continue;}
            let mut ids = self.read_index(&attempt.connector, &attempt.ticket_id,execution)?;
            ids.retain(|id| id != &attempt.id);
            ids.insert(0, attempt.id.clone());
            ids.truncate(32);
            let bytes = serde_json::to_vec(&ids).map_err(|_| unavailable())?;
            hangar_core::fs::atomic_write(&self.index_path(&attempt.connector, &attempt.ticket_id,execution)?, &bytes).map_err(|_| unavailable())?;
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> Result<(), Failure> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        Err(Failure::new(422, "ticket_action_invalid", "identidade da tentativa inválida"))
    } else { Ok(()) }
}

fn unavailable() -> Failure { Failure::new(503, "ticket_action_unavailable", "registro de ação indisponível") }
