//! R6+R7: o Arquivo (`GET /api/archive`, `/api/archive/{project}`, `/api/archive/recent` e
//! `/api/archive-por-cwd`) no Rust, comparado com a captura do Python
//! (`tests/fixtures/search/contract/archive/archive_{index,noindex}.json`) sobre o HOME de
//! `tests/fixtures/search/home/`, com o índice pronto e sem ele. O esperado é a captura com as
//! diferenças aceitas aplicadas: F7 (reserva sem usuário não ocupa vaga), F8 (conversa interna fora,
//! só a que o índice já marcou) e F9 (conversa que mudou de pasta só na pasta do caminho mais novo).
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
    /// O diário do índice: (código, detalhe).
    reports: Arc<Mutex<Vec<(&'static str, String)>>>,
    list: Arc<hangar_server::list::bridge::ListBridge>,
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
    async fn with(mode: Mode, edit: impl FnOnce(&Path)) -> Self { Self::build(mode, edit, &[]).await }

    /// `extra`: variáveis a mais no ambiente das contas, com `{HOME}` trocado pelo HOME do teste.
    async fn build(mode: Mode, edit: impl FnOnce(&Path), extra: &[(&str, &str)]) -> Self {
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
        let extra = extra.iter().map(|(k, v)| (*k, v.replace("{HOME}", &home.to_string_lossy())));
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
            env.into_iter().chain(extra).map(|(k, v)| (k.to_owned(), v)).collect()));
        // O índice numa pasta própria, com as duas contas da captura.
        let dirs = vec![claude.to_string_lossy().into_owned(), home.join(".claude-b").to_string_lossy().into_owned()];
        let server_dir = claude.clone();
        let accounts: hangar_server::search::AccountSource = Arc::new(move || Ok(hangar_server::search::server_accounts(&dirs, &server_dir)));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let diary = reports.clone();
        let search = hangar_server::search::Index::new(&dir.path().join("idx"), accounts,
            Arc::new(move |code, detail| diary.lock().unwrap().push((code, detail))), Default::default())
            .with_agents(state.accounts.env.clone());
        if mode == Mode::Index { search.update().unwrap(); }
        state.search = search.clone();
        let list = Arc::new(live_list(dir.path(), &home, upstream));
        state.list = list.clone();
        let app = router(Arc::new(state));
        let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        Self { _dir: dir, home, address, hits, client: reqwest::Client::new(), search, reports, list, tasks: vec![up_task, server] }
    }

    fn reported(&self, code: &str) -> usize { self.reports.lock().unwrap().iter().filter(|(c, _)| *c == code).count() }

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

