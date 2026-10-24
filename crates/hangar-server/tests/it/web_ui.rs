//! Tela web (`frontend/dist`) servida pelo Rust na porta do dono, a chave ligar/desligar e a busca
//! da tela pela chave das fontes.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with(crate::refused_address()).await
    }

    async fn start_with(upstream: std::net::SocketAddr) -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream, internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}"), task }
    }

    /// `~/.hangar/current`: a raiz da instalação que o servidor enxerga.
    fn root(&self) -> std::path::PathBuf { self.home.path().join(".hangar/current") }

    async fn get(&self, path: &str) -> reqwest::Response {
        reqwest::Client::new().get(format!("{}{path}", self.base)).send().await.unwrap()
    }

    async fn setting(&self, body: Option<Value>) -> reqwest::Response {
        let client = reqwest::Client::new();
        let url = format!("{}/api/web-ui", self.base);
        let request = match body {
            Some(body) => client.put(url).header("content-type", "application/json").body(body.to_string()),
            None => client.get(url),
        };
        request.bearer_auth("fixture-owner").send().await.unwrap()
    }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

async fn code(response: reqwest::Response) -> String {
    let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    value["detail"]["code"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn package_mode_serves_the_ui_of_the_manifest_key() {
    let fixture = Fixture::start().await;
    write(&fixture.root().join("manifest.json"), &json!({"version": "0.1.0.1", "commit": "c", "channel": "main", "dist_key": KEY}).to_string());
    let ui = fixture.home.path().join(".hangar/ui").join(KEY);
    write(&ui.join("index.html"), "<p>pacote</p>");
    write(&ui.join("assets/app-123.js"), "console.log(1)");
    // A pasta do checkout não vale quando o manifesto diz a chave.
    write(&fixture.root().join("frontend/dist/index.html"), "<p>checkout</p>");

    let index = fixture.get("/").await;
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["cache-control"], "no-cache");
    assert!(index.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(index.headers().contains_key("etag"));
    assert_eq!(index.text().await.unwrap(), "<p>pacote</p>");

    let asset = fixture.get("/assets/app-123.js").await;
    assert_eq!(asset.status(), 200);
    assert_eq!(asset.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert!(asset.headers()["content-type"].to_str().unwrap().contains("javascript"));
    assert_eq!(asset.text().await.unwrap(), "console.log(1)");
}

#[tokio::test]
async fn without_manifest_the_checkout_dist_is_served_and_a_directory_falls_back_to_its_index() {
    let fixture = Fixture::start().await;
    write(&fixture.root().join("frontend/dist/index.html"), "<p>checkout</p>");
    write(&fixture.root().join("frontend/dist/sub/index.html"), "<p>sub</p>");
    assert_eq!(fixture.get("/").await.text().await.unwrap(), "<p>checkout</p>");
    assert_eq!(fixture.get("/sub/").await.text().await.unwrap(), "<p>sub</p>");
    // Fora da pasta não sai nada, nem com `..` codificado.
    write(&fixture.root().join("frontend/secret.txt"), "segredo");
    assert_ne!(fixture.get("/%2e%2e/secret.txt").await.status(), 200);
}

#[tokio::test]
async fn revalidation_with_the_etag_is_a_304() {
    let fixture = Fixture::start().await;
    write(&fixture.root().join("frontend/dist/index.html"), "<p>checkout</p>");
    let etag = fixture.get("/").await.headers()["etag"].clone();
    let again = reqwest::Client::new().get(format!("{}/", fixture.base)).header("if-none-match", etag).send().await.unwrap();
    assert_eq!(again.status(), 304);
}

#[tokio::test]
async fn turned_off_the_ui_answers_with_a_code_and_the_api_keeps_working() {
    let fixture = Fixture::start().await;
    write(&fixture.root().join("frontend/dist/index.html"), "<p>checkout</p>");
    write(&fixture.root().join("frontend/dist/assets/app-1.js"), "x");

    let initial: Value = fixture.setting(None).await.json().await.unwrap();
    assert_eq!(initial["web_ui"], true);
    assert_eq!(fixture.setting(Some(json!({"web_ui": false}))).await.status(), 200);

    let index = fixture.get("/").await;
    assert_eq!(index.status(), 404);
    assert_eq!(code(index).await, "web_ui_disabled");
    assert_eq!(code(fixture.get("/assets/app-1.js").await).await, "web_ui_disabled");
    let read: Value = fixture.setting(None).await.json().await.unwrap();
    assert_eq!(read["web_ui"], false);

    assert_eq!(fixture.setting(Some(json!({"web_ui": true}))).await.status(), 200);
    assert_eq!(fixture.get("/").await.status(), 200);
}

#[tokio::test]
async fn the_setting_is_owner_only_and_validated() {
    let fixture = Fixture::start().await;
    let stranger = reqwest::Client::new().get(format!("{}/api/web-ui", fixture.base)).send().await.unwrap();
    assert_eq!(stranger.status(), 403);
    assert_eq!(fixture.setting(Some(json!({"web_ui": "no"}))).await.status(), 400);
}

#[tokio::test]
async fn a_missing_ui_folder_goes_to_python_as_before() {
    let python = axum::Router::new().fallback(|| async { "python" });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, python).await.unwrap(); });
    let fixture = Fixture::start_with(upstream).await;
    write(&fixture.root().join("manifest.json"), &json!({"version": "0.1.0.1", "commit": "c", "channel": "main", "dist_key": KEY}).to_string());
    let index = fixture.get("/").await;
    assert_eq!(index.status(), 200);
    assert_eq!(index.text().await.unwrap(), "python");
    task.abort();
}

