//! `/api/diag` e `/api/diag/arquivo` atendidos pelo Rust, com o diário numa casa temporária. O esperado
//! vem do que a rota antiga respondeu (captura de 2026-11-16, commit 1504c6c53); o upstream recusado
//! prova que nada vai ao Python.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

const OWNER: &str = "diario-dono";

struct Fixture { home: tempfile::TempDir, base: String, task: tokio::task::JoinHandle<()> }

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

impl Fixture {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, internal_secret: "diario-interno".into(),
            auth_token: OWNER.into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
            ("LOCALAPPDATA".into(), home.path().join("AppData/Local").to_string_lossy().into_owned()),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}"), task }
    }

    fn diary(&self) -> PathBuf {
        if cfg!(windows) { self.home.path().join("AppData/Local/hangar/logs/diario") } else { self.home.path().join(".hangar/logs/diario") }
    }

    fn today(&self) -> PathBuf {
        self.diary().join(format!("uso-{}.jsonl", chrono::Local::now().format("%Y-%m-%d")))
    }

    fn request(&self, method: reqwest::Method, path: &str, owner: bool) -> reqwest::RequestBuilder {
        let request = reqwest::Client::builder().no_gzip().build().unwrap().request(method, format!("{}{path}", self.base));
        if owner { request.bearer_auth(OWNER) } else { request }
    }
}

async fn json_of(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    (status, serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or(Value::Null))
}

/// D-01, D-03, D-09, D-10: o dono grava no arquivo do dia, só com os campos aceitos e `origem` tela;
/// corpo ilegível responde `gravadas` 0 sem erro.
#[tokio::test(flavor = "multi_thread")]
async fn owner_post_writes_the_screen_line_of_the_day() {
    let f = Fixture::start().await;
    let body = json!({"eventos": [{"evento": "app.abriu", "nivel": "ok", "cli": "abc", "seq": 1, "so": "Linux", "extra": "sai"}]});
    let reply = f.request(reqwest::Method::POST, "/api/diag", true).json(&body).send().await.unwrap();
    assert_eq!(reply.headers()["content-type"], "application/json");
    assert_eq!(json_of(reply).await, (200, json!({"gravadas": 1})));
    let text = std::fs::read_to_string(f.today()).unwrap();
    let row: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["ts", "evento", "nivel", "cli", "seq", "so", "origem"]);
    assert_eq!((&row["evento"], &row["origem"]), (&json!("app.abriu"), &json!("tela")));
    assert!(text.starts_with("{\"ts\": \""), "separadores do arquivo antigo: {text}");
    let invalid = f.request(reqwest::Method::POST, "/api/diag", true).body("nao-json!").send().await.unwrap();
    assert_eq!(json_of(invalid).await, (200, json!({"gravadas": 0})));
    assert_eq!(std::fs::read_to_string(f.today()).unwrap(), text);
}

/// D-15, D-16: o resumo traz os números e as últimas linhas; `ultimas` que não é inteiro é 422.
#[tokio::test(flavor = "multi_thread")]
async fn summary_counts_files_and_validates_the_count() {
    let f = Fixture::start().await;
    let (status, empty) = json_of(f.request(reqwest::Method::GET, "/api/diag", true).send().await.unwrap()).await;
    assert_eq!((status, empty), (200, json!({"dias": 0, "bytes": 0, "arquivos": [], "dias_guardados": 7, "ultimas": []})));
    let batch = json!([{"evento": "a", "seq": 1}, {"evento": "b", "seq": 2}]);
    f.request(reqwest::Method::POST, "/api/diag", true).json(&batch).send().await.unwrap();
    let (status, summary) = json_of(f.request(reqwest::Method::GET, "/api/diag?ultimas=1", true).send().await.unwrap()).await;
    assert_eq!(status, 200);
    assert_eq!((&summary["dias"], &summary["dias_guardados"]), (&json!(1), &json!(7)));
    assert_eq!(summary["ultimas"].as_array().unwrap().len(), 1);
    assert_eq!(summary["ultimas"][0]["evento"], "b");
    let (status, refused) = json_of(f.request(reqwest::Method::GET, "/api/diag?ultimas=abc", true).send().await.unwrap()).await;
    assert_eq!((status, refused), (422, json!({"detail": [{"type": "int_parsing", "loc": ["query", "ultimas"],
        "msg": "Input should be a valid integer, unable to parse string as an integer", "input": "abc"}]})));
}

