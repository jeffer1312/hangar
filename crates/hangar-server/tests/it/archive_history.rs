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
    /// O diário do índice da busca, que o Arquivo também usa: (código, detalhe).
    reports: Arc<Mutex<Vec<(&'static str, String)>>>,
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
        // Escapado como texto JSON: o `\` do caminho do Windows quebrava a linha do índice do Kimi.
        let escaped = serde_json::to_string(&*home.to_string_lossy()).unwrap();
        std::fs::write(&index, std::fs::read_to_string(&index).unwrap().replace("{HOME}", escaped.trim_matches('"'))).unwrap();
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
        let reports = Arc::new(Mutex::new(Vec::new()));
        let diary = reports.clone();
        state.search = hangar_server::search::Index::new(&dir.path().join("idx"), Arc::new(|| Ok(Vec::new())),
            Arc::new(move |code, detail| diary.lock().unwrap().push((code, detail))), Default::default());
        let app = router(Arc::new(state));
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        Self { _dir: dir, home, address, hits, client: reqwest::Client::new(), reports, tasks: vec![up_task, server] }
    }

    /// Os detalhes registrados sob `code`.
    fn reported(&self, code: &str) -> Vec<String> {
        self.reports.lock().unwrap().iter().filter(|(c, _)| *c == code).map(|(_, d)| d.clone()).collect()
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
    // Outra grafia da mesma pasta (barra final; no Windows `\\?\` e `/`) é a mesma conta do catálogo.
    assert_case(&h, "normal|config_a", &history(NORMAL.0, NORMAL.1, &format!("?config_dir={}/", h.dir(".claude-a")))).await;
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

// A transferência Claude→Codex não tem caso real: o caso é o `joined` de
// backend/tests/test_conversation_history.py, com o registro gravado como `save_transfer` grava.
const THREAD: &str = "019f99de-9572-7221-89c0-80c62f883d44";
const TRANSFER: &str = "a563a017-af37-42e8-a8e5-2bd837584f8e";
const FROZEN_IMAGE: &[u8] = b"frozen-image";

fn sha(bytes: &[u8]) -> String { ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect() }

fn line(row: &Value) -> String { format!("{row}\n") }

fn codex_message(text: &str, item_id: &str, day: u32) -> Value {
    json!({"timestamp": format!("2026-02-{day:02}T00:00:00Z"), "type": "response_item",
        "payload": {"type": "message", "id": item_id, "role": "user", "content": [{"type": "input_text", "text": text}]}})
}

struct Joined { collision: String, rollout: PathBuf, prefix: String, record: PathBuf }

/// O `joined`: origem com ramo compactado, fork e resumo; rollout com o "ok" importado e o "novo".
fn joined(h: &Harness) -> Joined {
    let base = h.home.join("transfer");
    std::fs::create_dir_all(&base).unwrap();
    let new = codex_message("novo", "new", 2);
    let collision = hangar_server::transcript::LineParser::new(hangar_server::transcript::Provider::Codex)
        .feed(line(&new).as_bytes(), 0)[0].id.clone();
    let image = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, FROZEN_IMAGE);
    let rows = [
        json!({"type": "user", "uuid": collision, "parentUuid": null, "timestamp": "2026-01-01T00:00:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "ok"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/webp", "data": image}}]}}),
        json!({"type": "assistant", "uuid": "tool", "parentUuid": collision, "timestamp": "2026-01-01T00:01:00Z",
            "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "call", "name": "Read", "input": {"file_path": "/original/report.md"}}]}}),
        json!({"type": "user", "uuid": "result", "parentUuid": "tool", "timestamp": "2026-01-01T00:02:00Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call", "content": "resultado original"}]}}),
        json!({"type": "system", "subtype": "compact_boundary", "uuid": "compact", "parentUuid": null, "logicalParentUuid": "result"}),
        json!({"type": "user", "uuid": "summary", "parentUuid": "compact", "isCompactSummary": true,
            "message": {"role": "user", "content": "RESUMO NÃO DEVE ENTRAR"}}),
        json!({"type": "assistant", "uuid": "after", "parentUuid": "summary", "timestamp": "2026-01-01T00:03:00Z",
            "message": {"role": "assistant", "content": "antigo"}}),
        json!({"type": "assistant", "uuid": "fork", "parentUuid": collision, "isSidechain": true,
            "message": {"role": "assistant", "content": "FORK NÃO DEVE ENTRAR"}}),
        json!({"type": "last-prompt", "leafUuid": "after"}),
    ];
    let snapshot = base.join("snapshot.jsonl");
    let snapshot_bytes: String = rows.iter().map(line).collect();
    std::fs::write(&snapshot, &snapshot_bytes).unwrap();
    let selected = [collision.as_str(), "tool", "result", "compact", "after"];
    let prefix = line(&json!({"type": "session_meta", "payload": {"id": THREAD, "cwd": base}})) + &line(&codex_message("ok", "imported", 1));
    let codex_home = h.home.join(".codex");
    let rollout = codex_home.join("sessions/2026/02/02").join(format!("rollout-fixture-{THREAD}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, prefix.clone() + &line(&new)).unwrap();
    let record = json!({"id": TRANSFER, "name": "s", "source_life": "k:original", "phase": "complete",
        "source": {"path": snapshot, "provider": "claude", "digest": sha(snapshot_bytes.as_bytes()), "selected_uuids": selected},
        "origin_meta": {"provider": "claude", "jsonl": base.join("original.jsonl"), "cwd": base, "key": "original"},
        "destination_meta": {"provider": "codex", "thread_id": THREAD, "codex_home": codex_home, "lifecycle_id": "k:original", "transfer_id": TRANSFER},
        "boundary": {"thread_id": THREAD, "rollout_path": rollout, "min_offset": prefix.len(), "imported_item_ids": ["imported"], "prefix_digest": sha(prefix.as_bytes())},
        "error_code": null});
    let store = h.home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(store.join("indexes")).unwrap();
    let path = store.join(format!("{TRANSFER}.json"));
    std::fs::write(&path, record.to_string()).unwrap();
    // `_thread_key`: json.dumps([home resolvido, thread], ensure_ascii=False).
    let key = format!("[{}, {}]", serde_json::to_string(&codex_home.canonicalize().unwrap()).unwrap(), serde_json::to_string(THREAD).unwrap());
    std::fs::write(store.join("indexes").join(format!("thread-{}.json", sha(key.as_bytes()))), json!([TRANSFER]).to_string()).unwrap();
    Joined { collision, rollout, prefix, record: path }
}

fn set_phase(j: &Joined, phase: &str) {
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&j.record).unwrap()).unwrap();
    record["phase"] = json!(phase);
    std::fs::write(&j.record, record.to_string()).unwrap();
}

