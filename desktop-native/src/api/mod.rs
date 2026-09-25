pub mod dto;
pub mod sse;

use std::time::Duration;
use reqwest::{Client, Response, StatusCode, header};
use serde_json::{Value, json};
use url::Url;
use dto::{ChatEvent, CommandInfo, Delivery, SessionInfo, UploadFile, Uploaded};

pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
const UPLOAD_SECONDS: u64 = 180;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Source { Upload(String), Cited(String), Transcript(String, usize) }

#[derive(Clone)]
pub struct Api { client: Client, base: Url }

#[derive(Clone, Debug)]
pub struct Failure {
    pub status: Option<u16>,
    pub detail: String,
    pub retry_after: Option<u64>,
    pub uncertain: bool,
}

impl Failure {
    pub fn local(detail: impl Into<String>) -> Self {
        Self { status: None, detail: detail.into(), retry_after: None, uncertain: false }
    }
    fn transport(post: bool) -> Self {
        Self { uncertain: post, ..Self::local(if post { "delivery_uncertain" } else { "network_error" }) }
    }
}

fn failure_detail(body: Option<Value>, status: u16) -> String {
    body.and_then(|value| value.get("detail").and_then(|detail| match detail {
        Value::String(message) => Some(message.clone()),
        Value::Object(fields) => fields.get("msg").and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .or_else(|| fields.get("code").and_then(Value::as_str)).map(str::to_owned),
        // Recusa de validação (422): uma lista de `{msg}`, uma por campo.
        Value::Array(items) => Some(items.iter().filter_map(|item| item.get("msg").and_then(Value::as_str)).collect::<Vec<_>>().join("; "))
            .filter(|message| !message.is_empty()),
        _ => None,
    })).unwrap_or_else(|| format!("HTTP {status}"))
}

