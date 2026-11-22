//! Diretório de convidados no serviço: a máquina entrega os hashes dos nomes que ela guarda, para a tela
//! de entrada achar onde cada convidado entra. O nome em texto nunca vai ao serviço.
use std::time::Duration;

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;

use super::ConnectError;
use super::state::{Linked, service_base};

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    taken: Vec<String>,
}

/// Troca a lista desta máquina pela de `user_hashes` (vazia apaga). Devolve os hashes que já são de outra conta.
pub async fn publish(linked: &Linked, user_hashes: &[String]) -> Result<Vec<String>, ConnectError> {
    let url = service_base(&linked.service_url)?.join("/v1/guests").map_err(|_| Linked::invalid())?;
    // Sem seguir redirecionamento: a chave da máquina iria junto para outro endereço.
    let client = hangar_core::net::http_client().redirect(reqwest::redirect::Policy::none()).timeout(TIMEOUT).build()
        .map_err(|_| ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_guests_unreachable", "cliente HTTP indisponível"))?;
    let response = client.put(url).bearer_auth(&linked.machine_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(json!({"user_hashes": user_hashes}).to_string())
        .send().await
        .map_err(|_| ConnectError::new(StatusCode::BAD_GATEWAY, "connect_guests_unreachable", "o serviço do Connect não respondeu"))?;
    let status = response.status();
    if !status.is_success() {
        // 404: serviço ainda sem o diretório. 409: máquina pendente ou revogada.
        let code = match status.as_u16() {
            404 => "connect_guests_unsupported",
            401 | 403 | 409 => "connect_guests_refused",
            429 => "connect_service_busy",
            _ => "connect_guests_failed",
        };
        return Err(ConnectError::new(StatusCode::BAD_GATEWAY, code, format!("o serviço do Connect recusou os convidados (HTTP {})", status.as_u16())));
    }
    let invalid = || ConnectError::new(StatusCode::BAD_GATEWAY, "connect_guests_invalid", "o serviço do Connect devolveu uma resposta inválida");
    let bytes = response.bytes().await.map_err(|_| invalid())?;
    let answer: Answer = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    Ok(answer.taken.into_iter().filter(|hash| user_hashes.contains(hash)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn linked(service_url: String) -> Linked {
        Linked { service_url, account: "acme".into(), machine_id: "pc-a".into(), machine_key: "chave-da-maquina".into(),
            host: "pc-a.acme.hangar.dev.br".into(), guest_host: String::new(), preview_host: String::new(), frps_addr: String::new(),
            frps_token: String::new(), service_public_key: String::new(), guest_published: false, preview_published: false }
    }

    /// Serviço falso: responde `status`/`body` uma vez e devolve o pedido cru que recebeu.
    async fn service(status: &'static str, body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw);
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    if rest.len() >= length { break; }
                }
                if n == 0 { break; }
            }
            let reply = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            stream.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&raw).into_owned()
        });
        (format!("http://{addr}"), task)
    }

    #[tokio::test]
    async fn publishes_the_hashes_with_the_machine_key_and_returns_the_taken_ones() {
        let (base, request) = service("200 OK", r#"{"taken":["bbb","de-outro-pedido"]}"#).await;
        let taken = publish(&linked(base), &["aaa".into(), "bbb".into()]).await.unwrap();
        assert_eq!(taken, vec!["bbb".to_owned()], "só o que foi pedido vale");
        let raw = request.await.unwrap();
        assert!(raw.starts_with("PUT /v1/guests HTTP/1.1"), "{raw}");
        assert!(raw.to_ascii_lowercase().contains("authorization: bearer chave-da-maquina"), "{raw}");
        assert!(raw.ends_with(r#"{"user_hashes":["aaa","bbb"]}"#), "{raw}");
    }

    #[tokio::test]
    async fn an_old_service_without_the_directory_is_its_own_code() {
        let (base, _request) = service("404 Not Found", "{}").await;
        assert_eq!(publish(&linked(base), &[]).await.unwrap_err().code, "connect_guests_unsupported");
        let (base, _request) = service("409 Conflict", "{}").await;
        assert_eq!(publish(&linked(base), &[]).await.unwrap_err().code, "connect_guests_refused");
    }
}
