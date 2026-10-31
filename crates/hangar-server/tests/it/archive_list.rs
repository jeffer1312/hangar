//! R6+R7: o Arquivo (`GET /api/archive`, `/api/archive/{project}`, `/api/archive/recent` e
//! `/api/archive-por-cwd`) no Rust, comparado com a captura do Python
//! (`tests/fixtures/search/contract/archive/archive_{index,noindex}.json`) sobre o HOME de
//! `tests/fixtures/search/home/`, com o índice pronto e sem ele. O esperado é a captura com as
//! diferenças aceitas aplicadas: F7 (reserva sem usuário não ocupa vaga), F8 (conversa interna fora) e
//! F9 (conversa que mudou de pasta só na pasta do caminho mais novo).
use axum::{Router, extract::Request, http::StatusCode, response::IntoResponse};
use hangar_server::{auth::TrustedHosts, config::Config, routes::{AppState, router}};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};

const OWNER: &str = "dono-de-teste";
const SECRET: &str = "interno-de-teste";
const T0: u64 = 1_793_000_000;
// Os mtimes da captura, do mais velho ao mais novo (T0 + 100 * posição).
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
    ".claude-a/projects/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1/0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl",
];
const RESERVE: &str = ".claude-a/projects/-home-jefferson/f2afaa93-d2aa-45db-9d68-b430c22deab2.jsonl";
const NORMAL_CWD: &str = "/home/jefferson/Projetos/hangar-computer-control-9c0b634f-t1";
const HANGAR: &str = "-home-jefferson-Projetos-hangar";
const INTERNAL: &str = "a5a5a5a5-0000-4000-8000-000000000001";
const MOVED: &str = "db401d8f-fc08-463c-a3cc-4734fc5dc634";
const MOVED_OLD_PROJECT: &str = "-home-jefferson-Projetos-hangar-mobile-deliveries-cad3e6fe-t32";
const THREAD: &str = "019f99de-9572-7221-89c0-80c62f883d44";
const TRANSFER: &str = "a563a017-af37-42e8-a8e5-2bd837584f8e";

fn fixtures() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search") }

fn capture(mode: Mode) -> Value {
    let name = match mode { Mode::Index => "archive_index.json", Mode::NoIndex => "archive_noindex.json" };
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

fn set_mtime(path: &Path, at: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
}

#[derive(Clone, Copy, PartialEq)]
enum Mode { Index, NoIndex }

struct Harness {
    _dir: tempfile::TempDir,
    home: PathBuf,
    address: SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    client: reqwest::Client,
    search: hangar_server::search::Index,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) { for task in &self.tasks { task.abort(); } }
}

/// O HOME da captura: fixtures copiadas, mtimes fixos, a reserva de agora há pouco e o nome fechado
/// da cópia mais nova de `db401d8f`.
fn prepare_home(home: &Path) {
    copy_tree(&fixtures().join("home"), home);
    let index = home.join(".kimi-code/session_index.jsonl");
    std::fs::write(&index, std::fs::read_to_string(&index).unwrap().replace("{HOME}", &home.to_string_lossy())).unwrap();
    for (i, rel) in MTIMES.iter().enumerate() {
        set_mtime(&home.join(rel), UNIX_EPOCH + Duration::from_secs(T0 + 100 * (i as u64 + 1)));
    }
    set_mtime(&home.join(RESERVE), SystemTime::now() - Duration::from_secs(60));
    let moved_new = home.join(MTIMES[8]).canonicalize().unwrap();
    std::fs::write(home.join(".claude-a/closed-sessions.json"),
        json!({moved_new.to_string_lossy(): {"name": "nome-fechado", "closed_at": T0}}).to_string()).unwrap();
}

impl Harness {
    async fn new(mode: Mode) -> Self { Self::with(mode, |_| {}).await }

