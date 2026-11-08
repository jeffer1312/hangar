//! Projeções locais do estado da lista e diagnósticos compartilhados.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};

use hangar_api::session::SessionRow;
use serde::Deserialize;
use serde_json::{Value, json};

pub const UNAVAILABLE: &str = "list_facts_unavailable";
/// Provedores de terminal cujo ator publica os fatos da lista.
pub const OTHERS: [&str; 3] = ["pi", "omp", "kimi"];

/// Estado publicado pelo ator da sessão.
#[derive(Clone, Debug, Deserialize)]
pub struct RowState {
    pub state: String,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    /// O ID das `options`, como o Rust o publicou: a projeção só o transporta, e sem ele as opções
    /// não respondem nada.
    #[serde(default)]
    pub interaction_id: Option<String>,
    pub problema: Option<String>,
    pub status_line: Option<String>,
    #[serde(default)]
    pub pending_questions: u32,
    #[serde(default)]
    pub startup_steps: Vec<String>,
    pub last_activity: Option<f64>,
    #[serde(default)]
    pub limited: bool,
    pub limit_reset: Option<String>,
    #[serde(default)]
    pub stalled: bool,
    #[serde(default)]
    pub last_reply: Option<String>,
    #[serde(default)]
    pub last_reply_at: Option<f64>,
}

pub type States = Arc<HashMap<String, RowState>>;

/// Estado dos atores e fatos locais da mesma rodada.
#[derive(Debug)]
pub struct ListFacts {
    pub states: States,
    /// Linhas inteiras que a transferência substitui (pelo nome) ou cria.
    pub overrides: Vec<SessionRow>,
    /// Transferências em curso: nem classificadas nem decoradas.
    pub frozen: HashSet<String>,
    pub orq: Vec<SessionRow>,
    pub owners: HashMap<String, String>,
    /// Escondidas do dono (sessão de convidado que ele não vê).
    pub hidden: HashSet<String>,
    pub problems: BTreeMap<String, String>,
    pub stall_seconds: f64,
    /// JSON dos terminais de atalho, a última leitura boa; `None` antes da primeira.
    pub shortcuts: Option<String>,
    /// Transferências nunca lidas: a linha que só elas mostram é desconhecida, não ausente. Todas
    /// as linhas levam o problema, e o hub não serve a lista ao dono.
    pub unknown: bool,
}

impl Default for ListFacts {
    fn default() -> Self {
        Self { states: States::default(), overrides: Vec::new(), frozen: HashSet::new(), orq: Vec::new(),
            owners: HashMap::new(), hidden: HashSet::new(), problems: BTreeMap::new(),
            stall_seconds: 300.0, shortcuts: None, unknown: true }
    }
}

pub struct Fetched {
    pub states: States,
    pub ok: bool,
    pub fresh: bool,
}

/// Mantém o diagnóstico da lista; o estado vem dos atores em processo.
pub struct FactsClient { pub diag: crate::diag::DiagClient }

impl FactsClient {
    pub fn new(upstream: SocketAddr, secret: String) -> Self {
        Self { diag: crate::diag::DiagClient::new(upstream, secret) }
    }
}

/// Problema mais preciso que a linha já tenha fica.
pub fn mark_stale(rows: &mut [SessionRow], facts: &ListFacts, ok: bool) {
    for row in rows.iter_mut().filter(|r| facts.unknown || (!ok && OTHERS.contains(&r.provider.as_str()))) {
        row.problema.get_or_insert_with(|| UNAVAILABLE.into());
    }
}

/// Falha vista em código síncrono (arquivo lido dentro do `spawn_blocking`) a caminho do diário:
/// a produção esvazia e manda pelo `diag`. Uma por minuto por (sessão, código); fila com teto.
/// `fields`: o que diagnostica a falha (`Value::Null` sem nada); com ele a nota vai por `report_with`.
pub struct Note { pub event: &'static str, pub session: String, pub code: String, pub reason: &'static str, pub fields: Value }

