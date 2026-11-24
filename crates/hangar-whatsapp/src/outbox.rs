//! Rascunhos de saída do C5 (api 2): a lista dos pendentes para o cartão do dono e a recusa dos que sobraram de
//! outra vida do Hangar. A espera da tool `whatsapp_draft` e as frases dela moram em `wait`.
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeDelta, Timelike, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};

use crate::client::{CallError, Client};
use crate::http::call_failure;

mod wait;

pub use wait::{DONT_REPEAT, DRAFT_MAYBE_CREATED, Declined, DraftPort, Progress, Read, wait_draft};

/// O envio por rascunho nasce na api 2 do binário.
pub const SEND_API: u32 = 2;
/// Releitura sem pedido: o binário expira rascunho sozinho, e o cartão some junto.
pub const REFRESH: Duration = Duration::from_secs(30);
/// O prazo próprio da tool `whatsapp_draft`, contado do começo da chamada: o MCP corta entre 600 e 660 s. O cartão
/// conta até ele (`answer_by`), porque resposta depois dele não chega a quem pediu.
pub const TOOL_DEADLINE: TimeDelta = TimeDelta::seconds(570);

/// O evento `whatsapp_outbox`: os rascunhos pendentes como o binário os descreve, cada um com o `answer_by`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Feed {
    pub drafts: Vec<Value>,
}

/// O que o servidor liga à lista: o aviso ao celular e o diário.
pub trait Hooks: Send + Sync {
    /// Rascunho pendente que a leitura anterior não tinha.
    fn new_draft(&self, draft: &Value);
    /// Leitura ou recusa que falhou: vai ao diário e ao log, nunca à tela.
    fn failed(&self, code: &'static str, detail: &str);
}

pub struct Outbox {
    /// `None` até a primeira leitura boa de um binário com envio: sem ela o evento não sai.
    tx: watch::Sender<Option<Arc<Feed>>>,
    wake: Notify,
    state: Mutex<State>,
}

struct State {
    /// A subida desta vida do Hangar: só o rascunho criado antes dela é de outra vida.
    started: DateTime<Utc>,
    /// Todas as recusas da subida deram certo; falha tenta de novo na volta seguinte.
    swept: bool,
    /// Pendentes da última leitura boa: id fora dela é rascunho novo.
    seen: HashSet<String>,
    /// A falha em curso: o diário recebe a primeira, não uma a cada releitura.
    failing: Option<&'static str>,
}

enum ReadError {
    Off,
    Failed(&'static str, String),
}

impl Default for Outbox {
    fn default() -> Self {
        Self::started_at(Utc::now())
    }
}

impl Outbox {
    /// `started` é a subida desta vida do Hangar, injetada para a prova não depender do relógio.
    pub fn started_at(started: DateTime<Utc>) -> Self {
        let state = State { started, swept: false, seen: HashSet::new(), failing: None };
        Self { tx: watch::channel(None).0, wake: Notify::new(), state: Mutex::new(state) }
    }

    pub fn feed(&self) -> watch::Receiver<Option<Arc<Feed>>> {
        self.tx.subscribe()
    }

    /// Relê agora: rascunho criado ou respondido muda a lista antes do próximo tique.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Laço da lista: uma rodada a cada [`REFRESH`] ou a cada [`Outbox::poke`].
    pub(crate) async fn run(&self, client: &Client, hooks: &dyn Hooks) {
        loop {
            self.refresh(client, hooks).await;
            tokio::select! {
                () = tokio::time::sleep(REFRESH) => {}
                () = self.wake.notified() => {}
            }
        }
    }

