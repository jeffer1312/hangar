//! A espera da tool `whatsapp_draft` até o dono responder no cartão, o rascunho expirar ou o prazo da chamada
//! vencer, e as frases que ela devolve em cada desfecho, inclusive na chamada cancelada.
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::at;

/// Teto do binário entre a aprovação e o fim do envio (upload de 100 MB).
const SENDING_SECS: i64 = 600;
/// O binário confere a expiração a cada pedido; a folga só cobre relógios um pouco diferentes.
const EXPIRY_GRACE_SECS: i64 = 5;
/// Releitura do rascunho saindo e da leitura que falhou: nesses casos a lista dos pendentes não muda e não acorda.
const REREAD: TimeDelta = TimeDelta::seconds(3);
/// Pendente, a mudança da lista acorda a espera; a volta da lista (`REFRESH`) é o teto, para a leitura da
/// lista que falhou não deixar a tool dormindo até o prazo.
const PENDING_REREAD: TimeDelta = TimeDelta::seconds(super::REFRESH.as_secs() as i64);
/// Quanto a chamada cancelada espera a recusa para dizer se o rascunho saiu da fila.
const DECLINE_WAIT: Duration = Duration::from_secs(10);
/// Folga entre o fim do trabalho depois do prazo e o menor corte que a sessão aceita: a resposta ainda tem de chegar.
const CUTOFF_MARGIN: TimeDelta = TimeDelta::seconds(5);

pub const DONT_REPEAT: &str = "não crie outro rascunho para o mesmo envio";
pub const DRAFT_MAYBE_CREATED: &str = "o rascunho pode ter sido criado; ele só sai se o dono aprovar e expira em 10 min: \
não crie outro rascunho para o mesmo envio";
const CANCELLED: &str = "whatsapp_draft_cancelled: a chamada foi cancelada";

/// A leitura do rascunho na espera.
#[derive(Clone, Debug)]
pub enum Read {
    Found(Value),
    /// 404 `not_found`: os dados do WhatsApp foram apagados.
    Gone,
    Failed(String),
}

/// O que a recusa do rascunho encontrou.
#[derive(Clone, Debug, PartialEq)]
pub enum Declined {
    Declined,
    Expired,
    /// O dono respondeu antes: o rascunho lido depois da recusa negada, `None` quando não se leu.
    Answered(Option<Value>),
    Unconfirmed,
}

impl Declined {
    /// A resposta da rota de recusa e, quando o dono já tinha respondido, a leitura do rascunho.
    pub fn of(status: u16, value: &Value, read: Option<(u16, Value)>) -> Self {
        match (status, value["detail"]["code"].as_str()) {
            (200, _) => Self::Declined,
            (409, Some("whatsapp_expired")) => Self::Expired,
            (409, Some("whatsapp_answered")) => Self::Answered(read.filter(|(status, _)| *status == 200).map(|(_, draft)| draft)),
            _ => Self::Unconfirmed,
        }
    }
}

/// O que a espera usa de fora: a rota do dono, o relógio e a pausa. A prova troca tudo, inclusive o relógio.
/// Leitura e recusa terminam até `by`; `None` quando não terminaram a tempo.
pub trait DraftPort {
    fn now(&self) -> DateTime<Utc>;
    fn read(&mut self, id: &str, by: DateTime<Utc>) -> impl std::future::Future<Output = Option<Read>> + Send;
    /// `Err` com a frase da rota quando a recusa não se confirmou.
    fn decline(&mut self, id: &str, by: DateTime<Utc>) -> impl std::future::Future<Output = Option<Result<Declined, String>>> + Send;
    /// Até `until`, ou antes quando a lista dos pendentes muda.
    fn pause(&mut self, until: DateTime<Utc>) -> impl std::future::Future<Output = ()> + Send;
}

/// O andamento do rascunho num lugar só: a espera escreve; o fim da chamada, cancelada ou largada, lê para decidir
/// se recusa e o que responder.
#[derive(Default)]
pub struct Progress(Mutex<Track>);

#[derive(Default)]
struct Track {
    id: Option<String>,
    /// O último estado lido: só o `pending` é recusado quando a espera é largada.
    last_state: String,
    done: bool,
    decline: Option<JoinHandle<Declined>>,
}

impl Progress {
    fn lock(&self) -> MutexGuard<'_, Track> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// O rascunho existe e nasce pendente.
    pub fn created(&self, id: &str) {
        let mut track = self.lock();
        track.id = Some(id.to_owned());
        track.last_state = "pending".into();
    }