fn rewrite_prefix(path: &Path) {
    let raw = std::fs::read_to_string(path).unwrap();
    std::fs::write(path, raw.replacen(r#""text":"ok""#, r#""text":"no""#, 1)).unwrap();
}

/// ARQ-40 (`test_join_excludes_import_fork_summary_and_preserves_tool_pairs` e
/// `test_archive_uses_account_thread_after_name_sidecar_closed`): o histórico do rollout transferido é a
/// origem congelada (sem fork, sem resumo do compact) mais só os turnos novos, com ids da transferência.
#[tokio::test]
async fn arq40_transferred_rollout_history_is_the_composition() {
    let h = Harness::new().await;
    let j = joined(&h);
    let (status, body) = h.get(&history("x", THREAD, "?provider=codex")).await;
    assert_eq!(status, 200, "{body}");
    let events = body.as_array().unwrap();
    let texts: Vec<Value> = events.iter().map(|e| if e["text"].is_null() { e["result"].clone() } else { e["text"].clone() }).collect();
    assert_eq!(texts, vec![json!("ok"), Value::Null, json!("resultado original"), json!("antigo"), json!("novo")]);
    assert_eq!(events[0]["id"], json!(format!("transfer:{TRANSFER}:claude:{}", j.collision)));
    assert_eq!(events[4]["id"], json!(format!("transfer:{TRANSFER}:codex:{}", j.collision)));
    assert_eq!(events[1]["tool_use_id"], events[2]["tool_use_id"]);
    assert_eq!(events[1]["tool_use_id"], json!(format!("transfer:{TRANSFER}:claude:call")));
    let ids: std::collections::HashSet<_> = events.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), events.len());
    // `tail` sobre o composto: só as mensagens com texto.
    let (status, tail) = h.get(&history("x", THREAD, "?provider=codex&tail=2")).await;
    assert_eq!(status, 200);
    assert_eq!(tail, Value::Array(vec![events[3].clone(), events[4].clone()]));
    assert!(h.hits.lock().unwrap().is_empty(), "foi repassado ao Python");
}

