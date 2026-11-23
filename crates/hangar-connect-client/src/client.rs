//! O cliente e o pedido comum ao serviço e às máquinas, como o `request` de `connectAccount.ts`.
use std::time::Duration;

use serde_json::Value;
use url::{Host, Url};

use crate::crypto::{Keys, derive_keys};
use crate::error::{Code, Error, fail, fail_with};
use crate::transport::{Method, Request, Transport};

pub const CONNECT_SERVICE_URL: &str = "https://connect.hangar.dev.br";

/// O domínio do Connect: toda máquina que o serviço indica mora abaixo dele.
pub(crate) const CONNECT_DOMAIN: &str = "hangar.dev.br";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Conta do Connect, liberação do aparelho, entrada nas máquinas e login do convidado, sem servidor local.
pub struct ConnectClient<T> {
    pub(crate) transport: T,
    pub(crate) timeout: Duration,
    /// PBKDF2 de 600 mil voltas: as provas trocam por uma conta barata.
    pub(crate) derive: fn(&str, &[u8], u32) -> Keys,
    /// Falha de fundo que não muda o resultado, como a saída do login do convidado: leva só o código.
    pub(crate) warn: fn(&str),
}

fn tracing_warn(line: &str) {
    tracing::warn!("{line}");
}

impl<T: Transport> ConnectClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport, timeout: REQUEST_TIMEOUT, derive: derive_keys, warn: tracing_warn }
    }

    /// Para onde vão as falhas de fundo (padrão: `tracing::warn!`); o app nativo não tem assinante do `tracing`.
    pub fn on_warning(mut self, warn: fn(&str)) -> Self {
        self.warn = warn;
        self
    }

    /// Prazo de cada pedido ao serviço e às máquinas (padrão 15 s). O login do convidado tem o dele.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub(crate) async fn call(&self, base: &str, path: &str, method: Method, body: Option<Value>,
        headers: Vec<(&'static str, String)>) -> Result<Value, Error> {
        let url = Url::parse(base).map_err(|_| fail(Code::ConnectRequestFailed))?;
        // A senha da conta passa por aqui: http só em loopback, que é o caso das provas.
        if url.scheme() != "https" && !is_local(&url) {
            return Err(fail(Code::ConnectServiceInsecure));
        }
        let mut all = vec![("Content-Type", "application/json".to_owned())];
        all.extend(headers);
        let response = self.transport.send(Request {
            method,
            url: format!("{}{path}", base.trim_end_matches('/')),
            headers: all,
            body: body.map(|body| body.to_string().into_bytes()),
            timeout: self.timeout,
        }).await?;
        let status = response.status;
        if (300..400).contains(&status) {
            return Err(Error::network("redirecionamento recusado"));
        }
        if !(200..300).contains(&status) {
            // Do corpo do erro só sai o motivo conhecido, nunca o texto: ele pode ecoar o que recebeu.
            let reason = serde_json::from_slice::<Value>(&response.body).ok()
                .and_then(|body| body.get("reason").and_then(Value::as_str).map(str::to_owned));
            let code = match reason.as_deref() {
                Some("device_required") => Code::ConnectDeviceRequired,
                Some("device_pending") => Code::ConnectDevicePending,
                Some("device_revoked") => Code::ConnectDeviceRevoked,
                _ => Code::ConnectRequestFailed,
            };
            return Err(fail_with(code, status));
        }
        if status == 204 {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|_| fail(Code::ConnectResponseInvalid))
    }

    pub(crate) async fn post(&self, base: &str, path: &str, body: Value, headers: Vec<(&'static str, String)>) -> Result<Value, Error> {
        self.call(base, path, Method::Post, Some(body), headers).await
    }
}

/// Texto não vazio do campo, como o `filled` do core.
pub(crate) fn filled<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty())
}

/// `encodeURIComponent`.
pub(crate) fn segment(raw: &str) -> String {
    raw.bytes().map(|byte| match byte {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (byte as char).to_string(),
        _ => format!("%{byte:02X}"),
    }).collect()
}

/// O `isLocalMachineUrl` do core: endereço que não sai da máquina.
pub(crate) fn is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified()
            || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback() || ip.is_unspecified()),
        None => false,
    }
}

/// O `normalizeMachineUrl` do core: http(s) com host, sem usuário, consulta, fragmento nem barra no fim.
pub(crate) fn normalize_machine_url(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.len() > 2048 || value.contains(['?', '#', '\\']) || value.chars().any(char::is_whitespace) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    let host = url.host_str().filter(|host| !host.is_empty());
    if !matches!(url.scheme(), "http" | "https") || host.is_none() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segment_is_encoded_like_encode_uri_component() {
        assert_eq!(segment("a b/ç?"), "a%20b%2F%C3%A7%3F");
        assert_eq!(segment("dev-1_x.~*'()!"), "dev-1_x.~*'()!");
    }

    #[test]
    fn machine_url_is_normalized_like_the_core() {
        assert_eq!(normalize_machine_url(" https://pc.ana.hangar.dev.br/ ").as_deref(), Some("https://pc.ana.hangar.dev.br"));
        assert_eq!(normalize_machine_url("https://pc.ana.hangar.dev.br/x//").as_deref(), Some("https://pc.ana.hangar.dev.br/x"));
        for bad in ["endereço torto", "ftp://pc.dev", "https://u:p@pc.dev", "https://pc.dev/?a=1", "https://pc.dev/#x", ""] {
            assert_eq!(normalize_machine_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn loopback_hosts_are_local() {
        for local in ["http://localhost:1", "http://a.localhost", "http://127.0.0.2", "http://[::1]", "http://0.0.0.0", "http://[::ffff:127.0.0.1]"] {
            assert!(is_local(&Url::parse(local).unwrap()), "{local}");
        }
        assert!(!is_local(&Url::parse("http://192.168.0.2").unwrap()));
        assert!(!is_local(&Url::parse("http://connect.hangar.dev.br").unwrap()));
    }
}
