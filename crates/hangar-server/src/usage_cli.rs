//! Coleta e comparação dos intervalos de uso registrados pela orquestração.
use crate::{
    orchestration::{Integer, integer},
    transcript::Provider,
};
use chrono::{DateTime, FixedOffset, SecondsFormat, Timelike, Utc};
use indexmap::IndexMap;
use num_bigint::BigInt;
use ring::digest::{Context, SHA256};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
};

const TOKENS: [&str; 4] = ["input", "cache_read", "cache_write", "output"];
const COUNTERS: [&str; 6] = [
    "input",
    "cache_read",
    "cache_write",
    "output",
    "steps",
    "bytes",
];
const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "omp", "kimi"];
const METHODOLOGY: [&str; 4] = [
    "Tokens = totais finais menos iniciais; entrada exclui cache lido e criado.",
    "Janela observada inclui espera; sua soma pode incluir trabalho em paralelo.",
    "Subagentes só entram com pares próprios; isto não presume o total da equipe.",
    "Modelo e esforço são observações declaradas, sem atribuição de tokens por modelo.",
];

pub struct SnapshotArgs<'a> {
    pub provider: &'a str,
    pub transcript: &'a Path,
    pub role: &'a str,
    pub session: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|s| !hangar_core::pycompat::py::strip(s).is_empty())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path)
        .map(hangar_core::fs::without_verbatim)
        .map_err(|error| error.to_string())
}

fn hostname() -> Result<String, String> {
    sysinfo::System::host_name()
        .ok_or_else(|| "Não foi possível identificar a máquina da coleta.".into())
}

fn digest(path: &Path, size: u64) -> Result<String, String> {
    let mut stream = File::open(path).map_err(|error| error.to_string())?;
    let mut context = Context::new(&SHA256);
    let mut remaining = size;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = stream
            .read(&mut buffer[..limit])
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err(format!("Fonte truncada: {}", path.display()));
        }
        context.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn file_state(path: &Path) -> Result<((u64, u64), u64, std::time::SystemTime), String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    Ok((
        crate::stats::identity(&file, &metadata).map_err(|error| error.to_string())?,
        metadata.len(),
        metadata.modified().map_err(|error| error.to_string())?,
    ))
}

pub fn snapshot(args: SnapshotArgs<'_>) -> Result<Value, String> {
    if !args.transcript.is_absolute()
        || hangar_core::pycompat::py::strip(args.role).is_empty()
        || hangar_core::pycompat::py::strip(args.session).is_empty()
    {
        return Err("Informe transcript absoluto, papel e sessão.".into());
    }
    let path = canonical(args.transcript)?;
    let before = file_state(&path)?;
    let provider = Provider::parse(args.provider)
        .filter(|_| PROVIDERS.contains(&args.provider))
        .ok_or_else(|| format!("Provider não suportado: {}", args.provider))?;
    let totals = crate::stats::Accumulator::new(provider, path.clone())
        .usage_totals()
        .map_err(|error| error.to_string())?;
    if COUNTERS
        .iter()
        .any(|key| integer(&totals[*key]).is_none_or(|n| n.0 < BigInt::default()))
    {
        return Err("Fonte contém contadores de uso inconsistentes.".into());
    }
    let bytes = totals["bytes"]
        .as_u64()
        .ok_or_else(|| "Limite da fonte inválido.".to_owned())?;
    let sha256 = digest(&path, bytes)?;
    let after = file_state(&path)?;
    if before != after {
        return Err("Fonte mudou durante a coleta; repita quando a sessão estiver ociosa.".into());
    }
    if after.1 != bytes {
        return Err("Fonte contém linha parcial; repita quando a escrita terminar.".into());
    }
    Ok(json!({
        "version":1, "ts":Utc::now().to_rfc3339_opts(SecondsFormat::Micros,false),
        "role":args.role, "session":args.session, "provider":args.provider,
        "model_observed":args.model, "effort_observed":args.effort,
        "source":{"host":hostname()?,"path":path,"device":after.0.0,"inode":after.0.1},
        "sha256":sha256, "totals":totals,
        "coverage":if totals["steps"] == 0 {"sem_uso_observado"} else {"uso_observado"}
    }))
}

struct Validated<'a> {
    raw: &'a Value,
    stamp: DateTime<FixedOffset>,
    counts: [BigInt; 6],
    identity: (String, u64, u64),
}

