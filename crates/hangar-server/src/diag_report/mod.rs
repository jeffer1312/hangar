//! Pacote de diagnóstico do "Reportar problema": o texto do relatório (C2/C3) e o diário inteiro
//! compactado (C2b). Quem envia é a rota; aqui só se coleta, limpa e monta.
mod collect;
mod route;
mod scrub;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Local, TimeZone};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Instant;

pub use route::{WORKER_BASE, report_route};
pub use scrub::Scrubber;

#[derive(serde::Deserialize)]
pub struct ClientInput {
    pub text: Option<String>,
    pub client: String,
    pub failures: Vec<ClientFailure>,
}

#[derive(serde::Deserialize)]
pub struct ClientFailure {
    pub ts: f64,
    pub route: String,
    pub status: u16,
    pub code: Option<String>,
}

pub struct Bundle {
    /// O `Report` (C2) serializado, até `BODY_MAX`.
    pub body: Vec<u8>,
    /// O diário inteiro (C2b), gzip, até 10 MiB; vazio sem pasta de logs.
    pub diary_gz: Vec<u8>,
}

/// Teto do corpo serializado que vai ao worker.
pub const BODY_MAX: usize = 250 * 1024;
/// Teto do texto da pessoa, na entrada (C1) e no relatório.
pub(crate) const TEXT_MAX: usize = 2000;
const SUMMARY_FAILURES: usize = 20;
const SUMMARY_SESSIONS: usize = 15;
const SUMMARY_ACCOUNTS: usize = 10;

/// Tudo o que entra no texto, já reduzido aos campos permitidos.
pub(crate) struct Facts {
    pub now: DateTime<Local>,
    pub versions: collect::Versions,
    pub sessions: collect::Sessions,
    pub accounts: Result<Vec<collect::AccountFact>, String>,
    pub diary: collect::Diary,
    pub server_log: Vec<String>,
    pub python: BTreeMap<String, [u32; 2]>,
    pub updates: Vec<String>,
}

/// Prazo do `build` inteiro: estourado, segue com o que já foi coletado.
const BUILD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(40);

type Files = (collect::Diary, Vec<String>, BTreeMap<String, [u32; 2]>, Vec<String>);
type DiaryPaths = Arc<Vec<(String, PathBuf)>>;

/// Leitura de disco fora do executor; `Err` é a nota que vai no lugar do `hangar-server.log`.
async fn off_thread<T: Send + 'static>(deadline: Instant, f: impl FnOnce() -> T + Send + 'static) -> Result<T, &'static str> {
    match tokio::time::timeout_at(deadline, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err("(leitura dos logs falhou)"),
        Err(_) => Err("(leitura dos logs passou do prazo)"),
    }
}

/// Os quatro leitores em paralelo; o que falha sai vazio e deixa a nota no topo do server log.
async fn read_logs(base: Option<PathBuf>, paths: DiaryPaths, s: Arc<Scrubber>, now: DateTime<Local>, deadline: Instant) -> Files {
    let Some(base) = base.map(Arc::new) else {
        return (collect::Diary::default(), vec!["(sem pasta de logs)".to_owned()], BTreeMap::new(), Vec::new());
    };
    let diary = { let s = s.clone(); off_thread(deadline, move || collect::read_diary(&paths, now, &s)) };
    let server_log = { let (b, s) = (base.clone(), s.clone()); off_thread(deadline, move || collect::read_server_log(&b, now, &s)) };
    let python = { let (b, s) = (base.clone(), s.clone()); off_thread(deadline, move || collect::python_counts(&b, now, &s)) };
    let updates = off_thread(deadline, move || collect::read_updates(&base, &s));
    let (diary, server_log, python, updates) = tokio::join!(diary, server_log, python, updates);
    let failed = [diary.as_ref().err(), server_log.as_ref().err(), python.as_ref().err(), updates.as_ref().err()]
        .into_iter().flatten().next().copied();
    let mut server_log = server_log.unwrap_or_default();
    if let Some(note) = failed { server_log.insert(0, note.to_owned()); }
    (diary.unwrap_or_default(), server_log, python.unwrap_or_default(), updates.unwrap_or_default())
}

