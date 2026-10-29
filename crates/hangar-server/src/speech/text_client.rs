//! Cliente externo de texto; credenciais e conteúdo nunca passam por outro processo.
use std::{sync::OnceLock, time::Duration};

use serde_json::{Value, json};

use super::{SpeechError, config::SpeechConfig};

pub async fn chat(
    config: &SpeechConfig,
    system: &str,
    prompt: &str,
    temperature: f64,
    timeout: Duration,
) -> Result<String, SpeechError> {
    let (endpoint, key, model) = config.text_provider();
    if key.is_empty() {
        let detail = if endpoint == "https://api.groq.com/openai/v1" {
            "Chave do provedor não configurada: configure a organização do texto em Configurações → Voz."
        } else {
            "Chave do provedor não configurada: preencha a Chave do LLM em Configurações → Avançado."
        };
        return Err(SpeechError::new(503, detail));
    }
    let mut body = json!({"model":model,"messages":[{"role":"system","content":system},
        {"role":"user","content":prompt}],"temperature":temperature});
    let effort = config.string("llm_reasoning_effort");
    if !trim(&effort).is_empty() {
        body["reasoning_effort"] = json!(trim(&effort));
    }
    if prefer_throughput(&endpoint, &model) {
        body["provider"] = json!({"sort":"throughput"});
    }
    let response = client()?
        .post(format!(
            "{}/chat/completions",
            endpoint.trim_end_matches('/')
        ))
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .header("user-agent", "hangar/1.0")
        .timeout(timeout)
        .body(crate::transcript::pyjson::dumps_unicode(&body, false))
        .send()
        .await
        .map_err(transport_error)?;
    let status = response.status();
    if !status.is_success() {
        // O corpo pode conter credenciais em qualquer codificação; nenhuma variante é pública.
        let detail = match status.as_u16() {
            401 | 403 => "Autenticação recusada pelo serviço de texto.",
            429 => "Limite de chamadas atingido no serviço de texto.",
            300..=399 => "O serviço de texto tentou redirecionar a solicitação.",
            _ => "O serviço de texto respondeu com erro.",
        };
        return Err(SpeechError::new(
            502,
            format!("provedor {}: {detail}", status.as_u16()),
        ));
    }
    let bytes = read_response(response).await;
    parse_content(&bytes?)
}

fn transport_error(error: reqwest::Error) -> SpeechError {
    // Até a cadeia de causas pode repetir URL, credencial ou cabeçalho; só a categoria é segura.
    let detail = if error.is_timeout() {
        "O serviço de texto não respondeu no prazo."
    } else if error.is_connect() {
        "Não foi possível conectar ao serviço de texto."
    } else if error.is_builder() {
        "O endereço ou a credencial do serviço de texto são inválidos."
    } else {
        "Falha no transporte da solicitação ao serviço de texto."
    };
    SpeechError::new(502, detail)
}

fn client() -> Result<&'static reqwest::Client, SpeechError> {
    // O transporte conserva conexões; endpoint, credencial e prazo continuam sendo por pedido.
    // Um redirecionamento não autoriza enviar a credencial ao endereço novo.
    static CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            crate::http_client()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| SpeechError::new(502, "Não foi possível iniciar o cliente do provedor."))
}

fn prefer_throughput(endpoint: &str, model: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| host == "openrouter.ai")
        && !model.contains(':')
}

