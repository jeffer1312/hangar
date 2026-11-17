//! O que o Python fazia por fora das rotas e passa a ser do Rust em toda porta do dono: preflight,
//! teto do corpo, 404 de caminho sem handler, linha `api.servidor` e o status da migração (R4).
//! Os esperados vêm das capturas do Python em `contract/roteador/` (commit 1504c6c53).
use crate::fake::*;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ORIGIN: &str = "https://app.exemplo";
const EXPOSE: &str = "ETag, x-hangar-css-width, x-hangar-css-height";

/// I-12 (r01, r02, r03): o preflight de qualquer caminho sai antes do login, sem chegar ao Python.
#[tokio::test]
async fn preflight_of_any_path_is_answered_before_login() {
    let (python, _) = spawn_fake().await;
    let server = spawn_server_for(&python, config("127.0.0.1")).await;
    for path in ["/api/nada/aqui", "/api/whoami", "/internal/ping"] {
        let r = client().request(reqwest::Method::OPTIONS, format!("http://{server}{path}"))
            .header("origin", ORIGIN).header("access-control-request-method", "POST")
            .header("access-control-request-headers", "authorization,content-type,x-hangar-req").send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        let h = r.headers().clone();
        assert_eq!(h["access-control-allow-origin"], "*", "{path}");
        assert_eq!(h["access-control-allow-methods"], "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT", "{path}");
        assert_eq!(h["access-control-max-age"], "600", "{path}");
        assert_eq!(h["access-control-allow-headers"], "authorization,content-type,x-hangar-req", "{path}");
        assert_eq!(h["content-type"], "text/plain; charset=utf-8", "{path}");
        assert_eq!(r.text().await.unwrap(), "OK", "{path}");
    }
}

/// I-13 (r13): `Content-Length` acima de 100 MiB é 413 antes do login, com CORS, sem ler o corpo.
#[tokio::test]
async fn declared_body_over_100_mib_is_413_before_login() {
    let (python, _) = spawn_fake().await;
    let server = spawn_server_for(&python, config("127.0.0.1")).await;
    let mut s = tokio::net::TcpStream::connect(server).await.unwrap();
    s.write_all(format!("POST /api/diag HTTP/1.1\r\nHost: h\r\nOrigin: {ORIGIN}\r\nContent-Length: 104857601\r\n\r\nx").as_bytes())
        .await.unwrap();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 1024];
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !String::from_utf8_lossy(&seen).contains("request body too large") {
            let n = s.read(&mut chunk).await.unwrap();
            assert!(n > 0, "a conexão fechou antes da resposta: {}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&chunk[..n]);
        }
    }).await.expect("a recusa não chegou");
    let text = String::from_utf8_lossy(&seen).to_lowercase();
    assert!(text.starts_with("http/1.1 413"), "{text}");
    assert!(text.contains("access-control-allow-origin: *"), "{text}");
    assert!(text.contains(&format!("access-control-expose-headers: {}", EXPOSE.to_lowercase())), "{text}");
    assert!(text.contains("content-type: text/plain; charset=utf-8"), "{text}");
}

/// I-18 (r09, r11) e decisão 19: caminho sem handler é 404 do Rust para o dono, com CORS; quem não é
/// o dono leva a recusa de sempre (r08: 401 `erro_nao_autorizado`), e nada vai ao Python.
#[tokio::test]
async fn path_without_handler_is_404_for_the_owner_and_refused_for_the_stranger() {
    let (python, _) = spawn_fake().await;
    let server = spawn_server_for(&python, config("127.0.0.1")).await;
    let r = client().get(format!("http://{server}/api/nada/12345/xyz")).bearer_auth(OWNER).header("origin", ORIGIN)
        .send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(r.headers()["access-control-expose-headers"], EXPOSE);
    let r = client().post(format!("http://{server}/api/sessions/minha-sessao/coisa")).bearer_auth(OWNER)
        .json(&serde_json::json!({"a": 1})).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = client().get(format!("http://{server}/api/nada/12345/xyz")).header("origin", ORIGIN).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["detail"]["code"], "erro_nao_autorizado");
}

