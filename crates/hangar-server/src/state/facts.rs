//! Fatos do estado: a leitura do plugin no `plugin::Store` do Rust (`plugin_reading`) e a memória dos
//! fatos que a ponte da lista ainda aceita por empurrão (`state.facts`). Nenhuma chamada ao Python:
//! o retrato e os serviços que o `Monitor` pedia a ele são do Rust.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

/// `vivo` no Python: long-poll aberto ou batida há menos disto (`ESPERA_S + 10`).
pub const ALIVE: Duration = Duration::from_secs(35);
/// Estado anunciado pelo plugin sem long-poll aberto (`VALIDADE_ESTADO_S`).
pub const PLUGIN_STATE: Duration = Duration::from_secs(90);
/// Pergunta segurada depois do último `visto` (`pergunta_pendente`).
pub const QUESTION: Duration = Duration::from_secs(35);
/// Releitura do retrato: abaixo dos 30 s em que o interesse vence no Python.
pub const SNAPSHOT_REFRESH: Duration = Duration::from_secs(25);
/// `problema` do estado quando o retrato não veio: fica o último valor, nunca estado inventado.
pub const UNAVAILABLE: &str = "state_facts_unavailable";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginState { pub state: String, pub reason: Option<String>, pub age_ms: u64 }

/// Leitura do Store compartilhado; o vínculo vem do dono, nunca do pedido ou do poll.
#[derive(Clone, Debug, Deserialize)]
pub struct PluginFacts {
    pub plugin_state: Option<PluginState>,
    pub waiter_open: bool,
    pub heartbeat_age_ms: Option<u64>,
    pub question: Option<Value>,
    pub questions: Vec<Value>,
    pub suggestion: String,
}

impl PluginFacts {
    pub fn alive(&self) -> bool {
        self.waiter_open || self.heartbeat_age_ms.is_some_and(|age| Duration::from_millis(age) < ALIVE)
    }

    /// Plugin vivo segura o último estado sem prazo: entre um `/pull` e o seguinte o long-poll fica fechado por
    /// um instante, e um `idle` antigo sumir ali deixa a tela valer.
    pub fn state(&self) -> Option<String> {
        self.plugin_state.as_ref().filter(|state| self.alive() || Duration::from_millis(state.age_ms) <= PLUGIN_STATE)
            .map(|state| state.state.clone())
    }

    /// Há quanto tempo o plugin mandou o estado que `state` devolve.
    pub fn state_age(&self) -> Option<Duration> {
        self.plugin_state.as_ref().filter(|state| self.alive() || Duration::from_millis(state.age_ms) <= PLUGIN_STATE)
            .map(|state| Duration::from_millis(state.age_ms))
    }
}

#[derive(Clone)]
pub struct PluginReading {
    pub binding: crate::interactions::InteractionBinding,
    pub facts: Result<PluginFacts, String>,
}

pub async fn plugin_reading(registry: &crate::runtime::gateway::RuntimeRegistry, name: &str) -> Result<PluginReading, String> {
    let binding = registry.interaction_binding_readonly(name).await.map_err(|error| error.code)?;
    let facts = registry.plugin().map_err(|error| error.code.clone()).and_then(|store| {
        store.facts(&binding, Instant::now()).map_err(|error| error.0.to_owned())
    }).and_then(|facts| serde_json::from_value(facts).map_err(|_| "plugin_facts_invalid".into()));
    let current = registry.interaction_binding_readonly(name).await.map_err(|error| error.code)?;
    if current != binding { return Err("erro_interacao_desatualizada".into()); }
    Ok(PluginReading { binding, facts })
}

