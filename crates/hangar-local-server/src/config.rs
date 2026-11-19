//! A porta em que o serviço sobe o servidor, resolvida como o `hangar-server` a resolve na partida
//! (`hangar_boot::settings::Settings::resolve`): `CP_PORT` do ambiente do processo (o `Environment=`
//! da unit), senão a do `.env` dos dados do usuário, senão a padrão. A porta da conexão do app pode
//! ser outra: ler e agir pela dela confundiria a unit de pé com um servidor parado.
use std::path::{Path, PathBuf};

use crate::host::Host;
use crate::service::{Os, UNIT};
use crate::DEFAULT_PORT;

/// O que impediu de ler a porta do jeito que a subida lê. A porta usada segue a regra de cima com o
/// que deu para ler; o código vai ao log de quem chama.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// `local_server_unit_unreadable`, `local_server_env_unreadable`, `local_server_env_file_unreadable`
    /// ou `local_server_port_invalid`.
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub port: u16,
    pub issue: Option<Issue>,
}

/// O `.env` que a subida lê: `~/.hangar/config/.env` quando existe ou a máquina migrou
/// (`~/.hangar/current`); senão o da pasta `backend/` do serviço. A regra do
/// `hangar_core::user_data::path`, que este crate não importa. Disco que não deixa conferir é erro,
/// como lá: a subida recusa, e a leitura daqui fica com ressalva.
pub fn env_path(home: &Path, backend: Option<&Path>) -> std::io::Result<Option<PathBuf>> {
    let new = home.join(".hangar").join("config").join(".env");
    if new.try_exists()? || home.join(".hangar").join("current").try_exists()? { return Ok(Some(new)); }
    Ok(backend.map(|dir| dir.join(".env")))
}

/// O valor cru do `CP_PORT` de um `.env` no formato que o servidor lê: chave sem caixa, a última
/// linha vale, aspas fecham o valor e `#` depois de espaço é comentário. `None` sem a chave.
pub fn env_value(text: &str) -> Option<String> {
    let raw = text.lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with('#') { return None; }
        let (key, value) = line.split_once('=')?;
        key.trim().eq_ignore_ascii_case("CP_PORT").then(|| value.trim())
    }).last()?;
    let value = match raw.chars().next() {
        Some(quote @ ('"' | '\'')) => raw[1..].split(quote).next().unwrap_or_default(),
        _ => raw.char_indices().find(|&(at, c)| c == '#' && raw[..at].ends_with(char::is_whitespace)).map_or(raw, |(at, _)| &raw[..at]),
    };
    Some(value.trim().to_owned())
}

/// A leitura da unit que decide a porta.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unit {
    /// O `CP_PORT` cru do `Environment=`: vence o `.env`.
    pub port: Option<String>,
    /// A pasta `backend/` de uma máquina que ainda não migrou.
    pub dir: Option<PathBuf>,
    /// `EnvironmentFile=` na ordem da unit; `true` quando o arquivo pode faltar (`-` na frente).
    pub env_files: Vec<(PathBuf, bool)>,
}

/// `Environment=`, `EnvironmentFiles=` e `WorkingDirectory=` do `systemctl show`.
pub fn parse_unit(text: &str) -> Unit {
    let field = |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).map(str::trim);
    let port = field("Environment").and_then(|env| assignments(env).into_iter()
        .filter_map(|pair| pair.split_once('=').map(|(key, value)| (key.to_owned(), value.to_owned())))
        .filter(|(key, _)| key.eq_ignore_ascii_case("CP_PORT")).last()
        .map(|(_, value)| value));
    let dir = field("WorkingDirectory").filter(|dir| !dir.is_empty() && *dir != "[not set]").map(PathBuf::from);
    // Uma linha por arquivo: `EnvironmentFiles=/caminho (ignore_errors=no)`.
    let env_files = text.lines().filter_map(|line| line.trim().strip_prefix("EnvironmentFiles=")).filter(|v| !v.is_empty())
        .map(|value| match value.rsplit_once(" (ignore_errors=") {
            Some((path, flag)) => (PathBuf::from(path), flag.starts_with("yes")),
            None => (PathBuf::from(value), false),
        }).collect();
    Unit { port, dir, env_files }
}

