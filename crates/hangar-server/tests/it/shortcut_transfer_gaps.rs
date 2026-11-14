//! Atalhos: os ramos que a conferência achou sem caso. Os valores esperados saem do Python da ponta
//! 7ef9011ba (`shortcut_transfer.py`, `shortcut_scripts.py`, `runtime_config.py`), rodado à parte
//! com cada entrada abaixo; o nome do teste diz os itens ATA.
use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;

use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::routes::AppState;
use hangar_server::shortcut_transfer::ServerHost;
use hangar_shortcut_transfer::{Error, Host, ImportRequest, Machine, Snapshot, export, has_placeholder, import, scrub};

use crate::fake::OWNER;

#[cfg(target_os = "linux")]
const EXCLUDED: &str = "Uma dependência local foi excluída: arquivo ausente, tipo, caminho ou tamanho não permitido.";
#[cfg(target_os = "linux")]
const CREDENTIAL: &str = "Arquivo de credencial é dependência externa; não foi lido nem incluído.";
const VERIFY_WARNING: &str = "Importar roda o comando de verificação de cada atalho logo depois: confira os comandos acima.";

struct Scene {
    _root: tempfile::TempDir,
    home: PathBuf,
    state: Arc<AppState>,
    app: axum::Router,
}

impl Scene {
    fn new() -> Self {
        Self::with_shell(Some("/bin/sh"))
    }

    fn with_shell(shell: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mut env = BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ]);
        if let Some(shell) = shell {
            env.insert("SHELL".to_owned(), shell.to_owned());
        }
        let mut state = AppState::new(crate::fake::config(crate::refused_address(), "127.0.0.1"));
        state.accounts = AccountService::new(AccountEnvironment::from_map(env));
        let state = Arc::new(state);
        let app = hangar_server::routes::router(state.clone());
        Self { _root: root, home, state, app }
    }

    fn home(&self) -> String {
        self.home.to_string_lossy().into_owned()
    }

    fn config_path(&self) -> PathBuf {
        self.home.join(".claude/runtime-config.json")
    }

    /// `shortcuts` gravado como o Python (`json.dumps(items)`), sem passar pela validação.
    fn save(&self, items: Value) {
        let text = hangar_core::pycompat::pyjson::dumps(&items, false);
        std::fs::create_dir_all(self.config_path().parent().unwrap()).unwrap();
        std::fs::write(self.config_path(), serde_json::to_string_pretty(&json!({"shortcuts": text})).unwrap()).unwrap();
    }

    fn saved(&self) -> String {
        let config: Value = serde_json::from_slice(&std::fs::read(self.config_path()).unwrap()).unwrap();
        config["shortcuts"].as_str().unwrap().to_owned()
    }

    fn script(&self, relative: &str, content: &[u8], mode: u32) -> PathBuf {
        let target = self.home.join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = mode;
        target
    }

    fn host(&self) -> ServerHost {
        ServerHost(self.state.clone())
    }

    fn request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {OWNER}"));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let mut request = builder.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
        request
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        reply(self.app.clone().oneshot(Self::request(method, uri, body)).await.unwrap()).await
    }

    async fn export(&self, query: &str) -> (u16, Value) {
        self.send("GET", &format!("/api/shortcuts/export{query}"), None).await
    }

    async fn import(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/api/shortcuts/import", Some(body)).await
    }

    async fn verify(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/api/shortcuts/verify", Some(body)).await
    }
}

