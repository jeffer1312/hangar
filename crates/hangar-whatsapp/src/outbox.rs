//! Rascunhos de saída do C5 (api 2): a lista dos pendentes para o cartão do dono, a recusa dos que
//! sobraram de outra vida do Hangar e a conta da espera da tool `whatsapp_draft`.
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};

use crate::client::{CallError, Client};

/// O envio por rascunho nasce na api 2 do binário.
pub const SEND_API: u32 = 2;
/// Releitura sem pedido: o binário expira rascunho sozinho, e o cartão some junto.
pub const REFRESH: Duration = Duration::from_secs(30);
/// Teto do binário entre a aprovação e o fim do envio (upload de 100 MB).
const SENDING_SECS: i64 = 600;
/// O binário confere a expiração a cada pedido; a folga só cobre relógios um pouco diferentes.
const EXPIRY_GRACE_SECS: i64 = 5;
const DONT_REPEAT: &str = "não crie outro rascunho para o mesmo envio";

/// O evento `whatsapp_outbox`: os rascunhos pendentes como o binário os descreve.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Feed {
    pub drafts: Vec<Value>,
    /// A lista veio de uma leitura boa do binário com envio: vazia, ainda assim é resposta, e o cliente que
    /// abriu a folha por um aviso precisa dela. Sem envio ou desligado fica falso, e o evento não sai.
    #[serde(skip)]
    pub read: bool,
}

/// O que o servidor liga à lista: o aviso ao celular e o diário.
pub trait Hooks: Send + Sync {
    /// Rascunho pendente que a leitura anterior não tinha.
    fn new_draft(&self, draft: &Value);
    /// Leitura ou recusa que falhou: vai ao diário e ao log, nunca à tela.
    fn failed(&self, code: &'static str, detail: &str);
}

pub struct Outbox {
    tx: watch::Sender<std::sync::Arc<Feed>>,
    wake: Notify,
    state: Mutex<State>,
}

struct State {
    /// Criados por esta vida do Hangar: a recusa da subida não os toca.
    created: HashSet<String>,
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
        let state = State { created: HashSet::new(), started, swept: false, seen: HashSet::new(), failing: None };
        Self { tx: watch::channel(std::sync::Arc::default()).0, wake: Notify::new(), state: Mutex::new(state) }
    }

    pub fn feed(&self) -> watch::Receiver<std::sync::Arc<Feed>> {
        self.tx.subscribe()
    }

    /// Relê agora: rascunho criado ou respondido muda a lista antes do próximo tique.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn created(&self, id: &str) {
        self.lock().created.insert(id.to_owned());
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

