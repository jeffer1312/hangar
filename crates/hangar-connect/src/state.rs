//! Ligação desta máquina: o que o serviço do Connect entregou, em `~/.hangar/connect/connect.json`.
//!
//! A chave de máquina e o token do `frps` ficam aqui e no `frpc.toml`. A senha da conta nunca entra.
use std::io;
use std::path::Path;
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::ConnectError;
use super::config::{frps_endpoint, valid_host, valid_label, valid_secret};
use super::devices::ServiceIdentity;

const STATE_FILE: &str = "connect.json";
const SERVICE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    pub service_url: String,
    pub account: String,
    pub password: String,
    pub machine_name: String,
    #[serde(default)]
    pub create_account: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Linked {
    pub service_url: String,
    pub account: String,
    pub machine_id: String,
    pub machine_key: String,
    pub host: String,
    pub guest_host: String,
    pub preview_host: String,
    pub frps_addr: String,
    pub frps_token: String,
    /// Base64, como veio do serviço.
    pub service_public_key: String,
    /// O primeiro convite já pediu o site do convidado. Fica: cada host gasta um certificado do limite semanal.
    #[serde(default)]
    pub guest_published: bool,
    /// Como `guest_published`, para o site da prévia de porta.
    #[serde(default)]
    pub preview_published: bool,
}

impl Linked {
    pub(super) fn invalid() -> ConnectError {
        ConnectError::new(StatusCode::BAD_GATEWAY, "connect_service_invalid", "o serviço do Connect devolveu uma ligação inválida")
    }

    fn public_key(&self) -> Option<[u8; 32]> {
        base64::engine::general_purpose::STANDARD.decode(&self.service_public_key).ok()?.try_into().ok()
    }

    /// Vale também para o que veio do disco: tudo aqui acaba cru no TOML e no Caddyfile.
    pub fn validate(&self) -> Result<(), ConnectError> {
        let names = valid_label(&self.account) && valid_label(&self.machine_id)
            && self.host.matches('.').count() >= 2 && valid_host(&self.host)
            && valid_host(&self.guest_host) && valid_host(&self.preview_host)
            && frps_endpoint(&self.frps_addr).is_some();
        let secrets = valid_secret(&self.machine_key) && valid_secret(&self.frps_token);
        if names && secrets && self.public_key().is_some() { Ok(()) } else { Err(Self::invalid()) }
    }

    pub fn identity(&self) -> Result<ServiceIdentity, ConnectError> {
        Ok(ServiceIdentity {
            public_key: self.public_key().ok_or_else(Self::invalid)?,
            machine_id: self.machine_id.clone(),
            account: self.account.clone(),
        })
    }
}

fn io_failed(code: &'static str, what: &str, cause: &io::Error) -> ConnectError {
    ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, code, format!("{what}: {cause}"))
}

fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    // `0600` não protege no Windows: a pasta só abre para o usuário e o que nasce dentro herda.
    #[cfg(windows)]
    hangar_core::private_acl::restrict_path(dir, "OICI")?;
    Ok(())
}

/// Antes de qualquer segredo: falha aqui recusa a gravação, nunca grava aberto.
pub fn protect(dir: &Path) -> Result<(), ConnectError> {
    private_dir(dir).map_err(|cause| io_failed("connect_private_failed", "não consegui proteger a pasta do Connect", &cause))
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ConnectError> {
    hangar_core::fs::atomic_write(path, bytes)
        .map_err(|cause| io_failed("connect_private_failed", "não consegui gravar o arquivo do Connect", &cause))
}

fn unreadable(cause: &str) -> ConnectError {
    ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_state_unreadable", format!("a ligação do Connect não foi lida ({cause})"))
}

/// `None` é só arquivo ausente: ilegível não vale como "não ligado". A causa do serde fica fora do log e do
/// erro, porque cita trechos do arquivo.
pub fn read(dir: &Path) -> Result<Option<Linked>, ConnectError> {
    let path = dir.join(STATE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            tracing::warn!(code = "connect_state_unreadable", kind = ?error.kind(), "estado do Connect ilegível");
            return Err(unreadable(&error.kind().to_string()));
        }
    };
    match serde_json::from_slice::<Linked>(&bytes).ok().filter(|linked| linked.validate().is_ok()) {
        Some(linked) => Ok(Some(linked)),
        None => {
            tracing::warn!(code = "connect_state_unreadable", "estado do Connect ilegível");
            Err(unreadable("conteúdo inválido"))
        }
    }
}

/// Ligação nova por cima de uma ilegível: a antiga fica ao lado, para quem quiser recuperar.
pub fn set_aside(dir: &Path) -> Result<(), ConnectError> {
    std::fs::rename(dir.join(STATE_FILE), dir.join(format!("{STATE_FILE}.bad")))
        .map_err(|cause| io_failed("connect_state_unreadable", "não consegui guardar a ligação ilegível do Connect", &cause))
}

pub fn write(dir: &Path, linked: &Linked) -> Result<(), ConnectError> {
    protect(dir)?;
    let bytes = serde_json::to_vec_pretty(linked).map_err(|_| Linked::invalid())?;
    write_private(&dir.join(STATE_FILE), &bytes)
}