/// F8 (só com o índice: fora dele a conversa interna entra, como no Python) e F9 numa lista de
/// conversas da captura.
fn accepted(entries: &Value, mode: Mode) -> Value {
    Value::Array(entries.as_array().unwrap().iter().filter(|e| {
        !(mode == Mode::Index && e["session_id"] == INTERNAL) && !(e["session_id"] == MOVED && e["project"] == MOVED_OLD_PROJECT)
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

/// O esperado da lista de pastas: F8 (com o índice) tira `a5a5a5a5` da contagem e F9 tira a pasta
/// que só tinha a cópia velha de `db401d8f`.
fn folders_expected(mode: Mode) -> Value {
    let mut folders = capture(mode)["folders"].as_array().unwrap().clone();
    folders.retain(|f| f["project"] != MOVED_OLD_PROJECT);
    let internal = u64::from(mode == Mode::Index);
    for f in folders.iter_mut().filter(|f| f["project"] == HANGAR) { f["count"] = json!(f["count"].as_u64().unwrap() - internal); }
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
            let want = if want["body"].is_array() { accepted(&want["body"], mode) } else { want["body"].clone() };
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
        let want = if want["body"].is_array() { accepted(&want["body"], Mode::Index) } else { account_msg(&want["body"]) };
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
        let all = accepted(&capture(mode)["recent"]["500"], mode);
        let first = |n: usize| Value::Array(all.as_array().unwrap()[..n].to_vec());
        for (query, want) in [("?cap=1", first(1)), ("?cap=3", first(3)), ("", all.clone()), ("?cap=0", first(1)),
            ("?cap=500", all.clone()), ("?cap=-4", first(1))] {
            let (status, body) = h.get(&format!("/api/archive/recent{query}")).await;
            assert_eq!(status, 200, "{query}: {body}");
            assert_eq!(body, want, "{query}");
        }
        let third = if mode == Mode::Index { "db401d8f" } else { "a5a5a5a5" };
        assert_eq!(sids(&first(3)), ["0cc60e6c", "75fea57d", third]);
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
            let want = if want["body"].is_array() { accepted(&want["body"], mode) } else { account_msg(&want["body"]) };
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
    assert_eq!(body, accepted(&capture(Mode::Index)["folder"][HANGAR]["body"], Mode::Index));
}

/// F8 sem custo de leitura: com o índice não pronto, a lista de pastas abre só os transcripts que o
/// Python abria (o cabeçalho do mais novo de cada pasta por conta e a varredura da reserva recente).
/// A cópia de `db401d8f` em `-home-jefferson-Projetos-hangar` não é a mais nova da pasta: não abre.
// Só o inotify avisa abertura de arquivo; nos outros sistemas o contador ficaria sempre vazio.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn f8_folders_without_index_open_only_what_the_python_opened() {
    let h = Harness::new(Mode::NoIndex).await;
    let opened = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let seen = opened.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event && matches!(event.kind, notify::EventKind::Access(notify::event::AccessKind::Open(_))) {
            seen.lock().unwrap().extend(event.paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "jsonl")));
        }
    }).unwrap();
    for projects in [".claude-a/projects", ".claude-b/projects"] {
        notify::Watcher::watch(&mut watcher, &h.home.join(projects), notify::RecursiveMode::Recursive).unwrap();
    }
    let (status, body) = h.get("/api/archive").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, folders_expected(Mode::NoIndex));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let python: std::collections::HashSet<PathBuf> = [MTIMES[13], MTIMES[11], RESERVE, MTIMES[6], MTIMES[5], MTIMES[10], MTIMES[7]]
        .iter().map(|rel| h.home.join(rel)).collect();
    let opened = opened.lock().unwrap().clone();
    assert!(!opened.is_empty(), "o observador não viu nenhuma abertura");
    let extra: Vec<_> = opened.difference(&python).collect();
    assert!(extra.is_empty(), "abriu além do Python: {extra:?}");
}

/// Cada abertura de `.jsonl`/`.json` dentro de `roots` enquanto `during` roda (o inotify avisa cada uma).
#[cfg(target_os = "linux")]
async fn opened_during<F: std::future::Future<Output = ()>>(roots: &[PathBuf], during: F) -> Vec<PathBuf> {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event && matches!(event.kind, notify::EventKind::Access(notify::event::AccessKind::Open(_))) {
            seen.lock().unwrap().extend(event.paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "jsonl" || e == "json")));
        }
    }).unwrap();
    for root in roots { notify::Watcher::watch(&mut watcher, root, notify::RecursiveMode::Recursive).unwrap(); }
    during.await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(watcher);
    opened.lock().unwrap().clone()
}

/// M4 nos outros agentes: com o índice pronto, pastas, conversas e recentes não abrem o transcript de
/// Codex, Pi, omp nem Kimi (cabeçalho e última mensagem vêm do índice); fora do índice, o disco.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn m4_agents_listing_with_the_index_opens_no_agent_transcript() {
    let agents: Vec<PathBuf> = [".codex/sessions", ".codex-b/sessions", ".pi/agent/sessions", ".omp/agent/sessions", ".kimi-code/sessions"]
        .iter().map(PathBuf::from).collect();
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::new(mode).await;
        let roots: Vec<PathBuf> = agents.iter().map(|r| h.home.join(r)).collect();
        let opened = opened_during(&roots, async {
            for path in ["/api/archive".to_owned(), format!("/api/archive/{HANGAR}"), "/api/archive/recent".to_owned()] {
                let (status, body) = h.get(&path).await;
                assert_eq!(status, 200, "{path}: {body}");
            }
        }).await;
        let transcripts: Vec<PathBuf> = MTIMES[..5].iter().map(|rel| h.home.join(rel)).collect();
        let read: Vec<&PathBuf> = transcripts.iter().filter(|p| opened.contains(*p)).collect();
        match mode {
            Mode::Index => assert!(read.is_empty(), "abriu transcript de agente com o índice pronto: {read:?}"),
            Mode::NoIndex => assert_eq!(read.len(), 5, "sem índice o disco é lido: {opened:?}"),
        }
        h.assert_not_forwarded();
    }
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
    assert_eq!(body, accepted(&capture(Mode::NoIndex)["folder"][HANGAR]["body"], Mode::NoIndex));
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