/// PRV-21 (`test_archive_uses_account_thread...` e `test_invalid_boundary_is_never_reinterpreted...`):
/// registro não concluído, prefixo regravado, rollout truncado ou origem sumida é 409, nunca histórico cru.
#[tokio::test]
async fn prv21_invalid_transfer_history_is_409() {
    let invalid = json!({"detail": {"code": "session_transfer_history_invalid", "params": {}, "msg": "histórico da transferência indisponível"}});
    let h = Harness::new().await;
    let j = joined(&h);
    set_phase(&j, "imported");
    assert_eq!(h.get(&history("x", THREAD, "?provider=codex")).await, (409, invalid.clone()));
    set_phase(&j, "complete");
    rewrite_prefix(&j.rollout);
    assert_eq!(h.get(&history("x", THREAD, "?provider=codex")).await, (409, invalid.clone()));
    std::fs::write(&j.rollout, &j.prefix[..j.prefix.len() - 1]).unwrap();
    assert_eq!(h.get(&history("x", THREAD, "?provider=codex&tail=2")).await, (409, invalid.clone()));
    let h = Harness::new().await;
    joined(&h);
    std::fs::remove_file(h.home.join("transfer/snapshot.jsonl")).unwrap();
    assert_eq!(h.get(&history("x", THREAD, "?provider=codex")).await, (409, invalid));
    // A outra conversa Codex, sem transferência, segue lida do rollout.
    assert_case(&h, &format!("codex|{CODEX_B}"), &history("x", CODEX_B, "?provider=codex")).await;
}

/// ARQ-45 (`historical_image`): no rollout transferido a imagem vem da origem congelada pelo id da
/// transferência; outro id não procura no rollout.
#[tokio::test]
async fn arq45_transferred_rollout_image_comes_from_the_frozen_source() {
    let h = Harness::new().await;
    let j = joined(&h);
    let id = format!("transfer:{TRANSFER}:claude:{}", j.collision);
    let response = h.client.get(h.url(&image("x", THREAD, &id, "0", "?provider=codex"))).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/webp");
    assert_eq!(response.headers()["cache-control"], "max-age=31536000, immutable");
    assert_eq!(&response.bytes().await.unwrap()[..], FROZEN_IMAGE);
    for (uuid, idx) in [(id.as_str(), "1"), ("nao", "0")] {
        let (status, body) = h.get(&image("x", THREAD, uuid, idx, "?provider=codex")).await;
        assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_imagem_nao_encontrada")), "{uuid}");
    }
    assert!(h.hits.lock().unwrap().is_empty());
}

/// ARQ-44 (`archived_image` e os três `test_archive_image_legacy_url_*`): a imagem `transfer:` vai pelo
/// registro e pela thread exata, na conta registrada, segue o rollout movido para `archived_sessions` da
/// mesma conta e recusa prefixo alterado.
#[tokio::test]
async fn arq44_transfer_image_by_record() {
    let h = Harness::new().await;
    assert_image(&h, "transfer_prefix", &image(LONG.0, LONG.1, "transfer:abc:claude:x", "0", "")).await;
    let absent = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    for uuid in [format!("transfer:{absent}:claude:u1"), format!("transfer:{absent}:codex:u1")] {
        let (status, body) = h.get(&image("x", CODEX_A, &uuid, "0", "?provider=codex")).await;
        assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_imagem_nao_encontrada")), "{uuid}");
    }
    let j = joined(&h);
    let id = format!("transfer:{TRANSFER}:claude:{}", j.collision);
    let url = image("project", THREAD, &id, "0", "");
    let response = h.client.get(h.url(&url)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/webp");
    assert_eq!(&response.bytes().await.unwrap()[..], FROZEN_IMAGE);
    let (status, body) = h.get(&image("project", "different-thread", &id, "0", "")).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_imagem_nao_encontrada")));
    let (status, body) = h.get(&format!("{url}?codex_account=other")).await;
    assert_eq!((status, body["detail"]["code"].clone(), body["detail"]["params"].clone()),
        (409, json!("codex_account_archive_mismatch"), json!({"account_id": "other", "origin_account": "default"})));
    // O movimento nativo para `archived_sessions` da mesma conta.
    let moved = h.home.join(".codex/archived_sessions").join(j.rollout.file_name().unwrap());
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&j.rollout, &moved).unwrap();
    assert_eq!(h.client.get(h.url(&url)).bearer_auth(OWNER).send().await.unwrap().status(), 200);
    rewrite_prefix(&moved);
    let (status, body) = h.get(&url).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("session_transfer_history_invalid")));
    // Registro ilegível é o 404 de conversa, como o `ValueError` do `load_transfer`.
    std::fs::write(&j.record, "{}").unwrap();
    let (status, body) = h.get(&url).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("erro_nao_encontrado")));
    assert!(h.hits.lock().unwrap().is_empty());
}

