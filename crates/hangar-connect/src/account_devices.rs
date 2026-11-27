//! Aparelhos da conta no serviço do Connect (`/v1/devices`), falados com a chave desta máquina.
//!
//! Quem libera e revoga é a conta: aqui só se lê, se repassa o pedido e se aplica a revogação aos aparelhos locais.
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;

use hangar_connect_client::{AccountSession, Code, ConnectClient, Error, ReqwestTransport};
pub use hangar_connect_client::AccountDevice;

use super::ConnectError;
use super::devices::Store;
use super::state::{Linked, machine_session, service_client};
use super::supervisor::off_thread;

/// Pedido da tela e leitura periódica: serviço fora não segura nenhum dos dois.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Por que a conta não atendeu. Vira [`ConnectError`] na rota; o laço da conta decide pela variante.
#[derive(Clone, Debug)]
pub enum Failure {
    InvalidId,
    Unreachable,
    /// Resposta fora da forma ou maior que o limite.
    Invalid,
    /// 401: a conta não reconhece a chave desta máquina.
    MachineUnknown,
    /// 403 `device_pending`: este PC ainda espera liberação na conta.
    MachinePending,
    /// A lista pedida com a chave desta máquina voltou revogada: a conta tirou este PC.
    MachineRevoked,
    DeviceRefused,
    DeviceUnknown,
    DeviceRevoked,
    Busy,
    Failed(u16),
    /// Ligação gravada fora da forma ou gravação local que falhou.
    Local(ConnectError),
}

impl Failure {
    /// A conta recusou a leitura a esta máquina: a revogação lá não chegaria aqui.
    pub fn machine_refused(&self) -> bool {
        matches!(self, Self::MachinePending | Self::MachineUnknown | Self::MachineRevoked)
    }
}

impl From<hangar_connect_client::Error> for Failure {
    fn from(error: hangar_connect_client::Error) -> Self {
        match error.status() {
            Some(401) => Self::MachineUnknown,
            // O 403 de PC ainda pendente na conta não é recusa de aparelho.
            Some(403) if error.code() == Code::ConnectDevicePending => Self::MachinePending,
            Some(403) => Self::DeviceRefused,
            Some(404) => Self::DeviceUnknown,
            Some(409) => Self::DeviceRevoked,
            Some(429) => Self::Busy,
            Some(status) => Self::Failed(status),
            None if error.code() == Code::NetworkFailed => Self::Unreachable,
            None => Self::Invalid,
        }
    }
}

impl From<Failure> for ConnectError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::InvalidId => Self::new(StatusCode::BAD_REQUEST, "connect_account_device_invalid", "aparelho inválido"),
            Failure::Unreachable => Self::new(StatusCode::BAD_GATEWAY, "connect_service_unreachable", "o serviço do Connect não respondeu"),
            Failure::Invalid => Linked::invalid(),
            Failure::MachineUnknown => Self::new(StatusCode::CONFLICT, "connect_account_machine_unknown",
                "a conta não reconhece este computador: ligue-o de novo ao Connect"),
            Failure::MachinePending => Self::new(StatusCode::FORBIDDEN, "connect_account_machine_pending",
                "este computador espera liberação: libere-o em \"Aparelhos da conta\" num aparelho já liberado"),
            Failure::MachineRevoked => Self::new(StatusCode::FORBIDDEN, "connect_account_machine_revoked",
                "a conta revogou este computador: ligue-o de novo ao Connect"),
            Failure::DeviceRefused => Self::new(StatusCode::FORBIDDEN, "connect_account_device_refused", "o serviço do Connect recusou o pedido para esse aparelho"),
            Failure::DeviceUnknown => Self::new(StatusCode::NOT_FOUND, "connect_account_device_unknown", "esse aparelho não está na conta"),
            Failure::DeviceRevoked => Self::new(StatusCode::CONFLICT, "connect_account_device_revoked", "esse aparelho foi revogado na conta"),
            Failure::Busy => Self::new(StatusCode::TOO_MANY_REQUESTS, "connect_service_busy", "o serviço do Connect pediu para esperar; tente mais tarde"),
            Failure::Failed(status) => Self::new(StatusCode::BAD_GATEWAY, "connect_service_failed", format!("o serviço do Connect falhou (HTTP {status})")),
            Failure::Local(error) => error,
        }
    }
}

