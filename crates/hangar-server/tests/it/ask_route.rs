//! `POST /api/ask-history` (R4, ASK-*): termos da pergunta, busca OU no índice e `claude -p`. O
//! esperado vem da captura do Python (`tests/fixtures/search/contract/ask/ask.json`) sobre as mesmas
//! fixtures da busca (cena de `search_route`), com as diferenças aprovadas da R2 nos trechos (F4, M8,
//! N1). O `claude -p` é um `Runner` falso: guarda o prompt e a conta de cada chamada.
use super::fake::{OWNER, client};
use super::search_route::{FROM_COPY_MTIME, Opts, REWRITTEN, Scene, reranked};
use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use hangar_server::claude_p::{Output, Runner};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn capture() -> Value {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/contract/ask/ask.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["data"].clone()
}

/// As saídas do `subprocess.run` falso da captura, por caso.
fn output(kind: &str) -> Output {
    let (code, stdout, stderr) = match kind {
        "ok" => (0, "  resposta da sessão sessao-viva \n", ""),
        "fail" => (1, "", "boom no stderr"),
        "empty" => (0, "   ", ""),
        "limit" => (0, "You've hit your weekly limit · resets 3pm", ""),
        _ => unreachable!("{kind}"),
    };
    Output { code, stdout: stdout.into(), stderr: stderr.into() }
}

/// Sem cotas lidas, como a captura (`cotas_claude = []`): só a conta padrão.
#[derive(Default)]
struct FakeCli { answers: Mutex<Vec<Output>>, calls: Mutex<Vec<(String, Option<String>)>> }

impl FakeCli {
    fn answering(kinds: &[&str]) -> Arc<Self> {
        Arc::new(Self { answers: Mutex::new(kinds.iter().map(|k| output(k)).collect()), ..Self::default() })
    }
    fn calls(&self) -> Vec<(String, Option<String>)> { self.calls.lock().unwrap().clone() }
}

impl Runner for FakeCli {
    fn run<'a>(&'a self, prompt: &'a str, account: Option<&'a str>) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((prompt.to_owned(), account.map(str::to_owned)));
            Ok(self.answers.lock().unwrap().remove(0))
        })
    }
    fn quotas(&self, _fresh: bool) -> BoxFuture<'_, Result<Vec<Value>, String>> { Box::pin(async { Ok(Vec::new()) }) }
}

async fn scene(cli: &Arc<FakeCli>) -> Scene {
    Scene::new(Opts { claude_p: Some(cli.clone()), ..Opts::default() }).await
}

