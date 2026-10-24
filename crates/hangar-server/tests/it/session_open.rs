//! Abertura de sessão: o despacho do `POST /api/sessions` e o comando `hangar-server open`.
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::fake::*;

/// Todo pedido válido é do Rust (a recusa também); o corpo que não é pedido segue ao Python intacto, sem
/// marca nenhuma (a do cliente não passa): o 422 é o dele.
#[tokio::test]
async fn create_opens_claude_terminal_in_rust_and_forwards_the_rest() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let post = |body: &'static [u8]| client().post(format!("http://{srv}/api/sessions")).bearer_auth(OWNER)
        .header("content-type", "application/json").header("x-hangar-open", "python").body(body.to_vec()).send();
    let resp = post(br#"{"name":"s1","cwd":"/nao/existe","provider":"claude","headless":false}"#).await.unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(serde_json::from_str::<Value>(&resp.text().await.unwrap()).unwrap()["detail"]["code"], "erro_cwd_inexistente");
    let codex = post(br#"{"name":"s1","cwd":"/nao/existe","provider":"codex","headless":true}"#).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&codex.text().await.unwrap()).unwrap()["detail"]["code"], "erro_cwd_inexistente");
    assert_eq!(fake.hits_to("/api/sessions"), 0, "o Rust respondeu sozinho");
    let headless = post(br#"{"name":"s1","cwd":"/nao/existe","provider":"claude","headless":true}"#).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&headless.text().await.unwrap()).unwrap()["detail"]["code"], "erro_cwd_inexistente");
    assert_eq!(fake.hits_to("/api/sessions"), 0, "o Claude sem terminal também é do Rust");
    for body in [&br#"{"name":"s1","cwd":"/x","bogus":1}"#[..], b"nem json"] {
        let resp = client().post(format!("http://{srv}/api/sessions")).bearer_auth(OWNER)
            .header("content-type", "application/json").header("x-hangar-open", "python").body(body.to_vec()).send().await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "from-python");
        assert_eq!(fake.last_body(), body);
        assert!(fake.last_hit().1.get("x-hangar-open").is_none(), "a marca do cliente não passa");
    }
    assert_eq!(fake.hits_to("/api/sessions"), 2);
}

/// O que entrou direto no Python (Connect) chega pela ponte privada (`<nome>/open`), só com o segredo, e
/// abre (ou é recusado) no Rust.
#[tokio::test]
async fn the_private_claude_bridge_opens_what_entered_python_directly() {
    let (_python, upstream) = spawn_fake().await;
    let state = std::sync::Arc::new(hangar_server::routes::AppState::new(config(upstream, "127.0.0.1")));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let router = hangar_server::routes::terminal_router(state);
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let open = |body: Value, secret: bool| {
        let request = client().post(format!("http://{private}/__hangar_server/claude/s1/open")).header("content-type", "application/json");
        let request = if secret { request.header("x-hangar-internal", SECRET) } else { request };
        async move { request.body(body.to_string()).send().await.unwrap() }
    };
    let codex = serde_json::json!({"request": {"name": "s1", "cwd": "/nao/existe", "provider": "codex"}, "guest": null});
    assert_eq!(open(codex.clone(), false).await.status(), 404, "sem o segredo a ponte não existe");
    let refused: Value = serde_json::from_str(&open(codex, true).await.text().await.unwrap()).unwrap();
    assert_eq!(refused["detail"]["code"], "erro_cwd_inexistente", "o Codex abre (e recusa) no Rust");
    let claude = serde_json::json!({"request": {"name": "s1", "cwd": "/nao/existe", "provider": "claude", "headless": false}, "guest": null});
    let refused: Value = serde_json::from_str(&open(claude, true).await.text().await.unwrap()).unwrap();
    assert_eq!(refused["detail"]["code"], "erro_cwd_inexistente", "a recusa é a da abertura do Rust");
    // O pedido torto é recusa de validação, não "o Rust não atende": o 421 o faria parecer o Rust fora.
    let bogus = open(serde_json::json!({"request": {"name": "s1", "cwd": "/x", "bogus": 1}, "guest": null}), true).await;
    assert_eq!(bogus.status(), 422);
    let refused: Value = serde_json::from_str(&bogus.text().await.unwrap()).unwrap();
    assert_eq!(refused["detail"]["code"], "erro_corpo_invalido");
}

/// O bastão do dono vai ao Python com a marca de que o Rust abre a sucessora Claude com terminal; a
/// resposta sem a marca (outro provedor, recusa) volta como veio. Sem ser o dono, nada de marca.
#[tokio::test]
async fn the_owner_bastao_goes_to_python_marked_for_rust() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let post = |token: Option<&'static str>| {
        let request = client().post(format!("http://{srv}/api/sessions/cc/bastao")).header("content-type", "application/json")
            .header("x-hangar-open", "rust").body(r#"{"name":"cc2"}"#);
        async move { (match token { Some(t) => request.bearer_auth(t), None => request }).send().await.unwrap() }
    };
    assert_eq!(post(Some(OWNER)).await.text().await.unwrap(), "from-python");
    assert_eq!(fake.last_hit().1["x-hangar-open"], "rust");
    post(None).await;
    assert!(fake.last_hit().1.get("x-hangar-open").is_none(), "a marca de fora não passa");
}

