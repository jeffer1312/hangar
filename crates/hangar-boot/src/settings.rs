//! A configuração que o Python montava para o filho: o `.env` dos dados do usuário e o ambiente do
//! processo, o processo vencendo e a chave sem caixa (o pydantic com `env_prefix="CP_"`).
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;

pub const GUEST_PORT: u16 = 8766;
const DEFAULT_TOKEN: &str = "change-me";
const LOOPBACK: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// O `.env` no que o Hangar grava: `CHAVE=valor`, a última linha vale, aspas fecham o valor e `#`
/// depois de espaço é comentário. Sem `export` nem `${VAR}`; a chave volta em minúsculas.
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { return None; }
        let (key, value) = line.split_once('=')?;
        Some((key.trim().to_ascii_lowercase(), unquote(value.trim())))
    }).collect()
}

fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            return rest.find(quote).map_or(rest, |end| &rest[..end]).to_owned();
        }
    }
    let cut = value.char_indices().find(|&(at, c)| c == '#' && value[..at].ends_with(char::is_whitespace))
        .map_or(value.len(), |(at, _)| at);
    value[..cut].trim_end().to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub bind: String,
    pub port: u16,
    pub token: String,
    pub forwarded: String,
}

impl Settings {
    pub fn resolve(env_file: Option<&str>, environ: &[(String, String)]) -> Result<Settings, String> {
        let file = env_file.map(parse_env_file).unwrap_or_default();
        let get = |name: &str| environ.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
            .or_else(|| file.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()));
        let port = match get("CP_PORT") {
            None => 8765,
            Some(raw) => raw.trim().parse().map_err(|_| format!("CP_PORT inválido: {raw:?}"))?,
        };
        Ok(Settings {
            bind: get("CP_LAN_BIND_IP").unwrap_or_else(|| "127.0.0.1".into()),
            port,
            token: get("CP_AUTH_TOKEN").unwrap_or_else(|| DEFAULT_TOKEN.into()),
            forwarded: get("CP_FORWARDED_ALLOW_IPS").unwrap_or_else(|| "127.0.0.1".into()),
        })
    }
}

/// O que a subida põe no ambiente antes do runtime, e o aviso que o Python imprimia.
#[derive(Debug)]
pub struct Boot {
    pub vars: Vec<(&'static str, String)>,
    pub warning: Option<String>,
}

pub struct Places<'a> {
    pub home: &'a Path,
    /// `CLAUDE_CONFIG_DIR`; vazio vale ausente.
    pub claude_config: Option<&'a Path>,
    pub log_base: Option<&'a Path>,
}

/// `present`: a variável já está no ambiente do processo, e ele vence.
pub fn standalone_env(settings: &Settings, places: &Places, present: impl Fn(&str) -> bool) -> Result<Boot, String> {
    let warning = guard(settings)?;
    let ip = bind_ip(&settings.bind)?;
    let mut vars = vec![
        ("CP_AUTH_TOKEN", settings.token.clone()),
        ("CP_FORWARDED_ALLOW_IPS", settings.forwarded.clone()),
        ("HANGAR_SERVER_LISTEN", SocketAddr::new(ip, settings.port).to_string()),
        ("HANGAR_INTERNAL_SECRET", hangar_core::ids::random_hex(32)),
        ("HANGAR_RUNTIME_INSTANCE", hangar_core::ids::random_hex(16)),
    ];
    // Com o backend aberto para a rede, o convite também vale na casa; senão só pelo Funnel (loopback).
    if settings.port != GUEST_PORT {
        let host = if ip.is_unspecified() { Ipv4Addr::UNSPECIFIED } else { Ipv4Addr::LOCALHOST };
        vars.push(("HANGAR_SERVER_GUEST_LISTEN", SocketAddr::from((host, GUEST_PORT)).to_string()));
    }
    let claude = places.claude_config.filter(|dir| !dir.as_os_str().is_empty()).map(Path::to_path_buf)
        .unwrap_or_else(|| places.home.join(".claude"));
    vars.push(("HANGAR_SHARES_FILE", claude.join("shares.json").to_string_lossy().into_owned()));
    if let Some(base) = places.log_base {
        let folder = base.join("privado");
        create_private_dir(&folder).map_err(|e| format!("pasta de logs {}: {e}", folder.display()))?;
        vars.push(("HANGAR_SERVER_LOG", folder.join("hangar-server.log").to_string_lossy().into_owned()));
    }
    vars.retain(|(key, _)| key.starts_with("CP_") || !present(key));
    Ok(Boot { vars, warning })
}

fn guard(settings: &Settings) -> Result<Option<String>, String> {
    if settings.token != DEFAULT_TOKEN { return Ok(None); }
    if !LOOPBACK.contains(&settings.bind.as_str()) {
        return Err(format!("Refusing to start: CP_AUTH_TOKEN is still the default 'change-me' while binding {}. \
            Set CP_AUTH_TOKEN to a strong secret, or bind 127.0.0.1 for local dev.", settings.bind));
    }
    Ok(Some("WARNING: using the default 'change-me' token on loopback. Set CP_AUTH_TOKEN before exposing this on your LAN.".into()))
}

/// `auto` vira o IP da rede local; nome de host vira IP (IPv4 antes), porque a escuta só aceita IP.
pub fn bind_ip(bind: &str) -> Result<IpAddr, String> {
    if bind == "auto" { return Ok(detect_lan_ip()); }
    if let Ok(ip) = bind.parse::<IpAddr>() { return Ok(ip); }
    let found: Vec<SocketAddr> = (bind, 0).to_socket_addrs().map(Iterator::collect).unwrap_or_default();
    found.iter().find(|addr| addr.is_ipv4()).or(found.first()).map(SocketAddr::ip)
        .ok_or_else(|| format!("CP_LAN_BIND_IP sem endereço: {bind}"))
}

/// A interface por onde sairia o tráfego para fora; nenhum pacote é enviado.
pub fn detect_lan_ip() -> IpAddr {
    let probe = || -> std::io::Result<IpAddr> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect("8.8.8.8:80")?;
        Ok(socket.local_addr()?.ip())
    };
    probe().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { use std::os::unix::fs::DirBuilderExt; std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) }
    #[cfg(not(unix))]
    { std::fs::create_dir_all(dir) }
}
