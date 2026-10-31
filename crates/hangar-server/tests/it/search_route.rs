//! `GET /api/search` (R2, SRCH-*): trechos de conversa por palavra. O esperado de cada consulta vem da
//! captura do Python (`tests/fixtures/search/contract/search/search_index.json`) sobre as fixtures de
//! `tests/fixtures/search/home`, copiadas para uma pasta temporária com os mesmos mtimes da captura,
//! aplicadas as diferenças aprovadas (F1, F2, F3, F4, M3, N1).
use super::fake::{Fake, OWNER, client, spawn_fake};
use axum::http::StatusCode;
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    list::{bridge::{ListBridge, ListEnv, parse_dirs}, facts::FactsClient, mux::Mux},
    routes::{AppState, router},
    search::{Index, Reporter, Timing},
};
use serde_json::{Value, json};
use std::{collections::{BTreeMap, HashSet}, net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, SystemTime}};

const NORMAL: &str = ".claude-a/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl";
const LIVE_NAME: &str = "sessao-viva";
const T0: u64 = 1_793_000_000;
/// Os mtimes da captura: o i-ésimo arquivo fica em `T0 + 100 * (i + 1)`.
const MTIMES: [&str; 14] = [
    ".codex/sessions/2026/08/30/rollout-2026-08-30T20-47-12-01a05511-e980-71b3-93a9-befda5704b20.jsonl",
    ".pi/agent/sessions/--home-jefferson-Projetos-hangar-.worktrees-contas-modelos--/2026-08-11T09-37-57-248Z_d337d6b8-0b3c-4bbe-8034-939d7c48f246.jsonl",
    ".omp/agent/sessions/-tmp-claude-1000--home-jefferson-Projetos-hangar-ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da-scratchpad-omp-teste/2026-09-03T21-35-10-766Z_01a06932-79ae-77e9-b95a-286b6e2e9288.jsonl",
    ".kimi-code/sessions/wd_hangar_5112ff7a84e0/session_ab697d05-66da-4b2a-8e80-3d769951e752/agents/main/wire.jsonl",
    ".codex-b/sessions/2026/09/23/rollout-2026-09-23T17-41-59-01a0d000-f6ac-74e3-b19b-98d42af1a712.jsonl",
    ".claude-b/projects/-tmp-cx-probe-work/f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53.jsonl",
    ".claude-a/projects/-home-jefferson/42a7c450-7f7e-4fd7-84ed-ee699d11098d.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar-mobile-deliveries-cad3e6fe-t32/db401d8f-fc08-463c-a3cc-4734fc5dc634.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar/db401d8f-fc08-463c-a3cc-4734fc5dc634.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.from-a.jsonl",
    ".claude-b/projects/-home-jefferson-Projetos-hangar/a5a5a5a5-0000-4000-8000-000000000001.jsonl",
    ".claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-2237-43fe-a669-bb1ed97c6eae.jsonl",
    ".claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-2237-43fe-a669-bb1ed97c6eae/subagents/agent-a23f817306f1d1b6a.jsonl",
    NORMAL,
];
const RESERVE: &str = ".claude-a/projects/-home-jefferson/f2afaa93-d2aa-45db-9d68-b430c22deab2.jsonl";
/// mtime da cópia `.from-a` na captura (índice 9 de `MTIMES`): só o `rg` a lia (F4).
const FROM_COPY_MTIME: f64 = 1_793_001_000.0;
/// O `/compact` que o `--resume` regravou: o índice do Python o tinha, o histórico não (M8, F5).
const REWRITTEN: &str = "1c3fcd4c-a0d5-5f03-978c-2be4a25ee107";
/// Chave de mentira que casa o provedor do Pi e do omp e o modelo padrão do Kimi com `kimi:apikey`.
const KIMI_CONFIG: &str = "default_model=\"apikey/k3\"\n[providers.apikey]\ntype=\"kimi\"\napi_key=\"sk-teste\"\nbase_url=\"https://example.invalid\"\n";
const PI_AUTH: &str = r#"{"openai-codex":{"key":"sk-teste"},"opencode-go":{"key":"sk-teste"}}"#;

