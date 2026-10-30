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
pub fn merge(estado: &mut Map<String, Value>, home: &Path, state_dir: &Path) {
    let Some(key) = failure_key(estado) else { return };
    match read_outcome(state_dir).filter(|o| o.falha == key) {
        Some(Outcome { codigo: Some(code), .. }) => { estado.insert("relatorio".into(), code.into()); }
        Some(Outcome { erro: Some(error), .. }) => { estado.insert("relatorio_erro".into(), error.into()); }
        _ if consent(home).ok().flatten() == Some(true) => { estado.insert("relatorio".into(), "pendente".into()); }
        _ => {}
    }
}

static SENDING: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Manda o relatório da falha atual, uma vez por falha, se a pessoa consentiu.
pub async fn check_failure(st: &AppState) {
    let _one = SENDING.lock().await;
    let home = st.accounts.env.home.clone();
    let state_dir = crate::update::state_dir(&st.accounts.env.claude_base);
    let estado = crate::update::StateFile::new(&state_dir).read();
    let Some(key) = failure_key(&estado) else { return };
    if read_outcome(&state_dir).is_some_and(|o| o.falha == key) || consent(&home).ok().flatten() != Some(true) { return; }
    let report: String = format!("Enviado sozinho: a atualização para {} falhou. {}", text(&estado, "versao_para"), text(&estado, "erro"))
        .chars().take(super::TEXT_MAX).collect();
    let version = crate::update::running_package().map_or(env!("CARGO_PKG_VERSION"), |p| p.version.as_str());
    let mut outcome = Outcome { falha: key, ..Outcome::default() };
    for (attempt, wait) in RETRY_WAITS.iter().enumerate() {
        tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
        let input = super::ClientInput { text: Some(report.clone()), client: format!("auto-update {version}"), failures: Vec::new(), group: None, group_token: None };
        match super::route::send(st, input).await {
            Ok((sent, _)) => { outcome.codigo = Some(sent.code); break; }
            Err(f) if f.code == "diag_report_rate_limited" && attempt + 1 < RETRY_WAITS.len() => {}
            Err(f) => { outcome.erro = Some(f.code.to_owned()); break; }
        }
    }
    match &outcome.erro {
        Some(code) => st.diag.report("rust.diag_report_auto", "", code, "relatório automático da atualização não enviado"),
        None => tracing::info!(code = outcome.codigo.as_deref().unwrap_or(""), "relatório automático da atualização enviado"),
    }
    let saved = serde_json::to_vec(&outcome).map_err(io::Error::other)
        .and_then(|bytes| crate::runtime::queue::atomic_write(&state_dir.join(OUTCOME_FILE), &bytes));
    if let Err(e) = saved { tracing::warn!(code = "auto_report_outcome_unsaved", "relatório automático: resultado não gravado: {e}"); }
}

/// Confere na subida (a falha com reinício volta com outro processo) e a cada executor que sai.
pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        check_failure(&st).await;
        loop {
            crate::update::executor::EXITED.notified().await;
            check_failure(&st).await;
        }
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
    struct Worker { status: u16, reports: std::sync::Mutex<Vec<String>> }

    async fn reply(axum::extract::State(w): axum::extract::State<Arc<Worker>>, body: String) -> (StatusCode, String) {
        w.reports.lock().unwrap().push(body);
        let reply = if w.status == 201 { json!({"id": "diag/abcd2345", "code": "abcd2345"}) } else { json!({"erro": "x"}) };
        (StatusCode::from_u16(w.status).unwrap(), reply.to_string())
    }

    struct Machine { st: Arc<AppState>, worker: Arc<Worker>, home: tempfile::TempDir, task: tokio::task::JoinHandle<()> }

    impl Drop for Machine { fn drop(&mut self) { self.task.abort(); } }

    impl Machine {
        async fn start(status: u16) -> Machine {
            let home = tempfile::tempdir().unwrap();
            let worker = Arc::new(Worker { status, reports: std::sync::Mutex::default() });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let router = axum::Router::new().route("/api/relatorio", axum::routing::post(reply)).with_state(worker.clone());
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
            let mut st = AppState::new(Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
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

        fn screen(&self) -> Map<String, Value> {
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

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_send_is_recorded_and_not_repeated() {
        let m = Machine::start(500).await;
        m.fail(Some(true));
        check_failure(&m.st).await;
        check_failure(&m.st).await;
        assert_eq!(m.worker.reports.lock().unwrap().len(), 1);
        assert_eq!(m.screen()["relatorio_erro"], "diag_report_refused");
    }
}