/// O status de cada falha fica como era, mas a causa real vai ao diário, uma vez por arquivo.
#[cfg(unix)]
#[tokio::test]
async fn archive_history_failures_keep_the_status_and_write_the_cause_once() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new().await;
    // Transcript sem permissão: 503 com código.
    let transcript = h.home.join(".claude-a/projects").join(NORMAL.0).join(format!("{}.jsonl", NORMAL.1));
    std::fs::set_permissions(&transcript, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read(&transcript).is_ok(); // root lê mesmo assim
    let first = h.get(&history(NORMAL.0, NORMAL.1, "")).await;
    let second = h.get(&history(NORMAL.0, NORMAL.1, "")).await;
    std::fs::set_permissions(&transcript, std::fs::Permissions::from_mode(0o644)).unwrap();
    if !readable {
        assert_eq!((first.0, first.1["error_code"].clone()), (503, json!("archive_history_io")));
        assert_eq!(second.0, 503);
        let causes = h.reported("arquivo.transcript_ilegivel");
        assert_eq!(causes.len(), 1, "{causes:?}");
        assert!(causes[0].contains(NORMAL.1) && causes[0].contains("ermission denied"), "{causes:?}");
    }
    // Histórico da transferência inválido: o 409 de sempre, com a causa no diário.
    let j = joined(&h);
    rewrite_prefix(&j.rollout);
    for _ in 0..2 {
        assert_eq!(h.get(&history("x", THREAD, "?provider=codex")).await.0, 409);
    }
    // Registro ilegível: o 404 de conversa, com a causa no diário.
    std::fs::write(&j.record, "{}").unwrap();
    let id = format!("transfer:{TRANSFER}:claude:{}", j.collision);
    for _ in 0..2 {
        assert_eq!(h.get(&image("project", THREAD, &id, "0", "")).await.0, 404);
    }
    let causes = h.reported("arquivo.transferencia_ilegivel");
    assert_eq!(causes.len(), 2, "uma por fonte, não por pedido: {causes:?}");
    assert!(causes.iter().all(|c| c.len() > TRANSFER.len()), "{causes:?}");
}

/// ARQ-38: `tail` acima de 200 corta em 200, as últimas, em ordem.
#[tokio::test]
async fn arq38_tail_is_capped_at_200() {
    let h = Harness::new().await;
    let sid = "11111111-2222-4333-8444-666666666666";
    let dir = h.home.join(".claude-a/projects/-tmp-teto");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = (0..250).map(|i| json!({"type": "user", "uuid": format!("u{i}"), "timestamp": format!("2026-10-01T12:{:02}:{:02}Z", i / 60, i % 60),
        "message": {"role": "user", "content": format!("mensagem {i}")}}).to_string() + "\n").collect();
    std::fs::write(dir.join(format!("{sid}.jsonl")), body).unwrap();
    let (status, body) = h.get(&history("-tmp-teto", sid, "?tail=500")).await;
    assert_eq!(status, 200, "{body}");
    let ids: Vec<String> = body.as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect();
    assert_eq!(ids, (50..250).map(|i| format!("u{i}")).collect::<Vec<_>>());
}

