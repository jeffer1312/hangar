//! Cliente da API C2 do binário, em `127.0.0.1:<port>` do `control.json`. Todo pedido sai assinado e
//! toda resposta só vale com a assinatura do binário conferida.
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::control::{self, Control, ReadError, SIGNATURE_HEADER};

const TIMEOUT: Duration = Duration::from_secs(15);
/// O C2 corta em 200 mensagens por página; o teto só impede que uma resposta sem fim encha a memória.
const MAX_REPLY: usize = 16 << 20;

/// O que o binário respondeu, já com a assinatura conferida.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    /// Sem `control.json`, com o processo dele morto ou sem conexão na porta: o serviço está fora.
    Off(String),
    /// Resposta sem assinatura válida, ou chave ilegível para conferir.
    Untrusted(String),
    /// Conexão aberta que falhou no meio, prazo esgotado ou resposta grande demais.
    Transport(String),
}

#[derive(Clone)]
pub struct Client {
    data: PathBuf,
    http: reqwest::Client,
}

impl Client {
    pub fn new(data: PathBuf) -> Self {
        let http = hangar_core::net::http_client().timeout(TIMEOUT).no_proxy().build().unwrap_or_default();
        Self { data, http }
    }

    /// O `control.json` com o processo dele vivo; é o que diz que o serviço está de pé.
    pub fn running(&self) -> Option<Control> {
        control::read_control(&self.data).ok().filter(control::alive)
    }

    /// `path` é o caminho do C2 (`/v1/chats`); `query` vai codificada como formulário.
    ///
    /// # Errors
    ///
    /// [`CallError::Off`] sem serviço; [`CallError::Untrusted`] quando a resposta não tem a assinatura
    /// do binário; [`CallError::Transport`] para o resto.
    pub async fn call(&self, method: reqwest::Method, path: &str, query: &[(&str, &str)], body: Option<&Value>) -> Result<Reply, CallError> {
        let body = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let first = self.control()?;
        match self.send(&first, &method, path, query, &body).await {
            Err(Attempt::Connect(detail)) => {
                // O binário pode ter reiniciado noutra porta: relê o `control.json` uma vez.
                let again = self.control()?;
                if again == first {
                    return Err(CallError::Off(detail));
                }
                self.send(&again, &method, path, query, &body).await.map_err(Attempt::into_error)
            }
            other => other.map_err(Attempt::into_error),
        }
    }

    /// `GET /v1/status` com 200.
    ///
    /// # Errors
    ///
    /// Os de [`Client::call`], e [`CallError::Transport`] para outro status ou corpo que não é JSON.
    pub async fn status(&self) -> Result<Value, CallError> {
        let reply = self.call(reqwest::Method::GET, "/v1/status", &[], None).await?;
        if reply.status != 200 {
            return Err(CallError::Transport(format!("status respondeu {}", reply.status)));
        }
        serde_json::from_slice(&reply.body).map_err(|e| CallError::Transport(format!("status ilegível: {e}")))
    }

    fn control(&self) -> Result<Control, CallError> {
        match control::read_control(&self.data) {
            Ok(control) if control::alive(&control) => Ok(control),
            Ok(control) => Err(CallError::Off(format!("o processo {} do control.json não existe", control.pid))),
            Err(ReadError::Missing) => Err(CallError::Off("sem control.json".into())),
            Err(ReadError::Invalid(detail)) => Err(CallError::Off(detail)),
        }
    }

    async fn send(&self, at: &Control, method: &reqwest::Method, path: &str, query: &[(&str, &str)], body: &[u8]) -> Result<Reply, Attempt> {
        let key = control::read_key(&self.data).map_err(|e| Attempt::Untrusted(format!("control.key: {e:?}")))?;
        let mut url = reqwest::Url::parse(&format!("http://127.0.0.1:{}{path}", at.port))
            .map_err(|e| Attempt::Transport(format!("endereço: {e}")))?;
        if !query.is_empty() {
            url.set_query(Some(&form_urlencoded::Serializer::new(String::new()).extend_pairs(query).finish()));
        }
        // Assina a forma que vai no fio: caminho e query do próprio `Url` que o reqwest envia.
        let uri = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut request = self.http.request(method.clone(), url)
            .header(SIGNATURE_HEADER, control::sign_request(&key, method.as_str(), &uri, ts, body));
        if !body.is_empty() {
            request = request.header("content-type", "application/json").body(body.to_vec());
        }
        let mut response = request.send().await.map_err(|e| {
            if e.is_connect() { Attempt::Connect(e.to_string()) } else { Attempt::Transport(e.to_string()) }
        })?;
        let status = response.status().as_u16();
        let signature = response.headers().get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| Attempt::Transport(e.to_string()))? {
            if data.len() + chunk.len() > MAX_REPLY {
                return Err(Attempt::Transport("resposta grande demais".into()));
            }
            data.extend_from_slice(&chunk);
        }
        let Some(signature) = signature else {
            return Err(Attempt::Untrusted(format!("resposta {status} sem assinatura")));
        };
        if !control::verify_response(&key, &ts.to_string(), status, &data, &signature) {
            return Err(Attempt::Untrusted(format!("resposta {status} com assinatura que não confere")));
        }
        Ok(Reply { status, body: data })
    }
}

enum Attempt {
    Connect(String),
    Untrusted(String),
    Transport(String),
}

impl Attempt {
    fn into_error(self) -> CallError {
        match self {
            Self::Connect(detail) => CallError::Off(detail),
            Self::Untrusted(detail) => CallError::Untrusted(detail),
            Self::Transport(detail) => CallError::Transport(detail),
        }
    }
}