    /// Uma rodada. Sem serviço ou com binário sem envio, a lista fica vazia e nada vai ao diário; falha de
    /// leitura mantém a última lista publicada.
    pub(crate) async fn refresh(&self, client: &Client, hooks: &dyn Hooks) {
        if !client.claimed().is_some_and(|control| control.api >= SEND_API) {
            self.publish(Vec::new(), false);
            return;
        }
        let mut drafts = match pending(client).await {
            Ok(drafts) => drafts,
            Err(error) => return self.failed(error, hooks),
        };
        let started = self.lock().started;
        if !self.lock().swept {
            // Nenhuma tool desta vida espera pelo que ficou de antes do reinício: sai da fila do dono. Vale o horário
            // de criação, não só o id: a criação desta vida pode estar no meio do caminho, ainda sem id guardado.
            let created = self.lock().created.clone();
            let stale: Vec<String> = drafts.iter().filter(|d| from_before(d, started)).filter_map(draft_id)
                .filter(|id| !created.contains(*id)).map(str::to_owned).collect();
            let mut all_declined = true;
            for id in &stale {
                if let Err(detail) = decline(client, id).await {
                    all_declined = false;
                    tracing::warn!(%detail, "whatsapp: rascunho de outra vida do Hangar não foi recusado; tenta de novo na próxima volta");
                    hooks.failed("whatsapp_outbox_sweep_failed", &detail);
                }
            }
            self.lock().swept = all_declined;
            drafts = match pending(client).await {
                Ok(drafts) => drafts,
                Err(error) => return self.failed(error, hooks),
            };
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
        self.publish(drafts, true);
    }

    fn failed(&self, error: ReadError, hooks: &dyn Hooks) {
        match error {
            ReadError::Off => self.publish(Vec::new(), false),
            ReadError::Failed(code, detail) => {
                if self.lock().failing.replace(code) != Some(code) {
                    tracing::warn!(code, %detail, "whatsapp: rascunhos pendentes sem leitura; vale a última lista");
                    hooks.failed(code, &detail);
                }
            }
        }
    }

    fn publish(&self, drafts: Vec<Value>, read: bool) {
        let feed = Feed { drafts, read };
        self.tx.send_if_modified(|current| {
            if **current == feed { return false; }
            *current = std::sync::Arc::new(feed);
            true
        });
    }
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
        Err(CallError::Untrusted(detail)) => Err(ReadError::Failed("whatsapp_untrusted", detail)),
        Err(CallError::Transport(detail)) => Err(ReadError::Failed("whatsapp_unreachable", detail)),
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

/// O que a tool faz com a última leitura do rascunho.
#[derive(Clone, Debug, PartialEq)]
pub enum Wait {
    /// Estado final, ou o prazo da chamada com o envio já aprovado: o resultado da tool.
    Done(Value),
    /// Pendente ou saindo: lê de novo até `until`.
    Until(DateTime<Utc>),
    /// Pendente depois de expirar ou do prazo da chamada: a tool recusa e responde `expired`.
    PendingOverdue,
    /// Saindo há mais que o teto do binário: a tool para sem saber se a mensagem saiu.
    SendingOverdue,
    /// Estado ou horário fora do contrato.
    Unknown(String),
}

/// `draft` é a leitura do `GET /v1/outbox/{id}`; `None` é o 404 `not_found` (dados apagados), estado final.
/// `now` vem de quem chama, para a conta não depender do relógio; `call_deadline` é o prazo da própria chamada, que
/// o MCP corta antes dos prazos do binário.
pub fn wait_step(id: &str, draft: Option<&Value>, now: DateTime<Utc>, call_deadline: DateTime<Utc>) -> Wait {
    let Some(draft) = draft else { return Wait::Done(gone(id)) };
    let state = draft["state"].as_str().unwrap_or_default();
    match state {
        "sent" | "declined" | "expired" | "failed" => Wait::Done(outcome(id, draft)),
        "pending" => match at(draft, "expires_at") {
            Some(expires) => {
                let until = (expires + TimeDelta::seconds(EXPIRY_GRACE_SECS)).min(call_deadline);
                if now >= until { Wait::PendingOverdue } else { Wait::Until(until) }
            }
            None => Wait::Unknown("rascunho pendente sem expires_at".into()),
        },
        "sending" => match at(draft, "answered_at") {
            Some(answered) if now >= answered + TimeDelta::seconds(SENDING_SECS) => Wait::SendingOverdue,
            Some(_) if now >= call_deadline => Wait::Done(still_sending(id)),
            Some(answered) => Wait::Until((answered + TimeDelta::seconds(SENDING_SECS)).min(call_deadline)),
            None => Wait::Unknown("rascunho saindo sem answered_at".into()),
        },
        other => Wait::Unknown(format!("estado de rascunho desconhecido: {other:?}")),
    }
}

/// O prazo da chamada chegou com o envio aprovado e ainda saindo.
pub fn still_sending(id: &str) -> Value {
    json!({"status": "sending", "id": id,
        "note": "o dono aprovou; a mensagem está saindo. Não crie outro rascunho; confira com whatsapp_read"})
}

/// Recusado pela própria tool depois de expirar sem resposta do dono.
pub fn expired(id: &str) -> Value {
    json!({"status": "expired", "id": id, "note": format!("o dono não respondeu a tempo; {DONT_REPEAT} sem ele pedir de novo")})
}

/// O resultado da tool para um estado final: `message_id` e `error` só quando vieram.
pub fn outcome(id: &str, draft: &Value) -> Value {
    let state = draft["state"].as_str().unwrap_or_default();
    let mut out = json!({"status": state, "id": id});
    if let Some(message) = draft["message_id"].as_str().filter(|m| !m.is_empty()) {
        out["message_id"] = json!(message);
    }
    let error = draft["error"].as_str().filter(|e| !e.is_empty());
    if let Some(error) = error {
        out["error"] = json!(error);
    }
    let note = match (state, error) {
        ("sent", _) => return out,
        ("declined", _) => format!("o dono recusou; {DONT_REPEAT}"),
        ("expired", _) => format!("o dono não respondeu a tempo; {DONT_REPEAT} sem ele pedir de novo"),
        (_, Some("unconfirmed")) => format!("a mensagem pode ter saído (o WhatsApp não confirmou); {DONT_REPEAT}"),
        (_, Some("reachout_locked")) => format!("o WhatsApp travou mensagens a contatos novos; insistir leva a ban: {DONT_REPEAT}"),
        _ => format!("o envio falhou; {DONT_REPEAT}"),
    };
    out["note"] = json!(note);
    out
}

/// Os dados do WhatsApp foram apagados com o rascunho: o que houve com ele não se sabe mais.
fn gone(id: &str) -> Value {
    json!({"status": "failed", "id": id, "error": "not_found",
        "note": format!("o rascunho sumiu do serviço (dados do WhatsApp apagados) e não se sabe se a mensagem saiu; {DONT_REPEAT}")})
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

    fn draft(state: &str) -> Value {
        json!({"id": "d1", "state": state, "created_at": "2026-11-24T10:00:00Z", "expires_at": "2026-11-24T10:10:00Z",
            "answered_at": if state == "pending" { Value::Null } else { json!("2026-11-24T10:02:00Z") },
            "message_id": if state == "sent" { json!("3EB0ABC") } else { json!("") }, "error": "", "wa_code": null})
    }

    /// Prazo da chamada longe: só os prazos do binário contam.
    fn far() -> DateTime<Utc> {
        t("2099-01-01T00:00:00Z")
    }

    #[test]
    fn a_pending_draft_waits_until_it_expires() {
        let now = t("2026-11-24T10:05:00Z");
        assert_eq!(wait_step("d1", Some(&draft("pending")), now, far()), Wait::Until(t("2026-11-24T10:10:05Z")));
        assert_eq!(wait_step("d1", Some(&draft("pending")), t("2026-11-24T10:10:05Z"), far()), Wait::PendingOverdue);
        let mut bad = draft("pending");
        bad["expires_at"] = json!("ontem");
        assert!(matches!(wait_step("d1", Some(&bad), now, far()), Wait::Unknown(_)));
    }

    /// O envio aprovado tem até 10 min depois da aprovação, não depois da criação.
    #[test]
    fn a_sending_draft_waits_ten_minutes_after_the_approval() {
        assert_eq!(wait_step("d1", Some(&draft("sending")), t("2026-11-24T10:11:00Z"), far()), Wait::Until(t("2026-11-24T10:12:00Z")));
        assert_eq!(wait_step("d1", Some(&draft("sending")), t("2026-11-24T10:12:00Z"), far()), Wait::SendingOverdue);
    }

    /// O prazo da chamada vence antes dos do binário: pendente vira recusa; aprovado e saindo é resposta própria.
    #[test]
    fn the_call_deadline_ends_the_wait_first() {
        let deadline = t("2026-11-24T10:09:30Z");
        assert_eq!(wait_step("d1", Some(&draft("pending")), t("2026-11-24T10:05:00Z"), deadline), Wait::Until(deadline));
        assert_eq!(wait_step("d1", Some(&draft("pending")), deadline, deadline), Wait::PendingOverdue);
        assert_eq!(wait_step("d1", Some(&draft("sending")), t("2026-11-24T10:05:00Z"), deadline), Wait::Until(deadline));
        let Wait::Done(out) = wait_step("d1", Some(&draft("sending")), deadline, deadline) else { panic!("saindo no prazo") };
        assert_eq!((&out["status"], &out["id"]), (&json!("sending"), &json!("d1")));
        assert!(out["note"].as_str().unwrap().contains("whatsapp_read"), "{out}");
        assert_eq!(wait_step("d1", Some(&draft("sent")), deadline, deadline),
            Wait::Done(json!({"status": "sent", "id": "d1", "message_id": "3EB0ABC"})), "estado final vence o prazo");
    }

    #[test]
    fn final_states_are_the_tool_result() {
        let now = t("2026-11-24T10:05:00Z");
        assert_eq!(wait_step("d1", Some(&draft("sent")), now, far()), Wait::Done(json!({"status": "sent", "id": "d1", "message_id": "3EB0ABC"})));
        for state in ["declined", "expired", "failed"] {
            let Wait::Done(out) = wait_step("d1", Some(&draft(state)), now, far()) else { panic!("{state}") };
            assert_eq!((&out["status"], &out["id"]), (&json!(state), &json!("d1")));
            assert!(out.get("message_id").is_none() && out.get("error").is_none(), "{out}");
            assert!(out["note"].as_str().unwrap().contains(DONT_REPEAT), "{out}");
        }
        assert!(matches!(wait_step("d1", Some(&draft("rascunho")), now, far()), Wait::Unknown(_)));
    }

    /// `unconfirmed`: a mensagem talvez tenha saído; a tool diz isso e manda não repetir.
    #[test]
    fn an_unconfirmed_failure_says_the_message_may_have_left() {
        let mut failed = draft("failed");
        failed["error"] = json!("unconfirmed");
        let Wait::Done(out) = wait_step("d1", Some(&failed), t("2026-11-24T10:05:00Z"), far()) else { panic!() };
        assert_eq!(out["error"], json!("unconfirmed"));
        let note = out["note"].as_str().unwrap();
        assert!(note.contains("pode ter saído") && note.contains(DONT_REPEAT), "{note}");
    }

    /// Depois de um `wipe` o binário responde 404: estado final, sem saber se saiu.
    #[test]
    fn a_wiped_draft_is_final() {
        let Wait::Done(out) = wait_step("d1", None, t("2026-11-24T10:05:00Z"), far()) else { panic!() };
        assert_eq!((&out["status"], &out["id"], &out["error"]), (&json!("failed"), &json!("d1"), &json!("not_found")));
        assert!(out["note"].as_str().unwrap().contains(DONT_REPEAT));
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

    #[test]
    fn only_safe_ids_go_in_the_path() {
        assert!(id_ok("01JD7Z-abc_9"));
        for bad in ["", "../x", "a/b", "a?b", "a b", &"x".repeat(65)] {
            assert!(!id_ok(bad), "{bad}");
        }
    }
}