pub fn plugin_surfaces(questions: &[Value]) -> Result<(Option<&Value>, Option<&Value>), String> {
    let (mut selector, mut form) = (None, None);
    for question in questions {
        let id = question["id"].as_str().filter(|id| !id.is_empty()).ok_or("plugin_question_invalid")?;
        question["interaction_id"].as_str().filter(|id| !id.is_empty()).ok_or("plugin_question_invalid")?;
        // Permissões seguradas juntas saem uma de cada vez, na ordem estável do Store: respondida a primeira,
        // a seguinte aparece (o app faz tudo, nada fica para o terminal).
        if id.starts_with("perm:") {
            selector.get_or_insert(question);
            continue;
        }
        if question["questions"].as_array().is_none_or(Vec::is_empty) { return Err("plugin_question_invalid".into()); }
        if form.replace(question).is_some() { return Err("erro_interacao_indisponivel".into()); }
    }
    Ok((selector, form))
}

/// As superfícies são independentes: erro no seletor não retira o formulário confirmado.
pub fn project_plugin_state(state: &mut hangar_api::state::StateEvent, questions: &[Value]) -> Result<(), String> {
    let (selectors, forms): (Vec<_>, Vec<_>) = questions.iter().cloned()
        .partition(|question| question["id"].as_str().is_some_and(|id| id.starts_with("perm:")));
    let mut error = None;
    match plugin_surfaces(&forms) {
        Ok((_, Some(question))) => {
            if state.codex_question.as_ref().is_some_and(|current| current.get("interaction_id") != question.get("interaction_id")) {
                error = Some("erro_interacao_indisponivel".to_owned());
            } else {
                let mut payload = question.as_object().cloned().ok_or("plugin_question_invalid")?;
                payload.insert("request_id".into(), question["id"].clone());
                payload.insert("provider".into(), json!("claude"));
                state.codex_question = Some(payload);
                state.state = "awaiting_input".into();
            }
        }
        Err(code) => error = Some(code),
        _ => {},
    }
    match plugin_surfaces(&selectors) {
        Ok((Some(question), _)) => {
            let shown = crate::terminal_state::held_question(question).ok_or("plugin_question_invalid")?;
            if state.interaction_id.as_deref().is_some_and(|id| Some(id) != question["interaction_id"].as_str())
                || state.question.is_some() && (state.question != shown.question || state.options.as_ref() != Some(&shown.options)) {
                error = Some("erro_interacao_indisponivel".into());
            } else {
                state.state = "awaiting_input".into();
                state.label = None;
                state.question = shown.question;
                state.options = Some(shown.options);
                state.interaction_id = question["interaction_id"].as_str().map(str::to_owned);
            }
        }
        Err(code) => error = Some(code),
        _ => {},
    }
    error.map_or(Ok(()), Err)
}

pub fn preserve_cards(state: &mut hangar_api::state::StateEvent, previous: &hangar_api::state::StateEvent) {
    if state.codex_question.is_none() { state.codex_question = previous.codex_question.clone(); }
    if state.interaction_id.is_none() && previous.interaction_id.is_some() {
        state.interaction_id = previous.interaction_id.clone();
        state.question = previous.question.clone();
        state.options = previous.options.clone();
        state.claude_plan_pending = previous.claude_plan_pending.clone();
    }
    if state.codex_question.is_some() || state.interaction_id.is_some() { state.state = "awaiting_input".into(); }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Question {
    pub id: String,
    pub questions: Value,
    pub tool: Option<String>,
    pub resumo: Option<String>,
    pub seen_age_ms: u64,
}

/// Todos os campos são obrigatórios: chave que falta é contrato quebrado, nunca vazio.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StateFacts {
    pub seq: u64,
    pub plugin_state: Option<PluginState>,
    pub waiter_open: bool,
    pub heartbeat_age_ms: Option<u64>,
    pub question: Option<Question>,
    pub suggestion: String,
    /// Largura da conversa no pane quando há painel ancorado (corte da prévia em `+5`).
    pub body_columns: Option<u32>,
    pub band_anchor: Option<String>,
    /// Quanto faltava da janela de troca terminal ⇄ sem terminal quando o Python enviou.
    pub in_transfer_ms: u64,
    pub transfer_active: bool,
    pub permission_op: bool,
}