    /// `edit` mexe no HOME antes do índice ser montado.
    async fn with(mode: Mode, edit: impl FnOnce(&Path)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        prepare_home(&home);
        edit(&home);
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
        let claude = home.join(".claude-a");
        let env = [("HOME", home.to_string_lossy().into_owned()), ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", claude.to_string_lossy().into_owned())];
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
            env.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()));
        // O índice numa pasta própria, com as duas contas da captura.
        let dirs = vec![claude.to_string_lossy().into_owned(), home.join(".claude-b").to_string_lossy().into_owned()];
        let server_dir = claude.clone();
        let accounts: hangar_server::search::AccountSource = Arc::new(move || Ok(hangar_server::search::server_accounts(&dirs, &server_dir)));
        let search = hangar_server::search::Index::new(&dir.path().join("idx"), accounts, Arc::new(|_, _| {}), Default::default());
        if mode == Mode::Index { search.update().unwrap(); }
        state.search = search.clone();
        state.list = Arc::new(live_list(dir.path(), &home, upstream));
        let app = router(Arc::new(state));
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        Self { _dir: dir, home, address, hits, client: reqwest::Client::new(), search, tasks: vec![up_task, server] }
    }

    fn url(&self, path: &str) -> String { format!("http://{}{path}", self.address) }

    /// (status, corpo com o HOME trocado por `{HOME}`, como a captura).
    async fn get(&self, path: &str) -> (u16, Value) {
        let response = self.client.get(self.url(path)).bearer_auth(OWNER).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap().replace(&*self.home.to_string_lossy(), "{HOME}");
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    fn assert_not_forwarded(&self) {
        let hits = self.hits.lock().unwrap();
        assert!(!hits.iter().any(|h| h.starts_with("/api/archive")), "foi repassado ao Python: {hits:?}");
    }

    fn dir(&self, name: &str) -> String { self.home.join(name).to_string_lossy().into_owned() }
}

/// A lista do dono com uma sessão Claude viva (`sessao-viva`) no transcript normal da captura.
fn live_list(root: &Path, home: &Path, upstream: SocketAddr) -> hangar_server::list::bridge::ListBridge {
    use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
    let script = root.join("tmux");
    crate::write_executable(&script, format!(
        "#!/bin/sh\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf 'sessao-viva\\t1\\t\\t{NORMAL_CWD}\\t%%0\\t\\t\\t\\t0\\t0\\n'\n"));
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude-a"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    ListBridge::new(ListEnv { mux: hangar_server::list::mux::Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        hangar_server::list::facts::FactsClient::new(upstream, SECRET.into()))
}

/// F8 e F9 numa lista de conversas da captura.
fn accepted(entries: &Value) -> Value {
    Value::Array(entries.as_array().unwrap().iter().filter(|e| {
        e["session_id"] != INTERNAL && !(e["session_id"] == MOVED && e["project"] == MOVED_OLD_PROJECT)
    }).cloned().collect())
}

/// O `msg` das contas Codex: o roteiro de captura escreve "conta", a rota escreve a frase.
fn account_msg(v: &Value) -> Value {
    let mut v = v.clone();
    if v["detail"]["code"].as_str().is_some_and(|c| c.starts_with("codex_account")) { v["detail"]["msg"] = Value::Null; }
    v
}

fn enc(text: &str) -> String { form_urlencoded::byte_serialize(text.as_bytes()).collect() }

fn sids(v: &Value) -> Vec<&str> { v.as_array().unwrap().iter().map(|e| &e["session_id"].as_str().unwrap()[..8]).collect() }

/// ARQ-01: as quatro rotas são só do dono; sem token o pedido vai ao Python, que recusa.
#[tokio::test]
async fn arq01_owner_only() {
    let h = Harness::new(Mode::NoIndex).await;
    for path in ["/api/archive", format!("/api/archive/{HANGAR}").as_str(), "/api/archive/recent", "/api/archive-por-cwd?cwd=/x"] {
        let response = h.client.get(h.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 401, "{path}");
        assert!(h.hits.lock().unwrap().iter().any(|hit| hit == path), "{path} não foi ao Python");
    }
    h.hits.lock().unwrap().clear();
    let response = h.client.get(h.url(&format!("/api/archive?token={OWNER}"))).send().await.unwrap();
    assert_eq!(response.status(), 200);
    h.assert_not_forwarded();
}

