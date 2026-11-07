//! `frpc.toml` e `Caddyfile`. Os valores entram crus nos dois arquivos: a validação é o que impede injeção.
use std::path::Path;

use axum::http::StatusCode;

use super::state::Linked;
use super::{CONNECT_PORT, ConnectError};

// Fixas: porta sorteada pode ser tomada antes de o Caddy subir, e `kill_leftover` solta justamente estas.
pub const CADDY_HTTP_PORT: u16 = 18080;
pub const CADDY_HTTPS_PORT: u16 = 18443;

pub fn valid_label(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=63).contains(&bytes.len())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && bytes[0] != b'-' && bytes[bytes.len() - 1] != b'-'
}

pub fn valid_host(value: &str) -> bool {
    value.len() <= 253 && value.contains('.') && value.split('.').all(valid_label)
}

pub fn valid_secret(value: &str) -> bool {
    (16..=256).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b))
}

/// `frps_addr` vem como `host:porta`; sem porta vale a 443.
pub fn frps_endpoint(addr: &str) -> Option<(&str, u16)> {
    let (host, port) = match addr.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok().filter(|port| *port != 0)?),
        None => (addr, 443),
    };
    valid_host(host).then_some((host, port))
}

pub fn render_frpc(linked: &Linked) -> Result<String, ConnectError> {
    linked.validate()?;
    let (server, server_port) = frps_endpoint(&linked.frps_addr).ok_or_else(Linked::invalid)?;
    let (token, key, host) = (&linked.frps_token, &linked.machine_key, &linked.host);
    // heartbeatInterval: com multiplexação o frpc não manda Ping, e conta vencida seguiria no ar.
    Ok(format!(r#"serverAddr = "{server}"
serverPort = {server_port}
transport.protocol = "wss"
auth.method = "token"
auth.token = "{token}"
loginFailExit = false
metadatas.key = "{key}"
transport.heartbeatInterval = 30

[[proxies]]
name = "{host}"
type = "https"
localIP = "127.0.0.1"
localPort = {CADDY_HTTPS_PORT}
customDomains = ["{host}"]
"#))
}

/// `disable_http_challenge`: a porta 80 da VPS nunca chega aqui, e cada HTTP-01 sorteado gasta o limite de falhas.
pub fn render_caddyfile(linked: &Linked, dir: &Path) -> Result<String, ConnectError> {
    linked.validate()?;
    let storage = dir.join("caddy").to_string_lossy().replace('\\', "/");
    // `{` abre placeholder do Caddyfile mesmo entre aspas.
    if storage.contains(['"', '\n', '\r', '{', '}']) {
        return Err(ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_path_invalid",
            format!("a pasta do Connect não cabe no Caddyfile: {storage}")));
    }
    let host = &linked.host;
    Ok(format!("{{
\tadmin off
\thttp_port {CADDY_HTTP_PORT}
\thttps_port {CADDY_HTTPS_PORT}
\tauto_https disable_redirects
\tstorage file_system \"{storage}\"
\tlog {{
\t\tlevel WARN
\t}}
}}

{host} {{
\tbind 127.0.0.1
\ttls {{
\t\tissuer acme {{
\t\t\tdisable_http_challenge
\t\t}}
\t}}
\treverse_proxy 127.0.0.1:{CONNECT_PORT}
}}
"))
}