async fn reply(response: axum::response::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 64 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn invalid(code: &str, motivo: &str) -> Value {
    json!({"detail": {"code": code, "params": {"motivo": motivo}, "msg": motivo}})
}

fn fake(prefix: &str) -> String {
    format!("{prefix}{}", "x1Y2".repeat(7))
}

fn shell(command: &str) -> Value {
    json!([{"id": "a", "type": "shell", "label": "A", "command": command}])
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Tudo o que `run` registrou no log, em qualquer nível.
fn logged<T>(run: impl FnOnce() -> T) -> (T, String) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let result = tracing::subscriber::with_default(subscriber, run);
    let bytes = log.0.lock().unwrap().clone();
    (result, String::from_utf8_lossy(&bytes).into_owned())
}

/// O servidor com a leitura da configuração quebrada: o verify falha por dentro.
struct Unreadable(ServerHost);

impl Host for Unreadable {
    fn machine(&self) -> Machine {
        self.0.machine()
    }
    fn shortcuts(&self) -> Result<Value, Error> {
        Err(Error::Io("disco".into()))
    }
    fn validate(&self, text: &str) -> Result<(), String> {
        self.0.validate(text)
    }
    fn update(&self, change: &mut dyn FnMut(&Value) -> Result<String, Error>) -> (Option<Snapshot>, Result<(), Error>) {
        self.0.update(change)
    }
    fn undo(&self, serialized: &str, before: &Snapshot) -> Result<(), String> {
        self.0.undo(serialized, before)
    }
    fn login_path(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'static>> {
        self.0.login_path()
    }
}

// ── scrub: variantes de cada regra (ATA-14, 16, 17, 20, 21) e nome do marcador (ATA-11) ──────────

/// ATA-14/16/17/20/21: `(texto, fish, shell)` → `(saída, nomes)` do `scrub` do Python.
#[test]
fn scrub_rule_variants_match_the_python() {
    let token = |prefix: &str| format!("echo {}", fake(prefix));
    let cases: Vec<(String, bool, String, Vec<&str>)> = vec![
        // ATA-14: todos os nomes de flag; o mais longo vence (`--access-token` não é `--token`).
        ("a --passwd x1".into(), true, "a --passwd ⟦SEGREDO:senha⟧".into(), vec!["senha"]),
        ("a --pass x1".into(), true, "a --pass ⟦SEGREDO:senha⟧".into(), vec!["senha"]),
        ("a --access-token=x1".into(), true, "a --access-token=⟦SEGREDO:token⟧".into(), vec!["token"]),
        ("a --auth-token x1".into(), true, "a --auth-token ⟦SEGREDO:token⟧".into(), vec!["token"]),
        ("a --apikey x1".into(), true, "a --apikey ⟦SEGREDO:api_key⟧".into(), vec!["api_key"]),
        ("a --client-secret=x1".into(), true, "a --client-secret=⟦SEGREDO:segredo⟧".into(), vec!["segredo"]),
        // A flag casa só em minúsculas, como no Python.
        ("a --PASSWORD x1".into(), true, "a --PASSWORD x1".into(), vec![]),
        // ATA-16: os parentes do mysql.
        ("mysqldump -u root -pS3 db".into(), true, "mysqldump -u root -p⟦SEGREDO:senha⟧ db".into(), vec!["senha"]),
        ("mysqladmin -pS3 status".into(), true, "mysqladmin -p⟦SEGREDO:senha⟧ status".into(), vec!["senha"]),
        ("mariadb -pS3 db".into(), true, "mariadb -p⟦SEGREDO:senha⟧ db".into(), vec!["senha"]),
        ("mariadb-dump -pS3 db".into(), true, "mariadb-dump -p⟦SEGREDO:senha⟧ db".into(), vec!["senha"]),
        // ATA-17: Basic, Token e caixa diferente.
        (r#"curl -H "Authorization: Basic YWJj" https://x"#.into(), true, r#"curl -H "Authorization: Basic ⟦SEGREDO:token⟧" https://x"#.into(), vec!["token"]),
        (r#"curl -H "Authorization: Token abc" https://x"#.into(), true, r#"curl -H "Authorization: Token ⟦SEGREDO:token⟧" https://x"#.into(), vec!["token"]),
        (r#"curl -H "authorization: bearer abc" https://x"#.into(), true, r#"curl -H "authorization: bearer ⟦SEGREDO:token⟧" https://x"#.into(), vec!["token"]),
        // ATA-21: os demais prefixos de chave solta.
        (token("sk-"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("sk-ant-"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("gho_"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("ghu_"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("ghs_"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("ghr_"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (token("github_pat_"), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (format!("echo xoxb-{}", "1a2b".repeat(3)), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (format!("echo AKIA{}", "X1Y2".repeat(4)), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        (format!("echo eyJ{}.eyJ{}.{}", "ab12".repeat(3), "cd34".repeat(3), "ef56".repeat(3)), true, "echo ⟦SEGREDO:token⟧".into(), vec!["token"]),
        // ATA-20: `set NOME valor` só vale com `shell=True`.
        ("set PASSWORD abc".into(), false, "set PASSWORD abc".into(), vec![]),
        ("set PASSWORD abc".into(), true, "set PASSWORD ⟦SEGREDO:PASSWORD⟧".into(), vec!["PASSWORD"]),
    ];
    for (text, shell, out, names) in cases {
        assert_eq!(scrub(&text, false, shell), (out, names.into_iter().map(str::to_owned).collect()), "{text:?} shell={shell}");
    }
}

/// ATA-11: nome de marcador com `.` e `-` (`[A-Za-z0-9_.-]+`) é achado e preenchido no import.
#[tokio::test(flavor = "multi_thread")]
async fn placeholder_names_take_dots_and_hyphens() {
    assert_eq!(has_placeholder(Some("x ⟦SEGREDO:db.pass-1⟧ ⟦SEGREDO:b⟧")).as_deref(), Some("db.pass-1"));
    let scene = Scene::new();
    let body = json!({"data": [{"id": "a", "type": "shell", "label": "A", "command": "x ⟦SEGREDO:db.pass-1⟧"}],
        "apply": true, "secrets": {"a": {"db.pass-1": "s3"}}});
    assert_eq!(scene.import(body).await, (200, json!({"added": 1, "replaced": 0, "placeholders": [], "verify": [], "verify_commands": []})));
    assert_eq!(scene.saved(), r#"[{"id": "a", "type": "shell", "label": "A", "command": "x s3"}]"#);
}

// ── segredo fora do log (ATA-05, ATA-10) ──────────────────────────────────────────────────────────

/// ATA-05, ATA-10: importar (aplicado, prévia e recusado) e exportar não registram o valor do
/// segredo, nem o que veio em `secrets` nem o achado pela varredura.
#[test]
fn import_and_export_never_log_the_secret() {
    let scene = Scene::new();
    let host = scene.host();
    let filled = "valor-sigiloso-QWZ";
    let literal = "literal-sigiloso-PLK";
    let refused = "falha-sigilosa-RTY";
    let saved = "outro-sigiloso-MNB";
    let request = |data: Value, apply: bool, secrets: Value| ImportRequest {
        data,
        apply,
        secrets: secrets.as_object().cloned().unwrap_or_default(),
    };
    let (result, log) = logged(|| {
        let applied = import(&host, request(
            json!([{"id": "rdp", "type": "shell", "label": "RDP", "command": "x /p:⟦SEGREDO:senha⟧"},
                   {"id": "cli", "type": "shell", "label": "CLI", "command": format!("cli --token {literal}")}]),
            true,
            json!({"rdp": {"senha": filled}}),
        ));
        let preview = import(&host, request(json!([{"id": "p", "type": "shell", "label": "P", "command": format!("cli --token {literal}")}]),
            false, json!({"p": {"token": filled}})));
        let failed = import(&host, request(json!([{"id": "f", "type": "foguete"}]), true, json!({"f": {"senha": refused}})));
        (applied, preview, failed)
    });
    let (applied, preview, failed) = result;
    assert!(applied.is_ok() && preview.is_ok() && failed.is_err(), "{applied:?} {preview:?} {failed:?}");
    assert!(scene.saved().contains(filled), "o valor chega à configuração");
    for secret in [filled, literal, refused] {
        assert!(!log.contains(secret), "log com {secret}: {log}");
    }

    scene.save(shell(&format!("xfreerdp /p:{saved}")));
    let (exported, log) = logged(|| export(&host, None, true, || "/usr/bin:/bin".to_owned()));
    let exported = exported.unwrap();
    assert_eq!(exported["shortcuts"][0]["command"], "xfreerdp /p:⟦SEGREDO:senha⟧");
    assert!(!log.contains(saved), "log com {saved}: {log}");
}

// ── export: limites e casos sem prova (ATA-39, 50, 56, 57, 58, 60) ──────────────────────────────────

/// ATA-56: link, arquivo com byte nulo e arquivo que não é UTF-8 ficam fora, cada um com o aviso.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_excludes_links_binary_and_non_utf8_files() {
    let scene = Scene::new();
    scene.script(".local/share/real", b"#!/bin/sh\necho real", 0o700);
    scene.script(".local/bin/nul", b"#!/bin/sh\necho a\0b", 0o700);
    std::os::unix::fs::symlink(scene.home.join(".local/share/real"), scene.home.join(".local/bin/lnk")).unwrap();
    scene.script(".local/bin/bad", b"#!/bin/sh\necho \xff\xfe", 0o700);
    scene.save(shell("lnk; nul; bad"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("lnk; nul; bad"), "scripts": [],
        "warnings": [
            format!("{EXCLUDED} Caminho: ⟦HOME⟧/.local/bin/lnk"),
            format!("{EXCLUDED} Caminho: ⟦HOME⟧/.local/bin/nul"),
            format!("{EXCLUDED} Caminho: ⟦HOME⟧/.local/bin/bad"),
        ], "removed": 0})));
}

/// ATA-57: o total dos scripts tem teto de 1 MiB; o que passaria do teto fica fora com aviso e o
/// seguinte, que cabe, entra.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_total_limit_is_one_mebibyte() {
    let scene = Scene::new();
    let big = vec![b'a'; 120 * 1024];
    let names: Vec<String> = (0..9).map(|i| format!("b{i}")).chain(["small".to_owned()]).collect();
    for name in &names[..9] {
        scene.script(&format!(".local/bin/{name}"), &big, 0o700);
    }
    scene.script(".local/bin/small", b"echo", 0o700);
    scene.save(shell(&names.join("; ")));
    let (status, body) = scene.export("").await;
    assert_eq!(status, 200, "{body}");
    let paths: Vec<&str> = body["scripts"].as_array().unwrap().iter().map(|s| s["path"].as_str().unwrap()).collect();
    let expected: Vec<String> = (0..8).map(|i| format!(".local/bin/b{i}")).chain([".local/bin/small".to_owned()]).collect();
    assert_eq!(paths, expected);
    assert_eq!(body["warnings"], json!([format!("{EXCLUDED} Caminho: ⟦HOME⟧/.local/bin/b8")]));
    assert_eq!(body["removed"], 0);
}

/// ATA-60: arquivo sem bit de execução vai com `executable: false`.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_marks_a_file_without_the_execute_bit() {
    let scene = Scene::new();
    scene.script(".config/fish/functions/f.fish", b"function f\n echo\nend", 0o644);
    scene.save(shell("f"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("f"),
        "scripts": [{"path": ".config/fish/functions/f.fish", "content": "function f\n echo\nend", "executable": false}],
        "warnings": [], "removed": 0})));
}

/// ATA-58: o " Caminho:" só entra com caminho literal seguro: até 400 caracteres, só `[\w./-]`, sem
/// `..`, e o aviso inteiro até 1024 bytes.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_names_the_path_only_when_it_is_safe_and_short() {
    let at_limit = format!("/tmp/{}.pw", "a".repeat(392));
    let cases = [
        (at_limit.clone(), format!("{CREDENTIAL} Caminho: {at_limit}")),
        (format!("/tmp/{}.pw", "a".repeat(393)), CREDENTIAL.to_owned()),
        ("/tmp/a+b.pw".to_owned(), CREDENTIAL.to_owned()),
        ("/tmp/../x.pw".to_owned(), CREDENTIAL.to_owned()),
        (format!("/tmp/{}.pw", "字".repeat(390)), CREDENTIAL.to_owned()),
    ];
    for (token, warning) in cases {
        let scene = Scene::new();
        let command = format!("cat {token}");
        scene.save(shell(&command));
        let (status, body) = scene.export("").await;
        assert_eq!((status, &body["warnings"]), (200, &json!([warning])), "{token}");
    }
}

/// ATA-50: cada nome de credencial vira aviso com o caminho e nunca é lido.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_warns_on_every_credential_name() {
    let scene = Scene::new();
    let names = [".ssh/config", ".aws/c", ".gnupg/k", "credentials", "secrets", "id_rsa", "id_ed25519", ".env", ".env.local",
        "x.pem", "x.key", "Y.PEM"];
    for name in names {
        scene.script(name, b"NEVER_READ", 0o600);
    }
    let command = format!("cat {}", names.iter().map(|n| format!("~/{n}")).collect::<Vec<_>>().join(" "));
    scene.save(shell(&command));
    let warnings: Vec<String> = names.iter().map(|n| format!("{CREDENTIAL} Caminho: ⟦HOME⟧/{n}")).collect();
    let (status, body) = scene.export("").await;
    assert_eq!((status, &body), (200, &json!({"version": 2, "shortcuts": shell(&command), "scripts": [], "warnings": warnings, "removed": 0})));
    assert!(!body.to_string().contains("NEVER_READ"));
}

/// ATA-39 (`test_export_subset_recurses_scripts_fish_and_never_reads_password`): sem
/// `include_scripts`, a dependência que existe no disco não é coletada nem gera aviso.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_without_scripts_reads_no_dependency() {
    let scene = Scene::new();
    scene.script(".local/bin/main", b"#!/bin/sh\nexec second", 0o700);
    scene.script(".local/bin/second", b"NEVER_READ", 0o700);
    scene.save(shell("main ~/.local/x.pw"));
    let (status, body) = scene.export("?include_scripts=false").await;
    assert_eq!((status, &body), (200, &json!({"version": 2, "shortcuts": shell("main ~/.local/x.pw"), "scripts": [], "warnings": [], "removed": 0})));
    assert!(!body.to_string().contains("NEVER_READ"));
}

#[cfg(target_os = "linux")]
fn script_entry(path: &str, content: &str, executable: bool) -> Value {
    json!({"path": path, "content": content, "executable": executable})
}

/// ATA-40: os avisos do export passam pela varredura: o caminho junto pelo shlex (`gh"p_…"`) leva uma
/// chave que a varredura do comando não pegou, e o aviso sai com o marcador.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_scrubs_its_warnings() {
    let scene = Scene::new();
    let command = format!("/opt/gh\"p_{}\"/y", "x1Y2".repeat(7));
    scene.save(shell(&command));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell(&command), "scripts": [],
        "warnings": ["Dependência fora das pastas locais permitidas não foi incluída. Caminho: /opt/⟦SEGREDO:token⟧/y"],
        "removed": 0})));
}

