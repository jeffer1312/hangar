//! Por que a vida anterior do servidor saiu. Quem vai reiniciar o servidor grava antes a marca de
//! intenção (`~/.hangar/stop-intent.json`) com o pid de quem vai ser substituído; a subida seguinte
//! junta a marca, a parada gravada em `server-life.json` e, na queda do Linux, o resultado da unit
//! no journal, e grava no diário uma linha `hangar_server.saida`.
use std::path::{Path, PathBuf};
use std::time::Duration;

use hangar_api::status::ExitKind;
use hangar_boot::life::Previous;
use serde_json::{Map, Value, json};

pub const FILE: &str = "stop-intent.json";
/// A marca só explica uma saída que veio logo depois dela.
pub const VALID_FOR: Duration = Duration::from_secs(600);
#[cfg(target_os = "linux")]
const UNIT: &str = "hangar-backend.service";
#[cfg(target_os = "linux")]
const JOURNAL_WAIT: Duration = Duration::from_secs(3);

/// A marca de intenção lida na subida.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub kind: ExitKind,
    pub cause: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// O servidor que ia ser substituído.
    pub pid: u32,
    /// Segundos desde a época.
    pub ts: u64,
}

/// A marca gravada antes de um reinício: a pasta do Hangar e o pid marcado.
#[must_use = "o reinício que falha precisa apagar a marca"]
pub struct Marked(Option<(PathBuf, u32)>);

impl Marked {
    /// Sem marca: reinício que não para nada (instalação na mão).
    pub fn none() -> Self { Marked(None) }

    pub fn settle<T, E>(self, restarted: &Result<T, E>) {
        if restarted.is_err() { self.restart_failed(); }
    }

    /// Só apaga a marca com o servidor marcado ainda de pé: reinício recusado. Se ele já saiu (a unit
    /// parou o velho e a nova não subiu), a marca explica essa saída e fica; o prazo e o pid a limitam.
    pub fn restart_failed(self) {
        let Some((hangar_dir, pid)) = self.0 else { return };
        if !matches!(hangar_boot::life::running_pid(&hangar_dir), Ok(Some(running)) if running == pid) { return; }
        if let Err(error) = std::fs::remove_file(hangar_dir.join(FILE)) && error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(code = "stop_intent_not_removed", kind = ?error.kind(), "marca de intenção do reinício que falhou não apagada; ela vence em 10 min");
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

pub fn kind_code(kind: ExitKind) -> &'static str {
    match kind {
        ExitKind::Crash => "crash",
        ExitKind::Update => "update",
        ExitKind::Requested => "requested",
    }
}

fn kind_of(code: &str) -> Option<ExitKind> {
    match code {
        "crash" => Some(ExitKind::Crash),
        "update" => Some(ExitKind::Update),
        "requested" => Some(ExitKind::Requested),
        _ => None,
    }
}

/// Grava a marca imediatamente antes de `restarter.restart()`. O pid é o do servidor de pé segundo
/// `server-life.json`, não o deste processo: o executor da atualização roda à parte. Sem servidor
/// de pé ou sem gravar, o reinício segue e a saída vira a parada que ele mesmo gravar.
pub fn before_restart(hangar_dir: &Path, kind: ExitKind, cause: &str, from: Option<&str>, to: Option<&str>) -> Marked {
    let pid = match hangar_boot::life::running_pid(hangar_dir) {
        Ok(Some(pid)) => pid,
        Ok(None) => {
            tracing::warn!(code = "stop_intent_without_server", cause, "nenhum servidor de pé em server-life.json; o reinício segue sem marca de intenção");
            return Marked(None);
        }
        Err(error) => {
            tracing::warn!(code = "stop_intent_life_unreadable", kind = ?error.kind(), cause, "server-life.json ilegível; o reinício segue sem marca de intenção");
            return Marked(None);
        }
    };
    let mut intent = Map::new();
    intent.insert("tipo".into(), json!(kind_code(kind)));
    intent.insert("causa".into(), json!(cause));
    if let Some(from) = from.filter(|from| !from.is_empty()) { intent.insert("de".into(), json!(from)); }
    if let Some(to) = to.filter(|to| !to.is_empty()) { intent.insert("para".into(), json!(to)); }
    intent.insert("pid".into(), json!(pid));
    intent.insert("ts".into(), json!(now_secs()));
    let written = std::fs::create_dir_all(hangar_dir)
        .and_then(|()| hangar_core::fs::atomic_write(&hangar_dir.join(FILE), Value::Object(intent).to_string().as_bytes()));
    match written {
        Ok(()) => Marked(Some((hangar_dir.to_path_buf(), pid))),
        Err(error) => {
            tracing::warn!(code = "stop_intent_unwritable", kind = ?error.kind(), cause, "marca de intenção não gravada; o reinício segue sem ela");
            Marked(None)
        }
    }
}

/// Lê e apaga a marca: cada subida a usa uma vez, e uma que não vale para esta vida não valerá depois.
pub fn take(hangar_dir: &Path) -> Result<Option<Intent>, std::io::ErrorKind> {
    let path = hangar_dir.join(FILE);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.kind()),
    };
    if let Err(error) = std::fs::remove_file(&path) && error.kind() != std::io::ErrorKind::NotFound {
        return Err(error.kind());
    }
    parse(&raw).map(Some).ok_or(std::io::ErrorKind::InvalidData)
}

