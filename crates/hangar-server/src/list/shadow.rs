//! Rodada em sombra (`CP_LIST_SHADOW=1`): com o Python ainda dono da lista, o Rust a produz a cada
//! tique sem servir, compara campo a campo com a assinatura da lista que o Python serviu (vem na
//! resposta dos fatos) e grava `rust.list_shadow_diff` no diário só com o nome da sessão e o do
//! campo. Nada do Rust é entregue: sem rebaixar marcador, sem mexer na presença do app.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_api::session::SessionRow;
use serde_json::{Map, Value, json};

use super::bridge::{ListBridge, ProduceFacts};
use super::classify::RUNTIME_ABSENT;
use super::sig;
use crate::diag::DiagClient;

pub const ENV: &str = "CP_LIST_SHADOW";
/// O tique do `_ListRefresher`.
const TICK: Duration = Duration::from_millis(1500);
/// O Python sem lista recente (ninguém com ela aberta): nada a comparar, e a descoberta não roda à toa.
const IDLE: Duration = Duration::from_secs(10);
/// Sombra cega por motivo esperado (ninguém com a lista aberta, multiplexador recusando) vai ao diário
/// só depois deste tempo seguido; falha de verdade, na hora.
const BLIND_LIMIT: Duration = Duration::from_secs(120);
/// Janela da contagem: cada (sessão, campo) vai ao diário no máximo uma vez nela, com a contagem. Não
/// menor que o limite de uma linha por minuto do diário, que derrubaria o relatório repetido.
const WINDOW: Duration = crate::warn_limit::WARN_INTERVAL;
/// Teto de diferenças gravadas por janela: um defeito em todas as linhas não inunda o diário; quantas
/// ficaram de fora vai junto.
const MAX_REPORTS: usize = 50;
pub const DIFFS_DROPPED: &str = "diffs_dropped";
pub const ROW_MISSING: &str = "row_missing";
pub const ROW_EXTRA: &str = "row_extra";
pub const ROW_UNSERIALIZABLE: &str = "row_unserializable";

/// Assinatura de cada linha que o Python serviu, por nome: `{campo: valor reduzido}`.
pub type PySigs = HashMap<String, Map<String, Value>>;
/// (sessão, campo)
pub type Diff = (String, String);

/// Diferenças de propósito, registradas pelas Tasks que as criaram: não vão ao diário. Erro da
/// produção com estes códigos não é diferença: o Python lia a mesma recusa como zero sessões.
pub const ACCEPTED_ERRORS: [&str; 2] = ["mux_refused", "mux_unparsed"];
/// Campos que o retrato do runtime preenche nas sessões Claude sem terminal. Só aceitos quando a
/// linha diz que o retrato não a trouxe (`RUNTIME_ABSENT`: servidor sem runtime ligado).
const RUNTIME_FIELDS: [&str; 6] = ["state", "label", "question", "status_line", "pending_questions", "problema"];

pub fn enabled() -> bool { enabled_from(std::env::var(ENV).ok().as_deref()) }

fn enabled_from(v: Option<&str>) -> bool { v == Some("1") }

/// Diferença que alguma Task fez de propósito, com o motivo ao lado. `py_state`: o estado da linha
/// na lista do Python.
fn accepted(row: &SessionRow, field: &str, py_state: Option<&str>) -> bool {
    match row.problema.as_deref() {
        // Captura que falhou (Task 12): o Rust fica no marcador sem rebaixar e mostra a falha.
        Some("list_capture_failed") if ["state", "problema", "label"].contains(&field) => return true,
        // Runtime sem terminal com erro (Task 12): a falha aparece na linha em vez de parada calada.
        Some("list_runtime_unavailable") if ["state", "problema"].contains(&field) => return true,
        _ => {}
    }
    let absent = row.provider == "claude" && row.problema.as_deref() == Some(RUNTIME_ABSENT);
    (absent && RUNTIME_FIELDS.contains(&field))
        // A última resposta só existe na linha parada: sem retrato, ela diverge junto com o estado.
        || (absent && ["last_reply", "last_reply_at"].contains(&field) && py_state != Some(row.state.as_str()))
        // A descoberta não sabe a credencial de Kimi/Pi/omp (Task 7); só o fato a preenche (Task 14).
        || (["kimi", "pi", "omp"].contains(&row.provider.as_str()) && field == "conta" && row.conta.is_none())
}

