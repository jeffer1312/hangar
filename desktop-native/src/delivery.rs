use std::collections::{HashMap, HashSet, VecDeque};
use crate::api::dto::{SessionInfo,ChatEvent};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionKey {
    pub server: String,
    pub name: String,
    pub jsonl: String,
}

impl SessionKey {
    pub fn new(server: &str, session: &SessionInfo) -> Option<Self> {
        if !session.readable() { return None; }
        Some(Self { server: server.into(), name: session.name.clone(), jsonl: session.jsonl.clone()? })
    }

    pub fn for_open_chat(server: &str, session: &SessionInfo) -> Option<Self> {
        Self::new(server, session).or_else(|| {
            if !session.waiting_for_first_input() { return None; }
            // Cada vida guarda o próprio rascunho até o vínculo com o histórico.
            Self::opening(server, session)
        })
    }

    /// A chave de antes do histórico desta vida e a do histórico, quando ele já existe; outra vida não casa.
    pub fn opening_to_transcript(server: &str, session: &SessionInfo) -> Option<(Self, Self)> {
        Some((Self::opening(server, session)?, Self::new(server, session)?))
    }

    fn opening(server: &str, session: &SessionInfo) -> Option<Self> {
        let life = session.lifecycle_id.as_deref()?;
        Some(Self { server: server.into(), name: session.name.clone(), jsonl: format!("opening:{life}") })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    Delivered,
    Queued,
    Uncertain,
    Rejected(String),
}

/// Folga para o relógio desta máquina adiantado em relação ao servidor.
const ECHO_SLACK_SECS: f64 = 30.0;

#[derive(Clone, Debug)]
struct SendRecord {
    text: String,
    known: HashSet<String>,
    pending: bool,
    confirmed: bool,
    /// Segundos Unix do Enter: linha mais velha que isso (menos a folga) é de outro envio.
    started: f64,
    /// A conversa mostrou esta mensagem; a pergunta lateral confirmada no Enter não conta como prova.
    echoed: bool,
    /// O eco que provou veio sem horário.
    echo_untimed: bool,
    /// Linha sem horário prova este envio: o histórico estava na tela no Enter e nenhum igual estava a caminho.
    untimed: bool,
    outcome: Option<SendOutcome>,
    /// O que saiu do campo no Enter: a bolha mostra isso enquanto o envio não responde e, se falhar, volta ao campo.
    /// Com anexo é a legenda; o `text` leva também os caminhos.
    field: Option<String>,
    history: Option<String>,
}

#[derive(Default)]
pub struct DeliveryTracker {
    records: HashMap<SessionKey, SendRecord>,
    /// Texto mandado com outro envio em voo, na ordem; modo e grupo são os decididos no Enter.
    held: HashMap<SessionKey, VecDeque<Held>>,
    /// Transcript trocado na mesma sessão (`/clear`): a resposta do envio em voo chega com a chave antiga.
    moved: HashMap<SessionKey, SessionKey>,
    /// Envios incertos dados por chegados por uma linha sem horário: uma igual de outro aparelho também prova, e o
    /// rastro fica no log.
    untimed_proofs: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SendTiming { Queued, Direct, Jev { task: String, message: String }, Conversation }

/// A fala do agente dá sentido a respostas curtas que o pedido inicial, sozinho, não explica.
pub fn timing_context(task: &str, recent: &str) -> String {
    if task.trim().is_empty() && recent.trim().is_empty() { return String::new(); }
    if recent.trim().is_empty() { return task.chars().take(1000).collect(); }
    // Perguntas e pedidos de confirmação costumam fechar a fala do agente.
    let recent = recent.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect::<String>();
    format!("Pedido atual:\n{}\n\nFala recente do agente:\n{}",
        task.chars().take(350).collect::<String>(), recent)
}

impl From<bool> for SendTiming {
    fn from(direct: bool) -> Self { if direct { Self::Direct } else { Self::Queued } }
}

impl SendTiming {
    /// Sem a identidade do turno no Enter, uma espera local não pode orientar automaticamente outro trabalho.
    pub fn after_wait(self) -> Self {
        match self { Self::Jev { message, .. } => Self::Jev { task: String::new(), message }, other => other }
    }
}

pub type Held = (String, String, SendTiming, Option<Vec<String>>);

/// Envio incerto que a conversa mostrou: o que voltou ao campo, o texto que saiu e o que entra no histórico do campo.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrived {
    pub field: String,
    pub text: String,
    pub history: Option<String>,
}

impl DeliveryTracker {
    pub fn begin(&mut self, key: SessionKey, text: String, known: HashSet<String>) -> bool {
        self.begin_beside(key, text, known, &[])
    }