    /// A espera foi largada com o rascunho ainda pendente: largar o futuro não basta, o rascunho mora no binário.
    pub fn needs_decline(&self) -> bool {
        let track = self.lock();
        !track.done && track.last_state == "pending"
    }

    /// A recusa disparada pela espera largada, que a chamada cancelada espera para responder.
    pub fn declining(&self, decline: JoinHandle<Declined>) {
        self.lock().decline = Some(decline);
    }

    fn saw(&self, state: &str) {
        state.clone_into(&mut self.lock().last_state);
    }

    fn finish(&self) {
        self.lock().done = true;
    }

    /// O texto da chamada cancelada: espera a recusa disparada e diz o que houve com o rascunho.
    pub async fn cancelled(&self) -> String {
        let (id, last_state, done, decline) = {
            let mut track = self.lock();
            (track.id.clone(), track.last_state.clone(), track.done, track.decline.take())
        };
        cancelled_text(id.as_deref(), &last_state, done, decline).await
    }
}

async fn cancelled_text(id: Option<&str>, last_state: &str, done: bool, decline: Option<JoinHandle<Declined>>) -> String {
    let Some(id) = id else {
        return format!("{CANCELLED} antes de o rascunho existir; {DRAFT_MAYBE_CREATED}");
    };
    let unconfirmed = || format!("{CANCELLED} e a recusa do rascunho {id} não se confirmou; ele só sai se o dono aprovar e expira \
em 10 min: {DONT_REPEAT}");
    let Some(decline) = decline else {
        return match (last_state, done) {
            ("sending", _) => format!("{CANCELLED}; rascunho {id}: {}", note(&still_sending(id))),
            (_, true) => format!("{CANCELLED} depois de o rascunho {id} chegar ao fim"),
            _ => unconfirmed(),
        };
    };
    match tokio::time::timeout(DECLINE_WAIT, decline).await {
        Ok(Ok(Declined::Declined)) => format!("{CANCELLED} e o rascunho {id} foi recusado; nada saiu"),
        Ok(Ok(Declined::Expired)) => format!("{CANCELLED} e o rascunho {id} já tinha expirado; nada saiu"),
        Ok(Ok(Declined::Answered(draft))) => format!("{CANCELLED} depois de o dono responder o rascunho {id}: {}", answered(id, draft.as_ref())),
        _ => unconfirmed(),
    }
}

/// O que o dono respondeu, pela mesma nota que a tool devolveria. Sem leitura, pode ter sido o "Enviar".
fn answered(id: &str, draft: Option<&Value>) -> String {
    let result = match draft.map(|d| (d, d["state"].as_str().unwrap_or_default())) {
        Some((_, "sending")) => still_sending(id),
        Some((draft, "sent" | "declined" | "expired" | "failed")) => outcome(id, draft),
        _ => return format!("a resposta não se leu e a mensagem pode sair: {DONT_REPEAT}"),
    };
    match result["note"].as_str() {
        Some(_) => note(&result),
        // O `sent` não leva nota: o estado já diz.
        None => format!("a mensagem saiu; {DONT_REPEAT}"),
    }
}

fn note(result: &Value) -> String {
    result["note"].as_str().unwrap_or_default().to_owned()
}

