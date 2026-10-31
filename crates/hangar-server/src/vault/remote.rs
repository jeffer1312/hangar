//! A dona do cofre vista de outra máquina, pelo `peers.json`. Corpo que pode levar senha chega cru
//! em `Zeroizing` e vira tipo com `Zeroizing<String>` direto, sem passar por `serde_json::Value`.
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

use super::{Status, Summary, wire::{AskReply, AskReq, UseReply, UseReq, WaitReply, WaitState, request_id_ok}};
use crate::groups::peers::{PeerClient, PeerError};

/// Nenhuma variante leva valor de senha: o texto vem da rede ou da recusa da dona.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// `erro_cofre_dona_fora`: a dona não respondeu, recusou o token ou o endereço dela não chega ao
    /// `hangar-server` (Connect, versão sem cofre). Leva o motivo.
    OwnerDown(String),
    /// `erro_cofre_pedido_perdido`: a dona não conhece o pedido (reiniciou ou já o fechou).
    Lost,
    /// A dona recusou com um código dela (`erro_cofre_trancado`, `erro_cofre_entrada_desconhecida`…).
    Refused { status: u16, code: String, msg: String },
}

impl RemoteError {
    pub fn code(&self) -> &str {
        match self {
            Self::OwnerDown(_) => "erro_cofre_dona_fora",
            Self::Lost => "erro_cofre_pedido_perdido",
            Self::Refused { code, .. } => code,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::OwnerDown(reason) => reason,
            Self::Lost => "a dona do cofre não conhece mais este pedido",
            Self::Refused { msg, .. } => msg,
        }
    }
}

/// Resposta do `use`: pedido aberto na dona ou, senha liberada, o valor.
pub enum UseStart { Request(String), Value(Zeroizing<String>) }

pub enum WaitStatus {
    Pending,
    Allowed(Zeroizing<String>),
    Saved { entry: String, temporary: bool },
    Denied,
    Declined,
    TimedOut,
}

pub struct Remote<'a> { pub peers: &'a PeerClient, pub owner: &'a str }

