//! `frpc.toml` e `Caddyfile`. Os valores entram crus nos dois arquivos: a validação é o que impede injeção.
use std::path::Path;

use axum::http::StatusCode;

use super::state::Linked;
use super::{CONNECT_PORT, ConnectError, GUEST_PORT};

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

/// Só `<máquina>--guest.<conta>.hangar.dev.br`: o par entrega a este host o token desta máquina, e
/// `<máquina>.<conta>` é a porta do dono.
pub fn is_guest_host(host: &str) -> bool {
    let Some((machine, account)) = host.strip_suffix(".hangar.dev.br").and_then(|rest| rest.split_once('.')) else { return false };
    valid_label(machine) && valid_label(account) && machine.strip_suffix("--guest").is_some_and(valid_label)
}

/// `frps_addr` vem como `host:porta`; sem porta vale a 443.
pub fn frps_endpoint(addr: &str) -> Option<(&str, u16)> {
    let (host, port) = match addr.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok().filter(|port| *port != 0)?),
        None => (addr, 443),
    };
    valid_host(host).then_some((host, port))
}

fn frpc_proxy(host: &str) -> String {
    format!(r#"[[proxies]]
name = "{host}"
type = "https"
localIP = "127.0.0.1"
localPort = {CADDY_HTTPS_PORT}
customDomains = ["{host}"]
"#)
}

pub fn render_frpc(linked: &Linked) -> Result<String, ConnectError> {
    linked.validate()?;
    let (server, server_port) = frps_endpoint(&linked.frps_addr).ok_or_else(Linked::invalid)?;
    let (token, key, proxy) = (&linked.frps_token, &linked.machine_key, frpc_proxy(&linked.host));
    let guest = if linked.guest_published { format!("\n{}", frpc_proxy(&linked.guest_host)) } else { String::new() };
    // heartbeatInterval: com multiplexação o frpc não manda Ping, e conta vencida seguiria no ar.
    Ok(format!(r#"serverAddr = "{server}"
serverPort = {server_port}
transport.protocol = "wss"
auth.method = "token"
auth.token = "{token}"
loginFailExit = false
metadatas.key = "{key}"
transport.heartbeatInterval = 30

{proxy}{guest}"#))
}

fn caddy_site(host: &str, port: u16) -> String {
    format!("{host} {{
\tbind 127.0.0.1
\ttls {{
\t\tissuer acme {{
\t\t\tdisable_http_challenge
\t\t}}
\t}}
\treverse_proxy 127.0.0.1:{port}
}}
")
}

/// Fora do `render_caddyfile`: só entra com a 8766 aberta por este servidor, quem decide é o supervisor.
pub fn render_guest_site(linked: &Linked) -> String {
    caddy_site(&linked.guest_host, GUEST_PORT)
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
    let site = caddy_site(&linked.host, CONNECT_PORT);
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

{site}"))
}