    /// Uma rodada. Sem serviço ou com binário sem envio, nada vai ao diário e o cartão na tela some; falha de
    /// leitura mantém a última lista publicada.
    pub(crate) async fn refresh(&self, client: &Client, hooks: &dyn Hooks) {
        if !client.claimed().is_some_and(|control| control.api >= SEND_API) {
            return self.off();
        }
        let mut drafts = match pending(client).await {
            Ok(drafts) => drafts,
            Err(error) => return self.failed(error, hooks),
        };
        let (started, swept) = {
            let state = self.lock();
            (state.started, state.swept)
        };
        if !swept {
            // Nenhuma tool desta vida espera pelo que ficou de antes do reinício: sai da fila do dono. Vale o horário
            // de criação: o rascunho desta vida, mesmo no meio da criação, é de depois da subida.
            let stale: Vec<String> = drafts.iter().filter(|d| from_before(d, started)).filter_map(draft_id).map(str::to_owned).collect();
            let mut declined = HashSet::new();
            for id in &stale {
                match decline(client, id).await {
                    Ok(()) => { declined.insert(id.as_str()); }
                    Err(detail) => {
                        tracing::warn!(%detail, "whatsapp: rascunho de outra vida do Hangar não foi recusado; tenta de novo na próxima volta");
                        hooks.failed("whatsapp_outbox_sweep_failed", &detail);
                    }
                }
            }
            self.lock().swept = declined.len() == stale.len();
            drafts.retain(|d| draft_id(d).is_none_or(|id| !declined.contains(id)));
        }
        let fresh: Vec<&Value> = {
            let mut state = self.lock();
            state.failing = None;
            let ids: HashSet<String> = drafts.iter().filter_map(draft_id).map(str::to_owned).collect();
            // O de outra vida que ainda não saiu da fila não avisa o celular.
            let fresh = drafts.iter()
                .filter(|d| !from_before(d, started) && draft_id(d).is_some_and(|id| !state.seen.contains(id)))
                .collect();
            state.seen = ids;
            fresh
        };
        for draft in fresh {
            hooks.new_draft(draft);
        }
        self.publish(drafts);
    }

    fn failed(&self, error: ReadError, hooks: &dyn Hooks) {
        match error {
            ReadError::Off => self.off(),
            ReadError::Failed(code, detail) => {
                if self.lock().failing.replace(code) != Some(code) {
                    tracing::warn!(code, %detail, "whatsapp: rascunhos pendentes sem leitura; vale a última lista");
                    hooks.failed(code, &detail);
                }
            }
        }
    }

    fn publish(&self, drafts: Vec<Value>) {
        let feed = Feed { drafts: drafts.into_iter().map(with_answer_by).collect() };
        self.tx.send_if_modified(|current| {
            if current.as_deref() == Some(&feed) { return false; }
            *current = Some(Arc::new(feed));
            true
        });
    }