/// Kimi, Pi e omp têm nome de `sanitize_session_name`, que no Rust só desfaz os acentos do
/// português (`discover_other.rs`): letra de fora some em vez de virar a base, e o nome fica mais
/// curto. Mesma conversa (transcript e pasta, ambos conhecidos) com o nome do Rust contido no do
/// Python, a partir da mesma letra, é essa sessão.
fn renamed(row: &SessionRow, py_name: &str, sig: &Map<String, Value>) -> bool {
    let (Some(jsonl), Some(cwd)) = (&row.jsonl, &row.cwd) else { return false };
    let mut rest = py_name.chars();
    ["kimi", "pi", "omp"].contains(&row.provider.as_str())
        && sig.get("jsonl").and_then(Value::as_str) == Some(jsonl) && sig.get("cwd").and_then(Value::as_str) == Some(cwd)
        && row.name.chars().next().is_some_and(|c| py_name.starts_with(c))
        && row.name.chars().all(|c| rest.any(|p| p == c))
}

/// O valor do campo como o `_list_sig` o reduz.
fn field_value(row: &SessionRow, raw: &Value, field: &str) -> Value {
    match field {
        "status_line" => sig::status_sig(row.status_line.as_deref()),
        "context" => json!(sig::context_sig(row.context.as_ref())),
        "label" if row.provider == "codex" && !row.tracked => json!(row.label),
        "label" => json!(row.label.as_deref().is_some_and(|l| !l.is_empty())),
        "plan_tasks" => json!(row.plan_tasks.as_deref().unwrap_or_default()),
        // Campo que o Rust não tem vale nulo: o do Python com valor aparece como diferença.
        _ => raw.get(field).cloned().unwrap_or(Value::Null),
    }
}

/// Igualdade do JSON com número comparado pelo valor: `1` do Python e `1.0` do Rust são o mesmo.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

/// Campos em que a lista do Rust diverge da do Python, fora as diferenças aceitas.
pub fn compare(rust: &[SessionRow], py: &PySigs) -> HashSet<Diff> {
    let mut out = HashSet::new();
    let by_name: HashMap<&str, &SessionRow> = rust.iter().map(|r| (r.name.as_str(), r)).collect();
    let mut extra: Vec<&SessionRow> = rust.iter().filter(|r| !py.contains_key(&r.name)).collect();
    for (name, sig) in py {
        let row = match by_name.get(name.as_str()) {
            Some(row) => *row,
            None => match extra.iter().position(|r| renamed(r, name, sig)) {
                Some(k) => extra.swap_remove(k),
                None => {
                    out.insert((name.clone(), ROW_MISSING.into()));
                    continue;
                }
            },
        };
        let Ok(raw) = serde_json::to_value(row) else {
            out.insert((name.clone(), ROW_UNSERIALIZABLE.into()));
            continue;
        };
        let py_state = sig.get("state").and_then(Value::as_str);
        for (field, want) in sig.iter().filter(|(f, _)| *f != "name") {
            if !same(&field_value(row, &raw, field), want) && !accepted(row, field, py_state) {
                out.insert((name.clone(), field.clone()));
            }
        }
    }
    out.extend(extra.into_iter().map(|r| (r.name.clone(), ROW_EXTRA.into())));
    out
}

/// Conta cada (sessão, campo) divergente em toda rodada comparada e entrega a contagem uma vez por
/// janela. Diferença intermitente também chega; a contagem sobre as rodadas separa o atraso de um tique
/// da lista do Python (1 de N) do que diverge sempre. Rodada cega não conta nem zera.
#[derive(Default)]
pub struct Reporter { counts: HashMap<Diff, u32>, rounds: u32, since: Option<Instant> }

/// O que a janela viu: diferenças com quantas rodadas cada uma divergiu, as mais frequentes primeiro.
#[derive(Debug, PartialEq, Eq)]
pub struct Report { pub diffs: Vec<(Diff, u32)>, pub rounds: u32, pub dropped: usize }

impl Reporter {
    pub fn record(&mut self, found: HashSet<Diff>, now: Instant) {
        self.since.get_or_insert(now);
        self.rounds += 1;
        for d in found {
            *self.counts.entry(d).or_default() += 1;
        }
    }

