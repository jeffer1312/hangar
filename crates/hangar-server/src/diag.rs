//! Falhas do servidor vão ao diário local exportável, limitado por diário, sessão e código.
use axum::body::Body;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Código da falha que uma rota do Rust devolveu, para o diário sem reler o corpo da resposta.
#[derive(Clone, Copy)]
pub(crate) struct FailureCode(pub &'static str);

pub(crate) fn coded(
    mut response: axum::response::Response,
    code: &'static str,
) -> axum::response::Response {
    response.extensions_mut().insert(FailureCode(code));
    response
}

/// O código conserva o formato do diário (`[A-Za-z0-9_:]{1,64}`).
fn wire_code(code: &str) -> String {
    let code: String = code
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if code.is_empty() {
        "sem_codigo".into()
    } else {
        code
    }
}

/// Conserva os nomes que os leitores e os filtros do diário já reconhecem.
fn wire_event(event: &str) -> String {
    const PRESERVED: [&str; 5] = [
        "runtime.send_failed",
        "runtime.send_uncertain",
        "runtime.command_deferred",
        "opcao.nao_convergiu",
        "opcao.envio_falhou",
    ];
    let ok = |name: &str| {
        name.len() <= 48
            && !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    };
    if PRESERVED.contains(&event) || event.strip_prefix("rust.").is_some_and(ok) {
        return event.to_owned();
    }
    let name: String = event
        .strip_prefix("rust.")
        .unwrap_or(event)
        .chars()
        .map(|c| if c.is_ascii_lowercase() { c } else { '_' })
        .take(48)
        .collect();
    format!("rust.{name}")
}

type DiaryClock = Arc<dyn Fn() -> chrono::DateTime<chrono::Local> + Send + Sync>;

#[derive(Clone)]
struct DiarySettings {
    directory: Option<std::path::PathBuf>,
    explicit: bool,
    clock: Option<DiaryClock>,
}

#[derive(Clone)]
pub struct DiagClient {
    settings: Arc<Mutex<DiarySettings>>,
    pending: Arc<PendingWrites>,
}

#[derive(Default)]
struct PendingWrites {
    active: std::sync::atomic::AtomicUsize,
    idle: tokio::sync::Notify,
}
struct PendingWrite(Arc<PendingWrites>);
impl Drop for PendingWrite {
    fn drop(&mut self) {
        if self
            .0
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            self.0.idle.notify_waiters();
        }
    }
}

impl DiagClient {
    pub fn new(_upstream: SocketAddr, _secret: String) -> Self {
        Self {
            settings: Arc::new(Mutex::new(DiarySettings {
                directory: if cfg!(test) { None } else { diary_dir() },
                explicit: false,
                clock: None,
            })),
            pending: Arc::default(),
        }
    }

    /// Escolhe o destino antes de ligar as fontes; os clones compartilham essa escolha.
    /// Um destino explícito permite provas isoladas mesmo quando o diário da máquina está desligado.
    pub fn with_diary(self, directory: Option<std::path::PathBuf>) -> Self {
        {
            let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            settings.directory = directory;
            settings.explicit = true;
        }
        self
    }

    /// O relógio do arquivo também pode ser escolhido antes de ligar as fontes.
    pub fn with_clock(
        self,
        clock: impl Fn() -> chrono::DateTime<chrono::Local> + Send + Sync + 'static,
    ) -> Self {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clock = Some(Arc::new(clock));
        self
    }

    /// Aguarda os registros já enviados por este cliente e seus clones.
    pub async fn flush(&self) {
        loop {
            let idle = self.pending.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self
                .pending
                .active
                .load(std::sync::atomic::Ordering::Acquire)
                == 0
            {
                return;
            }
            idle.await;
        }
    }

    /// Metadados curtos do diagnóstico, sem texto de conversa; o escritor conserva apenas escalares.
    pub fn report_with(
        &self,
        event: &'static str,
        session: &str,
        code: &str,
        reason: &'static str,
        fields: serde_json::Value,
    ) {
        self.emit(event, session, code, reason, fields, false, true);
    }