    /// `queued`: os textos do painel "Na fila" no Enter.
    pub fn begin_beside(&mut self, key: SessionKey, text: String, known: HashSet<String>, queued: &[String]) -> bool {
        if self.pending(&key) { return false; }
        // A pergunta lateral nunca vira mensagem da conversa: a resposta do backend já é a confirmação.
        let confirmed = crate::composer::side_question(&text);
        let same = |other: &str| matches_real(other, &text);
        let previous = self.records.get(&key).filter(|record| same(record.text.as_str()));
        // Um igual ainda a caminho ecoaria sem horário como se fosse este: aí só o horário prova.
        // No reenvio do incerto, o igual a caminho é a mesma mensagem, e o eco dele vale.
        let resend = previous.is_some_and(|record| matches!(record.outcome, Some(SendOutcome::Uncertain)));
        let ambiguous = !resend && (previous.is_some_and(|record| matches!(record.outcome, Some(SendOutcome::Delivered | SendOutcome::Queued)))
            || self.held.get(&key).into_iter().flatten().any(|(held, ..)| same(held.as_str()))
            || queued.iter().any(|entry| same(entry.as_str())));
        let untimed = !known.is_empty() && !ambiguous;
        self.records.insert(key, SendRecord { text, known, pending: true, confirmed, started: now_secs(), echoed: false, echo_untimed: false, untimed,
            outcome: None, field: None, history: None });
        true
    }

    /// O envio em voo saiu do campo: o texto dele fica à vista e, se falhar, volta para lá.
    pub fn mark_typed(&mut self, key: &SessionKey, field: String) {
        self.mark_history(key, field.clone());
        if let Some(record) = self.records.get_mut(key).filter(|record| record.pending) { record.field = Some(field); }
    }

    /// Comando escolhido pela sugestão: o histórico usa o comando completo, e a limpeza conserva o rascunho original.
    pub fn mark_history(&mut self, key: &SessionKey, text: String) {
        if let Some(record) = self.records.get_mut(key).filter(|record| record.pending) { record.history = Some(text); }
    }

    pub fn history(&self, key: &SessionKey, text: &str) -> Option<String> {
        self.records.get(key).filter(|record| record.pending && record.text == text).and_then(|record| record.history.clone())
    }

    pub fn typed(&self, key: &SessionKey, text: &str) -> Option<String> {
        self.records.get(key).filter(|record| record.pending && record.text == text).and_then(|record| record.field.clone())
    }

    /// Enter com outro envio em voo: o texto espera a vez em vez de se perder.
    pub fn hold(&mut self, key: SessionKey, text: String, field: String, timing: impl Into<SendTiming>, group: Option<Vec<String>>) {
        self.held.entry(key).or_default().push_back((text, field, timing.into(), group));
    }

    pub fn next_held(&mut self, key: &SessionKey) -> Option<Held> {
        let queue = self.held.get_mut(key)?;
        let next = queue.pop_front();
        if queue.is_empty() { self.held.remove(key); }
        next
    }

    /// O envio falhou: o que esperava atrás dele sai da espera e volta para o campo.
    pub fn take_held(&mut self, key: &SessionKey) -> Vec<String> {
        self.held.remove(key).map(|queue| queue.into_iter().map(|(_, field, ..)| field).collect()).unwrap_or_default()
    }

    /// O que saiu do campo e o backend ainda não respondeu, na ordem do envio.
    pub fn outgoing(&self, key: &SessionKey) -> Vec<&str> {
        // Já confirmado pela conversa: a mensagem real está à vista e a bolha seria a segunda.
        let flying = self.records.get(key).filter(|record| record.pending && !record.confirmed).and_then(|record| record.field.as_deref())
            .filter(|field| !field.is_empty());
        flying.into_iter().chain(self.held.get(key).into_iter().flatten().map(|(_, field, ..)| field.as_str())).collect()
    }

    /// O transcript da sessão trocou sem ela deixar de existir: envio em voo e espera seguem com ela.
    pub fn rekey(&mut self, from: &SessionKey, to: &SessionKey) {
        if from == to { return; }
        if let Some(record) = self.records.remove(from) { self.records.insert(to.clone(), record); }
        if let Some(queue) = self.held.remove(from) { self.held.entry(to.clone()).or_default().extend(queue); }
        for target in self.moved.values_mut().filter(|target| *target == from) { *target = to.clone(); }
        self.moved.insert(from.clone(), to.clone());
    }

    /// A chave de hoje de quem respondeu com a de antes da troca de transcript.
    pub fn current(&self, key: SessionKey) -> SessionKey {
        self.moved.get(&key).cloned().unwrap_or(key)
    }

    pub fn pending(&self, key: &SessionKey) -> bool {
        self.records.get(key).is_some_and(|record| record.pending)
    }

