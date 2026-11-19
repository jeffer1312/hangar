//! Por que a vida anterior do servidor saiu. Quem vai reiniciar o servidor grava antes a marca de
//! intenção (`~/.hangar/stop-intent.json`) com o pid de quem vai ser substituído; a subida seguinte
//! junta a marca, a parada gravada em `server-life.json` e, na queda do Linux, o resultado da unit
//! no journal, e grava no diário uma linha `hangar_server.saida`.
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use hangar_api::status::ExitKind;
use hangar_boot::life::Previous;

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
    /// "De pé" é o processo vivo com o mesmo nascimento: morto sem `end`, o arquivo ainda diz `running`.
    pub fn restart_failed(self) {
        let Some((hangar_dir, pid)) = self.0 else { return };
        if !matches!(hangar_boot::life::running_pid(&hangar_dir, same_process), Ok(Some(running)) if running == pid) { return; }
        if let Err(error) = std::fs::remove_file(hangar_dir.join(FILE)) && error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(code = "stop_intent_not_removed", kind = ?error.kind(), "marca de intenção do reinício que falhou não apagada; ela vence em 10 min");
        }
    }
}

/// Por que o servidor vai reiniciar. Quem reinicia de fato grava a marca com ela antes de derrubar o
/// processo; quem não derruba nada (instalação na mão) não grava.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartIntent {
    pub kind: ExitKind,
    pub cause: &'static str,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl RestartIntent {
    pub fn new(kind: ExitKind, cause: &'static str) -> Self { RestartIntent { kind, cause, from: None, to: None } }

    pub fn between(mut self, from: Option<&str>, to: Option<&str>) -> Self {
        self.from = from.map(str::to_owned);
        self.to = to.map(str::to_owned);
        self
    }

    pub fn mark(&self, hangar_dir: &Path) -> Marked {
        before_restart(hangar_dir, self.kind, self.cause, self.from.as_deref(), self.to.as_deref())
    }
}