/// O passo da criação é do Rust (o Python também cria pelo repasse dele): a rota responde daqui.
#[tokio::test]
async fn the_creation_progress_is_answered_by_rust() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let resp = client().get(format!("http://{srv}/api/sessions/creation-progress?name=ningu%C3%A9m")).bearer_auth(OWNER).send().await.unwrap();
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null);
    assert_eq!(body, serde_json::json!({"step": null, "params": {}}));
    assert_eq!(fake.hits_to("/api/sessions/creation-progress"), 0);
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session-open").join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// `<ROOT>/a/b` vira o caminho desta máquina, com o separador dela (o fixture foi gravado no Linux e
/// as contas comparam o caminho como texto); `json` escapa a barra invertida para dentro de string JSON.
fn place(text: &str, root: &Path, json: bool) -> String {
    let root = root.to_string_lossy();
    regex::Regex::new(r#"<ROOT>((?:/[^"/\s]*)*)"#).unwrap().replace_all(text, |c: &regex::Captures| {
        let path = format!("{root}{}", &c[1]).replace('/', std::path::MAIN_SEPARATOR_STR);
        if json { path.replace('\\', "\\\\") } else { path }
    }).into_owned()
}

/// O estado inicial do cenário numa raiz de verdade, como o harness o monta para o Python.
fn materialize(fixture: &Value, root: &Path) {
    let state = &fixture["scenario"]["state"];
    for dir in state["dirs"].as_array().into_iter().flatten() {
        std::fs::create_dir_all(place(dir.as_str().unwrap(), root, false)).unwrap();
    }
    for (path, content) in state["files"].as_object().into_iter().flatten() {
        let path = place(path, root, false);
        std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        let text = match content { Value::String(text) => place(text, root, false), json => place(&json.to_string(), root, true) };
        std::fs::write(path, text).unwrap();
    }
}

/// Instala o comando como numa cópia do Hangar: `<raiz>/inst/bin/hangar-server` ao lado de
/// `<raiz>/inst/backend/.env`, de onde ele lê token, porta e as opções `CP_*` do servidor.
fn install(root: &Path, env_file: &str) -> std::path::PathBuf {
    let bin = root.join("inst/bin/hangar-server");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(root.join("inst/backend")).unwrap();
    crate::copy_executable(Path::new(env!("CARGO_BIN_EXE_hangar-server")), &bin);
    std::fs::write(root.join("inst/backend/.env"), env_file).unwrap();
    bin
}

/// Ambiente vazio; no Windows o `SystemRoot` fica, porque sem ele o Winsock não sobe e nenhuma conexão sai.
fn clean_env(command: &mut Command) -> &mut Command {
    command.env_clear();
    if let Some(system_root) = std::env::var_os("SystemRoot") { command.env("SystemRoot", system_root); }
    command
}