#[cfg(not(test))]
const MAX_NOTES: usize = 64;
// Nos testes ninguém esvazia a fila.
#[cfg(test)]
const MAX_NOTES: usize = 100_000;
static NOTES: LazyLock<Mutex<Vec<Note>>> = LazyLock::new(Default::default);
/// Nos testes, tudo o que entrou na fila: a rodada da lista de outro teste a esvazia em paralelo.
#[cfg(test)]
static NOTED: LazyLock<Mutex<Vec<(String, String)>>> = LazyLock::new(Default::default);

pub fn note(event: &'static str, session: &str, code: String, reason: &'static str) { note_with(event, session, code, reason, Value::Null) }

pub fn note_with(event: &'static str, session: &str, code: String, reason: &'static str, fields: Value) {
    let mut q = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    // Cheia (ninguém produzindo para esvaziar): o aviso vai ao log, e o teto por minuto não é gasto.
    if q.len() >= MAX_NOTES {
        if crate::warn_limit::allow(None, "list_notes_full") {
            tracing::warn!(event, code, reason, "fila do diário da lista cheia; esta falha fica só no log");
        }
        return;
    }
    if crate::warn_limit::allow(Some(session), &format!("note:{event}:{code}")) {
        #[cfg(test)]
        NOTED.lock().unwrap_or_else(|e| e.into_inner()).push((session.to_owned(), code.clone()));
        q.push(Note { event, session: session.to_owned(), code, reason, fields });
    }
}

pub fn take_notes() -> Vec<Note> { std::mem::take(&mut *NOTES.lock().unwrap_or_else(|e| e.into_inner())) }

#[cfg(test)]
pub fn notes_for(session: &str) -> Vec<String> {
    NOTED.lock().unwrap().iter().filter(|(noted, _)| noted == session).map(|(_, code)| code.clone()).collect()
}

/// Fatos sobre as linhas descobertas. Devolve as que vão à classificação e à decoração e as postas
/// de lado (transferência em curso, orquestração), que saem como o Python as deu, no fim da lista.
pub fn apply(rows: Vec<SessionRow>, facts: &ListFacts, ok: bool) -> (Vec<SessionRow>, Vec<SessionRow>) {
    let mut overrides: HashMap<&str, &SessionRow> = facts.overrides.iter().map(|r| (r.name.as_str(), r)).collect();
    let mut work = Vec::with_capacity(rows.len());
    let mut aside = Vec::new();
    for row in rows {
        let external = overrides.remove(row.name.as_str());
        // Quem decide é a sessão descoberta: da Claude, vida e pergunta são do Rust e só a transferência
        // vem do Python; a de outro agente em troca (mesmo que o destino seja Claude) segue a linha dela.
        let claude = row.provider == "claude";
        let row = match external {
            Some(ext) if claude => {
                let mut row = row;
                row.transfer_id.clone_from(&ext.transfer_id);
                row.transfer_phase.clone_from(&ext.transfer_phase);
                if ext.problema.is_some() { row.problema.clone_from(&ext.problema); }
                row
            }
            Some(ext) => ext.clone(),
            None => row,
        };
        if (!claude || row.transfer_id.is_some()) && facts.frozen.contains(&row.name) { aside.push(row) } else { work.push(row) }
    }
    // Linha que a transferência cria (a sessão de origem já saiu da descoberta).
    for row in facts.overrides.iter().filter(|r| overrides.contains_key(r.name.as_str())) {
        if facts.frozen.contains(&row.name) { aside.push(row.clone()) } else { work.push(row.clone()) }
    }
    aside.extend(facts.orq.iter().cloned());
    for row in work.iter_mut().filter(|r| OTHERS.contains(&r.provider.as_str())) {
        if let Some(s) = facts.states.get(&row.name) {
            row.state.clone_from(&s.state);
            row.label.clone_from(&s.label);
            row.question.clone_from(&s.question);
            row.options.clone_from(&s.options);
            row.interaction_id.clone_from(&s.interaction_id);
            row.problema.clone_from(&s.problema);
            row.status_line.clone_from(&s.status_line);
            row.pending_questions = s.pending_questions;
            row.startup_steps.clone_from(&s.startup_steps);
            row.last_activity = s.last_activity;
            row.limited = s.limited;
            row.limit_reset.clone_from(&s.limit_reset);
            row.stalled = s.stalled;
            row.last_reply.clone_from(&s.last_reply);
            row.last_reply_at = s.last_reply_at;
        }
        if !ok { row.problema = Some(UNAVAILABLE.into()); }
    }
    if facts.unknown {
        for row in work.iter_mut() { row.problema = Some(UNAVAILABLE.into()); }
    }
    for row in work.iter_mut().chain(aside.iter_mut()) {
        row.owner = facts.owners.get(&row.name).cloned();
    }
    (work, aside)
}

