//! O pacote diário que o servidor manda ao receptor (`schema: 1`). O receptor desserializa com estes
//! mesmos tipos: evento, dimensão e valor são texto no fio para ele gravar o que ainda não conhece.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::buckets::Bucket;
use crate::events::{Code, DimValue, Dims, Event};

pub const SCHEMA: u32 = 1;
/// `env.commit` de pacote que não é de release.
pub const DEV_COMMIT: &str = "dev";
/// Teto de linhas de log distintas num pacote: acima dele a linha nova só soma `log_dropped`.
pub const MAX_LOG_LINES: usize = 100;
/// Teto da mensagem de uma linha de log, em caracteres.
pub const MAX_LOG_CHARS: usize = 300;
pub const LOG_LEVELS: &[&str] = &["warn", "error"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub schema: u32,
    pub batch_id: String,
    pub machine_id: String,
    pub owner_id: String,
    /// Dia em UTC, `AAAA-MM-DD`.
    pub day: String,
    pub env: Env,
    pub counters: Vec<Counter>,
    /// Linhas de aviso e erro do log do servidor, já limpas na máquina; campo opcional do `schema: 1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<LogLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// `warn` ou `error`.
    pub level: String,
    pub target: String,
    pub message: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub version: String,
    pub commit: String,
    pub os: String,
    pub arch: String,
    pub channel: String,
}

/// Canais de pacote publicados; branch de checkout ou de pré-release é nome escolhido e sai `other`.
pub const CHANNELS: &[&str] = &["latest", "beta"];

impl Env {
    /// Sistema e arquitetura deste processo. Versão fora do formato e canal fora de `CHANNELS` saem
    /// `other`; o commit só vai quando o pacote em uso é de release e é hex curto, senão `dev`.
    pub fn new(version: &str, release_commit: Option<&str>, channel: &str) -> Env {
        Env {
            version: DimValue::version(version).as_str().to_owned(),
            commit: release_commit.filter(|c| is_commit(c)).unwrap_or(DEV_COMMIT).to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            channel: Code::known(channel, CHANNELS).as_str().to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counter {
    pub event: String,
    #[serde(default)]
    pub dims: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    pub count: u64,
}

impl Counter {
    pub fn new(event: Event, dims: Dims, bucket: Option<Bucket>, count: u64) -> Counter {
        Counter { event: event.as_str().to_owned(), dims: dims.into_wire(), bucket: bucket.map(|b| b.as_str().to_owned()), count }
    }
}

impl Batch {
    /// A forma que o receptor grava e o servidor manda: a mesma regra dos dois lados.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != SCHEMA { return Err("schema"); }
        if !is_id(&self.batch_id) { return Err("batch_id"); }
        if !is_id(&self.machine_id) { return Err("machine_id"); }
        if !is_id(&self.owner_id) { return Err("owner_id"); }
        if !is_day(&self.day) { return Err("day"); }
        let env = &self.env;
        if ![&env.version, &env.commit, &env.os, &env.arch, &env.channel].iter().all(|v| is_name(v)) { return Err("env"); }
        for counter in &self.counters {
            if !is_name(&counter.event) { return Err("event"); }
            if !counter.dims.iter().all(|(key, value)| is_name(key) && is_name(value)) { return Err("dims"); }
            if counter.bucket.as_deref().is_some_and(|b| !is_name(b)) { return Err("bucket"); }
        }
        if self.logs.len() > MAX_LOG_LINES { return Err("logs"); }
        for line in &self.logs {
            if !LOG_LEVELS.contains(&line.level.as_str()) { return Err("log_level"); }
            if !is_name(&line.target) { return Err("log_target"); }
            let chars = line.message.chars().count();
            if !(1..=MAX_LOG_CHARS).contains(&chars) || line.message.chars().any(char::is_control) { return Err("log_message"); }
            if line.count == 0 { return Err("log_count"); }
        }
        Ok(())
    }
}

/// Formato de nome que o receptor aceita (evento, chave e valor): o mesmo `code_ok` do relatório.
pub const fn is_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    // `while` em vez de iterador: `Code::fixed` usa esta regra em contexto `const`.
    let mut i = 0;
    while i < bytes.len() {
        if !matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b':' | b'-') {
            return false;
        }
        i += 1;
    }
    true
}