/// Um rollout com o registro da transferência ilegível sai sozinho, com uma linha no diário; os outros
/// rollouts Codex continuam no Arquivo.
#[tokio::test]
async fn codex_rollout_with_an_unreadable_transfer_record_is_skipped_and_the_rest_listed() {
    let h = Harness::with(Mode::NoIndex, |home: &Path| {
        transferred(home, "complete");
        for entry in std::fs::read_dir(home.join(".hangar/conversation-transfers/indexes")).unwrap().flatten() {
            std::fs::write(entry.path(), "{").unwrap();
        }
    }).await;
    for _ in 0..2 {
        let (status, body) = h.get(&format!("/api/archive-por-cwd?cwd={}&provider=codex", enc("/home/jefferson/Projetos/hangar"))).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(sids(&body), ["01a05511"]);
    }
    assert_eq!(h.reported("arquivo.conversa_ilegivel"), 1, "uma linha por rollout, não por pedido");
}

/// Pasta de projeto sem permissão sai da lista com uma linha no diário; o resto continua.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_project_folder_is_skipped_with_one_diary_line() {
    use std::os::unix::fs::PermissionsExt;
    let locked = std::sync::OnceLock::new();
    let h = Harness::with(Mode::NoIndex, |home: &Path| {
        let dir = home.join(".claude-b/projects/-tmp-cx-probe-work");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        locked.set(dir).unwrap();
    }).await;
    let dir = locked.get().unwrap();
    let readable = std::fs::read_dir(dir).is_ok(); // root lê mesmo assim
    let (status, body) = h.get("/api/archive").await;
    let (_, again) = h.get("/api/archive").await;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable { return; }
    assert_eq!(status, 200, "{body}");
    let projects: Vec<&str> = body.as_array().unwrap().iter().map(|f| f["project"].as_str().unwrap()).collect();
    assert!(projects.contains(&HANGAR) && !projects.contains(&"-tmp-cx-probe-work"), "{projects:?}");
    assert_eq!(body, again);
    assert_eq!(h.reported("arquivo.pasta_ilegivel"), 1, "uma linha por pasta, não por pedido");
}

fn at(secs: u64) -> SystemTime { UNIX_EPOCH + Duration::from_secs(secs) }

fn write_lines(path: &Path, lines: &[Value], mtime: SystemTime) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, lines.iter().map(line).collect::<String>()).unwrap();
    set_mtime(path, mtime);
}

fn user(uuid: &str, text: &str, cwd: &str) -> Value {
    json!({"type": "user", "uuid": uuid, "cwd": cwd, "message": {"role": "user", "content": text}})
}

/// O que o Claude Code grava antes da 1ª mensagem: só `attachment`, com `"user"` citado no texto.
fn attachments(cwd: &str) -> Vec<Value> {
    (0..3).map(|i| json!({"parentUuid": null, "isSidechain": false, "type": "attachment", "uuid": format!("a{i}"), "cwd": cwd,
        "timestamp": "2026-01-01T00:00:00Z", "attachment": {"type": "hook_success", "hookName": "SessionStart:startup",
        "content": "contexto do hook: \"user\" e \"type\""}})).collect()
}

const HANGAR_CWD: &str = "/home/jefferson/Projetos/hangar";

/// ARQ-09, 11 (`test_linha_de_usuario_sem_previa_continua_listada`, `test_pasta_so_com_reserva_nao_aparece`):
/// linha `user` com `isMeta`, sem prévia, mantém a conversa; `"user"` dentro de `attachment` não conta,
/// e a pasta só com essa reserva não aparece.
#[tokio::test]
async fn arq11_meta_user_line_lists_and_a_reserve_only_folder_does_not() {
    fn edit(home: &Path) {
        let projects = home.join(".claude-a/projects");
        let recent = SystemTime::now() - Duration::from_secs(60);
        write_lines(&projects.join("-home-u-proj/55555555-5555-4555-8555-555555555555.jsonl"), &attachments("/home/u/proj"), recent);
        let mut lines = attachments("/home/u/meta");
        lines.push(json!({"type": "user", "uuid": "m1", "isMeta": true, "cwd": "/home/u/meta",
            "message": {"role": "user", "content": "Caveat: meta"}}));
        write_lines(&projects.join("-home-u-meta/66666666-6666-4666-8666-666666666666.jsonl"), &lines, recent);
    }
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, edit).await;
        let (_, folders) = h.get("/api/archive").await;
        let count = |p: &str| folders.as_array().unwrap().iter().find(|f| f["project"] == p).map(|f| f["count"].clone());
        assert_eq!((count("-home-u-meta"), count("-home-u-proj")), (Some(json!(1)), None));
        assert_eq!(h.get("/api/archive/-home-u-proj").await, (200, json!([])));
        let (_, body) = h.get("/api/archive/-home-u-meta").await;
        assert_eq!(sids(&body), ["66666666"]);
    }
}

