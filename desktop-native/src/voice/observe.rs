//! Observação da máquina para a voz: processos, CPU e memória, só leitura. Roda no app, fora do isolamento do shell do
//! organizador, que tem espaço de processos próprio e não enxerga os da máquina.
use std::time::Duration;
use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub const DEFAULT_SAMPLE: Duration = Duration::from_millis(1000);
const MAX_SAMPLE: Duration = Duration::from_secs(5);
pub const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 80;
const CMD_WIDTH: usize = 200;

#[derive(Debug, PartialEq)]
pub struct Request { pub filter: Option<String>, pub by_memory: bool, pub sample: Duration, pub limit: usize }

impl Request {
    pub fn new(filter: Option<String>, by_memory: bool, sample_ms: Option<u64>, limit: Option<u64>) -> Self {
        let sample = sample_ms.map_or(DEFAULT_SAMPLE, Duration::from_millis).clamp(MINIMUM_CPU_UPDATE_INTERVAL, MAX_SAMPLE);
        let limit = limit.map_or(DEFAULT_LIMIT, |l| l as usize).clamp(1, MAX_LIMIT);
        Self { filter: filter.map(|f| f.to_lowercase()), by_memory, sample, limit }
    }
}

/// Palavras de chave/argumento cujo valor é segredo.
const SECRET_WORDS: [&str; 10] = ["token", "secret", "password", "passwd", "apikey", "api_key", "api-key", "auth", "bearer", "credential"];

fn secret_name(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_lowercase();
    SECRET_WORDS.iter().any(|w| name.contains(w)) || name == "key" || name.ends_with("_key") || name.ends_with("-key")
}

/// Valor que parece segredo pela forma: longo, sem barra, com letras e números misturados.
fn secret_shape(arg: &str) -> bool {
    arg.len() >= 24 && !arg.contains(['/', '\\', ' ']) && arg.chars().any(|c| c.is_ascii_digit()) && arg.chars().any(|c| c.is_ascii_alphabetic())
        && arg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '=' | '.'))
}

/// Linha de comando sem segredos: `--token X`, `TOKEN=X` e valores com cara de chave viram `***`.
pub fn redact(args: &[String]) -> String {
    let mut out: Vec<String> = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for arg in args {
        let shown = if hide_next { "***".to_owned() }
            else if let Some((name, _)) = arg.split_once('=').filter(|(name, _)| secret_name(name)) { format!("{name}=***") }
            else if secret_shape(arg) { "***".to_owned() }
            else { arg.clone() };
        hide_next = !hide_next && arg.starts_with('-') && !arg.contains('=') && secret_name(arg);
        out.push(shown);
    }
    let line = out.join(" ");
    if line.chars().count() <= CMD_WIDTH { line } else { format!("{}…", line.chars().take(CMD_WIDTH).collect::<String>()) }
}

fn mib(bytes: u64) -> String { format!("{:.0} MiB", bytes as f64 / 1_048_576.) }

fn age(secs: u64) -> String {
    match secs { s if s >= 3600 => format!("{} h {} min", s / 3600, s / 60 % 60), s if s >= 60 => format!("{} min", s / 60), s => format!("{s} s") }
}

/// Duas leituras separadas pela amostra: o uso de CPU é a diferença entre elas (100% = um núcleo inteiro).
pub fn observe(request: &Request) -> String {
    let kind = ProcessRefreshKind::new().with_cpu().with_memory().with_cmd(UpdateKind::OnlyIfNotSet);
    let mut system = System::new();
    system.refresh_cpu_usage();
    system.refresh_processes_specifics(ProcessesToUpdate::All, kind);
    std::thread::sleep(request.sample);
    system.refresh_cpu_usage();
    system.refresh_memory();
    system.refresh_processes_specifics(ProcessesToUpdate::All, kind);
    let mut rows: Vec<(f32, u64, String)> = system.processes().values().filter_map(|p| {
        let name = p.name().to_string_lossy().into_owned();
        let args: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let cmd = redact(&args);
        let hit = request.filter.as_ref().is_none_or(|f| name.to_lowercase().contains(f) || cmd.to_lowercase().contains(f));
        hit.then(|| {
            let parent = p.parent().map_or_else(|| "-".to_owned(), |pid| pid.to_string());
            let line = format!("- pid {} (pai {parent}) {name}: CPU {:.1}%, memória {}, há {}{}", p.pid(), p.cpu_usage(), mib(p.memory()), age(p.run_time()),
                if cmd.is_empty() { String::new() } else { format!(" — {cmd}") });
            (p.cpu_usage(), p.memory(), line)
        })
    }).collect();
    let matched = rows.len();
    if request.by_memory { rows.sort_by(|a, b| b.1.cmp(&a.1)); } else { rows.sort_by(|a, b| b.0.total_cmp(&a.0)); }
    let load = System::load_average();
    let mut text = format!("Máquina: {} núcleos, CPU total {:.1}%, memória usada {} de {}, carga {:.2} {:.2} {:.2}, amostra de {} ms.\n",
        system.cpus().len(), system.global_cpu_usage(), mib(system.used_memory()), mib(system.total_memory()), load.one, load.five, load.fifteen,
        request.sample.as_millis());
    let filter = request.filter.as_deref().map(|f| format!(" com \"{f}\"")).unwrap_or_default();
    text.push_str(&format!("Processos: {} no total, {matched}{filter}; os {} primeiros por {}:\n", system.processes().len(), rows.len().min(request.limit),
        if request.by_memory { "memória" } else { "CPU" }));
    for (_, _, line) in rows.into_iter().take(request.limit) { text.push_str(&line); text.push('\n'); }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn args(line: &str) -> Vec<String> { line.split(' ').map(str::to_owned).collect() }

    #[test]
    fn redact_hides_secrets_and_keeps_the_rest() {
        assert_eq!(redact(&args("python -m app.main --port 8765")), "python -m app.main --port 8765");
        assert_eq!(redact(&args("srv --token abc123 --x 1")), "srv --token *** --x 1");
        assert_eq!(redact(&args("srv CP_AUTH_TOKEN=deadbeef")), "srv CP_AUTH_TOKEN=***");
        assert_eq!(redact(&args("srv --api-key=k1")), "srv --api-key=***");
        assert_eq!(redact(&args("srv 9f8e7d6c5b4a39281706f5e4d3c2b1a0")), "srv ***", "valor com cara de chave");
        assert_eq!(redact(&args("/home/jefferson/.cache/hangar-voz/hangar-native")), "/home/jefferson/.cache/hangar-voz/hangar-native");
        assert_eq!(redact(&args("claude --session-id 01a1218b-bbb5-7a10-aaab-8fc7a61fcf33")), "claude --session-id ***",
            "uuid longo some por precaução; o nome do processo segue");
    }

    #[test]
    fn request_clamps_sample_and_limit() {
        let r = Request::new(Some("Hangar".into()), false, Some(60_000), Some(1000));
        assert_eq!((r.filter.as_deref(), r.sample, r.limit), (Some("hangar"), MAX_SAMPLE, MAX_LIMIT));
        assert_eq!(Request::new(None, true, Some(1), Some(0)).sample, MINIMUM_CPU_UPDATE_INTERVAL);
    }
}
