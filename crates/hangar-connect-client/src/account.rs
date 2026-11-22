//! Conta do Connect: login, nome (conta ou convidado), máquinas e a liberação única deste aparelho.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ConnectClient, filled, segment};
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

/// Linha de `GET /v1/devices`; tempos em segundos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDevice {
    pub device_id: String,
    pub name: String,
    pub status: DeviceStatus,
    pub created_at: i64,
    pub approved_at: Option<i64>,
    pub revoked_at: Option<i64>,
    /// O código que o aparelho mostra, pronto do serviço (que nunca manda o `install_id`); ausente em serviço antigo.
    #[serde(default)]
    pub install_code: Option<String>,
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
pub enum NameLookup {
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
    pub async fn lookup_name(&self, service_url: &str, name: &str) -> Result<NameLookup, Error> {
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
                        .map(|host| host.as_str().filter(|host| valid_hostname(host)).map(str::to_owned))
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
    /// antes de "não confere": a máquina fora do ar pode ser a que aceitaria) e os de [`Self::lookup_name`].
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

fn valid_hostname(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|label| {
        !label.is_empty() && !label.starts_with('-') && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}
