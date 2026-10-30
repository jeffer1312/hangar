//! Corpos das rotas do cofre entre máquinas (`use`, `ask-entry`, `requests/{id}/wait`): quem pede
//! (`remote.rs`) e a dona usam os mesmos tipos. Tipo que leva o valor não tem `Debug` e não é enum
//! `untagged` nem com `tag`: nesses o serde copia o valor num buffer intermediário que não zera.
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

/// Quanto a dona segura um `wait` sem novidade antes de responder `pending`.
pub const WAIT_HOLD: Duration = Duration::from_secs(25);
const _: () = assert!(WAIT_HOLD.as_secs() < crate::groups::peers::LONG_POLL_TIMEOUT.as_secs());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UseKind { Fill, Exec, File }

/// `POST /api/vault/use`. `entry` é o nome da senha; `target` o domínio, o comando ou `"arquivo"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseReq {
    pub origin_server: String,
    pub session_key: String,
    pub session: String,
    pub entry: String,
    pub kind: UseKind,
    pub reason: String,
    pub target: String,
}

/// O `server_id` das respostas é o da dona que atendeu: quem pede confere que falou com a máquina
/// que o `vault.json` nomeia. Ausente (dona sem id configurado) é aceito.
pub fn own_id(server_id: &str) -> Option<String> { (!server_id.is_empty()).then(|| server_id.to_owned()) }

/// Resposta do `use`: `{request_id}` (pedir sempre) ou `{value}` (liberada).
#[derive(Default, Serialize, Deserialize)]
pub struct UseReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

/// `POST /api/vault/ask-entry`: cadastro pelo cartão.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskReq {
    pub origin_server: String,
    pub session_key: String,
    pub session: String,
    pub label: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskReply {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitState { Pending, Allowed, Saved, Denied, Declined, TimedOut, Cancelled }

/// `GET /api/vault/requests/{id}/wait`: `value` só em `allowed`; `entry` e `temporary` só em `saved`.
#[derive(Serialize, Deserialize)]
pub struct WaitReply {
    pub status: WaitState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

/// Id de pedido aceito na URL: a dona gera hex; qualquer outra coisa não é pedido dela.
pub fn request_id_ok(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