    pub(crate) fn report_response(
        &self,
        event: &'static str,
        scope: &str,
        response: &axum::response::Response,
        reason: &'static str,
    ) {
        let status = response.status().as_u16();
        if let Some(FailureCode(code)) = response.extensions().get::<FailureCode>().copied()
            && (status == 409 || status >= 500)
        {
            self.report(event, scope, code, reason);
        }
    }

    /// O motivo é fixo no código; erros formatados e texto de conversa não entram por esta interface.
    pub fn report(&self, event: &'static str, session: &str, code: &str, reason: &'static str) {
        self.emit(event, session, code, reason, json!({}), true, true);
    }

    /// O chamador limita por episódio; a escalada e a recuperação não usam a janela global.
    pub(crate) fn report_episode(
        &self, event: &'static str, session: &str, code: &str,
        reason: &'static str, fields: serde_json::Value, warning: bool,
    ) {
        self.emit(event, session, code, reason, fields, warning, false);
    }

    fn emit(
        &self,
        event: &'static str,
        session: &str,
        code: &str,
        reason: &'static str,
        fields: serde_json::Value,
        plain: bool,
        limited: bool,
    ) {
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let directory =
            if !settings.explicit && DIARY_OFF.load(std::sync::atomic::Ordering::Relaxed) {
                None
            } else {
                settings.directory
            };
        let scope = format!("diag:{directory:?}:{event}:{code}");
        if limited && !crate::warn_limit::allow(Some(session), &scope) {
            return;
        }
        let Some(directory) = directory else {
            tracing::warn!(
                code = "diag_directory_unavailable",
                "diário local indisponível; linha não gravada"
            );
            return;
        };
        let warning = matches!(
            event,
            "rust.groups_sweep_failed" | "rust.groups_sweep_recovered"
        ) || (plain
            && matches!(event, "runtime.send_uncertain" | "runtime.command_deferred"));
        let reason_key = if plain { "detalhe" } else { "motivo" };
        let mut line = json!({"evento":wire_event(event), "nivel":if warning {"aviso"}else{"erro"},
            "sessao":session.chars().take(128).collect::<String>(), "codigo":wire_code(code)});
        line[reason_key] = json!(reason);
        if let (Some(line), serde_json::Value::Object(extra)) = (line.as_object_mut(), fields) {
            for (key, value) in extra {
                if ![
                    "evento",
                    "nivel",
                    "sessao",
                    "codigo",
                    "motivo",
                    "ts",
                    "backend",
                    "pid_backend",
                    "origem",
                    reason_key,
                ]
                .contains(&key.as_str())
                {
                    line.insert(key, value);
                }
            }
        }
        self.pending
            .active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        record_with_clock(
            Some(directory),
            vec![line],
            "diag",
            settings.clock,
            Some(PendingWrite(self.pending.clone())),
        );
    }
}

/// Os tetos por dia do `diag.py`: passado o primeiro só falha entra; no segundo, nada.
pub const DIARY_DAY_CAP: u64 = 4 << 20;
const DIARY_HARD_CAP: u64 = 8 << 20;

/// A pasta das linhas do Rust no diário exportável: `diario/servidor-rust`. O Python junta todo
/// `uso-*.jsonl` da árvore do diário (baixar, últimas linhas, poda dos 7 dias), e cada arquivo fica
/// com um escritor só: no Windows o append de dois processos no mesmo arquivo se sobrescreve.
pub fn diary_dir() -> Option<std::path::PathBuf> {
    if DIARY_OFF.load(std::sync::atomic::Ordering::Relaxed)
        || hangar_core::process::cargo_test_binary()
    {
        return None;
    }
    Some(
        crate::runtime::local_policy::log_base()?
            .join("diario")
            .join("servidor-rust"),
    )
}

/// Os testes de integração rodam o servidor no processo deles, fora do `cfg!(test)`: sem isto
/// gravariam no diário real de quem roda os testes. `diary_dir_in` fica de fora: segue o HOME do
/// ambiente da conta, que os testes já apontam para pasta temporária e leem.
static DIARY_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[doc(hidden)]
pub fn disable_diary() {
    DIARY_OFF.store(true, std::sync::atomic::Ordering::Relaxed)
}
/// `diary_dir` pelo ambiente-base do registro.
pub fn diary_dir_in(
    env: &crate::accounts::environment::AccountEnvironment,
) -> Option<std::path::PathBuf> {
    // Teste que montou o ambiente com o HOME real não grava no diário da máquina.
    if hangar_core::process::cargo_test_binary()
        && std::env::home_dir().is_some_and(|home| home == env.home)
    {
        return None;
    }
    Some(
        crate::runtime::local_policy::log_base_in(env)?
            .join("diario")
            .join("servidor-rust"),
    )
}

