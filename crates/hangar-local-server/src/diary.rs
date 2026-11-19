//! O diário de uso: o motivo da falha sai das linhas do servidor e do lançador
//! (`diario/servidor-rust/`); as ações daqui vão para `diario/servidor-local/`, um escritor por
//! arquivo (no Windows o append de dois processos no mesmo arquivo se sobrescreve).
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Uma linha do dia passa disso só em dia ruim; o motivo está no fim.
const TAIL_MAX: u64 = 1 << 20;
const DAYS_KEPT: i64 = 7;

/// Mesmo lugar do `log_paths.base()` do Python: o log é do Hangar da máquina, não da conta. No
/// Windows, `%LOCALAPPDATA%\hangar\logs` (sem a variável, a pasta padrão dela sob `home`); fora
/// dele, `~/.hangar/logs`. Sem `home` e sem a variável, nada. O `hangar_core::paths` usa esta.
pub fn log_base_from(home: Option<&Path>, local_appdata: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if cfg!(windows) {
        let root = local_appdata.filter(|dir| !dir.is_empty()).map(PathBuf::from)
            .or_else(|| home.map(|home| home.join("AppData").join("Local")))?;
        return Some(root.join("hangar").join("logs"));
    }
    Some(home?.join(".hangar").join("logs"))
}

/// `log_base_from` com o `LOCALAPPDATA` deste processo.
pub fn log_base(home: &Path) -> PathBuf {
    log_base_from(Some(home), std::env::var_os("LOCALAPPDATA").as_deref()).unwrap_or_else(|| home.join(".hangar").join("logs"))
}

/// Onde o servidor e o lançador gravam (`hangar_core::diag::diary_dir`).
pub fn server_dir(log_base: &Path) -> PathBuf { log_base.join("diario").join("servidor-rust") }
/// Onde as ações daqui gravam.
pub fn local_dir(log_base: &Path) -> PathBuf { log_base.join("diario").join("servidor-local") }

/// A última falha de subida no diário, se a vida mais recente terminou nela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiaryFailure {
    pub event: String,
    pub code: String,
    pub detail: String,
}

/// Do fim para o começo, no arquivo mais novo (e no anterior, se o mais novo não diz nada): a
/// primeira linha de vida decide. Subida ou parada limpa depois da falha a tornam velha.
pub fn last_failure(server_dir: &Path) -> Option<DiaryFailure> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(server_dir).ok()?.flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("uso-") && n.ends_with(".jsonl")))
        .collect();
    files.sort();
    for file in files.iter().rev().take(2) {
        for line in tail(file).lines().rev() {
            let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
            match decide(&event) {
                Decision::Failure(failure) => return Some(failure),
                Decision::Healthy => return None,
                Decision::Skip => {}
            }
        }
    }
    None
}

enum Decision {
    Failure(DiaryFailure),
    Healthy,
    Skip,
}

fn decide(event: &Value) -> Decision {
    let text = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let name = text("evento");
    let error = text("nivel") == "erro";
    let failure = |code: String, detail: String| Decision::Failure(DiaryFailure { event: name.clone(), code, detail });
    match name.as_str() {
        "backend.inicio" | "lancador.parada" => Decision::Healthy,
        "backend.encerrando" if error => failure(text("codigo").if_empty("server_failed"), text("causa")),
        "backend.encerrando" => Decision::Healthy,
        // `aviso`: o servidor subiu mesmo assim.
        "lancador.falhou" | "hangar_server.partida" if error => failure(text("codigo"), text("detalhe")),
        "lancador.queda" => failure(text("codigo").if_empty("server_exited"), text("saida")),
        _ => Decision::Skip,
    }
}

trait IfEmpty {
    fn if_empty(self, other: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, other: &str) -> String { if self.is_empty() { other.to_owned() } else { self } }
}

/// O fim do arquivo, no máximo `TAIL_MAX`, sem a primeira linha cortada.
fn tail(path: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(TAIL_MAX);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() { return String::new(); }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if start == 0 { text } else { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_owned()) }
}

/// Uma ação no diário: `{"evento": "local_server.acao", "acao": …, "nivel": ok|erro, …}`, no
/// formato do `write_diary` do servidor (texto cortado em 300, campo vazio fora).
pub fn record_action(log_base: &Path, action: &str, error: Option<&str>, fields: &[(&str, &str)]) -> std::io::Result<()> {
    let now = chrono::Local::now();
    let mut line = Map::new();
    line.insert("ts".into(), json!(now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)));
    line.insert("evento".into(), json!("local_server.acao"));
    line.insert("nivel".into(), json!(if error.is_some() { "erro" } else { "ok" }));
    line.insert("acao".into(), json!(action));
    for (key, value) in fields.iter().copied().chain(error.map(|code| ("codigo", code))) {
        let value = value.trim();
        if !value.is_empty() { line.insert(key.into(), json!(value.chars().take(300).collect::<String>())); }
    }
    line.insert("origem".into(), json!("servidor-local"));
    line.insert("pid".into(), json!(std::process::id()));
    let dir = local_dir(log_base);
    std::fs::create_dir_all(&dir)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("uso-{}.jsonl", now.format("%Y-%m-%d"))))?;
    file.write_all(format!("{}\n", Value::Object(line)).as_bytes())?;
    prune(&dir, now.date_naive());
    Ok(())
}

/// Sete dias, como o diário do servidor.
fn prune(dir: &Path, today: chrono::NaiveDate) {
    let Some(cutoff) = today.checked_sub_signed(chrono::Duration::days(DAYS_KEPT - 1)) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(day) = name.strip_prefix("uso-").and_then(|n| n.strip_suffix(".jsonl")) else { continue };
        if chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok_and(|day| day < cutoff) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