/// O esperado da lista de pastas: F8 tira `a5a5a5a5` da contagem e F9 tira a pasta que só tinha a
/// cópia velha de `db401d8f`.
fn folders_expected(mode: Mode) -> Value {
    let mut folders = capture(mode)["folders"].as_array().unwrap().clone();
    folders.retain(|f| f["project"] != MOVED_OLD_PROJECT);
    for f in folders.iter_mut().filter(|f| f["project"] == HANGAR) { f["count"] = json!(f["count"].as_u64().unwrap() - 1); }
    Value::Array(folders)
}

/// ARQ-02, 03, 04, 05, 06, 08, 09, 14, 16, F8, F9, M4: as pastas de todas as contas somadas, com os
/// outros agentes na pasta do Claude, mtime desc, com e sem índice.
#[tokio::test]
async fn arq02_folders_match_the_python() {
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::new(mode).await;
        let (status, body) = h.get("/api/archive").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, folders_expected(mode));
        h.assert_not_forwarded();
    }
}

/// ARQ-22..28, 36, 37, PRV-04, 06..08, 11..14, 17, F8, F9: cada pasta da captura, com prévia, última
/// mensagem, conta, viva, nome no fechamento e os outros agentes; erros de caminho e de pasta.
#[tokio::test]
async fn arq22_folder_conversations_match_the_python() {
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::new(mode).await;
        for (project, want) in capture(mode)["folder"].as_object().unwrap() {
            let (status, body) = h.get(&format!("/api/archive/{}", enc(project))).await;
            assert_eq!(status, want["status"].as_u64().unwrap() as u16, "{project}: {body}");
            let want = if want["body"].is_array() { accepted(&want["body"]) } else { want["body"].clone() };
            assert_eq!(body, want, "{project}");
        }
        h.assert_not_forwarded();
    }
}

/// ARQ-29, 30, PRV-15, 17: `codex_account` filtra o Codex antes do teto; conta desconhecida ou de
/// nome inválido é o erro da conta, com a frase do `_erro_conta_codex`.
#[tokio::test]
async fn arq29_folder_codex_account() {
    let h = Harness::new(Mode::Index).await;
    for (case, want) in capture(Mode::Index)["folder_codex_account"].as_object().unwrap() {
        let (project, account) = case.split_once('|').unwrap();
        let (status, body) = h.get(&format!("/api/archive/{project}?codex_account={}", enc(account))).await;
        assert_eq!(status, want["status"].as_u64().unwrap() as u16, "{case}: {body}");
        let want = if want["body"].is_array() { accepted(&want["body"]) } else { account_msg(&want["body"]) };
        assert_eq!(account_msg(&body), want, "{case}");
    }
    let (_, body) = h.get(&format!("/api/archive/{HANGAR}?codex_account=zz")).await;
    assert_eq!(body["detail"]["msg"], "conta Codex não encontrada");
    let (_, body) = h.get(&format!("/api/archive/{HANGAR}?codex_account=BAD%20NAME")).await;
    assert_eq!(body["detail"]["msg"], "nome de conta Codex inválido");
    h.assert_not_forwarded();
}

/// ARQ-34, 35, F7, F8, F9: as mais novas de tudo, `cap` limitado a 1..100; a reserva sem usuário
/// (a mais nova de todas) não toma a vaga de `cap=1`.
#[tokio::test]
async fn arq34_recent() {
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::new(mode).await;
        let all = accepted(&capture(mode)["recent"]["500"]);
        let first = |n: usize| Value::Array(all.as_array().unwrap()[..n].to_vec());
        for (query, want) in [("?cap=1", first(1)), ("?cap=3", first(3)), ("", all.clone()), ("?cap=0", first(1)),
            ("?cap=500", all.clone()), ("?cap=-4", first(1))] {
            let (status, body) = h.get(&format!("/api/archive/recent{query}")).await;
            assert_eq!(status, 200, "{query}: {body}");
            assert_eq!(body, want, "{query}");
        }
        assert_eq!(sids(&first(3)), ["0cc60e6c", "75fea57d", "db401d8f"]);
        assert_eq!(capture(mode)["recent"]["40"], capture(mode)["recent"]["500"]);
        let (status, body) = h.get("/api/archive/recent?cap=abc").await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["detail"][0]["loc"], json!(["query", "cap"]));
        h.assert_not_forwarded();
    }
}

