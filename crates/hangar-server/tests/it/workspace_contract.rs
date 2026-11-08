//! Contratos públicos de validação, acesso e transferência, com upstream-armadilha.
use crate::{fake::{OWNER, client}, workspace_fixture};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::atomic::Ordering};

const READ: &str = r#"{"path":"sub/ação.txt","text":"primeira\nlinha mudada\n","size":22,"truncated":false,"digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#;
const MISSING_MESSAGE: &str = r#"{"detail":[{"type":"missing","loc":["body","message"],"msg":"Field required","input":{}}]}"#;
const INVALID_JSON: &str = r#"{"detail":[{"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}}]}"#;
const BUSY: &str = r#"{"detail":{"code":"session_transfer_busy","msg":"A sessão está trocando de agente; tente novamente quando terminar.","params":{}}}"#;
const UNAUTHORIZED: &str = r#"{"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}"#;
const FORBIDDEN_LOGIN: &str = r#"{"detail":{"code":"erro_fora_do_convidado","params":{},"msg":"fora do acesso do convidado"}}"#;
const FORBIDDEN_SHARE: &str = r#"{"detail":{"code":"erro_fora_do_convite","params":{},"msg":"fora da sessao compartilhada"}}"#;

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(root).args(args)
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim_end().to_owned()
}

fn make_repo(repo: &Path) {
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    git(repo, &["init", "-b", "main"]);
    for (key, value) in [("user.name", "Teste"), ("user.email", "teste@example.invalid"),
        ("core.hooksPath", ""), ("commit.gpgSign", "false"), ("color.ui", "false"), ("core.quotePath", "true")] {
        git(repo, &["config", key, value]);
    }
    std::fs::write(repo.join("sub/ação.txt"), "primeira\nlinha\n").unwrap();
    std::fs::write(repo.join("outro.txt"), "outro\n").unwrap();
    git(repo, &["add", "sub/ação.txt", "outro.txt"]);
    let result = std::process::Command::new("git").arg("-C").arg(repo).args(["commit", "-m", "Inicial"])
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z").output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    std::fs::write(repo.join("sub/ação.txt"), "primeira\nlinha mudada\n").unwrap();
}

struct Scene {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    panes: PathBuf,
    f: workspace_fixture::Fixture,
}

impl Scene {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = hangar_workspace::real(dir.path());
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir(&home).unwrap();
        make_repo(&repo);
        let panes = home.join("panes.txt");
        std::fs::write(&panes, Self::pane_rows(&repo, 1700000000)).unwrap();
        #[cfg(unix)]
        let mux = {
            let mux = home.join("contract-mux");
            crate::write_executable(&mux, format!("#!/bin/sh\ncase \"$1\" in\nlist-panes) cat '{}'; if [ -f '{}' ]; then printf seen > '{}'; fi ;;\ndisplay-message) printf '1700000000\\n' ;;\nhas-session) exit 0 ;;\nesac\n", panes.display(), home.join("observe-mux").display(), home.join("mux-observed").display()));
            mux
        };
        #[cfg(windows)]
        let mux = {
            let mux = home.join("contract-mux.cmd");
            std::fs::write(&mux, format!("@echo off\r\nif \"%~1\"==\"list-panes\" type \"{}\"\r\nif \"%~1\"==\"display-message\" echo 1700000000\r\nexit /b 0\r\n", panes.display())).unwrap();
            mux
        };
        let f = workspace_fixture::policy_fixture(&root, &home, &mux).await;
        for name in ["fixture", "fixture-login"] {
            let transcript = home.join(format!("{name}.jsonl"));
            std::fs::write(&transcript, json!({"cwd":repo,"text":"sub/ação.txt"}).to_string()).unwrap();
            f.list.seed(name, transcript.to_str().unwrap());
        }
        Self { _dir:dir, root, repo, home, panes, f }
    }

    fn pane_rows(repo: &Path, birth: u64) -> String {
        format!("fixture\t1\t\t{}\t%1\t\tclaude\t{birth}\t0\t0\nfixture-login\t1\t\t{}\t%2\t\tclaude\t{birth}\t0\t0\n", repo.display(), repo.display())
    }

    fn effects(&self) -> (String, String, String, Vec<u8>) {
        (git(&self.repo, &["rev-parse", "HEAD"]), git(&self.repo, &["status", "--porcelain=v1"]),
            git(&self.repo, &["ls-files", "--stage"]), std::fs::read(self.repo.join("sub/ação.txt")).unwrap())
    }

    fn request(&self, method: &str, route: &str, input: &str) -> reqwest::RequestBuilder {
        client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{}{}", self.f.addr, route))
            .bearer_auth(OWNER).header("content-type", "application/json").body(input.to_owned())
    }

    fn no_upstream(&self) {
        assert_eq!(self.f.context_hits.load(Ordering::Relaxed), 0, "callback de contexto");
        assert_eq!(self.f.hits(), 0, "repasse ao handler antigo");
    }

    fn transfer(&self) {
        let id = "11111111-2222-4333-8444-555555555555";
        let base = self.home.join(".hangar/conversation-transfers");
        std::fs::create_dir_all(base.join("indexes")).unwrap();
        std::fs::write(base.join(format!("{id}.json")), json!({"id":id,"name":"fixture",
            "source_life":"t:1700000000","phase":"source_stopped","source":null,
            "origin_meta":{"name":"fixture","provider":"claude","cwd":self.repo,"jsonl":self.home.join("fixture.jsonl"),"key":"transfer-fixture-origin"},
            "destination_meta":null,"boundary":null,"error_code":null}).to_string()).unwrap();
        let key = ring::digest::digest(&ring::digest::SHA256, b"fixture").as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        std::fs::write(base.join("indexes").join(format!("session-{key}.json")), json!([id]).to_string()).unwrap();
    }
}

