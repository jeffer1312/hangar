//! R8: `GET /api/archive/{project}/{session_id}/history` e `.../transcript-image/{uuid}/{idx}` no Rust,
//! comparados com a captura do Python (`tests/fixtures/search/contract/archive/`) sobre o HOME real de
//! `tests/fixtures/search/home/`.
use axum::{Router, extract::Request, http::StatusCode, response::IntoResponse};
use hangar_server::{auth::TrustedHosts, config::Config, routes::{AppState, router}};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, Mutex}};

const OWNER: &str = "dono-de-teste";
const SECRET: &str = "interno-de-teste";
const NORMAL: (&str, &str) = ("-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1", "0cc60e6c-eafc-4915-9936-f0c7b81191d9");
const LONG: (&str, &str) = ("-home-jefferson", "42a7c450-7f7e-4fd7-84ed-ee699d11098d");
const IMAGE_UUID: &str = "e73695a8-1a12-41dd-a965-b6f7bb44f09d";
const CODEX_A: &str = "01a05511-e980-71b3-93a9-befda5704b20";
const CODEX_B: &str = "01a0d000-f6ac-74e3-b19b-98d42af1a712";

fn fixtures() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search") }

fn capture(name: &str) -> Value {
    let raw = std::fs::read_to_string(fixtures().join("contract/archive").join(name)).unwrap();
    serde_json::from_str::<Value>(&raw).unwrap()["data"].clone()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() { copy_tree(&entry.path(), &target) } else { std::fs::copy(entry.path(), &target).unwrap(); }
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    home: PathBuf,
    address: SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) { for task in &self.tasks { task.abort(); } }
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        copy_tree(&fixtures().join("home"), &home);
        let index = home.join(".kimi-code/session_index.jsonl");
        std::fs::write(&index, std::fs::read_to_string(&index).unwrap().replace("{HOME}", &home.to_string_lossy())).unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let seen = hits.clone();
        // O Python de mentira: recusa sem o token do dono e marca o que recebeu.
        let python = move |request: Request| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(request.uri().to_string());
                if request.headers().get("authorization").is_none_or(|t| t != &format!("Bearer {OWNER}")) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                (StatusCode::OK, [("content-type", "application/json")], json!({"from_python": true}).to_string()).into_response()
            }
        };
        let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = up.local_addr().unwrap();
        let up_task = tokio::spawn(async move { axum::serve(up, Router::new().fallback(python)).await.unwrap() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config { listen: address, upstream, internal_secret: SECRET.into(), auth_token: OWNER.into(),
            log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
        hangar_server::accounts::quotas::disable_open_network_read();
        let env = [("HOME", home.to_string_lossy().into_owned()), ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", home.join(".claude-a").to_string_lossy().into_owned())];
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
            env.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()));
        let app = router(Arc::new(state));
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        Self { _dir: dir, home, address, hits, client: reqwest::Client::new(), tasks: vec![up_task, server] }
    }

    fn url(&self, path: &str) -> String { format!("http://{}{path}", self.address) }

    /// (status, corpo com o HOME trocado por `{HOME}`, como a captura).
    async fn get(&self, path: &str) -> (u16, Value) {
        let response = self.client.get(self.url(path)).bearer_auth(OWNER).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap().replace(&*self.home.to_string_lossy(), "{HOME}");
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    fn forwarded(&self, needle: &str) -> bool { self.hits.lock().unwrap().iter().any(|hit| hit.contains(needle)) }

    fn dir(&self, name: &str) -> String { self.home.join(name).to_string_lossy().into_owned() }
}

fn history(project: &str, sid: &str, query: &str) -> String { format!("/api/archive/{project}/{sid}/history{query}") }

/// A captura sem o `msg` das contas Codex: o roteiro de captura escreve "conta", a rota escreve a frase.
fn without_account_msg(mut v: Value) -> Value {
    if v["detail"]["code"].as_str().is_some_and(|c| c.starts_with("codex_account")) { v["detail"]["msg"] = Value::Null; }
    v
}

async fn assert_case(h: &Harness, case: &str, path: &str) {
    let want = &capture("history.json")[case];
    let (status, body) = h.get(path).await;
    assert_eq!(status, want["status"].as_u64().unwrap() as u16, "{case}: {body}");
    assert_eq!(without_account_msg(body), without_account_msg(want["body"].clone()), "{case}");
    assert!(h.hits.lock().unwrap().is_empty(), "{case} foi repassado ao Python");
}

/// ARQ-01: sem token o pedido vai ao Python, que recusa; com `?token=` o Rust atende as duas rotas.
#[tokio::test]
async fn arq01_owner_only_and_query_token_is_accepted() {
    let h = Harness::new().await;
    let path = history(NORMAL.0, NORMAL.1, "");
    let response = h.client.get(h.url(&path)).send().await.unwrap();
    assert_eq!(response.status(), 401);
    assert!(h.forwarded(&path));
    h.hits.lock().unwrap().clear();
    let response = h.client.get(h.url(&format!("{path}?token={OWNER}"))).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let image = format!("/api/archive/{}/{}/transcript-image/{IMAGE_UUID}/0?token={OWNER}", LONG.0, LONG.1);
    let response = h.client.get(h.url(&image)).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(h.hits.lock().unwrap().is_empty(), "o dono foi repassado ao Python");
}

/// ARQ-38: conta e agente validados antes; `tail` limitado a 200, `<= 0` é o histórico inteiro, texto é 422.
#[tokio::test]
async fn arq38_history_validation_and_tail_bounds() {
    let h = Harness::new().await;
    for (case, query) in [("normal|tail0", "?tail=0".to_owned()), ("normal|tail-3", "?tail=-3".to_owned()), ("normal|tail500", "?tail=500".to_owned()),
        ("normal|config_a", format!("?config_dir={}", h.dir(".claude-a"))), ("normal|bad_config", "?config_dir=/nao/conta".to_owned()),
        ("normal|bad_provider", "?provider=gemini".to_owned())] {
        assert_case(&h, case, &history(NORMAL.0, NORMAL.1, &query)).await;
    }
    let (status, body) = h.get(&history(NORMAL.0, NORMAL.1, "?tail=abc")).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["loc"], json!(["query", "tail"]));
}