fn capture() -> Value {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/contract/search/search_index.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["data"]["results"].clone()
}

struct Opts { ready: bool, live: Option<PathBuf>, fixtures: bool }

impl Default for Opts {
    fn default() -> Self { Self { ready: true, live: Some(PathBuf::from(NORMAL)), fixtures: true } }
}

struct Scene {
    _dir: tempfile::TempDir,
    home: PathBuf,
    python: Arc<Fake>,
    index: Index,
    reports: Arc<Mutex<Vec<(String, String)>>>,
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

fn set_mtime(path: &Path, secs: u64) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

impl Scene {
    async fn new(opts: Opts) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        if opts.fixtures {
            copy_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/home"), &home);
            for (i, rel) in MTIMES.iter().enumerate() { set_mtime(&home.join(rel), T0 + 100 * (i as u64 + 1)); }
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
            set_mtime(&home.join(RESERVE), now - 60);
            // R5: o índice do Kimi aponta para este HOME, e credenciais de mentira dão conta ao Pi, ao omp
            // e ao Kimi (o mtime de cada transcript fica o da captura).
            let kimi = home.join(".kimi-code/session_index.jsonl");
            std::fs::write(&kimi, std::fs::read_to_string(&kimi).unwrap().replace("{HOME}", &home.to_string_lossy())).unwrap();
            std::fs::write(home.join(".kimi-code/config.toml"), KIMI_CONFIG).unwrap();
            std::fs::write(home.join(".pi/agent/auth.json"), PI_AUTH).unwrap();
        } else {
            std::fs::create_dir_all(&home).unwrap();
        }
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
        state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".to_owned(), text(&home)),
            ("USERPROFILE".to_owned(), text(&home)),
            ("CLAUDE_CONFIG_DIR".to_owned(), text(&home.join(".claude-a"))),
            ("PATH".to_owned(), String::new()),
        ])));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let report: Reporter = Arc::new(move |code, detail| sink.lock().unwrap().push((code.to_owned(), detail)));
        // Prazos longos: nenhuma passada sozinha no meio do teste.
        let timing = Timing { first_pass: Duration::from_secs(3600), debounce: Duration::from_secs(3600), stale: Duration::from_secs(3600) };
        let index = Index::for_accounts(state.accounts.clone(), &dir.path().join("idx"), report, timing);
        if opts.ready { index.update().unwrap(); }
        state.search = index.clone();
        // Multiplexador de mentira: uma sessão (`sessao-viva`) quando o teste pede um transcript vivo.
        let script = dir.path().join("tmux");
        let pane = if opts.live.is_some() { format!("{LIVE_NAME}\\t1\\t\\t{}\\t%%0\\t\\t\\t\\t0\\t0\\n", home.display()) } else { String::new() };
        super::write_executable(&script, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf '{pane}'\n"));
        let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude-a"), "codex_home": home.join(".codex"),
            "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"),
            "kimi_home": home.join(".kimi-code")}).to_string());
        let list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
            capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
            FactsClient::new(upstream, "isolated".into())));
        if let Some(live) = &opts.live { list.seed(LIVE_NAME, &text(&home.join(live))); }
        state.list = list;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        Self { _dir: dir, home, python, index, reports, address, task }
    }

    async fn raw(&self, query: &str) -> (StatusCode, Value) {
        let response = client().get(format!("http://{}/api/search{query}", self.address)).bearer_auth(OWNER).send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn search(&self, q: &str) -> Value {
        let q: String = form_urlencoded::byte_serialize(q.as_bytes()).collect();
        let (status, body) = self.raw(&format!("?q={q}")).await;
        assert_eq!(status, StatusCode::OK, "{q}: {body}");
        body
    }

    fn account(&self, letter: char) -> String { self.home.join(format!(".claude-{letter}")).to_string_lossy().into_owned() }

    /// A captura com as diferenças aprovadas: sem a cópia `.from-` (F4), sem a linha que o `--resume`
    /// regravou (M8), sem repetido por (session_id, event_id) ficando o mais novo (F1) e com agente e
    /// conta (N1).
    fn expected(&self, hits: &Value) -> Value {
        let mut seen = HashSet::new();
        let out = hits.as_array().unwrap().iter()
            .filter(|h| h["mtime"].as_f64() != Some(FROM_COPY_MTIME))
            .filter(|h| h["event_id"] != REWRITTEN)
            .filter(|h| seen.insert((h["session_id"].clone(), h["event_id"].clone())))
            .map(|h| self.with_account(h)).collect();
        Value::Array(out)
    }

    fn with_account(&self, hit: &Value) -> Value {
        let in_b = ["f8f12de6-7941-4bd0-8f0c-02bc0b4b7f53", "db401d8f-fc08-463c-a3cc-4734fc5dc634", "a5a5a5a5-0000-4000-8000-000000000001"]
            .contains(&hit["session_id"].as_str().unwrap());
        let dir = self.account(if in_b { 'b' } else { 'a' });
        let mut hit = hit.clone();
        let obj = hit.as_object_mut().unwrap();
        obj.insert("provider".into(), json!("claude"));
        obj.insert("conta".into(), json!(format!("claude:{dir}")));
        obj.insert("config_dir".into(), json!(dir));
        obj.insert("codex_account".into(), Value::Null);
        hit
    }
}