#[tokio::test]
async fn revalidation_accepts_lists_weak_tags_and_star_and_gzip_has_its_own_etag() {
    let fixture = Fixture::start().await;
    write(&fixture.root().join("frontend/dist/assets/big.js"), &"x".repeat(4096));
    // Pedido cru: o reqwest do workspace descomprime sozinho e esconde o `content-encoding`.
    let address = fixture.base.trim_start_matches("http://").to_owned();
    let get = |headers: Vec<(&'static str, String)>| raw_get(address.clone(), "/assets/big.js", headers);
    let plain = get(vec![]).await;
    assert_eq!(plain.0, 200);
    assert_eq!(plain.1["accept-ranges"], "none");
    assert_eq!(plain.1["vary"], "Accept-Encoding");
    assert!(!plain.1.contains_key("content-encoding"));
    let plain_tag = plain.1["etag"].clone();
    let gz = get(vec![("accept-encoding", "gzip".into())]).await;
    assert_eq!(gz.1["content-encoding"], "gzip");
    let gz_tag = gz.1["etag"].clone();
    assert_ne!(gz_tag, plain_tag);

    let list = get(vec![("if-none-match", format!("\"outra\", W/{plain_tag}"))]).await;
    assert_eq!(list.0, 304);
    assert_eq!(list.1["vary"], "Accept-Encoding");
    assert_eq!(get(vec![("if-none-match", "*".into())]).await.0, 304);
    // A tag da versão gzip não vale para quem não pede gzip.
    assert_eq!(get(vec![("if-none-match", gz_tag.clone())]).await.0, 200);
    assert_eq!(get(vec![("if-none-match", gz_tag), ("accept-encoding", "gzip".into())]).await.0, 304);
}

/// Status e cabeçalhos (nome em minúsculas) de um GET HTTP/1.1 cru.
async fn raw_get(address: String, path: &str, headers: Vec<(&'static str, String)>) -> (u16, std::collections::HashMap<String, String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    for (k, v) in headers { request += &format!("{k}: {v}\r\n"); }
    stream.write_all(format!("{request}\r\n").as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let head = String::from_utf8_lossy(&raw[..raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap()]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let map = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    (status, map)
}

// --- ui::fetch -------------------------------------------------------------------------------

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (name, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, format!("./{name}"), body.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Uma release falsa: `frontend-dist-<chave>.tar.gz` e o `.sha256` no formato do `sha256sum`.
async fn fake_release(assets: Vec<(String, Vec<u8>)>) -> (String, tokio::task::JoinHandle<()>) {
    let assets: Arc<std::collections::HashMap<String, Vec<u8>>> = Arc::new(assets.into_iter().collect());
    let router = axum::Router::new().route("/{name}", axum::routing::get(move |axum::extract::Path(name): axum::extract::Path<String>| {
        let assets = assets.clone();
        async move {
            match assets.get(&name) {
                Some(body) => (axum::http::StatusCode::OK, body.clone()),
                None => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (base, tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); }))
}

fn release_for(key: &str, archive: &[u8], sha: &str) -> Vec<(String, Vec<u8>)> {
    let name = format!("frontend-dist-{key}.tar.gz");
    vec![(format!("{name}.sha256"), format!("{sha}  {name}\n").into_bytes()), (name, archive.to_vec())]
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).map(|it| it.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn fetch_with_a_wrong_sha_leaves_nothing_behind() {
    let archive = tarball(&[("index.html", "<p>novo</p>")]);
    let (base, task) = fake_release(release_for(KEY, &archive, &"0".repeat(64))).await;
    let ui = tempfile::tempdir().unwrap();
    let err = hangar_server::ui::fetch_from(&reqwest::Client::new(), &base, KEY, ui.path()).await.unwrap_err();
    assert_eq!(err.code, "ui_sha_mismatch");
    assert_eq!(entries(ui.path()), Vec::<String>::new());
    task.abort();
}

#[tokio::test]
async fn fetch_renames_into_place_and_keeps_the_last_two_keys() {
    let keys: Vec<String> = ["1", "2", "3"].iter().map(|c| c.repeat(64)).collect();
    let mut assets = Vec::new();
    for key in &keys {
        let archive = tarball(&[("index.html", key), ("assets/app.js", "x")]);
        assets.extend(release_for(key, &archive, &sha256_hex(&archive)));
    }
    let (base, task) = fake_release(assets).await;
    let ui = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();

    let dir = hangar_server::ui::fetch_from(&client, &base, &keys[0], ui.path()).await.unwrap();
    assert_eq!(dir, ui.path().join(&keys[0]));
    assert_eq!(std::fs::read_to_string(dir.join("index.html")).unwrap(), keys[0]);
    assert_eq!(std::fs::read_to_string(dir.join("assets/app.js")).unwrap(), "x");
    assert_eq!(entries(ui.path()), vec![keys[0].clone()], "sem sobra .tmp");

    for key in &keys[1..] {
        hangar_server::ui::fetch_from(&client, &base, key, ui.path()).await.unwrap();
    }
    assert_eq!(entries(ui.path()), vec![keys[1].clone(), keys[2].clone()]);
    task.abort();
}

#[tokio::test]
async fn pruning_never_removes_the_key_the_running_install_serves() {
    let keys: Vec<String> = ["a", "b", "c"].iter().map(|c| c.repeat(64)).collect();
    let mut assets = Vec::new();
    for key in &keys {
        let archive = tarball(&[("index.html", key)]);
        assets.extend(release_for(key, &archive, &sha256_hex(&archive)));
    }
    let (base, task) = fake_release(assets).await;
    let home = tempfile::tempdir().unwrap();
    let ui = home.path().join(".hangar/ui");
    // Em uso: a chave A, a do manifesto da raiz atual, até o reinício.
    write(&home.path().join(".hangar/current/manifest.json"), &json!({"version": "0.1.0.1", "commit": "c", "channel": "main", "dist_key": keys[0]}).to_string());
    let client = reqwest::Client::new();
    for key in &keys {
        hangar_server::ui::fetch_from(&client, &base, key, &ui).await.unwrap();
    }
    assert_eq!(entries(&ui), vec![keys[0].clone(), keys[2].clone()]);
    task.abort();
}

#[tokio::test]
async fn fetch_refuses_a_key_that_is_not_a_sha256_and_a_key_never_published() {
    let (base, task) = fake_release(Vec::new()).await;
    let ui = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    assert_eq!(hangar_server::ui::fetch_from(&client, &base, "../x", ui.path()).await.unwrap_err().code, "ui_key_invalid");
    assert_eq!(hangar_server::ui::fetch_from(&client, &base, KEY, ui.path()).await.unwrap_err().code, "ui_key_not_published");
    assert_eq!(entries(ui.path()), Vec::<String>::new());
    task.abort();
}
