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
    /// `local_server_unit_unreadable`, `local_server_env_unreadable` ou `local_server_port_invalid`.
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
/// `hangar_core::user_data::path`, que este crate não importa.
pub fn env_path(home: &Path, backend: Option<&Path>) -> Option<PathBuf> {
    let new = home.join(".hangar").join("config").join(".env");
    // Disco que não deixa conferir conta como o lugar novo: é o que a subida também tentaria ler.
    let exists = |path: &Path| path.try_exists().unwrap_or(true);
    if exists(&new) || exists(&home.join(".hangar").join("current")) { return Some(new); }
    backend.map(|dir| dir.join(".env"))
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

/// `Environment=` e `WorkingDirectory=` da unit: o `CP_PORT` herdado (cru) vence o `.env`, e a pasta
/// é a `backend/` de uma máquina que ainda não migrou.
pub fn parse_unit(text: &str) -> (Option<String>, Option<PathBuf>) {
    let field = |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).map(str::trim);
    let port = field("Environment").and_then(|env| env.split_whitespace()
        .filter_map(|pair| pair.trim_matches('"').split_once('='))
        .filter(|(key, _)| key.eq_ignore_ascii_case("CP_PORT")).last()
        .map(|(_, value)| value.to_owned()));
    let dir = field("WorkingDirectory").filter(|dir| !dir.is_empty() && *dir != "[not set]").map(PathBuf::from);
    (port, dir)
}

/// A porta do serviço desta máquina. A tarefa do Windows e o agente do launchd não passam porta
/// pelo ambiente: lá vale o `.env` dos dados do usuário.
///
/// `CP_PORT` presente e fora de número (vazio incluído) faz o servidor recusar a subida
/// (`config_invalid`): não há porta certa, a leitura segue na padrão e o `Issue` diz por quê.
pub fn resolve(host: &dyn Host, os: Os, home: &Path) -> Resolved {
    let mut issue = None;
    let (inherited, backend) = match os {
        Os::Linux => match host.run("systemctl", &["--user", "show", UNIT, "--no-pager", "-p", "Environment", "-p", "WorkingDirectory"]) {
            Ok(out) if out.code == Some(0) => parse_unit(&out.stdout),
            Ok(out) => {
                issue = Some(Issue { code: "local_server_unit_unreadable", detail: format!("systemctl saiu com {:?}", out.code) });
                (None, None)
            }
            Err(e) => {
                issue = Some(Issue { code: "local_server_unit_unreadable", detail: format!("systemctl não rodou: {e}") });
                (None, None)
            }
        },
        _ => (None, None),
    };
    let raw = match inherited {
        Some(raw) => Some(raw),
        None => match env_path(home, backend.as_deref()).map(|path| (std::fs::read_to_string(&path), path)) {
            None => None,
            Some((Ok(text), _)) => env_value(&text),
            // Sem o arquivo é a máquina na porta padrão, não falha.
            Some((Err(e), _)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Some((Err(e), path)) => {
                issue = Some(Issue { code: "local_server_env_unreadable", detail: format!("{}: {e}", path.display()) });
                None
            }
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