impl Remote<'_> {
    /// O estado do cofre na dona. `?local=1`: a dona responde o dela sem encaminhar a outra (dois
    /// `vault.json` apontando um para o outro dariam volta sem fim).
    pub async fn status(&self) -> Result<Status, RemoteError> {
        let answer: Value = self.plain(Method::GET, "/api/vault/status?local=1", None).await?;
        self.same_owner(answer.get("server_id").and_then(Value::as_str))?;
        serde_json::from_value(answer).map_err(|_| self.unreadable())
    }

    /// A lista da dona; `Summary` não tem senha, e campo a mais que viesse fica de fora.
    pub async fn entries(&self) -> Result<Vec<Summary>, RemoteError> { self.plain(Method::GET, "/api/vault/entries?local=1", None).await }

    pub async fn use_(&self, req: &UseReq) -> Result<UseStart, RemoteError> {
        let body = serde_json::to_value(req).map_err(|_| self.unreadable())?;
        let reply = self.secret::<UseReply>(false, Method::POST, "/api/vault/use", Some(&body)).await?;
        self.same_owner(reply.server_id.as_deref())?;
        match reply {
            UseReply { value: Some(value), .. } => Ok(UseStart::Value(value)),
            UseReply { request_id: Some(id), .. } if request_id_ok(&id) => Ok(UseStart::Request(id)),
            _ => Err(self.unreadable()),
        }
    }

    pub async fn ask_entry(&self, req: &AskReq) -> Result<String, RemoteError> {
        let body = serde_json::to_value(req).map_err(|_| self.unreadable())?;
        let AskReply { request_id, server_id } = self.plain::<AskReply>(Method::POST, "/api/vault/ask-entry", Some(&body)).await?;
        self.same_owner(server_id.as_deref())?;
        if request_id_ok(&request_id) { Ok(request_id) } else { Err(self.unreadable()) }
    }

    /// Uma consulta longa: a dona segura até `wire::WAIT_HOLD` e responde `pending` sem novidade.
    pub async fn wait(&self, id: &str) -> Result<WaitStatus, RemoteError> {
        if !request_id_ok(id) { return Err(RemoteError::Lost); }
        let reply: WaitReply = self.secret(true, Method::GET, &format!("/api/vault/requests/{id}/wait"), None).await?;
        self.same_owner(reply.server_id.as_deref())?;
        Ok(match (reply.status, reply.value) {
            (WaitState::Pending, _) => WaitStatus::Pending,
            (WaitState::Allowed, Some(value)) => WaitStatus::Allowed(value),
            (WaitState::Allowed, None) => return Err(self.unreadable()),
            (WaitState::Saved, _) => match reply.entry {
                Some(entry) => WaitStatus::Saved { entry, temporary: reply.temporary.unwrap_or(false) },
                None => return Err(self.unreadable()),
            },
            (WaitState::Denied, _) => WaitStatus::Denied,
            (WaitState::Declined, _) => WaitStatus::Declined,
            (WaitState::TimedOut, _) => WaitStatus::TimedOut,
        })
    }

    /// Quem pediu desistiu. Pedido já fechado na dona volta `Lost`.
    pub async fn cancel(&self, id: &str) -> Result<(), RemoteError> {
        if !request_id_ok(id) { return Err(RemoteError::Lost); }
        self.peers.call(self.owner, Method::DELETE, &format!("/api/vault/requests/{id}"), None).await.map(drop).map_err(|e| self.fail(e))
    }

    async fn plain<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<&Value>) -> Result<T, RemoteError> {
        let answer = self.peers.call(self.owner, method, path, body).await.map_err(|e| self.fail(e))?;
        serde_json::from_value(answer.unwrap_or(Value::Null)).map_err(|_| self.unreadable())
    }

    async fn secret<T: DeserializeOwned>(&self, long: bool, method: Method, path: &str, body: Option<&Value>) -> Result<T, RemoteError> {
        let sent = self.peers.send_secret(long, self.owner, method, path, crate::groups::peers::json_body(body),
            crate::groups::peers::MAX_BODY).await;
        let (_, raw) = sent.map_err(|e| self.fail(e))?;
        // O erro do serde pode citar o trecho lido: fica de fora.
        serde_json::from_slice(&raw).map_err(|_| self.unreadable())
    }

    /// Como o `call_group`: `server_id` ausente é aceito; presente e diferente é outra máquina
    /// atendendo pelo endereço da dona.
    fn same_owner(&self, reported: Option<&str>) -> Result<(), RemoteError> {
        match reported {
            Some(reported) if reported != self.owner => Err(RemoteError::OwnerDown(format!(
                "a máquina '{}' do peers.json responde como '{reported}': confira o CP_SERVER_ID dela", self.owner))),
            _ => Ok(()),
        }
    }

    fn unreadable(&self) -> RemoteError { RemoteError::OwnerDown(format!("{} respondeu fora do contrato do cofre", self.owner)) }

    fn fail(&self, error: PeerError) -> RemoteError {
        let (status, detail) = match error {
            PeerError::Refused { status, detail } => (status, detail),
            other => return RemoteError::OwnerDown(other.text(self.owner)),
        };
        let code = detail.get("code").and_then(Value::as_str).map(str::to_owned);
        match (status, code) {
            (404, Some(code)) if code == "erro_cofre_pedido_perdido" => RemoteError::Lost,
            (401, _) => RemoteError::OwnerDown(format!("{} recusou o token do peers.json", self.owner)),
            // 403 do cofre só sai na porta do convidado: o endereço no peers.json não é o do dono.
            (403, _) => RemoteError::OwnerDown(format!("o endereço de {} no peers.json cai na porta do convidado", self.owner)),
            // Pedido com senha não segue redirect.
            (300..=399, _) => RemoteError::OwnerDown(format!("o endereço de {} no peers.json responde com redirect (HTTP {status})", self.owner)),
            (_, Some(code)) => {
                let msg = detail.get("msg").and_then(Value::as_str).unwrap_or(code.as_str()).to_owned();
                RemoteError::Refused { status, code, msg }
            }
            // A 8768 (Connect, Python) e um Hangar sem cofre respondem 404 sem código.
            (404, None) => RemoteError::OwnerDown(format!(
                "o endereço de {} no peers.json não chega ao cofre (Connect ou versão sem cofre)", self.owner)),
            (_, None) => RemoteError::OwnerDown(PeerError::Refused { status, detail }.text(self.owner)),
        }
    }
}