/// ARQ-05, 07, 31, 32, 33, F8, F9: as conversas de uma pasta pelo cwd, do agente e da conta pedidos;
/// pasta sem conversa é `[]`; conta e agente inválidos são 400.
#[tokio::test]
async fn arq31_by_cwd() {
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::new(mode).await;
        let (a, b) = (h.dir(".claude-a"), h.dir(".claude-b"));
        let hangar = "/home/jefferson/Projetos/hangar";
        let fixture = "/tmp/hangar-native-task4-fixture-jf";
        for (case, want) in capture(mode)["por_cwd"].as_object().unwrap() {
            let query = match case.as_str() {
                "hangar|claude|a" => format!("cwd={hangar}&config_dir={}", enc(&a)),
                "hangar|claude|b" => format!("cwd={hangar}&config_dir={}", enc(&b)),
                "hangar|claude|bad_dir" => format!("cwd={hangar}&config_dir=/nao/conta"),
                "hangar|bad_provider" => format!("cwd={hangar}&provider=gemini"),
                "hangar|claude|cap1" => format!("cwd={hangar}&cap=1"),
                "hangar|codex|b" => format!("cwd={hangar}&provider=codex&codex_account=b"),
                "fixture-jf|codex|b" => format!("cwd={fixture}&provider=codex&codex_account=b"),
                "fixture-jf|codex|default" => format!("cwd={fixture}&provider=codex&codex_account=default"),
                "fixture-jf|codex|zz" => format!("cwd={fixture}&provider=codex&codex_account=zz"),
                other => {
                    let (cwd, provider) = other.rsplit_once('|').unwrap();
                    format!("cwd={}&provider={provider}", enc(cwd))
                }
            };
            let (status, body) = h.get(&format!("/api/archive-por-cwd?{query}")).await;
            assert_eq!(status, want["status"].as_u64().unwrap() as u16, "{case}: {body}");
            let want = if want["body"].is_array() { accepted(&want["body"]) } else { account_msg(&want["body"]) };
            assert_eq!(account_msg(&body), want, "{case}");
        }
        let (status, body) = h.get("/api/archive-por-cwd").await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["detail"][0]["loc"], json!(["query", "cwd"]));
        h.assert_not_forwarded();
    }
}

/// M4: com o índice pronto, a lista não abre o transcript: o conteúdo trocado no disco, com o mesmo
/// tamanho e o mesmo mtime, não muda prévia nem última mensagem.
#[tokio::test]
async fn m4_index_listing_does_not_read_the_transcript() {
    let h = Harness::new(Mode::Index).await;
    let path = h.home.join(MTIMES[11]);
    let (size, modified) = { let md = std::fs::metadata(&path).unwrap(); (md.len(), md.modified().unwrap()) };
    std::fs::write(&path, vec![b'x'; size as usize]).unwrap();
    set_mtime(&path, modified);
    let (status, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, accepted(&capture(Mode::Index)["folder"][HANGAR]["body"]));
}

/// ARQ-15: índice ilegível cai no disco; a resposta é a mesma.
#[tokio::test]
async fn arq15_unreadable_index_falls_back_to_disk() {
    let h = Harness::new(Mode::NoIndex).await;
    std::fs::create_dir_all(h.search.path().parent().unwrap()).unwrap();
    std::fs::write(h.search.path(), b"isto nao e sqlite").unwrap();
    let (status, body) = h.get("/api/archive").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, folders_expected(Mode::NoIndex));
    let (_, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(body, accepted(&capture(Mode::NoIndex)["folder"][HANGAR]["body"]));
}

