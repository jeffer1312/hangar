//! Conta do Connect: login, nome (conta ou convidado), máquinas e a liberação única deste aparelho.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{CONNECT_DOMAIN, ConnectClient, filled, segment};
use crate::error::{Code, Error, fail, fail_with};
use crate::sync_login::VaultServer;
use crate::transport::{Method, Transport};

/// Credencial deste aparelho na conta (`POST /v1/devices`); vai ao serviço, nunca à máquina.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKey {
    pub id: String,
    pub secret: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSession {
    pub service_url: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectMachine {
    pub machine_id: String,
    pub name: String,
    pub host: String,
    pub online: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    Pending,
    Approved,
    Revoked,
}

/// Linha de `GET /v1/devices`; tempos em segundos, com fração (o serviço grava o relógio inteiro).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountDevice {
    pub device_id: String,
    pub name: String,
    /// `pending`, `approved` ou `revoked`. Texto: um estado novo no serviço não derruba a lista toda.
    pub status: String,
    /// `machine` (PC da conta) ou `device`; serviço antigo não manda e todo mundo é aparelho.
    #[serde(default = "device_kind")]
    pub kind: String,
    pub created_at: f64,
    #[serde(default)]
    pub approved_at: Option<f64>,
    #[serde(default)]
    pub revoked_at: Option<f64>,
    /// O código que o aparelho mostra, pronto do serviço (que nunca manda o `install_id`); ausente em serviço antigo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_code: Option<String>,
}

impl AccountDevice {
    pub fn is_revoked(&self) -> bool {
        self.status == "revoked"
    }
}

fn device_kind() -> String {
    "device".to_owned()
}

