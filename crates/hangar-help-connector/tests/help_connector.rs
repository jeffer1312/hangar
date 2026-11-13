//! Conector do Help pelo host real, contra uma API isolada no loopback: a API é o único dublê.
//! As respostas são literais no formato do OpenAPI do Help; nenhuma credencial real.
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hangar_connectors::host::{Host, HostOptions};
use hangar_connectors::manifest::Manifest;
use hangar_connectors::registry::Registry;
use hangar_connectors::work_tracking::{Filter, GetParams, ListParams};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HELP: &str = env!("CARGO_BIN_EXE_hangar-help-test-connector");

#[derive(Default)]
struct Api { queries: Mutex<Vec<String>>, tokens: Mutex<Vec<String>>, accepted: Mutex<Vec<String>> }

fn person(login: &str, nome: &str) -> Value {
    json!({"id": format!("uuid-{login}"), "login": login, "nome": nome, "time_id": null})
}

fn pm(chave: &str, responsavel: Value, categoria: &str) -> Value {
    json!({
        "projeto_arquivado": false, "chave": chave, "tipo": "Bug", "resumo": format!("Resumo de {chave}"),
        "status": {"id": 3, "nome": "Em desenvolvimento", "categoria": categoria},
        "responsavel": responsavel, "time": {"id": "t1", "nome": "Equipe A"}, "responsavel_qa": null, "relator": person("ana", "Ana"),
        "story_points": null, "tem_subtarefas": false, "epico": null, "empresas": [], "sprint": null,
        "versoes_afetadas": [], "versoes_correcao": [], "builds": [],
        "criada_em": "2026-11-01T09:00:00-03:00", "atualizada_em": "2026-11-10T14:05:00-03:00", "status_desde": "2026-11-10T14:05:00-03:00",
        "flag": false, "pm_interna": false, "requer_teste_qa": false
    })
}

fn authorized(api: &Api, headers: &HeaderMap) -> bool {
    let token = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
    api.tokens.lock().unwrap().push(token.clone());
    api.accepted.lock().unwrap().iter().any(|accepted| token == format!("Bearer {accepted}"))
}