async fn wire(response: reqwest::Response, status: u16, expected: &str) {
    let actual_status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap();
    assert_eq!(actual_status, status, "{body}");
    let expected_json = serde_json::from_str::<Value>(expected);
    if let Ok(expected) = expected_json {
        assert_eq!(headers.get("content-type").unwrap(), "application/json", "{body}");
        assert_eq!(serde_json::from_str::<Value>(&body).ok(), Some(expected), "{body}");
    } else { assert_eq!(body, expected); }
}

macro_rules! decoder {
    ($name:ident, $method:literal, $path:literal, $input:literal, $expected:expr) => {
        #[tokio::test]
        async fn $name() {
            let scene = Scene::new().await;
            let before = scene.effects();
            let response = scene.request($method, $path, $input).send().await.unwrap();
            assert_eq!(scene.effects(), before, "o decoder não pode alterar arquivo, índice ou HEAD");
            wire(response, 422, $expected).await;
            scene.no_upstream();
        }
    };
}

decoder!(g02_checkout_string, "POST", "/api/sessions/fixture/checkout", r#"{"branch":2}"#, r#"{"detail":[{"type":"string_type","loc":["body","branch"],"msg":"Input should be a valid string","input":2}]}"#);
decoder!(g02_action_enum, "POST", "/api/sessions/fixture/git", r#"{"action":"evil"}"#, r#"{"detail":[{"type":"literal_error","loc":["body","action"],"msg":"Input should be 'status', 'pull', 'fetch', 'stash', 'stash-pop', 'log', 'revert-abort' or 'cherry-pick-abort'","input":"evil","ctx":{"expected":"'status', 'pull', 'fetch', 'stash', 'stash-pop', 'log', 'revert-abort' or 'cherry-pick-abort'"}}]}"#);
decoder!(g02_path_string, "POST", "/api/sessions/fixture/git/diff", r#"{"path":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","path"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_path_diff_scope, "POST", "/api/sessions/fixture/git/path-diff", r#"{"path":"sub/ação.txt","escopo":4}"#, r#"{"detail":[{"type":"string_type","loc":["body","escopo"],"msg":"Input should be a valid string","input":4}]}"#);
decoder!(g02_commit_message_minimum, "POST", "/api/sessions/fixture/git/commit", r#"{"message":"","paths":["sub/ação.txt"]}"#, r#"{"detail":[{"type":"string_too_short","loc":["body","message"],"msg":"String should have at least 1 character","input":"","ctx":{"min_length":1}}]}"#);
decoder!(g02_commit_path_item, "POST", "/api/sessions/fixture/git/commit", r#"{"message":"x","paths":[42]}"#, r#"{"detail":[{"type":"string_type","loc":["body","paths",0],"msg":"Input should be a valid string","input":42}]}"#);
decoder!(g02_commit_error_order, "POST", "/api/sessions/fixture/git/commit", r#"{"extra":true,"paths":[42],"new_branch":5,"message":""}"#, r#"{"detail":[{"type":"string_too_short","loc":["body","message"],"msg":"String should have at least 1 character","input":"","ctx":{"min_length":1}},{"type":"string_type","loc":["body","paths",0],"msg":"Input should be a valid string","input":42},{"type":"string_type","loc":["body","new_branch"],"msg":"Input should be a valid string","input":5},{"type":"extra_forbidden","loc":["body","extra"],"msg":"Extra inputs are not permitted","input":true}]}"#);
decoder!(g02_sha_string, "POST", "/api/sessions/fixture/git/revert", r#"{"sha":1}"#, r#"{"detail":[{"type":"string_type","loc":["body","sha"],"msg":"Input should be a valid string","input":1}]}"#);
decoder!(g02_reset_mode, "POST", "/api/sessions/fixture/git/reset", r#"{"sha":"20bc9e7267b8e5a4a32370c1855c4db636e589e6","mode":"evil"}"#, r#"{"detail":[{"type":"literal_error","loc":["body","mode"],"msg":"Input should be 'soft', 'mixed' or 'hard'","input":"evil","ctx":{"expected":"'soft', 'mixed' or 'hard'"}}]}"#);
decoder!(g02_branch_name, "POST", "/api/sessions/fixture/git/branch", r#"{"name":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","name"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_tag_message, "POST", "/api/sessions/fixture/git/tag", r#"{"name":"tag-fixture","message":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","message"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_file_digest, "POST", "/api/sessions/fixture/files/write", r#"{"path":"sub/ação.txt","text":"perdida","digest":42}"#, r#"{"detail":[{"type":"string_type","loc":["body","digest"],"msg":"Input should be a valid string","input":42}]}"#);
decoder!(g02_resolver_items, "POST", "/api/sessions/fixture/files/resolver", r#"{"caminhos":[42]}"#, r#"{"detail":[{"type":"string_type","loc":["body","caminhos",0],"msg":"Input should be a valid string","input":42}]}"#);
decoder!(g02_mkdir_name, "POST", "/api/fs/mkdir", r#"{"root":"/mnt/case/work","name":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","name"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_folder_root, "POST", "/api/fs/git/fetch", r#"{"root":1}"#, r#"{"detail":[{"type":"string_type","loc":["body","root"],"msg":"Input should be a valid string","input":1}]}"#);
decoder!(g02_folder_switch_branch, "POST", "/api/fs/git/switch", r#"{"root":"/mnt/case/work","branch":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","branch"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_folder_branch_name, "POST", "/api/fs/git/branch", r#"{"root":"/mnt/case/work","name":5}"#, r#"{"detail":[{"type":"string_type","loc":["body","name"],"msg":"Input should be a valid string","input":5}]}"#);
decoder!(g02_required_query_path, "GET", "/api/sessions/fixture/files/read", "", r#"{"detail":[{"type":"missing","loc":["query","path"],"msg":"Field required","input":null}]}"#);
decoder!(g02_query_integer, "GET", "/api/sessions/fixture/git/log?n=abc", "", r#"{"detail":[{"type":"int_parsing","loc":["query","n"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":"abc"}]}"#);
decoder!(g02_query_boolean, "GET", "/api/sessions/fixture/files/list?so_modificados=evil", "", r#"{"detail":[{"type":"bool_parsing","loc":["query","so_modificados"],"msg":"Input should be a valid boolean, unable to interpret input","input":"evil"}]}"#);
decoder!(g02_commit_null, "POST", "/api/sessions/fixture/git/commit", "null", r#"{"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}"#);
decoder!(g02_commit_array, "POST", "/api/sessions/fixture/git/commit", "[]", r#"{"detail":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":[]}]}"#);
decoder!(g02_commit_empty_object, "POST", "/api/sessions/fixture/git/commit", "{}", MISSING_MESSAGE);
decoder!(g02_commit_malformed, "POST", "/api/sessions/fixture/git/commit", "{", INVALID_JSON);
decoder!(g02_commit_absent_body, "POST", "/api/sessions/fixture/git/commit", "", r#"{"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}"#);
decoder!(g02_commit_extra, "POST", "/api/sessions/fixture/git/commit", r#"{"message":"Selecionado","paths":["sub/ação.txt"],"amend":false,"new_branch":null,"extra":true}"#, r#"{"detail":[{"type":"extra_forbidden","loc":["body","extra"],"msg":"Extra inputs are not permitted","input":true}]}"#);
#[tokio::test]
async fn g02_methods_keep_allow_and_head_body_without_upstream() {
    let scene = Scene::new().await;
    let before = scene.effects();
    let mut failures = Vec::new();
    for (route, allowed, methods) in [
        ("/api/sessions/fixture/git/commit", "POST", &["GET", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"][..]),
        ("/api/sessions/fixture/files/read?path=sub%2Fa%C3%A7%C3%A3o.txt", "GET", &["POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"][..]),
    ] {
        for method in methods {
            let response = scene.request(method, route, "").send().await.unwrap();
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = response.text().await.unwrap();
            let expected = if *method == "HEAD" { "" } else { r#"{"detail":"Method Not Allowed"}"# };
            if status != 405 || headers.get("allow").and_then(|v|v.to_str().ok()) != Some(allowed)
                || headers.get("content-type").and_then(|v|v.to_str().ok()) != Some("application/json")
                || headers.get("content-length").and_then(|v|v.to_str().ok()) != Some("31") || body != expected {
                failures.push(format!("{method} {route}: status={status}, headers={headers:?}, body={body}"));
            }
        }
    }
    assert_eq!(scene.effects(), before);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    scene.no_upstream();
}

#[tokio::test]
async fn g02_trailing_slashes_preserve_method_and_query_without_upstream() {
    let scene = Scene::new().await;
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let mut failures = Vec::new();
    for (method, path, query) in [("GET", "files/read", "path=sub%2Fa%C3%A7%C3%A3o.txt"), ("POST", "git/commit", "n=2&x=%2F")] {
        let response = c.request(method.parse::<reqwest::Method>().unwrap(),
            format!("http://{}/api/sessions/fixture/{path}/?{query}", scene.f.addr)).bearer_auth(OWNER).send().await.unwrap();
        let status = response.status();
        let location = response.headers().get("location").cloned();
        let body = response.text().await.unwrap();
        let expected = format!("http://{}/api/sessions/fixture/{path}?{query}", scene.f.addr);
        if status != 307 || location.as_ref().and_then(|v|v.to_str().ok()) != Some(expected.as_str()) || !body.is_empty() {
            failures.push(format!("{method} {path}: status={status}, location={location:?}, body={body}, esperado={expected}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    scene.no_upstream();
}

#[tokio::test]
async fn g02_missing_session_is_local_404() {
    let scene = Scene::new().await;
    let response = scene.request("GET", "/api/sessions/missing/files/list", "").send().await.unwrap();
    wire(response, 404, r#"{"detail":{"code":"erro_sessao_inexistente","params":{},"msg":"sessao nao encontrada"}}"#).await;
    scene.no_upstream();
}

#[tokio::test]
async fn g02_session_without_cwd_is_local_404() {
    let scene = Scene::new().await;
    std::fs::write(&scene.panes, Scene::pane_rows(Path::new(""), 1700000000)).unwrap();
    let response = scene.request("GET", "/api/sessions/fixture/files/list", "").send().await.unwrap();
    wire(response, 404, r#"{"detail":{"code":"erro_sessao_inexistente","params":{},"msg":"sessao nao encontrada"}}"#).await;
    scene.no_upstream();
}

#[tokio::test]
async fn g02_malformed_json_precedes_auth_dependency() {
    let scene = Scene::new().await;
    let response = client().post(format!("http://{}/api/sessions/fixture/git/commit", scene.f.addr))
        .header("content-type", "application/json").body("{").send().await.unwrap();
    wire(response, 422, INVALID_JSON).await;
    scene.no_upstream();
}

#[tokio::test]
async fn g02_nan_keeps_plain_500_and_workspace_cors() {
    let scene = Scene::new().await;
    let response = scene.request("POST", "/api/sessions/fixture/checkout", r#"{"branch":NaN}"#)
        .header("origin", "http://other-server.invalid").send().await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap();
    assert_eq!(status, 500, "{body}");
    assert_eq!(body, "Internal Server Error");
    assert_eq!(headers["content-type"], "text/plain; charset=utf-8");
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert_eq!(headers["access-control-expose-headers"], "ETag");
    scene.no_upstream();
}

#[tokio::test]
async fn g02_cors_wraps_native_validation_and_preflight() {
    let scene = Scene::new().await;
    for (method, path, input, status, expected) in [
        ("GET", "/api/sessions/fixture/files/read?path=sub%2Fa%C3%A7%C3%A3o.txt", "", 200, READ),
        ("POST", "/api/sessions/fixture/git/commit", "{}", 422, MISSING_MESSAGE),
    ] {
        let response = scene.request(method, path, input).header("origin", "http://other-server.invalid").send().await.unwrap();
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        assert_eq!(response.headers()["access-control-expose-headers"], "ETag");
        wire(response, status, expected).await;
    }
    scene.no_upstream();
}

#[tokio::test]
async fn g02_preflight_keeps_all_captured_headers_without_auth() {
    let scene = Scene::new().await;
    for (path, method) in [("/api/sessions/fixture/files/read", "GET"), ("/api/worktrees/fetch", "POST")] {
        let response = client().request(reqwest::Method::OPTIONS, format!("http://{}{path}", scene.f.addr))
            .header("origin", "http://other-server.invalid").header("access-control-request-method", method)
            .header("access-control-request-headers", "authorization,content-type").send().await.unwrap();
        assert_eq!(response.status(), 200);
        for (name, value) in [("content-type", "text/plain; charset=utf-8"), ("access-control-allow-origin", "*"),
            ("access-control-allow-methods", "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT"), ("access-control-max-age", "600"),
            ("access-control-allow-headers", "authorization,content-type")] { assert_eq!(response.headers()[name], value); }
        assert_eq!(response.text().await.unwrap(), "OK");
    }
    scene.no_upstream();
}

#[tokio::test]
async fn g02_branch_string_boolean_has_the_captured_effect() {
    let scene = Scene::new().await;
    let file = std::fs::read(scene.repo.join("sub/ação.txt")).unwrap();
    let response = scene.request("POST", "/api/sessions/fixture/git/branch", r#"{"name":"coerced","switch_after":"true"}"#).send().await.unwrap();
    wire(response, 200, r#"{"ok":true,"output":"Switched to a new branch 'coerced'"}"#).await;
    assert_eq!(git(&scene.repo, &["branch", "--show-current"]), "coerced");
    assert_eq!(std::fs::read(scene.repo.join("sub/ação.txt")).unwrap(), file);
    scene.no_upstream();
}

fn four_dirty_paths(scene: &Scene) {
    for path in ["bin.dat", "escaping-link", "novo.txt"] { std::fs::write(scene.repo.join(path), "fixture").unwrap(); }
}

#[tokio::test]
async fn g02_folder_switch_string_boolean_preserves_dirty_refusal() {
    let scene = Scene::new().await;
    four_dirty_paths(&scene);
    git(&scene.repo, &["branch", "feature"]);
    let before = scene.effects();
    let response = scene.request("POST", "/api/fs/git/switch", &json!({"root":scene.root,"path":scene.repo,
        "branch":"feature","confirm_sessions":"false"}).to_string()).send().await.unwrap();
    assert_eq!(scene.effects(), before);
    wire(response, 409, r#"{"detail":{"code":"erro_git_folder_dirty","msg":"4 arquivo(s) com alteração não commitada: nada foi feito. Commite ou guarde essas alterações antes; o Hangar não descarta trabalho.","params":{"n":4}}}"#).await;
    scene.no_upstream();
}

#[tokio::test]
async fn g02_folder_branch_string_booleans_create_without_checkout() {
    let scene = Scene::new().await;
    four_dirty_paths(&scene);
    let response = scene.request("POST", "/api/fs/git/branch", &json!({"root":scene.root,"path":scene.repo,
        "name":"folder-coerced","checkout":"false","confirm_sessions":"true"}).to_string()).send().await.unwrap();
    wire(response, 200, &json!({"dirty":4,"ahead":null,"behind":null,"repo":true,"current":"main","upstream":null,
        "toplevel":scene.repo,"last_fetch":null,"sessions":["fixture","fixture-login"]}).to_string()).await;
    assert_eq!(git(&scene.repo, &["branch", "--show-current"]), "main");
    assert_eq!(git(&scene.repo, &["rev-parse", "refs/heads/folder-coerced"]), git(&scene.repo, &["rev-parse", "HEAD"]));
    scene.no_upstream();
}

#[tokio::test]
async fn g02_mkdir_ignores_extra_fields_without_losing_its_effect() {
    let scene = Scene::new().await;
    let response = scene.request("POST", "/api/fs/mkdir", &json!({"root":scene.root,"name":"mkdir-extra","extra":true}).to_string())
        .send().await.unwrap();
    let status = response.status();
    let raw = response.text().await.unwrap();
    assert_eq!(status, 200, "{raw}");
    let parsed = serde_json::from_str::<Value>(&raw);
    assert!(parsed.is_ok(), "corpo JSON do mkdir: {raw}");
    let body = parsed.unwrap();
    assert!(scene.root.join("mkdir-extra").is_dir());
    let mtime = std::fs::metadata(scene.root.join("mkdir-extra")).unwrap().modified().unwrap()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    assert!((body["mtime"].as_f64().unwrap() - mtime).abs() <= 0.000001);
    assert_eq!(body, json!({"name":"mkdir-extra","path":scene.root.join("mkdir-extra"),"is_git":false,
        "has_claude_md":false,"mtime":mtime}));
    scene.no_upstream();
}

#[tokio::test]
async fn g02_body_limits_remain_specific_to_workspace_and_worktrees() {
    let scene = Scene::new().await;
    four_dirty_paths(&scene);
    for size in [4194304usize, 4194305] {
        let mut body = r#"{"action":"status"}"#.to_owned();
        body.extend(std::iter::repeat_n(' ', size - body.len()));
        let response = scene.request("POST", "/api/sessions/fixture/git", &body).send().await.unwrap();
        if size == 4194304 {
            wire(response, 200, r###"{"ok":true,"output":"## main\n M \"sub/a\\303\\247\\303\\243o.txt\"\n?? bin.dat\n?? escaping-link\n?? novo.txt"}"###).await;
        } else { wire(response, 413, r#"{"detail":"request body too large"}"#).await; }
    }
    for size in [65536usize, 65537] {
        let mut body = r#"{"location":"","inactive_days":null,"delete_merged":false,"delete_unchanged":false,"keep_when":"uncommitted-changes"}"#.to_owned();
        body.extend(std::iter::repeat_n(' ', size - body.len()));
        let response = scene.request("PUT", "/api/worktrees/settings", &body).send().await.unwrap();
        if size == 65536 { wire(response, 200, r#"{"location":"","inactive_days":null,"delete_merged":false,"delete_unchanged":false,"keep_when":"uncommitted-changes"}"#).await; }
        else { wire(response, 422, r#"{"detail":"corpo de worktrees inválido"}"#).await; }
    }
    scene.no_upstream();
}

async fn fixed_clock_child(name: &str) -> bool {
    if std::env::var("HANGAR_WORKSPACE_CONTRACT_CLOCK").ok().as_deref() == Some(name) { return false; }
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("workspace_contract::{name}"), "--nocapture"])
        .env("HANGAR_WORKSPACE_CONTRACT_CLOCK", name).env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z").env("TZ", "UTC").env("LC_ALL", "C.UTF-8")
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1").kill_on_drop(true).output().await.unwrap();
    assert!(output.status.success(), "processo isolado: {}\n{}\n{}", output.status,
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    true
}

#[tokio::test]
async fn g02_commit_amend_string_boolean_matches_the_captured_commit() {
    if fixed_clock_child("g02_commit_amend_string_boolean_matches_the_captured_commit").await { return; }
    let scene = Scene::new().await;
    assert_eq!(git(&scene.repo, &["rev-parse", "HEAD"]), "20bc9e7267b8e5a4a32370c1855c4db636e589e6", "estado inicial da captura");
    let response = scene.request("POST", "/api/sessions/fixture/git/commit", r#"{"message":"Reword","paths":[],"amend":"true"}"#).send().await.unwrap();
    wire(response, 200, r#"{"ok":true,"output":"[main 74aeff1] Reword\n Date: Wed Jan 1 00:00:00 2020 +0000\n 2 files changed, 3 insertions(+)\n create mode 100644 outro.txt\n create mode 100644 \"sub/a\\303\\247\\303\\243o.txt\""}"#).await;
    assert_eq!(git(&scene.repo, &["rev-parse", "HEAD"]), "74aeff19b86e2d2e2f426316260feb3c6a12e72b");
    assert_eq!(git(&scene.repo, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&scene.repo, &["show", "HEAD:sub/ação.txt"]), "primeira\nlinha");
    assert_eq!(std::fs::read_to_string(scene.repo.join("sub/ação.txt")).unwrap(), "primeira\nlinha mudada\n");
    scene.no_upstream();
}

#[tokio::test]
async fn g02_mime_keeps_typed_native_json_but_fallback_uses_the_mime() {
    if fixed_clock_child("g02_mime_keeps_typed_native_json_but_fallback_uses_the_mime").await { return; }
    for content_type in [Some("text/plain"), None] {
        let scene = Scene::new().await;
        let mut request = client().post(format!("http://{}/api/sessions/fixture/git/commit", scene.f.addr)).bearer_auth(OWNER)
            .body(r#"{"message":"Selecionado","paths":["sub/ação.txt"],"amend":false,"new_branch":null}"#);
        if let Some(mime) = content_type { request = request.header("content-type", mime); }
        wire(request.send().await.unwrap(), 200, r#"{"ok":true,"output":"[main a332a7e] Selecionado\n 1 file changed, 1 insertion(+), 1 deletion(-)"}"#).await;
        assert_eq!(git(&scene.repo, &["log", "-1", "--format=%s"]), "Selecionado");
        assert_eq!(git(&scene.repo, &["status", "--porcelain=v1"]), "");
        scene.no_upstream();
    }
    let scene = Scene::new().await;
    let response = client().post(format!("http://{}/api/sessions/fixture/git/commit", scene.f.addr))
        .bearer_auth(OWNER).header("content-type", "text/plain").body("{}").send().await.unwrap();
    wire(response, 422, r#"{"detail":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":"{}"}]}"#).await;
    scene.no_upstream();
}

async fn mismatch(response: reqwest::Response, expected_status: u16, expected_body: &str) -> Option<String> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap();
    let expected: Value = serde_json::from_str(expected_body).unwrap();
    (status != expected_status || serde_json::from_str::<Value>(&body).ok() != Some(expected)
        || headers.get("content-type").and_then(|v|v.to_str().ok()) != Some("application/json"))
        .then(||format!("status={status}, esperado={expected_status}, headers={headers:?}, body={body}, esperado_body={expected_body}"))
}

#[tokio::test]
async fn p01_owner_cookie_invite_pair_and_login_keep_their_own_access() {
    if fixed_clock_child("p01_owner_cookie_invite_pair_and_login_keep_their_own_access").await { return; }
    let mut failures = Vec::new();
    for role in ["owner", "none", "wrong", "cookie", "full", "view", "login-own", "login-other", "owner-guest-port"] {
        let scene = Scene::new().await;
        let policy = scene.f.policy.as_ref().unwrap();
        policy.login.store.create("Ana", scene.repo.to_str().unwrap(), false, true, "ana".into(), "login-token".into()).unwrap();
        policy.login.store.claim("fixture-login", "ana", Some("t:1700000000")).unwrap();
        let full = policy.share.store.create_redeemed("fixture", Some("t:1700000000".into()), hangar_server::share::store::Kind::Share).unwrap().1;
        let view = policy.share.store.create_redeemed("fixture", Some("t:1700000000".into()), hangar_server::share::store::Kind::Pair).unwrap().1;
        let name = if role == "login-own" { "fixture-login" } else { "fixture" };
        let addr = if matches!(role, "full" | "view" | "owner-guest-port") { policy.guest } else { scene.f.addr };
        let before = scene.effects();
        for (method, route, input) in [("GET", "files/read?path=sub%2Fa%C3%A7%C3%A3o.txt", ""),
            ("POST", "git/commit", r#"{"message":"Selecionado","paths":["sub/ação.txt"],"amend":false,"new_branch":null}"#)] {
            let mut request = client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{addr}/api/sessions/{name}/{route}"))
                .header("content-type", "application/json").body(input);
            request = match role {
                "none" => request,
                "wrong" => request.bearer_auth("wrong-owner"),
                "cookie" => request.header("cookie", format!("cp_token={OWNER}")),
                "full" => request.bearer_auth(&full),
                "view" => request.bearer_auth(&view),
                "login-own" | "login-other" => request.bearer_auth("login-token"),
                _ => request.bearer_auth(OWNER),
            };
            let (status, expected) = match role {
                "none" | "wrong" | "owner-guest-port" => (401, UNAUTHORIZED),
                "cookie" if method == "POST" => (401, UNAUTHORIZED),
                "view" => (403, FORBIDDEN_SHARE),
                "login-other" => (403, FORBIDDEN_LOGIN),
                _ if method == "GET" => (200, READ),
                _ => (200, r#"{"ok":true,"output":"[main a332a7e] Selecionado\n 1 file changed, 1 insertion(+), 1 deletion(-)"}"#),
            };
            if let Some(why) = mismatch(request.send().await.unwrap(), status, expected).await { failures.push(format!("{role} {method}: {why}")); }
            if method == "POST" {
                if status == 200 {
                    if git(&scene.repo, &["log", "-1", "--format=%s"]) != "Selecionado" || git(&scene.repo, &["status", "--porcelain=v1"]) != "" {
                        failures.push(format!("{role}: commit autorizado sem o efeito contratado"));
                    }
                } else if scene.effects() != before { failures.push(format!("{role}: escrita recusada alterou arquivo/índice/HEAD")); }
            }
        }
        if role == "view" {
            let response = client().get(format!("http://{}/api/sessions/fixture/history?limit=5", policy.guest)).bearer_auth(&view).send().await.unwrap();
            if let Some(why) = mismatch(response, 200, "[]").await { failures.push(format!("view history: {why}")); }
        }
        if scene.f.hits() != 0 || scene.f.context_hits.load(Ordering::Relaxed) != 0 {
            failures.push(format!("{role}: repasse={}, contexto={}", scene.f.hits(), scene.f.context_hits.load(Ordering::Relaxed)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn p01_login_roots_are_private_and_invite_does_not_gain_global_access() {
    let scene = Scene::new().await;
    let policy = scene.f.policy.as_ref().unwrap();
    policy.login.store.create("Ana", scene.repo.to_str().unwrap(), false, true, "ana".into(), "login-token".into()).unwrap();
    let full = policy.share.store.create_redeemed("fixture", Some("t:1700000000".into()),
        hangar_server::share::store::Kind::Share).unwrap().1;
    let response = client().get(format!("http://{}/api/fs/roots", scene.f.addr)).bearer_auth("login-token").send().await.unwrap();
    wire(response, 200, &json!([{"name":"repo","path":scene.repo}]).to_string()).await;
    let response = client().get(format!("http://{}/api/fs/roots", policy.guest)).bearer_auth(&full).send().await.unwrap();
    wire(response, 403, FORBIDDEN_SHARE).await;
    scene.no_upstream();
}

async fn prove_transfer_fixture(scene: &Scene, expected_blocked: bool) {
    let rows = scene.f.list.discover(None).await.unwrap();
    let (_, blocked) = hangar_server::list::local_transfers::transfers(&rows,
        scene.f.list.env().dirs.as_ref().unwrap()).expect("registro de transferência original válido");
    assert_eq!(blocked.contains("fixture"), expected_blocked, "precondição da vida da transferência");
}

macro_rules! during_transfer {
    ($name:ident, $route:literal, $input:literal, $status:literal, $expected:expr) => {
        #[tokio::test]
        async fn $name() {
            let scene = Scene::new().await;
            scene.transfer();
            prove_transfer_fixture(&scene, true).await;
            let before = scene.effects();
            let response = scene.request("POST", $route, $input).send().await.unwrap();
            assert_eq!(scene.effects(), before, "mutações durante transferência não podem alterar arquivo, índice ou HEAD");
            wire(response, $status, $expected).await;
            scene.no_upstream();
        }
    };
}

during_transfer!(x02_valid_discard_is_blocked_before_its_effect, "/api/sessions/fixture/git/discard", r#"{"path":"sub/ação.txt"}"#, 409, BUSY);
during_transfer!(x02_valid_file_write_is_blocked_before_its_effect, "/api/sessions/fixture/files/write", r#"{"path":"sub/ação.txt","text":"perdida","digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#, 409, BUSY);
during_transfer!(x02_syntax_error_precedes_transfer_guard, "/api/sessions/fixture/git/commit", "{", 422, INVALID_JSON);
during_transfer!(x02_valid_json_null_follows_transfer_guard, "/api/sessions/fixture/git/commit", "null", 409, BUSY);
during_transfer!(x02_valid_json_invalid_dto_follows_transfer_guard, "/api/sessions/fixture/git/commit", "{}", 409, BUSY);
during_transfer!(x02_cited_write_invalid_dto_follows_transfer_guard, "/api/sessions/fixture/file/text", r#"{"path":"sub/ação.txt","extra":true}"#, 409, BUSY);
decoder!(x02_invalid_commit_without_transfer_is_422, "POST", "/api/sessions/fixture/git/commit", "{}", MISSING_MESSAGE);
decoder!(x02_invalid_cited_write_without_transfer_is_422, "POST", "/api/sessions/fixture/file/text", r#"{"path":"sub/ação.txt","extra":true}"#, r#"{"detail":[{"type":"missing","loc":["body","text"],"msg":"Field required","input":{"path":"sub/ação.txt","extra":true}},{"type":"extra_forbidden","loc":["body","extra"],"msg":"Extra inputs are not permitted","input":true}]}"#);

#[tokio::test]
async fn x02_recreated_name_does_not_inherit_the_previous_transfer() {
    let scene = Scene::new().await;
    scene.transfer();
    prove_transfer_fixture(&scene, true).await;
    let before = scene.effects();
    std::fs::write(&scene.panes, Scene::pane_rows(&scene.repo, 1700000001)).unwrap();
    let response = scene.request("POST", "/api/sessions/fixture/git/commit", "{}").send().await.unwrap();
    let rows = scene.f.list.discover(None).await.unwrap();
    assert_eq!(rows.iter().find(|r|r.name == "fixture").unwrap().lifecycle_id.as_deref(), Some("t:1700000001"));
    prove_transfer_fixture(&scene, false).await;
    assert_eq!(scene.effects(), before);
    wire(response, 422, MISSING_MESSAGE).await;
    scene.no_upstream();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn x02_ingress_hold_waits_for_the_last_effect_and_receipt() {
    use std::io::{Read, Write};
    use std::time::Duration;
    let scene = Scene::new().await;
    let hooks = scene.home.join("hooks");
    std::fs::create_dir(&hooks).unwrap();
    let ready = scene.home.join("hook-ready");
    let release = scene.home.join("hook-release");
    for path in [&ready, &release] {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    }
    let mut ready_keepalive = std::fs::OpenOptions::new().read(true).write(true).open(&ready).unwrap();
    let release_keepalive = std::fs::OpenOptions::new().read(true).write(true).open(&release).unwrap();
    crate::write_executable(&hooks.join("pre-commit"), format!("#!/bin/sh\nprintf 'R' > '{}'\nread -r release < '{}'\n", ready.display(), release.display()));
    git(&scene.repo, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let before = scene.effects();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let mut reader = ready_keepalive.try_clone().unwrap();
    let thread = std::thread::spawn(move || {
        let mut byte = [0];
        reader.read_exact(&mut byte).unwrap();
        let _ = started_tx.send(());
        let released = release_rx.recv_timeout(Duration::from_secs(30)).is_ok();
        let mut writer = release_keepalive;
        writer.write_all(b"go\n").unwrap();
        released
    });
    let first = tokio::spawn(scene.request("POST", "/api/sessions/fixture/git/commit",
        r#"{"message":"Ingress","paths":["sub/ação.txt"]}"#).send());
    let started = tokio::time::timeout(Duration::from_secs(20), started_rx).await.is_ok();
    let runtime = scene.f.runtime.clone();
    let mut closing = Box::pin(runtime.ingress().hold("fixture", Duration::from_secs(25)));
    let early = futures_util::poll!(closing.as_mut());
    let closed_early = early.is_ready();
    let following = tokio::time::timeout(Duration::from_secs(5), scene.request("POST", "/api/sessions/fixture/files/write",
        r#"{"path":"sub/ação.txt","text":"perdida","digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#).send()).await;
    // A limpeza acontece antes das asserções para o RED também recolher o Git e o hook.
    if !started { ready_keepalive.write_all(b"X").unwrap(); }
    let _ = release_tx.send(());
    assert!(thread.join().unwrap(), "o hook só pode sair pela liberação explícita");
    let response = tokio::time::timeout(Duration::from_secs(20), first).await.unwrap().unwrap().unwrap();
    let closing_ok = match early { std::task::Poll::Ready(result) => result.is_ok(), std::task::Poll::Pending => closing.await.is_ok() };
    let after = scene.effects();
    runtime.ingress().release("fixture");
    assert!(started, "o processo Git deve ter entrado no pre-commit");
    assert!(!closed_early, "o fechamento terminou enquanto a mutação Git estava bloqueada no hook");
    assert!(closing_ok);
    assert_ne!(after.0, before.0);
    assert_eq!(after.3, b"primeira\nlinha mudada\n");
    let expected = json!({"ok":true,"output":format!("[main {}] Ingress\n 1 file changed, 1 insertion(+), 1 deletion(-)", &after.0[..7])});
    wire(response, 200, &expected.to_string()).await;
    wire(following.unwrap().unwrap(), 409, BUSY).await;
    scene.no_upstream();
}

#[cfg(unix)]
#[tokio::test]
async fn x02_ingress_wait_rechecks_life_before_the_first_effect() {
    use notify::Watcher;
    use std::{sync::{Arc, Mutex}, time::Duration};
    let scene = Scene::new().await;
    scene.f.runtime.ingress().close("fixture", Duration::from_secs(5)).await.unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(tx)));
    let observed = scene.home.join("mux-observed");
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event|event.paths.contains(&observed)) {
            if let Some(sender) = sender.lock().unwrap().take() { let _ = sender.send(()); }
        }
    }).unwrap();
    watcher.watch(&scene.home, notify::RecursiveMode::NonRecursive).unwrap();
    std::fs::write(scene.home.join("observe-mux"), "1").unwrap();
    let before = scene.effects();
    let pending = tokio::spawn(scene.request("POST", "/api/sessions/fixture/files/write",
        r#"{"path":"sub/ação.txt","text":"perdida","digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#).send());
    let discovered = tokio::time::timeout(Duration::from_secs(10), rx).await.is_ok_and(|value|value.is_ok());
    std::fs::write(&scene.panes, Scene::pane_rows(&scene.repo, 1700000001)).unwrap();
    scene.f.runtime.ingress().open("fixture");
    let response = tokio::time::timeout(Duration::from_secs(10), pending).await.unwrap().unwrap().unwrap();
    assert!(discovered, "a primeira identidade deve vir do Mux antes da recriação");
    assert_eq!(scene.effects(), before, "o pedido da vida anterior não pode escrever depois da espera");
    wire(response, 409, BUSY).await;
    scene.no_upstream();
}

#[tokio::test]
async fn x02_unreadable_transfer_record_refuses_before_effects() {
    let scene = Scene::new().await;
    scene.transfer();
    prove_transfer_fixture(&scene, true).await;
    std::fs::write(scene.home.join(".hangar/conversation-transfers/11111111-2222-4333-8444-555555555555.json"), "{").unwrap();
    let rows = scene.f.list.discover(None).await.unwrap();
    assert!(hangar_server::list::local_transfers::transfers(&rows, scene.f.list.env().dirs.as_ref().unwrap()).is_err());
    let before = scene.effects();
    let response = scene.request("POST", "/api/sessions/fixture/files/write",
        r#"{"path":"sub/ação.txt","text":"perdida","digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#).send().await.unwrap();
    assert_eq!(scene.effects(), before, "registro ilegível não autoriza escrita");
    let body = workspace_fixture::refusal(response, "workspace_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "transferencia");
    scene.no_upstream();
}

#[tokio::test]
async fn g02_review_query_cited_write_takes_path_only_from_the_body() {
    let scene = Scene::new().await;
    let before = scene.effects();
    let response = scene.request("POST", "/api/sessions/fixture/file/text",
        r#"{"path":"sub/ação.txt","text":"original","digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}"#).send().await.unwrap();
    wire(response, 200, r#"{"path":"sub/ação.txt","size":8,"digest":"0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"}"#).await;
    let after = scene.effects();
    assert_eq!((after.0, after.2), (before.0, before.2));
    assert_eq!(after.3, b"original");
    let read = scene.request("GET", "/api/sessions/fixture/file/text?path=sub%2Fa%C3%A7%C3%A3o.txt", "").send().await.unwrap();
    wire(read, 200, r#"{"path":"sub/ação.txt","text":"original","size":8,"truncated":false,"digest":"0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"}"#).await;
    let missing = scene.request("GET", "/api/sessions/fixture/file/text", "").send().await.unwrap();
    wire(missing, 422, r#"{"detail":[{"type":"missing","loc":["query","path"],"msg":"Field required","input":null}]}"#).await;
    scene.no_upstream();
}

#[tokio::test]
async fn g02_review_query_cited_raw_file_ignores_unknown_root_flag() {
    let scene = Scene::new().await;
    let file = std::fs::File::options().read(true).write(true).open(scene.repo.join("sub/ação.txt")).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1700000000))).unwrap();
    drop(file);
    let response = scene.request("GET", "/api/sessions/fixture/file?path=sub%2Fa%C3%A7%C3%A3o.txt&root=evil", "").send().await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.text().await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "primeira\nlinha mudada\n");
    for (name, expected) in [("content-type", "text/plain; charset=utf-8"), ("content-length", "22"),
        ("etag", "\"isolated-17979cfe362a0000-16\""), ("cache-control", "max-age=60"),
        ("x-content-type-options", "nosniff"), ("referrer-policy", "no-referrer"),
        ("accept-ranges", "bytes"), ("last-modified", "Tue, 14 Nov 2023 22:13:20 GMT")] {
        assert_eq!(headers[name], expected, "{name}");
    }
    scene.no_upstream();
}

#[tokio::test]
async fn g02_review_query_uncited_file_does_not_gain_root_access() {
    let scene = Scene::new().await;
    std::fs::write(scene.repo.join("private.txt"), "segredo").unwrap();
    let response = scene.request("GET", "/api/sessions/fixture/file?path=private.txt&root=evil", "").send().await.unwrap();
    wire(response, 403, r#"{"detail":{"code":"erro_arquivo_nao_citado","params":{},"msg":"file not referenced in this conversation"}}"#).await;
    assert_eq!(std::fs::read_to_string(scene.repo.join("private.txt")).unwrap(), "segredo");
    scene.no_upstream();
}

#[test]
fn g02_public_external_inputs_never_panic_or_reach_python() {
    use proptest::{prelude::*, test_runner::{Config, RngSeed, TestRunner}};
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let scene = runtime.block_on(Scene::new());
    let mut runner = TestRunner::new(Config { cases:32, rng_seed:RngSeed::Fixed(0x475032),
        failure_persistence:None, max_shrink_iters:0, ..Config::default() });
    runner.run(&(0usize..3, proptest::collection::vec(any::<u8>(), 0..256)), |(route, bytes)| {
        runtime.block_on(async {
            let (method, path) = [("POST", "/api/sessions/fixture/checkout"),
                ("POST", "/api/sessions/fixture/git/commit"), ("GET", "/api/sessions/fixture/files/read")][route];
            let response = scene.request(method, path, "").body(bytes).send().await.unwrap();
            let status = response.status().as_u16();
            let body = response.text().await.unwrap();
            prop_assert!(status < 500, "status={status}, corpo={body}");
            prop_assert_eq!(scene.f.hits(), 0, "entrada externa foi ao handler antigo");
            prop_assert_eq!(scene.f.context_hits.load(Ordering::Relaxed), 0);
            Ok(())
        })
    }).unwrap();
}

decoder!(g02_commit_private_fields, "POST", "/api/sessions/fixture/git/commit", r#"{"message":"Selecionado","paths":["sub/ação.txt"],"amend":false,"new_branch":null,"cwd":"/outro","jsonl":"/outro.jsonl","roots":["/"]}"#, r#"{"detail":[{"type":"extra_forbidden","loc":["body","cwd"],"msg":"Extra inputs are not permitted","input":"/outro"},{"type":"extra_forbidden","loc":["body","jsonl"],"msg":"Extra inputs are not permitted","input":"/outro.jsonl"},{"type":"extra_forbidden","loc":["body","roots"],"msg":"Extra inputs are not permitted","input":["/"]}]}"#);
