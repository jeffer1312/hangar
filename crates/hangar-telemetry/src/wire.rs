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
/// Teto de `count` de um contador ou linha. Um dia de uma máquina não chega perto disso; o teto impede
/// que a soma do receptor (`bigint`) estoure com pacote forjado.
pub const MAX_COUNT: u64 = 10_000_000;
/// Teto de registros de falha distintos num pacote (por assinatura); acima dele só soma `count` nos que já estão.
pub const MAX_RECORDS: usize = 200;
/// Campos de um registro e teto do valor de cada um, em caracteres.
pub const MAX_RECORD_FIELDS: usize = 40;
pub const MAX_FIELD_CHARS: usize = 500;
/// Passos guardados antes da primeira vez de uma falha.
pub const MAX_RECORD_TRAIL: usize = 10;
/// Teto de passos da trilha do dia; repetições seguidas do mesmo passo viram um com `count`.
pub const MAX_TRAIL_STEPS: usize = 5000;
pub const RECORD_LEVELS: &[&str] = &["warn", "error"];
/// O que houve depois da última vez de uma falha: o mesmo passo deu certo em até 2 min, a pessoa fez
/// outra coisa, ou nada mais aconteceu no dia.
pub const AFTER: &[&str] = &["recovered", "other_action", "none"];
/// De onde o binário subiu: pacote de release instalado ou checkout de desenvolvimento.
pub const INSTALLS: &[&str] = &["package", "checkout", "other"];
pub const STEP_LEVELS: &[&str] = &["ok", "warn", "error"];

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
    /// Retrato do dia ainda aberto: o receptor troca o que tinha deste `batch_id` pelo novo, e o envio
    /// final do dia (sem `partial`) troca de novo. Campo opcional do `schema: 1`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// Falhas e avisos do diário de uso, com contexto; campo opcional do `schema: 1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<Record>,
    /// Cada passo do dia no diário de uso, só pelo nome; campo opcional do `schema: 1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trail: Vec<Step>,
    /// Hora UTC do envio (`AAAA-MM-DDTHH:MM:SSZ`): o receptor calcula a diferença do relógio da máquina.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// Ambiente da máquina no envio, em valores fechados (`events::MACHINE`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub machine: BTreeMap<String, String>,
}

/// Uma falha ou aviso do diário de uso, agrupado pela assinatura (evento, código, etapa, tipo do erro).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// `warn` ou `error`.
    pub level: String,
    pub event: String,
    pub count: u64,
    /// Hora UTC `HH:MM:SS` da primeira e da última vez no dia.
    pub first: String,
    pub last: String,
    /// Campos do diário que passam, já limpos na máquina: nome do campo → valor.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// Passos logo antes da primeira vez, do mais antigo ao mais novo: `acao` ou `acao:tela`.
    #[serde(default)]
    pub trail: Vec<String>,
    /// Passos logo antes da última vez, no mesmo formato.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trail_last: Vec<String>,
    /// Estado da máquina e da sessão na primeira e na última vez (`events::CONTEXT`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ctx_first: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ctx_last: BTreeMap<String, String>,
    /// Um de `AFTER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// A contagem bateu no teto `MAX_COUNT`: o número real é maior.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
}

/// Um passo da trilha do dia.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Hora UTC `HH:MM:SS` da primeira vez desta sequência.
    pub t: String,
    pub event: String,
    /// `ok`, `warn` ou `error`.
    pub level: String,
    /// Tela ou vista em que aconteceu, quando o diário diz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// Vezes seguidas.
    pub count: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
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
    /// Um de `INSTALLS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

/// Canais de pacote publicados; branch de checkout ou de pré-release é nome escolhido e sai `other`.
pub const CHANNELS: &[&str] = &["latest", "beta"];

impl Env {
    /// Sistema e arquitetura deste processo. Versão fora do formato e canal fora de `CHANNELS` saem
    /// `other`; o commit só vai quando o pacote em uso é de release e é hex curto, senão `dev`.
    pub fn new(version: &str, release_commit: Option<&str>, channel: &str, install: &str) -> Env {
        Env {
            install: Some(Code::known(install, INSTALLS).as_str().to_owned()),
            version: DimValue::version(version).as_str().to_owned(),
            commit: release_commit.filter(|c| is_commit(c)).unwrap_or(DEV_COMMIT).to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            channel: Code::known(channel, CHANNELS).as_str().to_owned(),
        }
    }
}