fn pi_session(dir: &Path, sid: &str, text: &str, mtime: u64) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("2026-10-01T10-00-00-000Z_{sid}.jsonl"));
    let lines = [json!({"type": "session", "version": 3, "id": sid, "timestamp": "2026-10-01T10:00:00.000Z", "cwd": "/repo"}),
        json!({"type": "message", "id": "m1", "parentId": null, "timestamp": "2026-10-01T10:00:01.000Z",
            "message": {"role": "user", "content": [{"type": "text", "text": text}], "timestamp": 1_790_000_000_000u64}})];
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime)).unwrap();
}

/// PRV-09: o mesmo sid do Pi em duas pastas: o mais novo vence; sid que não é UUID é 400.
#[tokio::test]
async fn prv09_pi_newest_wins_and_non_uuid_is_refused() {
    let h = Harness::new().await;
    let sid = "aaaaaaaa-1111-4222-8333-444444444444";
    let root = h.home.join(".pi/agent/sessions");
    pi_session(&root.join("--repo-velho--"), sid, "texto velho", 1_790_000_000);
    pi_session(&root.join("--repo-novo--"), sid, "texto novo", 1_790_000_500);
    let (status, body) = h.get(&history("x", sid, "?provider=pi")).await;
    assert_eq!(status, 200, "{body}");
    let texts: Vec<&str> = body.as_array().unwrap().iter().filter_map(|e| e["text"].as_str()).collect();
    assert_eq!(texts, ["texto novo"]);
    let (status, body) = h.get(&history("x", "nao-uuid", "?provider=pi")).await;
    assert_eq!((status, &body["detail"]["code"]), (400, &json!("erro_path_invalido")), "{body}");
}

/// PRV-19 (`test_codex_id_duplicado_exige_conta`): o mesmo sid em duas contas Codex sem conta pedida é
/// 409; com a conta, o rollout dela.
#[tokio::test]
async fn prv19_same_codex_sid_in_two_accounts_needs_the_account() {
    let h = Harness::new().await;
    let from = std::fs::read_dir(h.home.join(".codex/sessions/2026/08/30")).unwrap().flatten().next().unwrap().path();
    let to = h.home.join(".codex-b/sessions/2026/08/30");
    std::fs::create_dir_all(&to).unwrap();
    std::fs::copy(&from, to.join(from.file_name().unwrap())).unwrap();
    let (status, body) = h.get(&history("x", CODEX_A, "?provider=codex")).await;
    assert_eq!((status, &body["detail"]["code"]), (409, &json!("codex_account_ambiguous_rollout")), "{body}");
    let (status, body) = h.get(&history("x", CODEX_A, "?provider=codex&codex_account=b")).await;
    assert_eq!(status, 200, "{body}");
}

/// O histórico e a imagem levam CORS como o resto das rotas do Rust, também no 422: com `Origin`, `*`.
#[tokio::test]
async fn history_and_image_answer_with_cors() {
    let h = Harness::new().await;
    for (path, status) in [(history(NORMAL.0, NORMAL.1, ""), 200), (image(LONG.0, LONG.1, IMAGE_UUID, "0", ""), 200),
        (history(NORMAL.0, NORMAL.1, "?tail=abc"), 422), (image(LONG.0, LONG.1, IMAGE_UUID, "a", ""), 422)] {
        let response = h.client.get(h.url(&path)).bearer_auth(OWNER).header("origin", "http://app.local").send().await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap()), Some("*"), "{path}");
    }
}