/// O que o serviço entrega ao registrar uma máquina (`POST /v1/machines`). A chave troca a cada registro.
#[derive(Clone, Deserialize)]
pub struct MachineRegistration {
    pub machine_id: String,
    pub machine_key: String,
    pub host: String,
    pub guest_host: String,
    pub preview_host: String,
    pub frps_addr: String,
    pub frps_token: String,
    pub service_public_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NameLookup {
    Account,
    /// As máquinas que guardam o cofre do convidado, sem esquema.
    Guest { hosts: Vec<String> },
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInResult {
    Account(AccountSession),
    Guest { host: String, servers: Vec<VaultServer> },
}

impl<T: Transport> ConnectClient<T> {
    /// Pergunta ao serviço só o nome: conta, convidado (com as máquinas do cofre dele) ou nada.
    ///
    /// # Errors
    ///
    /// [`Code::ConnectTooManyAttempts`] no 429, [`Code::ConnectResponseInvalid`] para resposta fora da forma e os de
    /// qualquer pedido ([`Code::NetworkFailed`], [`Code::ConnectRequestFailed`]). Serviço sem o diretório (404) é conta.
    pub(crate) async fn lookup_name(&self, service_url: &str, name: &str) -> Result<NameLookup, Error> {
        let answer = match self.post(service_url, "/v1/lookup", json!({"name": normalize_name(name)}), Vec::new()).await {
            Ok(answer) => answer,
            Err(error) if error.status() == Some(404) => return Ok(NameLookup::Account),
            Err(error) if error.status() == Some(429) => return Err(fail_with(Code::ConnectTooManyAttempts, 429)),
            Err(error) => return Err(error),
        };
        match answer.get("kind").and_then(Value::as_str) {
            Some("account") => Ok(NameLookup::Account),
            Some("none") => Ok(NameLookup::None),
            Some("guest") => {
                let hosts = answer.get("hosts").and_then(Value::as_array)
                    .and_then(|hosts| hosts.iter()
                        .map(|host| host.as_str().filter(|host| valid_hostname(host) && in_connect_domain(host)).map(str::to_owned))
                        .collect::<Option<Vec<_>>>())
                    .filter(|hosts| !hosts.is_empty());
                hosts.map(|hosts| NameLookup::Guest { hosts }).ok_or_else(|| fail(Code::ConnectResponseInvalid))
            }
            _ => Err(fail(Code::ConnectResponseInvalid)),
        }
    }

    /// Entrada pelo campo único "conta ou usuário". A senha da convidada nunca vai ao serviço: ele só recebe o
    /// nome, e o login dela roda numa máquina de cada vez, parando na primeira que aceita.
    ///
    /// # Errors
    ///
    /// [`Code::CredentialsMismatch`] para nome desconhecido ou senha recusada (pela conta ou por todas as máquinas),
    /// [`Code::SyncTooManyAttempts`] quando o limite foi o que recusou, a primeira outra falha de máquina (rede vem
    /// antes de "não confere": a máquina fora do ar pode ser a que aceitaria) e os da busca do nome no serviço.
    pub async fn sign_in_anywhere(&self, service_url: &str, name: &str, password: &str) -> Result<SignInResult, Error> {
        let user = normalize_name(name);
        let hosts = match self.lookup_name(service_url, &user).await? {
            NameLookup::None => return Err(fail(Code::CredentialsMismatch)),
            NameLookup::Account => {
                return match self.sign_in(service_url, &user, password).await {
                    Ok(session) => Ok(SignInResult::Account(session)),
                    Err(error) if error.status() == Some(401) => Err(fail_with(Code::CredentialsMismatch, 401)),
                    Err(error) => Err(error),
                };
            }
            NameLookup::Guest { hosts } => hosts,
        };
        let mut failures = Vec::new();
        for host in hosts {
            match self.sync_sign_in(&format!("https://{host}"), &user, password).await {
                Ok(servers) => return Ok(SignInResult::Guest { host, servers }),
                Err(error) => failures.push(error),
            }
        }
        let quiet = |code: Code| matches!(code, Code::CredentialsMismatch | Code::SyncTooManyAttempts);
        if let Some(other) = failures.iter().find(|error| !quiet(error.code())) {
            return Err(other.clone());
        }
        if failures.iter().any(|error| error.code() == Code::SyncTooManyAttempts) {
            return Err(fail_with(Code::SyncTooManyAttempts, 429));
        }
        Err(fail_with(Code::CredentialsMismatch, 401))
    }

    /// `POST /v1/sessions`.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido (401 = senha recusada, com o status) e [`Code::ConnectResponseInvalid`] sem `token`.
    pub async fn sign_in(&self, service_url: &str, name: &str, password: &str) -> Result<AccountSession, Error> {
        self.account_session(service_url, "/v1/sessions", name, password).await
    }

    /// `POST /v1/accounts`: a conta nasce e já devolve a sessão, no formato do login.
    ///
    /// # Errors
    ///
    /// Como [`Self::sign_in`]; 409 = nome já usado.
    pub async fn sign_up(&self, service_url: &str, name: &str, password: &str) -> Result<AccountSession, Error> {
        self.account_session(service_url, "/v1/accounts", name, password).await
    }

    /// Registra a máquina que chama na conta. Só o servidor usa: o aparelho entra por [`Self::add_machine`].
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido e [`Code::ConnectResponseInvalid`] com campo faltando.
    pub async fn register_machine(&self, session: &AccountSession, name: &str) -> Result<MachineRegistration, Error> {
        let answer = self.post(&session.service_url, "/v1/machines", json!({"name": name}), bearer(session)).await?;
        serde_json::from_value(answer).map_err(|_| fail(Code::ConnectResponseInvalid))
    }

    /// # Errors
    ///
    /// Os de qualquer pedido e [`Code::ConnectResponseInvalid`] com linha fora da forma.
    pub async fn list_machines(&self, session: &AccountSession) -> Result<Vec<ConnectMachine>, Error> {
        let rows = self.call(&session.service_url, "/v1/machines", Method::Get, None, bearer(session)).await?;
        let valid = |row: &Value| ["machine_id", "name", "host"].iter().all(|key| filled(row, key).is_some())
            && row.get("online").is_some_and(Value::is_boolean);
        if !rows.as_array().is_some_and(|rows| rows.iter().all(valid)) {
            return Err(fail(Code::ConnectResponseInvalid));
        }
        serde_json::from_value(rows).map_err(|_| fail(Code::ConnectResponseInvalid))
    }

    /// Registra este aparelho na conta: nasce pendente e espera a liberação de um aparelho ou PC já liberado.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido e [`Code::ConnectResponseInvalid`] sem `device_id`/`device_secret`.
    pub async fn register_device(&self, session: &AccountSession, name: &str, install_id: Option<&str>) -> Result<DeviceKey, Error> {
        let mut body = json!({"name": name});
        if let Some(install_id) = install_id.filter(|id| !id.is_empty()) {
            body["install_id"] = json!(install_id);
        }
        let answer = self.post(&session.service_url, "/v1/devices", body, auth(session)).await?;
        match (filled(&answer, "device_id"), filled(&answer, "device_secret")) {
            (Some(id), Some(secret)) => Ok(DeviceKey { id: id.to_owned(), secret: secret.to_owned() }),
            _ => Err(fail(Code::ConnectResponseInvalid)),
        }
    }

    /// Status deste aparelho na conta.
    ///
    /// # Errors
    ///
    /// 404 (status no erro) = pedido vencido ou desconhecido: registrar de novo. Os de qualquer pedido.
    pub async fn device_status(&self, session: &AccountSession, device: &DeviceKey) -> Result<DeviceStatus, Error> {
        let with_device = AccountSession { device: Some(device.clone()), ..session.clone() };
        let path = format!("/v1/devices/{}", segment(&device.id));
        let answer = self.call(&session.service_url, &path, Method::Get, None, auth(&with_device)).await?;
        answer.get("status").cloned().and_then(|status| serde_json::from_value(status).ok())
            .ok_or_else(|| fail(Code::ConnectResponseInvalid))
    }

    /// Aparelhos da conta; só responde a aparelho liberado.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido (o 403 traz o motivo do aparelho) e [`Code::ConnectResponseInvalid`].
    pub async fn list_account_devices(&self, session: &AccountSession) -> Result<Vec<AccountDevice>, Error> {
        let rows = self.call(&session.service_url, "/v1/devices", Method::Get, None, auth(session)).await?;
        serde_json::from_value(rows).map_err(|_| fail(Code::ConnectResponseInvalid))
    }

    /// # Errors
    ///
    /// Os de qualquer pedido.
    pub async fn approve_account_device(&self, session: &AccountSession, id: &str) -> Result<(), Error> {
        self.post(&session.service_url, &format!("/v1/devices/{}/approve", segment(id)), json!({}), auth(session)).await.map(drop)
    }

    /// Recusa o pendente ou tira o acesso do liberado, em todas as máquinas da conta.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido.
    pub async fn revoke_account_device(&self, session: &AccountSession, id: &str) -> Result<(), Error> {
        let path = format!("/v1/devices/{}", segment(id));
        self.call(&session.service_url, &path, Method::Delete, None, auth(session)).await.map(drop)
    }

    /// Migração: a máquina pede a liberação de um aparelho da conta, e o serviço só libera com o segredo dele.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido.
    pub async fn vouch_account_device(&self, session: &AccountSession, id: &str, secret: &str) -> Result<(), Error> {
        let path = format!("/v1/devices/{}/vouch", segment(id));
        self.post(&session.service_url, &path, json!({"device_secret": secret}), auth(session)).await.map(drop)
    }

    /// Troca a lista de convidados da máquina (`session` com a chave dela) pela de `user_hashes`; vazia apaga.
    /// Devolve os hashes pedidos que já são de outra conta.
    ///
    /// # Errors
    ///
    /// Os de qualquer pedido e [`Code::ConnectResponseInvalid`] com resposta fora da forma.
    pub async fn put_guests(&self, session: &AccountSession, user_hashes: &[String]) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Answer {
            #[serde(default)]
            taken: Vec<String>,
        }
        let body = json!({"user_hashes": user_hashes});
        let answer = self.call(&session.service_url, "/v1/guests", Method::Put, Some(body), bearer(session)).await?;
        let answer: Answer = serde_json::from_value(answer).map_err(|_| fail(Code::ConnectResponseInvalid))?;
        Ok(answer.taken.into_iter().filter(|hash| user_hashes.contains(hash)).collect())
    }

    async fn account_session(&self, service_url: &str, path: &str, name: &str, password: &str) -> Result<AccountSession, Error> {
        let answer = self.post(service_url, path, json!({"name": name, "password": password}), Vec::new()).await?;
        let token = filled(&answer, "token").ok_or_else(|| fail(Code::ConnectResponseInvalid))?;
        Ok(AccountSession { service_url: service_url.to_owned(), token: token.to_owned(), device: None })
    }
}

/// O serviço compara em minúsculas ASCII, e conta e convidado nascem assim.
pub fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Nome de convidado: tem `.` ou `@`, que conta do Connect não aceita, e nenhum espaço.
pub fn valid_guest_user(user: &str) -> bool {
    user.contains(['.', '@']) && !user.chars().any(char::is_whitespace)
}

/// Código de conferência do celular: o mesmo que o celular calcula (`installCode` em `packages/core`, mesmos casos de
/// teste). Sai do SHA-256 do `install_id`, não de um pedaço dele, e a lista manda só ele, nunca o id.
/// shortcut: são 30 bits; quem tem a senha da conta e viu o código no celular ainda acha outro id com o mesmo código
/// por força bruta (~2^30 hashes). Por isso o cartão mostra também o nome e há quanto tempo o pedido chegou.
pub fn install_code(install_id: &str) -> String {
    // Sem 0/O e 1/I: a pessoa compara de olho, numa tela e na outra.
    const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let digest = ring::digest::digest(&ring::digest::SHA256, install_id.as_bytes());
    let head = u32::from_be_bytes(digest.as_ref()[..4].try_into().expect("o SHA-256 tem 32 bytes")) >> 2;
    let code: String = (0..6).map(|i| ALPHABET[((head >> (25 - 5 * i)) & 31) as usize] as char).collect();
    format!("{}-{}", &code[..3], &code[3..])
}

pub(crate) fn bearer(session: &AccountSession) -> Vec<(&'static str, String)> {
    vec![("Authorization", format!("Bearer {}", session.token))]
}

/// Bearer da conta e, com aparelho, a prova dele.
pub(crate) fn auth(session: &AccountSession) -> Vec<(&'static str, String)> {
    let mut headers = bearer(session);
    if let Some(device) = &session.device {
        headers.push(("X-Hangar-Device", device.id.clone()));
        headers.push(("X-Hangar-Device-Secret", device.secret.clone()));
    }
    headers
}

/// A senha da convidada vai às máquinas indicadas: só as abaixo do domínio do Connect, nunca um host qualquer.
fn in_connect_domain(host: &str) -> bool {
    host.strip_suffix(CONNECT_DOMAIN).is_some_and(|rest| rest.len() > 1 && rest.ends_with('.'))
}

fn valid_hostname(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|label| {
        !label.is_empty() && !label.starts_with('-') && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}