/// Fatos e o instante em que chegaram aqui: as idades contam a partir dele.
#[derive(Clone, Debug)]
pub struct Received { pub facts: Arc<StateFacts>, pub at: Instant }

impl Received {
    /// Idade agora de um valor que tinha `age_ms` quando chegou.
    fn age(&self, age_ms: u64, now: Instant) -> Duration {
        Duration::from_millis(age_ms).saturating_add(now.saturating_duration_since(self.at))
    }

    /// `plugin_bridge.vivo`.
    pub fn alive(&self, now: Instant) -> bool {
        self.facts.waiter_open || self.facts.heartbeat_age_ms.is_some_and(|a| self.age(a, now) < ALIVE)
    }

    /// `plugin_bridge.estado_recente`: sem prazo com o long-poll aberto.
    pub fn plugin_state(&self, now: Instant) -> Option<&PluginState> {
        self.facts.plugin_state.as_ref().filter(|p| self.alive(now) || self.age(p.age_ms, now) <= PLUGIN_STATE)
    }

    /// Há quanto tempo o plugin mandou o estado que `plugin_state` devolve.
    pub fn plugin_state_age(&self, now: Instant) -> Option<Duration> {
        self.plugin_state(now).map(|p| self.age(p.age_ms, now))
    }

    /// `plugin_bridge.pergunta_pendente`.
    pub fn question(&self, now: Instant) -> Option<&Question> {
        self.facts.question.as_ref().filter(|q| self.age(q.seen_age_ms, now) <= QUESTION)
    }

    /// `sessions.em_troca`.
    pub fn in_transfer(&self, now: Instant) -> bool {
        self.facts.transfer_active || Duration::from_millis(self.facts.in_transfer_ms) > now.saturating_duration_since(self.at)
    }
}

/// Resultado de um empurrão.
#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    /// `gap`: faltou sequência no meio (envio que falhou no Python): o retrato tem que ser relido.
    Accepted { gap: bool },
    /// Sequência já vista: chegou fora de ordem ou depois de um retrato mais novo.
    Dropped,
    /// Ninguém observa a sessão aqui (empurrão atrasado depois do `forget`): não cria entrada.
    Unwatched,
}

#[derive(Default)]
struct Entry {
    last: Option<Received>,
    wake: Arc<tokio::sync::Notify>,
    /// Faltou sequência desde o último retrato: quem observa relê o retrato.
    gap: bool,
}

/// Último fato de cada sessão observada. Entrada só nasce por `watch` (o `Monitor` que nasce) e sai
/// por `forget`: o mapa tem o tamanho dos `Monitor`s vivos.
#[derive(Default)]
pub struct FactsStore { entries: Mutex<HashMap<String, Entry>> }