/// ARQ-13: transcript que o stat não alcança (link quebrado) sai; conta com `projects` ilegível sai com
/// uma linha no diário e as outras seguem.
#[cfg(unix)]
#[tokio::test]
async fn arq13_dangling_transcript_and_unreadable_account_are_skipped() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, |home: &Path| std::os::unix::fs::symlink("/nao/existe.jsonl",
            home.join(".claude-a/projects").join(HANGAR).join("77777777-7777-4777-8777-777777777777.jsonl")).unwrap()).await;
        let (status, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
        assert_eq!(status, 200, "{body}");
        assert!(!sids(&body).contains(&"77777777"), "{body}");
    }
    let h = Harness::new(Mode::NoIndex).await;
    let projects = h.home.join(".claude-b/projects");
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&projects).is_ok(); // root lê mesmo assim
    let (status, body) = h.get("/api/archive").await;
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable { return; }
    assert_eq!(status, 200, "{body}");
    let folders: Vec<&str> = body.as_array().unwrap().iter().map(|f| f["project"].as_str().unwrap()).collect();
    assert!(folders.contains(&HANGAR) && !folders.contains(&"-tmp-cx-probe-work"), "{folders:?}");
    assert_eq!(h.reported("arquivo.pasta_ilegivel"), 1);
}

/// ARQ-17: o cabeçalho é lido nas 60 primeiras linhas (prévia e cwd depois delas ficam de fora, com e sem
/// índice); arquivo que não abre sai com prévia e cwd vazios.
#[tokio::test]
async fn arq17_header_is_read_in_60_lines_and_an_unreadable_file_is_empty() {
    fn edit(home: &Path) {
        let mut lines: Vec<Value> = (0..60).map(|i| json!({"type": "progress", "uuid": format!("p{i}")})).collect();
        lines.push(user("u1", "tarde demais", "/home/u/longo"));
        write_lines(&home.join(".claude-a/projects/-home-u-longo/88888888-8888-4888-8888-888888888888.jsonl"), &lines, at(T0 + 5));
        write_lines(&home.join(".claude-a/projects/-home-u-fechado/99999999-9999-4999-8999-999999999999.jsonl"),
            &[user("u1", "segredo", "/home/u/fechado")], at(T0 + 6));
    }
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, edit).await;
        let (_, body) = h.get("/api/archive/-home-u-longo").await;
        assert_eq!((sids(&body), &body[0]["preview"], &body[0]["cwd"], &body[0]["ultima"]),
            (vec!["88888888"], &json!(""), &Value::Null, &json!("tarde demais")), "{body}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let h = Harness::with(Mode::NoIndex, edit).await;
        let path = h.home.join(".claude-a/projects/-home-u-fechado/99999999-9999-4999-8999-999999999999.jsonl");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::File::open(&path).is_ok();
        let (status, body) = h.get("/api/archive/-home-u-fechado").await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if readable { return; }
        assert_eq!(status, 200, "{body}");
        assert_eq!((sids(&body), &body[0]["preview"], &body[0]["cwd"], &body[0]["ultima"]),
            (vec!["99999999"], &json!(""), &Value::Null, &json!("")), "{body}");
    }
}

