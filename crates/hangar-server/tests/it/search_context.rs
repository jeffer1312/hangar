//! `GET /api/search/context` (R3, CTX-01..16): o trecho em volta de um achado da busca. O esperado de
//! cada caso vem da captura do Python (`contract/context/` da migração) sobre as fixtures de
//! `tests/fixtures/search/home`, copiadas para uma pasta temporária.
use super::fake::{Fake, OWNER, client, spawn_fake};
use axum::http::StatusCode;
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::{AppState, router},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::SocketAddr, path::{Path, PathBuf}, sync::Arc};

const NORMAL_P: &str = "-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1";
const NORMAL_S: &str = "0cc60e6c-eafc-4915-9936-f0c7b81191d9";
const MID: &str = "d680b1f0-7798-4fa1-99e4-7a4489733b1e";

/// `normal_around10` da captura: as 12 mensagens com texto da conversa normal, em ordem.
const NORMAL: [&str; 12] = [
    "e28dad79-1f8e-4080-8d13-078fc75b457b", "b2e0ff08-b196-46af-92c2-c24c9cb10506",
    "9b8c07e4-eeeb-4330-bbdc-b03b87781b47", "22793930-5948-41b9-bfd6-05bda0c62579",
    "fc24023f-dd47-43c4-af74-54ae2d325ca9", "2a51872b-a9b0-4544-bf07-93f4e4a20d0d",
    "d680b1f0-7798-4fa1-99e4-7a4489733b1e", "8f07b64f-0545-41e5-a0d1-fe8fc46d1024",
    "f207bd0c-89b1-435e-b6b2-b310d57b91ef", "bb7c2442-1001-4997-94f9-8de68f5aded5",
    "5e8cb008-5006-4c01-9db4-b3f34791fd91", "dd7ece7e-9d51-4d61-9243-44cc7fbc8840",
];

struct Scene {
    _dir: tempfile::TempDir,
    home: PathBuf,
    python: Arc<Fake>,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Scene {
    fn drop(&mut self) { self.task.abort(); }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() { copy_tree(&entry.path(), &target) } else { std::fs::copy(entry.path(), target).unwrap(); }
    }
}

impl Scene {
    /// HOME com as fixtures; `.claude-a` é a conta do servidor, as outras vêm do catálogo (ou de `fixed`).
    async fn new(fixed: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        copy_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home"), &home);
        let kimi = home.join(".kimi-code/session_index.jsonl");
        std::fs::write(&kimi, std::fs::read_to_string(&kimi).unwrap().replace("{HOME}", &home.to_string_lossy())).unwrap();
        let (python, upstream) = spawn_fake().await;
        let cfg = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            auth_token: OWNER.into(),
            internal_secret: "isolated".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        };
        let mut state = AppState::new(cfg);
        state.hangar_dir = dir.path().join(".hangar");
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let mut env = BTreeMap::from([
            ("HOME".to_owned(), text(&home)),
            ("USERPROFILE".to_owned(), text(&home)),
            ("CLAUDE_CONFIG_DIR".to_owned(), text(&home.join(".claude-a"))),
            ("PATH".to_owned(), String::new()),
        ]);
        if let Some(fixed) = fixed { env.insert("CP_CLAUDE_CONFIG_DIRS".into(), fixed.replace("{HOME}", &text(&home))); }
        state.accounts = AccountService::new(AccountEnvironment::from_map(env));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        Self { _dir: dir, home, python, address, task }
    }

    async fn get(&self, query: &str) -> (StatusCode, Value) {
        let response = client().get(format!("http://{}/api/search/context?{query}", self.address))
            .bearer_auth(OWNER).send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn ctx(&self, project: &str, sid: &str, event: &str, around: Option<&str>) -> (StatusCode, Value) {
        let around = around.map_or(String::new(), |a| format!("&around={a}"));
        self.get(&format!("project={project}&session_id={sid}&event_id={event}{around}")).await
    }
}

fn ids(body: &Value) -> Vec<String> {
    body.as_array().expect("lista de eventos").iter().map(|e| e["id"].as_str().unwrap().to_owned()).collect()
}

fn detail(code: &str, msg: &str) -> Value { json!({"detail": {"code": code, "params": {}, "msg": msg}}) }

