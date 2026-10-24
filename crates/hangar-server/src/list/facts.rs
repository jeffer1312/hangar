//! Fatos da lista. Do Python (`POST /internal/list/facts`) só o estado, a conta e a última resposta
//! de Pi, omp e Kimi, e só com linha deles na lista; o resto (transferências, orquestrações, acesso,
//! atalhos, problemas) a ponte lê do disco (`local_facts`). Pergunta só quando a entrada muda ou o
//! último ficou velho, com prazo de 1 s; sem resposta, fica o último valor e o motivo vai ao diário.
//! Passada a carência (`GRACE`), as linhas Pi/omp/Kimi levam `problema = list_facts_unavailable`.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use hangar_api::session::SessionRow;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};

/// Prazo da resposta, e idade máxima da última para a mesma entrada.
pub const FACTS_TIMEOUT: Duration = Duration::from_secs(1);
pub const FACTS_TTL: Duration = Duration::from_secs(1);
/// Falha isolada não marca a lista: com resposta boa recente, as linhas seguem limpas até
/// `GRACE_FAILS` falhas seguidas ou `GRACE` sem resposta boa. Cada falha vai ao diário mesmo assim.
const GRACE: Duration = Duration::from_secs(5);
const GRACE_FAILS: u32 = 3;
const MAX_BODY: usize = 8 << 20;
pub const UNAVAILABLE: &str = "list_facts_unavailable";
/// Provedores cujo estado o Python ainda calcula (`registry.list_with_state`).
pub const OTHERS: [&str; 3] = ["pi", "omp", "kimi"];

/// Estado de uma linha não migrada, com o código de hoje do Python.
#[derive(Debug, Deserialize)]
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
    /// A descoberta não sabe a credencial.
    #[serde(default)]
    pub conta: Option<String>,
    /// O histórico deles ainda é lido pelo Python.
    #[serde(default)]
    pub last_reply: Option<String>,
    #[serde(default)]
    pub last_reply_at: Option<f64>,
}

pub type States = Arc<HashMap<String, RowState>>;

/// Os fatos de uma rodada: `states` do Python, o resto lido do disco pela ponte.
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

/// A resposta do Python; chave que falta é contrato quebrado, nunca vazio.
#[derive(Deserialize)]
struct Answer { states: HashMap<String, RowState> }

#[derive(Serialize)]
struct Request<'a> { rows: &'a [SessionRow], pane_pids: &'a BTreeMap<String, u32> }

pub struct Fetched {
    pub states: States,
    /// `false`: o Python não respondeu e a carência acabou; `states` é o último bom (ou vazio).
    pub ok: bool,
    /// `true` só com resposta desta pergunta (ou do cache dela): quem decide quem morreu não usa
    /// fatos da carência.
    pub fresh: bool,
}

/// `ok = false`: a última pergunta falhou; até `at + FACTS_TTL` o tique não espera o prazo de novo.
struct Last { key: u64, at: Instant, ok: bool, states: States,
    /// Falhas seguidas e a hora da última resposta boa, para a carência.
    fails: u32, good_at: Option<Instant> }

impl Last {
    fn fetched(&self) -> Fetched {
        let graced = !self.ok && self.fails < GRACE_FAILS && self.good_at.is_some_and(|t| t.elapsed() < GRACE);
        Fetched { states: self.states.clone(), ok: self.ok || graced, fresh: self.ok }
    }
}

pub struct FactsClient {
    upstream: SocketAddr,
    secret: String,
    http: crate::proxy::HttpClient,
    /// O diário que o dono exporta: falha da lista que só chegasse ao `tracing` sumiria dele.
    pub diag: crate::diag::DiagClient,
    /// Trava assíncrona = uma pergunta por vez; quem chega no meio reaproveita a resposta.
    last: tokio::sync::Mutex<Option<Last>>,
}

impl FactsClient {
    pub fn new(upstream: SocketAddr, secret: String) -> Self {
        let diag = crate::diag::DiagClient::new(upstream, secret.clone());
        Self { upstream, secret, http: crate::proxy::client(), diag, last: tokio::sync::Mutex::new(None) }
    }

    /// `rows`: só as linhas Pi, omp e Kimi.
    pub async fn fetch(&self, rows: &[SessionRow], pane_pids: &BTreeMap<String, u32>) -> Fetched {
        let mut last = self.last.lock().await;
        let body = match serde_json::to_vec(&Request { rows, pane_pids }) {
            Ok(b) => b,
            Err(_) => return self.failed(&mut last, "list_facts_request".into()),
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut h);
        let key = h.finish();
        if let Some(l) = last.as_ref().filter(|l| l.at.elapsed() < FACTS_TTL && (l.key == key || !l.ok)) {
            return l.fetched();
        }
        match self.ask(body).await {
            Ok(states) => {
                let now = Instant::now();
                let l = Last { key, at: now, ok: true, states: Arc::new(states), fails: 0, good_at: Some(now) };
                let got = l.fetched();
                *last = Some(l);
                got
            }
            Err(code) => self.failed(&mut last, code),
        }
    }