    pub fn outcome(&self, key: &SessionKey) -> Option<&SendOutcome> {
        self.records.get(key).and_then(|record| record.outcome.as_ref())
    }

    pub fn complete(&mut self, key: &SessionKey, text: &str, outcome: SendOutcome) -> bool {
        let Some(record) = self.records.get_mut(key) else { return false; };
        if !record.pending || record.text != text { return false; }
        record.pending = false;
        let settled = match &outcome {
            SendOutcome::Delivered | SendOutcome::Queued => record.confirmed,
            // A mensagem já apareceu na conversa durante o voo: a resposta cortada não deixa dúvida.
            SendOutcome::Uncertain => record.echoed,
            SendOutcome::Rejected(_) => false,
        };
        let untimed_proof = settled && matches!(outcome, SendOutcome::Uncertain) && record.echo_untimed;
        if settled { self.records.remove(key); }
        else { record.outcome = Some(outcome); }
        self.untimed_proofs += usize::from(untimed_proof);
        true
    }

    /// Quantos envios incertos uma linha sem horário deu por chegados desde a última leitura.
    pub fn take_untimed_proofs(&mut self) -> usize {
        std::mem::take(&mut self.untimed_proofs)
    }

    /// Devolve o envio incerto que esta linha prova, já fora do registro.
    pub fn confirm_real(&mut self, key: &SessionKey, id: &str, ts: Option<f64>, real_text: &str) -> Option<Arrived> {
        let record = self.records.get_mut(key)?;
        if record.known.contains(id) { return None; }
        if !matches_real(real_text, &record.text) { return None; }
        let proved = proves(record, id, ts);
        if record.pending {
            record.confirmed = true;
            if proved {
                // Um eco com horário vale mais que um sem.
                record.echo_untimed = if record.echoed { record.echo_untimed && ts.is_none() } else { ts.is_none() };
                record.echoed = true;
            }
            return None;
        }
        match record.outcome.clone() {
            Some(SendOutcome::Delivered | SendOutcome::Queued) => { self.records.remove(key); None }
            Some(SendOutcome::Uncertain) if proved => {
                let SendRecord { field, text, history, .. } = self.records.remove(key)?;
                self.untimed_proofs += usize::from(ts.is_none());
                Some(Arrived { field: field.unwrap_or_else(|| text.clone()), text, history })
            }
            _ => None,
        }
    }

    pub fn confirm_history(&mut self, key: &SessionKey, events: &[ChatEvent]) -> Option<Arrived> {
        let record = self.records.get(key)?;
        // Uma página antiga com o mesmo texto não confirma um envio novo.
        let start = match events.iter().rposition(|event| record.known.contains(&event.id)) {
            Some(frontier) => frontier + 1,
            // Sem nada à vista no Enter, só o horário da linha separa este envio de um antigo (`proves`).
            None if record.known.is_empty() && matches!(record.outcome, Some(SendOutcome::Uncertain)) => 0,
            None => return None,
        };
        for event in &events[start..] {
            if event.kind == "user_msg" && let Some(text) = event.text.as_deref()
                && let Some(arrived) = self.confirm_real(key, &event.id, event.ts, text) {
                return Some(arrived);
            }
        }
        None
    }


    /// Texto digitado agora no terminal que a conversa ainda não mostrou: é o que a interrupção devolve.
    /// O que foi para a fila durável (`Queued`) não está no campo do terminal e segue entregue por ela.
    pub fn take_unconfirmed(&mut self, key: &SessionKey) -> Option<String> {
        if !self.has_unconfirmed(key) { return None; }
        self.records.remove(key).map(|record| record.text)
    }