async fn post(scene: &Scene, body: &[u8], owner: bool) -> (StatusCode, Value) {
    let mut req = client().post(format!("http://{}/api/ask-history", scene.address))
        .header("content-type", "application/json").body(body.to_vec());
    if owner { req = req.bearer_auth(OWNER); }
    let response = req.send().await.unwrap();
    let status = response.status();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

async fn ask(scene: &Scene, question: &str) -> (StatusCode, Value) {
    post(scene, json!({"question": question}).to_string().as_bytes(), true).await
}

/// Os trechos da captura com as diferenças aprovadas da busca: sem a cópia `.from-` (F4), sem a linha
/// que o `--resume` regravou (M8) e com agente e conta (N1).
fn expected_hits(scene: &Scene, hits: &Value) -> Value {
    Value::Array(hits.as_array().unwrap().iter()
        .filter(|h| h["mtime"].as_f64() != Some(FROM_COPY_MTIME) && h["event_id"] != REWRITTEN)
        .map(|h| scene.with_account(h)).collect())
}

#[tokio::test]
async fn ask_route_answers_with_the_cli_like_the_capture() {
    // ASK-06, 08, 13, 14, 17, 20, 25 e 33: os trechos completos e o stdout aparado sem corte; o prompt
    // vai ao CLI na conta padrão.
    let want = &capture()["route"]["ok"];
    let cli = FakeCli::answering(&["ok"]);
    let scene = scene(&cli).await;
    let (status, body) = ask(&scene, "onde falei sobre o clippy?").await;
    assert_eq!(status.as_u16(), want["status"].as_u64().unwrap() as u16, "{body}");
    assert_eq!(body["answer"], want["body"]["answer"]);
    let (got, want_hits) = reranked(&body["hits"], &expected_hits(&scene, &want["body"]["hits"]), false);
    assert_eq!(got, want_hits);
    assert_eq!(cli.calls(), [(want["calls"][0]["input"].as_str().unwrap().to_owned(), None)]);
}

#[tokio::test]
async fn ask_route_searches_each_term_and_builds_the_prompt_like_the_capture() {
    // ASK-10..14, 16 e 17: OU entre termos, sem repetir (sessão, trecho), mtime decrescente; rótulo
    // viva/arquivada com o nome da sessão ou a pasta do projeto; só stopwords ou nada achado não chama o CLI.
    let cap = capture();
    for (q, want_prompt) in cap["prompt"].as_object().unwrap() {
        let cli = FakeCli::answering(&["ok"]);
        let scene = scene(&cli).await;
        let (status, body) = ask(&scene, q).await;
        assert_eq!(status, StatusCode::OK, "{q}: {body}");
        let want_hits = expected_hits(&scene, &cap["search_terms"][q]);
        let (got, want_hits) = reranked(&body["hits"], &want_hits, false);
        assert_eq!(got, want_hits, "q={q:?}");
        if want_hits.as_array().unwrap().is_empty() {
            assert_eq!(body, cap["route"]["no_hits"]["body"], "q={q:?}");
            assert!(cli.calls().is_empty(), "q={q:?}");
        } else {
            assert_eq!(cli.calls(), [(want_prompt.as_str().unwrap().to_owned(), None)], "q={q:?}");
        }
    }
}

#[tokio::test]
async fn ask_route_cli_failure_is_502_with_the_message_as_text() {
    // ASK-09, 28..32: `detail` é a frase crua do `claude -p` (string); limite com uma conta só não tenta outra.
    let cap = capture();
    for kind in ["fail", "empty", "limit"] {
        let want = &cap["route"][kind];
        let cli = FakeCli::answering(&[kind]);
        let scene = scene(&cli).await;
        let (status, body) = ask(&scene, "onde falei sobre o clippy?").await;
        assert_eq!(status.as_u16(), want["status"].as_u64().unwrap() as u16, "{kind}: {body}");
        assert_eq!(body, want["body"], "{kind}");
        assert_eq!(cli.calls().len(), want["calls"].as_array().unwrap().len(), "{kind}");
    }
}

#[tokio::test]
async fn ask_route_without_hits_answers_empty_without_the_cli() {
    // ASK-07 e ASK-12.
    let cap = capture();
    let cli = FakeCli::answering(&[]);
    let scene = scene(&cli).await;
    for q in ["zzznadaqui qqqwww", "o que é isso"] {
        assert_eq!(ask(&scene, q).await, (StatusCode::OK, cap["route"]["no_hits"]["body"].clone()), "q={q:?}");
    }
    assert!(cli.calls().is_empty());
}

#[tokio::test]
async fn ask_route_kill_switch_refuses_before_the_cli() {
    // ASK-04 e ASK-05: `automations` do `runtime-config.json`; o corpo é validado antes, como no FastAPI.
    let cap = capture();
    let cli = FakeCli::answering(&[]);
    let scene = scene(&cli).await;
    std::fs::write(scene.home.join(".claude-a/runtime-config.json"), r#"{"automations": false}"#).unwrap();
    let want = &cap["route"]["kill_switch"];
    let (status, body) = ask(&scene, "onde falei sobre o clippy?").await;
    assert_eq!((status.as_u16(), &body), (want["status"].as_u64().unwrap() as u16, &want["body"]));
    assert_eq!(post(&scene, br#"{"question": ""}"#, true).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(cli.calls().is_empty());
}

#[tokio::test]
async fn ask_route_body_is_strict_like_fastapi() {
    // ASK-02 e ASK-03: corpos de 422 do FastAPI/pydantic com o modelo `AskHistoryBody` (capturados).
    let long = "x".repeat(501);
    let cases: Vec<(Vec<u8>, Value)> = vec![
        (br#"{"question": "oi", "foo": 1}"#.to_vec(),
            json!([{"type": "extra_forbidden", "loc": ["body", "foo"], "msg": "Extra inputs are not permitted", "input": 1}])),
        (b"{}".to_vec(), json!([{"type": "missing", "loc": ["body", "question"], "msg": "Field required", "input": {}}])),
        (br#"{"question": ""}"#.to_vec(), json!([{"type": "string_too_short", "loc": ["body", "question"],
            "msg": "String should have at least 1 character", "input": "", "ctx": {"min_length": 1}}])),
        (json!({"question": long}).to_string().into_bytes(), json!([{"type": "string_too_long", "loc": ["body", "question"],
            "msg": "String should have at most 500 characters", "input": long, "ctx": {"max_length": 500}}])),
        (br#"{"question": 5}"#.to_vec(), json!([{"type": "string_type", "loc": ["body", "question"], "msg": "Input should be a valid string", "input": 5}])),
        (b"".to_vec(), json!([{"type": "missing", "loc": ["body"], "msg": "Field required", "input": null}])),
        (b"null".to_vec(), json!([{"type": "missing", "loc": ["body"], "msg": "Field required", "input": null}])),
        (b"[1]".to_vec(), json!([{"type": "model_attributes_type", "loc": ["body"],
            "msg": "Input should be a valid dictionary or object to extract fields from", "input": [1]}])),
        (br#"{"zz": 1, "question": "", "aa": 2}"#.to_vec(), json!([
            {"type": "string_too_short", "loc": ["body", "question"], "msg": "String should have at least 1 character", "input": "", "ctx": {"min_length": 1}},
            {"type": "extra_forbidden", "loc": ["body", "zz"], "msg": "Extra inputs are not permitted", "input": 1},
            {"type": "extra_forbidden", "loc": ["body", "aa"], "msg": "Extra inputs are not permitted", "input": 2}])),
    ];
    let cli = FakeCli::answering(&[]);
    let scene = scene(&cli).await;
    for (body, detail) in cases {
        assert_eq!(post(&scene, &body, true).await, (StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": detail})),
            "{}", String::from_utf8_lossy(&body));
    }
    // JSON quebrado: o `loc` leva a posição do erro, como o `json.loads`.
    for (body, pos) in [(&b"{x"[..], 1), (b"  ", 2), (b"{\"question\": \"a\",\n x}", 19)] {
        let (status, got) = post(&scene, body, true).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!((&got["detail"][0]["type"], &got["detail"][0]["loc"]), (&json!("json_invalid"), &json!(["body", pos])), "{got}");
    }
    // 500 caracteres (não bytes) passam.
    assert_eq!(ask(&scene, &"é".repeat(500)).await.0, StatusCode::OK);
    assert!(cli.calls().is_empty());
}

#[tokio::test]
async fn ask_route_is_answered_by_rust_only_for_the_owner() {
    // ASK-01: sem o token do dono segue ao Python, que recusa como hoje.
    let cli = FakeCli::answering(&[]);
    let scene = scene(&cli).await;
    assert_eq!(ask(&scene, "zzznadaqui").await.0, StatusCode::OK);
    assert_eq!(scene.python.hits_to("/api/ask-history"), 0);
    let _ = post(&scene, br#"{"question": "clippy"}"#, false).await;
    assert_eq!(scene.python.hits_to("/api/ask-history"), 1);
    assert!(cli.calls().is_empty());
    assert!(hangar_server::migration_status::rust_route(&axum::http::Method::POST, "/api/ask-history"));
}

#[tokio::test]
async fn ask_route_answer_is_not_cut_nor_refused() {
    // ASK-33: a resposta do Perguntar não passa pelo corte de 2000 nem pela recusa do refino (pergunta no fim).
    let long = format!("{}?", "sessão sessao-viva ".repeat(150));
    let cli = Arc::new(FakeCli { answers: Mutex::new(vec![Output { code: 0, stdout: format!("\n{long}\n"), stderr: String::new() }]), ..FakeCli::default() });
    let scene = scene(&cli).await;
    let (status, body) = ask(&scene, "onde falei sobre o clippy?").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(long.chars().count() > 2000);
    assert_eq!(body["answer"], long.trim_end());
}
