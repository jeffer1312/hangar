//! Falhas do servidor vão ao diário local exportável, limitado por diário, sessão e código.
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Código da falha que uma rota do Rust devolveu, para o diário sem reler o corpo da resposta.
#[derive(Clone, Copy)]
pub struct FailureCode(pub &'static str);

pub fn coded(
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

    pub fn report_response(
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

    /// Uma linha já montada, como a `api.servidor` do acesso que o Python gravava, no destino deste
    /// cliente. Só campos escalares conhecidos: texto de conversa não entra por aqui.
    pub fn record_line(&self, line: serde_json::Value) {
        let settings = self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let directory = if !settings.explicit && DIARY_OFF.load(std::sync::atomic::Ordering::Relaxed) { None } else { settings.directory };
        let Some(directory) = directory else { return };
        self.pending.active.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        record_with_clock(Some(directory), vec![line], "acesso", settings.clock, Some(PendingWrite(self.pending.clone())));
    }

    /// O motivo é fixo no código; erros formatados e texto de conversa não entram por esta interface.
    pub fn report(&self, event: &'static str, session: &str, code: &str, reason: &'static str) {
        self.emit(event, session, code, reason, json!({}), true, true);
    }

    /// O chamador limita por episódio; a escalada e a recuperação não usam a janela global.
    pub fn report_episode(
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
        || crate::process::cargo_test_binary()
    {
        return None;
    }
    Some(
        crate::paths::log_base()?
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
    env: &crate::account_env::AccountEnvironment,
) -> Option<std::path::PathBuf> {
    let base = crate::paths::log_base_in(env)?;
    // Teste cujo ambiente aponta para os logs reais (HOME ou LOCALAPPDATA) não grava no diário da máquina.
    if crate::process::cargo_test_binary()
        && crate::paths::log_base().is_some_and(|real| real == base)
    {
        return None;
    }
    Some(base.join("diario").join("servidor-rust"))
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