/// ARQ-10, 11: arquivo parado há mais de 15 min conta como conversa (a reserva velha aparece, como no
/// Python), com e sem índice; a de agora há pouco, sem linha `user` nem prévia, não.
#[tokio::test]
async fn arq10_old_reserve() {
    let old = |home: &Path| set_mtime(&home.join(RESERVE), SystemTime::now() - Duration::from_secs(20 * 60));
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, old).await;
        let (_, body) = h.get("/api/archive/-home-jefferson").await;
        assert_eq!(sids(&body), ["f2afaa93", "42a7c450"]);
        let h = Harness::new(mode).await;
        let (_, body) = h.get("/api/archive/-home-jefferson").await;
        assert_eq!(sids(&body), ["42a7c450"]);
    }
}

/// F12: com o índice, a última mensagem real vale mesmo antes do último 1 MB do arquivo.
#[tokio::test]
async fn f12_index_keeps_the_last_message_beyond_one_megabyte() {
    let big = |home: &Path| {
        let path = home.join(MTIMES[6]);
        let mut raw = std::fs::read(&path).unwrap();
        for _ in 0..3 {
            raw.extend(json!({"type": "progress", "data": "x".repeat(600_000)}).to_string().bytes());
            raw.push(b'\n');
        }
        std::fs::write(&path, raw).unwrap();
        set_mtime(&path, UNIX_EPOCH + Duration::from_secs(T0 + 700));
    };
    let want = capture(Mode::Index)["folder"]["-home-jefferson"]["body"][0]["ultima"].clone();
    assert_ne!(want, "");
    let h = Harness::with(Mode::Index, big).await;
    let (_, body) = h.get("/api/archive/-home-jefferson").await;
    assert_eq!(body[0]["ultima"], want);
    let h = Harness::with(Mode::NoIndex, big).await;
    let (_, body) = h.get("/api/archive/-home-jefferson").await;
    assert_eq!(body[0]["ultima"], "", "sem o índice o fim lido é o do Python");
}