/// ATA-44: função fish vence o homônimo no PATH, e `~/bin` entra na busca mesmo fora do PATH.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_prefers_the_fish_function_and_searches_home_bin() {
    let scene = Scene::new();
    scene.script(".local/bin/tool", b"echo bin", 0o700);
    scene.script(".config/fish/functions/tool.fish", b"function tool\n echo fish\nend", 0o644);
    scene.script("bin/other", b"echo other", 0o700);
    scene.save(shell("tool; other"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("tool; other"), "scripts": [
        script_entry(".config/fish/functions/tool.fish", "function tool\n echo fish\nend", false),
        script_entry("bin/other", "echo other", true),
    ], "warnings": [], "removed": 0})));
}

/// ATA-45: `\` + quebra de linha junta as linhas (`next` é argumento, não comando) e nada do script é
/// executado para achar dependência.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_joins_continued_lines_and_runs_nothing() {
    let scene = Scene::new();
    let main = "#!/bin/sh\necho \\\nnext\ntouch ~/executado\n";
    scene.script(".local/bin/main", main.as_bytes(), 0o700);
    scene.script(".local/bin/next", b"echo next", 0o700);
    scene.save(shell("main"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("main"),
        "scripts": [script_entry(".local/bin/main", main, true)], "warnings": [], "removed": 0})));
    assert!(!scene.home.join("executado").exists());
}