/// ARQ-39: `tail` traz só pedido e resposta com texto, em ordem; a janela cresce além dos 256 KB.
#[tokio::test]
async fn arq39_tail_reads_from_the_end_and_grows_the_window() {
    let h = Harness::new().await;
    assert_case(&h, "normal|tail1", &history(NORMAL.0, NORMAL.1, "?tail=1")).await;
    assert_case(&h, "normal|tail5", &history(NORMAL.0, NORMAL.1, "?tail=5")).await;
    // Uma resposta seguida de 1 MB de linhas que não são mensagem: a primeira janela não a acha.
    let sid = "11111111-2222-4333-8444-555555555555";
    let dir = h.home.join(".claude-a/projects/-tmp-janela");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = json!({"type":"user","uuid":"u1","timestamp":"2026-10-01T12:00:00Z","message":{"role":"user","content":"a pergunta"}}).to_string() + "\n";
    let filler = json!({"type":"system","timestamp":"2026-10-01T12:00:01Z","content":"x".repeat(1000)}).to_string() + "\n";
    body.push_str(&filler.repeat(1100));
    std::fs::write(dir.join(format!("{sid}.jsonl")), body).unwrap();
    for path in [history("-tmp-janela", sid, "?tail=1")] {
        let (status, body) = h.get(&path).await;
        assert_eq!(status, 200);
        assert_eq!(body.as_array().unwrap().iter().map(|e| e["id"].clone()).collect::<Vec<_>>(), vec![json!("u1")]);
    }
}

/// ARQ-41: o histórico inteiro do Claude é o `merged_history` (reescrita, `held:`, linha longa, reserva).
#[tokio::test]
async fn arq41_full_claude_history_matches_the_python() {
    let h = Harness::new().await;
    assert_case(&h, "normal|tail0", &history(NORMAL.0, NORMAL.1, "")).await;
    assert_case(&h, "rewrite", &history("-home-jefferson-Projetos-hangar", "75fea57d-2237-43fe-a669-bb1ed97c6eae", "")).await;
    assert_case(&h, "held", &history("-tmp-cx-probe-work", "f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53", "")).await;
    assert_case(&h, "long", &history(LONG.0, LONG.1, "")).await;
    assert_case(&h, "reserve", &history("-home-jefferson", "f2afaa93-d2aa-45db-9d68-b430c22deab2", "")).await;
}