/// PRV-03: agente que falha sai da lista sem derrubar os outros (Codex com rollout em duas contas).
#[tokio::test]
async fn prv03_broken_agent_does_not_drop_the_others() {
    let broken = |home: &Path| {
        std::fs::remove_dir_all(home.join(".codex-b/sessions")).unwrap();
        std::os::unix::fs::symlink(home.join(".codex/sessions"), home.join(".codex-b/sessions")).unwrap();
    };
    let h = Harness::with(Mode::Index, broken).await;
    let (status, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(sids(&body), ["75fea57d", "db401d8f", "session_"]);
    let (_, body) = h.get("/api/archive-por-cwd?cwd=/home/jefferson/Projetos/hangar/.worktrees/contas-modelos&provider=pi").await;
    assert_eq!(sids(&body), ["d337d6b8"]);
}

/// PRV-12: sessão Kimi sem `wire.jsonl` fica fora.
#[tokio::test]
async fn prv12_kimi_without_wire_is_left_out() {
    let h = Harness::with(Mode::Index, |home: &Path| std::fs::remove_file(home.join(MTIMES[3])).unwrap()).await;
    let (_, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(sids(&body), ["75fea57d", "db401d8f", "01a05511"]);
}

/// PRV-07: subagente do Pi e do omp (`<stem>/…`) não é conversa.
#[tokio::test]
async fn prv07_pi_subagents_are_left_out() {
    let sub = |home: &Path| {
        let main = home.join(MTIMES[1]);
        let nested = main.with_extension("").join("task/run-1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::copy(&main, nested.join("session.jsonl")).unwrap();
    };
    let h = Harness::with(Mode::Index, sub).await;
    let (_, body) = h.get("/api/archive/recent").await;
    assert_eq!(body.as_array().unwrap().iter().filter(|e| e["provider"] == "pi").count(), 1);
}

/// ARQ-27: `closed-sessions.json` corrompido não derruba a lista; fica sem nome.
#[tokio::test]
async fn arq27_corrupt_closed_names() {
    let h = Harness::with(Mode::Index, |home: &Path| std::fs::write(home.join(".claude-a/closed-sessions.json"), "{").unwrap()).await;
    let (status, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.as_array().unwrap().iter().all(|e| e["name"].is_null()), "{body}");
}

fn sha(bytes: &[u8]) -> String { ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect() }

fn line(row: &Value) -> String { format!("{row}\n") }

/// Um rollout que veio de uma transferência Claude→Codex sem turno novo: o fim cru é a mensagem
/// importada ("ok"), o da composição é a última da origem ("antigo").
fn transferred(home: &Path, phase: &str) -> PathBuf {
    let base = home.join("transfer");
    std::fs::create_dir_all(&base).unwrap();
    let rows = [
        json!({"type": "user", "uuid": "u1", "parentUuid": null, "timestamp": "2026-01-01T00:00:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "ok"}]}}),
        json!({"type": "assistant", "uuid": "after", "parentUuid": "u1", "timestamp": "2026-01-01T00:03:00Z",
            "message": {"role": "assistant", "content": "antigo"}}),
    ];
    let snapshot = base.join("snapshot.jsonl");
    let snapshot_bytes: String = rows.iter().map(line).collect();
    std::fs::write(&snapshot, &snapshot_bytes).unwrap();
    let message = json!({"timestamp": "2026-02-01T00:00:00Z", "type": "response_item",
        "payload": {"type": "message", "id": "imported", "role": "user", "content": [{"type": "input_text", "text": "ok"}]}});
    let prefix = line(&json!({"type": "session_meta", "payload": {"id": THREAD, "cwd": base}})) + &line(&message);
    let codex_home = home.join(".codex");
    let rollout = codex_home.join("sessions/2026/02/02").join(format!("rollout-fixture-{THREAD}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, &prefix).unwrap();
    let record = json!({"id": TRANSFER, "name": "s", "source_life": "k:original", "phase": phase,
        "source": {"path": snapshot, "provider": "claude", "digest": sha(snapshot_bytes.as_bytes()), "selected_uuids": ["u1", "after"]},
        "origin_meta": {"provider": "claude", "jsonl": base.join("original.jsonl"), "cwd": base, "key": "original"},
        "destination_meta": {"provider": "codex", "thread_id": THREAD, "codex_home": codex_home, "lifecycle_id": "k:original", "transfer_id": TRANSFER},
        "boundary": {"thread_id": THREAD, "rollout_path": rollout, "min_offset": prefix.len(), "imported_item_ids": ["imported"], "prefix_digest": sha(prefix.as_bytes())},
        "error_code": null});
    let store = home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(store.join("indexes")).unwrap();
    std::fs::write(store.join(format!("{TRANSFER}.json")), record.to_string()).unwrap();
    let key = format!("[{}, {}]", serde_json::to_string(&codex_home.canonicalize().unwrap()).unwrap(), serde_json::to_string(THREAD).unwrap());
    std::fs::write(store.join("indexes").join(format!("thread-{}.json", sha(key.as_bytes()))), json!([TRANSFER]).to_string()).unwrap();
    base
}

/// ARQ-21: a última mensagem do Codex transferido sai do histórico composto.
#[tokio::test]
async fn arq21_transferred_codex_last_message_is_the_composition() {
    let dir = std::sync::OnceLock::new();
    let h = Harness::with(Mode::Index, |home: &Path| { dir.set(transferred(home, "complete")).unwrap(); }).await;
    let cwd = dir.get().unwrap().to_string_lossy().into_owned();
    let (status, body) = h.get(&format!("/api/archive-por-cwd?cwd={}&provider=codex", enc(&cwd))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(sids(&body), [&THREAD[..8]]);
    assert_eq!(body[0]["ultima"], "antigo");
}

/// PRV-16: transferência em andamento esconde o rollout.
#[tokio::test]
async fn prv16_transfer_in_progress_hides_the_rollout() {
    let dir = std::sync::OnceLock::new();
    let h = Harness::with(Mode::Index, |home: &Path| { dir.set(transferred(home, "imported")).unwrap(); }).await;
    let cwd = dir.get().unwrap().to_string_lossy().into_owned();
    let (status, body) = h.get(&format!("/api/archive-por-cwd?cwd={}&provider=codex", enc(&cwd))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!([]));
}
