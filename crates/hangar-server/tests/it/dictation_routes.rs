//! Organização preserva a transcrição e não escolhe outro serviço quando o solicitado falha.
use axum::{
    Router,
    routing::{get, post},
};
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn serve(router: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, task)
}

async fn organize(mode: &str, response: Value) -> (Value, usize) {
    organize_text(
        mode,
        response,
        "  Hoje vamos conferir o ditado completo.\n",
        "limpar",
    )
    .await
}

pub(crate) async fn organize_text(
    mode: &str,
    response: Value,
    raw: &str,
    style: &str,
) -> (Value, usize) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let (llm, llm_task) = serve(Router::new().route(
        "/v1/chat/completions",
        post(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let response = response.clone();
            async move { ([("content-type", "application/json")], response.to_string()) }
        }),
    ))
    .await;
    let config = json!({"organization": {"dictation_organization_mode": mode,
        "llm_base_url":format!("http://{llm}/v1"), "llm_api_key":"fixture-key", "llm_model":"fixture-model",
        "ditado_estilo":"limpar"}});
    let (upstream, upstream_task) = serve(Router::new().route(
        "/internal/transcription/config",
        get(move || {
            let config = config.clone();
            async move { ([("content-type", "application/json")], config.to_string()) }
        }),
    ))
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(Config {
        listen: addr,
        upstream,
        internal_secret: "dictation-secret".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/__hangar_server/dictation/organize"))
        .header("x-hangar-internal", "dictation-secret")
        .header("content-type", "application/json")
        .body(json!({"raw":raw, "style":style}).to_string())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    server.abort();
    upstream_task.abort();
    llm_task.abort();
    assert_eq!(status, 200, "organização não respondeu: {body}");
    (
        serde_json::from_str::<Value>(&body).unwrap()["result"].clone(),
        hits.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn dictation_none_preserves_whitespace_and_never_contacts_configured_llm() {
    let (result, hits) = organize(
        "none",
        json!({"choices":[{"message":{"content":"Texto indevido."}}]}),
    )
    .await;
    assert_eq!(result["text"], "  Hoje vamos conferir o ditado completo.\n");
    assert_eq!(result["raw"], result["text"]);
    assert_eq!(result["estilo_aplicado"], "cru");
    assert!(result["aviso"].is_null());
    assert_eq!(hits, 0, "o modo desligado consultou o LLM");
}

#[tokio::test]
async fn dictation_external_preserves_raw_when_provider_returns_no_text() {
    let (result, hits) = organize(
        "external_api",
        json!({"choices":[{"message":{"content":null}}]}),
    )
    .await;
    assert_eq!(result["text"], "  Hoje vamos conferir o ditado completo.\n");
    assert_eq!(result["organization_code"], "dictation_organization_failed");
    assert_eq!(hits, 1, "a tentativa foi repetida em outro serviço");
}

#[tokio::test]
async fn dictation_external_applies_style_without_changing_the_transcription() {
    let (result, hits) = organize(
        "external_api",
        json!({"choices":[{"message":{"content":"Hoje vamos conferir o ditado completo."}}]}),
    )
    .await;
    assert_eq!(result["text"], "Hoje vamos conferir o ditado completo.");
    assert_eq!(result["raw"], "  Hoje vamos conferir o ditado completo.\n");
    assert_eq!(result["estilo_aplicado"], "limpar");
    assert!(result["aviso"].is_null());
    assert_eq!(hits, 1);
}

#[tokio::test]
async fn dictation_guards_reject_invention_summary_and_invisible_output() {
    for output in [
        "A aplicação precisa configurar um servidor novo de produção.",
        "Resumo.",
        "\u{200b}\u{feff}",
    ] {
        let raw = "Precisamos conferir o ditado completo preservando os arquivos, os caminhos e todas as restrições descritas pela pessoa durante a gravação.";
        let (result, hits) = organize_text(
            "external_api",
            json!({"choices":[{"message":{"content":output}}]}),
            raw,
            "limpar",
        )
        .await;
        assert_eq!(result["text"], raw, "a saída indevida foi aceita: {output}");
        assert_eq!(result["organization_code"], "dictation_output_rejected");
        assert_eq!(hits, 1);
    }
}

#[tokio::test]
async fn dictation_guards_allow_conjugation_and_spoken_correction() {
    for (raw, output, style) in [
        (
            "eu clicava ali e trocava o modelo seguindo o padrao",
            "Eu clico ali e troco o modelo, seguir o padrão.",
            "limpar",
        ),
        ("usa o postgres nao o redis", "Usa o Redis.", "prosa"),
    ] {
        let (result, hits) = organize_text(
            "external_api",
            json!({"choices":[{"message":{"content":output}}]}),
            raw,
            style,
        )
        .await;
        assert_eq!(result["text"], output);
        assert!(result["aviso"].is_null());
        assert_eq!(hits, 1);
    }
    let (result, _) = organize_text(
        "external_api",
        json!({"choices":[{"message":{"content":"Usa o Redis."}}]}),
        "usa o postgres nao o redis",
        "limpar",
    )
    .await;
    assert_eq!(result["text"], "usa o postgres nao o redis");
    assert_eq!(result["organization_code"], "dictation_output_rejected");
}

#[tokio::test]
async fn dictation_short_briefing_uses_prose_and_slash_command_never_calls_llm() {
    let raw = "Hoje vamos conferir o ditado completo.";
    let (result, _) = organize_text(
        "external_api",
        json!({"choices":[{"message":{"content":raw}}]}),
        raw,
        "briefing",
    )
    .await;
    assert_eq!(result["estilo_aplicado"], "cru");
    let (result, hits) = organize_text("external_api", json!({}), "/clear", "limpar").await;
    assert_eq!(result["text"], "/clear");
    assert!(result["aviso"].is_null());
    assert_eq!(hits, 0);
}

#[tokio::test]
async fn dictation_preserves_the_measured_cases_from_the_previous_organizer() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../../resources/dictation-cases.json")).unwrap();
    for case in cases {
        let raw = case["raw"].as_str().unwrap();
        let output = case["output"].as_str().unwrap();
        let (result, hits) = organize_text(
            "external_api",
            json!({"choices":[{"message":{"content":output}}]}),
            raw,
            case["style"].as_str().unwrap(),
        )
        .await;
        assert_eq!(hits, 1);
        if case["accepted"] == true {
            assert!(
                result["aviso"].is_null(),
                "{} rejeitado: {result}",
                case["name"]
            );
            assert_ne!(result["text"], raw);
        } else {
            assert_eq!(result["text"], raw, "{} aceito indevidamente", case["name"]);
            assert_eq!(result["organization_code"], "dictation_output_rejected");
        }
    }
}