/// ARQ-42: Codex, Pi, omp e Kimi linha a linha pelo parser de cada um, inteiro e com `tail`.
#[tokio::test]
async fn arq42_other_agents_use_their_own_parser() {
    let h = Harness::new().await;
    for (provider, sid) in [("codex", CODEX_A), ("codex", CODEX_B), ("pi", "d337d6b8-0b3c-4bbe-8034-939d7c48f246"),
        ("omp", "01a06932-79ae-77e9-b95a-286b6e2e9288"), ("kimi", "session_ab697d05-66da-4b2a-8e80-3d769951e752")] {
        assert_case(&h, &format!("{provider}|{sid}"), &history("qualquer", sid, &format!("?provider={provider}"))).await;
        assert_case(&h, &format!("{provider}|{sid}|tail2"), &history("qualquer", sid, &format!("?provider={provider}&tail=2"))).await;
    }
    assert_case(&h, "codex|b|acc_b", &history("x", CODEX_B, "?provider=codex&codex_account=b")).await;
}

/// ARQ-43: a cadeia de erros do histórico: caminho 400, ausente 404, conta Codex com o status dela.
#[tokio::test]
async fn arq43_history_errors() {
    let h = Harness::new().await;
    assert_case(&h, "bad_project", &history("..x", NORMAL.1, "")).await;
    assert_case(&h, "bad_sid", &history(NORMAL.0, "x", "")).await;
    assert_case(&h, "missing", &history(NORMAL.0, "00000000-0000-4000-8000-000000000000", "")).await;
    assert_case(&h, "normal|config_b", &history(NORMAL.0, NORMAL.1, &format!("?config_dir={}", h.dir(".claude-b")))).await;
    assert_case(&h, "codex|b|acc_default", &history("x", CODEX_B, "?provider=codex&codex_account=default")).await;
    assert_case(&h, "codex|b|acc_zz", &history("x", CODEX_B, "?provider=codex&codex_account=zz")).await;
    assert_case(&h, "codex|bad_sid", &history("x", "nao-uuid", "?provider=codex")).await;
    assert_case(&h, "kimi|bad_sid", &history("x", "ab697d05-66da-4b2a-8e80-3d769951e752", "?provider=kimi")).await;
    assert_case(&h, "pi|missing", &history("x", "00000000-0000-4000-8000-000000000000", "?provider=pi")).await;
}

/// ARQ-53 (F10): `tail` passa pelo filtro de reescrita: as 3 últimas são as do histórico inteiro.
#[tokio::test]
async fn arq53_tail_applies_the_rewrite_filter() {
    let h = Harness::new().await;
    let full = capture("history.json")["rewrite"]["body"].as_array().unwrap().clone();
    let msgs: Vec<Value> = full.into_iter().filter(|e| matches!(e["kind"].as_str(), Some("user_msg" | "assistant_msg")) && e["text"].as_str().is_some_and(|t| !t.is_empty())).collect();
    let (status, body) = h.get(&history("-home-jefferson-Projetos-hangar", "75fea57d-2237-43fe-a669-bb1ed97c6eae", "?tail=3")).await;
    assert_eq!(status, 200);
    assert_eq!(body, Value::Array(msgs[msgs.len() - 3..].to_vec()));
    assert!(!body.to_string().contains("1c3fcd4c"), "o /compact regravado voltou");
}

/// ARQ-54 (F11): `tail` não repete o `held:` reenviado.
#[tokio::test]
async fn arq54_tail_drops_repeated_held_entries() {
    let h = Harness::new().await;
    let (status, body) = h.get(&history("-tmp-cx-probe-work", "f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53", "?tail=5")).await;
    assert_eq!(status, 200);
    assert_eq!(body, capture("history.json")["held"]["body"]);
}

fn image(project: &str, sid: &str, uuid: &str, idx: &str, query: &str) -> String {
    format!("/api/archive/{project}/{sid}/transcript-image/{uuid}/{idx}{query}")
}

async fn assert_image(h: &Harness, case: &str, path: &str) {
    let want = &capture("image.json")[case];
    let response = h.client.get(h.url(path)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status().as_u16(), want["status"].as_u64().unwrap() as u16, "{case}");
    if want["status"] == 200 {
        let headers = response.headers().clone();
        let raw = response.bytes().await.unwrap();
        let sha: String = ring::digest::digest(&ring::digest::SHA256, &raw).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let got = json!({"sha256": sha, "len": raw.len(), "media_type": headers["content-type"].to_str().unwrap(),
            "cache_control": headers["cache-control"].to_str().unwrap()});
        assert_eq!(got, want["body"], "{case}");
    } else {
        let body: Value = response.json().await.unwrap();
        assert_eq!(body, want["body"], "{case}");
    }
    assert!(h.hits.lock().unwrap().is_empty(), "{case} foi repassado ao Python");
}