/// O aviso único de "Não enviar" (`POST /v1/declined`): nenhum identificador, só o que o receptor conta por dia.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub version: String,
    pub os: String,
    pub channel: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefusalKind {
    /// A primeira resposta ao aviso foi não.
    Declined,
    /// Tinha aceitado e desligou.
    Revoked,
}

impl RefusalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RefusalKind::Declined => "declined",
            RefusalKind::Revoked => "revoked",
        }
    }
}

impl Refusal {
    pub fn new(kind: RefusalKind, env: &Env) -> Refusal {
        Refusal { kind, version: env.version.clone(), os: env.os.clone(), channel: env.channel.clone() }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        for (field, value) in [("version", &self.version), ("os", &self.os), ("channel", &self.channel)] {
            if !is_name(value) { return Err(field); }
        }
        Ok(())
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
}

impl Counter {
    pub fn new(event: Event, dims: Dims, bucket: Option<Bucket>, count: u64) -> Counter {
        Counter { event: event.as_str().to_owned(), dims: dims.into_wire(), bucket: bucket.map(|b| b.as_str().to_owned()), count, capped: false }
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
        if env.install.as_deref().is_some_and(|i| !INSTALLS.contains(&i)) { return Err("env_install"); }
        if self.sent_at.as_deref().is_some_and(|t| !is_instant(t)) { return Err("sent_at"); }
        if !is_closed_map(&self.machine) { return Err("machine"); }
        for counter in &self.counters {
            if !is_name(&counter.event) { return Err("event"); }
            if !counter.dims.iter().all(|(key, value)| is_name(key) && is_name(value)) { return Err("dims"); }
            if counter.bucket.as_deref().is_some_and(|b| !is_name(b)) { return Err("bucket"); }
            if counter.count > MAX_COUNT { return Err("count"); }
        }
        if self.logs.len() > MAX_LOG_LINES { return Err("logs"); }
        for line in &self.logs {
            if !LOG_LEVELS.contains(&line.level.as_str()) { return Err("log_level"); }
            if !is_name(&line.target) { return Err("log_target"); }
            let chars = line.message.chars().count();
            if !(1..=MAX_LOG_CHARS).contains(&chars) || line.message.chars().any(is_invisible) { return Err("log_message"); }
            if line.count == 0 || line.count > MAX_COUNT { return Err("log_count"); }
        }
        if self.records.len() > MAX_RECORDS { return Err("records"); }
        for record in &self.records {
            if !RECORD_LEVELS.contains(&record.level.as_str()) { return Err("record_level"); }
            if !is_name(&record.event) { return Err("record_event"); }
            if record.count == 0 || record.count > MAX_COUNT { return Err("record_count"); }
            if !is_time(&record.first) || !is_time(&record.last) { return Err("record_time"); }
            if record.fields.len() > MAX_RECORD_FIELDS { return Err("record_fields"); }
            if !record.fields.iter().all(|(key, value)| is_name(key) && is_text(value, MAX_FIELD_CHARS)) { return Err("record_field"); }
            if [&record.trail, &record.trail_last].iter().any(|t| t.len() > MAX_RECORD_TRAIL || !t.iter().all(|s| is_name(s))) { return Err("record_trail"); }
            if !is_closed_map(&record.ctx_first) || !is_closed_map(&record.ctx_last) { return Err("record_ctx"); }
            if record.after.as_deref().is_some_and(|a| !AFTER.contains(&a)) { return Err("record_after"); }
        }
        if self.trail.len() > MAX_TRAIL_STEPS { return Err("trail"); }
        for step in &self.trail {
            if !is_time(&step.t) { return Err("step_time"); }
            if !is_name(&step.event) || !STEP_LEVELS.contains(&step.level.as_str()) { return Err("step"); }
            if step.screen.as_deref().is_some_and(|s| !is_name(s)) { return Err("step_screen"); }
            if step.count == 0 || step.count > MAX_COUNT { return Err("step_count"); }
        }
        Ok(())
    }
}

/// Caractere que não aparece ou mexe no que vem depois: controle (Cc), formato (Cf, como U+200B,
/// U+202E e U+FEFF) e separador de linha ou parágrafo (U+2028, U+2029).
pub fn is_invisible(c: char) -> bool {
    // ponytail: faixas Cf do Unicode 15.1 escritas à mão; categoria nova só entra aqui.
    c.is_control() || matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x180E
        | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB
        | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
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

/// Mapa de valores fechados: até `MAX_RECORD_FIELDS` pares no formato de nome.
fn is_closed_map(map: &BTreeMap<String, String>) -> bool {
    map.len() <= MAX_RECORD_FIELDS && map.iter().all(|(key, value)| is_name(key) && is_name(value))
}

/// `AAAA-MM-DDTHH:MM:SSZ`.
pub(crate) fn is_instant(value: &str) -> bool {
    value.len() == 20 && value.as_bytes()[10] == b'T' && value.ends_with('Z') && is_day(&value[..10]) && is_time(&value[11..19])
}

/// Texto livre já limpo: 1 a `max` caracteres, nenhum invisível.
fn is_text(value: &str, max: usize) -> bool { (1..=max).contains(&value.chars().count()) && !value.chars().any(is_invisible) }

/// `HH:MM:SS`.
pub(crate) fn is_time(value: &str) -> bool {
    let b = value.as_bytes();
    let number = |i: usize| (b[i] - b'0') * 10 + (b[i + 1] - b'0');
    b.len() == 8 && b[2] == b':' && b[5] == b':'
        && b.iter().enumerate().all(|(i, c)| i == 2 || i == 5 || c.is_ascii_digit())
        && number(0) < 24 && number(3) < 60 && number(6) < 60
}

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
            env: Env { version: "0.42.0".into(), commit: DEV_COMMIT.into(), os: "linux".into(), arch: "x86_64".into(), channel: "latest".into(), install: None },
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
            partial: false,
            records: Vec::new(),
            trail: Vec::new(),
            sent_at: None,
            machine: BTreeMap::new(),
        }
    }

    fn record() -> Record {
        Record {
            level: "error".into(), event: "sessao.abrir".into(), count: 2, first: "10:00:01".into(), last: "11:30:00".into(),
            fields: [("codigo".to_owned(), "erro_pasta".to_owned()), ("detalhe".to_owned(), "pasta <caminho> fora do permitido".to_owned())].into(),
            trail: vec!["app.visivel".into(), "sessao.nova".into()],
            trail_last: Vec::new(), ctx_first: BTreeMap::new(), ctx_last: BTreeMap::new(), after: None, capped: false,
        }
    }

    #[test]
    fn records_and_trail_follow_the_contract() {
        let mut batch = sample();
        batch.partial = true;
        batch.records = vec![record()];
        batch.trail = vec![Step { t: "10:00:00".into(), event: "app.visivel".into(), level: "ok".into(), screen: Some("chat".into()), count: 3, capped: false }];
        assert_eq!(batch.validate(), Ok(()));
        let value = serde_json::to_value(&batch).unwrap();
        assert_eq!(value["partial"], json!(true));
        assert_eq!(value["records"][0]["fields"]["codigo"], json!("erro_pasta"));
        assert_eq!(value["trail"][0], json!({"t": "10:00:00", "event": "app.visivel", "level": "ok", "screen": "chat", "count": 3}));
        assert_eq!(serde_json::from_value::<Batch>(value).unwrap(), batch);

        let bad = |change: fn(&mut Batch)| { let mut b = batch.clone(); change(&mut b); b.validate() };
        assert_eq!(bad(|b| b.records[0].level = "info".into()), Err("record_level"));
        assert_eq!(bad(|b| b.records[0].first = "25:00:00".into()), Err("record_time"));
        assert_eq!(bad(|b| { b.records[0].fields.insert("detalhe".into(), "a\u{202e}b".into()); }), Err("record_field"));
        assert_eq!(bad(|b| { b.records[0].fields.insert("Campo Livre".into(), "x".into()); }), Err("record_field"));
        assert_eq!(bad(|b| { b.records[0].fields.insert("detalhe".into(), "x".repeat(MAX_FIELD_CHARS + 1)); }), Err("record_field"));
        assert_eq!(bad(|b| b.records[0].trail = vec!["Nome Livre".into()]), Err("record_trail"));
        assert_eq!(bad(|b| b.trail[0].count = 0), Err("step_count"));
        assert_eq!(bad(|b| b.trail[0].screen = Some("Tela X".into())), Err("step_screen"));
        assert_eq!(bad(|b| b.records = vec![record(); MAX_RECORDS + 1]), Err("records"));
    }

    #[test]
    fn context_machine_and_clock_follow_the_contract() {
        let mut batch = sample();
        batch.env.install = Some("checkout".into());
        batch.sent_at = Some("2026-11-26T23:59:01Z".into());
        batch.machine = [("os".to_owned(), "linux".to_owned()), ("ram_total".to_owned(), "16-32g".to_owned())].into();
        let mut rec = record();
        rec.ctx_first = [("mem_free".to_owned(), "lt10".to_owned())].into();
        rec.ctx_last = rec.ctx_first.clone();
        rec.trail_last = vec!["session.browser:chat".into()];
        rec.after = Some("recovered".into());
        rec.capped = true;
        batch.records = vec![rec];
        assert_eq!(batch.validate(), Ok(()));
        let value = serde_json::to_value(&batch).unwrap();
        assert_eq!(value["records"][0]["after"], json!("recovered"));
        assert_eq!(value["env"]["install"], json!("checkout"));
        assert_eq!(serde_json::from_value::<Batch>(value).unwrap(), batch);
        let bad = |change: fn(&mut Batch)| { let mut b = batch.clone(); change(&mut b); b.validate() };
        assert_eq!(bad(|b| b.env.install = Some("source".into())), Err("env_install"));
        assert_eq!(bad(|b| b.sent_at = Some("2026-11-26 23:59:01".into())), Err("sent_at"));
        assert_eq!(bad(|b| { b.machine.insert("os".into(), "Ubuntu 24.04".into()); }), Err("machine"));
        assert_eq!(bad(|b| { b.records[0].ctx_last.insert("model".into(), "Meu Motor".into()); }), Err("record_ctx"));
        assert_eq!(bad(|b| b.records[0].after = Some("gave_up".into())), Err("record_after"));
        assert_eq!(bad(|b| b.records[0].trail_last = vec!["/home/x".into()]), Err("record_trail"));
        assert_eq!(Env::new("0.42.0", None, "latest", "checkout").install.as_deref(), Some("checkout"));
        assert_eq!(Env::new("0.42.0", None, "latest", "x").install.as_deref(), Some("other"));
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
        assert_eq!(Env::new("0.42.0", None, "latest", "package").commit, "dev");
        assert_eq!(Env::new("0.42.0", Some("abc1234"), "latest", "package").commit, "abc1234");
        assert_eq!(Env::new("0.42.0", Some("main"), "latest", "package").commit, "dev");
        assert_eq!(Env::new("0.42.0", Some("ABC1234"), "latest", "package").commit, "dev");
        assert_eq!(Env::new("0.42.0", None, "feat/x", "package").channel, "other");
        assert_eq!(Env::new("0.42.0", None, "beta", "package").channel, "beta");
        assert_eq!(Env::new("minha versão", None, "latest", "package").version, "other");
        assert_eq!(Env::new("0.42.0", None, "latest", "package").os, std::env::consts::OS);
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
        for hidden in ['\u{2028}', '\u{2029}', '\u{202E}', '\u{200B}', '\u{FEFF}', '\u{AD}'] {
            assert_eq!(with(log(&format!("a{hidden}b"))).validate(), Err("log_message"), "U+{:04X}", hidden as u32);
        }
        assert_eq!(with(log("acentuação, ç e — ficam")).validate(), Ok(()));
        assert_eq!(with(LogLine { level: "info".into(), ..log("x") }).validate(), Err("log_level"));
        assert_eq!(with(LogLine { target: "Hangar Server".into(), ..log("x") }).validate(), Err("log_target"));
        assert_eq!(with(LogLine { count: 0, ..log("x") }).validate(), Err("log_count"));
        assert_eq!(with(LogLine { count: MAX_COUNT, ..log("x") }).validate(), Ok(()));
        assert_eq!(with(LogLine { count: MAX_COUNT + 1, ..log("x") }).validate(), Err("log_count"));
        let mut big = sample();
        big.counters[0].count = MAX_COUNT + 1;
        assert_eq!(big.validate(), Err("count"));
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

    #[test]
    fn refusal_carries_only_kind_version_os_and_channel() {
        let env = Env::new("0.42.0", None, "latest", "package");
        let wire = serde_json::to_value(Refusal::new(RefusalKind::Revoked, &env)).unwrap();
        assert_eq!(wire, json!({"kind": "revoked", "version": "0.42.0", "os": std::env::consts::OS, "channel": "latest"}));
        let with_id = json!({"kind": "declined", "version": "0.42.0", "os": "linux", "channel": "latest", "machine_id": "1".repeat(32)});
        assert!(serde_json::from_value::<Refusal>(with_id).is_err(), "identificador não passa");
        assert!(serde_json::from_value::<Refusal>(json!({"kind": "maybe", "version": "0.42.0", "os": "linux", "channel": "latest"})).is_err());
        let bad: Refusal = serde_json::from_value(json!({"kind": "declined", "version": "0.42.0", "os": "Linux Mint", "channel": "latest"})).unwrap();
        assert_eq!(bad.validate(), Err("os"));
    }
}