/// Compartilhada = convite ativo na loja do Rust, a única dona de `shares.json`.
pub fn mark_shared(rows: &mut [SessionRow], active: &HashSet<String>) {
    for row in rows {
        row.shared = active.contains(&row.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};


    /// A rodada da lista esvazia a fila em paralelo aos testes: o que um teste anotou continua visível.
    #[test]
    fn a_note_stays_visible_after_the_round_takes_the_queue() {
        note("list_test", "notes-drained", "list_test_code".into(), "teste");
        drop(take_notes());
        assert_eq!(notes_for("notes-drained"), ["list_test_code"]);
    }

    #[test]
    fn external_overrides_and_frozen_flags_do_not_decide_claude_life_or_question() {
        let row = |provider: &str, id: &str| serde_json::from_value::<SessionRow>(json!({"name":"s","provider":provider,"interaction_id":id})).unwrap();
        let facts = ListFacts { overrides: vec![row("claude", "old-python")], frozen: HashSet::from(["s".into()]),
            unknown: false, ..Default::default() };
        let (work, aside) = apply(vec![row("claude", "rust")], &facts, true);
        assert!(aside.is_empty());
        assert_eq!(work[0].interaction_id.as_deref(), Some("rust"));
        let facts = ListFacts { overrides: vec![row("pi", "pi")], frozen: HashSet::from(["s".into()]), unknown: false, ..Default::default() };
        let (work, aside) = apply(vec![row("pi", "")], &facts, true);
        assert!(work.is_empty());
        assert_eq!(aside[0].interaction_id.as_deref(), Some("pi"));
    }

    /// Parity M13: a última resposta de Pi, omp e Kimi (histórico ainda do Python) chega à linha.
    #[test]
    fn other_agents_last_reply_comes_with_the_facts() {
        let row = serde_json::from_value::<SessionRow>(json!({"name":"p","provider":"pi"})).unwrap();
        let state: RowState = serde_json::from_value(json!({"state":"idle","label":null,"question":null,"options":null,"problema":null,
            "status_line":null,"last_activity":5.0,"limit_reset":null,"last_reply":"pronto","last_reply_at":4.0})).unwrap();
        let facts = ListFacts { states: Arc::new(HashMap::from([("p".into(), state)])), unknown: false, ..Default::default() };
        let (work, _) = apply(vec![row], &facts, true);
        assert_eq!((work[0].last_reply.as_deref(), work[0].last_reply_at), (Some("pronto"), Some(4.0)));
    }

    /// Parity A4 (`test_facts_transfer_overrides_by_name`): a troca Claude → Codex põe fase e falha na
    /// linha Claude, não a classifica enquanto corre, e a linha de recuperação aparece sem a origem.
    #[test]
    fn claude_transfer_keeps_phase_and_recovery_row() {
        let row = |v: Value| serde_json::from_value::<SessionRow>(v).unwrap();
        let ext = row(json!({"name":"s","provider":"claude","interaction_id":"old-python","transfer_id":"t1","transfer_phase":"prepared","problema":"restore_failed"}));
        let facts = ListFacts { overrides: vec![ext.clone()], frozen: HashSet::from(["s".into()]), unknown: false, ..Default::default() };
        let (work, aside) = apply(vec![row(json!({"name":"s","provider":"claude","interaction_id":"rust"}))], &facts, true);
        assert!(work.is_empty(), "transferência em curso não é classificada");
        assert_eq!((aside[0].transfer_id.as_deref(), aside[0].transfer_phase.as_deref(), aside[0].problema.as_deref()),
            (Some("t1"), Some("prepared"), Some("restore_failed")));
        assert_eq!(aside[0].interaction_id.as_deref(), Some("rust"), "a pergunta segue a do Rust");
        let (work, aside) = apply(vec![], &facts, true);
        assert_eq!(work.len() + aside.len(), 1, "linha de recuperação sem a origem descoberta");
        let done = ListFacts { overrides: vec![ext], frozen: HashSet::new(), unknown: false, ..Default::default() };
        let (work, _) = apply(vec![row(json!({"name":"s","provider":"claude"}))], &done, true);
        assert_eq!(work[0].transfer_phase.as_deref(), Some("prepared"), "fase terminal classifica e mostra a fase");
    }

    #[test]
    fn projected_options_keep_their_id() {
        let answer: Value = serde_json::from_str(r#"{"states":{"k":{"state":"awaiting_input","label":null,"question":"Rodar?",
                "options":["Sim","Não"],"interaction_id":"shown-a","problema":null,"status_line":null,
                "last_activity":null,"limit_reset":null}}}"#).unwrap();
        let row = serde_json::from_value::<SessionRow>(json!({"name": "k", "provider": "kimi", "interaction_id": "velho"})).unwrap();
        let (work, _) = apply(vec![row], &ListFacts { states: Arc::new(serde_json::from_value(answer["states"].clone()).unwrap()), unknown: false, ..Default::default() }, true);
        assert_eq!((work[0].options.as_ref().map(Vec::len), work[0].interaction_id.as_deref()), (Some(2), Some("shown-a")),
            "opções sem o ID delas não respondem; ID de outra pergunta também não");
    }

    /// Estado do Python velho marca só Pi/omp/Kimi: Claude e Codex não dependem dele.
    #[test]
    fn stale_python_marks_only_its_providers_without_hiding_a_sharper_problem() {
        let row = |v: Value| serde_json::from_value::<SessionRow>(v).unwrap();
        let mut rows = vec![row(json!({"name": "c", "provider": "claude"})), row(json!({"name": "x", "provider": "codex"})),
            row(json!({"name": "p", "provider": "pi"})), row(json!({"name": "o", "provider": "omp", "problema": "list_runtime_unavailable"}))];
        let good = ListFacts { unknown: false, ..Default::default() };
        mark_stale(&mut rows, &good, true);
        assert!(rows.iter().all(|r| r.problema.is_none() || r.name == "o"));
        mark_stale(&mut rows, &good, false);
        let got: Vec<_> = rows.iter().map(|r| r.problema.as_deref()).collect();
        assert_eq!(got, [None, None, Some(UNAVAILABLE), Some("list_runtime_unavailable")]);
        let (work, aside) = apply(vec![row(json!({"name": "c", "provider": "claude"})), row(json!({"name": "p", "provider": "pi"}))],
            &ListFacts { orq: vec![row(json!({"name": "g-orq", "provider": "orq"}))], unknown: false, ..Default::default() }, false);
        assert_eq!((work[0].problema.as_deref(), work[1].problema.as_deref(), aside[0].problema.as_deref()), (None, Some(UNAVAILABLE), None));
        mark_stale(&mut rows, &ListFacts::default(), true);
        assert_eq!(rows[0].problema.as_deref(), Some(UNAVAILABLE), "transferências nunca lidas: toda linha");
    }
}