/// As linhas que o próprio Rust grava no diário exportável, no formato do `diag.registrar`: texto
/// cortado em 300, campo vazio fora, `origem` servidor. Bloqueante.
pub fn write_diary(
    dir: &std::path::Path,
    events: &[serde_json::Value],
    now: chrono::DateTime<chrono::Local>,
) -> std::io::Result<()> {
    use std::io::Write;
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("uso-{}.jsonl", now.format("%Y-%m-%d")));
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    if size >= DIARY_HARD_CAP {
        return Ok(());
    }
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
    let mut text = String::new();
    for fields in events.iter().filter_map(serde_json::Value::as_object) {
        if size >= DIARY_DAY_CAP
            && fields.get("nivel").and_then(serde_json::Value::as_str) == Some("ok")
        {
            continue;
        }
        let mut line = serde_json::Map::from_iter([("ts".to_owned(), json!(ts))]);
        for (key, value) in fields {
            match value {
                serde_json::Value::String(text) if !text.trim().is_empty() => {
                    line.insert(
                        key.clone(),
                        json!(text.trim().chars().take(300).collect::<String>()),
                    );
                }
                serde_json::Value::Number(_) => {
                    line.insert(key.clone(), value.clone());
                }
                _ => {}
            }
        }
        line.extend([
            (
                "backend".to_owned(),
                json!(concat!("hangar-server ", env!("CARGO_PKG_VERSION"))),
            ),
            ("pid_backend".to_owned(), json!(std::process::id())),
            ("origem".to_owned(), json!("servidor")),
        ]);
        text += &format!("{}\n", serde_json::Value::Object(line));
    }
    for (cap, what) in [(DIARY_DAY_CAP, "sucesso"), (DIARY_HARD_CAP, "tudo")] {
        if size < cap && cap <= size + text.len() as u64 {
            text += &format!(
                "{}\n",
                json!({"ts": ts, "evento": "diag.teto", "nivel": "aviso", "origem": "servidor", "etapa": what,
                "detalhe": format!("teto de {cap} bytes no dia")})
            );
        }
    }
    if text.is_empty() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(text.as_bytes())?;
    prune_diary(dir, now.date_naive());
    Ok(())
}

fn prune_diary(dir: &std::path::Path, today: chrono::NaiveDate) {
    let Some(cutoff) = today.checked_sub_signed(chrono::Duration::days(6)) else {
        return;
    };
    let failed = |error: std::io::Error| tracing::warn!(code = "diag_prune_failed", kind = ?error.kind(), "arquivo antigo do diário não foi removido");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            failed(error);
            return;
        }
    };
    let mut entries: Vec<_> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                failed(error);
                None
            }
        })
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        match entry.file_type() {
            Ok(kind) if kind.is_file() => {}
            Ok(_) => continue,
            Err(error) => {
                failed(error);
                continue;
            }
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(day) = name
            .strip_prefix("uso-")
            .and_then(|n| n.strip_suffix(".jsonl"))
        else {
            continue;
        };
        if day.len() != 10 {
            continue;
        }
        let Ok(day) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
            continue;
        };
        if day < cutoff {
            // Um dia protegido não impede a retenção dos outros arquivos.
            if let Err(error) = std::fs::remove_file(entry.path()) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    failed(error);
                }
            }
        }
    }
}

/// `write_diary` agora; falha vira aviso no log, nunca erro para quem chama. Bloqueante.
pub fn record_blocking(
    dir: Option<&std::path::Path>,
    events: &[serde_json::Value],
    what: &'static str,
) {
    record_at_blocking(dir, events, what, chrono::Local::now());
}