/// O id vira pedaço do caminho no serviço.
pub fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn checked(id: &str) -> Result<(), Failure> {
    if valid_id(id) { Ok(()) } else { Err(Failure::InvalidId) }
}

/// O cliente do serviço e a sessão com a chave desta máquina.
fn connect(linked: &Linked) -> Result<(ConnectClient<ReqwestTransport>, AccountSession), Failure> {
    let session = machine_session(linked).map_err(Failure::Local)?;
    Ok((service_client(TIMEOUT).ok_or(Failure::Unreachable)?, session))
}

pub async fn list(linked: &Linked) -> Result<Vec<AccountDevice>, Failure> {
    let (client, session) = connect(linked)?;
    client.list_account_devices(&session).await.map_err(listing_failure)
}

/// Na lista quem pede é esta máquina: revogado ali é ela, não um aparelho da lista.
fn listing_failure(error: Error) -> Failure {
    match (error.status(), error.code()) {
        (Some(403), Code::ConnectDeviceRevoked) | (Some(409), _) => Failure::MachineRevoked,
        _ => error.into(),
    }
}

pub async fn approve(linked: &Linked, id: &str) -> Result<(), Failure> {
    checked(id)?;
    let (client, session) = connect(linked)?;
    Ok(client.approve_account_device(&session, id).await?)
}

/// Já revogado ou fora da conta vale como feito: o que falta é tirar daqui, senão o aparelho entra até a próxima leitura.
pub async fn revoke(linked: &Linked, id: &str) -> Result<(), Failure> {
    checked(id)?;
    let (client, session) = connect(linked)?;
    match client.revoke_account_device(&session, id).await.map_err(Failure::from) {
        Err(Failure::DeviceRevoked | Failure::DeviceUnknown) => Ok(()),
        other => other,
    }
}

/// Outra máquina da conta. Já apagada vale como feito: quem pediu só quer que ela suma da lista.
pub async fn remove_machine(linked: &Linked, id: &str) -> Result<(), Failure> {
    checked(id)?;
    let (client, session) = connect(linked)?;
    match client.remove_machine(&session, id).await.map_err(Failure::from) {
        Err(Failure::DeviceUnknown) => Ok(()),
        other => other,
    }
}

/// Migração: o serviço só libera se o segredo for o do aparelho.
pub async fn vouch(linked: &Linked, id: &str, secret: &str) -> Result<(), Failure> {
    checked(id)?;
    let (client, session) = connect(linked)?;
    Ok(client.vouch_account_device(&session, id, secret).await?)
}

/// Aplica a lista inteira de revogados a cada leitura: bilhete pré-aprovado resgatado depois da leitura
/// anterior nasce aprovado mesmo com o aparelho já revogado na conta.
pub async fn apply_revocations(linked: &Linked, devices: &Arc<Store>) -> Result<(), Failure> {
    let revoked: Vec<String> = list(linked).await?.into_iter().filter(AccountDevice::is_revoked).map(|d| d.device_id).collect();
    if revoked.is_empty() {
        return Ok(());
    }
    let devices = devices.clone();
    off_thread(move || devices.revoke_account_devices(&revoked).map_err(|_| {
        ConnectError::new(StatusCode::SERVICE_UNAVAILABLE, "connect_store_failed", "os aparelhos do Connect não foram gravados")
    })).await.map_err(Failure::Local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoked_answer_to_the_list_refuses_this_machine() {
        for error in [Error::new(Code::ConnectDeviceRevoked, Some(403)), Error::new(Code::ConnectRequestFailed, Some(409)),
            Error::new(Code::ConnectDevicePending, Some(403))] {
            assert!(listing_failure(error.clone()).machine_refused(), "{error}");
        }
        for error in [Error::new(Code::ConnectRequestFailed, Some(403)), Error::new(Code::ConnectRequestFailed, Some(502)),
            Error::network("sem rede")] {
            assert!(!listing_failure(error.clone()).machine_refused(), "{error}");
        }
        // Fora da lista, 409 é o aparelho pedido, não esta máquina.
        assert!(!Failure::from(Error::new(Code::ConnectRequestFailed, Some(409))).machine_refused());
    }
}
