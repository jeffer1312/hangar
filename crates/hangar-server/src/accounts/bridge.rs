//! Fatos não adquirem a proteção de existência: o chamador pode já ter exclusividade.
use super::AccountKey;
pub use super::types::UsageFacts;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, time::Duration};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountUsage {
    pub key: AccountKey,
    pub facts: UsageFacts,
}

/// Cliente privado, vinculado à instância. Falha de transporte nunca vira fatos completos vazios.
pub struct AccountsBridge {
    client: reqwest::Client,
    upstream: SocketAddr,
    secret: String,
    instance: String,
}

impl AccountsBridge {
    pub fn new(
        upstream: SocketAddr,
        secret: String,
        instance: String,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            upstream,
            secret,
            instance,
        })
    }

    pub async fn facts(&self, keys: &[AccountKey]) -> Result<Vec<AccountUsage>, &'static str> {
        if !self.upstream.ip().is_loopback() {
            return Err("account_bridge_address");
        }
        let response = self
            .client
            .post(format!("http://{}/internal/accounts/facts", self.upstream))
            .header("x-hangar-internal", &self.secret)
            .header("x-hangar-runtime-instance", &self.instance)
            .header("content-type", "application/json")
            .body(serde_json::json!({"keys": keys}).to_string())
            .send()
            .await
            .map_err(|_| "account_facts_unavailable")?;
        if !response.status().is_success() {
            return Err("account_facts_unavailable");
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "account_facts_unavailable")?;
        let usage: Vec<AccountUsage> =
            serde_json::from_slice(&bytes).map_err(|_| "account_facts_invalid")?;
        if usage.len() != keys.len() || usage.iter().zip(keys).any(|(row, key)| &row.key != key) {
            return Err("account_facts_invalid");
        }
        Ok(usage)
    }
}