/// Espera o rascunho criado chegar ao fim até `deadline`, o prazo da chamada. Pendente no prazo é recusado e
/// responde `expired`; aprovado e ainda saindo responde `sending`. O que sobra depois do prazo (recusa, releitura)
/// termina antes do menor corte que a sessão aceita, senão a resposta não chegaria a quem pediu.
pub async fn wait_draft<P: DraftPort>(port: &mut P, progress: &Progress, id: &str, created: Value, deadline: DateTime<Utc>) -> Result<Value, String> {
    let cutoff = deadline - super::TOOL_DEADLINE + super::CALL_CUTOFF_MIN - CUTOFF_MARGIN;
    let mut read = Read::Found(created);
    let mut until = None;
    // A recusa do prazo foi negada porque o dono respondeu: daí em diante, sem leitura, a mensagem pode sair.
    let mut answered_late = false;
    loop {
        let now = port.now();
        let step = match &read {
            Read::Failed(_) if answered_late => return Err(answered_unread(id, progress)),
            // Serviço reiniciando no meio da espera: tenta de novo até o prazo do último estado lido.
            Read::Failed(error) => match until {
                Some(at) if now < at => Wait::Until(at),
                _ => return Err(after_creation(id, error)),
            },
            Read::Found(draft) => {
                progress.saw(draft["state"].as_str().unwrap_or_default());
                wait_step(id, Some(draft), now, deadline)
            }
            Read::Gone => wait_step(id, None, now, deadline),
        };
        let pending = matches!(&read, Read::Found(draft) if draft["state"] == "pending");
        let wake = match step {
            Wait::Done(result) => {
                progress.finish();
                return Ok(result);
            }
            Wait::Until(at) => {
                until = Some(at);
                // Pendente, a resposta do dono muda a lista dos pendentes e acorda a pausa; nos outros casos nada avisa.
                (now + if pending { PENDING_REREAD } else { REREAD }).min(at)
            }
            Wait::PendingOverdue => match port.decline(id, cutoff).await {
                None => return Err(cut_off(id, answered_late, progress)),
                Some(Ok(Declined::Declined | Declined::Expired)) => {
                    progress.finish();
                    return Ok(expired(id));
                }
                // O dono respondeu no último instante: o estado real decide.
                Some(Ok(Declined::Answered(_))) => {
                    answered_late = true;
                    let Some(next) = port.read(id, cutoff).await else { return Err(cut_off(id, answered_late, progress)) };
                    read = next;
                    continue;
                }
                Some(Ok(Declined::Unconfirmed)) => return Err(after_creation(id, "whatsapp_unreachable: a recusa no prazo não se confirmou")),
                Some(Err(error)) => return Err(after_creation(id, &error)),
            },
            Wait::SendingOverdue => {
                progress.finish();
                return Err(sending_overdue(id));
            }
            Wait::Unknown(what) => return Err(after_creation(id, &format!("whatsapp_bad_reply: {what}"))),
        };
        port.pause(wake).await;
        let Some(next) = port.read(id, cutoff).await else { return Err(cut_off(id, answered_late, progress)) };
        read = next;
    }
}

/// O dono respondeu à recusa do prazo e a leitura seguinte falhou: diz que ele respondeu e que a mensagem pode sair.
/// A espera chega ao fim ali: a chamada largada não recusa de novo o que o dono já respondeu.
fn answered_unread(id: &str, progress: &Progress) -> String {
    progress.finish();
    format!("whatsapp_draft_unconfirmed: o dono respondeu o rascunho {id} no fim do prazo; {}", answered(id, None))
}

/// O tempo depois do prazo acabou sem resposta do serviço. Sem resposta do dono, o rascunho segue pendente e a
/// chamada largada o recusa; o que houve com ele não se sabe.
fn cut_off(id: &str, answered_late: bool, progress: &Progress) -> String {
    if answered_late { return answered_unread(id, progress); }
    format!("whatsapp_draft_unconfirmed: o prazo da chamada acabou sem resposta do serviço sobre o rascunho {id}; o dono pode ter \
aprovado e a mensagem pode sair: {DONT_REPEAT}")
}

/// Erro depois de o rascunho existir: o id vai junto, para o agente não pedir outro.
fn after_creation(id: &str, error: &str) -> String {
    format!("{error}; rascunho {id}: {DONT_REPEAT}")
}

fn sending_overdue(id: &str) -> String {
    format!("whatsapp_draft_unconfirmed: o rascunho {id} foi aprovado e não terminou de sair em 10 min; a mensagem pode ter saído: {DONT_REPEAT}")
}