impl Api {
    pub fn new(address: &str, token: &str) -> Result<Self, Failure> {
        let mut base = Url::parse(address.trim()).map_err(|_| Failure::local("invalid_url"))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none()
            || !base.username().is_empty() || base.password().is_some() || base.query().is_some() || base.fragment().is_some() {
            return Err(Failure::local("invalid_url"));
        }
        if !base.path().ends_with('/') { base.set_path(&format!("{}/", base.path())); }
        let mut value = header::HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| Failure::local("invalid_token"))?;
        value.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value);
        let client = Client::builder().default_headers(headers).connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| Failure::local("network_error"))?;
        Ok(Self { client, base })
    }

    pub fn identity(&self) -> String { self.base.as_str().to_owned() }

    pub fn endpoint(&self, session: Option<&str>, action: Option<&str>) -> Url {
        let mut url = self.base.clone();
        let mut parts = url.path_segments_mut().expect("validated HTTP base");
        parts.pop_if_empty().push("api").push("sessions");
        if let Some(name) = session { parts.push(name); }
        if let Some(action) = action { parts.push(action); }
        drop(parts);
        url
    }

    async fn checked(response: Response, post: bool) -> Result<Response, Failure> {
        if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED { return Ok(response); }
        let status = response.status().as_u16();
        let retry_after = response.headers().get(header::RETRY_AFTER).and_then(|s| s.to_str().ok()).and_then(|s| s.parse().ok());
        let body = response.json::<Value>().await.ok();
        let detail = failure_detail(body, status);
        Err(Failure { status: Some(status), detail: detail.chars().take(500).collect(), retry_after, uncertain: post && status >= 500 })
    }

    pub async fn sessions(&self) -> Result<Vec<SessionInfo>, Failure> {
        let r = self.client.get(self.endpoint(None, None)).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn history(&self, name: &str, limit: usize, etag: Option<&str>) -> Result<History, Failure> {
        let mut url = self.endpoint(Some(name), Some("history"));
        url.query_pairs_mut().append_pair("limit", &limit.to_string());
        let mut req = self.client.get(url).timeout(Duration::from_secs(30));
        if let Some(tag) = etag { req = req.header(header::IF_NONE_MATCH, tag); }
        let r = Self::checked(req.send().await.map_err(|_| Failure::transport(false))?, false).await?;
        if r.status() == StatusCode::NOT_MODIFIED { return Ok(History { events: None, etag: etag.map(str::to_owned) }); }
        let etag = r.headers().get(header::ETAG).and_then(|h| h.to_str().ok()).map(str::to_owned);
        let events = r.json().await.map_err(|_| Failure::local("invalid_response"))?;
        Ok(History { events: Some(events), etag })
    }

    pub async fn send(&self, name: &str, text: &str) -> Result<Delivery, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("input")))
            .json(&json!({"text": text, "steer": false})).timeout(Duration::from_secs(60))
            .send().await.map_err(|_| Failure::transport(true))?;
        let delivery: Delivery = Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))?;
        if !delivery.ok { return Err(Failure::transport(true)); }
        Ok(delivery)
    }

    // Texto vai direto ao turno em voo (Codex, sem terminal); a resposta não traz `delivered`.
    pub async fn steer_text(&self, name: &str, text: &str) -> Result<Delivery, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("steer")))
            .json(&json!({"text": text})).timeout(Duration::from_secs(60))
            .send().await.map_err(|_| Failure::transport(true))?;
        let value: Value = Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) { return Err(Failure::transport(true)); }
        Ok(Delivery { ok: true, delivered: true, steered: true, native: false })
    }

    pub async fn commands(&self, name: &str) -> Result<Vec<CommandInfo>, Failure> {
        let r = self.client.get(self.endpoint(Some(name), Some("commands"))).timeout(Duration::from_secs(30))
            .send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    // Corpo cru, sem multipart. Queda depois de mandar é incerteza: o arquivo pode ter sido salvo.
    pub async fn upload(&self, name: &str, filename: &str, mime: &str, bytes: Vec<u8>) -> Result<Uploaded, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("upload")))
            .header(header::CONTENT_TYPE, mime)
            .header("X-Filename", crate::composer::encode_component(if filename.is_empty() { "arquivo" } else { filename }))
            .body(bytes).timeout(Duration::from_secs(UPLOAD_SECONDS))
            .send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    pub async fn uploads(&self, name: &str) -> Result<Vec<UploadFile>, Failure> {
        #[derive(serde::Deserialize)]
        struct Listing { #[serde(default)] files: Vec<UploadFile> }
        let r = self.client.get(self.endpoint(Some(name), Some("uploads"))).timeout(Duration::from_secs(30))
            .send().await.map_err(|_| Failure::transport(false))?;
        let listing: Listing = Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))?;
        Ok(listing.files)
    }

    // Bytes autenticados de um anexo: do cofre, de caminho citado ou de imagem do transcript.
    pub async fn fetch(&self, name: &str, source: &Source) -> Result<Vec<u8>, Failure> {
        let mut url = self.endpoint(Some(name), None);
        match source {
            Source::Upload(file) => { url.path_segments_mut().expect("validated HTTP base").extend(["uploads", file]); }
            Source::Cited(path) => {
                url.path_segments_mut().expect("validated HTTP base").push("file");
                url.query_pairs_mut().append_pair("path", path);
            }
            Source::Transcript(id, index) => { url.path_segments_mut().expect("validated HTTP base").extend(["transcript-image", id, &index.to_string()]); }
        }
        let r = self.client.get(url).timeout(Duration::from_secs(UPLOAD_SECONDS)).send().await.map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if r.content_length().is_some_and(|n| n > MAX_BYTES) { return Err(Failure::local("attach_too_big")); }
        let bytes = r.bytes().await.map_err(|_| Failure::transport(false))?;
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        Ok(bytes.to_vec())
    }

    pub async fn interrupt(&self, name: &str, clear: bool) -> Result<(), Failure> {
        let mut url = self.endpoint(Some(name), Some("interrupt"));
        url.query_pairs_mut().append_pair("clear", if clear { "true" } else { "false" });
        let r = self.client.post(url).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?;
        Ok(())
    }

    // Mutação sem retry: timeout ou queda depois de enviar vira incerteza, nunca repetição.
    pub async fn act(&self, name: &str, path: &[&str], body: Option<Value>, delete: bool, seconds: u64) -> Result<Value, Failure> {
        let mut url = self.endpoint(Some(name), None);
        url.path_segments_mut().expect("validated HTTP base").extend(path);
        let mut req = if delete { self.client.delete(url) } else { self.client.post(url) };
        if let Some(body) = body { req = req.json(&body); }
        let r = req.timeout(Duration::from_secs(seconds)).send().await.map_err(|_| Failure::transport(true))?;
        let r = Self::checked(r, true).await?;
        r.json().await.map_err(|_| Failure::transport(true))
    }

    // Leitura sem efeito colateral: queda é rede, nunca incerteza.
    pub async fn read(&self, name: &str, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let mut url = self.endpoint(Some(name), None);
        url.path_segments_mut().expect("validated HTTP base").extend(path);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        let r = self.client.get(url).timeout(Duration::from_secs(seconds)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn config(&self) -> Result<Value, Failure> {
        let mut url = self.base.clone();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "config"]);
        let r = self.client.get(url).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// `/api/<path>` do servidor (fora de uma sessão), com o método pedido.
    fn server_url(&self, path: &[&str], query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().push("api").extend(path);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        url
    }

    /// Leitura do servidor (cotação, diário, estado da atualização): queda é rede, nunca incerteza.
    pub async fn server_read(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.get(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// Arquivo inteiro do servidor (o diário), com o mesmo teto dos anexos.
    pub async fn server_bytes(&self, path: &[&str], seconds: u64) -> Result<Vec<u8>, Failure> {
        let r = self.client.get(self.server_url(path, &[])).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if r.content_length().is_some_and(|n| n > MAX_BYTES) { return Err(Failure::local("attach_too_big")); }
        let bytes = r.bytes().await.map_err(|_| Failure::transport(false))?;
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        Ok(bytes.to_vec())
    }

    /// Mutação no servidor, sem retry: queda depois de enviar é incerteza.
    pub async fn server_post(&self, path: &[&str], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.post(self.server_url(path, &[])).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Mutação (PUT, POST, DELETE), com ou sem corpo JSON, sem retry: queda depois de enviar é incerteza.
    pub async fn server_send(&self, method: reqwest::Method, path: &[&str], body: Option<Value>, seconds: u64) -> Result<Value, Failure> {
        let mut req = self.client.request(method, self.server_url(path, &[]));
        if let Some(body) = body { req = req.json(&body); }
        let r = req.timeout(Duration::from_secs(seconds)).send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// DELETE com parâmetros na URL (cancelar o login do Codex leva a tentativa na query).
    pub async fn server_delete(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.delete(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Paleta do papel de parede desta máquina. O backend só responde a pedidos locais: ligado a outro
    /// servidor volta 403, e 404 quer dizer que o desktop não gera paleta.
    pub async fn desktop_palette(&self) -> Result<Value, Failure> {
        let mut url = self.base.clone();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "desktop", "palette"]);
        let r = self.client.get(url).timeout(Duration::from_secs(10)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// Foto do papel de parede desta máquina, para o fundo Desktop em Vidro. Mesma regra da paleta: 403 fora
    /// do loopback, 404 sem papel de parede.
    pub async fn desktop_wallpaper(&self) -> Result<Vec<u8>, Failure> {
        let mut url = self.base.clone();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "desktop", "wallpaper"]);
        let r = self.client.get(url).timeout(Duration::from_secs(20)).send().await.map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        let limit = crate::media::BACKDROP_MAX_BYTES;
        if r.content_length().is_some_and(|n| n > limit) { return Err(Failure::local("backdrop_too_big")); }
        // Resposta sem tamanho declarado para no teto enquanto chega, sem juntar tudo antes de conferir.
        let mut body = Vec::new();
        let mut chunks = std::pin::pin!(r.bytes_stream());
        use futures::StreamExt;
        while let Some(chunk) = chunks.next().await {
            body.extend_from_slice(&chunk.map_err(|_| Failure::transport(false))?);
            if body.len() as u64 > limit { return Err(Failure::local("backdrop_too_big")); }
        }
        Ok(body)
    }

    async fn stream(&self, name: Option<&str>, cursor: &str) -> Result<Response, Failure> {
        let mut req = self.client.get(self.endpoint(name, Some("events"))).header(header::ACCEPT, "text/event-stream");
        if !cursor.is_empty() { req = req.header("Last-Event-ID", cursor); }
        let r = tokio::time::timeout(Duration::from_secs(15), req.send()).await
            .map_err(|_| Failure::transport(false))?.map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if !r.headers().get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("").starts_with("text/event-stream") {
            return Err(Failure::local("invalid_response"));
        }
        Ok(r)
    }
}

pub struct History { pub events: Option<Vec<ChatEvent>>, pub etag: Option<String> }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_detail_keeps_string_and_object_messages() {
        assert_eq!(failure_detail(Some(json!({"detail": "plain rejection"})), 400), "plain rejection");
        assert_eq!(failure_detail(Some(json!({"detail": {"code": "turn_missing", "params": {}, "msg": "Nenhum turno ativo"}})), 409), "Nenhum turno ativo");
        assert_eq!(failure_detail(Some(json!({"detail": {"code": "turn_missing", "params": {}}})), 409), "turn_missing");
        assert_eq!(failure_detail(Some(json!({"detail": [{"msg": "at most 40 characters"}]})), 422), "at most 40 characters");
    }
}