async fn eu(State(api): State<Arc<Api>>, headers: HeaderMap) -> Response {
    if !authorized(&api, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    hangar_core::http::json_response(StatusCode::OK, json!({"id": "uuid-maria", "login": "maria", "nome": "Maria", "email": "maria@exemplo.invalid", "avatar_url": null}))
}

async fn pagina(State(api): State<Arc<Api>>, headers: HeaderMap, RawQuery(query): RawQuery) -> Response {
    let query = query.unwrap_or_default();
    api.queries.lock().unwrap().push(query.clone());
    if !authorized(&api, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let get = |name: &str| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    if get("busca").as_deref() == Some("redireciona") {
        return (StatusCode::FOUND, [(header::LOCATION, "http://127.0.0.2:9/roubar")]).into_response();
    }
    let page = get("pagina").and_then(|page| page.parse::<u32>().ok()).unwrap_or(1);
    let items = if page == 1 { vec![pm("PM-101", person("maria", "Maria"), "em_andamento"), pm("PM-102", Value::Null, "a_fazer")] } else { vec![pm("PM-103", person("joao", "João"), "concluido")] };
    hangar_core::http::json_response(StatusCode::OK, json!({"itens": items, "total": 3, "pagina": page, "tamanho": 2}))
}

async fn fluxo(State(api): State<Arc<Api>>, headers: HeaderMap) -> Response {
    if !authorized(&api, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    let to = |id: i64, nome: &str, categoria: &str| json!({"id": id * 10, "nome": "t", "destino": {"id": id, "nome": nome, "categoria": categoria}});
    hangar_core::http::json_response(StatusCode::OK, json!([to(1, "Criado", "a_fazer"), to(3, "Em Progresso", "em_andamento"),
        to(5, "Sem Testes", "concluido"), to(3, "Em Progresso", "em_andamento")]))
}

async fn detalhe(State(api): State<Arc<Api>>, headers: HeaderMap, UrlPath(chave): UrlPath<String>) -> Response {
    if !authorized(&api, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    if chave != "PM-101" { return hangar_core::http::json_response(StatusCode::NOT_FOUND, json!({"erro": "PM não encontrada"})); }
    let mut value = pm("PM-101", person("maria", "Maria"), "em_andamento");
    value["descricao"] = json!("## Passos\n\n1. Abrir a **tela**\n2. Imprimir");
    value["comentarios"] = json!([{"texto": "não deve aparecer"}]);
    hangar_core::http::json_response(StatusCode::OK, value)
}

struct Bench { dir: tempfile::TempDir, api: Arc<Api>, base: String, host: Arc<Host>, server: tokio::task::JoinHandle<()> }

fn isolated(name: &str) -> bool {
    if std::env::var_os("HANGAR_HELP_TEST_CHILD").is_some() { return false; }
    // O conector herda o ambiente: cada cenário roda sem credenciais do processo executor.
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.env_clear().env("HANGAR_HELP_TEST_CHILD", "1").args(["--exact", name, "--nocapture"]);
    for key in ["SystemRoot", "WINDIR", "PATH", "TMP", "TEMP", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) { command.env(key, value); }
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    true
}

impl Bench {
    async fn new(token: Option<&str>) -> Self {
        let api = Arc::new(Api::default());
        api.accepted.lock().unwrap().extend(["token-ficticio".to_owned(), "token-novo".to_owned()]);
        let app = axum::Router::new()
            .route("/api/eu", axum::routing::get(eu))
            .route("/api/pms/pagina", axum::routing::get(pagina))
            .route("/api/fluxo", axum::routing::get(fluxo))
            .route("/api/pms/{chave}", axum::routing::get(detalhe))
            .with_state(api.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let dir = tempfile::tempdir().unwrap();
        for folder in ["pmedico", "claude"] { std::fs::create_dir(dir.path().join(folder)).unwrap(); }
        if let Some(token) = token { write_token(dir.path(), token); }
        let host = Host::new(Registry::new(dir.path().join("connectors.json")), HostOptions { exit_grace: Duration::from_millis(500), ..HostOptions::default() });
        let (config, claude) = (dir.path().join("pmedico"), dir.path().join("claude"));
        let command = [HELP, "--id", "help-test", "--help-url", &base,
            "--config-dir", config.to_str().unwrap(), "--claude-config-dir", claude.to_str().unwrap()];
        let manifest = json!({"manifest_version": 1, "id": "help-test", "name": "Help de teste", "version": "0.1.0",
            "protocol_version": 1, "capabilities": {"work_tracking": 1}, "command": command});
        host.add(Manifest::parse(manifest.to_string().as_bytes(), dir.path()).unwrap()).unwrap();
        Self { dir, api, base, host, server }
    }

    async fn list(&self, filter: Filter, cursor: Option<String>) -> Result<hangar_connectors::work_tracking::ListResult, hangar_connectors::CallError> {
        self.host.list_tickets("help-test", ListParams { filter, limit: 2, cursor }).await
    }

    fn last_query(&self) -> Vec<(String, String)> {
        let query = self.api.queries.lock().unwrap().last().cloned().unwrap_or_default();
        form_urlencoded::parse(query.as_bytes()).into_owned().collect()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.server.abort();
        let host = self.host.clone();
        std::thread::spawn(move || tokio::runtime::Runtime::new().unwrap().block_on(host.shutdown())).join().unwrap();
    }
}

fn write_token(dir: &std::path::Path, token: &str) {
    let file = dir.join("pmedico").join("credenciais.json");
    std::fs::write(file, json!({"HELP_TOKEN": token, "_atualizado_em": {"HELP_TOKEN": "2026-11-12T12:00:00+00:00"}}).to_string()).unwrap();
}

fn query(text: &str) -> Filter { Filter { query: Some(text.into()), ..Filter::default() } }

#[tokio::test(flavor = "multi_thread")]
async fn list_maps_help_fields_to_the_contract() {
    if isolated("list_maps_help_fields_to_the_contract") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    let page = bench.list(query("impressão"), None).await.unwrap();
    assert_eq!(bench.last_query(), [("pagina".into(), "1".into()), ("tamanho".into(), "2".into()), ("busca".into(), "impressão".into())]);
    assert_eq!(bench.api.tokens.lock().unwrap().last().unwrap(), "Bearer token-ficticio");
    let first = serde_json::to_value(&page.items[0]).unwrap();
    assert_eq!(first, json!({
        "id": "PM-101", "key": "PM-101", "title": "Resumo de PM-101", "url": format!("{}/pm/PM-101", bench.base),
        "status": {"id": "3", "name": "Em desenvolvimento", "category": "in_progress"},
        "assignee": {"id": "maria", "name": "Maria"}, "project": null, "updated_at": "2026-11-10T14:05:00-03:00"
    }));
    assert_eq!(page.items[1].assignee, None);
    assert_eq!(serde_json::to_value(page.items[1].status.category).unwrap(), "todo");
    assert_eq!(page.total, Some(3));
    assert!(page.next_cursor.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_walks_pages_and_is_bound_to_the_filters() {
    if isolated("cursor_walks_pages_and_is_bound_to_the_filters") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    let first = bench.list(query("impressão"), None).await.unwrap();
    let second = bench.list(query("impressão"), first.next_cursor.clone()).await.unwrap();
    assert!(bench.last_query().contains(&("pagina".into(), "2".into())));
    assert_eq!(second.items[0].id, "PM-103");
    assert_eq!(serde_json::to_value(second.items[0].status.category).unwrap(), "done");
    assert_eq!(second.next_cursor, None);
    let error = bench.list(query("outra coisa"), first.next_cursor).await.unwrap_err();
    assert_eq!(error.code(), "invalid_cursor");
}

#[tokio::test(flavor = "multi_thread")]
async fn assignee_me_becomes_the_login_of_the_token_owner() {
    if isolated("assignee_me_becomes_the_login_of_the_token_owner") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    bench.list(Filter { assignee: Some("me".into()), ..Filter::default() }, None).await.unwrap();
    assert!(bench.last_query().contains(&("responsaveis".into(), "maria".into())));
    bench.list(Filter { assignee: Some("joao".into()), ..Filter::default() }, None).await.unwrap();
    assert!(bench.last_query().contains(&("responsaveis".into(), "joao".into())));
}

#[tokio::test(flavor = "multi_thread")]
async fn detail_brings_markdown_and_nothing_else() {
    if isolated("detail_brings_markdown_and_nothing_else") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    let detail = bench.host.get_ticket("help-test", GetParams { ticket_id: "PM-101".into() }).await.unwrap();
    assert_eq!(detail.description_markdown, "## Passos\n\n1. Abrir a **tela**\n2. Imprimir");
    let value = serde_json::to_value(&detail).unwrap();
    assert!(value.get("comentarios").is_none());
    let error = bench.host.get_ticket("help-test", GetParams { ticket_id: "PM-999".into() }).await.unwrap_err();
    assert_eq!(error.code(), "not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_or_refused_token_is_auth_required_and_new_login_applies_without_restart() {
    if isolated("missing_or_refused_token_is_auth_required_and_new_login_applies_without_restart") { return; }
    let bench = Bench::new(None).await;
    assert_eq!(bench.list(query("x"), None).await.unwrap_err().code(), "auth_required");
    assert!(bench.api.tokens.lock().unwrap().is_empty(), "sem token, nenhum pedido sai");
    write_token(bench.dir.path(), "token-recusado");
    assert_eq!(bench.list(query("x"), None).await.unwrap_err().code(), "auth_required");
    write_token(bench.dir.path(), "token-novo");
    bench.list(query("x"), None).await.unwrap();
    assert_eq!(bench.api.tokens.lock().unwrap().last().unwrap(), "Bearer token-novo");
}

#[tokio::test(flavor = "multi_thread")]
async fn redirect_is_refused_instead_of_followed() {
    if isolated("redirect_is_refused_instead_of_followed") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    let error = bench.list(query("redireciona"), None).await.unwrap_err();
    assert_eq!(error.code(), "unavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn state_filter_becomes_the_status_ids_of_the_help_categories() {
    if isolated("state_filter_becomes_the_status_ids_of_the_help_categories") { return; }
    let bench = Bench::new(Some("token-ficticio")).await;
    bench.list(Filter { state: Some("open".into()), ..Filter::default() }, None).await.unwrap();
    let sent: Vec<String> = bench.last_query().into_iter().filter(|(k, _)| k == "status_ids").map(|(_, v)| v).collect();
    assert_eq!(sent, ["1", "3"], "a fazer e em andamento, sem repetir");
    bench.list(Filter { state: Some("done".into()), ..Filter::default() }, None).await.unwrap();
    let sent: Vec<String> = bench.last_query().into_iter().filter(|(k, _)| k == "status_ids").map(|(_, v)| v).collect();
    assert_eq!(sent, ["5"]);
    let first = bench.list(Filter { state: Some("open".into()), ..Filter::default() }, None).await.unwrap();
    let crossed = bench.list(Filter::default(), first.next_cursor).await;
    assert!(matches!(crossed, Err(hangar_connectors::CallError::Connector(ref e)) if e.code == hangar_connectors::protocol::ErrorCode::InvalidCursor), "o cursor guarda o estado pedido");
}