    /// Há o que `take_unconfirmed` devolveria, sem tirar: o Parar só o devolve depois do Esc confirmado.
    pub fn has_unconfirmed(&self, key: &SessionKey) -> bool {
        self.records.get(key).is_some_and(|record| !record.pending && matches!(record.outcome, Some(SendOutcome::Delivered)))
    }
}

/// A linha é desta conversa e deste envio: nem a fila (`queued-…`), nem uma igual de antes do Enter.
fn proves(record: &SendRecord, id: &str, ts: Option<f64>) -> bool {
    if id.starts_with("queued-") { return false; }
    match ts {
        Some(ts) => ts >= record.started - ECHO_SLACK_SECS,
        // A fala do Claude vem sem horário: só vale com o histórico na tela no Enter e sem outro igual a caminho.
        None => record.untimed,
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

pub fn matches_real(real: &str, sent: &str) -> bool {
    let real = real.trim();
    let sent = sent.trim();
    if sent.is_empty() { return false; }
    // Mensagem com anexo: o transcript pode gravar só a legenda.
    let legend = crate::composer::caption(sent);
    real == sent || real.lines().any(|line| line.trim() == sent)
        || sent.chars().count() >= 8 && real.lines().any(|line| line.trim().starts_with(sent))
        || legend != sent && !legend.is_empty() && crate::composer::caption(real) == legend
        // Mensagens que chegam no meio do turno o Claude grava num `user` só, separadas por quebra de linha.
        || format!("\n{real}\n").contains(&format!("\n{sent}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn timing_context_keeps_the_recent_question_even_with_a_long_original_request() {
        let context = timing_context(&"Descrição extensa da tarefa. ".repeat(100), "As PMs prontas também ficam na sprint anterior?");
        assert!(context.contains("As PMs prontas também ficam na sprint anterior?"));
        assert!(context.chars().count() <= 1000);
        let long_explanation = format!("{} Posso incluir a coluna?", "Detalhes do trabalho. ".repeat(50));
        assert!(timing_context("Ajustar a tabela", &long_explanation).ends_with("Posso incluir a coluna?"));
        assert!(timing_context("", "").is_empty());
        assert!(timing_context(&"á".repeat(2000), &"é".repeat(2000)).chars().count() <= 1000);
    }

    fn key(jsonl: &str) -> SessionKey {
        SessionKey { server: "http://fixture/".into(), name: "same-name".into(), jsonl: jsonl.into() }
    }

    #[test]
    fn kimi_and_antigravity_can_start_with_a_first_message_before_the_transcript_exists() {
        let mut session = SessionInfo {
            name: "kimi".into(), provider: "kimi".into(), state: "idle".into(), tracked: Some(false),
            lifecycle_id: Some("k:first".into()), ..Default::default()
        };
        assert!(!session.readable());
        assert!(session.takes_messages());
        let before = SessionKey::for_open_chat("http://fixture/", &session).unwrap();
        let reopened = SessionInfo { lifecycle_id: Some("k:second".into()), ..session.clone() };
        assert_ne!(SessionKey::for_open_chat("http://fixture/", &reopened).unwrap(), before);
        let linked = SessionInfo { tracked: Some(true), jsonl: Some("/wire.jsonl".into()), ..session.clone() };
        let linked_key = SessionKey::for_open_chat("http://fixture/", &linked).unwrap();
        assert_eq!(linked_key.jsonl, "/wire.jsonl");
        assert_eq!(SessionKey::opening_to_transcript("http://fixture/", &linked), Some((before.clone(), linked_key.clone())));
        assert_eq!(SessionKey::opening_to_transcript("http://fixture/", &session), None, "sem histórico não há para onde ir");
        let other_life = SessionInfo { lifecycle_id: Some("k:second".into()), ..linked.clone() };
        assert_ne!(SessionKey::opening_to_transcript("http://fixture/", &other_life).unwrap().0, before, "outra vida não leva o rascunho desta");
        let mut delivery = DeliveryTracker::default();
        assert!(delivery.begin(before.clone(), "primeira mensagem".into(), HashSet::new()));
        delivery.rekey(&before, &linked_key);
        assert_eq!(delivery.current(before), linked_key);
        assert!(delivery.pending(&linked_key));

        session.state = "dead".into();
        assert!(!session.takes_messages());
        session.state = "idle".into();
        session.lifecycle_id = Some("t:1".into());
        assert!(!session.takes_messages());
        session.lifecycle_id = Some("k:first".into());
        session.guest_kind = Some("pair".into());
        assert!(!session.takes_messages());
        let agy = SessionInfo { name: "agy".into(), provider: "antigravity".into(), guest_kind: None, ..session.clone() };
        assert!(agy.takes_messages(), "o agy também só cria a conversa no primeiro envio");
        assert!(!SessionInfo { provider: "pi".into(), ..agy }.takes_messages());
    }

    #[test]
    fn input_history_belongs_to_the_current_delivery_even_with_equal_held_text() {
        let mut tracker = DeliveryTracker::default();
        let key = key("one");
        assert!(tracker.begin(key.clone(), "mensagem".into(), HashSet::new()));
        tracker.hold(key.clone(), "mensagem".into(), "texto do campo".into(), false, None);
        assert_eq!(tracker.history(&key, "mensagem"), None);
        assert!(tracker.complete(&key, "mensagem", SendOutcome::Delivered));
        let (wire, field, _, _) = tracker.next_held(&key).unwrap();
        assert!(tracker.begin(key.clone(), wire.clone(), HashSet::new()));
        tracker.mark_typed(&key, field);
        assert_eq!(tracker.history(&key, &wire).as_deref(), Some("texto do campo"));
        assert!(tracker.complete(&key, &wire, SendOutcome::Rejected("recusada".into())));
        assert_eq!(tracker.history(&key, &wire), None);
        assert!(tracker.begin(key.clone(), wire.clone(), HashSet::new()));
        assert_eq!(tracker.history(&key, &wire), None);
    }

    #[test]
    fn input_history_keeps_the_full_inline_command_without_marking_a_cleared_field() {
        let mut tracker = DeliveryTracker::default();
        let key = key("one");
        assert!(tracker.begin(key.clone(), "/compact".into(), HashSet::new()));
        tracker.mark_history(&key, "/compact".into());
        assert_eq!(tracker.history(&key, "/compact").as_deref(), Some("/compact"));
        assert_eq!(tracker.history(&key, "/co"), None);
        assert_eq!(tracker.typed(&key, "/compact"), None);
    }

    #[test]
    fn pending_send_blocks_a_second_post_across_selection_changes() {
        let mut tracker = DeliveryTracker::default();
        assert!(tracker.begin(key("one"), "first".into(), HashSet::new()));
        assert!(tracker.begin(key("other"), "other session".into(), HashSet::new()));
        assert!(!tracker.begin(key("one"), "duplicate".into(), HashSet::new()));
        assert!(tracker.pending(&key("one")));
        assert!(tracker.complete(&key("one"), "first", SendOutcome::Delivered));
        assert!(tracker.begin(key("one"), "second intentional send".into(), HashSet::new()));
    }

    #[test]
    fn recreated_session_does_not_inherit_pending_delivery() {
        let mut tracker = DeliveryTracker::default();
        assert!(tracker.begin(key("old-jsonl"), "first".into(), HashSet::new()));
        assert!(!tracker.pending(&key("new-jsonl")));
        assert!(tracker.begin(key("new-jsonl"), "new session".into(), HashSet::new()));
        assert!(tracker.complete(&key("old-jsonl"), "first", SendOutcome::Queued));
        assert!(tracker.pending(&key("new-jsonl")));
    }

    #[test]
    fn transcript_confirmed_before_http_result_clears_provisional_status() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::new());
        tracker.confirm_real(&key("one"), "new-id", None, "hello");
        assert!(tracker.complete(&key("one"), "hello", SendOutcome::Delivered));
        assert!(!tracker.pending(&key("one")));
        assert!(tracker.outcome(&key("one")).is_none());
    }

    #[test]
    fn ticket_delivery_history_confirms_only_messages_after_the_known_frontier() {
        let mut tracker=DeliveryTracker::default();
        tracker.begin(key("ticket"),"hello".into(),HashSet::from(["known-assistant".into()]));
        tracker.complete(&key("ticket"),"hello",SendOutcome::Delivered);
        let event=|id: &str,kind: &str,text: &str|ChatEvent {id:id.into(),kind:kind.into(),text:Some(text.into()),..Default::default()};
        tracker.confirm_history(&key("ticket"),&[event("older-user","user_msg","hello")]);
        assert!(tracker.outcome(&key("ticket")).is_some(),"uma página antiga não confirma o envio novo");
        tracker.confirm_history(&key("ticket"),&[event("older-user","user_msg","hello"),event("known-assistant","assistant_msg","anterior"),event("actual-user","user_msg","hello")]);
        assert!(tracker.outcome(&key("ticket")).is_none(),"a mensagem nova já está no histórico");
    }

    #[test]
    fn side_question_needs_no_transcript_echo() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "/btw o que mudou?".into(), HashSet::new());
        tracker.mark_typed(&key("one"), "/btw o que mudou?".into());
        assert!(tracker.outgoing(&key("one")).is_empty(), "sem bolha apagada esperando a conversa");
        assert!(tracker.complete(&key("one"), "/btw o que mudou?", SendOutcome::Delivered));
        assert!(tracker.outcome(&key("one")).is_none(), "sem \"aguardando confirmação\" preso");
        // Falha continua à vista.
        tracker.begin(key("one"), "/btw".into(), HashSet::new());
        tracker.complete(&key("one"), "/btw", SendOutcome::Rejected("x".into()));
        assert!(tracker.outcome(&key("one")).is_some());
    }