fn parse(raw: &[u8]) -> Option<Intent> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Intent {
        kind: kind_of(value.get("tipo")?.as_str()?)?,
        cause: text("causa")?,
        from: text("de"),
        to: text("para"),
        pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
        ts: value.get("ts")?.as_u64()?,
    })
}

/// Como a vida anterior saiu: o C2 sem o `erro`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitRecord {
    pub kind: ExitKind,
    pub cause: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl ExitRecord {
    /// A linha do diário. Só campos texto: o gravador descarta lista e booleano.
    pub fn line(&self, erro: Option<&str>) -> Value {
        let mut line = Map::new();
        line.insert("evento".into(), json!("hangar_server.saida"));
        line.insert("nivel".into(), json!("aviso"));
        line.insert("tipo".into(), json!(kind_code(self.kind)));
        for (key, value) in [("causa", self.cause.as_deref()), ("de", self.from.as_deref()), ("para", self.to.as_deref()), ("erro", erro)] {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) { line.insert(key.into(), json!(value)); }
        }
        Value::Object(line)
    }
}

/// A marca vale só para o pid da vida anterior e por `VALID_FOR`; ela vence a parada gravada, porque
/// um reinício que o systemd teve de matar pelo prazo ainda é o reinício pedido. Sem marca: parada
/// gravada sem falha é pedida (causa = o sinal); o resto é queda, com a causa do journal quando há.
pub fn classify(previous: &Previous, intent: Option<&Intent>, journal: Option<&str>, now: u64) -> ExitRecord {
    let valid = intent.filter(|intent| previous.pid == Some(intent.pid) && intent.ts.abs_diff(now) < VALID_FOR.as_secs());
    if let Some(intent) = valid {
        return ExitRecord { kind: intent.kind, cause: Some(intent.cause.clone()), from: intent.from.clone(), to: intent.to.clone() };
    }
    match &previous.stop {
        Some(stop) if !stop.failed => ExitRecord { kind: ExitKind::Requested, cause: stop.cause.clone(), from: None, to: None },
        stop => ExitRecord {
            kind: ExitKind::Crash,
            cause: journal.map(journal_cause).or_else(|| stop.as_ref().and_then(|stop| stop.cause.clone())),
            from: None,
            to: None,
        },
    }
}

/// O `Result` da unit como código: `oom-kill` é falta de memória; o resto, com `_` no lugar de `-`.
pub fn journal_cause(result: &str) -> String {
    match result {
        "oom-kill" => "out_of_memory".into(),
        other => other.chars().take(40).map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect(),
    }
}

/// O resultado da última falha da unit desde `since` (ISO de quando a vida ficou pronta).
pub trait Journal {
    fn failed_result(&self, since: Option<&str>) -> Option<String>;
}

/// `journalctl --user -u hangar-backend.service`: o `Result` da unit já foi zerado quando o serviço
/// sobe de novo, e só a linha "Failed with result" guarda a causa. Fora do Linux, nada.
pub struct SystemdJournal;