pub async fn build(st: &crate::routes::AppState, input: ClientInput) -> Bundle {
    let deadline = Instant::now() + BUILD_DEADLINE;
    // A pasta de logs e a casa vêm do ambiente do servidor: no teste, a casa de teste.
    let base = crate::runtime::local_policy::log_base_in(&st.accounts.env);
    let scrubber = Arc::new(Scrubber::for_env(&st.accounts.env));
    let now = Local::now();
    // Um passeio só por `diario/`, lido pelo texto e pelo anexo.
    let paths: DiaryPaths = match base.clone() {
        Some(base) => Arc::new(off_thread(deadline, move || collect::diary_files(&base)).await.unwrap_or_default()),
        None => Arc::default(),
    };
    let diary_gz = {
        let (p, s) = (paths.clone(), scrubber.clone());
        tokio::task::spawn_blocking(move || collect::diary_gz(&p, &s, collect::DIARY_GZ_MAX))
    };
    let s = &*scrubber;
    let (versions, sessions, accounts, (diary, server_log, python, updates)) = tokio::join!(
        async { tokio::time::timeout_at(deadline, collect::versions(st, s)).await.unwrap_or_else(|_| collect::Versions::local()) },
        async {
            tokio::time::timeout_at(deadline, collect::sessions(st, s)).await
                .unwrap_or(collect::Sessions { list_error: Some(collect::LIST_TIMEOUT), facts: Vec::new() })
        },
        async { tokio::time::timeout_at(deadline, collect::accounts(st, s)).await.unwrap_or_else(|_| Err("diag_deadline".to_owned())) },
        read_logs(base, paths, scrubber.clone(), now, deadline),
    );
    let facts = Facts { now, versions, sessions, accounts, diary, server_log, python, updates };
    let (app, os) = (facts.versions.app(), facts.versions.arch.clone());
    let assembled = {
        let s = scrubber.clone();
        tokio::task::spawn_blocking(move || assemble(&facts, &input, &s, BODY_MAX))
    };
    // A montagem e o anexo têm pelo menos 5 s mesmo com o prazo gasto na coleta.
    let late = deadline.max(Instant::now() + collect::PROBE_WAIT);
    let body = match tokio::time::timeout_at(late, assembled).await {
        Ok(Ok(body)) => body,
        _ => report_body(&app, "", &os, "(montagem do relatório falhou ou passou do prazo no servidor)"),
    };
    let diary_gz = match tokio::time::timeout_at(late, diary_gz).await {
        Ok(Ok(gz)) => gz,
        _ => Vec::new(),
    };
    Bundle { body, diary_gz }
}

/// Só os caracteres que o worker aceita nos campos curtos (`SHORT`).
fn short(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric() || " _.:+()/·-".contains(*c)).take(200).collect()
}

fn report_body(app: &str, commit: &str, os: &str, report: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "v": 1, "kind": "diagnostico", "app": short(app), "commit": short(commit), "os": short(os),
        "step": "diagnostico", "code": null, "outcome": "aberto", "agent": null, "report": report,
    })).unwrap_or_default()
}

fn at(ts: f64) -> String {
    Local.timestamp_opt(ts as i64, 0).single().map_or("?".to_owned(), |t| t.format("%m-%d %H:%M:%S").to_string())
}

fn failure_line(f: &ClientFailure, s: &Scrubber) -> String {
    let status = if f.status == 0 { "rede".to_owned() } else { f.status.to_string() };
    format!("- {} {} {} {}", at(f.ts), status, collect::clean(s, &f.route, 200), f.code.as_deref().map_or("-".to_owned(), |c| collect::clean(s, c, 64)))
}

/// `title`, depois uma linha por item, ou `empty` sem item nenhum.
fn section(t: &mut String, title: &str, empty: &str, items: impl IntoIterator<Item = impl AsRef<str>>) {
    t.push_str(title);
    let start = t.len();
    for item in items { t.push_str(item.as_ref()); t.push('\n'); }
    if t.len() == start { t.push_str(empty); t.push('\n'); }
}