/// ATA-46: atribuição e palavra de controle puladas; `exec`/`command`/`env` com flags e atribuições
/// pulados; `function`/`fi` ignorados; `source` e `.` incluem o arquivo (o `.` corrige o Python, que o
/// pulava: decisão do usuário); interpretador inclui o 1º argumento, e com flag avisa (`--version` não).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_reads_each_segment_like_the_python() {
    let scene = Scene::new();
    let main = "#!/bin/sh\nFOO=1 dep1\nexec dep2\ncommand dep3\nenv -i X=1 dep4\nfunction f\nend\nsh dep5 arg\npython3 -u x.py\nbash --version\nsource dep6\n. dep7\nif dep8; then\nfi\n";
    scene.script(".local/bin/main", main.as_bytes(), 0o700);
    for dep in ["dep1", "dep2", "dep3", "dep4", "dep5", "dep6", "dep7", "dep8"] {
        scene.script(&format!(".local/bin/{dep}"), b"echo", 0o700);
    }
    scene.save(shell("main"));
    let mut scripts = vec![script_entry(".local/bin/main", main, true)];
    scripts.extend(["dep1", "dep2", "dep3", "dep4", "dep5", "dep6", "dep7", "dep8"].map(|dep| script_entry(&format!(".local/bin/{dep}"), "echo", true)));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("main"), "scripts": scripts,
        "warnings": ["Comando de interpretador com opções precisa de conferência manual das dependências."], "removed": 0})));
}