fn record_at_blocking(
    dir: Option<&std::path::Path>,
    events: &[serde_json::Value],
    what: &'static str,
    now: chrono::DateTime<chrono::Local>,
) {
    let Some(dir) = dir else {
        return tracing::warn!(
            code = "diag_directory_unavailable",
            what,
            "diário sem pasta de logs; linha não gravada"
        );
    };
    if let Err(error) = write_diary(dir, events, now) {
        tracing::warn!(code = "diag_write_failed", kind = ?error.kind(), what, "linha do diário não gravada");
    }
}

/// O escritor bloqueante fica fora das tarefas que atendem a conversa.
pub fn record(
    dir: Option<std::path::PathBuf>,
    events: Vec<serde_json::Value>,
    what: &'static str,
) -> tokio::task::JoinHandle<()> {
    record_with_clock(dir, events, what, None, None)
}

fn record_with_clock(
    dir: Option<std::path::PathBuf>,
    events: Vec<serde_json::Value>,
    what: &'static str,
    clock: Option<DiaryClock>,
    pending: Option<PendingWrite>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let _pending = pending;
        let now = clock.map_or_else(chrono::Local::now, |clock| clock());
        record_at_blocking(dir.as_deref(), &events, what, now)
    })
}

/// O produtor não Git do diagnóstico continua onde já existe; só esta publicação acrescenta a versão confirmada.
pub fn version_matches(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET && matches!(crate::workspace_http::decoded(path).as_ref(), "/api/diag" | "/api/diag/arquivo")
}

