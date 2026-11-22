//! Instalar o app nos aparelhos (`/api/device-install/*`, do hangar-server). A recusa vem como
//! `{"error":{"code","detail"}}`, fora do envelope das outras rotas: o código é o que a tela traduz.
use super::Api;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Recusa do servidor ou queda de rede (`code` = `connection_failed`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceError { pub code: String, pub detail: Option<String> }

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub udid: String, pub name: String, pub ios_version: String, pub trusted: bool,
    /// Bloqueado ou desconectado: aí `trusted: false` não quer dizer "não confiado".
    #[serde(default)] pub error: Option<DeviceError>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Scan { pub service: String, pub devices: Vec<Device> }

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Progress {
    pub stage: String, pub error: Option<DeviceError>, pub source_url: String,
    /// Só em `awaiting_cert_choice`: os certificados que lotam a conta.
    #[serde(default)] pub certs: Option<Vec<Cert>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Cert { pub name: String, pub machine_name: String, pub serial: String }

fn lost() -> DeviceError { DeviceError { code: "connection_failed".into(), detail: None } }

/// A recusa do corpo; sem ela, 404 é servidor de antes destas rotas e 401 é token recusado.
fn refusal(status: u16, body: Option<Value>) -> DeviceError {
    let own = body.and_then(|v| v.get("error").cloned()).and_then(|e| serde_json::from_value(e).ok());
    let code = |c: &str| DeviceError { code: c.into(), detail: None };
    own.unwrap_or_else(|| match status { 404 => code("server_outdated"), 401 => code("unauthorized"), _ => lost() })
}

impl Api {
    async fn device_call(&self, method: Method, path: &str, query: &[(&str, &str)], body: Option<Value>, seconds: u64) -> Result<Value, DeviceError> {
        let mut req = self.client.request(method, self.server_url(&["device-install", path], query)).timeout(Duration::from_secs(seconds));
        if let Some(body) = body { req = req.json(&body); }
        let response = req.send().await.map_err(|_| lost())?;
        let status = response.status();
        let value: Option<Value> = response.json().await.ok();
        if status.is_success() { return value.ok_or_else(lost); }
        Err(refusal(status.as_u16(), value))
    }

    pub async fn device_scan(&self) -> Result<Scan, DeviceError> {
        let value = self.device_call(Method::GET, "devices", &[], None, 20).await?;
        serde_json::from_value(value).map_err(|_| lost())
    }

    /// O servidor segura a resposta até 120 s esperando o "Confiar" no aparelho.
    pub async fn device_trust(&self, udid: &str) -> Result<(), DeviceError> {
        self.device_call(Method::POST, "trust", &[], Some(json!({"udid": udid})), 135).await.map(drop)
    }

    /// A senha só existe dentro deste pedido. Melhor esforço: o `drop` não zera a memória e o
    /// `json!` guarda uma cópia própria.
    pub async fn device_start(&self, udid: &str, apple_id: &str, password: String) -> Result<String, DeviceError> {
        let body = json!({"udid": udid, "apple_id": apple_id, "password": password});
        drop(password);
        let value = self.device_call(Method::POST, "start", &[], Some(body), 30).await?;
        value.get("job").and_then(Value::as_str).map(str::to_owned).ok_or_else(lost)
    }

    /// `code: None` pede à Apple que mande o código de novo.
    pub async fn device_code(&self, job: &str, code: Option<&str>) -> Result<(), DeviceError> {
        let body = match code { Some(code) => json!({"job": job, "code": code}), None => json!({"job": job, "resend": true}) };
        self.device_call(Method::POST, "code", &[], Some(body), 30).await.map(drop)
    }

    /// `None` desiste do trabalho; `Some(seriais)` revoga esses certificados e segue.
    pub async fn device_certs(&self, job: &str, revoke: Option<Vec<String>>) -> Result<(), DeviceError> {
        let body = match revoke { Some(revoke) => json!({"job": job, "revoke": revoke}), None => json!({"job": job, "cancel": true}) };
        self.device_call(Method::POST, "certs", &[], Some(body), 30).await.map(drop)
    }

    /// Para o trabalho em andamento no aparelho, mesmo sem o `job` (tela reaberta).
    pub async fn device_cancel(&self, udid: &str) -> Result<(), DeviceError> {
        self.device_call(Method::POST, "cancel", &[], Some(json!({"udid": udid})), 30).await.map(drop)
    }

    pub async fn device_progress(&self, job: &str) -> Result<Progress, DeviceError> {
        let value = self.device_call(Method::GET, "progress", &[("job", job)], None, 10).await?;
        serde_json::from_value(value).map_err(|_| lost())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn devices_404_without_refusal_means_outdated_server() {
        assert_eq!(refusal(404, None).code, "server_outdated");
        assert_eq!(refusal(404, Some(json!({"detail": "Not Found"}))).code, "server_outdated");
        // 404 com recusa própria é do servidor novo (trabalho esquecido), não servidor antigo.
        let unknown = json!({"error": {"code": "device_install_unknown_job", "detail": null}});
        assert_eq!(refusal(404, Some(unknown)).code, "device_install_unknown_job");
        assert_eq!(refusal(502, None).code, "connection_failed");
        assert_eq!(refusal(401, Some(json!({"detail": "unauthorized"}))).code, "unauthorized");
    }
}