impl Journal for SystemdJournal {
    #[cfg(target_os = "linux")]
    fn failed_result(&self, since: Option<&str>) -> Option<String> {
        use std::io::Read as _;
        // Sem o começo da vida, a última linha pode ser de uma queda antiga.
        let since = chrono::DateTime::parse_from_rfc3339(since?).ok()?.timestamp();
        let mut child = std::process::Command::new("journalctl")
            .args(["--user", "-u", UNIT, "--no-pager", "-q", "-o", "cat", &format!("--since=@{since}")])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
            .spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut raw = Vec::new();
            let _ = stdout.read_to_end(&mut raw);
            let _ = sender.send(raw);
        });
        match receiver.recv_timeout(JOURNAL_WAIT) {
            Ok(raw) => {
                let _ = child.wait();
                last_failed_result(&String::from_utf8_lossy(&raw))
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                None
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn failed_result(&self, _since: Option<&str>) -> Option<String> {
        None
    }
}

/// `hangar-backend.service: Failed with result 'oom-kill'.` → `oom-kill`, da última linha assim.
pub fn last_failed_result(journal: &str) -> Option<String> {
    journal.lines().rev().find_map(|line| {
        let rest = &line[line.find("Failed with result '")? + "Failed with result '".len()..];
        Some(rest[..rest.find('\'')?].to_owned()).filter(|result| !result.is_empty())
    })
}

/// As linhas da subida sobre a vida anterior: o C2 e, com a marca ilegível, o aviso dela. Consome a
/// marca. `errors`: as últimas linhas de erro do log, lidas só na queda.
pub fn previous_exit(hangar_dir: &Path, previous: Option<&Previous>, errors: impl FnOnce() -> String, journal: &dyn Journal, now: u64) -> Vec<Value> {
    let mut lines = Vec::new();
    let intent = match take(hangar_dir) {
        Ok(intent) => intent,
        Err(kind) => {
            lines.push(json!({"evento": "hangar_server.intencao_ilegivel", "nivel": "aviso", "codigo": "stop_intent_unreadable", "tipo": format!("{kind:?}")}));
            None
        }
    };
    let Some(previous) = previous else { return lines };
    let mut record = classify(previous, intent.as_ref(), None, now);
    if record.kind == ExitKind::Crash {
        record = classify(previous, intent.as_ref(), journal.failed_result(previous.started.as_deref()).as_deref(), now);
    }
    let erro = (record.kind == ExitKind::Crash).then(errors);
    lines.push(record.line(erro.as_deref()));
    lines
}

/// Subida: a vida anterior vira a linha `hangar_server.saida`, com o journal desta máquina.
pub fn record_previous(hangar_dir: &Path, previous: Option<&Previous>, errors: impl FnOnce() -> String) -> Vec<Value> {
    previous_exit(hangar_dir, previous, errors, &SystemdJournal, now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_boot::life::Stop;

    const NOW: u64 = 1_800_000_000;

    fn life(pid: u32, unclean: bool, stop: Option<(&str, bool)>) -> Previous {
        Previous { pid: Some(pid), started: Some("2026-11-17T10:00:00-03:00".into()), unclean,
            stop: stop.map(|(cause, failed)| Stop { cause: Some(cause.into()), failed }) }
    }

    fn intent(kind: ExitKind, cause: &str, pid: u32, ts: u64) -> Intent {
        Intent { kind, cause: cause.into(), from: Some("0.1.0.9000".into()), to: Some("0.1.0.9010".into()), pid, ts }
    }

    #[test]
    fn exit_update_comes_from_the_intent() {
        let record = classify(&life(41, false, Some(("sigterm", false))), Some(&intent(ExitKind::Update, "package", 41, NOW - 30)), None, NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Update, cause: Some("package".into()), from: Some("0.1.0.9000".into()), to: Some("0.1.0.9010".into()) });
        // Reinício que o systemd teve de matar pelo prazo continua sendo o pedido.
        assert_eq!(classify(&life(41, true, None), Some(&intent(ExitKind::Update, "package", 41, NOW)), Some("timeout"), NOW).kind, ExitKind::Update);
    }

    #[test]
    fn exit_intent_of_another_pid_is_ignored() {
        let record = classify(&life(41, false, Some(("sigterm", false))), Some(&intent(ExitKind::Update, "package", 40, NOW)), None, NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Requested, cause: Some("sigterm".into()), from: None, to: None });
    }

    #[test]
    fn exit_expired_intent_is_ignored() {
        let old = intent(ExitKind::Requested, "restart_only", 41, NOW - VALID_FOR.as_secs());
        assert_eq!(classify(&life(41, true, None), Some(&old), None, NOW).kind, ExitKind::Crash);
        let fresh = intent(ExitKind::Requested, "restart_only", 41, NOW - VALID_FOR.as_secs() + 1);
        assert_eq!(classify(&life(41, true, None), Some(&fresh), None, NOW).kind, ExitKind::Requested);
    }

    #[test]
    fn exit_requested_stop_keeps_the_signal() {
        let record = classify(&life(41, false, Some(("sigint", false))), None, Some("signal"), NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Requested, cause: Some("sigint".into()), from: None, to: None });
        // Parada do lançador sem causa conhecida.
        let launcher = Previous { stop: Some(Stop { cause: None, failed: false }), ..life(41, false, None) };
        assert_eq!(classify(&launcher, None, None, NOW), ExitRecord { kind: ExitKind::Requested, cause: None, from: None, to: None });
    }

    #[test]
    fn exit_serve_failed_is_a_crash() {
        let failed = life(41, false, Some(("serve_failed", true)));
        assert_eq!(classify(&failed, None, None, NOW), ExitRecord { kind: ExitKind::Crash, cause: Some("serve_failed".into()), from: None, to: None });
        assert_eq!(classify(&failed, None, Some("exit-code"), NOW).cause.as_deref(), Some("exit_code"));
    }

    #[test]
    fn exit_oom_kill_is_out_of_memory() {
        let record = classify(&life(41, true, None), None, Some("oom-kill"), NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Crash, cause: Some("out_of_memory".into()), from: None, to: None });
        assert_eq!(classify(&life(41, true, None), None, None, NOW).cause, None, "Windows e macOS: sem causa");
    }

    #[test]
    fn exit_journal_result_comes_from_the_last_failure() {
        let journal = "Started hangar-backend.service.\n\
            hangar-backend.service: Main process exited, code=killed, status=9/KILL\n\
            hangar-backend.service: Failed with result 'signal'.\n\
            hangar-backend.service: A process of this unit has been killed by the OOM killer.\n\
            hangar-backend.service: Failed with result 'oom-kill'.\n\
            hangar-backend.service: Scheduled restart job, restart counter is at 3.\n";
        assert_eq!(last_failed_result(journal).as_deref(), Some("oom-kill"));
        assert_eq!(last_failed_result("Started hangar-backend.service.\n"), None);
        assert_eq!(journal_cause("core-dump"), "core_dump");
    }

    #[test]
    fn exit_line_has_only_text_fields() {
        let record = ExitRecord { kind: ExitKind::Update, cause: Some("package".into()), from: Some("a".into()), to: None };
        assert_eq!(record.line(Some(" ")), json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package", "de": "a"}));
        let crash = ExitRecord { kind: ExitKind::Crash, cause: None, from: None, to: None };
        assert_eq!(crash.line(Some("10:55:02 panic=true")), json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "crash", "erro": "10:55:02 panic=true"}));
    }

    struct Fixed(Option<&'static str>);
    impl Journal for Fixed {
        fn failed_result(&self, since: Option<&str>) -> Option<String> {
            assert_eq!(since, Some("2026-11-17T10:00:00-03:00"), "o journal é lido desde o começo da vida anterior");
            self.0.map(str::to_owned)
        }
    }

    /// Subida depois de um reinício pedido: a marca gravada antes do reinício explica a saída e é consumida.
    #[test]
    fn stop_intent_written_before_the_restart_explains_the_next_boot() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        hangar_boot::life::begin(hangar, "2026-11-17T10:00:00-03:00", |_, _| false).unwrap();
        hangar_boot::life::mark_running(hangar, 41, Some(41.0), "2026-11-17T10:00:00-03:00", |_, _| false).unwrap();
        let marked = before_restart(hangar, ExitKind::Requested, "restart_only", None, None);
        let written: Value = serde_json::from_slice(&std::fs::read(hangar.join(FILE)).unwrap()).unwrap();
        assert_eq!((written["tipo"].as_str(), written["causa"].as_str(), written["pid"].as_u64()), (Some("requested"), Some("restart_only"), Some(41)));
        assert!(written.get("de").is_none());
        marked.settle(&Ok::<(), ()>(()));
        hangar_boot::life::end_with(hangar, 41, Some("sigterm"), false).unwrap();
        let previous = hangar_boot::life::begin(hangar, "2026-11-17T10:01:00-03:00", |_, _| false).unwrap().previous;
        let lines = previous_exit(hangar, previous.as_ref(), || panic!("erro só na queda"), &Fixed(None), now_secs());
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "requested", "causa": "restart_only"})]);
        assert!(!hangar.join(FILE).exists(), "a subida consome a marca");
    }

    #[test]
    fn stop_intent_is_removed_when_the_restart_fails() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        hangar_boot::life::mark_running(hangar, 41, Some(41.0), "agora", |_, _| false).unwrap();
        before_restart(hangar, ExitKind::Update, "package", Some("1"), Some("2")).settle(&Err::<(), _>("recusado"));
        assert!(!hangar.join(FILE).exists(), "reinício recusado com o servidor ainda de pé");
        // Sem servidor de pé, nada é gravado.
        let empty = tempfile::tempdir().unwrap();
        before_restart(empty.path(), ExitKind::Update, "package", None, None).restart_failed();
        assert!(!empty.path().join(FILE).exists());
    }

    /// A unit parou o servidor velho e a nova não subiu: o reinício falha, mas a marca fica e a subida
    /// seguinte (a da volta) registra a atualização, não um pedido.
    #[test]
    fn stop_intent_stays_when_the_restart_fails_after_the_server_left() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        hangar_boot::life::begin(hangar, "2026-11-17T10:00:00-03:00", |_, _| false).unwrap();
        hangar_boot::life::mark_running(hangar, 41, Some(41.0), "2026-11-17T10:00:00-03:00", |_, _| false).unwrap();
        let marked = before_restart(hangar, ExitKind::Update, "package", Some("1"), Some("2"));
        hangar_boot::life::end_with(hangar, 41, Some("sigterm"), false).unwrap();
        marked.settle(&Err::<(), _>("a unit nova não subiu"));
        assert!(hangar.join(FILE).exists());
        let previous = hangar_boot::life::begin(hangar, "2026-11-17T10:02:00-03:00", |_, _| false).unwrap().previous;
        let lines = previous_exit(hangar, previous.as_ref(), String::new, &Fixed(None), now_secs());
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package", "de": "1", "para": "2"})]);
    }

    /// Queda sem marca: a causa vem do journal e o erro, das últimas linhas do log.
    #[test]
    fn stop_intent_absent_crash_reads_the_journal_and_the_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "{torto").unwrap();
        let previous = life(41, true, None);
        let lines = previous_exit(dir.path(), Some(&previous), || "10:55:02 panic=true".into(), &Fixed(Some("oom-kill")), NOW);
        assert_eq!(lines[0]["codigo"], "stop_intent_unreadable");
        assert_eq!(lines[1], json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "crash", "causa": "out_of_memory", "erro": "10:55:02 panic=true"}));
        assert!(!dir.path().join(FILE).exists(), "marca ilegível também sai");
    }

    /// Todo reinício do servidor no atualizador grava a marca antes: ponto novo esquecido falha aqui.
    #[test]
    fn stop_intent_precedes_every_restart() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("update");
        let test_module = regex::Regex::new(r"#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\(crate\))? )?mod \w+ \{").unwrap();
        let mut found = 0;
        let mut missing = Vec::new();
        let mut pending = vec![root];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { pending.push(path); continue; }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !name.ends_with(".rs") || name == "tests.rs" || name == "stop_intent.rs" { continue; }
                let text = std::fs::read_to_string(&path).unwrap();
                let code = test_module.find(&text).map_or(text.as_str(), |cut| &text[..cut.start()]);
                let lines: Vec<&str> = code.lines().collect();
                for (index, line) in lines.iter().enumerate() {
                    let restarts = line.contains("restarter.restart(") || line.contains("service.stop(")
                        || (line.contains(".restart_service(") && !line.contains("fn restart_service"));
                    if !restarts { continue; }
                    found += 1;
                    if !lines[index.saturating_sub(12)..=index].iter().any(|before| before.contains("before_restart(")) {
                        missing.push(format!("{}:{}", path.display(), index + 1));
                    }
                }
            }
        }
        assert!(found >= 10, "a busca achou só {found} reinícios: o padrão mudou?");
        assert!(missing.is_empty(), "reinício sem stop_intent::before_restart antes: {missing:?}");
    }
}