/// ARQ-24, 34 (`test_teto_corta_as_mais_VELHAS_dos_outros_providers`): o teto 100 vale sobre o total de
/// todos os agentes, mais novas primeiro; corta o Codex mais velho e mantém o Kimi mais novo. Os
/// recentes também param em 100.
#[tokio::test]
async fn arq24_cap_100_cuts_the_oldest_of_any_agent() {
    fn edit(home: &Path) {
        for i in 0..100u64 {
            let sid = format!("{:08x}-0000-4000-8000-{i:012x}", 0xc000_0000u64 + i);
            write_lines(&home.join(".claude-a/projects").join(HANGAR).join(format!("{sid}.jsonl")),
                &[user("u1", &format!("conversa {i}"), HANGAR_CWD)], at(T0 + 150 + i));
        }
    }
    for (mode, oldest) in [(Mode::Index, T0 + 153), (Mode::NoIndex, T0 + 154)] {
        let h = Harness::with(mode, edit).await;
        let (_, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
        let list = body.as_array().unwrap();
        assert_eq!(list.len(), 100);
        assert!(!sids(&body).contains(&"01a05511"), "o Codex mais velho fica fora");
        assert!(sids(&body).contains(&"session_"), "o Kimi mais novo fica");
        assert_eq!(list.last().unwrap()["mtime"].as_f64(), Some(oldest as f64));
        let (_, recent) = h.get("/api/archive/recent?cap=500").await;
        assert_eq!(recent.as_array().unwrap().len(), 100);
    }
}

/// ARQ-26: `live` pelo caminho real: transcript que é link simbólico continua vivo quando a lista dá
/// o caminho do arquivo de verdade.
#[cfg(unix)]
#[tokio::test]
async fn arq26_symlinked_transcript_is_live_by_its_real_path() {
    let real = |home: &Path| home.join("real/0cc60e6c-eafc-4915-9936-f0c7b81191d9.jsonl");
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, |home: &Path| {
            let path = home.join(MTIMES[13]);
            std::fs::create_dir_all(home.join("real")).unwrap();
            std::fs::rename(&path, real(home)).unwrap();
            std::os::unix::fs::symlink(real(home), &path).unwrap();
        }).await;
        h.list.seed("sessao-viva", &real(&h.home).to_string_lossy());
        let (_, body) = h.get("/api/archive/-home-jefferson-Projetos-hangar-computer-control-9c0b634f-t1").await;
        let entry = body.as_array().unwrap().iter().find(|e| e["session_id"] == "0cc60e6c-eafc-4915-9936-f0c7b81191d9").unwrap();
        assert_eq!(entry["live"], true, "{body}");
    }
}

/// ARQ-27 (`test_list_recent_le_os_nomes_uma_vez`): os nomes das sessões fechadas são lidos uma vez por
/// pedido.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn arq27_closed_names_are_read_once_per_request() {
    let h = Harness::new(Mode::NoIndex).await;
    let names = h.home.join(".claude-a/closed-sessions.json");
    for path in ["/api/archive/recent".to_owned(), format!("/api/archive/{HANGAR}")] {
        let opened = opened_during(&[h.home.join(".claude-a")], async { assert_eq!(h.get(&path).await.0, 200); }).await;
        assert_eq!(opened.iter().filter(|p| **p == names).count(), 1, "{path}");
    }
}

/// ARQ-29, 32, D11 (`test_archive_codex_filtra_a_conta_antes_do_limite`,
/// `test_archive_por_cwd_filtra_provider_antes_do_limite`): conta e agente filtram antes do teto; teto
/// negativo é 0; marcador de conta Codex inválido é 409.
#[tokio::test]
async fn arq29_account_and_agent_filter_before_the_cap() {
    let h = Harness::with(Mode::Index, |home: &Path| {
        let to = home.join(".codex-b/sessions/2026/10/01/rollout-2026-10-01T10-00-00-0b0b0b0b-1111-4222-8333-444444444444.jsonl");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(home.join(MTIMES[0]), &to).unwrap();
        set_mtime(&to, at(T0 + 2000));
    }).await;
    let by_cwd = |query: &str| format!("/api/archive-por-cwd?cwd={}&{query}", enc(HANGAR_CWD));
    for (query, want) in [("provider=codex&cap=1", vec!["0b0b0b0b"]), ("provider=codex&codex_account=default&cap=1", vec!["01a05511"]),
        ("provider=kimi&cap=1", vec!["session_"]), ("cap=-3", vec![])] {
        let (status, body) = h.get(&by_cwd(query)).await;
        assert_eq!(status, 200, "{query}: {body}");
        assert_eq!(sids(&body), want, "{query}");
    }
    std::fs::write(h.home.join(".codex-b/.hangar-codex-conta"), r#"{"version": 1, "id": "outra"}"#).unwrap();
    let (status, body) = h.get(&format!("/api/archive/{HANGAR}?codex_account=b")).await;
    assert_eq!((status, &body["detail"]["code"]), (409, &json!("codex_account_invalid_marker")), "{body}");
}

/// ARQ-33: pedido só do Claude não lê rollout; o do Codex lê (sem índice).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn arq33_claude_only_request_reads_no_rollout() {
    let h = Harness::new(Mode::NoIndex).await;
    let roots = [h.home.join(".codex/sessions"), h.home.join(".codex-b/sessions")];
    let rollout = h.home.join(MTIMES[0]);
    let path = |provider: &str| format!("/api/archive-por-cwd?cwd={}&provider={provider}", enc(HANGAR_CWD));
    let claude = opened_during(&roots, async { assert_eq!(h.get(&path("claude")).await.0, 200); }).await;
    assert!(claude.is_empty(), "{claude:?}");
    let codex = opened_during(&roots, async { assert_eq!(h.get(&path("codex")).await.0, 200); }).await;
    assert!(codex.contains(&rollout), "{codex:?}");
}