/// ATA-51: `$HOME`, `${HOME}` e `~/` expandidos, também no caminho do aviso; coringa é dinâmico.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_expands_home_and_warns_on_dynamic_tokens() {
    let scene = Scene::new();
    let main = "#!/bin/sh\nsource $HOME/.local/bin/lib1\nsource ${HOME}/.local/bin/lib2\nsource ~/.local/bin/lib3\nsource ~/.local/bin/x*\ncat $HOME/.local/vm.pw\n";
    scene.script(".local/bin/main", main.as_bytes(), 0o700);
    for lib in ["lib1", "lib2", "lib3"] {
        scene.script(&format!(".local/bin/{lib}"), b"echo lib", 0o700);
    }
    scene.save(shell("main"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("main"), "scripts": [
        script_entry(".local/bin/main", main, true),
        script_entry(".local/bin/lib1", "echo lib", true),
        script_entry(".local/bin/lib2", "echo lib", true),
        script_entry(".local/bin/lib3", "echo lib", true),
    ], "warnings": [
        format!("{CREDENTIAL} Caminho: ⟦HOME⟧/.local/vm.pw"),
        "Dependências dinâmicas precisam de conferência manual; nenhum comando foi executado.",
    ], "removed": 0})));
}

/// ATA-54: cada arquivo uma vez, mesmo citado duas vezes e com ciclo (`a` → `b` → `a`).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_visits_each_file_once() {
    let scene = Scene::new();
    scene.script(".local/bin/a", b"#!/bin/sh\nsource ~/.local/bin/b\n", 0o700);
    scene.script(".local/bin/b", b"#!/bin/sh\nsource ~/.local/bin/a\na\n", 0o700);
    scene.save(shell("a; a"));
    assert_eq!(scene.export("").await, (200, json!({"version": 2, "shortcuts": shell("a; a"), "scripts": [
        script_entry(".local/bin/a", "#!/bin/sh\nsource ~/.local/bin/b\n", true),
        script_entry(".local/bin/b", "#!/bin/sh\nsource ~/.local/bin/a\na\n", true),
    ], "warnings": [], "removed": 0})));
}