    /// Fecha a janela vencida: devolve o relatório se houve diferença.
    pub fn tick(&mut self, now: Instant) -> Option<Report> {
        if self.since.is_none_or(|s| now.duration_since(s) < WINDOW) {
            return None;
        }
        self.flush()
    }

    /// Fecha a janela agora (o laço caiu): o que ela contou não se perde.
    pub fn flush(&mut self) -> Option<Report> {
        let rounds = std::mem::take(&mut self.rounds);
        self.since = None;
        let mut diffs: Vec<(Diff, u32)> = std::mem::take(&mut self.counts).into_iter().collect();
        if diffs.is_empty() {
            return None;
        }
        diffs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let dropped = diffs.len().saturating_sub(MAX_REPORTS);
        diffs.truncate(MAX_REPORTS);
        Some(Report { diffs, rounds, dropped })
    }
}

/// `codigo` do diário: o campo e quantas das rodadas da janela divergiram nele. Nunca o valor.
/// No formato que o `/internal/diag` aceita (`[a-z0-9_]{1,64}`).
pub fn diff_code(field: &str, count: u32, rounds: u32) -> String { format!("{field}_{count}_of_{rounds}") }

fn dropped_code(dropped: usize) -> String { format!("{DIFFS_DROPPED}_{dropped}") }