#[tokio::test]
async fn search_context_requires_the_three_ids_and_defaults_around_to_three() {
    // CTX-01
    let scene = Scene::new(None).await;
    let (status, body) = scene.get("").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body, json!({"detail": [
        {"type": "missing", "loc": ["query", "project"], "msg": "Field required", "input": null},
        {"type": "missing", "loc": ["query", "session_id"], "msg": "Field required", "input": null},
        {"type": "missing", "loc": ["query", "event_id"], "msg": "Field required", "input": null},
    ]}));
    let (status, body) = scene.get(&format!("project={NORMAL_P}&session_id={NORMAL_S}&event_id={MID}&around=abc")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body, json!({"detail": [{"type": "int_parsing", "loc": ["query", "around"],
        "msg": "Input should be a valid integer, unable to parse string as an integer", "input": "abc"}]}));
    let (status, body) = scene.ctx(NORMAL_P, NORMAL_S, MID, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), NORMAL[3..10]);
    // Como o int do FastAPI: espaço em volta, `3.0` e o último valor repetido valem.
    assert_eq!(ids(&scene.ctx(NORMAL_P, NORMAL_S, MID, Some("%203%20")).await.1), NORMAL[3..10]);
    assert_eq!(ids(&scene.ctx(NORMAL_P, NORMAL_S, MID, Some("3.0")).await.1), NORMAL[3..10]);
    assert_eq!(ids(&scene.ctx(NORMAL_P, NORMAL_S, MID, Some("9&around=0")).await.1), [MID]);
}

#[tokio::test]
async fn search_context_clamps_around_between_zero_and_ten() {
    // CTX-02
    let scene = Scene::new(None).await;
    for (around, want) in [("0", &NORMAL[6..7]), ("10", &NORMAL[..]), ("50", &NORMAL[..]), ("-1", &NORMAL[6..7]),
                           ("99999999999999999999999", &NORMAL[..])] {
        let (status, body) = scene.ctx(NORMAL_P, NORMAL_S, MID, Some(around)).await;
        assert_eq!(status, StatusCode::OK, "around={around}");
        assert_eq!(ids(&body), want, "around={around}");
    }
}

#[tokio::test]
async fn search_context_cuts_at_the_edges_and_serializes_like_the_python() {
    // CTX-03 e CTX-04
    let scene = Scene::new(None).await;
    assert_eq!(ids(&scene.ctx(NORMAL_P, NORMAL_S, NORMAL[0], Some("3")).await.1), NORMAL[..4]);
    assert_eq!(ids(&scene.ctx(NORMAL_P, NORMAL_S, NORMAL[11], Some("3")).await.1), NORMAL[8..]);
    let (_, body) = scene.ctx(NORMAL_P, NORMAL_S, MID, Some("0")).await;
    assert_eq!(body, json!([{"kind": "assistant_msg", "id": MID, "text": "RED confirmado (tipos ausentes). Implementando `hcc-protocolo`.",
        "tool_name": null, "tool_input": null, "tool_use_id": null, "result": null, "is_error": null, "patch": null,
        "bg_agent_id": null, "ts": 1791545589.917, "cache_read": 136260, "cache_ttl_s": 3600, "desistiu": null,
        "hook_error": null, "skill": null, "orq": null, "queued_delivered": null, "queued_ts": null,
        "queued_confirmed": null, "queued_consumed": null, "image_count": null}]));
}

#[tokio::test]
async fn search_context_rejects_invalid_paths_and_missing_transcripts() {
    // CTX-05 e CTX-06
    let scene = Scene::new(None).await;
    let invalid = detail("erro_path_invalido", "invalid path");
    for (project, sid) in [("../etc", NORMAL_S), ("-home-jefferson.x", NORMAL_S), (NORMAL_P, "nao-e-uuid"),
                           (NORMAL_P, "0cc60e6c-eafc-4915-9936-f0c7b81191d9.from-a")] {
        assert_eq!(scene.ctx(project, sid, MID, Some("3")).await, (StatusCode::BAD_REQUEST, invalid.clone()), "{project} {sid}");
    }
    assert_eq!(scene.ctx(NORMAL_P, "00000000-0000-4000-8000-000000000000", MID, Some("3")).await,
        (StatusCode::NOT_FOUND, detail("erro_transcript_nao_encontrado", "transcript not found")));
}