/// A captura só tem conversas do Claude: os trechos dos outros agentes (R5, mais velhos nas fixtures,
/// então depois dos do Claude) ficam fora da comparação e têm teste próprio.
async fn assert_like_the_capture(scene: &Scene) {
    let capture = capture();
    for (q, hits) in capture.as_object().unwrap() {
        let mut got = scene.search(q).await;
        got.as_array_mut().unwrap().retain(|h| h["provider"] == "claude");
        assert_eq!(got, scene.expected(hits), "q={q:?}");
    }
}

#[tokio::test]
async fn search_route_finds_other_agents_with_provider_and_account() {
    // R5: trechos de Codex (duas contas), Pi, omp e Kimi saem com agente, conta e conta Codex, pelo
    // índice e pela varredura de antes dele.
    let pick = |hits: Value| -> Vec<Value> {
        hits.as_array().unwrap().iter().map(|h| json!({"provider": h["provider"], "conta": h["conta"], "codex_account": h["codex_account"],
            "config_dir": h["config_dir"], "project": h["project"], "session_id": h["session_id"], "cwd": h["cwd"], "role": h["role"],
            "event_id": h["event_id"].is_string()})).collect()
    };
    let hit = |provider: &str, conta: &str, codex: Option<&str>, project: &str, sid: &str, cwd: &str, role: &str| json!({
        "provider": provider, "conta": conta, "codex_account": codex, "config_dir": null, "project": project, "session_id": sid,
        "cwd": cwd, "role": role, "event_id": true});
    for ready in [true, false] {
        let scene = Scene::new(Opts { ready, ..Opts::default() }).await;
        let home = scene.home.to_string_lossy().into_owned();
        let (codex_a, codex_b) = (format!("codex:{home}/.codex"), format!("codex:{home}/.codex-b"));
        let hangar = "/home/jefferson/Projetos/hangar";
        let a = |role| hit("codex", &codex_a, Some("default"), "-home-jefferson-Projetos-hangar", "01a05511-e980-71b3-93a9-befda5704b20", hangar, role);
        assert_eq!(pick(scene.search("lider2").await), [a("user"), a("assistant")], "ready={ready}");
        let b = |role| hit("codex", &codex_b, Some("b"), "-tmp-hangar-native-task4-fixture-jf", "01a0d000-f6ac-74e3-b19b-98d42af1a712",
            "/tmp/hangar-native-task4-fixture-jf", role);
        assert_eq!(pick(scene.search("continued").await), [b("user"), b("assistant")], "ready={ready}");
        let pi = |role| hit("pi", "kimi:apikey", None, "-home-jefferson-Projetos-hangar--worktrees-contas-modelos",
            "d337d6b8-0b3c-4bbe-8034-939d7c48f246", "/home/jefferson/Projetos/hangar/.worktrees/contas-modelos", role);
        assert_eq!(pick(scene.search("ancora").await), [pi("user"), pi("assistant")], "ready={ready}");
        let omp = hit("omp", "kimi:apikey", None, "-tmp-claude-1000--home-jefferson-Projetos-hangar-ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da-scratchpad-omp-teste",
            "01a06932-79ae-77e9-b95a-286b6e2e9288", "/tmp/claude-1000/-home-jefferson-Projetos-hangar/ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da/scratchpad/omp-teste", "user");
        assert_eq!(pick(scene.search("responda apenas").await), [omp], "ready={ready}");
        let kimi = |role| hit("kimi", "kimi:apikey", None, "-home-jefferson-Projetos-hangar", "session_ab697d05-66da-4b2a-8e80-3d769951e752", hangar, role);
        assert_eq!(pick(scene.search("posso ajudar").await), [kimi("assistant"), kimi("assistant")], "ready={ready}");
    }
}