fn already_gone(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn forget_failed(cause: &io::Error) -> ConnectError {
    io_failed("connect_forget_failed", "não consegui apagar a ligação do Connect", cause)
}

pub fn remove(dir: &Path) -> Result<(), ConnectError> {
    already_gone(std::fs::remove_file(dir.join(STATE_FILE))).map_err(|cause| forget_failed(&cause))
}

/// O que sobra da ligação depois dos filhos parados. `caddy/` guarda a chave TLS do host.
pub fn remove_files(dir: &Path) -> Result<(), ConnectError> {
    for name in ["frpc.toml", "Caddyfile", "frpc.pid", "caddy.pid"] {
        already_gone(std::fs::remove_file(dir.join(name))).map_err(|cause| forget_failed(&cause))?;
    }
    already_gone(std::fs::remove_dir_all(dir.join("caddy"))).map_err(|cause| forget_failed(&cause))
}

pub(super) fn service_base(raw: &str) -> Result<reqwest::Url, ConnectError> {
    let refused = || ConnectError::new(StatusCode::BAD_REQUEST, "connect_service_url_invalid", "endereço do serviço inválido");
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| refused())?;
    // A senha da conta vai neste pedido: em claro, só sem sair da máquina.
    let local = url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if !(url.scheme() == "https" || local) || !url.username().is_empty() || url.password().is_some() {
        return Err(refused());
    }
    Ok(url)
}

fn service_refused(status: u16) -> ConnectError {
    let (status, code, msg) = match status {
        401 => (StatusCode::UNAUTHORIZED, "connect_login_refused", "conta ou senha recusada pelo serviço do Connect"),
        403 => (StatusCode::FORBIDDEN, "connect_machine_limit", "a conta chegou ao limite de máquinas"),
        409 => (StatusCode::CONFLICT, "connect_machine_conflict", "o serviço do Connect recusou: o registro mudou ao mesmo tempo; tente de novo"),
        429 => (StatusCode::TOO_MANY_REQUESTS, "connect_service_busy", "o serviço do Connect pediu para esperar; tente mais tarde"),
        400 | 422 => (StatusCode::BAD_REQUEST, "connect_service_refused", "o serviço do Connect recusou o nome da conta ou da máquina"),
        _ => (StatusCode::BAD_GATEWAY, "connect_service_failed", "o serviço do Connect falhou"),
    };
    ConnectError::new(status, code, msg)
}

async fn call<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, ConnectError> {
    let unreachable = || ConnectError::new(StatusCode::BAD_GATEWAY, "connect_service_unreachable", "o serviço do Connect não respondeu");
    let response = request.send().await.map_err(|error| {
        tracing::warn!(code = "connect_service_unreachable", "serviço do Connect fora: {}", error.without_url());
        unreachable()
    })?;
    if !response.status().is_success() {
        return Err(service_refused(response.status().as_u16()));
    }
    let bytes = response.bytes().await.map_err(|_| unreachable())?;
    serde_json::from_slice(&bytes).map_err(|_| Linked::invalid())
}

#[derive(Deserialize)]
struct Session { token: String }

#[derive(Deserialize)]
struct Machine {
    machine_id: String,
    machine_key: String,
    host: String,
    guest_host: String,
    preview_host: String,
    frps_addr: String,
    frps_token: String,
    service_public_key: String,
}

/// Entra na conta (ou a cria, com `create_account`) e registra a máquina. O serviço troca a chave a cada registro: quem chama precisa gravar o resultado.
pub async fn register(request: &LinkRequest) -> Result<Linked, ConnectError> {
    let base = service_base(&request.service_url)?;
    if !valid_label(&request.account) || !valid_label(&request.machine_name) {
        return Err(ConnectError::new(StatusCode::BAD_REQUEST, "connect_name_invalid", "nome de conta ou de máquina inválido"));
    }
    let endpoint = |path: &str| base.join(path).map_err(|_| Linked::invalid());
    // Sem seguir redirecionamento: a senha iria junto para outro endereço.
    let client = hangar_core::net::http_client().redirect(reqwest::redirect::Policy::none()).timeout(SERVICE_TIMEOUT).build()
        .map_err(|_| ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_service_unreachable", "cliente HTTP indisponível"))?;
    // Criar a conta já devolve a sessão dela, no mesmo formato do login.
    let start = if request.create_account { "/v1/accounts" } else { "/v1/sessions" };
    let session: Session = call(client.post(endpoint(start)?)
        .json(&json!({"name": request.account, "password": request.password}))).await
        .map_err(|error| if request.create_account && error.status == StatusCode::CONFLICT {
            ConnectError::new(StatusCode::CONFLICT, "connect_account_taken", "já existe uma conta com esse nome")
        } else { error })?;
    let machine: Machine = call(client.post(endpoint("/v1/machines")?).bearer_auth(&session.token)
        .json(&json!({"name": request.machine_name}))).await
        // A conta nasceu: criar de novo daria nome já usado, então quem chama passa a entrar nela.
        .map_err(|error| if request.create_account {
            ConnectError::new(error.status, "connect_account_created_link_failed", format!("conta criada; entre com ela para ligar este computador ({})", error.msg))
        } else { error })?;
    let linked = Linked {
        service_url: base.as_str().trim_end_matches('/').to_owned(),
        account: request.account.clone(),
        machine_id: machine.machine_id,
        machine_key: machine.machine_key,
        host: machine.host,
        guest_host: machine.guest_host,
        preview_host: machine.preview_host,
        frps_addr: machine.frps_addr,
        frps_token: machine.frps_token,
        service_public_key: machine.service_public_key,
        guest_published: false,
        preview_published: false,
    };
    linked.validate()?;
    Ok(linked)
}