/// D-17, D-19, D-21: o download leva o cabeçalho da máquina sem `python`, com a versão do servidor em
/// `backend`, e sai em gzip quando pedido e com 1 KB ou mais.
#[tokio::test(flavor = "multi_thread")]
async fn download_has_the_machine_header_and_gzip() {
    use std::io::Read;
    let f = Fixture::start().await;
    let batch: Vec<Value> = (0..20).map(|i| json!({"evento": format!("e{i}"), "detalhe": "x".repeat(60)})).collect();
    f.request(reqwest::Method::POST, "/api/diag", true).json(&batch).send().await.unwrap();
    let plain = f.request(reqwest::Method::GET, "/api/diag/arquivo", true).send().await.unwrap();
    assert_eq!(plain.status(), 200);
    assert_eq!(plain.headers()["content-type"], "application/x-ndjson");
    assert_eq!(plain.headers()["content-disposition"], "attachment; filename=\"hangar-uso.jsonl\"");
    assert!(plain.headers().get("content-encoding").is_none());
    let text = plain.text().await.unwrap();
    let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let keys: Vec<&String> = header.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["evento", "ts", "so_servidor", "formato", "sistema", "arquitetura", "cpus", "mux", "mux_fonte",
        "inicio_backend", "pid_backend", "recursos_no_download", "dias_guardados", "ordem", "campos_documentados_em", "nao_contem", "backend"]);
    assert_eq!((&header["evento"], &header["formato"], &header["dias_guardados"]), (&json!("diag.formato"), &json!(2), &json!(7)));
    assert_eq!(header["pid_backend"], std::process::id());
    let resources = &header["recursos_no_download"];
    assert!(resources.get("memoria_total_mb").is_some() || resources.get("recursos_erro").is_some(), "{resources}");
    assert_eq!(text.lines().skip(1).count(), 20);
    let compressed = f.request(reqwest::Method::GET, "/api/diag/arquivo", true).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(compressed.headers()["content-encoding"], "gzip");
    assert_eq!(compressed.headers()["vary"], "Accept-Encoding");
    let mut unzipped = String::new();
    flate2::read::GzDecoder::new(compressed.bytes().await.unwrap().as_ref()).read_to_string(&mut unzipped).unwrap();
    assert_eq!(unzipped.lines().skip(1).collect::<Vec<_>>(), text.lines().skip(1).collect::<Vec<_>>());
}

/// D-17: logo depois de subir, com a versão ainda não lida, o cabeçalho espera a leitura em vez de
/// dizer "indisponivel". A versão vem do manifesto do pacote em `~/.hangar/current`.
#[tokio::test(flavor = "multi_thread")]
async fn download_header_reads_the_version_on_a_cold_cache() {
    let f = Fixture::start().await;
    let current = f.home.path().join(".hangar/current");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(current.join("manifest.json"), json!({"version": "0.1.0.8207", "commit": "abc1234def567890", "channel": "latest"}).to_string()).unwrap();
    let text = f.request(reqwest::Method::GET, "/api/diag/arquivo", true).send().await.unwrap().text().await.unwrap();
    let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["backend"], "0.1.0.8207-gabc1234def56");
}

/// Sem o token do dono as três rotas recusam com 401 e nada é gravado (rota antiga: `require_auth`).
#[tokio::test(flavor = "multi_thread")]
async fn without_the_owner_token_every_route_refuses() {
    let f = Fixture::start().await;
    let unauthorized = json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}});
    let post = f.request(reqwest::Method::POST, "/api/diag", false).json(&json!([{"evento": "x"}])).send().await.unwrap();
    assert_eq!(json_of(post).await, (401, unauthorized.clone()));
    for path in ["/api/diag", "/api/diag/arquivo"] {
        let reply = f.request(reqwest::Method::GET, path, false).send().await.unwrap();
        assert_eq!(json_of(reply).await, (401, unauthorized.clone()), "{path}");
    }
    assert!(!f.today().exists());
}

/// D-23: o convidado com login grava no diário do dono, pelo Rust; o resumo segue fora do alcance dele.
#[tokio::test]
async fn login_guest_posts_to_the_owner_diary() {
    let (home, _python, connect) = crate::connect_port::guest_server().await;
    let posted = crate::fake::client().post(format!("http://{connect}/api/diag")).bearer_auth(crate::connect_port::GUEST)
        .json(&json!([{"evento": "g"}])).send().await.unwrap();
    assert_eq!(json_of(posted).await, (200, json!({"gravadas": 1})));
    let diary = if cfg!(windows) { home.path().join("AppData/Local/hangar/logs/diario") } else { home.path().join(".hangar/logs/diario") };
    let written = std::fs::read_to_string(diary.join(format!("uso-{}.jsonl", chrono::Local::now().format("%Y-%m-%d")))).unwrap();
    assert!(written.contains("\"evento\": \"g\"") && written.contains("\"origem\": \"tela\""), "{written}");
    let summary = crate::connect_port::get(connect, "/api/diag").bearer_auth(crate::connect_port::GUEST).send().await.unwrap();
    assert_eq!(summary.status(), 403);
}