/// O processo da marca de pé ainda é o mesmo: pid e nascimento iguais. Diferente do `life_alive` da
/// subida, este processo conta: quem reinicia pode ser o próprio servidor.
fn same_process(pid: u32, birth: f64) -> bool {
    crate::terminal_facts::agent_birth(pid).is_some_and(|now| crate::terminal_facts::same_birth(now, birth))
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

/// Grava a marca imediatamente antes de derrubar o servidor. O pid é o do servidor de pé segundo
/// `server-life.json`, não o deste processo: o executor da atualização roda à parte. Sem servidor
/// de pé ou sem gravar, o reinício segue e a saída vira a parada que ele mesmo gravar. Servidor que
/// já morreu sem `end` também não ganha marca: ela esconderia a queda dele.
pub fn before_restart(hangar_dir: &Path, kind: ExitKind, cause: &str, from: Option<&str>, to: Option<&str>) -> Marked {
    let pid = match hangar_boot::life::running_pid(hangar_dir, same_process) {
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
    /// Quando ela saiu (RFC 3339 UTC). O `ts` da linha é o da subida seguinte, que pode vir dias depois.
    pub when: Option<String>,
}

impl ExitRecord {
    fn from_intent(intent: &Intent, when: Option<String>) -> Self {
        ExitRecord { kind: intent.kind, cause: Some(intent.cause.clone()), from: intent.from.clone(), to: intent.to.clone(), when }
    }

    /// A linha do diário. Só campos texto: o gravador descarta lista e booleano.
    pub fn line(&self, erro: Option<&str>) -> Value {
        let mut line = Map::new();
        line.insert("evento".into(), json!("hangar_server.saida"));
        line.insert("nivel".into(), json!("aviso"));
        line.insert("tipo".into(), json!(kind_code(self.kind)));
        let fields = [("causa", self.cause.as_deref()), ("de", self.from.as_deref()), ("para", self.to.as_deref()),
            ("quando", self.when.as_deref()), ("erro", erro)];
        for (key, value) in fields {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) { line.insert(key.into(), json!(value)); }
        }
        Value::Object(line)
    }
}

/// A marca vale só para o pid da vida anterior e por `VALID_FOR`.
fn valid_for(intent: &Intent, pid: Option<u32>, now: u64) -> bool {
    pid == Some(intent.pid) && intent.ts.abs_diff(now) < VALID_FOR.as_secs()
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn utc_secs(secs: u64) -> Option<String> {
    DateTime::from_timestamp(i64::try_from(secs).ok()?, 0).map(stamp)
}

fn utc_text(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text).ok().map(|at| stamp(at.with_timezone(&Utc)))
}

/// A marca vence a parada gravada, porque um reinício que o systemd teve de matar pelo prazo ainda é
/// o reinício pedido. Sem marca: parada gravada sem falha é pedida (causa = o sinal); o resto é
/// queda, com a causa do journal quando há. O instante é o da parada gravada; sem ela, o da marca;
/// sem as duas (queda), o começo da vida, o único que o `server-life.json` guarda dela.
pub fn classify(previous: &Previous, intent: Option<&Intent>, journal: Option<&str>, now: u64) -> ExitRecord {
    // O prazo mede a marca contra a saída, não contra a subida: o servidor pode voltar horas depois.
    let stopped_at = previous.stop.as_ref().and_then(|stop| stop.at).unwrap_or(now);
    let valid = intent.filter(|intent| valid_for(intent, previous.pid, stopped_at));
    let when = previous.stop.as_ref().and_then(|stop| stop.at).and_then(utc_secs)
        .or_else(|| valid.and_then(|intent| utc_secs(intent.ts)))
        .or_else(|| previous.started.as_deref().and_then(utc_text));
    if let Some(intent) = valid {
        return ExitRecord::from_intent(intent, when);
    }
    match &previous.stop {
        Some(stop) if !stop.failed => ExitRecord { kind: ExitKind::Requested, cause: stop.cause.clone(), from: None, to: None, when },
        stop => ExitRecord {
            kind: ExitKind::Crash,
            cause: journal.map(journal_cause).or_else(|| stop.as_ref().and_then(|stop| stop.cause.clone())),
            from: None,
            to: None,
            when,
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
/// marca. `pid`: o da vida descrita no `server-life.json`, que confere a marca também sem `previous`.
/// `errors`: as últimas linhas de erro do log, lidas só na queda.
pub fn previous_exit(hangar_dir: &Path, previous: Option<&Previous>, pid: Option<u32>, errors: impl FnOnce() -> String, journal: &dyn Journal,
    now: u64, update: Option<&UpdateRun>) -> Vec<Value> {
    let mut lines = Vec::new();
    let intent = match take(hangar_dir) {
        Ok(intent) => intent,
        Err(kind) => {
            lines.push(json!({"evento": "hangar_server.intencao_ilegivel", "nivel": "aviso", "codigo": "stop_intent_unreadable", "tipo": format!("{kind:?}")}));
            None
        }
    };
    let life_pid = previous.map_or(pid, |previous| previous.pid);
    let stopped_at = previous.and_then(|previous| previous.stop.as_ref()).and_then(|stop| stop.at).unwrap_or(now);
    // Quem parou a vida anterior pode ser uma versão de antes da marca: o executor dela, no meio do
    // reinício para esta versão, explica a saída que a marca não explica.
    let intent = intent.filter(|intent| valid_for(intent, life_pid, stopped_at))
        .or_else(|| Some(update?.intent(life_pid?, stopped_at)));
    let Some(previous) = previous else {
        // A vida anterior parou sem gravar a causa (versão de antes da marca, na primeira atualização
        // para esta): a marca dela ainda explica a saída.
        if let Some(intent) = intent.as_ref() {
            lines.push(ExitRecord::from_intent(intent, utc_secs(intent.ts)).line(None));
        }
        return lines;
    };
    let mut record = classify(previous, intent.as_ref(), None, now);
    if record.kind == ExitKind::Crash {
        record = classify(previous, intent.as_ref(), journal.failed_result(previous.started.as_deref()).as_deref(), now);
    }
    let erro = (record.kind == ExitKind::Crash).then(errors);
    lines.push(record.line(erro.as_deref()));
    lines
}

/// Marca este processo de teste como o servidor de pé: a marca de intenção confere pid e nascimento
/// de um processo vivo. Devolve o pid.
#[cfg(test)]
pub(crate) fn mark_this_process_running(hangar_dir: &Path, now: &str) -> u32 {
    let pid = std::process::id();
    hangar_boot::life::mark_running(hangar_dir, pid, crate::terminal_facts::agent_birth(pid), None, now, |_, _| false).unwrap();
    pid
}

/// Subida: a vida anterior vira a linha `hangar_server.saida`, com o journal desta máquina.
/// `update_state`: a pasta do estado da atualização (`<claude>/.hangar-update`).
pub fn record_previous(hangar_dir: &Path, update_state: &Path, previous: Option<&Previous>, pid: Option<u32>,
    errors: impl FnOnce() -> String) -> Vec<Value> {
    let now = now_secs();
    let update = crate::update::running_package().and_then(|package| update_run(update_state, &package.version, now));
    previous_exit(hangar_dir, previous, pid, errors, &SystemdJournal, now, update.as_ref())
}

/// Uma atualização no passo de reiniciar, indo para a versão que acaba de subir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRun {
    pub from: Option<String>,
    pub to: String,
}

impl UpdateRun {
    fn intent(&self, pid: u32, ts: u64) -> Intent {
        Intent { kind: ExitKind::Update, cause: "package".into(), from: self.from.clone(), to: Some(self.to.clone()), pid, ts }
    }
}

/// O `estado.json` do executor: rodando, no passo `reiniciar` começado há menos de `VALID_FOR` e com
/// `versao_para` = `version`. Todo executor grava esses campos, também o das versões de antes da marca
/// de intenção. O prazo descarta o estado que um executor morto deixou: uma queda depois não é ele.
pub fn update_run(update_state: &Path, version: &str, now: u64) -> Option<UpdateRun> {
    let state: Value = serde_json::from_slice(&std::fs::read(update_state.join("estado.json")).ok()?).ok()?;
    let text = |key: &str| state.get(key).and_then(Value::as_str);
    let started = u64::try_from(DateTime::parse_from_rfc3339(text("etapa_inicio")?).ok()?.timestamp()).ok()?;
    (text("fase")? == "rodando" && text("etapa")? == "reiniciar" && text("versao_para")? == version
        && started.abs_diff(now) < VALID_FOR.as_secs())
        .then(|| UpdateRun { from: text("versao_de").map(str::to_owned), to: version.to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_boot::life::Stop;

    const NOW: u64 = 1_800_000_000;
    /// O `started` de `life` em UTC.
    const STARTED: &str = "2026-11-17T13:00:00Z";

    fn life(pid: u32, unclean: bool, stop: Option<(&str, bool)>) -> Previous {
        Previous { pid: Some(pid), started: Some("2026-11-17T10:00:00-03:00".into()), unclean,
            stop: stop.map(|(cause, failed)| Stop { cause: Some(cause.into()), failed, at: None }) }
    }

    fn intent(kind: ExitKind, cause: &str, pid: u32, ts: u64) -> Intent {
        Intent { kind, cause: cause.into(), from: Some("0.1.0.9000".into()), to: Some("0.1.0.9010".into()), pid, ts }
    }

    fn started() -> Option<String> {
        Some(STARTED.into())
    }

    #[test]
    fn exit_update_comes_from_the_intent() {
        let record = classify(&life(41, false, Some(("sigterm", false))), Some(&intent(ExitKind::Update, "package", 41, NOW - 30)), None, NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Update, cause: Some("package".into()), from: Some("0.1.0.9000".into()),
            to: Some("0.1.0.9010".into()), when: utc_secs(NOW - 30) });
        // Reinício que o systemd teve de matar pelo prazo continua sendo o pedido.
        assert_eq!(classify(&life(41, true, None), Some(&intent(ExitKind::Update, "package", 41, NOW)), Some("timeout"), NOW).kind, ExitKind::Update);
    }

    #[test]
    fn exit_intent_of_another_pid_is_ignored() {
        let record = classify(&life(41, false, Some(("sigterm", false))), Some(&intent(ExitKind::Update, "package", 40, NOW)), None, NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Requested, cause: Some("sigterm".into()), from: None, to: None, when: started() });
    }

    /// O instante é o da parada gravada; sem ela, o da marca; sem as duas, o começo da vida.
    #[test]
    fn exit_when_prefers_the_recorded_stop() {
        let mut stopped = life(41, false, Some(("sigterm", false)));
        stopped.stop.as_mut().unwrap().at = Some(NOW - 5);
        assert_eq!(classify(&stopped, Some(&intent(ExitKind::Update, "package", 41, NOW - 30)), None, NOW).when, utc_secs(NOW - 5));
        assert_eq!(classify(&stopped, None, None, NOW).when, utc_secs(NOW - 5));
        assert_eq!(classify(&life(41, true, None), Some(&intent(ExitKind::Update, "package", 41, NOW - 30)), None, NOW).when, utc_secs(NOW - 30));
        assert_eq!(classify(&life(41, true, None), None, None, NOW).when, started(), "queda sem parada gravada: o começo da vida");
        assert_eq!(classify(&Previous { started: None, ..life(41, true, None) }, None, None, NOW).when, None);
        assert_eq!(utc_secs(1_800_000_000).as_deref(), Some("2027-01-15T08:00:00Z"));
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
        assert_eq!(record, ExitRecord { kind: ExitKind::Requested, cause: Some("sigint".into()), from: None, to: None, when: started() });
        // Parada do lançador sem causa conhecida.
        let launcher = Previous { stop: Some(Stop { cause: None, failed: false, at: Some(NOW - 1) }), ..life(41, false, None) };
        assert_eq!(classify(&launcher, None, None, NOW), ExitRecord { kind: ExitKind::Requested, cause: None, from: None, to: None, when: utc_secs(NOW - 1) });
    }

    #[test]
    fn exit_serve_failed_is_a_crash() {
        let failed = life(41, false, Some(("serve_failed", true)));
        assert_eq!(classify(&failed, None, None, NOW), ExitRecord { kind: ExitKind::Crash, cause: Some("serve_failed".into()), from: None, to: None, when: started() });
        assert_eq!(classify(&failed, None, Some("exit-code"), NOW).cause.as_deref(), Some("exit_code"));
    }

    #[test]
    fn exit_oom_kill_is_out_of_memory() {
        let record = classify(&life(41, true, None), None, Some("oom-kill"), NOW);
        assert_eq!(record, ExitRecord { kind: ExitKind::Crash, cause: Some("out_of_memory".into()), from: None, to: None, when: started() });
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
        let record = ExitRecord { kind: ExitKind::Update, cause: Some("package".into()), from: Some("a".into()), to: None, when: None };
        assert_eq!(record.line(Some(" ")), json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package", "de": "a"}));
        let crash = ExitRecord { kind: ExitKind::Crash, cause: None, from: None, to: None, when: started() };
        assert_eq!(crash.line(Some("10:55:02 panic=true")), json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "crash",
            "quando": STARTED, "erro": "10:55:02 panic=true"}));
    }

    /// C2 de ponta a ponta: a linha passa pelo gravador real do diário e volta pelo leitor da tela.
    /// A hora da saída é o `quando`, não o `ts` da subida que gravou a linha.
    #[test]
    fn exit_line_round_trips_through_the_diary() {
        let dir = tempfile::tempdir().unwrap();
        let now = DateTime::from_timestamp(i64::try_from(NOW).unwrap(), 0).unwrap();
        let secs = |days: u64| NOW - days * 86_400;
        let update = ExitRecord { kind: ExitKind::Update, cause: Some("package".into()), from: Some("0.1.0.9000".into()),
            to: Some("0.1.0.9010".into()), when: utc_secs(secs(2)) };
        let crash = ExitRecord { kind: ExitKind::Crash, cause: Some("out_of_memory".into()), from: None, to: None, when: utc_secs(secs(1)) };
        let old = ExitRecord { kind: ExitKind::Crash, cause: None, from: None, to: None, when: utc_secs(secs(8)) };
        let lines = [update.line(None), crash.line(Some("06:11:58 code=a | 06:11:59 code=b")), old.line(None)];
        hangar_core::diag::write_diary(dir.path(), &lines, now.with_timezone(&chrono::Local)).unwrap();
        let exits = hangar_status::diary::DiaryReader::at(dir.path().to_owned()).exits(now).unwrap();
        assert_eq!(exits.items, [
            hangar_api::status::Exit { ts: update.when.clone().unwrap(), kind: ExitKind::Update, cause: Some("package".into()),
                from: Some("0.1.0.9000".into()), to: Some("0.1.0.9010".into()), error_lines: vec![] },
            hangar_api::status::Exit { ts: crash.when.clone().unwrap(), kind: ExitKind::Crash, cause: Some("out_of_memory".into()),
                from: None, to: None, error_lines: vec!["06:11:58 code=a".into(), "06:11:59 code=b".into()] },
        ], "a saída de 8 dias atrás fica fora da janela mesmo gravada hoje");
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
        let pid = mark_this_process_running(hangar, "2026-11-17T10:00:00-03:00");
        let marked = before_restart(hangar, ExitKind::Requested, "restart_only", None, None);
        let written: Value = serde_json::from_slice(&std::fs::read(hangar.join(FILE)).unwrap()).unwrap();
        assert_eq!((written["tipo"].as_str(), written["causa"].as_str(), written["pid"].as_u64()), (Some("requested"), Some("restart_only"), Some(u64::from(pid))));
        assert!(written.get("de").is_none());
        marked.settle(&Ok::<(), ()>(()));
        let stopped = now_secs();
        hangar_boot::life::end_with(hangar, pid, Some("sigterm"), false, stopped).unwrap();
        let begin = hangar_boot::life::begin(hangar, "2026-11-17T10:01:00-03:00", |_, _| false).unwrap();
        let lines = previous_exit(hangar, begin.previous.as_ref(), begin.pid, || panic!("erro só na queda"), &Fixed(None), stopped + 3600, None);
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "requested", "causa": "restart_only",
            "quando": utc_secs(stopped)})], "o quando é o instante da parada gravada, e a volta uma hora depois não vence a marca");
        assert!(!hangar.join(FILE).exists(), "a subida consome a marca");
    }

    #[test]
    fn stop_intent_is_removed_when_the_restart_fails() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        mark_this_process_running(hangar, "agora");
        before_restart(hangar, ExitKind::Update, "package", Some("1"), Some("2")).settle(&Err::<(), _>("recusado"));
        assert!(!hangar.join(FILE).exists(), "reinício recusado com o servidor ainda de pé");
        // Sem servidor de pé, nada é gravado.
        let empty = tempfile::tempdir().unwrap();
        before_restart(empty.path(), ExitKind::Update, "package", None, None).restart_failed();
        assert!(!empty.path().join(FILE).exists());
    }

    /// Servidor que morreu sem gravar a parada deixa `running` no arquivo: ele não ganha marca (ela
    /// esconderia a queda), e a marca de quem morreu depois dela fica mesmo com o reinício falhando.
    #[cfg(unix)]
    #[test]
    fn stop_intent_follows_the_process_not_the_running_flag() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        hangar_boot::life::mark_running(hangar, 41, Some(41.0), None, "agora", |_, _| false).unwrap();
        let _ = before_restart(hangar, ExitKind::Update, "migrate", None, None);
        assert!(!hangar.join(FILE).exists(), "pid 41 com esse nascimento não está de pé");

        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let birth = crate::terminal_facts::agent_birth(pid).unwrap();
        hangar_boot::life::mark_running(hangar, pid, Some(birth), None, "agora", |_, _| false).unwrap();
        let marked = before_restart(hangar, ExitKind::Update, "package", None, None);
        assert!(hangar.join(FILE).exists());
        child.kill().unwrap();
        child.wait().unwrap();
        marked.restart_failed();
        assert!(hangar.join(FILE).exists(), "o servidor saiu sem `end`: a marca explica essa saída");
    }

    /// A marca vale contra a hora da parada gravada; sem ela, contra a subida.
    #[test]
    fn exit_intent_is_checked_against_the_recorded_stop() {
        let mut stopped = life(41, false, Some(("sigterm", false)));
        stopped.stop.as_mut().unwrap().at = Some(NOW - 3600 + 5);
        let marked = intent(ExitKind::Update, "package", 41, NOW - 3600);
        assert_eq!(classify(&stopped, Some(&marked), None, NOW).kind, ExitKind::Update, "voltou uma hora depois");
        stopped.stop.as_mut().unwrap().at = Some(NOW - 3600 + VALID_FOR.as_secs());
        assert_eq!(classify(&stopped, Some(&marked), None, NOW).kind, ExitKind::Requested, "parada fora do prazo da marca");
    }

    /// A unit parou o servidor velho e a nova não subiu: o reinício falha, mas a marca fica e a subida
    /// seguinte (a da volta) registra a atualização, não um pedido.
    #[test]
    fn stop_intent_stays_when_the_restart_fails_after_the_server_left() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        hangar_boot::life::begin(hangar, "2026-11-17T10:00:00-03:00", |_, _| false).unwrap();
        let pid = mark_this_process_running(hangar, "2026-11-17T10:00:00-03:00");
        let marked = before_restart(hangar, ExitKind::Update, "package", Some("1"), Some("2"));
        let stopped = now_secs();
        hangar_boot::life::end_with(hangar, pid, Some("sigterm"), false, stopped).unwrap();
        marked.settle(&Err::<(), _>("a unit nova não subiu"));
        assert!(hangar.join(FILE).exists());
        let begin = hangar_boot::life::begin(hangar, "2026-11-17T10:02:00-03:00", |_, _| false).unwrap();
        let lines = previous_exit(hangar, begin.previous.as_ref(), begin.pid, String::new, &Fixed(None), now_secs(), None);
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package", "de": "1", "para": "2",
            "quando": utc_secs(stopped)})]);
    }

    /// Primeira atualização para esta versão: a vida anterior (versão sem a parada gravada) não deixa
    /// `previous`, mas a marca dela ainda vira a linha, com a hora da marca. Marca de outro pid sai sem linha.
    #[test]
    fn stop_intent_explains_a_life_that_left_without_stop_record() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        let write_intent = |pid: u32| std::fs::write(hangar.join(FILE), json!({"tipo": "update", "causa": "package",
            "de": "0.1.0.9000", "para": "0.1.0.9010", "pid": pid, "ts": NOW - 10}).to_string()).unwrap();
        write_intent(41);
        let lines = previous_exit(hangar, None, Some(41), || panic!("erro só na queda"), &Fixed(None), NOW, None);
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package",
            "de": "0.1.0.9000", "para": "0.1.0.9010", "quando": utc_secs(NOW - 10)})]);
        assert!(!hangar.join(FILE).exists(), "a subida consome a marca");
        write_intent(40);
        assert!(previous_exit(hangar, None, Some(41), String::new, &Fixed(None), NOW, None).is_empty());
        write_intent(41);
        assert!(previous_exit(hangar, None, Some(41), String::new, &Fixed(None), NOW + VALID_FOR.as_secs(), None).is_empty(), "marca vencida");
        assert!(!hangar.join(FILE).exists());
    }

    /// Queda sem marca: a causa vem do journal e o erro, das últimas linhas do log.
    #[test]
    fn stop_intent_absent_crash_reads_the_journal_and_the_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "{torto").unwrap();
        let previous = life(41, true, None);
        let lines = previous_exit(dir.path(), Some(&previous), previous.pid, || "10:55:02 panic=true".into(), &Fixed(Some("oom-kill")), NOW, None);
        assert_eq!(lines[0]["codigo"], "stop_intent_unreadable");
        assert_eq!(lines[1], json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "crash", "causa": "out_of_memory",
            "quando": STARTED, "erro": "10:55:02 panic=true"}));
        assert!(!dir.path().join(FILE).exists(), "marca ilegível também sai");
    }

    fn run() -> UpdateRun { UpdateRun { from: Some("0.1.0.9000".into()), to: "0.1.0.9010".into() } }

    /// Primeira atualização de uma versão sem a marca: no Windows o executor antigo encerra o servidor à
    /// força (saída não limpa, sem parada gravada). O estado dele, no meio do reinício para esta versão,
    /// explica a saída como atualização, nunca queda.
    #[test]
    fn update_run_explains_an_unclean_exit_without_intent() {
        let dir = tempfile::tempdir().unwrap();
        let previous = life(41, true, None);
        let lines = previous_exit(dir.path(), Some(&previous), previous.pid, || panic!("erro só na queda"), &Fixed(None), NOW, Some(&run()));
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package",
            "de": "0.1.0.9000", "para": "0.1.0.9010", "quando": utc_secs(NOW)})]);
    }

    /// No Linux a versão antiga para limpa e sem `previous`: o estado do executor também vira a linha. Uma
    /// marca que não vale para esta vida (de outra vida, deixada por uma volta de versão) não o esconde.
    #[test]
    fn update_run_explains_a_life_without_stop_record_even_with_a_stale_intent() {
        let dir = tempfile::tempdir().unwrap();
        let hangar = dir.path();
        std::fs::write(hangar.join(FILE), json!({"tipo": "update", "causa": "package", "pid": 40, "ts": NOW - 10}).to_string()).unwrap();
        let lines = previous_exit(hangar, None, Some(41), || panic!("erro só na queda"), &Fixed(None), NOW, Some(&run()));
        assert_eq!(lines, [json!({"evento": "hangar_server.saida", "nivel": "aviso", "tipo": "update", "causa": "package",
            "de": "0.1.0.9000", "para": "0.1.0.9010", "quando": utc_secs(NOW)})]);
        // A marca válida continua vencendo o estado.
        std::fs::write(hangar.join(FILE), json!({"tipo": "requested", "causa": "restart_only", "pid": 41, "ts": NOW - 10}).to_string()).unwrap();
        let lines = previous_exit(hangar, None, Some(41), String::new, &Fixed(None), NOW, Some(&run()));
        assert_eq!(lines[0]["tipo"], "requested");
    }

    /// Só o executor no passo de reiniciar, indo para a versão que sobe, conta.
    #[test]
    fn update_run_needs_the_restart_step_toward_this_version() {
        let dir = tempfile::tempdir().unwrap();
        let state = |value: Value| std::fs::write(dir.path().join("estado.json"), value.to_string()).unwrap();
        // O executor grava o começo da etapa na hora local, com o fuso.
        let started = "2027-01-15T05:00:00-03:00";
        let at = DateTime::parse_from_rfc3339(started).unwrap().timestamp() as u64;
        state(json!({"fase": "rodando", "etapa": "reiniciar", "etapa_inicio": started, "versao_de": "0.1.0.9000", "versao_para": "0.1.0.9010"}));
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + 20), Some(run()));
        assert_eq!(update_run(dir.path(), "0.1.0.9011", at + 20), None, "outra versão subiu");
        // Executor que morreu deixou o estado assim: uma queda bem depois não vira atualização.
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + VALID_FOR.as_secs()), None, "etapa velha");
        state(json!({"fase": "rodando", "etapa": "reiniciar", "versao_para": "0.1.0.9010"}));
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + 20), None, "sem o começo da etapa");
        state(json!({"fase": "pronto", "etapa": "reiniciar", "etapa_inicio": started, "versao_para": "0.1.0.9010"}));
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + 20), None, "atualização já encerrada");
        state(json!({"fase": "rodando", "etapa": "baixar", "etapa_inicio": started, "versao_para": "0.1.0.9010"}));
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + 20), None, "ainda baixando");
        std::fs::remove_file(dir.path().join("estado.json")).unwrap();
        assert_eq!(update_run(dir.path(), "0.1.0.9010", at + 20), None);
    }
}