#[tokio::test]
async fn search_context_searches_every_account_and_the_first_with_the_file_wins() {
    // CTX-07
    let scene = Scene::new(None).await;
    let account_b = ["6a423210-6022-4738-a03a-4933cb59ab2d", "f94331cf-dc2c-4f1e-890e-b6a3d87836a0",
        "6f580d44-3a57-48cf-a736-207526433f7f", "5890f428-ec93-4815-ada4-1451c1f963e3", "e377668a-01cd-4001-9271-9fefd467b08c"];
    for project in ["-home-jefferson-Projetos-hangar-mobile-deliveries-cad3e6fe-t32", "-home-jefferson-Projetos-hangar"] {
        let (status, body) = scene.ctx(project, "db401d8f-fc08-463c-a3cc-4734fc5dc634", account_b[1], Some("3")).await;
        assert_eq!((status, ids(&body)), (StatusCode::OK, account_b.map(String::from).to_vec()), "{project}");
    }
    // A mesma conversa nas duas contas: vale a da conta que vem antes no catálogo.
    for (fixed, want) in [("b:{HOME}/.claude-b,a:{HOME}/.claude-a", "so-na-b"), ("a:{HOME}/.claude-a,b:{HOME}/.claude-b", MID)] {
        let scene = Scene::new(Some(fixed)).await;
        let copy = scene.home.join(".claude-b/projects").join(NORMAL_P).join(format!("{NORMAL_S}.jsonl"));
        std::fs::write(&copy, format!("{}\n", json!({"type": "user", "uuid": "so-na-b", "timestamp": "2026-10-01T10:00:00Z",
            "message": {"role": "user", "content": MID}}))).unwrap();
        let (_, body) = scene.ctx(NORMAL_P, NORMAL_S, want, Some("0")).await;
        assert_eq!(ids(&body), [want], "{fixed}");
    }
}

#[tokio::test]
async fn search_context_reads_claude_without_provider_and_the_named_agent_with_it() {
    // CTX-08 com a R5 (N2): sem `provider` é Claude; com ele, o agente pedido — o Python ignorava o
    // parâmetro e lia a conversa Claude.
    let scene = Scene::new(None).await;
    let (status, body) = scene.get(&format!("project={NORMAL_P}&session_id={NORMAL_S}&event_id={MID}&around=0")).await;
    assert_eq!((status, ids(&body)), (StatusCode::OK, vec![MID.to_owned()]));
    let (status, body) = scene.get(&format!("project={NORMAL_P}&session_id={NORMAL_S}&event_id={MID}&around=0&provider=claude")).await;
    assert_eq!((status, ids(&body)), (StatusCode::OK, vec![MID.to_owned()]));
    let (status, body) = scene.get(&format!("project={NORMAL_P}&session_id={NORMAL_S}&event_id={MID}&around=0&provider=codex")).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("erro_transcript_nao_encontrado")), "{body}");
}

#[tokio::test]
async fn search_context_without_the_event_is_404() {
    // CTX-09
    let scene = Scene::new(None).await;
    assert_eq!(scene.ctx(NORMAL_P, NORMAL_S, "nao-existe", Some("3")).await,
        (StatusCode::NOT_FOUND, detail("erro_trecho_nao_encontrado", "a mensagem não está mais nessa conversa")));
}

/// Conversa sintética: relógio fora de ordem, linha sem relógio, blocos extras, `tool_result` e a fila.
fn synthetic(scene: &Scene) -> PathBuf {
    let dir = scene.home.join(".claude-a/projects/-p");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("11111111-1111-4111-8111-111111111111.jsonl");
    let lines = [
        json!({"type": "user", "uuid": "u1", "timestamp": "2026-10-01T10:00:02.000Z", "message": {"role": "user", "content": "primeira"}}),
        json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-10-01T10:00:01.000Z", "message": {"role": "assistant", "content": [
            {"type": "text", "text": "bloco um"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            {"type": "text", "text": "bloco dois"}]}}),
        json!({"type": "assistant", "uuid": "a2", "message": {"role": "assistant", "content": [{"type": "text", "text": "sem relogio"}]}}),
        json!({"type": "user", "uuid": "r1", "timestamp": "2026-10-01T10:00:03.000Z", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "saida"}]}}),
        json!({"type": "assistant", "uuid": "a3", "timestamp": "2026-10-01T10:00:06.000Z", "message": {"role": "assistant", "content": [
            {"type": "text", "text": "ultima"}]}}),
    ];
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let queue = scene.home.join(".claude-a/.hangar-queue");
    std::fs::create_dir_all(&queue).unwrap();
    std::fs::write(queue.join("__archive__.jsonl"), "{\"id\":\"q1\",\"text\":\"da fila\",\"ts\":1790848804.0,\"delivered\":true}\n").unwrap();
    path
}