    /// Sem serviço não há o que aprovar: o cartão que estava na tela some, e quem nunca leu segue sem evento.
    fn off(&self) {
        self.tx.send_if_modified(|current| {
            let shown = current.as_ref().is_some_and(|feed| !feed.drafts.is_empty());
            if shown { *current = Some(Arc::default()); }
            shown
        });
    }
}

/// Até quando a resposta do dono ainda chega a quem pediu: a expiração do binário ou o prazo da tool, o que vier
/// antes. Sem horário legível fica de fora, e o cliente conta até `expires_at`.
fn with_answer_by(mut draft: Value) -> Value {
    let tool = at(&draft, "created_at").and_then(|created| created.checked_add_signed(TOOL_DEADLINE));
    let by = [at(&draft, "expires_at"), tool].into_iter().flatten().min();
    if let (Some(by), Some(fields)) = (by, draft.as_object_mut()) {
        fields.insert("answer_by".into(), json!(by.to_rfc3339_opts(SecondsFormat::Secs, true)));
    }
    draft
}

/// Criado antes da subida desta vida. O binário grava em segundos: a subida também conta em segundos inteiros, e
/// o criado no mesmo segundo fica (expira sozinho), nunca o desta vida recusado.
fn from_before(draft: &Value, started: DateTime<Utc>) -> bool {
    let started = started.with_nanosecond(0).unwrap_or(started);
    at(draft, "created_at").is_some_and(|created| created < started)
}

fn draft_id(draft: &Value) -> Option<&str> {
    draft["id"].as_str().filter(|id| id_ok(id))
}

/// Id que vai no caminho do C5: letras, dígitos, `-` e `_`.
pub fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn pending(client: &Client) -> Result<Vec<Value>, ReadError> {
    match client.call(reqwest::Method::GET, "/v1/outbox", &[("state", "pending")], None).await {
        Ok(reply) if reply.status == 200 => {
            let body: Value = serde_json::from_slice(&reply.body)
                .map_err(|e| ReadError::Failed("whatsapp_outbox_bad_reply", e.to_string()))?;
            match body.get("drafts").and_then(Value::as_array) {
                Some(drafts) => Ok(drafts.clone()),
                None => Err(ReadError::Failed("whatsapp_outbox_bad_reply", "resposta sem drafts".into())),
            }
        }
        Ok(reply) => Err(ReadError::Failed("whatsapp_outbox_read_failed", format!("status {}", reply.status))),
        Err(CallError::Off(_)) => Err(ReadError::Off),
        Err(error) => {
            let (CallError::Off(detail) | CallError::Untrusted(detail) | CallError::Transport(detail)) = &error;
            Err(ReadError::Failed(call_failure(&error).1, detail.clone()))
        }
    }
}

/// Recusa no binário; já respondido, expirado ou sumido também tira o rascunho da fila.
async fn decline(client: &Client, id: &str) -> Result<(), String> {
    match client.call(reqwest::Method::POST, &format!("/v1/outbox/{id}/decline"), &[], None).await {
        Ok(reply) if reply.status == 200 || reply.status == 404 || reply.status == 409 => Ok(()),
        Ok(reply) => Err(format!("recusa respondeu {}", reply.status)),
        Err(CallError::Off(detail) | CallError::Untrusted(detail) | CallError::Transport(detail)) => Err(detail),
    }
}

fn at(draft: &Value, field: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(draft[field].as_str()?).ok().map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// Só o criado em segundo anterior à subida é de outra vida; o do mesmo segundo e o sem horário ficam.
    #[test]
    fn only_a_draft_created_before_the_start_is_from_another_life() {
        let started = t("2026-11-24T10:00:00Z") + TimeDelta::milliseconds(500);
        let created = |at: &str| json!({"id": "d1", "created_at": at});
        assert!(from_before(&created("2026-11-24T09:59:59Z"), started));
        assert!(!from_before(&created("2026-11-24T10:00:00Z"), started), "mesmo segundo: pode ser desta vida");
        assert!(!from_before(&created("2026-11-24T10:00:01Z"), started));
        assert!(!from_before(&json!({"id": "d1"}), started), "sem horário não se recusa às cegas");
    }

    /// O cartão conta até o que vencer antes: a expiração do binário ou o prazo da tool contado da criação.
    #[test]
    fn the_card_counts_down_to_the_earlier_of_the_expiry_and_the_tool_deadline() {
        let draft = |expires: &str| json!({"id": "d1", "created_at": "2026-11-24T10:00:00Z", "expires_at": expires});
        assert_eq!(with_answer_by(draft("2026-11-24T10:10:00Z"))["answer_by"], json!("2026-11-24T10:09:30Z"), "prazo da tool");
        assert_eq!(with_answer_by(draft("2026-11-24T10:05:00Z"))["answer_by"], json!("2026-11-24T10:05:00Z"), "expira antes");
        let unread = with_answer_by(json!({"id": "d1", "created_at": "ontem", "expires_at": "2026-11-24T10:10:00Z"}));
        assert_eq!(unread["answer_by"], json!("2026-11-24T10:10:00Z"), "sem criação legível, vale a expiração");
        assert!(with_answer_by(json!({"id": "d1"})).get("answer_by").is_none(), "sem horário nenhum, o cliente conta até expires_at");
    }

    #[test]
    fn only_safe_ids_go_in_the_path() {
        assert!(id_ok("01JD7Z-abc_9"));
        for bad in ["", "../x", "a/b", "a?b", "a b", &"x".repeat(65)] {
            assert!(!id_ok(bad), "{bad}");
        }
    }
}