/// Rodada sem comparação, pelo motivo. `quiet`: esperado por um tempo (ninguém com a lista do Python
/// aberta, multiplexador recusando); só vira registro se durar `BLIND_LIMIT`.
#[derive(Default)]
pub struct Blind { code: Option<&'static str>, since: Option<Instant>, told: bool }

impl Blind {
    /// `None` = a rodada comparou. Devolve o código a gravar agora, uma vez por sequência.
    pub fn round(&mut self, code: Option<&'static str>, quiet: bool, now: Instant) -> Option<&'static str> {
        let Some(c) = code else {
            if self.told {
                tracing::info!(code = self.code, "lista: rodada em sombra voltou a comparar");
            }
            *self = Self::default();
            return None;
        };
        if self.code != Some(c) {
            *self = Self { code: Some(c), since: Some(now), told: false };
        }
        let long = self.since.is_some_and(|s| now.duration_since(s) >= BLIND_LIMIT);
        if self.told || (quiet && !long) {
            return None;
        }
        self.told = true;
        Some(c)
    }
}

/// Liga a sombra se `CP_LIST_SHADOW=1`. O laço recomeça depois de um pânico, com registro.
pub fn spawn(list: Arc<ListBridge>, diag: DiagClient) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() {
        return None;
    }
    tracing::info!("lista: rodada em sombra ligada");
    // Fora do laço que pode cair: a janela em curso sobrevive ao pânico.
    let reporter = Arc::new(std::sync::Mutex::new(Reporter::default()));
    Some(tokio::spawn(async move {
        loop {
            // Abortar a de fora derruba esta junto.
            let mut inner = crate::AbortOnDrop(tokio::spawn(run(list.clone(), diag.clone(), reporter.clone())));
            match (&mut inner.0).await {
                Err(e) if e.is_panic() => {
                    diag.report("rust.list_shadow_failed", "", "panic", "rodada em sombra caiu; recomeça");
                    if let Some(rep) = lock(&reporter).flush() { emit(&diag, &rep); }
                    tokio::time::sleep(IDLE).await;
                }
                _ => return,
            }
        }
    }))
}

fn lock(r: &std::sync::Mutex<Reporter>) -> std::sync::MutexGuard<'_, Reporter> { r.lock().unwrap_or_else(|e| e.into_inner()) }

fn emit(diag: &DiagClient, rep: &Report) {
    for ((name, field), count) in &rep.diffs {
        diag.report("rust.list_shadow_diff", name, &diff_code(field, *count, rep.rounds),
            "lista do Rust diverge da do Python neste campo");
    }
    if rep.dropped > 0 {
        diag.report("rust.list_shadow_diff", "", &dropped_code(rep.dropped), "diferenças além do teto da janela");
    }
}

async fn run(list: Arc<ListBridge>, diag: DiagClient, reporter: Arc<std::sync::Mutex<Reporter>>) {
    let input = ProduceFacts { shadow: true, ..Default::default() };
    let mut blind = Blind::default();
    loop {
        // (motivo de não comparar, se é esperado por um tempo, espera)
        let (code, quiet, wait) = match list.produce(&input).await {
            Ok(p) if !p.facts_ok => (Some("facts_unavailable"), false, TICK),
            Ok(p) => match p.facts.shadow.as_ref() {
                Some(py) => {
                    let found = compare(&p.rows, py);
                    lock(&reporter).record(found, Instant::now());
                    (None, false, TICK)
                }
                None => (Some("python_list_absent"), true, IDLE),
            },
            Err(e) => (Some(e.code), ACCEPTED_ERRORS.contains(&e.code), TICK),
        };
        let rep = lock(&reporter).tick(Instant::now());
        if let Some(rep) = rep { emit(&diag, &rep); }
        if let Some(c) = blind.round(code, quiet, Instant::now()) {
            let event = if quiet { "rust.list_shadow_blind" } else { "rust.list_shadow_failed" };
            diag.report(event, "", c, "rodada em sombra sem comparar");
        }
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: Value) -> SessionRow { serde_json::from_value(v).unwrap() }

    /// A assinatura que o `list_facts._row_sig` do Python manda para a linha.
    fn sig_of(r: &SessionRow) -> Map<String, Value> {
        let raw = serde_json::to_value(r).unwrap();
        ["name", "cwd", "state", "jsonl", "label", "status_line", "context", "plan_tasks", "conta", "problema", "pending_questions"]
            .iter().map(|f| ((*f).to_owned(), field_value(r, &raw, f))).collect()
    }

    fn py(rows: &[SessionRow]) -> PySigs { rows.iter().map(|r| (r.name.clone(), sig_of(r))).collect() }

    #[test]
    fn off_by_default() {
        assert!(!enabled_from(None));
        assert!(!enabled_from(Some("0")));
        assert!(!enabled_from(Some("")));
        assert!(enabled_from(Some("1")));
    }

    #[test]
    fn diff_names_fields_only() {
        let cx = row(json!({"name": "cx", "provider": "codex", "tracked": false, "label": "texto da Rust", "state": "idle"}));
        let mut sigs = py(&[cx.clone()]);
        sigs.get_mut("cx").unwrap().insert("label".into(), json!("texto do Python"));
        let found = compare(&[cx], &sigs);
        assert_eq!(found, HashSet::from([("cx".into(), "label".into())]));
        // O que vai ao diário é o nome do campo; o texto do rótulo nunca sai daqui.
        assert!(found.iter().all(|(s, f)| !s.contains("texto") && !f.contains("texto")));
    }

    #[test]
    fn equal_lists_have_no_diff_and_numbers_compare_by_value() {
        let cc = row(json!({"name": "cc", "state": "working", "label": "x", "status_line": "🤖 Haiku │ 💬 1k/2k 50k/200k",
                            "context": {"used": 50_000, "window": 200_000}, "pending_questions": 1}));
        let mut sigs = py(&[cc.clone()]);
        assert!(compare(&[cc.clone()], &sigs).is_empty());
        // `label` de Claude é só presença: o texto do spinner muda a cada quadro.
        let mut other = cc.clone();
        other.label = Some("outro".into());
        assert!(compare(&[other], &sigs).is_empty());
        sigs.get_mut("cc").unwrap().insert("pending_questions".into(), json!(1.0));
        assert!(compare(&[cc], &sigs).is_empty());
    }

    #[test]
    fn missing_and_extra_rows() {
        let a = row(json!({"name": "a"}));
        let b = row(json!({"name": "b"}));
        let found = compare(&[b], &py(&[a]));
        assert_eq!(found, HashSet::from([("a".into(), ROW_MISSING.into()), ("b".into(), ROW_EXTRA.into())]));
    }

    #[test]
    fn accepted_differences_stay_out() {
        // Captura que falhou: o Rust fica no marcador sem rebaixar e mostra a falha.
        let mut cc = row(json!({"name": "cc", "state": "awaiting_input", "problema": "list_capture_failed"}));
        let mut sigs = py(&[row(json!({"name": "cc", "state": "idle"}))]);
        assert!(compare(&[cc.clone()], &sigs).is_empty());
        // Fora da falha, o mesmo estado diferente é diferença.
        cc.problema = None;
        assert_eq!(compare(&[cc], &sigs), HashSet::from([("cc".into(), "state".into()), ]));
        // Runtime sem terminal com erro.
        let hl = row(json!({"name": "hl", "headless": true, "state": "working", "problema": "list_runtime_unavailable"}));
        sigs = py(&[row(json!({"name": "hl", "headless": true, "state": "idle"}))]);
        assert!(compare(&[hl], &sigs).is_empty());
        // Sem terminal e sem retrato do runtime (a linha diz): estado e pergunta ficam no marcador.
        let hl = row(json!({"name": "hl", "headless": true, "state": "idle", "question": "q",
                            "problema": RUNTIME_ABSENT}));
        sigs = py(&[row(json!({"name": "hl", "headless": true, "state": "awaiting_input"}))]);
        assert!(compare(&[hl], &sigs).is_empty());
        // A última resposta só existe na linha parada: com o estado divergindo por falta do retrato, ela
        // diverge junto; com o mesmo estado, é comparada.
        let hl = row(json!({"name": "hl", "headless": true, "state": "idle", "last_reply": "r", "last_reply_at": 1.0,
                            "problema": RUNTIME_ABSENT}));
        let reply = |state: &str| {
            let mut s = py(&[row(json!({"name": "hl", "headless": true, "state": state}))]);
            s.get_mut("hl").unwrap().extend([("last_reply".to_owned(), Value::Null), ("last_reply_at".to_owned(), Value::Null)]);
            s
        };
        assert!(compare(&[hl.clone()], &reply("awaiting_input")).is_empty());
        assert_eq!(compare(&[hl], &reply("idle")), HashSet::from([("hl".into(), "last_reply".into()), ("hl".into(), "last_reply_at".into())]));
        sigs = py(&[row(json!({"name": "hl", "headless": true, "state": "awaiting_input"}))]);
        // Com o retrato, o estado do runtime é comparado como qualquer outro.
        let hl = row(json!({"name": "hl", "headless": true, "state": "idle", "question": "q"}));
        assert_eq!(compare(&[hl], &sigs), HashSet::from([("hl".into(), "state".into())]));
        // Conta de Kimi/Pi/omp, que só o fato preenche; a do Codex continua comparada.
        let k = row(json!({"name": "k", "provider": "kimi", "conta": null}));
        let cx = row(json!({"name": "cx", "provider": "codex", "conta": null}));
        let mut sigs = py(&[k.clone(), cx.clone()]);
        for n in ["k", "cx"] { sigs.get_mut(n).unwrap().insert("conta".into(), json!("kimi:x")); }
        assert_eq!(compare(&[k.clone(), cx.clone()], &sigs), HashSet::from([("cx".into(), "conta".into())]));
        // Com a conta preenchida pelo fato, ela é comparada.
        let mut k = k;
        k.conta = Some("kimi:y".into());
        assert_eq!(compare(&[k, cx], &sigs), HashSet::from([("k".into(), "conta".into()), ("cx".into(), "conta".into())]));
    }

    #[test]
    fn name_folded_differently_is_the_same_session() {
        // "pi-ā" vira "pi-a" no NFKD do Python e "pi" no Rust, que só conhece os acentos do português.
        let rust = row(json!({"name": "pi", "provider": "pi", "jsonl": "/t/1.jsonl", "cwd": "/w", "state": "idle"}));
        let mut python = rust.clone();
        python.name = "pi-a".into();
        assert!(compare(&[rust.clone()], &py(&[python.clone()])).is_empty());
        // Outra conversa com o mesmo começo de nome não é ela.
        python.jsonl = Some("/t/2.jsonl".into());
        assert_eq!(compare(&[rust.clone()], &py(&[python])).len(), 2);
        // Sem transcript, pasta igual não basta: duas sessões novas na mesma pasta se confundiriam.
        let (mut r2, mut p2) = (rust.clone(), rust.clone());
        r2.jsonl = None;
        p2.jsonl = None;
        p2.name = "pi-a".into();
        assert_eq!(compare(&[r2], &py(&[p2])).len(), 2);
        // Nome que não começa igual não é ela.
        let mut p3 = rust.clone();
        p3.name = "api".into();
        assert_eq!(compare(&[rust], &py(&[p3])).len(), 2);
    }

    #[test]
    fn blind_rounds_are_told_once() {
        let t0 = Instant::now();
        let mut b = Blind::default();
        // Falha de verdade: na hora, uma vez por sequência.
        assert_eq!(b.round(Some("facts_unavailable"), false, t0), Some("facts_unavailable"));
        assert_eq!(b.round(Some("facts_unavailable"), false, t0), None);
        assert_eq!(b.round(None, false, t0), None);
        assert_eq!(b.round(Some("facts_unavailable"), false, t0), Some("facts_unavailable"));
        // Esperado: só depois de 2 min seguidos.
        assert_eq!(b.round(Some("python_list_absent"), true, t0), None);
        assert_eq!(b.round(Some("python_list_absent"), true, t0 + Duration::from_secs(60)), None);
        assert_eq!(b.round(Some("python_list_absent"), true, t0 + BLIND_LIMIT), Some("python_list_absent"));
        assert_eq!(b.round(Some("python_list_absent"), true, t0 + BLIND_LIMIT * 2), None);
    }

    #[test]
    fn intermittent_diff_is_reported_with_its_count() {
        let d = |s: &str, f: &str| (s.to_owned(), f.to_owned());
        let t0 = Instant::now();
        let mut r = Reporter::default();
        r.record(HashSet::from([d("a", "state")]), t0);
        // Rodada cega no meio não zera nada.
        assert!(r.tick(t0 + Duration::from_secs(5)).is_none());
        r.record(HashSet::new(), t0 + Duration::from_secs(6));
        r.record(HashSet::from([d("a", "state"), d("b", "label")]), t0 + Duration::from_secs(8));
        assert!(r.tick(t0 + WINDOW - Duration::from_secs(1)).is_none(), "antes de fechar a janela");
        let rep = r.tick(t0 + WINDOW).unwrap();
        assert_eq!(rep.rounds, 3);
        assert_eq!(rep.diffs, vec![(d("a", "state"), 2), (d("b", "label"), 1)]);
        assert_eq!(rep.dropped, 0);
        // A janela seguinte começa do zero; sem diferença, nada sai.
        r.record(HashSet::new(), t0 + WINDOW * 2);
        assert!(r.tick(t0 + WINDOW * 4).is_none());
    }

    #[test]
    fn report_is_capped_and_tells_how_many_were_left() {
        let t0 = Instant::now();
        let mut r = Reporter::default();
        let many: HashSet<Diff> = (0..80).map(|i| (format!("s{i:02}"), "state".to_owned())).collect();
        r.record(many, t0);
        let rep = r.tick(t0 + WINDOW).unwrap();
        assert_eq!((rep.diffs.len(), rep.dropped), (MAX_REPORTS, 80 - MAX_REPORTS));
    }

    #[test]
    fn diary_code_carries_field_and_count_only() {
        assert_eq!(diff_code("label", 3, 20), "label_3_of_20");
        // O `/internal/diag` do Python só aceita `[a-z0-9_]{1,64}` (internal_api.py, `_DIAG_CODE`): fora
        // disso a diferença volta 400 e não chega ao diário.
        let ok = |c: &str| (1..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        // Os campos de `list_facts.SIG_FIELDS`, o maior nome é o que importa.
        let fields = ["name", "cwd", "branch", "git_cwd", "worktree_gone", "git_dirty", "git_ahead", "git_behind", "git_added", "git_removed", "state", "tracked", "headless", "jsonl", "question", "stalled", "limited", "lifecycle_id", "transfer_id", "transfer_phase", "last_reply", "last_reply_at", "pending_questions", "limit_reset", "then_target", "status_line", "context", "model", "label", "startup_steps", "loop_status", "loop_iter", "engine", "conta", "codex_service_tier", "plan_name", "plan_done", "plan_total", "plan_task", "plan_task_total", "plan_complete", "plan_tasks", "plan_hidden", "problema", "provider", "shared", "owner", "orq_arbiter", "pair_peers", "pair_gid", "pair_task", "pair_external"];
        for field in fields.into_iter().chain([ROW_MISSING, ROW_EXTRA, ROW_UNSERIALIZABLE]) {
            assert!(ok(&diff_code(field, 4_294_967_295, 4_294_967_295)), "{field}");
        }
        assert!(ok(&dropped_code(80)));
    }
}