impl FactsStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Passa a observar a sessão e devolve o aviso de empurrão aceito. Quem espera arma o
    /// `notified()` antes de ler `get`: `notify_waiters` não guarda o aviso para depois.
    pub fn watch(&self, name: &str) -> Arc<tokio::sync::Notify> {
        self.lock().entry(name.to_owned()).or_default().wake.clone()
    }

    /// Empurrão do Python. Aceito acorda quem observa a sessão (idade 0, como `esperar_evento`).
    pub fn push(&self, name: &str, facts: StateFacts, at: Instant) -> Push {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(name) else { return Push::Unwatched };
        let last = entry.last.as_ref().map(|r| r.facts.seq);
        if last.is_some_and(|l| facts.seq <= l) {
            return Push::Dropped;
        }
        let gap = last.is_some_and(|l| facts.seq > l.saturating_add(1));
        entry.gap |= gap;
        entry.last = Some(Received { facts: Arc::new(facts), at });
        entry.wake.notify_waiters();
        Push::Accepted { gap }
    }

    /// Retrato lido agora. Empurrão mais novo que chegou durante a leitura vence; com sequência
    /// igual ou maior, o retrato vale por inteiro e fecha o pulo. Não acorda: quem o leu é quem
    /// observa.
    pub fn snapshot(&self, name: &str, facts: StateFacts, at: Instant) {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(name) else { return };
        if entry.last.as_ref().is_some_and(|r| r.facts.seq > facts.seq) {
            return;
        }
        entry.gap = false;
        entry.last = Some(Received { facts: Arc::new(facts), at });
    }

    pub fn get(&self, name: &str) -> Option<Received> { self.lock().get(name).and_then(|e| e.last.clone()) }

    /// Faltou sequência desde o último retrato.
    pub fn needs_snapshot(&self, name: &str) -> bool { self.lock().get(name).is_some_and(|e| e.gap) }

    pub fn forget(&self, name: &str) { self.lock().remove(name); }

    /// `forget` de quem observava com `wake`, só se ninguém mais observa: o `Monitor` que acaba
    /// depois de outro da mesma sessão nascer não apaga a entrada do novo.
    pub fn forget_watcher(&self, name: &str, wake: &Arc<tokio::sync::Notify>) {
        let mut entries = self.lock();
        // Uma referência no mapa e a de quem chama.
        if entries.get(name).is_some_and(|e| Arc::ptr_eq(&e.wake, wake) && Arc::strong_count(wake) == 2) {
            entries.remove(name);
        }
    }
}