    fn failed(&self, last: &mut Option<Last>, code: String) -> Fetched {
        if crate::warn_limit::allow(None, &code) {
            tracing::warn!(code, "lista: fatos do Python indisponíveis; fica o último valor");
        }
        self.diag.report("rust.list_facts_unavailable", "", &code, "fatos do Python indisponíveis; fica o último valor");
        let (states, fails, good_at) = last.as_ref().map_or_else(|| (States::default(), 0, None), |l| (l.states.clone(), l.fails, l.good_at));
        let l = Last { key: 0, at: Instant::now(), ok: false, states, fails: fails + 1, good_at };
        let got = l.fetched();
        *last = Some(l);
        got
    }

    /// `Err` é o código que vai ao diário: motivo e status, nunca o corpo nem a mensagem do serde,
    /// que podem ecoar valores.
    async fn ask(&self, body: Vec<u8>) -> Result<HashMap<String, RowState>, String> {
        let req = axum::http::Request::post(format!("http://{}/internal/list/facts", self.upstream))
            .header("x-hangar-internal", &self.secret).header("content-type", "application/json")
            .body(Body::from(body)).map_err(|_| "list_facts_request".to_owned())?;
        let work = async {
            let resp = self.http.request(req).await.map_err(|e| unreachable_code("list_facts", &e))?;
            if !resp.status().is_success() {
                return Err(format!("list_facts_status:{}", resp.status().as_u16()));
            }
            let bytes = http_body_util::Limited::new(resp.into_body(), MAX_BODY).collect().await
                .map_err(|_| "list_facts_body".to_owned())?.to_bytes();
            serde_json::from_slice::<Answer>(&bytes).map(|a| a.states).map_err(|e| format!("list_facts_invalid:{:?}", e.classify()))
        };
        tokio::time::timeout(FACTS_TIMEOUT, work).await.map_err(|_| "list_facts_timeout".to_owned())?
    }
}

fn unreachable_code(prefix: &str, e: &hyper_util::client::legacy::Error) -> String {
    format!("{prefix}_{}", if e.is_connect() { "connect" } else { "unreachable" })
}

/// Estado do Python velho: só as linhas Pi/omp/Kimi levam o aviso; transferências nunca lidas, todas.
/// Problema mais preciso que a linha já tenha fica.
pub fn mark_stale(rows: &mut [SessionRow], facts: &ListFacts, ok: bool) {
    for row in rows.iter_mut().filter(|r| facts.unknown || (!ok && OTHERS.contains(&r.provider.as_str()))) {
        row.problema.get_or_insert_with(|| UNAVAILABLE.into());
    }
}

/// Falha vista em código síncrono (arquivo lido dentro do `spawn_blocking`) a caminho do diário:
/// a produção esvazia e manda pelo `diag`. Uma por minuto por (sessão, código); fila com teto.
pub struct Note { pub event: &'static str, pub session: String, pub code: String, pub reason: &'static str }

#[cfg(not(test))]
const MAX_NOTES: usize = 64;
// Nos testes ninguém esvazia a fila.
#[cfg(test)]
const MAX_NOTES: usize = 100_000;
static NOTES: LazyLock<Mutex<Vec<Note>>> = LazyLock::new(Default::default);
/// Nos testes, tudo o que entrou na fila: a rodada da lista de outro teste a esvazia em paralelo.
#[cfg(test)]
static NOTED: LazyLock<Mutex<Vec<(String, String)>>> = LazyLock::new(Default::default);

