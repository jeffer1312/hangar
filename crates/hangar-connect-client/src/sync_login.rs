//! Login de sincronização do convidado (`/api/sync/*`) numa máquina, como o `syncSignIn` do core: só lê o
//! cofre e devolve as máquinas dele. O cookie do login dá escrita no cofre e sai logo depois da leitura.
use std::collections::HashSet;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ConnectClient, is_local, normalize_machine_url};
use crate::crypto::{Blob, decrypt};
use crate::error::{Code, Error, fail, fail_with};
use crate::transport::{Method, Request, Response, Transport};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
// O `auth_hash` vale como senha na máquina: nunca sai em http, e o custo dele não fica a critério de quem responde.
const MIN_ITERATIONS: u64 = 600_000;
const MAX_ITERATIONS: u64 = 2_000_000;
const MAX_MACHINES: usize = 100;
const MAX_PAYLOAD: usize = 262_144;

/// Uma máquina do cofre, com o token de convidado dela.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultServer {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub token: String,
}

impl<T: Transport> ConnectClient<T> {
    /// `prelogin` → chaves (PBKDF2 + HKDF, fora da thread do runtime) → `login` → `vault` → `logout` → cofre
    /// decifrado. Endereço vazio ou de loopback no cofre foi gravado de dentro da própria máquina: vira `address`.
    ///
    /// # Errors
    ///
    /// [`Code::CredentialsMismatch`] (401 no login), [`Code::SyncTooManyAttempts`] (429), [`Code::NetworkFailed`],
    /// [`Code::SyncInsecure`] (endereço http), [`Code::SyncUnavailable`] (endereço torto ou `prelogin` fora da
    /// forma), [`Code::SyncRequestFailed`] (outra recusa, com o status) e [`Code::SyncVaultUnreadable`].
    pub(crate) async fn sync_sign_in(&self, address: &str, user: &str, password: &str) -> Result<Vec<VaultServer>, Error> {
        let hub = hub_address(address)?;
        let prelogin = self.sync_request(&hub, Method::Get, &format!("/api/sync/prelogin?{}", query("user", user)), None, None).await?;
        let (salt, iterations) = read_prelogin(&prelogin)?;
        let (derive, password) = (self.derive, password.to_owned());
        let keys = tokio::task::spawn_blocking(move || derive(&password, &salt, iterations)).await
            .map_err(|_| fail(Code::SyncUnavailable))?;
        let login = self.sync_request(&hub, Method::Post, "/api/sync/login", Some(json!({"user": user, "auth_hash": keys.auth_hash})), None).await?;
        match login.status {
            429 => return Err(fail_with(Code::SyncTooManyAttempts, 429)),
            401 => return Err(fail_with(Code::CredentialsMismatch, 401)),
            status if !(200..300).contains(&status) => return Err(fail_with(Code::SyncRequestFailed, status)),
            _ => {}
        }
        let cookie = login.set_cookies.iter().filter_map(|value| value.split(';').next())
            .find(|pair| pair.trim_start().starts_with("cp_sync=")).map(|pair| pair.trim().to_owned());
        let read = self.sync_request(&hub, Method::Get, "/api/sync/vault", None, cookie.as_deref()).await;
        // Sai mesmo com a leitura falhando: a sessão não fica aberta à toa.
        let logout = self.sync_request(&hub, Method::Post, "/api/sync/logout", None, cookie.as_deref()).await;
        if let Some(code) = logout_failure(&logout) {
            (self.warn)(&format!("connect: saída do login do convidado não confirmada codigo={code}"));
        }
        let read = read?;
        if !(200..300).contains(&read.status) {
            return Err(fail_with(Code::SyncRequestFailed, read.status));
        }
        let vault = serde_json::from_slice::<Value>(&read.body).ok();
        if vault.as_ref().and_then(|vault| vault.get("enc_blob")).is_some_and(Value::is_null) {
            return Ok(Vec::new());
        }
        vault.as_ref().and_then(|vault| servers(vault, &keys.enc_key, &hub)).ok_or_else(|| fail(Code::SyncVaultUnreadable))
    }