/// Decisão 19: caminho sem handler sob uma sessão é 404 antes da porta da sessão; sessão sem entrada
/// no registro não vira o 409 de "ocupada".
#[tokio::test]
async fn path_without_handler_is_404_before_the_session_gate() {
    let (python, _) = spawn_fake().await;
    let state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    let registry = std::sync::Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new("instance-test".into()));
    assert!(state.state.runtime.set(registry).is_ok());
    let server = spawn_state_for(&python, state).await;
    let r = client().post(format!("http://{server}/api/sessions/sem-entrada/coisa")).bearer_auth(OWNER)
        .json(&serde_json::json!({"a": 1})).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

/// I-14 e decisão 18: o pedido do Rust que falha no servidor ganha a linha `api.servidor` com a rota
/// mascarada e a causa do corpo; o 404 de caminho desconhecido, que o Python também registrava, não.
#[tokio::test]
async fn server_failure_writes_the_access_line_with_masked_route_and_cause() {
    let (python, _) = spawn_fake().await;
    let server = spawn_server_with_diary(config("127.0.0.1"), &python).await;
    let unknown = client().get(format!("http://{server}/api/nada/12345/xyz")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
    // Sem o registro de sessões, a escrita é 503 `runtime_unavailable` (session_write_routes).
    let failed = client().post(format!("http://{server}/api/sessions/minha-sessao/input")).bearer_auth(OWNER)
        .header("content-type", "application/json").body(r#"{"text":"oi"}"#).send().await.unwrap();
    assert_eq!(failed.status(), 503);
    let is_line = |row: &Value| row["evento"] == "api.servidor" && row["codigo"] == "503";
    let rows = wait_diary(python.diary_dir(), is_line).await;
    let line = rows.iter().find(|row| is_line(row)).unwrap();
    assert_eq!(line["nivel"], "erro");
    assert_eq!(line["detalhe"], "POST /api/sessions/*/input");
    assert_eq!(line["causa"], "runtime_unavailable");
    assert_eq!(line["etapa"], "cabecalhos");
    assert!(!line.to_string().contains("minha-sessao"), "nome de sessão no diário: {line}");
    assert!(!rows.iter().any(|row| row["evento"] == "api.servidor" && row["codigo"] == "404"), "{rows:?}");
}

/// A tela da migração saiu: o dono pedindo o status leva o 404 de rota inexistente, como qualquer
/// caminho que o servidor não tem.
#[tokio::test]
async fn migration_status_is_gone() {
    let (python, _) = spawn_fake().await;
    let server = spawn_server_for(&python, config("127.0.0.1")).await;
    let r = client().get(format!("http://{server}/api/migration/status")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

/// D-22: o pedido a `/api/diag*` que falha no servidor não ganha a linha `api.servidor` (o diário se
/// alimentaria do próprio envio; o Python cortava pelo `startswith("/api/diag")`). A falha de outra rota,
/// feita depois, ganha a dela: a ausência não é atraso de gravação.
#[tokio::test(flavor = "multi_thread")]
async fn diag_failure_writes_no_access_line() {
    let (python, _) = spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.diag = python.bind_diary(state.diag.clone());
    state.side.locate = python.locate();
    let home = tempfile::tempdir().unwrap();
    state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map([
        ("HOME".into(), home.path().to_string_lossy().into_owned()),
        ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
    ].into()));
    // Porta fechada: o worker dos relatórios fora do ar é 502 `diag_report_unreachable`.
    state.diag_report_base = "http://127.0.0.1:9".into();
    let server = spawn_state(state).await;
    let report = client().post(format!("http://{server}/api/diag/report")).bearer_auth(OWNER)
        .json(&serde_json::json!({"text": "a fila travou", "client": "native 0.1.0 linux-x86_64",
            "failures": [{"ts": 1793130757.2, "route": "GET /api/sessions", "status": 0, "code": null}]})).send().await.unwrap();
    assert_eq!(report.status(), 502);
    let failed = client().post(format!("http://{server}/api/sessions/minha-sessao/input")).bearer_auth(OWNER)
        .header("content-type", "application/json").body(r#"{"text":"oi"}"#).send().await.unwrap();
    assert_eq!(failed.status(), 503);
    let rows = wait_diary(python.diary_dir(), |row| row["evento"] == "api.servidor" && row["codigo"] == "503").await;
    let diag: Vec<_> = rows.iter().filter(|row| row["evento"] == "api.servidor"
        && row["detalhe"].as_str().is_some_and(|detail| detail.contains("/api/diag"))).collect();
    assert!(diag.is_empty(), "{diag:?}");
}