async fn read_response(mut response: reqwest::Response) -> Result<Vec<u8>, SpeechError> {
    const LIMIT: usize = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SpeechError::new(502, "Falha ao ler a resposta do provedor."))?
    {
        if chunk.len() > LIMIT - bytes.len() {
            return Err(SpeechError::new(
                502,
                "A resposta do provedor excede o limite de leitura.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_content(bytes: &[u8]) -> Result<String, SpeechError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| SpeechError::new(502, "A resposta do provedor não é UTF-8 válido."))?;
    let value: Value = serde_json::from_str(text)
        .map_err(|_| SpeechError::new(502, "A resposta do provedor não é um JSON válido."))?;
    let text = value["choices"]
        .get(0)
        .and_then(|v| v["message"]["content"].as_str())
        .ok_or_else(|| {
            SpeechError::new(502, "A resposta do provedor não contém o texto esperado.")
        })?;
    let text = trim(text);
    if text.is_empty() {
        return Err(SpeechError::new(502, "O provedor devolveu texto vazio."));
    }
    Ok(text.to_owned())
}

pub(super) fn digest(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

pub(super) fn profile(config: &SpeechConfig) -> String {
    let (endpoint, key, model) = config.text_provider();
    digest(&format!(
        "{endpoint}\0{key}\0{model}\0{}",
        config.string("llm_reasoning_effort")
    ))
}

pub(super) fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}'))
}

#[cfg(test)]
pub(super) mod testing {
    use super::*;
    use axum::{
        Router,
        extract::{Request, State},
        response::{IntoResponse, Response},
        routing::post,
    };
    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Clone)]
    struct Replies {
        values: Arc<Mutex<VecDeque<(u16, Value)>>>,
        hits: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
        headers: Arc<Mutex<Vec<axum::http::HeaderMap>>>,
        gate: Option<(usize, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    }

    pub struct FakeServer {
        pub config: SpeechConfig,
        pub hits: Arc<AtomicUsize>,
        pub bodies: Arc<Mutex<Vec<Value>>>,
        pub headers: Arc<Mutex<Vec<axum::http::HeaderMap>>>,
        task: tokio::task::JoinHandle<()>,
        _home: tempfile::TempDir,
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    pub async fn fake(values: Vec<(u16, Value)>) -> FakeServer {
        fake_gated(values, None).await
    }

    pub async fn fake_gated(
        values: Vec<(u16, Value)>,
        first_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    ) -> FakeServer {
        fake_controlled(
            values,
            first_gate.map(|(entered, release)| (0, entered, release)),
        )
        .await
    }

    pub async fn fake_gated_at(
        values: Vec<(u16, Value)>,
        index: usize,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> FakeServer {
        fake_controlled(values, Some((index, entered, release))).await
    }

    async fn fake_controlled(
        values: Vec<(u16, Value)>,
        gate: Option<(usize, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    ) -> FakeServer {
        async fn reply(State(state): State<Replies>, request: Request) -> Response {
            state
                .headers
                .lock()
                .unwrap()
                .push(request.headers().clone());
            let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap();
            state
                .bodies
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes).unwrap());
            let index = state.hits.fetch_add(1, Ordering::SeqCst);
            let (status, value) = state
                .values
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| (200, json!({"choices":[{"message":{"content":"ok"}}]})));
            if let Some((at, entered, release)) = &state.gate
                && index == *at
            {
                entered.notify_one();
                release.notified().await;
            }
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                [("content-type", "application/json")],
                value.to_string(),
            )
                .into_response()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Replies {
            values: Arc::new(Mutex::new(values.into())),
            hits: Arc::default(),
            bodies: Arc::default(),
            headers: Arc::default(),
            gate,
        };
        let router = Router::new()
            .route("/v1/chat/completions", post(reply))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let home = tempfile::tempdir().unwrap();
        let config = SpeechConfig {
            values: json!({"llm_base_url":format!("http://{address}/v1"),"llm_api_key":"fixture-key"}),
            home: home.path().to_owned(),
            cache_dir: PathBuf::new(),
        };
        FakeServer {
            config,
            hits: state.hits,
            bodies: state.bodies,
            headers: state.headers,
            task,
            _home: home,
        }
    }

    pub fn content(value: &str) -> (u16, Value) {
        (200, json!({"choices":[{"message":{"content":value}}]}))
    }

    pub fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/speech/text_contract.json"
        ))
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};

    #[tokio::test]
    async fn request_preserves_the_contract_and_trims_unicode() {
        let mut server = fake(vec![content("\u{1c} ação \u{85}")]).await;
        server.config.values["llm_model"] = json!("custom");
        server.config.values["llm_reasoning_effort"] = json!("none");
        assert_eq!(
            chat(
                &server.config,
                "sistema",
                "ação",
                0.3,
                Duration::from_secs(2)
            )
            .await
            .unwrap(),
            "ação"
        );
        let bodies = server.bodies.lock().unwrap();
        assert_eq!(
            bodies[0],
            json!({"model":"custom","messages":[{"role":"system","content":"sistema"},{"role":"user","content":"ação"}],"temperature":0.3,"reasoning_effort":"none"})
        );
        let headers = server.headers.lock().unwrap();
        assert_eq!(headers[0]["authorization"], "Bearer fixture-key");
        assert_eq!(headers[0]["user-agent"], "hangar/1.0");
        assert_eq!(headers[0]["content-type"], "application/json");
    }

    #[tokio::test]
    async fn external_failures_and_non_text_content_never_become_success() {
        for value in [
            json!({}),
            json!({"choices":[]}),
            json!({"choices":[{"message":{"content":null}}]}),
            json!({"choices":[{"message":{"content":[{"text":"oi"}]}}]}),
            json!({"choices":[{"message":{"content":"  "}}]}),
        ] {
            let server = fake(vec![(200, value)]).await;
            assert_eq!(
                chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
                    .await
                    .unwrap_err()
                    .status,
                502
            );
        }
        let server = fake(vec![(429, json!({"error":"limite"}))]).await;
        let error = chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(error.status, 502);
        assert!(error.detail.starts_with("provedor 429:"));
    }

    #[tokio::test]
    async fn a_missing_key_fails_before_the_provider() {
        let mut server = fake(vec![]).await;
        server.config.values["llm_api_key"] = json!("");
        assert_eq!(
            chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
                .await
                .unwrap_err()
                .status,
            503
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn provider_resolution_matches_the_original_python() {
        for case in fixture()["provider_cases"].as_array().unwrap() {
            let config = SpeechConfig {
                values: case["config"].clone(),
                home: Default::default(),
                cache_dir: Default::default(),
            };
            assert_eq!(json!(config.text_provider()), case["expected"]);
        }
    }

    #[test]
    fn throughput_is_only_added_to_openrouter_without_a_user_suffix() {
        assert!(prefer_throughput(
            "https://openrouter.ai/api/v1",
            "openai/gpt-x"
        ));
        assert!(!prefer_throughput(
            "https://openrouter.ai/api/v1",
            "openai/gpt-x:floor"
        ));
        assert!(!prefer_throughput(
            "https://other.invalid/v1",
            "openai/gpt-x"
        ));
        assert!(!prefer_throughput("not a URL", "openai/gpt-x"));
    }

    #[tokio::test]
    async fn absent_reasoning_effort_is_absent_from_the_request() {
        let server = fake(vec![content("texto")]).await;
        chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(
            server.bodies.lock().unwrap()[0]
                .get("reasoning_effort")
                .is_none()
        );
    }

    #[tokio::test]
    async fn redirect_never_contacts_the_destination_with_the_credential() {
        let destination = fake(vec![]).await;
        let target = format!(
            "{}/chat/completions",
            destination.config.string("llm_base_url")
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new().fallback(move || {
            let target = target.clone();
            async move {
                (
                    axum::http::StatusCode::TEMPORARY_REDIRECT,
                    [("location", target)],
                    "moved",
                )
            }
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config = destination.config.clone();
        config.values["llm_base_url"] = json!(format!("http://{address}/v1"));
        let error = chat(&config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        task.abort();
        assert!(error.detail.starts_with("provedor 307:"));
        assert_eq!(
            destination.hits.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn oversized_success_and_invalid_json_are_visible_provider_errors() {
        let server = fake(vec![content(&"x".repeat(4 * 1024 * 1024))]).await;
        let error = chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.detail.contains("limite de leitura"));
        assert_eq!(parse_content(b"not JSON").unwrap_err().status, 502);
    }

    #[tokio::test]
    async fn http_failure_only_returns_the_status_and_a_safe_cause() {
        let value = json!({"error":"ação".repeat(300)});
        let server = fake(vec![(500, value)]).await;
        let error = chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(
            error.detail,
            "provedor 500: O serviço de texto respondeu com erro."
        );
    }

    #[tokio::test]
    async fn a_provider_error_never_returns_the_credential_it_echoed() {
        use base64::Engine;
        let escaped = "fixture-key"
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect::<String>();
        let encoded = base64::engine::general_purpose::STANDARD.encode("fixture-key");
        for echoed in ["fixture-key".to_owned(), escaped, encoded] {
            let server = fake(vec![(401, json!({"error":echoed}))]).await;
            let error = chat(&server.config, "s", "p", 0.0, Duration::from_secs(2))
                .await
                .unwrap_err();
            assert_eq!(error.status, 502);
            assert_eq!(
                error.detail,
                "provedor 401: Autenticação recusada pelo serviço de texto."
            );
            assert!(!error.detail.contains(&echoed));
        }
    }

    #[tokio::test]
    async fn transport_failures_never_return_the_url_or_its_secret_components() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = SpeechConfig {
            values: json!({"llm_base_url":format!("http://fixture-key@{address}/v1?secret=Zml4dHVyZS1rZXk="),"llm_api_key":"fixture-key"}),
            home: Default::default(),
            cache_dir: Default::default(),
        };
        let error = chat(&config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(error.status, 502);
        assert!(matches!(
            error.detail.as_str(),
            "Não foi possível conectar ao serviço de texto."
                | "O serviço de texto não respondeu no prazo."
        ));
        assert!(!error.detail.contains("fixture-key"));
        assert!(!error.detail.contains("Zml4dHVyZS1rZXk="));
        assert!(!error.detail.contains(&address.to_string()));
        let mut invalid = config.clone();
        invalid.values["llm_base_url"] = json!("not-a-url/fixture-key");
        let error = chat(&invalid, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(error.status, 502);
        assert_eq!(
            error.detail,
            "O endereço ou a credencial do serviço de texto são inválidos."
        );
    }

    async fn raw_response(response: Vec<u8>) -> (SpeechConfig, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0, "a requisição deve terminar antes da resposta");
                received.extend_from_slice(&chunk[..count]);
                if let Some(head) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..head]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if received.len() >= head + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let config = SpeechConfig {
            values: json!({"llm_base_url":format!("http://{address}/v1"),"llm_api_key":"fixture"}),
            home: Default::default(),
            cache_dir: Default::default(),
        };
        (config, task)
    }

    #[tokio::test]
    async fn an_unreadable_http_error_body_keeps_the_upstream_status() {
        let (config,task)=raw_response(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial".to_vec()).await;
        let error = chat(&config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        task.await.unwrap();
        assert_eq!(error.status, 502);
        assert_eq!(
            error.detail,
            "provedor 503: O serviço de texto respondeu com erro."
        );
    }

    #[tokio::test]
    async fn invalid_utf8_in_a_successful_provider_response_is_a_visible_error() {
        let body = b"{\"choices\":[{\"message\":{\"content\":\"invalid \xff\"}}]}";
        let mut response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes();
        response.extend_from_slice(body);
        let (config, task) = raw_response(response).await;
        let error = chat(&config, "s", "p", 0.0, Duration::from_secs(2))
            .await
            .unwrap_err();
        task.await.unwrap();
        assert_eq!(error.status, 502);
        assert_eq!(error.detail, "A resposta do provedor não é UTF-8 válido.");
        let valid = json!({"choices":[{"message":{"content":"caractere legítimo �"}}]}).to_string();
        assert_eq!(
            parse_content(valid.as_bytes()).unwrap(),
            "caractere legítimo �"
        );
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_provider_bytes_do_not_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096)) {
            let _ = parse_content(&bytes);
        }
    }
}
