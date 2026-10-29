//! D29: campos inválidos profundos conservam o erro tipado sem derrubar o processo.
use axum::{
    body::to_bytes,
    http::{HeaderMap, HeaderValue},
};
use hangar_server::engines::body::{self, Schema};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/engines/deep-bodies.json"
    ))
    .unwrap()
}

#[test]
fn engine_deep_bodies_keep_complete_validation_errors() {
    let cases = fixture();
    let mut failures = Vec::new();
    for (index, case) in cases["cases"].as_array().unwrap().iter().enumerate() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "engine_deep_body::engine_deep_body_child",
                "--ignored",
            ])
            .env("HANGAR_ENGINE_DEEP_CASE", index.to_string())
            .output()
            .unwrap();
        if !output.status.success() {
            failures.push(format!(
                "{} {} {}: {}\n{}",
                case["schema"],
                case["kind"],
                case["depth"],
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
#[ignore = "Executado pelo teste pai em processo isolado"]
async fn engine_deep_body_child() {
    let index: usize = std::env::var("HANGAR_ENGINE_DEEP_CASE")
        .unwrap()
        .parse()
        .unwrap();
    let cases = fixture();
    let case = &cases["cases"][index];
    let hex = case["hex"].as_str().unwrap();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect();
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let schema = if case["schema"] == "EngineModelBody" {
        Schema::Model
    } else {
        Schema::Alias
    };
    let (status, actual) = match body::typed(schema, &bytes, &headers) {
        Ok(value) => (200, value.to_string()),
        Err(response) => {
            let status = response.status().as_u16();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (status, String::from_utf8(bytes.to_vec()).unwrap())
        }
    };
    assert_eq!(status, case["semantic_status"].as_u64().unwrap() as u16);
    assert_eq!(actual, case["semantic_body"].as_str().unwrap());
}