/// Roda fora do runtime: o servidor falso do teste continua atendendo enquanto o comando espera.
async fn hangar_open(bin: &Path, root: &Path, args: &[&str], extra_env: &[(&str, String)]) -> (i32, Value, String) {
    let mut command = Command::new(bin);
    clean_env(&mut command).args(args).env("HOME", root.join("home")).env("PATH", root.join("bin"));
    for (key, value) in extra_env { command.env(key, value); }
    let out = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
    (out.status.code().unwrap(), serde_json::from_slice(&out.stdout).unwrap_or(Value::Null), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Com o servidor fora do ar o comando resolve dos mesmos arquivos e chega ao que o Python decidiu
/// (`create_inherit_from_headless_creator`: conta, modo e sem terminal vindos do sidecar da criadora).
/// O ambiente de quem chama não entra: conta (`CLAUDE_CONFIG_DIR`) e `CP_*` são os do servidor.
#[tokio::test]
async fn the_command_resolves_from_the_same_files_with_the_server_down() {
    let dir = tempfile::tempdir().unwrap();
    // Raiz canônica: o catálogo de contas compara o caminho resolvido.
    let root = dir.path().canonicalize().unwrap();
    // O prefixo `\\?\` do Windows não aceita `/`, e os caminhos do fixture vêm com ela.
    let root = std::path::PathBuf::from(root.to_string_lossy().strip_prefix(r"\\?\").map_or(root.clone().into_os_string(), Into::into));
    let fixture = fixture("create_inherit_from_headless_creator");
    materialize(&fixture, &root);
    // PATH vazio: nem tmux nem agentes, como uma máquina sem sessão de terminal.
    std::fs::create_dir_all(root.join("bin")).unwrap();
    // Se o comando lesse a conta ou o `CP_JEV_PADRAO` de quem chama, o Jev nasceria ligado.
    std::fs::write(root.join("home/.claude-b/runtime-config.json"), r#"{"jev_padrao": true}"#).unwrap();
    let caller = [("CLAUDE_CONFIG_DIR", root.join("home/.claude-b").to_string_lossy().into_owned()), ("CP_JEV_PADRAO", "true".into())];
    // Porta 1: ninguém atende.
    let bin = install(&root, "CP_AUTH_TOKEN=x\nCP_PORT=1\n");
    let body = place(&fixture["scenario"]["steps"][0]["json"].to_string(), &root, true);
    let expected = &fixture["result"]["steps"][0]["body"];
    let launched = &fixture["result"]["records"]["<ROOT>/home/.hangar/claude-headless/filha.json"]["created"]["json"];
    for command in ["resolve", "open"] {
        let (code, out, stderr) = hangar_open(&bin, &root, &[command, &body], &caller).await;
        assert_eq!(code, if command == "open" { 3 } else { 0 }, "{command}: {stderr}");
        let spec = &out["spec"];
        assert_eq!(spec["name"], expected["name"]);
        assert_eq!(spec["surface"] == "headless", expected["headless"] == true);
        assert_eq!(spec["account"]["config_dir"].as_str().map(|d| d.replace(&*root.to_string_lossy(), "<ROOT>").replace('\\', "/")).as_deref(),
            expected["config_dir"].as_str(), "{command}: {out}");
        assert_eq!(spec["account_source"], expected["account_source"]);
        assert_eq!(out["warnings"], expected["avisos"]);
        assert_eq!(spec["permission_mode"], launched["permission_mode"]);
        assert_eq!(spec["jev"], launched["jev"]);
    }
}

#[tokio::test]
async fn the_command_refuses_with_the_open_error() {
    let dir = tempfile::tempdir().unwrap();
    let bin = install(dir.path(), "");
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["resolve", r#"{"name":"s1","cwd":"/x","bogus":1}"#], &[]).await;
    assert_eq!(code, 1);
    assert_eq!(serde_json::from_str::<Value>(&stderr).unwrap()["detail"]["code"], "erro_corpo_invalido");
    assert_eq!(hangar_open(&bin, dir.path(), &["abrir"], &[]).await.0, 2);
}

/// A recusa do servidor sai em stderr, como a recusa local; stdout fica só com a sessão criada.
#[tokio::test]
async fn a_server_refusal_goes_to_stderr() {
    let refusal = r#"{"detail":{"code":"erro_nome_em_uso","params":{},"msg":"ja existe uma sessao com esse nome"}}"#;
    let app = axum::Router::new().route("/api/sessions", axum::routing::post(move || async move {
        (axum::http::StatusCode::CONFLICT, refusal)
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let bin = install(dir.path(), &format!("CP_AUTH_TOKEN=t\nCP_PORT={port}\n"));
    let out = {
        let mut command = Command::new(&bin);
        clean_env(&mut command).args(["open", r#"{"name":"dup","cwd":"/x"}"#]).env("HOME", dir.path());
        tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap()
    };
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), refusal);
}

/// Instalado em `~/.hangar/bin` (sem `backend/` acima), o comando acha o `.env` pelo ponteiro que o
/// servidor grava ao subir (`~/.hangar/backend-dir`).
#[tokio::test]
async fn the_installed_command_finds_the_env_file_through_the_pointer() {
    let (fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bin = home.join(".hangar/bin/hangar-server");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    crate::copy_executable(Path::new(env!("CARGO_BIN_EXE_hangar-server")), &bin);
    let backend = dir.path().join("checkout/backend");
    std::fs::create_dir_all(&backend).unwrap();
    std::fs::write(backend.join(".env"), format!("CP_AUTH_TOKEN=do-ponteiro\nCP_PORT={}\n", up.port())).unwrap();
    hangar_server::session_open::cli::publish_backend_dir(&home, &backend).unwrap();
    let body = r#"{"name":"s1","cwd":"/x","provider":"claude"}"#;
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", body], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fake.last_hit().1["authorization"], "Bearer do-ponteiro");
}

/// Máquina migrada: o `.env` mora em `~/.hangar/config/` e vence o da `backend/`; a `backend/` da
/// cópia continua achada pelo `app/` mesmo sem `.env` nela.
#[tokio::test]
async fn the_env_file_in_the_user_data_folder_wins() {
    let (fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("home/.hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".env"), format!("CP_AUTH_TOKEN=do-config\nCP_PORT={}\n", up.port())).unwrap();
    let body = r#"{"name":"s1","cwd":"/x","provider":"claude"}"#;
    let bin = install(dir.path(), "CP_AUTH_TOKEN=da-backend\nCP_PORT=1\n");
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", body], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fake.last_hit().1["authorization"], "Bearer do-config");
    std::fs::remove_file(dir.path().join("inst/backend/.env")).unwrap();
    std::fs::create_dir_all(dir.path().join("inst/backend/app")).unwrap();
    // `app/main.py` é comum a qualquer projeto Python: só ele não faz da pasta a do Hangar.
    std::fs::write(dir.path().join("inst/backend/app/main.py"), "").unwrap();
    assert!(!hangar_server::session_open::cli::is_backend(&dir.path().join("inst/backend")));
    std::fs::write(dir.path().join("inst/backend/app/rust_server.py"), "").unwrap();
    assert!(hangar_server::session_open::cli::is_backend(&dir.path().join("inst/backend")));
    assert!(!hangar_server::session_open::cli::is_backend(&dir.path().join("inst/bin")));
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", body], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fake.hits_to("/api/sessions"), 2);
}

/// Instalado em `~/.hangar/bin` sem ponteiro, o comando acha o token no `~/.hangar/config/.env`.
#[tokio::test]
async fn the_installed_command_finds_the_env_file_in_the_user_data_folder() {
    let (fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bin = home.join(".hangar/bin/hangar-server");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    crate::copy_executable(Path::new(env!("CARGO_BIN_EXE_hangar-server")), &bin);
    std::fs::create_dir_all(home.join(".hangar/config")).unwrap();
    std::fs::write(home.join(".hangar/config/.env"), format!("CP_AUTH_TOKEN=so-config\nCP_PORT={}\n", up.port())).unwrap();
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", r#"{"name":"s1","cwd":"/x","provider":"claude"}"#], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fake.last_hit().1["authorization"], "Bearer so-config");
}

/// Token e porta vêm do `backend/.env`, como no `hangar-send`; sem token nada é enviado.
#[tokio::test]
async fn the_env_file_gives_token_and_port_and_no_token_sends_nothing() {
    let (fake, up) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let body = r#"{"name":"s1","cwd":"/x","provider":"claude"}"#;
    let bin = install(dir.path(), &format!("CP_PORT={}\n", up.port()));
    // Nem o token nem o endereço de quem chama valem: só o `.env` do servidor.
    let caller = [("CP_AUTH_TOKEN", "do-chamador".into()), ("HANGAR_URL", format!("http://{up}"))];
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", body], &caller).await;
    assert_eq!(code, 4, "{stderr}");
    assert!(stderr.contains("CP_AUTH_TOKEN"), "{stderr}");
    assert_eq!(fake.hits_to("/api/sessions"), 0, "sem token, nada foi enviado");
    let bin = install(dir.path(), &format!("CP_AUTH_TOKEN=velho\nCP_PORT={}\nCP_AUTH_TOKEN=do-env\n", up.port()));
    let (code, _, stderr) = hangar_open(&bin, dir.path(), &["open", body], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fake.hits_to("/api/sessions"), 1);
    assert_eq!(fake.last_hit().1["authorization"], "Bearer do-env", "a última linha do .env vale");
    assert_eq!(fake.last_body(), body.as_bytes());
}

/// Com o servidor fora do ar, `hangar-server open` abre o Claude com terminal aqui mesmo, pela
/// mesma biblioteca (o tmux é um programa falso que grava o que recebe), e deixa o spec gravado para o
/// servidor adotar a sessão quando voltar.
#[cfg(unix)]
#[tokio::test]
async fn the_command_opens_claude_with_terminal_locally_with_the_server_down() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let fixture = fixture("create_claude_terminal_default");
    materialize(&fixture, &root);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let tmux = root.join("bin/tmux");
    std::fs::write(&tmux, "#!/bin/sh\nd=${0%/*}\necho \"$@\" >> \"$d/tmux.log\"\ncase \"$1\" in\n  new-session) : > \"$d/alive\" ;;\n  has-session) [ -f \"$d/alive\" ] ;;\n  list-panes) echo 'no server running on /tmp/tmux-teste/default' >&2; exit 1 ;;\nesac\n").unwrap();
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin = install(&root, "CP_AUTH_TOKEN=x\nCP_PORT=1\n");
    let body = fixture["scenario"]["steps"][0]["json"].to_string().replace("<ROOT>", &root.to_string_lossy());
    let (code, out, stderr) = hangar_open(&bin, &root, &["open", &body], &[]).await;
    assert_eq!(code, 0, "{stderr}");
    assert_eq!((out["name"].as_str(), out["provider"].as_str(), out["headless"].as_bool()), (Some("s1"), Some("claude"), Some(false)));
    let log = std::fs::read_to_string(root.join("bin/tmux.log")).unwrap();
    let created = log.lines().find(|l| l.starts_with("new-session")).unwrap();
    assert!(created.contains("CP_PROVIDER=claude") && created.contains("CLAUDE_CODE_NO_FLICKER=1"), "{created}");
    let key = created.split("CP_SESSION_KEY=").nth(1).unwrap().split(' ').next().unwrap();
    let spec: Value = serde_json::from_slice(&std::fs::read(root.join(format!("home/.hangar/session-specs/{key}.json"))).unwrap()).unwrap();
    assert_eq!((spec["name"].as_str(), spec["surface"].as_str()), (Some("s1"), Some("terminal")));
    let trusted: Value = serde_json::from_slice(&std::fs::read(root.join("home/.claude.json")).unwrap()).unwrap();
    assert_eq!(trusted["projects"][root.join("work/proj").to_string_lossy().as_ref()]["hasTrustDialogAccepted"], true);
}