/// ATA-56: FIFO no PATH não é arquivo regular e fica fora com aviso. Se o export abrir o FIFO
/// esperando um escritor, um socorro abre o outro lado depois de 5 s e o teste falha em vez de travar.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn export_excludes_a_fifo_without_blocking() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let scene = Scene::new();
    let fifo = scene.home.join(".local/bin/fifo");
    std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    assert!(std::process::Command::new("mkfifo").arg("-m").arg("700").arg(&fifo).status().unwrap().success());
    scene.save(shell("fifo"));
    let done = Arc::new(AtomicBool::new(false));
    let rescued = Arc::new(AtomicBool::new(false));
    let rescuer = {
        let (done, rescued, fifo) = (done.clone(), rescued.clone(), fifo.clone());
        std::thread::spawn(move || {
            let start = Instant::now();
            while !done.load(Ordering::SeqCst) {
                // O_NONBLOCK: abrir para escrita só dá certo com um leitor preso do outro lado.
                if start.elapsed() > Duration::from_secs(5)
                    && std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo).is_ok()
                {
                    rescued.store(true, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    let exported = tokio::time::timeout(Duration::from_secs(60), scene.export("")).await;
    done.store(true, Ordering::SeqCst);
    rescuer.join().unwrap();
    assert!(!rescued.load(Ordering::SeqCst), "o export ficou preso abrindo o FIFO");
    assert_eq!(exported.expect("export sem resposta em 60 s"), (200, json!({"version": 2, "shortcuts": shell("fifo"), "scripts": [],
        "warnings": [format!("{EXCLUDED} Caminho: ⟦HOME⟧/.local/bin/fifo")], "removed": 0})));
}

/// ATA-55: destino com `\` e destino que existe e não é arquivo regular recusam antes de gravar.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn import_refuses_backslash_and_non_regular_destinations() {
    let scene = Scene::new();
    std::fs::create_dir_all(scene.home.join(".local/bin/dir")).unwrap();
    for (path, motivo) in [(".local/bin/a\\b", "caminho de script inválido"), (".local/bin/dir", "destino do script não é um arquivo regular")] {
        let data = json!({"version": 2, "shortcuts": [], "scripts": [{"path": path, "content": "echo", "executable": true}]});
        for apply in [false, true] {
            assert_eq!(scene.import(json!({"data": data, "apply": apply})).await, (400, invalid("erro_shortcut_import_invalido", motivo)), "{path}");
        }
    }
    assert!(scene.home.join(".local/bin/dir").is_dir());
    assert!(!scene.home.join(".local/bin/a\\b").exists());
    assert!(!scene.config_path().exists());
}

// ── import (ATA-72, ATA-78) ─────────────────────────────────────────────────────────────────────

/// ATA-78: a lista juntada (salva + importada) é revalidada inteira; atalho inválido que já estava
/// salvo recusa a prévia e a aplicação sem gravar, e substituí-lo pelo id passa.
#[tokio::test(flavor = "multi_thread")]
async fn import_revalidates_the_merged_list() {
    let scene = Scene::new();
    scene.save(json!([{"id": "old", "type": "send_text", "label": "Old", "text": "  "}]));
    let before = std::fs::read(scene.config_path()).unwrap();
    let refused = (400, invalid("erro_shortcut_import_invalido", "shortcuts: item 1 (send_text) sem texto a enviar"));
    let incoming = json!([{"id": "new", "type": "send_text", "label": "N", "text": "oi"}]);
    assert_eq!(scene.import(json!({"data": incoming})).await, refused);
    assert_eq!(scene.import(json!({"data": incoming, "apply": true})).await, refused);
    assert_eq!(std::fs::read(scene.config_path()).unwrap(), before);
    let replacing = json!([{"id": "old", "type": "send_text", "label": "Old", "text": "oi"}]);
    assert_eq!(scene.import(json!({"data": replacing, "apply": true})).await,
        (200, json!({"added": 0, "replaced": 1, "placeholders": [], "verify": [], "verify_commands": []})));
    assert_eq!(scene.saved(), r#"[{"id": "old", "type": "send_text", "label": "Old", "text": "oi"}]"#);
}

/// ATA-72: no pacote versão 2, `⟦HOME⟧` em `command` e `verify` vira o HOME de destino.
#[tokio::test(flavor = "multi_thread")]
async fn import_v2_resolves_home_in_command_and_verify() {
    let scene = Scene::new();
    let home = scene.home();
    let data = json!({"version": 2, "shortcuts": [{"id": "a", "type": "shell", "label": "A", "command": "⟦HOME⟧/bin/x",
        "verify": "test -f ⟦HOME⟧/y"}], "scripts": []});
    let (status, body) = scene.import(json!({"data": data, "apply": true})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verify_commands"], json!([{"id": "a", "label": "A", "command": format!("test -f {home}/y")}]));
    assert_eq!(body["warnings"], json!([VERIFY_WARNING]));
    // Serializado pelo mesmo gravador: no Windows o HOME tem `\`, que o JSON escapa.
    let expected = json!([{"id": "a", "type": "shell", "label": "A", "command": format!("{home}/bin/x"), "verify": format!("test -f {home}/y")}]);
    assert_eq!(scene.saved(), hangar_core::pycompat::pyjson::dumps(&expected, false));
}

// ── verify (ATA-92, ATA-93, ATA-B4) ──────────────────────────────────────────────────────────────

/// ATA-92: sem `SHELL` o verify roda em `/bin/sh -c`, e o stderr vem junto do stdout.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn verify_falls_back_to_bin_sh_and_merges_stderr() {
    let scene = Scene::with_shell(None);
    let command = "echo $0; echo erro 1>&2";
    scene.save(json!([{"id": "a", "type": "shell", "label": "A", "command": "true", "verify": command}]));
    assert_eq!(scene.verify(json!({"ids": ["a"]})).await, (200, json!({"checks": [
        {"id": "a", "label": "A", "command": command, "code": 0, "output": "/bin/sh\nerro"}
    ], "skipped": []})));
}

/// ATA-93: o prazo do verify da rota é 60 s.
#[test]
fn verify_timeout_is_sixty_seconds() {
    assert_eq!(hangar_shortcut_transfer::VERIFY_TIMEOUT, std::time::Duration::from_secs(60));
}

/// ATA-B4, D7.5: falha inesperada no verify é 500 com código pela rota, não 500 cru.
#[tokio::test(flavor = "multi_thread")]
async fn verify_failure_is_500_with_code_on_the_route() {
    let scene = Scene::new();
    let edge = hangar_core::edge::Edge {
        trusted: scene.state.cfg.trusted.clone(),
        auth: scene.state.auth.clone(),
        diag: scene.state.diag.clone(),
    };
    let request = Scene::request("POST", "/api/shortcuts/verify", Some(json!({"ids": ["a"]})));
    let peer: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let response = hangar_shortcut_transfer::handle(edge, Arc::new(Unreadable(scene.host())), Some(peer), request).await;
    assert_eq!(reply(response).await, (500, invalid("erro_shortcut_verify_io", "disco")));
}

// ── marcadores de caminho desta máquina (ATA-99) ─────────────────────────────────────────────────

/// ATA-99 (`test_marcador_hangar_reconhece_current_e_versao`): HANGAR é o `current` do pacote, sem
/// resolver o link, e a pasta real da versão também vira `⟦HANGAR⟧`; CLAUDE é `~/.claude` e CODEX o
/// home padrão do Codex.
#[cfg(unix)]
#[test]
fn machine_roots_follow_the_package_and_the_agent_homes() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let real = home.join(".hangar/versions/X");
    std::fs::create_dir_all(real.join("backend/hooks")).unwrap();
    let current = home.join(".hangar/current");
    std::os::unix::fs::symlink(&real, &current).unwrap();
    let text = |path: &std::path::Path| path.to_string_lossy().into_owned();
    let mut state = AppState::new(crate::fake::config(crate::refused_address(), "127.0.0.1"));
    state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
        ("HOME".to_owned(), text(&home)),
        ("USERPROFILE".to_owned(), text(&home)),
    ])));
    let roots = ServerHost(Arc::new(state)).machine().roots;
    assert_eq!(
        (roots.hangar.as_str(), roots.claude.as_str(), roots.codex.as_str(), roots.home.as_str()),
        (text(&current).as_str(), text(&home.join(".claude")).as_str(), text(&home.join(".codex")).as_str(), text(&home).as_str())
    );
    let (c, r, h) = (text(&current), text(&real), text(&home));
    let input = format!("\"{c}/backend/hooks/a.py\" {r}/scripts/b.py {h}/x {h}/.claude/hooks/y.py {h}/.codex/config.toml");
    assert_eq!(
        hangar_core::path_markers::canonicalize(&input, &roots),
        "\"⟦HANGAR⟧/backend/hooks/a.py\" ⟦HANGAR⟧/scripts/b.py ⟦HOME⟧/x ⟦CLAUDE⟧/hooks/y.py ⟦CODEX⟧/config.toml"
    );
}