/// O que a tool faz com a última leitura do rascunho.
#[derive(Clone, Debug, PartialEq)]
enum Wait {
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
fn wait_step(id: &str, draft: Option<&Value>, now: DateTime<Utc>, call_deadline: DateTime<Utc>) -> Wait {
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
fn still_sending(id: &str) -> Value {
    json!({"status": "sending", "id": id,
        "note": "o dono aprovou; a mensagem está saindo. Não crie outro rascunho; confira com whatsapp_read"})
}

/// Recusado pela própria tool depois de expirar sem resposta do dono.
fn expired(id: &str) -> Value {
    outcome(id, &json!({"state": "expired"}))
}

/// O resultado da tool para um estado final: `message_id` e `error` só quando vieram.
fn outcome(id: &str, draft: &Value) -> Value {
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn t(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn draft(state: &str) -> Value {
        json!({"id": "d1", "state": state, "created_at": "2026-11-24T10:00:00Z", "expires_at": "2026-11-24T10:10:00Z",
            "answered_at": if state == "pending" { Value::Null } else { json!("2026-11-24T10:01:00Z") },
            "message_id": if state == "sent" { json!("3EB0ABC") } else { json!("") }, "error": "", "wa_code": null})
    }

    /// Prazo da chamada longe: só os prazos do binário contam.
    fn far() -> DateTime<Utc> {
        t("2099-01-01T00:00:00Z")
    }

    fn t0() -> DateTime<Utc> {
        t("2026-11-24T10:00:00Z")
    }

    fn deadline() -> DateTime<Utc> {
        t0() + super::super::TOOL_DEADLINE
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

    /// O envio aprovado tem o prazo `SENDING_SECS` contado da aprovação, não da criação.
    #[test]
    fn a_sending_draft_waits_ten_minutes_after_the_approval() {
        assert_eq!(wait_step("d1", Some(&draft("sending")), t("2026-11-24T10:10:00Z"), far()), Wait::Until(t("2026-11-24T10:11:00Z")));
        assert_eq!(wait_step("d1", Some(&draft("sending")), t("2026-11-24T10:11:00Z"), far()), Wait::SendingOverdue);
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
        assert_eq!(expired("d1")["note"], json!(format!("o dono não respondeu a tempo; {DONT_REPEAT} sem ele pedir de novo")));
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

    /// Relógio e respostas de mentira. Cada pausa anda `step` segundos sem passar do `until` pedido (a lista dos
    /// pendentes mudando antes); cada leitura e recusa leva `cost` segundos e, sem caber até o `by`, para nele sem
    /// resposta. O que acabar o roteiro repete `last`.
    struct FakePort { now: DateTime<Utc>, step: i64, cost: i64, reads: VecDeque<Read>, last: Read, decline: Result<Declined, String>,
        declined: usize, pauses: Vec<(DateTime<Utc>, DateTime<Utc>)> }

    impl FakePort {
        fn new(reads: Vec<Read>, last: Read) -> Self {
            Self { now: t0(), step: 60, cost: 0, reads: reads.into(), last, decline: Ok(Declined::Declined), declined: 0, pauses: Vec::new() }
        }

        fn spend(&mut self, by: DateTime<Utc>) -> bool {
            let done = self.now + TimeDelta::seconds(self.cost);
            if done > by {
                self.now = self.now.max(by);
                return false;
            }
            self.now = done;
            true
        }
    }

    impl DraftPort for FakePort {
        fn now(&self) -> DateTime<Utc> { self.now }
        async fn read(&mut self, _: &str, by: DateTime<Utc>) -> Option<Read> {
            if !self.spend(by) { return None; }
            Some(self.reads.pop_front().unwrap_or_else(|| self.last.clone()))
        }
        async fn decline(&mut self, _: &str, by: DateTime<Utc>) -> Option<Result<Declined, String>> {
            self.declined += 1;
            if !self.spend(by) { return None; }
            Some(self.decline.clone())
        }
        async fn pause(&mut self, until: DateTime<Utc>) {
            self.pauses.push((self.now, until));
            self.now = (self.now + TimeDelta::seconds(self.step)).min(until);
        }
    }

    fn progress() -> Progress {
        let progress = Progress::default();
        progress.created("d1");
        progress
    }

    /// O dono não responde: pendente, a espera dorme em voltas de `PENDING_REREAD` (a lista acorda antes quando muda) até o prazo
    /// da chamada, recusa e responde `expired`.
    #[tokio::test]
    async fn a_pending_draft_sleeps_to_the_call_deadline_and_is_declined_as_expired() {
        let mut port = FakePort::new(vec![], Read::Found(draft("pending")));
        port.step = 10_000;
        let progress = progress();
        let out = wait_draft(&mut port, &progress, "d1", draft("pending"), deadline()).await.unwrap();
        assert_eq!((&out["status"], &out["id"]), (&json!("expired"), &json!("d1")), "{out}");
        assert_eq!(port.declined, 1);
        assert_eq!(port.now, deadline());
        assert_eq!(port.pauses.len(), 19, "570 s em voltas de 30 s: {:?}", port.pauses);
        assert!(port.pauses.iter().all(|(from, until)| *until - *from == TimeDelta::seconds(30)), "{:?}", port.pauses);
        assert!(!progress.needs_decline(), "a espera chegou ao fim");
    }

    /// Aprovado e ainda saindo no prazo: relê a cada `REREAD` (a lista dos pendentes não avisa) e responde `sending`, sem
    /// recusar nada.
    #[tokio::test]
    async fn an_approved_draft_is_reread_every_few_seconds_and_answers_sending_at_the_deadline() {
        let mut port = FakePort::new(vec![Read::Found(draft("pending"))], Read::Found(draft("sending")));
        let progress = progress();
        let out = wait_draft(&mut port, &progress, "d1", draft("pending"), deadline()).await.unwrap();
        assert_eq!((&out["status"], &out["id"]), (&json!("sending"), &json!("d1")), "{out}");
        assert!(out["note"].as_str().unwrap().contains("whatsapp_read"));
        assert_eq!(port.declined, 0);
        let sending = &port.pauses[2..];
        assert!(!sending.is_empty() && sending.iter().all(|(from, until)| *until - *from <= TimeDelta::seconds(3)), "{sending:?}");
        assert!(!progress.needs_decline());
    }

    #[tokio::test]
    async fn the_owner_answer_ends_the_wait() {
        let mut port = FakePort::new(vec![Read::Found(draft("pending"))], Read::Found(draft("declined")));
        let out = wait_draft(&mut port, &progress(), "d1", draft("pending"), deadline()).await.unwrap();
        assert_eq!(out["status"], json!("declined"));
        let mut port = FakePort::new(vec![Read::Found(draft("sending"))], Read::Found(draft("sent")));
        let out = wait_draft(&mut port, &progress(), "d1", draft("pending"), deadline()).await.unwrap();
        assert_eq!(out, json!({"status": "sent", "id": "d1", "message_id": "3EB0ABC"}));
        assert_eq!(port.declined, 0);
    }

    /// A recusa do prazo negada porque o dono respondeu no último instante: vale o estado relido.
    #[tokio::test]
    async fn a_last_moment_answer_wins_over_the_deadline_decline() {
        let mut port = FakePort::new(vec![], Read::Found(draft("declined")));
        port.decline = Ok(Declined::Answered(None));
        port.now = deadline();
        let out = wait_draft(&mut port, &progress(), "d1", draft("pending"), deadline()).await.unwrap();
        assert_eq!(out["status"], json!("declined"), "{out}");
        assert_eq!(port.declined, 1);
    }

    /// O dono respondeu à recusa do prazo e a releitura falhou: a tool diz que ele respondeu e que a mensagem pode sair,
    /// e a chamada largada depois não recusa de novo.
    #[tokio::test]
    async fn an_unread_last_moment_answer_says_the_message_may_leave() {
        let mut port = FakePort::new(vec![], Read::Failed("whatsapp_unreachable: sem resposta".into()));
        port.decline = Ok(Declined::Answered(None));
        port.now = deadline();
        let progress = progress();
        let error = wait_draft(&mut port, &progress, "d1", draft("pending"), deadline()).await.unwrap_err();
        assert!(error.starts_with("whatsapp_draft_unconfirmed:") && error.contains("o dono respondeu o rascunho d1"), "{error}");
        assert!(error.contains("a mensagem pode sair") && error.contains(DONT_REPEAT), "{error}");
        assert_eq!(port.declined, 1);
        assert!(!progress.needs_decline(), "o dono já respondeu: nada a recusar");
    }

    /// Recusa e releitura depois do prazo da tool param antes do menor corte que a sessão aceita, mesmo com o serviço
    /// lento e o laço repetindo: a resposta tem de chegar a quem pediu.
    #[tokio::test]
    async fn the_work_after_the_deadline_ends_before_the_shortest_cutoff() {
        let cutoff = deadline() - super::super::TOOL_DEADLINE + super::super::CALL_CUTOFF_MIN - CUTOFF_MARGIN;
        assert!(cutoff > deadline());

        // A recusa não responde a tempo: estado desconhecido, e o rascunho segue para a chamada largada recusar.
        let mut port = FakePort::new(vec![], Read::Found(draft("pending")));
        port.now = deadline();
        port.cost = 60;
        let unanswered = progress();
        let error = wait_draft(&mut port, &unanswered, "d1", draft("pending"), deadline()).await.unwrap_err();
        assert!(error.starts_with("whatsapp_draft_unconfirmed:") && error.contains("rascunho d1"), "{error}");
        assert!(error.contains("a mensagem pode sair") && error.contains(DONT_REPEAT), "{error}");
        assert_eq!(port.now, cutoff);
        assert!(unanswered.needs_decline(), "sem resposta do dono, a chamada largada recusa");

        // O dono respondeu, a releitura volta pendente e o laço recusa de novo: o corte para tudo no limite.
        let mut port = FakePort::new(vec![], Read::Found(draft("pending")));
        port.decline = Ok(Declined::Answered(None));
        port.now = deadline();
        port.cost = 8;
        let answered = progress();
        let error = wait_draft(&mut port, &answered, "d1", draft("pending"), deadline()).await.unwrap_err();
        assert!(error.contains("o dono respondeu o rascunho d1") && error.contains("a mensagem pode sair"), "{error}");
        assert!(port.now <= cutoff, "{} > {cutoff}", port.now);
        assert!(port.declined >= 2, "o laço repetiu: {}", port.declined);
        assert!(!answered.needs_decline());
    }

    /// Serviço que não responde: relê a cada `REREAD` até o prazo e para com o id.
    #[tokio::test]
    async fn read_failures_stop_at_the_deadline_with_the_id() {
        let mut port = FakePort::new(vec![], Read::Failed("whatsapp_unreachable: sem resposta".into()));
        let error = wait_draft(&mut port, &progress(), "d1", draft("pending"), deadline()).await.unwrap_err();
        assert!(error.starts_with("whatsapp_unreachable:") && error.contains("rascunho d1") && error.contains(DONT_REPEAT), "{error}");
        assert!(port.now >= deadline());
        assert!(port.pauses[1..].iter().all(|(from, until)| *until - *from <= TimeDelta::seconds(3)), "{:?}", port.pauses);
    }

    /// Só o rascunho que a espera largou ainda pendente é recusado; aprovado ou terminado, não.
    #[test]
    fn only_a_pending_draft_is_declined_when_the_wait_is_dropped() {
        let progress = progress();
        assert!(progress.needs_decline());
        progress.saw("sending");
        assert!(!progress.needs_decline(), "aprovado: o envio segue");
        progress.saw("pending");
        progress.finish();
        assert!(!progress.needs_decline(), "a espera respondeu");
    }

    #[tokio::test]
    async fn the_cancelled_call_says_what_happened_to_the_draft() {
        let done = |declined: Declined| Some(tokio::spawn(async move { declined }));
        let text = cancelled_text(Some("d1"), "pending", false, done(Declined::Declined)).await;
        assert!(text.starts_with("whatsapp_draft_cancelled:") && text.contains("d1") && text.contains("nada saiu"), "{text}");
        let text = cancelled_text(Some("d1"), "sending", false, None).await;
        assert!(text.contains("a mensagem está saindo"), "aprovado antes do cancelamento: {text}");
        let text = cancelled_text(Some("d1"), "pending", false, done(Declined::Unconfirmed)).await;
        assert!(text.contains("não se confirmou") && text.contains(DONT_REPEAT), "{text}");
        let text = cancelled_text(None, "", false, None).await;
        assert!(text.contains("pode ter sido criado"), "{text}");
    }

    /// A recusa negada porque o dono já respondeu diz o que ele respondeu, pela nota da tool: recusado pelo dono não é
    /// "pode sair".
    #[tokio::test]
    async fn a_refused_decline_answers_by_the_real_state() {
        let answered = json!({"detail": {"code": "whatsapp_answered"}});
        let read = |state: &str, error: &str| Some((200, json!({"id": "d1", "state": state, "error": error})));
        let of = |state: &str, error: &str| Declined::of(409, &answered, read(state, error));
        assert_eq!(of("declined", ""), Declined::Answered(Some(json!({"id": "d1", "state": "declined", "error": ""}))));
        let text = |declined: Declined| cancelled_text(Some("d1"), "pending", false, Some(tokio::spawn(async move { declined })));
        let declined = text(of("declined", "")).await;
        assert!(declined.contains("o dono recusou") && !declined.contains("pode sair"), "{declined}");
        assert!(text(of("sending", "")).await.contains("a mensagem está saindo"));
        assert!(text(of("sent", "")).await.contains("a mensagem saiu"));
        assert!(text(of("failed", "unconfirmed")).await.contains("pode ter saído"));
        let unread = text(Declined::of(409, &answered, Some((502, json!({}))))).await;
        assert!(unread.contains("a mensagem pode sair") && unread.contains(DONT_REPEAT), "{unread}");
        assert_eq!(Declined::of(200, &json!({}), None), Declined::Declined);
        assert_eq!(Declined::of(409, &json!({"detail": {"code": "whatsapp_expired"}}), None), Declined::Expired);
        assert_eq!(Declined::of(502, &json!({}), None), Declined::Unconfirmed);
    }
}