pub async fn version_route(st: std::sync::Arc<crate::routes::AppState>, peer: SocketAddr, req: axum::extract::Request) -> axum::response::Response {
    use http_body_util::BodyExt;
    let (forward, owner) = match crate::routes::gate(&st, peer, &req).await {
        Ok(value) => value, Err(error) => return crate::auth::unavailable(error),
    };
    let download = crate::workspace_http::decoded(req.uri().path()) == "/api/diag/arquivo";
    let requested = req.headers().clone();
    let response = crate::routes::pass(&st, req, &forward).await;
    if !owner || !response.status().is_success() { return response; }
    let (mut parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return crate::session_write::detail(axum::http::StatusCode::BAD_GATEWAY,
            "diag_version_body_unavailable", "não foi possível ler o diário", json!({})),
    };
    let base = crate::runtime::local_policy::log_base_in(&st.accounts.env);
    let compressed = parts.headers.get(axum::http::header::CONTENT_ENCODING).is_some_and(|value| value == "gzip");
    let mut headers = parts.headers.clone();
    let transformed = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let bytes = if compressed {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut decoded).map_err(|_| ())?;
            decoded
        } else { bytes.to_vec() };
        let mut versions = crate::boot_version::PythonDiary::load(base);
        let bind = |versions: &mut crate::boot_version::PythonDiary, value: &serde_json::Value| {
            if let Err(error) = versions.bind(value) { tracing::warn!(kind = ?error.kind(), "diário: identidade Python não persistida"); }
        };
        let result = if download {
            let text = String::from_utf8(bytes).map_err(|_| ())?;
            let mut output = String::with_capacity(text.len());
            for line in text.split_inclusive('\n') {
                if let Ok(value) = serde_json::from_str(line) { bind(&mut versions, &value); }
                output.push_str(&versions.enrich_line(line));
            }
            Ok(output.into_bytes())
        } else {
            let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
            versions.enrich_summary(text).map(String::into_bytes).map_err(|_| ())
        };
        let bytes = result?;
        let mut vary: Vec<String> = headers.get(axum::http::header::VARY).and_then(|value| value.to_str().ok()).unwrap_or("")
            .split(',').map(str::trim).filter(|value| !value.is_empty() && !(compressed && value.eq_ignore_ascii_case("Accept-Encoding"))).map(str::to_owned).collect();
        if compressed { headers.remove(axum::http::header::CONTENT_ENCODING); }
        // O mínimo vale para a representação final, não para o corpo antes da versão.
        let varies_by_encoding = bytes.len() >= 1024;
        let bytes = crate::routes::maybe_gzip(&requested, &mut headers, bytes);
        if varies_by_encoding && !vary.iter().any(|value| value.eq_ignore_ascii_case("Accept-Encoding")) { vary.push("Accept-Encoding".into()); }
        if vary.is_empty() { headers.remove(axum::http::header::VARY); }
        else { headers.insert(axum::http::header::VARY, vary.join(", ").parse().map_err(|_| ())?); }
        Ok::<_, ()>((bytes, headers))
    }).await;
    let (bytes, headers) = match transformed {
        Ok(Ok(value)) => value,
        _ => return crate::session_write::detail(axum::http::StatusCode::BAD_GATEWAY,
            "diag_version_body_unavailable", "não foi possível ler o diário", json!({})),
    };
    parts.headers = headers;
    parts.headers.remove(axum::http::header::TRANSFER_ENCODING);
    parts.headers.insert(axum::http::header::CONTENT_LENGTH, bytes.len().into());
    axum::http::Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os filtros existentes dependem destes nomes estáveis.
    #[test]
    fn wire_names_preserve_the_existing_diary_contract() {
        assert_eq!(
            wire_event("session.limit_check"),
            "rust.session_limit_check"
        );
        assert_eq!(
            wire_event("runtime.registration_failed"),
            "rust.runtime_registration_failed"
        );
        assert_eq!(
            wire_event("rust.state_deliver_failed"),
            "rust.state_deliver_failed"
        );
        assert_eq!(
            wire_event("runtime.send_failed"),
            "runtime.send_failed",
            "os que o Python já aceita ficam"
        );
        assert_eq!(
            wire_code("limit_switch_continue_lifecycle-busy.x"),
            "limit_switch_continue_lifecycle_busy_x"
        );
        assert_eq!(
            (wire_code(&"a".repeat(80)).len(), wire_code("").as_str()),
            (64, "sem_codigo")
        );
    }

    /// No formato do `diag.registrar` do Python, num arquivo do dia que o export dele junta
    /// (`uso-*.jsonl` em qualquer pasta do diário): texto cortado em 300, vazio fora, `origem` servidor.
    /// Passado o teto do dia só falha entra, e a linha do teto avisa.
    #[test]
    fn the_diary_lines_are_the_python_ones_in_a_file_of_the_day() {
        use chrono::TimeZone;
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 15, 9, 30, 0)
            .unwrap();
        let events = [
            json!({"evento": "sessao.criar_etapa", "nivel": "ok", "sessao": "s1", "etapa": "validar", "detalhe": ""}),
            json!({"evento": "sessao.criar.fim", "nivel": "erro", "ms": 12, "detalhe": "x".repeat(400)}),
        ];
        write_diary(dir.path(), &events, now).unwrap();
        let file = dir.path().join("uso-2026-10-15.jsonl");
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            (
                lines[0]["evento"].as_str(),
                lines[0]["origem"].as_str(),
                lines[0]["etapa"].as_str()
            ),
            (
                Some("sessao.criar_etapa"),
                Some("servidor"),
                Some("validar")
            )
        );
        assert!(lines[0].get("detalhe").is_none(), "vazio fica fora");
        assert_eq!(lines[1]["detalhe"].as_str().unwrap().chars().count(), 300);
        assert_eq!(lines[1]["ms"], 12);
        assert_eq!(
            lines[0]["ts"].as_str().unwrap(),
            now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
        );
        assert_eq!(lines[0]["pid_backend"], std::process::id());
        std::fs::write(&file, vec![b'\n'; DIARY_DAY_CAP as usize]).unwrap();
        write_diary(dir.path(), &events, now).unwrap();
        let tail: Vec<serde_json::Value> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            tail.iter()
                .map(|l| l["evento"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["sessao.criar.fim"],
            "acima do teto só a falha"
        );
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;
    use std::path::Path;
    pub fn rows(dir: &Path) -> Vec<Value> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .flat_map(|path| {
                let raw = std::fs::read_to_string(path).unwrap();
                let complete = raw.rsplit_once('\n').map_or("", |(complete, _)| complete);
                complete
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    pub async fn wait_for(dir: &Path, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        use notify::Watcher;
        let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let _ = tx.send(event);
            })
            .unwrap();
        watcher
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let found = rows(dir);
            if found.iter().any(&predicate) {
                return found;
            }
            tokio::time::timeout_at(deadline, changes.recv())
                .await
                .expect("o evento não chegou ao diário local")
                .expect("a observação encerrou")
                .expect("a observação falhou");
        }
    }
}