/// O rollout transferido entra no índice da busca só depois da fronteira, com os ids do histórico
/// composto; o "ok" importado da origem fica fora (a parte Claude vem do transcript dela). O trecho
/// achado abre no contexto pelo mesmo id.
#[tokio::test]
async fn transferred_rollout_is_indexed_after_the_boundary_with_the_composed_ids() {
    let h = Harness::new().await;
    let j = joined(&h);
    let env = [("HOME", h.home.to_string_lossy().into_owned()), ("USERPROFILE", h.home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR", h.home.join(".claude-a").to_string_lossy().into_owned())];
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map(env.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let index = hangar_server::search::Index::new(&h.home.join("idx-transfer"), Arc::new(|| Ok(Vec::new())),
        Arc::new(|_, _| {}), Default::default()).with_agents(env);
    index.update().unwrap();
    // Pela grafia resolvida: no Windows o rollout Codex é gravado sem o `\\?\`.
    let rollout = hangar_server::accounts::catalog::resolved(&j.rollout);
    let found = |q: &str| index.search(&[q.to_owned()], 50).unwrap().unwrap().into_iter()
        .filter(|r| hangar_server::accounts::catalog::resolved(Path::new(&r.path)) == rollout)
        .map(|r| r.event_id).collect::<Vec<_>>();
    let id = format!("transfer:{TRANSFER}:codex:{}", j.collision);
    assert_eq!(found("novo"), [id.clone()]);
    assert!(found("ok").is_empty(), "a linha importada não entra pelo rollout");
    let project = h.home.join("transfer").to_string_lossy().replace('/', "-");
    let (status, body) = h.get(&format!("/api/search/context?project={project}&session_id={THREAD}&event_id={}&around=0&provider=codex",
        id.replace(':', "%3A"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.as_array().unwrap().iter().map(|e| e["id"].clone()).collect::<Vec<_>>(), [json!(id)]);
    // O turno novo depois de indexado: só a linha nova é lida.
    std::fs::OpenOptions::new().append(true).open(&j.rollout).map(|mut f| std::io::Write::write_all(&mut f,
        line(&codex_message("depois", "later", 3)).as_bytes())).unwrap().unwrap();
    index.update().unwrap();
    assert_eq!(found("depois").len(), 1);
    assert!(found("depois")[0].starts_with(&format!("transfer:{TRANSFER}:codex:")));
}

/// Transferência que a lista vê e o histórico composto não acha (o índice aponta o id noutra grafia):
/// o rollout não entra cru no índice, e o que ele já tinha fica.
// A outra grafia só desencontra num sistema de arquivos que diferencia maiúsculas.
#[cfg(not(any(windows, target_os = "macos")))]
#[tokio::test]
async fn transferred_rollout_without_a_readable_record_is_not_indexed_raw_and_keeps_its_rows() {
    let h = Harness::new().await;
    let j = joined(&h);
    let codex_home = h.home.join(".codex");
    let key = format!("[{}, {}]", serde_json::to_string(&codex_home.canonicalize().unwrap()).unwrap(), serde_json::to_string(THREAD).unwrap());
    let thread_index = h.home.join(".hangar/conversation-transfers/indexes").join(format!("thread-{}.json", sha(key.as_bytes())));
    let point = |id: &str| std::fs::write(&thread_index, json!([id]).to_string()).unwrap();
    let env = [("HOME", h.home.to_string_lossy().into_owned()), ("USERPROFILE", h.home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR", h.home.join(".claude-a").to_string_lossy().into_owned())];
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map(env.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let index = hangar_server::search::Index::new(&h.home.join("idx-transfer"), Arc::new(|| Ok(Vec::new())),
        Arc::new(|_, _| {}), Default::default()).with_agents(env);
    // Pela grafia resolvida: no Windows o rollout Codex é gravado sem o `\\?\`.
    let rollout = hangar_server::accounts::catalog::resolved(&j.rollout);
    let found = |q: &str| index.search(&[q.to_owned()], 50).unwrap().unwrap().into_iter()
        .filter(|r| hangar_server::accounts::catalog::resolved(Path::new(&r.path)) == rollout)
        .map(|r| r.event_id).collect::<Vec<_>>();
    point(&TRANSFER.to_uppercase());
    index.update().unwrap();
    assert!(found("ok").is_empty() && found("novo").is_empty(), "sem o registro, nada do rollout entra");
    point(TRANSFER);
    index.update().unwrap();
    assert_eq!(found("novo").len(), 1);
    point(&TRANSFER.to_uppercase());
    std::fs::OpenOptions::new().append(true).open(&j.rollout).map(|mut f| std::io::Write::write_all(&mut f,
        line(&codex_message("depois", "later", 3)).as_bytes())).unwrap().unwrap();
    index.update().unwrap();
    assert_eq!(found("novo").len(), 1, "o que o índice tinha fica");
    assert!(found("depois").is_empty());
}