/// ARQ-35 (`test_list_recent_cap_1_le_so_a_pasta_mais_nova`): `cap=1` sem índice abre só o que vem antes
/// da primeira conversa: a reserva recente (sem usuário) e a mais nova.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn arq35_recent_cap_1_without_index_opens_only_the_newest() {
    let h = Harness::new(Mode::NoIndex).await;
    let roots = [h.home.join(".claude-a/projects"), h.home.join(".claude-b/projects")];
    let opened = opened_during(&roots, async {
        let (_, body) = h.get("/api/archive/recent?cap=1").await;
        assert_eq!(sids(&body), ["0cc60e6c"]);
    }).await;
    let opened: std::collections::HashSet<PathBuf> = opened.into_iter().collect();
    assert_eq!(opened, [RESERVE, MTIMES[13]].iter().map(|rel| h.home.join(rel)).collect());
}

/// ARQ-06: a conta do processo fora do catálogo entra por último, sem pasta de conta nem rótulo.
#[tokio::test]
async fn arq06_server_account_outside_the_catalog_has_no_label() {
    let h = Harness::build(Mode::NoIndex, |_| {}, &[("CP_CLAUDE_CONFIG_DIRS", "B:{HOME}/.claude-b")]).await;
    let (status, body) = h.get(&format!("/api/archive/{HANGAR}")).await;
    assert_eq!(status, 200, "{body}");
    let entry = |sid: &str| body.as_array().unwrap().iter().find(|e| e["session_id"].as_str().unwrap().starts_with(sid)).unwrap().clone();
    let (a, b) = (entry("75fea57d"), entry("db401d8f"));
    assert_eq!((&a["config_dir"], &a["conta"]), (&Value::Null, &json!("")));
    assert_eq!((&b["config_dir"], &b["conta"]), (&json!("{HOME}/.claude-b"), &json!("B")));
}

fn pi_lines(junk: usize, cwd: &str, sid: &str) -> Vec<Value> {
    let mut lines: Vec<Value> = (0..junk).map(|i| json!({"type": "thinking_level_change", "id": format!("t{i}")})).collect();
    lines.push(json!({"type": "session", "version": 3, "id": sid, "timestamp": "2026-10-01T10:00:00.000Z", "cwd": cwd}));
    lines.push(json!({"type": "message", "id": "m1", "parentId": null, "timestamp": "2026-10-01T10:00:01.000Z",
        "message": {"role": "user", "content": [{"type": "text", "text": "oi"}], "timestamp": 1_790_000_000_000u64}}));
    lines
}