#[tokio::test]
async fn search_context_orders_by_clock_merges_the_archive_queue_and_numbers_extra_blocks() {
    // CTX-10, CTX-13 e CTX-14. Esperado: `merged_history("__archive__", …)` do Python sobre a mesma conversa
    // dá a1, a1:1 (tool_use), a1:2, a2, u1, r1 (tool_result), queued-q1, a3.
    let scene = Scene::new(None).await;
    synthetic(&scene);
    let sid = "11111111-1111-4111-8111-111111111111";
    let (status, body) = scene.ctx("-p", sid, "a2", Some("10")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), ["a1", "a1:2", "a2", "u1", "queued-q1", "a3"]);
    assert_eq!(body[4]["text"], "da fila");
    assert_eq!(ids(&scene.ctx("-p", sid, "queued-q1", Some("1")).await.1), ["u1", "queued-q1", "a3"]);
    assert_eq!(scene.ctx("-p", sid, "a1:1", Some("1")).await.0, StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn search_context_with_an_unreadable_transcript_is_404_of_the_excerpt() {
    // CTX-13: o Python abre o jsonl, falha e devolve histórico vazio.
    use std::os::unix::fs::PermissionsExt;
    let scene = Scene::new(None).await;
    let path = synthetic(&scene);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() { return; } // root lê mesmo assim
    assert_eq!(scene.ctx("-p", "11111111-1111-4111-8111-111111111111", "a2", Some("1")).await,
        (StatusCode::NOT_FOUND, detail("erro_trecho_nao_encontrado", "a mensagem não está mais nessa conversa")));
}

#[tokio::test]
async fn search_context_drops_what_the_resume_rewrote() {
    // CTX-11 (F5 é do índice: o contexto continua com o filtro de reescrita do histórico)
    let scene = Scene::new(None).await;
    let (p, s) = ("-home-jefferson-Projetos-hangar", "75fea57d-2237-43fe-a669-bb1ed97c6eae");
    assert_eq!(scene.ctx(p, s, "1c3fcd4c-a0d5-5f03-978c-2be4a25ee107", Some("3")).await,
        (StatusCode::NOT_FOUND, detail("erro_trecho_nao_encontrado", "a mensagem não está mais nessa conversa")));
    let (status, body) = scene.ctx(p, s, "611a6a8e-a4af-4b70-b180-a63135053e2d", Some("3")).await;
    assert_eq!((status, ids(&body)), (StatusCode::OK, ["9156b523-547a-47b1-aab4-b1639a0bbf9d", "adceca70-4d19-4869-86cb-fa797c76cc88",
        "b8f1317c-918f-4ab2-8f17-d496dde393d4", "611a6a8e-a4af-4b70-b180-a63135053e2d", "695b09f4-e231-4648-9611-c3cdbcd66ff5",
        "c00191d3-72bf-4272-9a9b-8bd4db958d2b"].map(String::from).to_vec()));
}

#[tokio::test]
async fn search_context_shows_a_repeated_held_prompt_once() {
    // CTX-12 e a linha longa com imagem (`long_line`)
    let scene = Scene::new(None).await;
    let (status, body) = scene.ctx("-tmp-cx-probe-work", "f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53", "held:9decf68c", Some("3")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([{"kind": "user_msg", "id": "held:9decf68c", "text": "BLOQUEIA isto", "tool_name": null,
        "tool_input": null, "tool_use_id": null, "result": null, "is_error": null, "patch": null, "bg_agent_id": null,
        "ts": 1792351432.01, "cache_read": null, "cache_ttl_s": null, "desistiu": true,
        "hook_error": "[/tmp/cx-probe-block.sh]: bloqueado pela sonda", "skill": null, "orq": null, "queued_delivered": null,
        "queued_ts": null, "queued_confirmed": null, "queued_consumed": null, "image_count": null}]));
    let (status, body) = scene.ctx("-home-jefferson", "42a7c450-7f7e-4fd7-84ed-ee699d11098d", "e73695a8-1a12-41dd-a965-b6f7bb44f09d", Some("3")).await;
    assert_eq!((status, ids(&body)), (StatusCode::OK, vec!["e73695a8-1a12-41dd-a965-b6f7bb44f09d".to_owned()]));
    assert_eq!(body[0]["image_count"], 1);
}

#[tokio::test]
async fn search_context_is_answered_by_rust_only_for_the_owner() {
    // Sem o token do dono o pedido segue ao Python, que recusa como hoje.
    let scene = Scene::new(None).await;
    let response = client().get(format!("http://{}/api/search/context?project={NORMAL_P}&session_id={NORMAL_S}&event_id={MID}", scene.address))
        .send().await.unwrap();
    let _ = response.status();
    assert_eq!(scene.python.hits_to("/api/search/context"), 1);
    scene.ctx(NORMAL_P, NORMAL_S, MID, None).await;
    assert_eq!(scene.python.hits_to("/api/search/context"), 1);
    assert!(hangar_server::migration_status::rust_route(&axum::http::Method::GET, "/api/search/context"));
}

#[tokio::test]
async fn search_context_opens_agent_conversations_at_the_event() {
    // R5: `provider` (e `codex_account` no Codex) lê pelo parser do agente, o mesmo do histórico do
    // Arquivo: o trecho é o pedaço dele em volta do evento achado.
    let scene = Scene::new(None).await;
    let ctx = |project: &str, sid: &str, event: &str, around: u8, extra: &str| {
        format!("project={project}&session_id={sid}&event_id={event}&around={around}{extra}")
    };
    let pi = ctx("-home-jefferson-Projetos-hangar--worktrees-contas-modelos", "d337d6b8-0b3c-4bbe-8034-939d7c48f246", "793d327e:1", 1, "&provider=pi");
    let (status, body) = scene.get(&pi).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["de39cff4", "793d327e:1", "a7fdff5e"]);
    let omp = ctx("-tmp-claude-1000--home-jefferson-Projetos-hangar-ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da-scratchpad-omp-teste",
        "01a06932-79ae-77e9-b95a-286b6e2e9288", "41c65634", 3, "&provider=omp");
    assert_eq!(ids(&scene.get(&omp).await.1), ["41c65634", "a8e46a50"]);
    let kimi = ctx("-home-jefferson-Projetos-hangar", "session_ab697d05-66da-4b2a-8e80-3d769951e752", "24f7ca2b-aa6e-43e0-baf7-bb65bf16cd2e", 1, "&provider=kimi");
    assert_eq!(ids(&scene.get(&kimi).await.1),
        ["msg_01KZRN34VM46DB40TA297VD8DK", "24f7ca2b-aa6e-43e0-baf7-bb65bf16cd2e", "msg_01KZRN3DFF88A2YVMSSNBTXY5Y"]);
    // Codex da conta `b`: as mensagens do histórico do Arquivo da mesma conversa.
    let (project, sid) = ("-tmp-hangar-native-task4-fixture-jf", "01a0d000-f6ac-74e3-b19b-98d42af1a712");
    let history = client().get(format!("http://{}/api/archive/{project}/{sid}/history?provider=codex&codex_account=b", scene.address))
        .bearer_auth(OWNER).send().await.unwrap().json::<Value>().await.unwrap();
    let msgs: Vec<String> = history.as_array().unwrap().iter()
        .filter(|e| ["user_msg", "assistant_msg"].contains(&e["kind"].as_str().unwrap()) && e["text"].as_str().is_some_and(|t| !t.is_empty()))
        .map(|e| e["id"].as_str().unwrap().to_owned()).collect();
    assert!(msgs.len() >= 3, "{history}");
    let (status, body) = scene.get(&ctx(project, sid, &msgs[1], 1, "&provider=codex&codex_account=b")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), msgs[..3]);
    assert_eq!(ids(&scene.get(&ctx(project, sid, &msgs[1], 1, "&provider=codex")).await.1), msgs[..3], "sem conta, a dona");
    // Conta errada e agente desconhecido: os códigos do Arquivo.
    let (status, body) = scene.get(&ctx(project, sid, &msgs[1], 1, "&provider=codex&codex_account=default")).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::CONFLICT, Some("codex_account_archive_mismatch")));
    let (status, body) = scene.get(&ctx(project, sid, &msgs[1], 1, "&provider=gemini")).await;
    assert_eq!((status, body), (StatusCode::BAD_REQUEST, detail("erro_provider_invalido", "provider invalido")));
    // Sem `provider` continua Claude (CTX-08): a conversa Pi não é achada como Claude.
    let (status, body) = scene.get(&pi.replace("&provider=pi", "")).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("erro_transcript_nao_encontrado")));
    assert_eq!(scene.python.hits_to("/api/search/context"), 0);
}