pub fn note(event: &'static str, session: &str, code: String, reason: &'static str) {
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
        q.push(Note { event, session: session.to_owned(), code, reason });
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
            row.conta.clone_from(&s.conta);
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
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    /// A rodada da lista esvazia a fila em paralelo aos testes: o que um teste anotou continua visível.
    #[test]
    fn a_note_stays_visible_after_the_round_takes_the_queue() {
        note("list_test", "notes-drained", "list_test_code".into(), "teste");
        drop(take_notes());
        assert_eq!(notes_for("notes-drained"), ["list_test_code"]);
    }

    /// Python de mentira: `answer(caminho, n-ésimo pedido dos fatos)` dá (status, corpo); guarda caminho e corpo.
    async fn python(answer: fn(&str, usize) -> (u16, String)) -> (SocketAddr, Seen) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Seen = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    loop {
                        let (mut first, mut length) = (String::new(), 0usize);
                        if reader.read_line(&mut first).await.unwrap_or(0) == 0 { return; }
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                            if line == "\r\n" { break; }
                            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = v.trim().parse().unwrap(); }
                        }
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).await.unwrap();
                        let path = first.split_whitespace().nth(1).unwrap().to_owned();
                        let n = {
                            let mut l = log.lock().unwrap();
                            l.push((path.clone(), serde_json::from_slice(&body).unwrap_or(Value::Null)));
                            l.iter().filter(|(p, _)| p == &path).count()
                        };
                        let (status, out) = answer(&path, n);
                        let head = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\n\r\n", out.len());
                        reader.get_mut().write_all(format!("{head}{out}").as_bytes()).await.unwrap();
                    }
                });
            }
        });
        (addr, seen)
    }

    async fn diary(seen: &Seen, event: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let hit = seen.lock().unwrap().iter().find(|(p, b)| p == "/internal/diag" && b["evento"] == event).map(|(_, b)| b.clone());
                if let Some(b) = hit { return b; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("diário não recebeu")
    }

    fn pi(name: &str) -> SessionRow { serde_json::from_value(json!({"name": name, "provider": "pi"})).unwrap() }

    /// O pedido leva só linhas e pids; a resposta é só `states`.
    #[tokio::test]
    async fn the_python_is_asked_only_for_rows_and_pane_pids() {
        let (addr, seen) = python(|_, _| (200, r#"{"states":{"p":{"state":"idle","label":null,"question":null,"options":null,
            "problema":null,"status_line":null,"last_activity":null,"limit_reset":null,"conta":"pi:x"}}}"#.into())).await;
        let client = FactsClient::new(addr, "s".into());
        let got = client.fetch(&[pi("p")], &BTreeMap::from([("p".into(), 7)])).await;
        assert!(got.ok && got.fresh);
        assert_eq!(got.states["p"].conta.as_deref(), Some("pi:x"));
        let body = seen.lock().unwrap()[0].1.clone();
        let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["pane_pids", "rows"]);
        assert_eq!(body["pane_pids"]["p"], 7);
    }

    #[tokio::test]
    async fn failure_reaches_the_diary_with_the_status() {
        let (addr, seen) = python(|path, _| if path == "/internal/diag" { (200, "{}".into()) } else { (503, "x".into()) }).await;
        let client = FactsClient::new(addr, "s".into());
        let got = client.fetch(&[], &BTreeMap::new()).await;
        assert!(!got.ok);
        assert_eq!(diary(&seen, "rust.list_facts_unavailable").await["codigo"], "list_facts_status:503");
    }

    #[tokio::test]
    async fn one_failure_after_a_good_answer_does_not_mark_the_list() {
        let (addr, seen) = python(|path, n| match (path, n) {
            ("/internal/diag", _) => (200, "{}".into()),
            (_, 1) => (200, r#"{"states":{}}"#.into()),
            _ => (503, "x".into()),
        }).await;
        let client = FactsClient::new(addr, "s".into());
        let first = client.fetch(&[pi("a")], &BTreeMap::new()).await;
        assert!(first.ok && first.fresh);
        let second = client.fetch(&[pi("b")], &BTreeMap::new()).await;
        assert!(second.ok && !second.fresh, "carência: lista limpa, mas não serve para decidir quem morreu");
        assert_eq!(diary(&seen, "rust.list_facts_unavailable").await["codigo"], "list_facts_status:503");
    }

    #[test]
    fn grace_ends_after_three_failures_or_five_seconds() {
        let last = |fails, good_at| Last { key: 0, at: Instant::now(), ok: false, states: States::default(), fails, good_at };
        let now = Instant::now();
        assert!(last(1, Some(now)).fetched().ok);
        assert!(last(2, Some(now)).fetched().ok);
        assert!(!last(3, Some(now)).fetched().ok);
        assert!(!last(1, now.checked_sub(GRACE)).fetched().ok);
        assert!(!last(1, None).fetched().ok, "sem resposta boa nunca houve carência");
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
        let answer: Answer = serde_json::from_str(r#"{"states":{"k":{"state":"awaiting_input","label":null,"question":"Rodar?",
                "options":["Sim","Não"],"interaction_id":"shown-a","problema":null,"status_line":null,
                "last_activity":null,"limit_reset":null}}}"#).unwrap();
        let row = serde_json::from_value::<SessionRow>(json!({"name": "k", "provider": "kimi", "interaction_id": "velho"})).unwrap();
        let (work, _) = apply(vec![row], &ListFacts { states: Arc::new(answer.states), unknown: false, ..Default::default() }, true);
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
