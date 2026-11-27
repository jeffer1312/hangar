//! Relatório automático quando a atualização falha: o consentimento da pessoa, perguntado uma vez
//! pelos clientes. Sem resposta, nada sai da máquina.
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::routes::{AppState, gate};
use crate::session_write::{detail, json_response};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Consent { auto_report: bool }

static CONSENT_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn consent_path(home: &Path) -> PathBuf { home.join(".hangar").join("auto-report.json") }

/// `None` = ainda não respondido.
pub fn consent(home: &Path) -> io::Result<Option<bool>> {
    match crate::runtime::queue::read_replaced(&consent_path(home)) {
        Ok(bytes) => serde_json::from_slice::<Consent>(&bytes).map(|c| Some(c.auto_report)).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET`/`PUT /api/auto-report` com `{"auto_report": bool}`; o `GET` sem resposta dá `null`. Só o dono.
pub async fn setting(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let refused = || detail(StatusCode::FORBIDDEN, "auto_report_owner_only", "só o dono pode ler ou mudar o relatório automático", json!({}));
    if crate::routes::on_guest_port(&req) || req.headers().contains_key("x-hangar-guest") { return refused(); }
    if !(match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
        return refused();
    }
    let save = if req.method() == Method::PUT {
        let parsed = to_bytes(req.into_body(), 1024).await.ok().and_then(|raw| serde_json::from_slice::<Consent>(&raw).ok());
        let Some(save) = parsed else {
            return detail(StatusCode::BAD_REQUEST, "auto_report_invalid", "esperado {\"auto_report\": true ou false}", json!({}));
        };
        Some(save)
    } else {
        None
    };
    let saving = save.is_some();
    let home = st.accounts.env.home.clone();
    let result = tokio::task::spawn_blocking(move || {
        if let Some(save) = save {
            let _guard = CONSENT_WRITE.lock().map_err(|_| io::Error::other("trava do consentimento indisponível"))?;
            let path = consent_path(&home);
            std::fs::create_dir_all(path.parent().expect("pasta do arquivo"))?;
            crate::runtime::queue::atomic_write(&path, &serde_json::to_vec(&save)?)?;
        }
        consent(&home)
    }).await;
    let cause = match result {
        Ok(Ok(answer)) => {
            // Quem diz sim depois de uma falha recente não espera o próximo reinício pelo envio.
            if saving && answer == Some(true) {
                let st = st.clone();
                tokio::spawn(async move { check_failure(&st).await });
            }
            return json_response(StatusCode::OK, json!({"auto_report": answer}));
        }
        Ok(Err(e)) => e.to_string(),
        Err(e) => format!("tarefa morreu: {e}"),
    };
    tracing::warn!(code = "auto_report_unavailable", "relatório automático: escolha não lida ou não guardada: {cause}");
    detail(StatusCode::SERVICE_UNAVAILABLE, "auto_report_unavailable", "não foi possível ler ou guardar a escolha do relatório automático", json!({}))
}

// ---------- envio quando a atualização falha ----------

/// O resultado do envio de uma falha: só este servidor grava, fora do `estado.json`, para nada
/// vazar para a rodada seguinte.
#[derive(Deserialize, Serialize, Default)]
struct Outcome {
    falha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codigo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    erro: Option<String>,
}

const OUTCOME_FILE: &str = "relatorio.json";
/// Falha mais velha que isso não sai sozinha: é a de antes do consentimento ou da versão com o recurso.
const FRESH: chrono::TimeDelta = chrono::TimeDelta::hours(24);
/// O worker limita por IP e no total; máquinas que falham juntas se espalham nas esperas.
const RETRY_WAITS: [u64; 4] = [0, 60, 180, 420];

fn text<'a>(estado: &'a Map<String, Value>, key: &str) -> &'a str { estado.get(key).and_then(Value::as_str).unwrap_or("") }

/// A falha atual, se houver: o `ts` muda a cada escrita, então a chave leva também a versão e o erro.
fn failure_key(estado: &Map<String, Value>) -> Option<String> {
    if text(estado, "fase") != "pronto" || estado.get("ok") != Some(&Value::Bool(false)) { return None; }
    let at = chrono::DateTime::parse_from_rfc3339(text(estado, "ts")).ok()?;
    if chrono::Utc::now().signed_duration_since(at) > FRESH { return None; }
    Some(format!("{}|{}|{}", text(estado, "ts"), text(estado, "versao_para"), text(estado, "erro")))
}

fn read_outcome(state_dir: &Path) -> Option<Outcome> {
    crate::runtime::queue::read_replaced(&state_dir.join(OUTCOME_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Os campos que `GET /api/atualizacao` junta ao `estado`: o código, o erro do envio, ou `pendente`
/// enquanto a falha atual espera o envio com consentimento.
pub fn merge(estado: &mut crate::update::StateSnapshot, home: &Path, state_dir: &Path) {
    let Some(key) = failure_key(estado) else { return };
    match read_outcome(state_dir).filter(|o| o.falha == key) {
        Some(Outcome { codigo: Some(code), .. }) => { estado.insert("relatorio".into(), code.into()); }
        Some(Outcome { erro: Some(error), .. }) => { estado.insert("relatorio_erro".into(), error.into()); }
        _ if consent(home).ok().flatten() == Some(true) => { estado.insert("relatorio".into(), "pendente".into()); }
        _ => {}
    }
}

static SENDING: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Sem rede ou com o worker limitando, a falha fica sem envio e volta a ser tentada na próxima subida.
fn is_transient(code: &str) -> bool { matches!(code, "diag_report_unreachable" | "diag_report_rate_limited") }

/// A falha atual, se a pessoa consentiu e ela ainda não saiu. Bloqueante.
fn unsent_failure(home: &Path, state_dir: &Path) -> Option<(String, crate::update::StateSnapshot)> {
    let estado = crate::update::StateFile::new(state_dir).read();
    let key = failure_key(&estado)?;
    if read_outcome(state_dir).is_some_and(|o| o.falha == key && !o.erro.as_deref().is_some_and(is_transient)) { return None; }
    match consent(home) {
        Ok(Some(true)) => Some((key, estado)),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(code = "auto_report_unavailable", "relatório automático: escolha não lida: {e}");
            None
        }
    }
}

/// Manda o relatório da falha atual, uma vez por falha, se a pessoa consentiu.
pub async fn check_failure(st: &AppState) { send_failure(st, &RETRY_WAITS).await }

async fn send_failure(st: &AppState, waits: &[u64]) {
    let _one = SENDING.lock().await;
    let home = st.accounts.env.home.clone();
    let state_dir = crate::update::state_dir(&st.accounts.env.claude_base);
    let unsent = || {
        let (home, state_dir) = (home.clone(), state_dir.clone());
        tokio::task::spawn_blocking(move || unsent_failure(&home, &state_dir))
    };
    let Ok(Some((key, estado))) = unsent().await else { return };
    let report: String = format!("Enviado sozinho: a atualização para {} falhou. {}", text(&estado, "versao_para"), text(&estado, "erro"))
        .chars().take(super::TEXT_MAX).collect();
    let version = crate::update::running_package().map_or(env!("CARGO_PKG_VERSION"), |p| p.version.as_str());
    let mut outcome = Outcome { falha: key.clone(), ..Outcome::default() };
    for (attempt, wait) in waits.iter().enumerate() {
        tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
        // A espera é longa: quem desligou o relatório ou começou outra rodada nesse meio-tempo não manda.
        if attempt > 0 && !matches!(unsent().await, Ok(Some((current, _))) if current == key) { return; }
        let input = super::ClientInput { text: Some(report.clone()), client: format!("auto-update {version}"), failures: Vec::new(), group: None, group_token: None };
        match super::route::send(st, input).await {
            Ok((sent, _)) => { outcome.codigo = Some(sent.code); break; }
            Err(f) if is_transient(f.code) && attempt + 1 < waits.len() => {}
            Err(f) => { outcome.erro = Some(f.code.to_owned()); break; }
        }
    }
    match &outcome.erro {
        Some(code) => st.diag.report("rust.diag_report_auto", "", code, "relatório automático da atualização não enviado"),
        None => tracing::info!(code = outcome.codigo.as_deref().unwrap_or(""), "relatório automático da atualização enviado"),
    }
    let saved = serde_json::to_vec(&outcome).map_err(io::Error::other)
        .and_then(|bytes| crate::runtime::queue::atomic_write(&state_dir.join(OUTCOME_FILE), &bytes));
    if let Err(e) = saved {
        tracing::warn!(code = "auto_report_outcome_unsaved", "relatório automático: resultado não gravado: {e}");
        st.diag.report("rust.diag_report_auto", "", "auto_report_outcome_unsaved", "resultado do relatório automático não gravado");
    }
}

// ---------- telemetria da atualização ----------

/// Conta o desfecho de uma atualização de pacote, uma vez por rodada. Marca que não se lê nem se grava
/// volta `Err`, sem contar nem dar a rodada por contada. Bloqueante (`once` grava disco).
fn count_outcome(telemetry: &hangar_telemetry::Telemetry, estado: &Map<String, Value>, channel: &str, now: chrono::DateTime<chrono::Utc>) -> io::Result<()> {
    use hangar_telemetry::events::{Code, DimValue, Dims, Event, UpdateOrigin, UpdateStepName};
    if text(estado, "fase") != "pronto" { return Ok(()); }
    let Some(ok) = estado.get("ok").and_then(Value::as_bool) else { return Ok(()) };
    // O executor antigo não grava `origem`: vale a rodada que já sabia a versão nova. `origem` nula é
    // reinício ou modo desenvolvedor.
    let origin = match estado.get("origem") {
        Some(Value::String(origin)) => UpdateOrigin::parse(origin).unwrap_or(UpdateOrigin::Other),
        None if estado.get("versao_para").is_some_and(Value::is_string) => UpdateOrigin::Other,
        _ => return Ok(()),
    };
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(text(estado, "ts")) else { return Ok(()) };
    // Rodada antiga (consentimento dado depois) não entra no dia de hoje.
    if now.signed_duration_since(at) > FRESH { return Ok(()); }
    let (from, to) = (text(estado, "versao_de"), text(estado, "versao_para"));
    if !telemetry.once(&format!("update:{from}->{to}:{}", text(estado, "ts")))? { return Ok(()); }
    let manual = estado.get("reiniciar_manual").and_then(Value::as_bool) == Some(true);
    let result = match (ok, to.is_empty(), manual) {
        (false, _, _) => "failed",
        (true, true, _) => "up_to_date",
        (true, false, true) => "updated_manual",
        (true, false, false) => "updated",
    };
    telemetry.count(Event::UpdateFinished, Dims::new().with("result", Code::fixed(result))
        .with("from", DimValue::version(from)).with("to", DimValue::version(to))
        .with("channel", Code::known(channel, hangar_telemetry::wire::CHANNELS)).with("origin", origin), 1);
    let failed_at = (!ok).then(|| text(estado, "etapa"));
    if let Some(Value::Object(took)) = estado.get("duracoes") {
        for (stage, ms) in took {
            let Some(ms) = ms.as_u64() else { continue };
            let dims = Dims::new().with("step", UpdateStepName::from_stage(stage)).with("ok", failed_at != Some(stage.as_str()));
            telemetry.duration(Event::UpdateStep, dims, std::time::Duration::from_millis(ms));
        }
    }
    if !ok {
        let code = Code::known(text(estado, "codigo"), crate::update::FAILURE_CODES);
        telemetry.count(Event::UpdateFailedCode, Dims::new().with("code", code), 1);
    }
    Ok(())
}

/// Quem conta é o servidor de pé, que lê o `estado.json`: o executor é outro processo. `false` quando a
/// contagem falhou e precisa de outra tentativa.
async fn count_update(st: &AppState) -> bool {
    let Some(telemetry) = st.telemetry.clone().filter(|t| t.enabled()) else { return true };
    let home = st.accounts.env.home.clone();
    let state_dir = crate::update::state_dir(&st.accounts.env.claude_base);
    let counted = tokio::task::spawn_blocking(move || {
        let estado = crate::update::StateFile::new(&state_dir).read();
        // Rodada que não terminou não conta: o canal só é lido para a que terminou.
        if text(&estado, "fase") != "pronto" { return Ok(()); }
        count_outcome(&telemetry, &estado, &round_channel(&estado, &home), chrono::Utc::now())
    }).await;
    match counted {
        Ok(Ok(())) => true,
        Ok(Err(e)) => { crate::telemetry_routes::report(&st.diag, hangar_telemetry::Failure::Disk(e)); false }
        Err(_) => { crate::telemetry_routes::task_failed(&st.diag); false }
    }
}

/// O canal da rodada; o executor antigo não o grava (e o reinício avulso o zera), e vale o da máquina.
fn round_channel(estado: &Map<String, Value>, home: &Path) -> String {
    estado.get("canal").and_then(Value::as_str).map(str::to_owned)
        .unwrap_or_else(|| crate::update::channel(&crate::update::Layout::new(home)))
}

/// A marca que não gravou não espera outra gravação do `estado.json` para tentar de novo.
const COUNT_RETRY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Avisa a cada gravação do `estado.json`. O executor é outro processo: depois do reinício, quem está
/// de pé não o lançou e só vê o arquivo.
fn watch_state(state_dir: &Path, changed: Arc<tokio::sync::Notify>) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher;
    std::fs::create_dir_all(state_dir).map_err(notify::Error::io)?;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // O notify assina abrir e fechar: a leitura do próprio estado acordaria sem fim.
        let wake = event.is_ok_and(|e| !e.kind.is_access() && e.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "estado.json")));
        if wake { changed.notify_one(); }
    })?;
    watcher.watch(state_dir, notify::RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// Confere na subida e a cada gravação do estado.
pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> { watch(st, None) }

/// `ready` avisa quando o vigia já observa o `estado.json` e a conferência da subida terminou.
fn watch(st: Arc<AppState>, ready: Option<tokio::sync::oneshot::Sender<()>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let changed = Arc::new(tokio::sync::Notify::new());
        let state_dir = crate::update::state_dir(&st.accounts.env.claude_base);
        let _watcher = watch_state(&state_dir, changed.clone()).inspect_err(|e| {
            tracing::warn!(code = "auto_report_watch_unavailable", "relatório automático: estado sem vigia: {e}");
            st.diag.report("rust.diag_report_auto", "", "auto_report_watch_unavailable", "falha de atualização só é conferida na subida");
        });
        let mut retry = !count_update(&st).await;
        check_failure(&st).await;
        if let Some(ready) = ready { let _ = ready.send(()); }
        // O envio espera minutos entre tentativas: num laço à parte, a rodada que termina nesse meio-tempo
        // ainda é contada. Avisos que chegam durante um envio viram uma conferência só, depois dele.
        let send = tokio::sync::Notify::new();
        let counting = async {
            loop {
                // A rodada fora do `FRESH` deixa de contar: a nova tentativa acaba sozinha.
                let changed_state = if retry { tokio::time::timeout(COUNT_RETRY, changed.notified()).await.is_ok() } else { changed.notified().await; true };
                retry = !count_update(&st).await;
                if changed_state { send.notify_one(); }
            }
        };
        let sending = async {
            loop {
                send.notified().await;
                check_failure(&st).await;
            }
        };
        tokio::join!(counting, sending);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_reads_the_answer_and_absence_is_unanswered() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(consent(home.path()).unwrap(), None);
        std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
        for (text, expected) in [(r#"{"auto_report":true}"#, true), (r#"{"auto_report":false}"#, false)] {
            std::fs::write(consent_path(home.path()), text).unwrap();
            assert_eq!(consent(home.path()).unwrap(), Some(expected));
        }
        std::fs::write(consent_path(home.path()), "{").unwrap();
        assert!(consent(home.path()).is_err());
    }

    use crate::accounts::{AccountService, environment::AccountEnvironment};
    use crate::auth::TrustedHosts;
    use crate::config::Config;

    /// O worker falso: conta os relatórios e responde com `status`.
    struct Worker { status: u16, reports: std::sync::Mutex<Vec<String>>, arrived: tokio::sync::Notify }

    async fn reply(axum::extract::State(w): axum::extract::State<Arc<Worker>>, body: String) -> (StatusCode, String) {
        w.reports.lock().unwrap().push(body);
        w.arrived.notify_one();
        let reply = if w.status == 201 { json!({"id": "diag/abcd2345", "code": "abcd2345"}) } else { json!({"erro": "x"}) };
        (StatusCode::from_u16(w.status).unwrap(), reply.to_string())
    }

    struct Machine { st: Arc<AppState>, worker: Arc<Worker>, home: tempfile::TempDir, task: tokio::task::JoinHandle<()> }

    impl Drop for Machine { fn drop(&mut self) { self.task.abort(); } }

    impl Machine {
        async fn start(status: u16) -> Machine {
            let home = tempfile::tempdir().unwrap();
            let worker = Arc::new(Worker { status, reports: std::sync::Mutex::default(), arrived: tokio::sync::Notify::new() });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let router = axum::Router::new().route("/api/relatorio", axum::routing::post(reply)).with_state(worker.clone());
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
            let mut st = AppState::new(Config { listen: "127.0.0.1:0".parse().unwrap(),
                internal_secret: "s".into(), auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
            st.accounts = AccountService::new(AccountEnvironment::from_map([
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.path().to_string_lossy().into_owned())].into()));
            st.diag_report_base = format!("http://{address}");
            Machine { st: Arc::new(st), worker, home, task }
        }

        fn state_dir(&self) -> PathBuf { crate::update::state_dir(&self.st.accounts.env.claude_base) }

        fn fail(&self, consent: Option<bool>) {
            crate::update::StateFile::new(&self.state_dir()).write(json!({"fase": "pronto", "ok": false, "versao_para": "0.1.0.8700",
                "erro": "o servidor não voltou depois de reiniciar"}));
            if let Some(answer) = consent {
                std::fs::create_dir_all(self.home.path().join(".hangar")).unwrap();
                std::fs::write(consent_path(self.home.path()), json!({"auto_report": answer}).to_string()).unwrap();
            }
        }

        fn screen(&self) -> crate::update::StateSnapshot {
            let mut estado = crate::update::StateFile::new(&self.state_dir()).read();
            merge(&mut estado, self.home.path(), &self.state_dir());
            estado
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failure_with_consent_is_sent_once_and_shown() {
        let m = Machine::start(201).await;
        m.fail(Some(true));
        assert_eq!(m.screen()["relatorio"], "pendente");
        tokio::join!(check_failure(&m.st), check_failure(&m.st));
        check_failure(&m.st).await;
        let reports = m.worker.reports.lock().unwrap().clone();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("Enviado sozinho: a atualização para 0.1.0.8700 falhou."), "{}", reports[0]);
        assert_eq!(m.screen()["relatorio"], "abcd2345");
        // Rodada nova: nada da falha anterior aparece.
        crate::update::StateFile::new(&m.state_dir()).write(json!({"fase": "rodando", "ok": null}));
        assert!(!m.screen().contains_key("relatorio"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_consent_nothing_is_sent() {
        for consent in [None, Some(false)] {
            let m = Machine::start(201).await;
            m.fail(consent);
            check_failure(&m.st).await;
            assert!(m.worker.reports.lock().unwrap().is_empty());
            assert!(!m.screen().contains_key("relatorio"));
        }
    }

    /// O executor é outro processo: depois do reinício, quem está de pé não o lançou e só vê o arquivo.
    #[tokio::test(flavor = "multi_thread")]
    async fn failure_written_after_the_server_came_up_is_sent() {
        let m = Machine::start(201).await;
        std::fs::create_dir_all(m.home.path().join(".hangar")).unwrap();
        std::fs::write(consent_path(m.home.path()), json!({"auto_report": true}).to_string()).unwrap();
        let (ready, watching) = tokio::sync::oneshot::channel();
        let task = watch(m.st.clone(), Some(ready));
        watching.await.unwrap();
        m.fail(None);
        let arrived = tokio::time::timeout(std::time::Duration::from_secs(20), m.worker.arrived.notified()).await;
        task.abort();
        assert!(arrived.is_ok(), "o relatório não chegou");
        assert_eq!(m.worker.reports.lock().unwrap().len(), 1);
    }

    /// Com um envio esperando entre tentativas, as rodadas que terminam no meio-tempo ainda são contadas.
    #[tokio::test(flavor = "multi_thread")]
    async fn update_rounds_are_counted_while_a_report_is_being_sent() {
        let mut m = Machine::start(201).await;
        let t = Arc::new(telemetry(m.home.path()));
        Arc::get_mut(&mut m.st).unwrap().telemetry = Some(t.clone());
        let (ready, watching) = tokio::sync::oneshot::channel();
        let task = watch(m.st.clone(), Some(ready));
        watching.await.unwrap();
        // O envio em curso: `check_failure` fica parado na trava.
        let sending = SENDING.lock().await;
        let counted = |n: u64| {
            let t = t.clone();
            tokio::time::timeout(std::time::Duration::from_secs(20), async move {
                while t.counters().iter().filter(|c| c.event == "update_finished").map(|c| c.count).sum::<u64>() < n { tokio::task::yield_now().await; }
            })
        };
        for (n, to) in [(1, "0.1.0.8700"), (2, "0.1.0.8800")] {
            crate::update::StateFile::new(&m.state_dir()).write(json!({"fase": "pronto", "ok": true, "origem": "auto", "canal": "latest",
                "versao_de": "0.1.0.8600", "versao_para": to}));
            assert!(counted(n).await.is_ok(), "rodada {to} não contada com o envio parado");
        }
        drop(sending);
        task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_send_is_recorded_and_not_repeated() {
        let m = Machine::start(500).await;
        m.fail(Some(true));
        check_failure(&m.st).await;
        check_failure(&m.st).await;
        assert_eq!(m.worker.reports.lock().unwrap().len(), 1);
        assert_eq!(m.screen()["relatorio_erro"], "diag_report_refused");
    }

    fn at() -> chrono::DateTime<chrono::Utc> { chrono::DateTime::parse_from_rfc3339("2026-11-26T10:00:00Z").unwrap().with_timezone(&chrono::Utc) }

    /// O relógio parado em `at()`: o `ts` do estado é de uma hora antes.
    fn telemetry(home: &Path) -> hangar_telemetry::Telemetry {
        let telemetry = hangar_telemetry::Telemetry::for_test(home, Arc::new(at));
        telemetry.set_consent(true).unwrap();
        telemetry
    }

    /// O estado gravado no fim, `ts` uma hora antes de `at()`.
    fn estado(fields: Value) -> Map<String, Value> {
        let Value::Object(mut map) = fields else { panic!("objeto") };
        map.insert("ts".into(), "2026-11-26T09:00:00+00:00".into());
        map
    }

    fn counter<'a>(counters: &'a [hangar_telemetry::wire::Counter], event: &str) -> &'a hangar_telemetry::wire::Counter {
        counters.iter().find(|c| c.event == event).unwrap_or_else(|| panic!("sem {event}: {counters:?}"))
    }

    #[test]
    fn update_channel_is_the_round_one_and_falls_back_to_the_machine_one() {
        let home = tempfile::tempdir().unwrap();
        let file = crate::update::channel_file(&crate::update::Layout::new(home.path()));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, json!({"channel": "feat/x"}).to_string()).unwrap();
        let machine = "feat/x";
        assert_eq!(super::round_channel(&estado(json!({"canal": "beta"})), home.path()), "beta");
        assert_eq!(super::round_channel(&estado(json!({"canal": null})), home.path()), machine, "canal nulo vale o da máquina");
        assert_eq!(super::round_channel(&estado(json!({})), home.path()), machine, "executor antigo, sem o campo");
    }

    #[test]
    fn update_failed_download_counts_once() {
        let home = tempfile::tempdir().unwrap();
        let t = telemetry(home.path());
        let failed = estado(json!({"fase": "pronto", "ok": false, "origem": "manual", "versao_de": "0.1.0.8600", "versao_para": "0.1.0.8700",
            "etapa": "baixar", "codigo": "update_download_failed", "duracoes": {"baixar": 2500}, "erro": "o download do pacote falhou: /home/x"}));
        count_outcome(&t, &failed, "latest", at()).unwrap();
        let counters = t.counters();
        let finished = counter(&counters, "update_finished");
        assert_eq!((finished.dims["result"].as_str(), finished.dims["origin"].as_str(), finished.count), ("failed", "manual", 1));
        assert_eq!((finished.dims["from"].as_str(), finished.dims["to"].as_str(), finished.dims["channel"].as_str()), ("0.1.0.8600", "0.1.0.8700", "latest"));
        let code = counter(&counters, "update_failed_code");
        assert_eq!((code.dims["code"].as_str(), code.count), ("update_download_failed", 1));
        let step = counter(&counters, "update_step");
        assert_eq!((step.dims["step"].as_str(), step.dims["ok"].as_str(), step.bucket.as_deref()), ("download", "false", Some("lt3s")));
        assert_eq!(counters.len(), 3);
        assert!(counters.iter().all(|c| c.dims.values().all(|v| !v.contains('/'))), "o erro em texto nunca sai");

        count_outcome(&t, &failed, "latest", at()).unwrap();
        assert_eq!(t.counters(), counters, "reler o mesmo estado não conta de novo");
    }

    #[test]
    fn update_whose_mark_cannot_be_written_is_not_counted_nor_marked() {
        let home = tempfile::tempdir().unwrap();
        let t = telemetry(home.path());
        let done = estado(json!({"fase": "pronto", "ok": true, "origem": "auto", "versao_de": "0.1.0.8600", "versao_para": "0.1.0.8700"}));
        // Uma pasta no lugar do arquivo das marcas: a leitura falha em todo sistema.
        let marks = home.path().join(".hangar/telemetry-v3/once.json");
        std::fs::create_dir_all(&marks).unwrap();
        assert!(count_outcome(&t, &done, "latest", at()).is_err());
        assert!(t.counters().is_empty());
        std::fs::remove_dir(&marks).unwrap();
        count_outcome(&t, &done, "latest", at()).unwrap();
        assert_eq!(counter(&t.counters(), "update_finished").count, 1, "a rodada não ficou dada por contada");
    }

    #[test]
    fn update_without_origin_is_other_and_non_package_rounds_do_not_count() {
        let home = tempfile::tempdir().unwrap();
        let t = telemetry(home.path());
        // Executor antigo: sem `origem` nem `codigo`.
        count_outcome(&t, &estado(json!({"fase": "pronto", "ok": true, "versao_de": "0.1.0.8600", "versao_para": "0.1.0.8700",
            "etapa": "reiniciar", "texto": "Atualizado"})), "feat/x", at()).unwrap();
        let counters = t.counters();
        let finished = counter(&counters, "update_finished");
        assert_eq!((finished.dims["result"].as_str(), finished.dims["origin"].as_str(), finished.dims["channel"].as_str()), ("updated", "other", "other"));
        assert_eq!(counters.len(), 1);

        // Reinício pelo servidor novo (`origem` nula), reinício pelo antigo (sem versão nova) e rodada ainda de pé.
        for other in [json!({"fase": "pronto", "ok": false, "origem": null, "versao_para": "0.1.0.8700", "codigo": "update_io"}),
                      json!({"fase": "pronto", "ok": true, "versao_de": null, "versao_para": null}),
                      json!({"fase": "rodando", "ok": null, "origem": "auto", "versao_para": "0.1.0.8800"})] {
            count_outcome(&t, &estado(other), "latest", at()).unwrap();
        }
        // Rodada de mais de um dia atrás: o consentimento veio depois dela.
        count_outcome(&t, &estado(json!({"fase": "pronto", "ok": true, "origem": "auto", "versao_para": "0.1.0.8900"})),
            "latest", at() + chrono::TimeDelta::days(2)).unwrap();
        assert_eq!(t.counters(), counters);
    }

    #[test]
    fn update_unknown_code_is_other_and_every_update_error_code_is_known() {
        let home = tempfile::tempdir().unwrap();
        let t = telemetry(home.path());
        count_outcome(&t, &estado(json!({"fase": "pronto", "ok": false, "origem": "auto", "versao_de": "0.1.0.8600",
            "versao_para": "0.1.0.8700", "codigo": "update_novo_codigo"})), "beta", at()).unwrap();
        assert_eq!(counter(&t.counters(), "update_failed_code").dims["code"], "other");

        use crate::update::UpdateError;
        use hangar_package::ExtractError;
        let errors = [UpdateError::Locked, UpdateError::Io(String::new()), UpdateError::Index(String::new()), UpdateError::NoPlatform(String::new()),
            UpdateError::Download(String::new()), UpdateError::Sha { expected: String::new(), got: String::new() }, UpdateError::Step(String::new()),
            UpdateError::Swap(String::new()), UpdateError::Package(ExtractError::ManifestInvalid(String::new())),
            UpdateError::Package(ExtractError::ShaMismatch(String::new())), UpdateError::Package(ExtractError::UnsafePath(String::new())),
            UpdateError::Package(ExtractError::ExistingDiffers(String::new())), UpdateError::Package(ExtractError::RenameFailed(io::Error::other("x"))),
            UpdateError::Package(ExtractError::Io(io::Error::other("x")))];
        for error in errors {
            assert!(crate::update::FAILURE_CODES.contains(&error.code()), "{}", error.code());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn limited_send_is_shown_and_tried_again_later() {
        let m = Machine::start(429).await;
        m.fail(Some(true));
        send_failure(&m.st, &[0, 0]).await;
        assert_eq!(m.worker.reports.lock().unwrap().len(), 2);
        assert_eq!(m.screen()["relatorio_erro"], "diag_report_rate_limited");
        send_failure(&m.st, &[0]).await;
        assert_eq!(m.worker.reports.lock().unwrap().len(), 3);
    }
}