/// PRV-04: o cwd do Pi vem das 5 primeiras linhas (depois delas a conversa sai); arquivo que não abre sai.
#[tokio::test]
async fn prv04_agent_cwd_comes_from_the_first_5_lines() {
    const SIX: &str = "aaaaaaaa-0000-4000-8000-000000000006";
    const FIVE: &str = "aaaaaaaa-0000-4000-8000-000000000005";
    fn edit(home: &Path) {
        let root = home.join(".pi/agent/sessions");
        write_lines(&root.join(format!("--repo-seis--/2026-10-01T10-00-00-000Z_{SIX}.jsonl")), &pi_lines(5, "/repo-seis", SIX), at(T0 + 7));
        write_lines(&root.join(format!("--repo-cinco--/2026-10-01T10-00-00-000Z_{FIVE}.jsonl")), &pi_lines(4, "/repo-cinco", FIVE), at(T0 + 8));
    }
    let pi = |cwd: &str| format!("/api/archive-por-cwd?cwd={cwd}&provider=pi");
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, edit).await;
        assert_eq!(h.get(&pi("/repo-seis")).await, (200, json!([])));
        let (_, body) = h.get(&pi("/repo-cinco")).await;
        assert_eq!(sids(&body), ["aaaaaaaa"]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let h = Harness::with(Mode::NoIndex, edit).await;
        let path = h.home.join(format!(".pi/agent/sessions/--repo-cinco--/2026-10-01T10-00-00-000Z_{FIVE}.jsonl"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::File::open(&path).is_ok();
        let got = h.get(&pi("/repo-cinco")).await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        if !readable { assert_eq!(got, (200, json!([]))); }
    }
}

/// PRV-11: sessão repetida no índice do Kimi: a entrada mais recente (a última) vence.
#[tokio::test]
async fn prv11_kimi_repeated_session_takes_the_last_entry() {
    fn edit(home: &Path) {
        let sid = "session_ab697d05-66da-4b2a-8e80-3d769951e752";
        let index = home.join(".kimi-code/session_index.jsonl");
        let mut text = std::fs::read_to_string(&index).unwrap();
        text.push_str(&line(&json!({"sessionId": sid, "sessionDir": home.join(".kimi-code/sessions/wd_hangar_5112ff7a84e0").join(sid),
            "workDir": "/repo-kimi-novo"})));
        std::fs::write(&index, text).unwrap();
    }
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, edit).await;
        let (_, body) = h.get("/api/archive-por-cwd?cwd=/repo-kimi-novo&provider=kimi").await;
        assert_eq!(sids(&body), ["session_"]);
        assert_eq!(h.get(&format!("/api/archive-por-cwd?cwd={}&provider=kimi", enc(HANGAR_CWD))).await, (200, json!([])));
    }
}

/// PRV-13: rollout em `archived_sessions` entra; o mesmo arquivo pelas duas pastas (link) entra uma vez.
#[cfg(unix)]
#[tokio::test]
async fn prv13_archived_rollouts_enter_once() {
    fn edit(home: &Path) {
        let moved = home.join(MTIMES[4]);
        let archived = home.join(".codex-b/archived_sessions").join(moved.file_name().unwrap());
        std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
        std::fs::rename(&moved, &archived).unwrap();
        let original = home.join(MTIMES[0]);
        let link = home.join(".codex/archived_sessions").join(original.file_name().unwrap());
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&original, link).unwrap();
    }
    for mode in [Mode::Index, Mode::NoIndex] {
        let h = Harness::with(mode, edit).await;
        let (_, body) = h.get("/api/archive-por-cwd?cwd=/tmp/hangar-native-task4-fixture-jf&provider=codex").await;
        assert_eq!(sids(&body), ["01a0d000"]);
        let (_, body) = h.get(&format!("/api/archive-por-cwd?cwd={}&provider=codex", enc(HANGAR_CWD))).await;
        assert_eq!(sids(&body), ["01a05511"]);
    }
}

/// PRV-10 (limitação reproduzida): sessão do omp num perfil (`~/.omp/profiles/<p>/agent`) não entra no
/// Arquivo; só a raiz sem perfil é lida.
#[tokio::test]
async fn prv10_omp_profile_sessions_stay_out() {
    let h = Harness::with(Mode::NoIndex, |home: &Path| {
        let from = home.join(MTIMES[2]);
        let to = home.join(".omp/profiles/x/agent/sessions").join(from.parent().unwrap().file_name().unwrap())
            .join("2026-10-01T10-00-00-000Z_bbbbbbbb-0000-4000-8000-000000000001.jsonl");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&from, &to).unwrap();
    }).await;
    let cwd = "/tmp/claude-1000/-home-jefferson-Projetos-hangar/ac1a7b0f-d0a7-4e66-8f05-f53ea5d4b4da/scratchpad/omp-teste";
    let (_, body) = h.get(&format!("/api/archive-por-cwd?cwd={}&provider=omp", enc(cwd))).await;
    assert_eq!(sids(&body), ["01a06932"]);
}