#[tokio::test]
async fn search_route_answers_from_the_index_like_the_capture() {
    // SRCH-01, 03..06, 08, 11..17, 20..23, 42, 44, 48 (F1), N1 e M3 (termo curto pelo índice)
    let scene = Scene::new(Opts::default()).await;
    assert!(scene.index.ready());
    assert_like_the_capture(&scene).await;
}

#[tokio::test]
async fn search_route_scans_the_transcripts_before_the_index_is_ready() {
    // SRCH-18 e a varredura em processo (M3): acento e caixa ignorados (F2), cópia `.from-` (F4),
    // subagente, linha de mais de 40000 bytes e conversa interna fora; o mesmo resultado do índice.
    let scene = Scene::new(Opts { ready: false, ..Opts::default() }).await;
    assert!(!scene.index.ready());
    assert_like_the_capture(&scene).await;
    assert!(!scene.index.ready(), "a busca não constrói o índice");
}

#[tokio::test]
async fn search_route_needs_every_word_in_one_message() {
    // SRCH-02 e SRCH-03: minúsculas, sem repetir palavra, qualquer ordem; o trecho centra no 1º termo.
    let want = {
        let scene_hits = capture()["Rust"].clone();
        scene_hits.as_array().unwrap().iter().find(|h| h["event_id"] == "e28dad79-1f8e-4080-8d13-078fc75b457b").unwrap().clone()
    };
    for ready in [true, false] {
        let scene = Scene::new(Opts { ready, ..Opts::default() }).await;
        for q in ["rust ok", "RUST   Ok rust"] {
            assert_eq!(scene.search(q).await, json!([scene.with_account(&want)]), "q={q:?} ready={ready}");
        }
    }
}

