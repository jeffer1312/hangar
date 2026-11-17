//! A configuração que o Python montava para o filho: o `.env` dos dados do usuário e o ambiente do
//! processo, o processo vencendo e a chave sem caixa (o pydantic com `env_prefix="CP_"`).
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;

pub const GUEST_PORT: u16 = 8766;
const DEFAULT_TOKEN: &str = "change-me";
const LOOPBACK: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// O leitor do `.env` é um só, no `hangar-core`: a subida e quem consulta uma chave depois leem igual.
pub use hangar_core::user_data::parse_env_file;

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

/// No Windows o nome da variável não tem caixa, mas o servidor lê o ambiente como mapa (`PATH`,
/// `PATHEXT`): as chaves sobem em maiúsculas, como o `os.environ` do Python as passava quando ele
/// abria o servidor. `(de, para)` de cada chave a trocar; as ocultas de unidade (`=C:`) ficam.
pub fn uppercase_renames<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    keys.into_iter().filter(|key| !key.starts_with('='))
        .filter_map(|key| { let upper = key.to_uppercase(); (upper != key).then(|| (key.to_owned(), upper)) }).collect()
}

/// As chaves internas que a subida decide. O ambiente herdado (unit, sessão que abriu o servidor à
/// mão) nunca vence: o Python também as regravava e tirava a porta do convidado que não montou.
pub const INTERNAL: [&str; 6] = ["HANGAR_SERVER_LISTEN", "HANGAR_SERVER_GUEST_LISTEN", "HANGAR_SERVER_LOG",
    "HANGAR_SHARES_FILE", "HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE"];

/// Só os testes, que sobem o servidor com porta, segredo e log próprios: com `1`, a chave interna
/// já presente no ambiente fica.
pub const KEEP_INHERITED: &str = "HANGAR_TEST_KEEP_INTERNAL_ENV";

/// O que a subida põe no ambiente antes do runtime, e o aviso que o Python imprimia.
#[derive(Debug)]
pub struct Boot {
    pub vars: Vec<(&'static str, String)>,
    /// Chaves internas herdadas que a subida não montou: saem do ambiente.
    pub remove: Vec<&'static str>,
    pub warning: Option<String>,
    /// O que a subida resolveu diferente do pedido, com código, para o stderr: o log ainda não abriu.
    pub notes: Vec<String>,
}

pub struct Places<'a> {
    pub home: &'a Path,
    /// `CLAUDE_CONFIG_DIR`; vazio vale ausente.
    pub claude_config: Option<&'a Path>,
    pub log_base: Option<&'a Path>,
}

/// `kept`: a chave interna herdada fica (só com `KEEP_INHERITED`, nos testes). As `CP_*` resolvidas
/// sempre vão: a precedência `.env`/processo delas já foi decidida em `Settings::resolve`.
pub fn standalone_env(settings: &Settings, places: &Places, kept: impl Fn(&str) -> bool) -> Result<Boot, String> {
    let warning = guard(settings)?;
    let mut notes = Vec::new();
    let ip = if settings.bind == "auto" {
        lan_ip().unwrap_or_else(|error| {
            notes.push(format!("bind_auto_fallback: CP_LAN_BIND_IP=auto sem interface de rede ({:?}); escutando só em 127.0.0.1", error.kind()));
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        })
    } else {
        bind_ip(&settings.bind)?
    };
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
    // Fora dos testes a chave mantida esconderia uma configuração errada: fica dito no stderr.
    for (key, _) in vars.iter().filter(|(key, _)| !key.starts_with("CP_") && kept(key)) {
        notes.push(format!("internal_env_kept: {key} herdada mantida por {KEEP_INHERITED}"));
    }
    vars.retain(|(key, _)| key.starts_with("CP_") || !kept(key));
    let remove = INTERNAL.into_iter().filter(|key| !kept(*key) && !vars.iter().any(|(set, _)| set == key)).collect();
    Ok(Boot { vars, remove, warning, notes })
}

fn guard(settings: &Settings) -> Result<Option<String>, String> {
    // Token vazio abriria a API a quem não manda token nenhum.
    if settings.token.is_empty() {
        return Err("Refusing to start: CP_AUTH_TOKEN is empty. Set CP_AUTH_TOKEN to a strong secret.".into());
    }
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
    let found: Vec<SocketAddr> = (bind, 0).to_socket_addrs().map(Iterator::collect)
        .map_err(|error| format!("CP_LAN_BIND_IP sem endereço: {bind} ({error})"))?;
    found.iter().find(|addr| addr.is_ipv4()).or(found.first()).map(SocketAddr::ip)
        .ok_or_else(|| format!("CP_LAN_BIND_IP sem endereço: {bind}"))
}

/// A interface por onde sairia o tráfego para fora; nenhum pacote é enviado. Sem rede, loopback:
/// quem precisa dizer que caiu nele usa `lan_ip`.
pub fn detect_lan_ip() -> IpAddr {
    lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

pub fn lan_ip() -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { use std::os::unix::fs::DirBuilderExt; std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) }
    #[cfg(not(unix))]
    { std::fs::create_dir_all(dir) }
}