    async fn sync_request(&self, hub: &str, method: Method, path: &str, body: Option<Value>, cookie: Option<&str>) -> Result<Response, Error> {
        let mut headers = vec![("Content-Type", "application/json".to_owned())];
        if let Some(cookie) = cookie {
            headers.push(("Cookie", cookie.to_owned()));
        }
        let request = Request {
            method, url: format!("{hub}{path}"), headers, body: body.map(|body| body.to_string().into_bytes()), timeout: REQUEST_TIMEOUT,
        };
        let response = self.transport.send(request).await?;
        if (300..400).contains(&response.status) {
            return Err(Error::network("redirecionamento recusado"));
        }
        Ok(response)
    }
}

/// O código da saída que falhou (o do erro, ou o status recusado); `None` quando saiu.
fn logout_failure(logout: &Result<Response, Error>) -> Option<String> {
    match logout {
        Ok(response) if (200..300).contains(&response.status) => None,
        Ok(response) => Some(format!("{} status={}", Code::SyncRequestFailed.as_str(), response.status)),
        Err(error) => Some(error.code().as_str().to_owned()),
    }
}

fn hub_address(address: &str) -> Result<String, Error> {
    let address = address.trim();
    let lower = address.to_ascii_lowercase();
    if lower.starts_with("http://") {
        return Err(fail(Code::SyncInsecure));
    }
    let full = if lower.starts_with("https://") { address.to_owned() } else { format!("https://{address}") };
    normalize_machine_url(&full).ok_or_else(|| fail(Code::SyncUnavailable))
}

fn read_prelogin(response: &Response) -> Result<(Vec<u8>, u32), Error> {
    let unavailable = || fail_with(Code::SyncUnavailable, response.status);
    if !(200..300).contains(&response.status) {
        return Err(unavailable());
    }
    let body: Value = serde_json::from_slice(&response.body).map_err(|_| unavailable())?;
    let salt = body.get("salt").and_then(Value::as_str).and_then(|salt| STANDARD.decode(salt).ok()).ok_or_else(unavailable)?;
    let iterations = body.get("iterations").and_then(Value::as_u64)
        .filter(|n| (MIN_ITERATIONS..=MAX_ITERATIONS).contains(n)).ok_or_else(unavailable)?;
    Ok((salt, u32::try_from(iterations).map_err(|_| unavailable())?))
}

/// Decifra e filtra como o `parseMachineImportPayload` do core; um item fora da forma invalida o cofre inteiro.
fn servers(vault: &Value, key: &[u8; 32], hub: &str) -> Option<Vec<VaultServer>> {
    let blob: Blob = serde_json::from_value(vault.get("enc_blob")?.clone()).ok()?;
    let plain = decrypt(key, &blob).filter(|plain| plain.len() <= MAX_PAYLOAD)?;
    let list: Vec<Value> = serde_json::from_slice(&plain).ok()?;
    if list.len() > MAX_MACHINES {
        return None;
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in list {
        let object = item.as_object()?;
        let flag = |name: &str| match object.get(name) {
            None => Some(false),
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => None,
        };
        if flag("disabled")? {
            continue;
        }
        if flag("invite")? | flag("ephemeral")? | flag("removed")? | flag("guest")? {
            continue;
        }
        let raw = object.get("baseUrl")?.as_str()?;
        let raw = if raw.is_empty() || normalize_machine_url(raw).and_then(|url| url::Url::parse(&url).ok()).is_some_and(|url| is_local(&url)) {
            hub
        } else {
            raw
        };
        let (id, label, token) = (text(object.get("id")?, 128)?, text(object.get("label")?, 256)?, text(object.get("token")?, 8192)?);
        if id.chars().any(char::is_whitespace) || token.chars().any(char::is_whitespace) {
            return None;
        }
        let base_url = normalize_machine_url(raw)?;
        if !seen.insert(base_url.clone()) {
            continue;
        }
        out.push(VaultServer { id: id.to_owned(), label: label.to_owned(), base_url, token: token.to_owned() });
    }
    Some(out)
}

fn text(value: &Value, max: usize) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty() && text.encode_utf16().count() <= max && !text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}'))
}

fn query(key: &str, value: &str) -> String {
    format!("{key}={}", crate::client::segment(value))
}