fn validate(value: &Value) -> Result<Validated<'_>, String> {
    if !value.is_object() || value["version"].as_i64() != Some(1) {
        return Err("Snapshot inválido ou versão desconhecida.".into());
    }
    for key in ["role", "session", "provider", "ts", "sha256"] {
        if text(&value[key]).is_none() {
            return Err(format!("Snapshot sem {key} válido."));
        }
    }
    if !PROVIDERS.contains(&value["provider"].as_str().unwrap()) {
        return Err("Provider não suportado no snapshot.".into());
    }
    if !value["totals"].is_object() || !value["source"].is_object() {
        return Err("Snapshot sem totais ou fonte.".into());
    }
    let mut counts = std::array::from_fn(|_| BigInt::default());
    for (i, key) in COUNTERS.iter().enumerate() {
        counts[i] = integer(&value["totals"][*key])
            .filter(|n| n.0 >= BigInt::default())
            .map(|n| n.0)
            .ok_or_else(|| format!("Contador inválido: {key}"))?;
    }
    if counts[4] == BigInt::default() && counts[..4].iter().any(|n| *n != BigInt::default()) {
        return Err("Snapshot tem tokens sem chamadas observadas.".into());
    }
    for key in ["device", "inode"] {
        if integer(&value["source"][key]).is_none_or(|n| n.0 < BigInt::default()) {
            return Err(format!("Identidade da fonte inválida: {key}"));
        }
    }
    let host = hostname()?;
    if value["source"]["host"] != host {
        return Err("O relatório precisa rodar na máquina que coletou os snapshots.".into());
    }
    let source_path = value["source"]["path"]
        .as_str()
        .filter(|path| Path::new(path).is_absolute())
        .ok_or_else(|| "Snapshot sem caminho absoluto da fonte.".to_owned())?;
    let stamp = DateTime::parse_from_rfc3339(value["ts"].as_str().unwrap())
        .map_err(|_| "Timestamp inválido ou sem fuso horário.".to_owned())?;
    let stamp = stamp
        .with_nanosecond(stamp.timestamp_subsec_micros() * 1000)
        .ok_or_else(|| "Timestamp inválido.".to_owned())?;
    let path = Path::new(source_path);
    let current = file_state(path)?;
    if canonical(path)? != path.components().collect::<PathBuf>()
        || integer(&value["source"]["device"]).unwrap().0 != BigInt::from(current.0.0)
        || integer(&value["source"]["inode"]).unwrap().0 != BigInt::from(current.0.1)
    {
        return Err(format!("Fonte substituída: {}", path.display()));
    }
    let bytes = value["totals"]["bytes"]
        .as_u64()
        .ok_or_else(|| format!("Fonte truncada: {}", path.display()))?;
    if digest(path, bytes)? != value["sha256"].as_str().unwrap() {
        return Err(format!("Fonte reescrita: {}", path.display()));
    }
    Ok(Validated {
        raw: value,
        stamp,
        counts,
        identity: (host, current.0.0, current.0.1),
    })
}

#[derive(Default)]
struct Role {
    intervals: usize,
    seconds: f64,
    tokens: [BigInt; 4],
}

fn tokens(values: &[BigInt]) -> Value {
    Value::Object(
        TOKENS
            .into_iter()
            .zip(values)
            .map(|(key, value)| {
                (
                    key.to_owned(),
                    serde_json::to_value(Integer(value.clone())).expect("inteiro JSON"),
                )
            })
            .collect(),
    )
}

