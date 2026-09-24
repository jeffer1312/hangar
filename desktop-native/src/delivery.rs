use std::collections::{HashMap, HashSet};
use crate::api::dto::SessionInfo;

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
}

#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    Delivered,
    Queued,
    Uncertain,
    Rejected(String),
}

#[derive(Clone, Debug)]
struct SendRecord {
    text: String,
    known: HashSet<String>,
    pending: bool,
    confirmed: bool,
    outcome: Option<SendOutcome>,
}

#[derive(Default)]
pub struct DeliveryTracker {
    records: HashMap<SessionKey, SendRecord>,
}

impl DeliveryTracker {
    pub fn begin(&mut self, key: SessionKey, text: String, known: HashSet<String>) -> bool {
        if self.pending(&key) { return false; }
        self.records.insert(key, SendRecord { text, known, pending: true, confirmed: false, outcome: None });
        true
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
        if record.confirmed && matches!(&outcome, SendOutcome::Delivered | SendOutcome::Queued) { self.records.remove(key); }
        else { record.outcome = Some(outcome); }
        true
    }

    pub fn confirm_real(&mut self, key: &SessionKey, id: &str, real_text: &str) {
        let Some(record) = self.records.get_mut(key) else { return; };
        if record.known.contains(id) { return; }
        if !matches_real(real_text, &record.text) { return; }
        if record.pending { record.confirmed = true; }
        else if matches!(record.outcome.as_ref(), Some(SendOutcome::Delivered | SendOutcome::Queued)) { self.records.remove(key); }
    }


    /// Texto digitado agora no terminal que a conversa ainda não mostrou: é o que a interrupção devolve.
    /// O que foi para a fila durável (`Queued`) não está no campo do terminal e segue entregue por ela.
    pub fn take_unconfirmed(&mut self, key: &SessionKey) -> Option<String> {
        let record = self.records.get(key)?;
        if record.pending || !matches!(record.outcome, Some(SendOutcome::Delivered)) { return None; }
        self.records.remove(key).map(|record| record.text)
    }
}

fn matches_real(real: &str, sent: &str) -> bool {
    let real = real.trim();
    let sent = sent.trim();
    if sent.is_empty() { return false; }
    // Mensagem com anexo: o transcript pode gravar só a legenda.
    let legend = crate::composer::caption(sent);
    real == sent || real.lines().any(|line| line.trim() == sent)
        || sent.chars().count() >= 8 && real.lines().any(|line| line.trim().starts_with(sent))
        || legend != sent && !legend.is_empty() && crate::composer::caption(real) == legend
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(jsonl: &str) -> SessionKey {
        SessionKey { server: "http://fixture/".into(), name: "same-name".into(), jsonl: jsonl.into() }
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
        tracker.confirm_real(&key("one"), "new-id", "hello");
        assert!(tracker.complete(&key("one"), "hello", SendOutcome::Delivered));
        assert!(!tracker.pending(&key("one")));
        assert!(tracker.outcome(&key("one")).is_none());
    }

    #[test]
    fn uncertainty_remains_visible_until_user_decides() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "hello".into(), HashSet::new());
        tracker.complete(&key("one"), "hello", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
        tracker.confirm_real(&key("one"), "new-id", "hello");
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn known_replayed_id_cannot_hide_an_uncertain_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "old-id", "ok");
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn new_id_with_same_text_can_confirm_the_send() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::from(["old-id".into()]));
        tracker.confirm_real(&key("one"), "new-id", "ok");
        tracker.complete(&key("one"), "ok", SendOutcome::Delivered);
        assert!(tracker.outcome(&key("one")).is_none());
    }

    #[test]
    fn new_text_match_before_uncertain_http_result_keeps_warning() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::new());
        tracker.confirm_real(&key("one"), "other-client-id", "ok");
        tracker.complete(&key("one"), "ok", SendOutcome::Uncertain);
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Uncertain));
    }

    #[test]
    fn attachment_message_confirms_by_caption_and_interrupt_returns_only_accepted_unseen_text() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "olha — 📎 imagem: /u/1.png".into(), HashSet::new());
        assert_eq!(tracker.take_unconfirmed(&key("one")), None);
        tracker.complete(&key("one"), "olha — 📎 imagem: /u/1.png", SendOutcome::Queued);
        tracker.confirm_real(&key("one"), "real", "olha");
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
    fn text_match_after_rejected_http_result_keeps_reason() {
        let mut tracker = DeliveryTracker::default();
        tracker.begin(key("one"), "ok".into(), HashSet::new());
        tracker.complete(&key("one"), "ok", SendOutcome::Rejected("HTTP 409".into()));
        tracker.confirm_real(&key("one"), "other-client-id", "ok");
        assert_eq!(tracker.outcome(&key("one")), Some(&SendOutcome::Rejected("HTTP 409".into())));
    }
}