/// Tudo antes das linhas do diário: o resumo e as seções do C3, até o título `## Diário (24 h)`.
fn head_text(facts: &Facts, input: &ClientInput, scrubber: &Scrubber) -> String {
    let now_s = facts.now.timestamp() as f64;
    let v = &facts.versions;
    let mut failures: Vec<&ClientFailure> = input.failures.iter().collect();
    failures.sort_by(|a, b| b.ts.total_cmp(&a.ts));
    let failures: Vec<String> = failures.into_iter().map(|f| failure_line(f, scrubber)).collect();
    let accounts: &[collect::AccountFact] = facts.accounts.as_deref().unwrap_or(&[]);
    let versions = format!("hangar-server {} · pacote {} · commit {} · {} · {} · claude {} · tmux {} · fuso {}",
        v.server, if v.package.is_empty() { "?" } else { &v.package }, if v.commit.is_empty() { "?" } else { &v.commit },
        v.os, v.arch, v.claude, v.mux, v.offset);
    let mut t = format!("# Relatório de diagnóstico · {}\n\n", facts.now.format("%Y-%m-%d %H:%M:%S %:z"));
    let said = input.text.as_deref().map(|text| collect::clean(scrubber, text, TEXT_MAX)).filter(|text| !text.is_empty());
    t += &format!("O que aconteceu:\n{}\n\n", said.as_deref().unwrap_or("(não descrito)"));
    t += &format!("Versões: {versions}\n\n");
    let troubled: Vec<_> = facts.sessions.facts.iter().filter(|s| s.troubled()).collect();
    t += &format!("Sessões com problema: {}\n", troubled.len());
    for s in troubled.iter().take(SUMMARY_SESSIONS) { t += &format!("- {}\n", s.line()); }
    let flagged: Vec<_> = accounts.iter().filter(|a| a.near_limit() || a.login_expiring(now_s) || a.spent_until.is_some_and(|u| u > now_s)).collect();
    t += &format!("\nContas perto do limite ou com login vencendo: {}\n", flagged.len());
    for a in flagged.iter().take(SUMMARY_ACCOUNTS) { t += &format!("- {}\n", a.line(now_s)); }
    t += &format!("\nFalhas recentes do cliente: {}\n", failures.len());
    for f in failures.iter().take(SUMMARY_FAILURES) { t += f; t.push('\n'); }

    t += &format!("\n## Cliente\n{}\nFalhas ({}):\n", collect::clean(scrubber, &input.client, 200), failures.len());
    for f in &failures { t += f; t.push('\n'); }

    t += "\n## Sessões\n";
    match facts.sessions.list_error {
        Some(collect::LIST_TIMEOUT) => t += "(sem resposta)\n",
        Some(code) => t += &format!("(lista indisponível: {code})\n"),
        None => {}
    }
    if facts.sessions.facts.is_empty() && facts.sessions.list_error.is_none() { t += "(nenhuma)\n"; }
    for s in &facts.sessions.facts { t += &format!("- {}\n", s.line()); }

    t += "\n## Contas\n";
    match &facts.accounts {
        Err(code) => t += &format!("(contas indisponíveis: {code})\n"),
        Ok(list) => section(&mut t, "", "(nenhuma)", list.iter().map(|a| format!("- {}", a.line(now_s)))),
    }

    t += &format!("\n## Versões\n{}\n", versions.replace(" · ", "\n"));

    section(&mut t, "\n## Reinícios e atualizações\nReinícios (24 h):\n", "(nenhum)",
        facts.diary.restarts.iter().map(|r| format!("- {}", r.restart_line())));
    section(&mut t, "Atualizações (últimas 10):\n", "(nenhuma)", facts.updates.iter().map(|u| format!("- {u}")));
    section(&mut t, "\n## hangar-server.log\n", "(nenhum aviso ou erro nas 24 h)", &facts.server_log);
    section(&mut t, "\n## Log do Python (contagem)\n", "(nenhum aviso ou erro nas 24 h)",
        facts.python.iter().map(|(logger, [warn, error])| format!("{logger}: {warn} avisos, {error} erros")));

    t += "\n## Diário (24 h)\n";
    t
}

fn cut_note(dropped: usize) -> String {
    if dropped > 0 { format!("(diário cortado: {dropped} linhas mais antigas)\n") } else { String::new() }
}

/// Tamanho de `text` dentro de uma string JSON, sem as aspas: o escape é por caractere, então o
/// tamanho de um trecho somado ao do outro dá o do texto junto.
fn json_len(text: &str) -> usize {
    serde_json::to_string(text).map_or(text.len(), |quoted| quoted.len() - 2)
}

/// Monta o texto e corta as linhas mais antigas do diário até o corpo caber em `max`. Cada parte
/// já vem limpa da coleta ou de `head_text`, sempre antes do seu corte; nada é limpo de novo aqui.
pub(crate) fn assemble(facts: &Facts, input: &ClientInput, scrubber: &Scrubber, max: usize) -> Vec<u8> {
    let v = &facts.versions;
    let app = v.app();
    let body_of = |text: &str| report_body(&app, &v.commit, &v.arch, text);
    let mut text = head_text(facts, input, scrubber);
    let diary: Vec<String> = facts.diary.problems.iter().map(collect::DiaryEntry::compact).collect();
    // Menor corte que cabe, pela soma do tamanho escapado de cada linha (`\n` vira 2 bytes); se
    // nem sem diário cabe, sai o diário inteiro.
    let mut dropped = 0;
    if !diary.is_empty() {
        let head = body_of(&text).len();
        let sizes: Vec<usize> = diary.iter().map(|line| json_len(line) + 2).collect();
        let mut kept: usize = sizes.iter().sum();
        while dropped < diary.len() && head + json_len(&cut_note(dropped)) + kept > max {
            kept -= sizes[dropped];
            dropped += 1;
        }
    }
    text += &cut_note(dropped);
    if diary.is_empty() { text += "(nenhum aviso ou erro nas 24 h)\n"; }
    for line in &diary[dropped..] { text += line; text.push('\n'); }
    // A serialização final confere o teto.
    let mut body = body_of(&text);
    // Sem diário e ainda grande: corta o fim do texto, no limite de caractere.
    const NOTE: &str = "\n(relatório cortado no teto)";
    while body.len() > max && !text.is_empty() {
        let over = body.len() - max + NOTE.len() + 64;
        let mut end = text.trim_end_matches(NOTE).len().saturating_sub(over);
        while !text.is_char_boundary(end) { end -= 1; }
        text.truncate(end);
        text += NOTE;
        body = body_of(&text);
    }
    body
}