pub fn report(pairs: &[(Value, Value)]) -> Result<Value, String> {
    if pairs.is_empty() {
        return Err("Informe pelo menos um par de snapshots.".into());
    }
    let mut intervals = Vec::new();
    let mut roles: IndexMap<String, Role> = IndexMap::new();
    let mut seen: HashMap<
        (String, u64, u64),
        Vec<(DateTime<FixedOffset>, DateTime<FixedOffset>, BigInt, BigInt)>,
    > = HashMap::new();
    for (start, end) in pairs {
        let (first, last) = (validate(start)?, validate(end)?);
        for key in ["source", "provider", "role", "session"] {
            if first.raw[key] != last.raw[key] {
                return Err(format!("Snapshots de fontes ou vínculos diferentes: {key}"));
            }
        }
        if last.stamp <= first.stamp {
            return Err("O fim precisa ser posterior ao início.".into());
        }
        let delta: [BigInt; 6] = std::array::from_fn(|i| &last.counts[i] - &first.counts[i]);
        if delta.iter().any(|n| *n < BigInt::default()) {
            return Err("Contadores regrediram entre os snapshots.".into());
        }
        if delta[4] == BigInt::default()
            || delta[5] == BigInt::default()
            || delta[..4].iter().all(|n| *n == BigInt::default())
        {
            return Err("Sem uso novo observado; não há consumo comprovado para este par.".into());
        }
        let previous = seen.entry(first.identity.clone()).or_default();
        for (t0, t1, b0, b1) in previous.iter() {
            if (first.stamp < *t1 && last.stamp > *t0)
                || (first.counts[5] < *b1 && last.counts[5] > *b0)
            {
                return Err(format!(
                    "Intervalos sobrepostos da mesma fonte: {}",
                    start["source"]["path"].as_str().unwrap()
                ));
            }
        }
        previous.push((
            first.stamp,
            last.stamp,
            first.counts[5].clone(),
            last.counts[5].clone(),
        ));
        let seconds = (last.stamp - first.stamp)
            .num_microseconds()
            .ok_or_else(|| "Intervalo não representável.".to_owned())? as f64
            / 1_000_000.0;
        intervals.push(json!({"role":start["role"],"session":start["session"],"provider":start["provider"],"source":start["source"],
            "start":start["ts"],"end":end["ts"],"window_seconds":seconds,
            "start_bytes":start["totals"]["bytes"],"end_bytes":end["totals"]["bytes"],"tokens":tokens(&delta[..4]),
            "model_observed":[start.get("model_observed"),end.get("model_observed")],
            "effort_observed":[start.get("effort_observed"),end.get("effort_observed")]}));
        let role = roles
            .entry(start["role"].as_str().unwrap().to_owned())
            .or_default();
        role.intervals += 1;
        role.seconds += seconds;
        for (sum, delta) in role.tokens.iter_mut().zip(&delta[..4]) {
            *sum += delta;
        }
    }
    let by_role: Map<String,Value> = roles.into_iter().map(|(name,role)| (name,json!({"intervals":role.intervals,"window_seconds_sum":role.seconds,"tokens":tokens(&role.tokens)}))).collect();
    Ok(
        json!({"coverage":"somente_fontes_registradas","methodology":METHODOLOGY,"by_role":by_role,"intervals":intervals}),
    )
}

fn execute(args: &[String]) -> Result<Value, String> {
    let Some(command) = args.first() else {
        return Err("Informe usage snapshot ou usage report.".into());
    };
    match command.as_str() {
        "snapshot" => {
            let mut fields: HashMap<&str, &str> = HashMap::new();
            let mut at = 1;
            while at < args.len() {
                let key = args[at]
                    .strip_prefix("--")
                    .ok_or_else(|| format!("Argumento desconhecido: {}", args[at]))?;
                let (key, value) = if let Some((key, value)) = key.split_once('=') {
                    (key, value)
                } else {
                    at += 1;
                    (
                        key,
                        args.get(at)
                            .map(String::as_str)
                            .filter(|v| !v.starts_with("--"))
                            .ok_or_else(|| format!("Informe --{key}."))?,
                    )
                };
                if ![
                    "provider",
                    "transcript",
                    "role",
                    "session",
                    "model",
                    "effort",
                ]
                .contains(&key)
                {
                    return Err(format!("Argumento desconhecido: --{key}"));
                }
                fields.insert(key, value);
                at += 1;
            }
            let required = |key| {
                fields
                    .get(key)
                    .copied()
                    .ok_or_else(|| format!("Informe --{key}."))
            };
            snapshot(SnapshotArgs {
                provider: required("provider")?,
                transcript: Path::new(required("transcript")?),
                role: required("role")?,
                session: required("session")?,
                model: fields.get("model").copied(),
                effort: fields.get("effort").copied(),
            })
        }
        "report" => {
            let mut pairs = Vec::new();
            let mut at = 1;
            while at < args.len() {
                if args[at] != "--pair" || at + 2 >= args.len() {
                    return Err("Informe --pair <início.json> <fim.json>.".into());
                }
                let read = |path: &str| -> Result<Value, String> {
                    let raw = fs::read(path).map_err(|error| error.to_string())?;
                    crate::transcript::decode_line(&raw)
                        .ok_or_else(|| "Snapshot JSON inválido.".into())
                };
                pairs.push((read(&args[at + 1])?, read(&args[at + 2])?));
                at += 3;
            }
            report(&pairs)
        }
        _ => Err(format!("Comando de consumo desconhecido: {command}")),
    }
}

pub fn cli(args: &[String]) -> ExitCode {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!(
            "usage snapshot --provider <claude|codex|pi|omp|kimi> --transcript <caminho-absoluto> --role <papel> --session <nome> [--model <modelo>] [--effort <esforço>]\nusage report --pair <início.json> <fim.json> [--pair <início.json> <fim.json> ...]"
        );
        return ExitCode::SUCCESS;
    }
    match execute(args)
        .and_then(|value| serde_json::to_string_pretty(&value).map_err(|error| error.to_string()))
    {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Consumo não medido: {error}");
            ExitCode::FAILURE
        }
    }
}