    fn now() -> f64 { now_secs() }

    #[test]
    fn uncertain_send_clears_when_the_conversation_shows_it() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::from(["old-id".into()]));
        tracker.mark_typed(&key("one"), "hello".into());
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        let arrived = tracker.confirm_real(&key("one"), "new-id", None, "hello");
        assert_eq!(arrived, Some(Arrived { field: "hello".into(), text: "hello".into(), history: Some("hello".into()) }),
            "o texto que voltou ao campo sai dele");
        assert_eq!(tracker.outcome(&key("one")), None);
    }

    #[test]
    fn untimed_proof_of_an_uncertain_send_leaves_a_trace() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert!(tracker.confirm_real(&key("one"), "new-id", None, "ok").is_some());
        assert_eq!(tracker.take_untimed_proofs(), 1, "outro aparelho também prova sem horário: fica o rastro");
        assert_eq!(tracker.take_untimed_proofs(), 0, "lido uma vez");

        tracker.begin(key("two"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.complete(&key("two"), "ok", SendOutcome::Uncertain);
        assert!(tracker.confirm_real(&key("two"), "new-id", Some(now()), "ok").is_some());
        assert_eq!(tracker.take_untimed_proofs(), 0, "com horário a prova é deste envio");
    }

    #[test]
    fn untimed_echo_during_the_flight_leaves_a_trace_when_it_settles_the_uncertain_answer() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "new-id", None, "ok");
        assert_eq!(tracker.take_untimed_proofs(), 0, "ainda em voo, nada foi dado por chegado");
        assert!(tracker.complete(&key("one"), "ok", SendOutcome::Uncertain));
        assert_eq!(tracker.outcome(&key("one")), None);
        assert_eq!(tracker.take_untimed_proofs(), 1);

        tracker.begin(key("two"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("two"), "untimed", None, "ok");
        tracker.confirm_real(&key("two"), "timed", Some(now()), "ok");
        assert!(tracker.complete(&key("two"), "ok", SendOutcome::Uncertain));
        assert_eq!(tracker.take_untimed_proofs(), 0, "o eco com horário também chegou: a prova é dele");
    }

    #[test]
    fn uncertain_arrival_keeps_the_chosen_command_for_the_field_history() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "/compact".into(), HashSet::from(["old-id".into()]));
        tracker.mark_typed(&key("one"), "/co".into());
        tracker.mark_history(&key("one"), "/compact".into());
        tracker.complete(&key("one"), "/compact", SendOutcome::Uncertain);
        let arrived = tracker.confirm_real(&key("one"), "new-id", None, "/compact");
        assert_eq!(arrived, Some(Arrived { field: "/co".into(), text: "/compact".into(), history: Some("/compact".into()) }),
            "o histórico guarda o comando escolhido, não o rascunho");
    }

    #[test]
    fn old_echo_outside_the_window_keeps_the_warning() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::from(["old-id".into()]));
        // Outra aba mandou o mesmo texto 2 min antes do Enter.
        tracker.confirm_real(&key("one"), "other-tab", Some(now() - 120.), "hello");
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        assert_eq!(tracker.confirm_real(&key("one"), "other-tab-2", Some(now() - 120.), "hello"), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        assert!(tracker.confirm_real(&key("one"), "new-id", Some(now() - 10.), "hello").is_some(), "dentro da folga vale");
    }

    #[test]
    fn untimed_echo_without_loaded_history_keeps_the_warning() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::new());
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        assert_eq!(tracker.confirm_real(&key("one"), "new-id", None, "hello"), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        assert!(tracker.confirm_real(&key("one"), "new-id", Some(now()), "hello").is_some(), "com horário recente vale");
    }

    #[test]
    fn untimed_echo_of_an_equal_message_still_on_its_way_keeps_the_warning() {
        let mut tracker = DeliveryTracker::default();
        // O primeiro "ok" foi para a fila e ainda não apareceu na conversa.
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.complete(&key("one"), "ok", SendOutcome::Queued);
        tracker.begin_beside(key("one"), "ok".into(), HashSet::from(["old-id".into()]), &["ok".into()]);
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        // O turno acaba e o primeiro "ok" entra sem horário: pode ser só ele.
        assert_eq!(tracker.confirm_real(&key("one"), "first-ok", None, "ok"), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        assert!(tracker.confirm_real(&key("one"), "second-ok", Some(now()), "ok").is_some(), "com horário recente vale");
    }

    #[test]
    fn untimed_echo_with_the_same_text_in_the_queue_panel_keeps_the_warning() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin_beside(key("one"), "ok".into(), HashSet::from(["old-id".into()]), &["outra".into(), "ok".into()]);
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert_eq!(tracker.confirm_real(&key("one"), "new-id", None, "ok"), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn untimed_echo_proves_the_resend_of_an_uncertain_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        tracker.begin_beside(key("one"), "ok".into(), HashSet::from(["old-id".into()]), &["ok".into()]);
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert!(tracker.confirm_real(&key("one"), "new-id", None, "ok").is_some(), "o igual a caminho é a mesma mensagem");
    }

    #[test]
    fn queued_row_does_not_settle_an_uncertain_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "queued-a", Some(now()), "hello");
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain), "a fila durante o voo não prova");
        assert_eq!(tracker.confirm_real(&key("one"), "queued-b", Some(now()), "hello"), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn side_question_uncertain_is_not_proved_by_begin() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "/btw o que mudou?".into(), HashSet::new());
        tracker.complete(&key("one"), "/btw o que mudou?", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn history_page_settles_an_uncertain_send_only_with_a_recent_time() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::new());
        tracker.mark_typed(&key("one"), "hello".into());
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        let event = |id: &str, ts: Option<f64>| ChatEvent { id: id.into(), kind: "user_msg".into(), text: Some("hello".into()), ts, ..Default::default() };
        assert_eq!(tracker.confirm_history(&key("one"), &[event("old", Some(now() - 600.)), event("untimed", None)]), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        assert_eq!(tracker.confirm_history(&key("one"), &[event("old", Some(now() - 600.)), event("new", Some(now()))]).map(|a| a.field),
            Some("hello".into()));
        assert_eq!(tracker.outcome(&key("one")), None);
    }

    #[test]
    fn known_replayed_id_cannot_hide_an_uncertain_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "old-id", None, "ok");
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn new_id_with_same_text_can_confirm_the_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "new-id", None, "ok");
        tracker.complete(&key("one"), "ok", SendOutcome::Delivered);
        assert!(tracker.outcome(&key("one")).is_none());
    }

    #[test]
    fn echo_during_the_flight_settles_an_uncertain_answer() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::new());
        tracker.confirm_real(&key("one"), "new-id", Some(now()), "ok");
        assert!(tracker.complete(&key("one"), "ok", SendOutcome::Uncertain));
        assert!(!tracker.pending(&key("one")));
        assert_eq!(tracker.outcome(&key("one")), None);
    }

    #[test]
    fn attachment_message_confirms_by_caption_and_interrupt_returns_only_accepted_unseen_text() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "olha — 📎 imagem: /u/1.png".into(), HashSet::new());
        assert_eq!(tracker.take_unconfirmed(&key("one")), None);
        tracker.complete(&key("one"), "olha — 📎 imagem: /u/1.png", SendOutcome::Queued);
        tracker.confirm_real(&key("one"), "real", None, "olha");
        assert!(tracker.outcome(&key("one")).is_none());
        tracker.begin(key("one"), "fila".into(), HashSet::new());
        tracker.complete(&key("one"), "fila", SendOutcome::Delivered);
        assert_eq!(tracker.take_unconfirmed(&key("one")).as_deref(), Some("fila"));
        assert!(tracker.outcome(&key("one")).is_none());
        tracker.begin(key("one"), "q".into(), HashSet::new());
        tracker.complete(&key("one"), "q", SendOutcome::Queued);
        assert_eq!(tracker.take_unconfirmed(&key("one")), None);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Queued));
    }

    #[test]
    fn multiline_text_merged_with_another_in_the_same_turn_confirms() {
        let sent = "T14 linhas\n001 a\n002 b";
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), sent.into(), HashSet::new());
        tracker.complete(&key("one"), sent, SendOutcome::Queued);
        tracker.confirm_real(&key("one"), "real", None, &format!("{sent}\n{sent}"));
        assert!(tracker.outcome(&key("one")).is_none());
        assert!(!matches_real("T14 linhas\n001 a", "T14 linhas\n001"));
    }

    #[test]
    fn text_sent_during_a_pending_send_waits_its_turn_in_order() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "first".into(), HashSet::new());
        tracker.mark_typed(&key("one"), "first".into());
        tracker.hold(key("one"), "second".into(), "second".into(), false, None);
        tracker.hold(key("one"), "third".into(), "third".into(), true, None);
        assert_eq!(tracker.outgoing(&key("one")), vec!["first", "second", "third"]);
        assert!(tracker.outgoing(&key("other")).is_empty());
        assert_eq!(tracker.typed(&key("one"), "first").as_deref(), Some("first"));
        tracker.complete(&key("one"), "first", SendOutcome::Queued);
        assert_eq!(tracker.typed(&key("one"), "first"), None);
        assert_eq!(tracker.next_held(&key("one")), Some(("second".into(), "second".into(), SendTiming::Queued, None)));
        assert_eq!(tracker.outgoing(&key("one")), vec!["third"]);
        assert_eq!(tracker.next_held(&key("one")), Some(("third".into(), "third".into(), SendTiming::Direct, None)));
        assert_eq!(tracker.next_held(&key("one")), None);
    }

    #[test]
    fn a_held_jev_send_keeps_the_task_and_message_from_enter() {
        let mut tracker = DeliveryTracker::default();
        let timing = SendTiming::Jev { task: "Criar a tela".into(), message: "Use azul".into() };
        tracker.hold(key("one"), "Use azul".into(), "Use azul".into(), timing.clone(), None);
        assert_eq!(tracker.next_held(&key("one")), Some(("Use azul".into(), "Use azul".into(), timing, None)));
    }

    #[test]
    fn waiting_for_another_send_or_upload_cannot_steer_a_later_turn() {
        let timing = SendTiming::Jev { task: "Criar a tela".into(), message: "Use azul".into() };
        assert_eq!(timing.after_wait(), SendTiming::Jev { task: String::new(), message: "Use azul".into() });
        assert_eq!(SendTiming::Direct.after_wait(), SendTiming::Direct);
        assert_eq!(SendTiming::Queued.after_wait(), SendTiming::Queued);
    }

    #[test]
    fn failed_send_gives_back_everything_that_waited_behind_it() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "first".into(), HashSet::new());
        tracker.hold(key("one"), "second".into(), "second".into(), false, None);
        tracker.hold(key("one"), "third".into(), "third".into(), false, None);
        // Envio que não saiu do campo (anexo, comando do painel) não aparece como bolha de espera.
        assert_eq!(tracker.outgoing(&key("one")), vec!["second", "third"]);
        tracker.complete(&key("one"), "first", SendOutcome::Rejected("HTTP 400".into()));
        assert_eq!(tracker.take_held(&key("one")), vec!["second".to_owned(), "third".to_owned()]);
        assert!(tracker.take_held(&key("one")).is_empty());
        assert!(tracker.outgoing(&key("one")).is_empty());
    }

    #[test]
    fn cleared_transcript_keeps_the_pending_send_and_its_queue() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("before-clear"), "first".into(), HashSet::new());
        tracker.mark_typed(&key("before-clear"), "first".into());
        tracker.hold(key("before-clear"), "second".into(), "second".into(), false, None);
        tracker.rekey(&key("before-clear"), &key("after-clear"));
        // Ainda "Enviando…" na chave nova: outro Enter espera em vez de sair em paralelo.
        assert_eq!(tracker.outgoing(&key("after-clear")), vec!["first", "second"]);
        assert!(!tracker.begin(key("after-clear"), "third".into(), HashSet::new()));
        tracker.rekey(&key("after-clear"), &key("second-clear"));
        // A resposta do POST chega com a chave de quando saiu.
        let answered = tracker.current(key("before-clear"));
        assert_eq!(answered, key("second-clear"));
        assert!(tracker.complete(&answered, "first", SendOutcome::Rejected("HTTP 500".into())));
        assert_eq!(tracker.take_held(&answered), vec!["second".to_owned()]);
    }

    #[test]
    fn held_text_keeps_the_group_chosen_on_enter() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "first".into(), HashSet::new());
        tracker.hold(key("one"), "to all".into(), "to all".into(), false, Some(vec!["same-name".into(), "peer".into()]));
        tracker.complete(&key("one"), "first", SendOutcome::Delivered);
        assert_eq!(tracker.next_held(&key("one")), Some(("to all".into(), "to all".into(), SendTiming::Queued, Some(vec!["same-name".into(), "peer".into()]))));
    }

    #[test]
    fn send_confirmed_by_the_conversation_before_the_post_answers_is_not_shown_twice() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::new());
        tracker.mark_typed(&key("one"), "hello".into());
        tracker.confirm_real(&key("one"), "new-id", None, "hello");
        assert!(tracker.pending(&key("one")));
        assert!(tracker.outgoing(&key("one")).is_empty());
    }

    #[test]
    fn attached_send_leaves_the_field_with_its_caption_and_gives_it_back_on_failure() {
        let mut tracker = DeliveryTracker::default();
        let prompt = "olha isso — 📎imagem: /tmp/up/colado-1.png";
        tracker.begin(key("one"), prompt.into(), HashSet::new());
        tracker.mark_typed(&key("one"), "olha isso".into());
        // A bolha mostra o que a pessoa escreveu, não o prompt com os caminhos dos anexos.
        assert_eq!(tracker.outgoing(&key("one")), vec!["olha isso"]);
        // Falhou: volta ao campo a legenda, que é o que saiu dele no Enter.
        assert_eq!(tracker.typed(&key("one"), prompt), Some("olha isso".to_owned()));
        assert!(tracker.complete(&key("one"), prompt, SendOutcome::Rejected("HTTP 500".into())));
        assert_eq!(tracker.typed(&key("one"), prompt), None);
    }

    #[test]
    fn text_match_after_rejected_http_result_keeps_reason() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::new());
        tracker.complete(&key("one"), "ok", SendOutcome::Rejected("HTTP 409".into()));
        tracker.confirm_real(&key("one"), "other-client-id", None, "ok");
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Rejected("HTTP 409".into())));
    }
}