/// ARQ-46: bytes, tipo e cache imutável; 404 de conversa e de imagem; 422 para `idx` que não é inteiro.
#[tokio::test]
async fn arq46_image_bytes_and_errors() {
    let h = Harness::new().await;
    let a_dir = h.dir(".claude-b");
    for (case, path) in [
        ("ok_idx0", image(LONG.0, LONG.1, IMAGE_UUID, "0", "")),
        ("idx1", image(LONG.0, LONG.1, IMAGE_UUID, "1", "")),
        ("idx_neg", image(LONG.0, LONG.1, IMAGE_UUID, "-1", "")),
        ("uuid_missing", image(LONG.0, LONG.1, "nao-existe", "0", "")),
        ("uuid_no_image", image(NORMAL.0, NORMAL.1, "dd7ece7e-9d51-4d61-9243-44cc7fbc8840", "0", "")),
        ("bad_project", image("..x", LONG.1, IMAGE_UUID, "0", "")),
        ("missing", image(LONG.0, "00000000-0000-4000-8000-000000000000", IMAGE_UUID, "0", "")),
        ("config_b", image(LONG.0, LONG.1, IMAGE_UUID, "0", &format!("?config_dir={a_dir}"))),
        ("bad_config", image(LONG.0, LONG.1, IMAGE_UUID, "0", "?config_dir=/nao/conta")),
        ("codex_no_image", image("x", CODEX_A, "nao", "0", "?provider=codex")),
    ] {
        assert_image(&h, case, &path).await;
    }
    let (status, body) = h.get(&image(LONG.0, LONG.1, IMAGE_UUID, "a", "")).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["loc"], json!(["path", "idx"]));
}

/// ARQ-44: imagem `transfer:` pelo registro da transferência: id inválido 404, registro ausente 404 de
/// imagem; com o registro, a leitura da origem congelada segue ao Python.
#[tokio::test]
async fn arq44_transfer_image_by_record() {
    let h = Harness::new().await;
    assert_image(&h, "transfer_prefix", &image(LONG.0, LONG.1, "transfer:abc:claude:x", "0", "")).await;
    let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let (status, body) = h.get(&image("x", CODEX_A, &format!("transfer:{id}:claude:u1"), "0", "?provider=codex")).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_imagem_nao_encontrada")));
    let (status, _) = h.get(&image("x", CODEX_A, &format!("transfer:{id}:codex:u1"), "0", "?provider=codex")).await;
    assert_eq!(status, 404);
    assert!(h.hits.lock().unwrap().is_empty());
    let records = h.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{id}.json")), "{}").unwrap();
    let (status, body) = h.get(&image("x", CODEX_A, &format!("transfer:{id}:claude:u1"), "0", "?provider=codex")).await;
    assert_eq!((status, body), (200, json!({"from_python": true})));
}

/// ARQ-40, ARQ-45 e PRV-21: rollout Codex que veio de uma transferência (índice da thread gravado)
/// tem o histórico composto e a imagem lidos pelo Python; sem índice, o Rust lê o rollout.
#[tokio::test]
async fn arq40_arq45_prv21_transferred_rollout_goes_to_the_composition() {
    let h = Harness::new().await;
    let codex_home = h.home.join(".codex").canonicalize().unwrap().to_string_lossy().into_owned();
    let key = format!("[{}, {}]", serde_json::to_string(&codex_home).unwrap(), serde_json::to_string(CODEX_A).unwrap());
    let digest: String = ring::digest::digest(&ring::digest::SHA256, key.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let indexes = h.home.join(".hangar/conversation-transfers/indexes");
    std::fs::create_dir_all(&indexes).unwrap();
    std::fs::write(indexes.join(format!("thread-{digest}.json")), r#"["aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"]"#).unwrap();
    for path in [history("x", CODEX_A, "?provider=codex"), history("x", CODEX_A, "?provider=codex&tail=2"),
        image("x", CODEX_A, "nao", "0", "?provider=codex")] {
        let (status, body) = h.get(&path).await;
        assert_eq!((status, body), (200, json!({"from_python": true})), "{path}");
    }
    // A outra conversa Codex, sem transferência, continua no Rust.
    h.hits.lock().unwrap().clear();
    assert_case(&h, &format!("codex|{CODEX_B}"), &history("x", CODEX_B, "?provider=codex")).await;
}