/// Resposta de `session.dead`.
#[derive(Debug, PartialEq, Eq)]
pub enum Dead { Ok, InTransfer }

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(seq: u64) -> StateFacts {
        StateFacts { seq, plugin_state: Some(PluginState { state: "working".into(), reason: None, age_ms: 80_000 }),
            waiter_open: false, heartbeat_age_ms: Some(30_000),
            question: Some(Question { id: "q".into(), questions: Value::Null, tool: None, resumo: None, seen_age_ms: 30_000 }),
            suggestion: String::new(), body_columns: None, band_anchor: None, in_transfer_ms: 4_000,
            transfer_active: false, permission_op: false }
    }

    #[test]
    fn validity_with_own_clock() {
        let t0 = Instant::now();
        let r = Received { facts: Arc::new(facts(1)), at: t0 };
        assert!(r.alive(t0) && r.plugin_state(t0).is_some() && r.question(t0).is_some() && r.in_transfer(t0));
        let later = t0 + Duration::from_secs(6);
        assert!(!r.alive(later), "batida de 30 s + 6 s passa dos 35 s");
        assert!(r.question(later).is_none());
        assert!(!r.in_transfer(later));
        assert!(r.plugin_state(later).is_some(), "80 s + 6 s ainda dentro dos 90 s");
        assert!(r.plugin_state(t0 + Duration::from_secs(11)).is_none());
        let open = Received { facts: Arc::new(StateFacts { waiter_open: true, ..facts(1) }), at: t0 };
        let hour = t0 + Duration::from_secs(3600);
        assert!(open.alive(hour) && open.plugin_state(hour).is_some(), "long-poll aberto não tem prazo");
    }

    #[test]
    fn a_live_plugin_keeps_its_old_state_between_two_polls() {
        // Sessão parada há 10 min: o long-poll fecha por um instante entre um `/pull` e o seguinte.
        let state = Some(PluginState { state: "idle".into(), reason: None, age_ms: 600_000 });
        let t0 = Instant::now();
        let between = Received { facts: Arc::new(StateFacts { plugin_state: state.clone(), heartbeat_age_ms: Some(2_000), ..facts(1) }), at: t0 };
        assert_eq!(between.plugin_state(t0).map(|p| p.state.as_str()), Some("idle"));
        let local = PluginFacts { plugin_state: state.clone(), waiter_open: false, heartbeat_age_ms: Some(2_000), question: None, questions: vec![], suggestion: String::new() };
        assert_eq!(local.state().as_deref(), Some("idle"));
        // Plugin que sumiu (sem batida recente): o estado velho deixa de valer.
        let gone = PluginFacts { heartbeat_age_ms: Some(60_000), ..local };
        assert_eq!(gone.state(), None);
    }

    #[test]
    fn older_sequence_dropped() {
        let store = FactsStore::default();
        let now = Instant::now();
        assert_eq!(store.push("s", facts(1), now), Push::Unwatched);
        store.watch("s");
        assert_eq!(store.push("s", facts(2), now), Push::Accepted { gap: false });
        assert_eq!(store.push("s", facts(1), now), Push::Dropped);
        assert_eq!(store.push("s", facts(2), now), Push::Dropped, "sequência já vista");
        assert_eq!(store.push("s", facts(4), now), Push::Accepted { gap: true });
        assert_eq!(store.get("s").unwrap().facts.seq, 4);
        assert!(store.needs_snapshot("s"));
        store.snapshot("s", facts(3), now);
        assert_eq!(store.get("s").unwrap().facts.seq, 4, "retrato mais velho que o empurrão não vence");
        store.snapshot("s", facts(4), now);
        assert!(!store.needs_snapshot("s"));
        assert_eq!(store.push("s", facts(5), now), Push::Accepted { gap: false });
    }

    #[test]
    fn selector_conflict_does_not_discard_an_independent_form() {
        let mut state = hangar_api::state::StateEvent { state: "awaiting_input".into(),
            question: Some("Menu do terminal".into()), ..Default::default() };
        let questions = vec![json!({"id":"perm:a","interaction_id":"selector-a","questions":[],"tool":"Bash","resumo":"cmd"}),
            json!({"id":"ask:b","interaction_id":"form-b","questions":[{"question":"Pasta?","options":[]}]})];
        assert!(project_plugin_state(&mut state, &questions).is_err());
        assert_eq!(state.codex_question.as_ref().unwrap()["interaction_id"], "form-b");
        assert!(state.interaction_id.is_none());
    }

    #[test]
    fn same_surface_ambiguity_is_not_an_empty_question() {
        let forms = vec![json!({"id":"ask:a","interaction_id":"a","questions":[{}]}), json!({"id":"ask:b","interaction_id":"b","questions":[{}]})];
        assert_eq!(plugin_surfaces(&forms).unwrap_err(), "erro_interacao_indisponivel");
    }

    /// Decisão do usuário (P21): duas permissões seguradas saem uma de cada vez; respondida a primeira,
    /// a segunda aparece. Nunca o problema no lugar do cartão.
    #[test]
    fn held_permissions_come_one_at_a_time() {
        let both = vec![json!({"id":"perm:a","interaction_id":"a"}), json!({"id":"perm:b","interaction_id":"b"})];
        assert_eq!(plugin_surfaces(&both).unwrap().0.unwrap()["interaction_id"], "a");
        assert_eq!(plugin_surfaces(&both[1..]).unwrap().0.unwrap()["interaction_id"], "b");
    }

    #[test]
    fn late_forget_keeps_the_new_watcher() {
        let store = FactsStore::default();
        let old = store.watch("s");
        let new = store.watch("s");
        store.forget_watcher("s", &old);
        assert_eq!(store.push("s", facts(1), Instant::now()), Push::Accepted { gap: false }, "o novo ainda observa");
        drop(new);
        store.forget_watcher("s", &old);
        assert_eq!(store.push("s", facts(2), Instant::now()), Push::Unwatched);
    }

    #[tokio::test]
    async fn push_wakes_the_observer() {
        let store = Arc::new(FactsStore::default());
        let wake = store.watch("s");
        let waiting = tokio::spawn(async move { wake.notified().await });
        tokio::task::yield_now().await;
        store.push("s", facts(1), Instant::now());
        tokio::time::timeout(Duration::from_secs(2), waiting).await.expect("não acordou").unwrap();
    }
}