fn is_hex(value: &str) -> bool { value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }

/// Os ids aleatórios: 16 bytes em hex minúsculo.
pub(crate) fn is_id(value: &str) -> bool { value.len() == 32 && is_hex(value) }

fn is_commit(value: &str) -> bool { (7..=40).contains(&value.len()) && is_hex(value) }

/// `AAAA-MM-DD`; o receptor confere o calendário ao gravar.
pub(crate) fn is_day(value: &str) -> bool {
    let b = value.as_bytes();
    let number = |i: usize| (b[i] - b'0') * 10 + (b[i + 1] - b'0');
    b.len() == 10 && b[4] == b'-' && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&number(5)) && (1..=31).contains(&number(8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Provider, SenderOrigin, Surface};
    use serde_json::json;

    fn sample() -> Batch {
        Batch {
            schema: SCHEMA,
            batch_id: "0123456789abcdef0123456789abcdef".into(),
            machine_id: "11111111111111111111111111111111".into(),
            owner_id: "22222222222222222222222222222222".into(),
            day: "2026-11-26".into(),
            env: Env { version: "0.42.0".into(), commit: DEV_COMMIT.into(), os: "linux".into(), arch: "x86_64".into(), channel: "latest".into() },
            counters: vec![
                Counter::new(
                    Event::MessageSent,
                    Dims::new().with("provider", Provider::Claude).with("surface", Surface::Terminal)
                        .with("steer", false).with("origin", SenderOrigin::Person),
                    None,
                    42,
                ),
                Counter::new(
                    Event::SessionOpened,
                    Dims::new().with("provider", Provider::Codex).with("surface", Surface::Headless).with("restore", false),
                    Some(Bucket::Lt3s),
                    5,
                ),
            ],
            logs: Vec::new(),
        }
    }

    fn log(message: &str) -> LogLine {
        LogLine { level: "error".into(), target: "hangar_server::list".into(), message: message.into(), count: 3 }
    }

    #[test]
    fn batch_serializes_to_the_contract() {
        let expected = json!({
            "schema": 1,
            "batch_id": "0123456789abcdef0123456789abcdef",
            "machine_id": "11111111111111111111111111111111",
            "owner_id": "22222222222222222222222222222222",
            "day": "2026-11-26",
            "env": {"version": "0.42.0", "commit": "dev", "os": "linux", "arch": "x86_64", "channel": "latest"},
            "counters": [
                {"event": "message_sent", "dims": {"provider": "claude", "surface": "terminal", "steer": "false", "origin": "person"}, "count": 42},
                {"event": "session_opened", "dims": {"provider": "codex", "surface": "headless", "restore": "false"}, "bucket": "lt3s", "count": 5}
            ]
        });
        let batch = sample();
        assert_eq!(serde_json::to_value(&batch).unwrap(), expected);
        assert_eq!(serde_json::from_value::<Batch>(expected).unwrap(), batch);
    }

    #[test]
    fn receiver_reads_what_it_does_not_know() {
        let body = json!({
            "schema": 1, "batch_id": "a", "machine_id": "b", "owner_id": "c", "day": "2026-11-26",
            "env": {"version": "9.0.0", "commit": "dev", "os": "linux", "arch": "x86_64", "channel": "beta", "new_field": 1},
            "counters": [{"event": "future_event", "dims": {"new_dim": "new_value"}, "count": 1}, {"event": "peers", "count": 2}],
            "new_top_level": true
        });
        let batch: Batch = serde_json::from_value(body).unwrap();
        assert_eq!(batch.counters[0].event, "future_event");
        assert!(batch.counters[1].dims.is_empty());
        assert_eq!(batch.counters[1].bucket, None);
    }

    #[test]
    fn env_checks_every_field() {
        assert_eq!(Env::new("0.42.0", None, "latest").commit, "dev");
        assert_eq!(Env::new("0.42.0", Some("abc1234"), "latest").commit, "abc1234");
        assert_eq!(Env::new("0.42.0", Some("main"), "latest").commit, "dev");
        assert_eq!(Env::new("0.42.0", Some("ABC1234"), "latest").commit, "dev");
        assert_eq!(Env::new("0.42.0", None, "feat/x").channel, "other");
        assert_eq!(Env::new("0.42.0", None, "beta").channel, "beta");
        assert_eq!(Env::new("minha versão", None, "latest").version, "other");
        assert_eq!(Env::new("0.42.0", None, "latest").os, std::env::consts::OS);
    }

    #[test]
    fn validate_follows_the_contract() {
        assert_eq!(sample().validate(), Ok(()));
        let mut bad = sample();
        bad.machine_id = "XYZ".into();
        assert_eq!(bad.validate(), Err("machine_id"));
        let mut bad = sample();
        bad.batch_id = "0123456789ABCDEF0123456789ABCDEF".into();
        assert_eq!(bad.validate(), Err("batch_id"));
        let mut bad = sample();
        bad.day = "26/11/2026".into();
        assert_eq!(bad.validate(), Err("day"));
        let mut bad = sample();
        bad.day = "2026-13-01".into();
        assert_eq!(bad.validate(), Err("day"));
        let mut bad = sample();
        bad.schema = 2;
        assert_eq!(bad.validate(), Err("schema"));
        let mut bad = sample();
        bad.env.channel = "feat/x".into();
        assert_eq!(bad.validate(), Err("env"));
        let mut bad = sample();
        bad.counters[0].dims.insert("permission".into(), "acceptEdits".into());
        assert_eq!(bad.validate(), Err("dims"));
        let mut bad = sample();
        bad.counters[1].bucket = Some("2.5s ".into());
        assert_eq!(bad.validate(), Err("bucket"));

        let mut body = serde_json::to_value(sample()).unwrap();
        body.as_object_mut().unwrap().remove("machine_id");
        assert!(serde_json::from_value::<Batch>(body).is_err());
    }

    #[test]
    fn validate_checks_the_log_lines() {
        let with = |line: LogLine| Batch { logs: vec![line], ..sample() };
        assert_eq!(with(log("lista não lida: <caminho>")).validate(), Ok(()));
        assert_eq!(with(log(&"é".repeat(MAX_LOG_CHARS))).validate(), Ok(()), "o teto conta caractere, não byte");
        assert_eq!(with(log(&"a".repeat(MAX_LOG_CHARS + 1))).validate(), Err("log_message"));
        assert_eq!(with(log("")).validate(), Err("log_message"));
        assert_eq!(with(log("duas\nlinhas")).validate(), Err("log_message"));
        assert_eq!(with(LogLine { level: "info".into(), ..log("x") }).validate(), Err("log_level"));
        assert_eq!(with(LogLine { target: "Hangar Server".into(), ..log("x") }).validate(), Err("log_target"));
        assert_eq!(with(LogLine { count: 0, ..log("x") }).validate(), Err("log_count"));
        let full = Batch { logs: (0..=MAX_LOG_LINES).map(|n| log(&n.to_string())).collect(), ..sample() };
        assert_eq!(full.validate(), Err("logs"));
        let body = serde_json::to_value(with(log("x"))).unwrap();
        assert_eq!(body["logs"], json!([{"level": "error", "target": "hangar_server::list", "message": "x", "count": 3}]));
    }

    #[test]
    fn name_format() {
        assert!(is_name("session.browser"));
        assert!(is_name("lt3s"));
        assert!(!is_name(""));
        assert!(!is_name("acceptEdits"));
        assert!(!is_name("/api/sessions/{name}"));
        assert!(!is_name(&"a".repeat(65)));
    }
}