#[tokio::test]
async fn search_route_marks_live_by_the_real_path_of_the_transcript() {
    // SRCH-17 e M6: o vínculo é pelo realpath dos dois lados, relido a cada pedido.
    #[cfg(unix)]
    {
        let scene = Scene::new(Opts { live: Some(PathBuf::from("link.jsonl")), ..Opts::default() }).await;
        std::os::unix::fs::symlink(scene.home.join(NORMAL), scene.home.join("link.jsonl")).unwrap();
        let hits = scene.search("clippy").await;
        assert!(hits.as_array().unwrap().iter().all(|h| h["live"] == true && h["session_name"] == LIVE_NAME), "{hits}");
    }
    // Outra sessão com o mesmo nome noutro transcript: o trecho antigo não ganha o nome.
    let other = ".claude-a/projects/-home-jefferson-Projetos-hangar/75fea57d-2237-43fe-a669-bb1ed97c6eae.jsonl";
    let scene = Scene::new(Opts { live: Some(PathBuf::from(other)), ..Opts::default() }).await;
    let hits = scene.search("clippy").await;
    assert_eq!(hits.as_array().unwrap().len(), 3);
    assert!(hits.as_array().unwrap().iter().all(|h| h["live"] == false && h["session_name"].is_null()), "{hits}");
    let hits = scene.search("Rust").await;
    let mine: Vec<&Value> = hits.as_array().unwrap().iter().filter(|h| h["session_id"] == "75fea57d-2237-43fe-a669-bb1ed97c6eae").collect();
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|h| h["live"] == true && h["session_name"] == LIVE_NAME), "{hits}");
}

#[tokio::test]
async fn search_route_drops_a_transcript_gone_since_the_index_pass() {
    // SRCH-26 e F3: o índice ainda tem o arquivo, a busca confere o mtime e o tira.
    let scene = Scene::new(Opts::default()).await;
    std::fs::remove_file(scene.home.join(NORMAL)).unwrap();
    assert_eq!(scene.search("clippy").await, json!([]));
}

