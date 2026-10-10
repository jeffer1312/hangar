//! Falha que o Rust atende sozinho vai ao diário do Python (`POST /internal/diag`): o
//! `hangar-server.log` não entra no arquivo que o dono exporta. Uma vez por minuto por (diário, sessão, código).
use axum::body::Body;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// Código da falha que uma rota do Rust devolveu, para o diário sem reler o corpo da resposta.
#[derive(Clone, Copy)]
pub(crate) struct FailureCode(pub &'static str);

pub(crate) fn coded(mut response: axum::response::Response, code: &'static str) -> axum::response::Response {
    response.extensions_mut().insert(FailureCode(code));
    response
}

#[derive(Clone)]
pub struct DiagClient { upstream:SocketAddr,secret:String,http:crate::proxy::HttpClient }

impl DiagClient {
    pub fn new(upstream:SocketAddr,secret:String) -> Self { Self { upstream,secret,http:crate::proxy::client() } }

    /// `event` começa com `rust.`; `reason` é frase fixa do código (por isso `'static`), nunca texto
    /// de conversa nem erro formatado, que pode ecoar valores.
    /// Falha marcada por `coded`: conflito e erro do servidor vão ao diário; entrada recusada
    /// (400, 404, 413, 422) é do usuário e só volta na resposta.
    pub(crate) fn report_response(&self,event:&'static str,scope:&str,response:&axum::response::Response,reason:&'static str) {
        let status = response.status().as_u16();
        if let Some(FailureCode(code)) = response.extensions().get::<FailureCode>().copied()
            && (status == 409 || status >= 500) {
            self.report(event,scope,code,reason);
        }
    }

    pub fn report(&self,event:&'static str,session:&str,code:&str,reason:&'static str) {
        // Por diário (o Python de destino): em produção há um só, e cada servidor de teste tem o seu.
        if !crate::warn_limit::allow(Some(session),&format!("diag:{}:{event}:{code}",self.upstream)) { return; }
        let session:String = session.chars().take(128).collect();
        let body = json!({"evento":event,"sessao":session,"codigo":code,"motivo":reason}).to_string();
        let (client,code) = (self.clone(),code.to_owned());
        tokio::spawn(async move {
            let request = axum::http::Request::post(format!("http://{}/internal/diag",client.upstream))
                .header("x-hangar-internal",&client.secret).header("content-type","application/json").body(Body::from(body));
            let detail = match request {
                Err(_) => "pedido inválido".into(),
                Ok(request) => match tokio::time::timeout(Duration::from_secs(5),client.http.request(request)).await {
                    Ok(Ok(response)) if response.status().is_success() => return,
                    Ok(Ok(response)) => format!("status={}",response.status().as_u16()),
                    Ok(Err(error)) => format!("connect={} {error}",error.is_connect()),
                    Err(_) => "timeout".into(),
                },
            };
            // O registro que não chegou fica inteiro no log, para não sumir.
            tracing::warn!(event,session=%session,code=%code,reason,detail=%detail,"diário do Python não recebeu a falha");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,Mutex};
    use tokio::io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt,BufReader};

    type Hits = Arc<Mutex<Vec<(String,serde_json::Value)>>>;

    /// Python falso do `/internal/diag`: guarda segredo e corpo de cada registro.
    async fn fake_python() -> (SocketAddr,Hits) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits:Hits = Arc::default();
        let seen = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream,_)) = listener.accept().await else { return };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let (mut length,mut secret) = (0usize,String::new());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if line == "\r\n" { break; }
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                        if let Some(value) = lower.strip_prefix("x-hangar-internal:") { secret = value.trim().into(); }
                    }
                    // Processos desta máquina sondam portas efêmeras; sem o segredo não é o cliente.
                    if secret.is_empty() { return; }
                    let mut body = vec![0;length]; reader.read_exact(&mut body).await.unwrap();
                    seen.lock().unwrap().push((secret,serde_json::from_slice(&body).unwrap()));
                    reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await.unwrap();
                });
            }
        });
        (address,hits)
    }

    async fn wait_hits(hits:&Hits,count:usize) {
        tokio::time::timeout(Duration::from_secs(5),async {
            while hits.lock().unwrap().len() < count { tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn report_reaches_python_once_per_minute_with_the_secret() {
        let (address,hits) = fake_python().await;
        let client = DiagClient::new(address,"secret-test".into());
        client.report("rust.history_failed","diag-session","history_io","leitura do transcript falhou");
        wait_hits(&hits,1).await;
        client.report("rust.history_failed","diag-session","history_io","leitura do transcript falhou");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let hits = hits.lock().unwrap();
        assert_eq!(hits.len(),1,"repetição no mesmo minuto não sai");
        assert_eq!(hits[0].0,"secret-test");
        assert_eq!(hits[0].1,json!({"evento":"rust.history_failed","sessao":"diag-session","codigo":"history_io",
            "motivo":"leitura do transcript falhou"}));
    }

    /// O limite é por diário: outro Python (outro servidor no mesmo processo) recebe o seu registro.
    #[tokio::test]
    async fn the_limit_is_per_diary_not_per_process() {
        let (first,first_hits) = fake_python().await;
        let (second,second_hits) = fake_python().await;
        DiagClient::new(first,"secret-test".into()).report("rust.history_failed","per-diary","history_io","leitura do transcript falhou");
        wait_hits(&first_hits,1).await;
        DiagClient::new(second,"secret-test".into()).report("rust.history_failed","per-diary","history_io","leitura do transcript falhou");
        wait_hits(&second_hits,1).await;
    }
}
