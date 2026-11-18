//! A porta em que o serviço sobe o servidor, resolvida como o `hangar-server` a resolve na partida
//! (`hangar_boot::settings::Settings::resolve`): `CP_PORT` do ambiente do processo (o `Environment=`
//! da unit), senão a do `.env` dos dados do usuário, senão a padrão. A porta da conexão do app pode
//! ser outra: ler e agir pela dela confundiria a unit de pé com um servidor parado.
use std::path::{Path, PathBuf};

use crate::host::Host;
use crate::service::{Os, UNIT};
use crate::DEFAULT_PORT;

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

/// O `CP_PORT` de um `.env` no formato que o servidor lê: chave sem caixa, a última linha vale,
/// aspas fecham o valor e `#` depois de espaço é comentário. Valor que não é porta: `None`, como
/// o servidor que recusaria subir com ele.
pub fn port_in_env(text: &str) -> Option<u16> {
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
    value.trim().parse().ok()
}

/// `Environment=` e `WorkingDirectory=` da unit: a porta herdada vence o `.env`, e a pasta é a
/// `backend/` de uma máquina que ainda não migrou.
pub fn parse_unit(text: &str) -> (Option<u16>, Option<PathBuf>) {
    let field = |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).map(str::trim);
    let port = field("Environment").and_then(|env| env.split_whitespace()
        .filter_map(|pair| pair.trim_matches('"').split_once('='))
        .filter(|(key, _)| key.eq_ignore_ascii_case("CP_PORT")).last()
        .and_then(|(_, value)| value.parse().ok()));
    let dir = field("WorkingDirectory").filter(|dir| !dir.is_empty() && *dir != "[not set]").map(PathBuf::from);
    (port, dir)
}

/// A porta do serviço desta máquina. A tarefa do Windows e o agente do launchd não passam porta
/// pelo ambiente: lá vale o `.env` dos dados do usuário.
pub fn service_port(host: &dyn Host, os: Os, home: &Path) -> u16 {
    let (inherited, backend) = match os {
        Os::Linux => host.run("systemctl", &["--user", "show", UNIT, "--no-pager", "-p", "Environment", "-p", "WorkingDirectory"])
            .ok().filter(|out| out.code == Some(0)).map(|out| parse_unit(&out.stdout)).unwrap_or_default(),
        _ => (None, None),
    };
    if let Some(port) = inherited { return port; }
    env_path(home, backend.as_deref()).and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| port_in_env(&text)).unwrap_or(DEFAULT_PORT)
}