#[tokio::test]
async fn search_route_does_not_scan_when_the_index_answers_empty() {
    // SRCH-21: linha escrita depois da passada não está no índice; a varredura a acharia.
    let scene = Scene::new(Opts::default()).await;
    // Relógio depois do fim da conversa: mais velho, o filtro de reescrita a descartaria.
    let line = json!({"type": "user", "uuid": "novo-1", "timestamp": "2026-10-30T10:00:00Z", "message": {"role": "user", "content": "zzpalavranova"}});
    let path = scene.home.join(NORMAL);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{line}\n"));
    std::fs::write(&path, text).unwrap();
    assert_eq!(scene.search("zzpalavranova").await, json!([]));
    let fresh = Scene::new(Opts { ready: false, ..Opts::default() }).await;
    std::fs::write(fresh.home.join(NORMAL), std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(fresh.search("zzpalavranova").await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn search_route_falls_back_to_the_scan_when_the_index_fails() {
    // SRCH-19: erro do índice vai ao diário e a resposta sai da varredura.
    let scene = Scene::new(Opts::default()).await;
    // Sem o banco e o WAL a leitura abre um arquivo vazio, sem as tabelas.
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", scene.index.path().display()));
    }
    assert_eq!(scene.search("clippy").await, scene.expected(&capture()["clippy"]));
    let reports = scene.reports.lock().unwrap().clone();
    assert!(reports.iter().any(|(code, _)| code == "busca.indice_falhou"), "{reports:?}");
}

/// `n` conversas em `.claude-a/projects/-p`, a i-ésima com mtime `T0 + i` e `per` pedidos com `palavra`.
fn many(home: &Path, n: usize, per: usize) {
    let dir = home.join(".claude-a/projects/-p");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        let path = dir.join(format!("00000000-0000-4000-8000-{i:012}.jsonl"));
        let lines: String = (0..per).map(|k| format!("{}\n", json!({"type": "user", "uuid": format!("u{i}-{k}"), "cwd": "/p",
            "timestamp": "2026-10-01T10:00:00Z", "message": {"role": "user", "content": format!("palavra {k}")}}))).collect();
        std::fs::write(&path, lines).unwrap();
        set_mtime(&path, T0 + i as u64);
    }
}

#[tokio::test]
async fn search_route_caps_at_fifty_hits_newest_conversation_first() {
    // SRCH-08, 09, 11 e 32: 20 conversas com 4 achados cada; ficam 3 por conversa, da mais nova para a
    // mais velha, até 50 (a 17ª entra com 2). SRCH-10: o teto é exato também na varredura.
    for ready in [true, false] {
        let scene = Scene::new(Opts { ready: false, live: None, fixtures: false }).await;
        many(&scene.home, 20, 4);
        std::fs::create_dir_all(scene.home.join(".claude-a")).unwrap();
        std::fs::write(scene.home.join(".claude-a/.credentials.json"), "{}").unwrap();
        if ready { scene.index.update().unwrap(); }
        let hits = scene.search("palavra").await;
        let got: Vec<String> = hits.as_array().unwrap().iter().map(|h| h["event_id"].as_str().unwrap().to_owned()).collect();
        let want: Vec<String> = (0..20).rev().flat_map(|i| (0..3).map(move |k| format!("u{i}-{k}"))).take(50).collect();
        assert_eq!(got, want, "ready={ready}");
    }
}

#[tokio::test]
async fn search_route_scan_reads_at_most_200_matching_lines_and_skips_files_over_64_mb() {
    // SRCH-34 e SRCH-35 (só a varredura; o índice não tem esses tetos).
    let scene = Scene::new(Opts { ready: false, live: None, fixtures: false }).await;
    std::fs::create_dir_all(scene.home.join(".claude-a/projects/-p")).unwrap();
    std::fs::write(scene.home.join(".claude-a/.credentials.json"), "{}").unwrap();
    let noise = |i: usize| format!("{}\n", json!({"type": "user", "uuid": format!("r{i}"), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t", "content": "zzalvo na saida"}]}}));
    let ask = |id: &str| format!("{}\n", json!({"type": "user", "uuid": id, "message": {"role": "user", "content": "zzalvo aqui"}}));
    let file = |sid: &str| scene.home.join(format!(".claude-a/projects/-p/{sid}.jsonl"));
    let (at_199, at_200) = ("00000000-0000-4000-8000-000000000199", "00000000-0000-4000-8000-000000000200");
    std::fs::write(file(at_199), (0..199).map(noise).collect::<String>() + &ask("depois-de-199")).unwrap();
    std::fs::write(file(at_200), (0..200).map(noise).collect::<String>() + &ask("depois-de-200")).unwrap();
    let big = "00000000-0000-4000-8000-0000000000bb";
    std::fs::write(file(big), ask("grande")).unwrap();
    std::fs::File::options().write(true).open(file(big)).unwrap().set_len(64 * 1024 * 1024 + 1).unwrap();
    let small = "00000000-0000-4000-8000-0000000000aa";
    std::fs::write(file(small), ask("pequeno")).unwrap();
    let hits = scene.search("zzalvo").await;
    let mut got: Vec<&str> = hits.as_array().unwrap().iter().map(|h| h["event_id"].as_str().unwrap()).collect();
    got.sort_unstable();
    assert_eq!(got, ["depois-de-199", "pequeno"]);
}

#[tokio::test]
async fn search_route_without_any_account_is_empty() {
    // SRCH-07: nenhuma pasta de conversas, nas duas formas.
    for ready in [true, false] {
        let scene = Scene::new(Opts { ready, live: None, fixtures: false }).await;
        assert_eq!(scene.search("clippy").await, json!([]), "ready={ready}");
    }
}

#[tokio::test]
async fn search_route_is_answered_by_rust_only_for_the_owner() {
    // SRCH-43: só o dono no Rust, `q` ausente vale "" e devolve [].
    let scene = Scene::new(Opts::default()).await;
    assert_eq!(scene.raw("").await, (StatusCode::OK, json!([])));
    assert_eq!(scene.raw("?q=%20%20").await, (StatusCode::OK, json!([])));
    assert_eq!(scene.python.hits_to("/api/search"), 0);
    let response = client().get(format!("http://{}/api/search?q=clippy", scene.address)).send().await.unwrap();
    let _ = response.status();
    assert_eq!(scene.python.hits_to("/api/search"), 1);
    assert!(hangar_server::migration_status::rust_route(&axum::http::Method::GET, "/api/search"));
}