/// As atribuições de uma linha `Environment=`: o `systemctl show` põe entre aspas duplas (com `\`
/// antes de `"`, `\`, `$` e crase) a que tem espaço, e cortar no espaço leria o pedaço de outra variável.
fn assignments(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => { quoted = !quoted; started = true; }
            '\\' if quoted => { if let Some(next) = chars.next() { word.push(next); } }
            c if c.is_whitespace() && !quoted => {
                if started { found.push(std::mem::take(&mut word)); started = false; }
            }
            c => { word.push(c); started = true; }
        }
    }
    if started { found.push(word); }
    found
}

/// A porta do serviço desta máquina. A tarefa do Windows e o agente do launchd não passam porta
/// pelo ambiente: lá vale o `.env` dos dados do usuário.
///
/// `CP_PORT` presente e fora de número (vazio incluído) faz o servidor recusar a subida
/// (`config_invalid`): não há porta certa, a leitura segue na padrão e o `Issue` diz por quê.
pub fn resolve(host: &dyn Host, os: Os, home: &Path) -> Resolved {
    let mut issue = None;
    let unit = match os {
        Os::Linux => match host.run("systemctl", &["--user", "show", UNIT, "--no-pager", "-p", "Environment", "-p", "EnvironmentFiles", "-p", "WorkingDirectory"]) {
            // Saída cortada pode não trazer o `Environment=`: não confirma a porta.
            Ok(out) if out.code == Some(0) && !out.timed_out => parse_unit(&out.stdout),
            Ok(out) => {
                issue = Some(Issue { code: "local_server_unit_unreadable", detail: format!("systemctl saiu com {:?} (cortada: {})", out.code, out.timed_out) });
                Unit::default()
            }
            Err(e) => {
                issue = Some(Issue { code: "local_server_unit_unreadable", detail: format!("systemctl não rodou: {e}") });
                Unit::default()
            }
        },
        _ => Unit::default(),
    };
    // O systemd aplica os `EnvironmentFile=` por cima do `Environment=`, o último arquivo vencendo.
    let mut inherited = unit.port;
    for (path, optional) in &unit.env_files {
        match std::fs::read_to_string(path) {
            Ok(text) => if let Some(value) = env_value(&text) { inherited = Some(value); },
            Err(e) if *optional && e.kind() == std::io::ErrorKind::NotFound => {}
            // Sem ler o arquivo, a porta lida pode não ser a que o serviço recebe.
            Err(e) => issue = Some(Issue { code: "local_server_env_file_unreadable", detail: format!("{}: {e}", path.display()) }),
        }
    }
    let backend = unit.dir;
    let raw = match inherited {
        Some(raw) => Some(raw),
        None => match env_path(home, backend.as_deref()) {
            Err(e) => {
                issue = Some(Issue { code: "local_server_env_unreadable", detail: format!("{}: {e}", home.join(".hangar").display()) });
                None
            }
            Ok(None) => None,
            Ok(Some(path)) => match std::fs::read_to_string(&path) {
                Ok(text) => env_value(&text),
                // Sem o arquivo é a máquina na porta padrão, não falha.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    issue = Some(Issue { code: "local_server_env_unreadable", detail: format!("{}: {e}", path.display()) });
                    None
                }
            },
        },
    };
    let port = match raw {
        None => DEFAULT_PORT,
        Some(raw) => raw.trim().parse().unwrap_or_else(|_| {
            issue = Some(Issue { code: "local_server_port_invalid", detail: format!("CP_PORT={raw:?}") });
            DEFAULT_PORT
        }),
    };
    if let Some(issue) = &issue { tracing::warn!(code = issue.code, detail = %issue.detail, port, "porta do serviço lida com ressalva"); }
    Resolved { port, issue }
}
