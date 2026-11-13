//! Computer Use (R1): leitura, alvos e SSH pelas rotas do Rust, com HOME e `ssh` falsos.
//! O esperado vem das capturas do Python (`contract/computer-control/r1-*.json`), com as diferenças
//! D1.x do contrato escritas à mão: `project_dir` "", `unavailable` null, sem modo local nem `uvx`.
//! `cliproxy.running` depende da porta 8317 real da máquina e fica fora da comparação.
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use hangar_core::guest::{Caller, GuestGrant};
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::routes::{AppState, router};

use crate::fake::{OWNER, config};

const PROMPT_ADMINISTRATOR: &str = r#"Configure este Windows para ser controlado pelo Hangar (MCP hangar-computer-control) a partir de outra máquina, por SSH com chave. Faça na ordem, confira cada passo e pare pra me perguntar se algo não bater.

1. Rode tudo num PowerShell elevado (como Administrador). Se não estiver elevado, me peça pra abrir um.
2. OpenSSH Server instalado, ligado e iniciando sozinho:
   Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
   Set-Service -Name sshd -StartupType Automatic; Start-Service sshd
3. Porta 22 liberada no firewall, se ainda não houver regra de entrada pra ela:
   New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
4. Autorize esta chave pública pro usuário administrator (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador logado e me diga qual é):
   ssh-ed25519 AAAAC3Nza dono@máquina
   - Usuário administrador: a chave vai em C:\ProgramData\ssh\administrators_authorized_keys, com permissão só pra Administradores e SYSTEM (use os SIDs, que valem em Windows de qualquer idioma):
     icacls.exe C:\ProgramData\ssh\administrators_authorized_keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'
   - Usuário comum: em C:\Users\<usuário>\.ssh\authorized_keys.
   Acrescente sem apagar as chaves que já estiverem lá.
5. Esse usuário precisa ser administrador (o agente sobe como tarefa agendada elevada) e ficar logado numa sessão gráfica ATIVA: tela desbloqueada e sessão RDP não desconectada. Pra deixar a sessão ativa sem cliente RDP: tscon <id> /dest:console.
6. Confira: o serviço sshd está Running, a regra de firewall existe e o arquivo de chaves contém a chave acima.

Não mude outras configurações de segurança além dessas. No fim, me diga o nome desta máquina na rede (ou o IP) e o usuário, pra eu cadastrar no Hangar."#;

const KEY: &str = "ssh-ed25519 AAAAC3Nza dono@máquina\n";
const RESOLVED: &str = "printf 'user administrator\\nhostname x\\nidentityfile ~/.ssh/id_ed25519\\n'";

enum Who {
    Owner,
    Nobody,
    Guest,
}

struct Scene {
    root: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    app: axum::Router,
    /// Pedidos que chegaram ao Python dublê (responde 418 "from-python").
    hits: Arc<AtomicUsize>,
    upstream: tokio::task::JoinHandle<()>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}

impl Scene {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let python = axum::Router::new().fallback(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                (StatusCode::IM_A_TEAPOT, "from-python")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move { axum::serve(listener, python).await.unwrap() });
        let mut state = AppState::new(config(upstream_address, ""));
        state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
            // A autoridade do servidor está no ambiente dele; o ssh filho não pode herdá-la.
            ("CP_AUTH_TOKEN".into(), "nao-pode-vazar".into()),
            ("HANGAR_INTERNAL_SECRET".into(), "nao-pode-vazar".into()),
        ])));
        let app = router(Arc::new(state));
        Self { root, home, bin, app, hits, upstream }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn pid_file(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// `ssh` falso que deixa um filho dormindo no grupo e fica preso: anota os dois pids.
    fn hanging_ssh(&self) {
        self.ssh(&format!(
            "/bin/sleep 300 &\nprintf '%s' $! > '{}'\nprintf '%s' $$ > '{}'\nexec /bin/sleep 300",
            self.pid_file("child.pid").display(),
            self.pid_file("main.pid").display()
        ));
    }

    /// O ssh e o filho que ele deixou morreram (zumbi conta como morto).
    async fn assert_group_dead(&self) {
        for name in ["main.pid", "child.pid"] {
            let pid = std::fs::read_to_string(self.pid_file(name)).unwrap();
            let mut alive = true;
            for _ in 0..50 {
                alive = process_alive(pid.trim());
                if !alive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(!alive, "{name} ({pid}) sobreviveu ao prazo");
        }
    }

    fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) {
        let path = self.home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn json_file(&self, rel: &str, value: Value) {
        self.write(rel, self.h(value).to_string());
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.home.join(rel)).ok()
    }

    /// Pacote instalado como nas capturas de alvo: binário, `install.json`, `targets/` e o exe.
    fn installed(&self) {
        self.write(".hangar/computer-control/hangar-computer-control", "\x7fELF-binario-falso");
        self.write(".hangar/computer-control/install.json", "{\n  \"tag\": \"v2.0.0\"\n}");
        std::fs::create_dir_all(self.home.join(".hangar/computer-control/targets")).unwrap();
        self.write(".hangar/computer-control/windows-agent.exe", "MZ-agente-falso");
    }

    /// Conta que o catálogo acha (`.credentials.json` + `projects/`); `age` ordena (mais novo primeiro).
    fn account(&self, name: &str, age: u64) {
        self.write(&format!("{name}/.credentials.json"), "{}");
        self.write(&format!("{name}/projects/p/s.jsonl"), "{}\n");
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000 - age);
        std::fs::File::open(self.home.join(name).join("projects")).unwrap().set_modified(modified).unwrap();
    }

    /// `ssh` falso no PATH: anota `$0` e o argv e roda `body`.
    fn ssh(&self, body: &str) {
        let args = self.root.path().join("ssh-args");
        crate::write_executable(&self.bin.join("ssh"),
            format!("#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > '{}'\n{body}\n", args.display()));
    }

    fn ssh_argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.path().join("ssh-args")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn ssh_path(&self) -> String {
        self.bin.join("ssh").to_string_lossy().into_owned()
    }

    /// `«HOME»` das capturas vira o HOME desta prova.
    fn h(&self, value: Value) -> Value {
        let home = self.home.to_string_lossy();
        match value {
            Value::String(text) => Value::String(text.replace("«HOME»", &home)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.h(v)).collect()),
            Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, self.h(v))).collect()),
            other => other,
        }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call_as(method, uri, body, Who::Owner).await
    }

    async fn call_as(&self, method: &str, uri: &str, body: Option<Value>, who: Who) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if matches!(who, Who::Owner) {
            builder = builder.header("authorization", format!("Bearer {OWNER}"));
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let mut request = builder.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        request.extensions_mut().insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
        if matches!(who, Who::Guest) {
            request.extensions_mut().insert(Caller::Guest(Arc::new(GuestGrant {
                token: "convidado".into(),
                token_hash: "hash".into(),
                sessions: HashMap::new(),
                pair_roots: vec![],
                invite: true,
            })));
        }
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self) -> (StatusCode, Value) {
        let (status, body) = self.call("GET", "/api/computer-control", None).await;
        (status, without_running(body))
    }

    async fn target(&self, body: Value) -> (StatusCode, Value) {
        let (status, body) = self.call("POST", "/api/computer-control/targets", Some(body)).await;
        (status, without_running(body))
    }

    async fn test_host(&self, body: Value) -> (StatusCode, Value) {
        self.call("POST", "/api/computer-control/test-host", Some(body)).await
    }

    async fn windows(&self, query: &str) -> (StatusCode, Value) {
        self.call("GET", &format!("/api/computer-control/windows-setup{query}"), None).await
    }

    fn assert_error(&self, got: &(StatusCode, Value), status: u16, code: &str, params: Value) {
        assert_eq!(got.0.as_u16(), status, "{}", got.1);
        assert_eq!(got.1["detail"]["code"], code, "{}", got.1);
        assert_eq!(got.1["detail"]["params"], self.h(params), "{}", got.1);
        assert!(got.1["detail"]["msg"].as_str().is_some_and(|m| !m.is_empty()), "{}", got.1);
    }
}

fn process_alive(pid: &str) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => false,
        Ok(stat) => !matches!(
            stat.rsplit_once(')').and_then(|(_, tail)| tail.split_whitespace().next()),
            Some("Z" | "X" | "x")
        ),
    }
}

fn without_running(mut body: Value) -> Value {
    if let Some(cliproxy) = body.get_mut("cliproxy").and_then(Value::as_object_mut) {
        cliproxy.remove("running");
    }
    body
}

fn merged(mut base: Value, patch: Value) -> Value {
    for (key, value) in patch.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// `r1-get-vazio` com D1.2 (`project_dir` "") e D1.5 (`unavailable` null).
fn empty_state() -> Value {
    json!({
        "agent_exe": {"path": "«HOME»/.hangar/computer-control/windows-agent.exe", "exists": false, "size": 0},
        "mode": "package",
        "installed_tag": "",
        "package_exists": false,
        "unavailable": null,
        "targets": [],
        "ssh_hosts": [],
        "local_available": false,
        "platform": "linux",
        "enabled": false,
        "project_dir": "",
        "agent_config": "",
        "agent_configs": [],
        "llm_from_jev_aux": true,
        "llm_url": "",
        "llm_model": "",
        "llm_effort": "",
        "llm_key_set": false,
        "llm_key_tail": "",
        "jev_key_set": false,
        "jev_key_tail": "",
        "jev_endpoint": "",
        "jev_model": "",
        "cliproxy": {"preset_url": "http://127.0.0.1:8317/v1/chat/completions", "has_keys": false, "installed": false, "key_is_cliproxy": false},
        "files": [{"path": "«HOME»/.claude.json", "enabled": false}]
    })
}

/// O estado com o pacote de `installed()` e sem alvo (`r1-alvo-ssh-completo` sem os alvos).
fn installed_state() -> Value {
    merged(empty_state(), json!({
        "agent_exe": {"path": "«HOME»/.hangar/computer-control/windows-agent.exe", "exists": true, "size": 15},
        "installed_tag": "v2.0.0",
        "package_exists": true,
        "local_available": true
    }))
}

/// Estado depois de criar um único alvo SSH `name` com `host`.
fn with_ssh_target(base: Value, name: &str, host: &str) -> Value {
    let path = format!("«HOME»/.hangar/computer-control/targets/{name}-agent.json");
    merged(base, json!({
        "targets": [{"name": name, "path": path, "transport": "ssh", "host": host, "os": "windows"}],
        "agent_config": path,
        "agent_configs": [path]
    }))
}

fn ssh_file(host: &str) -> String {
    format!("{{\n  \"transport\": \"ssh\",\n  \"host\": \"{host}\",\n  \"agent_path\": \"«HOME»/.hangar/computer-control/windows-agent.exe\"\n}}\n")
}

impl Scene {
    fn assert_file(&self, rel: &str, expected: &str) {
        let home = self.home.to_string_lossy();
        assert_eq!(self.read(rel).as_deref(), Some(expected.replace("«HOME»", &home).as_str()), "{rel}");
    }
}

// ── GET /api/computer-control ────────────────────────────────────────────────────────────────

/// ROT-06, ROT-13, ROT-23, ROT-41, ROT-42, ROT-45, ROT-47, ROT-51, D1.2, D1.5 (r1-get-vazio).
#[tokio::test]
async fn rot06_get_empty_home() {
    let s = Scene::new().await;
    assert_eq!(s.get().await, (StatusCode::OK, s.h(empty_state())));
    assert_eq!(s.hits(), 0, "o GET não pode ir ao Python");
}

/// Surrogate solto que o Claude Code grava (`\ud800`) é lido como no Python e sai como U+FFFD.
#[tokio::test]
async fn get_reads_lone_surrogate() {
    let s = Scene::new().await;
    s.write(".claude.json", r#"{"a": "\ud800", "mcpServers": {"hangar-computer-control": {"command": "x", "env": {"HCC_AGENT_CONFIG": "/alvo-\ud800.json"}}}}"#);
    assert_eq!(s.get().await, (StatusCode::OK, s.h(merged(empty_state(), json!({
        "enabled": true,
        "agent_config": "/alvo-\u{fffd}.json",
        "files": [{"path": "«HOME»/.claude.json", "enabled": true}]
    })))));
}

/// ROT-08, D3.1: `/install` responde 410 "atualize o app" sem tocar o disco nem ir ao Python; o
/// convidado recebe 403 e quem não tem token 401 (D1.1).
#[tokio::test]
async fn rot08_install_is_gone_and_touches_nothing() {
    let s = Scene::new().await;
    let got = s.call("POST", "/api/computer-control/install", None).await;
    s.assert_error(&got, 410, "erro_computer_control_update_app", json!({}));
    let guest = s.call_as("POST", "/api/computer-control/install", None, Who::Guest).await;
    assert_eq!((guest.0, guest.1["detail"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("erro_fora_do_convite")));
    let nobody = s.call_as("POST", "/api/computer-control/install", None, Who::Nobody).await;
    assert_eq!(nobody.0, StatusCode::UNAUTHORIZED);
    assert_eq!(s.hits(), 0);
    assert!(!s.home.join(".hangar/computer-control").exists(), "nada criado");
}

/// D1.5, D3.6: binário ausente depois de uma instalação automática que falhou aparece como
/// `install_failed` com o código; com o binário no lugar, a falha antiga não aparece.
#[tokio::test]
async fn d1_5_failed_install_shows_until_the_binary_exists() {
    let s = Scene::new().await;
    s.write(".hangar/computer-control/install-error.json", r#"{"code": "computer_control_release_failed", "msg": "o GitHub respondeu 503"}"#);
    assert_eq!(s.get().await, (StatusCode::OK, s.h(merged(empty_state(), json!({
        "unavailable": "install_failed",
        "install_error": "computer_control_release_failed"
    })))));
    s.installed();
    assert_eq!(s.get().await, (StatusCode::OK, s.h(installed_state())));
}

#[path = "computer_control_put.rs"]
mod put;

/// D1.1: `%2F` no caminho não escapa da conferência do dono.
#[tokio::test]
async fn guest_cannot_reach_python_by_encoding_the_path() {
    let s = Scene::new().await;
    s.installed();
    for uri in ["/api/computer-control%2Ftargets", "/api%2Fcomputer-control/targets"] {
        let got = s.call_as("POST", uri, Some(json!({"project_dir": "", "name": "vm", "host": "vm"})), Who::Guest).await;
        assert_eq!(got.0, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(got.1["detail"]["code"], "erro_fora_do_convite", "{uri}");
    }
    assert_eq!(s.hits(), 0);
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
}

/// ROT-22..25, ROT-41, ROT-42, ROT-44, ROT-45, ROT-47..49, ROT-51, JEV-26, HIS-04, HIS-69, D1.8 (r1-get-completo).
#[tokio::test]
async fn rot22_get_complete_package_targets_accounts_and_jev() {
    let s = Scene::new().await;
    let entry = json!({"command": "«HOME»/.hangar/computer-control/hangar-computer-control", "args": [], "env": {
        "HCC_AGENT_CONFIG": "«HOME»/.hangar/computer-control/targets/delphi-02-agent.json",
        "HCC_AGENTS_DIR": "«HOME»/.hangar/computer-control/targets",
        "TYPESAFE_API_KEY": "chave-do-jev-456", "VIRTUAL_ENV": ""}});
    s.account(".claude-x", 0);
    s.account(".claude-y", 10);
    s.account(".claude-z", 20);
    s.json_file(".claude-x/.claude.json", json!({"oauthAccount": {"a": 1}, "mcpServers": {"hangar-computer-control": entry.clone()}}));
    s.json_file(".claude-y/.claude.json", json!({"oauthAccount": {}}));
    s.json_file(".claude.json", json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"}, "hangar-computer-control": entry}}));
    s.write(".claude/runtime-config.json", "{\n  \"jev_api_key\": \"chave-do-jev-456\",\n  \"jev_aux_base_url\": \"http://x/v1\",\n  \"jev_aux_modelo\": \"m1\",\n  \"jev_aux_esforco\": \"high\",\n  \"jev_aux_api_key\": \"chave-do-llm-123\"\n}");
    s.installed();
    s.json_file(".hangar/computer-control/targets/delphi-02-agent.json",
        json!({"transport": "ssh", "host": "administrator@delphi-02", "agent_path": "«HOME»/.hangar/computer-control/windows-agent.exe"}));
    s.write(".hangar/computer-control/targets/leia-me.md", "não é alvo");
    s.json_file(".hangar/computer-control/targets/linux-agent.json",
        json!({"transport": "local", "command": ["«HOME»/.hangar/computer-control/hangar-computer-control", "agent"], "request_timeout": 15}));
    s.write(".hangar/computer-control/targets/lista-agent.json", "[]");
    s.write(".hangar/computer-control/targets/quebrado-agent.json", "not-json");
    s.write(".hangar/computer-control/targets/sem-transporte-agent.json", "{}");
    s.write(".hangar/computer-control/targets/x-agent.json.bak", "{}");
    s.write(".ssh/config", "Host delphi-02\n  User administrator\nHost tardis\n");
    let targets = "«HOME»/.hangar/computer-control/targets";
    let expected = json!({
        "agent_exe": {"path": "«HOME»/.hangar/computer-control/windows-agent.exe", "exists": true, "size": 15},
        "mode": "package",
        "installed_tag": "v2.0.0",
        "package_exists": true,
        "unavailable": null,
        "targets": [
            {"name": "delphi-02", "path": format!("{targets}/delphi-02-agent.json"), "transport": "ssh", "host": "administrator@delphi-02", "os": "windows"},
            {"name": "linux", "path": format!("{targets}/linux-agent.json"), "transport": "local", "host": "", "os": "linux"},
            {"name": "lista", "path": format!("{targets}/lista-agent.json"), "transport": "local", "host": "", "os": "linux"},
            {"name": "quebrado", "path": format!("{targets}/quebrado-agent.json"), "transport": "local", "host": "", "os": "linux"},
            {"name": "sem-transporte", "path": format!("{targets}/sem-transporte-agent.json"), "transport": "local", "host": "", "os": "linux"}
        ],
        "ssh_hosts": ["delphi-02", "tardis"],
        "local_available": true,
        "platform": "linux",
        "enabled": true,
        "project_dir": "",
        "agent_config": format!("{targets}/delphi-02-agent.json"),
        "agent_configs": [
            format!("{targets}/delphi-02-agent.json"),
            format!("{targets}/linux-agent.json"),
            format!("{targets}/lista-agent.json"),
            format!("{targets}/quebrado-agent.json"),
            format!("{targets}/sem-transporte-agent.json")
        ],
        "llm_from_jev_aux": true,
        "llm_url": "http://x/v1/chat/completions",
        "llm_model": "m1",
        "llm_effort": "high",
        "llm_key_set": true,
        "llm_key_tail": "-123",
        "jev_key_set": true,
        "jev_key_tail": "-456",
        "jev_endpoint": "https://api.typesafe.ai/v1/systemone",
        "jev_model": "jev-latest",
        "cliproxy": {"preset_url": "http://127.0.0.1:8317/v1/chat/completions", "has_keys": false, "installed": false, "key_is_cliproxy": false},
        "files": [
            {"path": "«HOME»/.claude.json", "enabled": true},
            {"path": "«HOME»/.claude-x/.claude.json", "enabled": true},
            {"path": "«HOME»/.claude-y/.claude.json", "enabled": false}
        ]
    });
    let (status, body) = s.get().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, s.h(expected));
    // HIS-04: a chave nunca sai inteira.
    assert!(!body.to_string().contains("chave-do-jev-456") && !body.to_string().contains("chave-do-llm-123"));
}

/// ROT-40, JEV-26, HIS-62 (r1-get-cauda-chave-7-e-8): 7 caracteres não mostram nada, 8 mostram os 4 finais.
#[tokio::test]
async fn rot40_key_tails() {
    let s = Scene::new().await;
    s.write(".claude/runtime-config.json", "{\n  \"jev_api_key\": \"1234567\",\n  \"jev_aux_base_url\": \"http://x/v1\",\n  \"jev_aux_modelo\": \"m\",\n  \"jev_aux_api_key\": \"abcd5678\"\n}");
    assert_eq!(s.get().await.1, s.h(merged(empty_state(), json!({
        "llm_url": "http://x/v1/chat/completions", "llm_model": "m", "llm_key_set": true, "llm_key_tail": "5678",
        "jev_key_set": true, "jev_key_tail": "", "jev_endpoint": "https://api.typesafe.ai/v1/systemone", "jev_model": "jev-latest"
    }))));
}

/// ROT-46, D1.2, D1.4 (r1-get-checkout-local): o checkout em ~/Projetos não vira modo local.
#[tokio::test]
async fn rot46_local_checkout_is_ignored() {
    let s = Scene::new().await;
    s.write("Projetos/hangar-computer-control/alvo-agent.json", "{}");
    s.write("Projetos/hangar-computer-control/target/release/hangar-computer-control", "bin");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(empty_state())));
}

/// ROT-50, HIS-64 (r1-get-cliproxy): chaves do config.yaml, binário no PATH, chave do auxiliar é do CLIProxy.
#[tokio::test]
async fn rot50_cliproxy() {
    let s = Scene::new().await;
    s.write(".claude/runtime-config.json", "{\n  \"jev_aux_base_url\": \"http://127.0.0.1:8317/v1\",\n  \"jev_aux_modelo\": \"m\",\n  \"jev_aux_api_key\": \"k#1\"\n}");
    s.write(".cli-proxy-api/config.yaml", "port: 8317\naccess:\n  api-keys:\n    - \"k#1\"\n    - k2\napi-keys:\n  - name: provedor\n");
    crate::write_executable(&s.bin.join("cli-proxy-api"), "#!/bin/sh\n");
    assert_eq!(s.get().await.1, s.h(merged(empty_state(), json!({
        "llm_url": "http://127.0.0.1:8317/v1/chat/completions", "llm_model": "m", "llm_key_set": true, "llm_key_tail": "",
        "cliproxy": {"preset_url": "http://127.0.0.1:8317/v1/chat/completions", "has_keys": true, "installed": true, "key_is_cliproxy": true}
    }))));
}

/// ROT-14, ROT-52 (r1-get-conta-json-invalido): um `.claude.json` de conta ilegível derruba o GET.
#[tokio::test]
async fn rot52_account_invalid_json() {
    let s = Scene::new().await;
    s.account(".claude-x", 0);
    s.write(".claude-x/.claude.json", "{quebrado");
    s.write(".claude.json", "{\"numStartups\": 3}");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude-x/.claude.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-16, B3 (r1-get-conta-mcpservers-nao-objeto): `mcpServers: []` é falso, vale ausente.
#[tokio::test]
async fn rot16_account_mcpservers_empty_list_is_absent() {
    let s = Scene::new().await;
    s.account(".claude-x", 0);
    s.write(".claude-x/.claude.json", "{\"mcpServers\": []}");
    s.write(".claude.json", "{\"numStartups\": 3}");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(merged(empty_state(), json!({"files": [
        {"path": "«HOME»/.claude.json", "enabled": false},
        {"path": "«HOME»/.claude-x/.claude.json", "enabled": false}
    ]})))));
}

/// ROT-76, D2.7 (r1-get-conta-vazia-texto): conta com 0 bytes vale `{}` (o Python dava 500).
#[tokio::test]
async fn rot76_account_empty_file_counts_as_unopened() {
    let s = Scene::new().await;
    s.account(".claude-x", 0);
    s.write(".claude-x/.claude.json", "");
    s.write(".claude.json", "{\"numStartups\": 3}");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(merged(empty_state(), json!({"files": [
        {"path": "«HOME»/.claude.json", "enabled": false},
        {"path": "«HOME»/.claude-x/.claude.json", "enabled": false}
    ]})))));
}

/// ROT-04, D1.9 (r1-get-entrada-env-nao-objeto): `env` lista vira 500 com código, não texto cru.
#[tokio::test]
async fn rot04_entry_env_not_object() {
    let s = Scene::new().await;
    s.json_file(".claude.json", json!({"mcpServers": {"hangar-computer-control": {
        "command": "«HOME»/.hangar/computer-control/hangar-computer-control", "env": ["x"]}}}));
    let (status, body) = s.get().await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["detail"]["code"], "erro_computer_control_io", "{body}");
}

/// ROT-16, ROT-45 (r1-get-entrada-nao-objeto): entrada texto conta como ausente.
#[tokio::test]
async fn rot45_entry_not_object_is_absent() {
    let s = Scene::new().await;
    s.write(".claude.json", "{\"mcpServers\": {\"hangar-computer-control\": \"x\"}}");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(empty_state())));
}

/// JEV-28, ROT-14 (r1-get-guardada-invalida).
#[tokio::test]
async fn jev28_parked_invalid_json() {
    let s = Scene::new().await;
    s.write(".hangar/computer-control.json", "[1,");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.hangar/computer-control.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-20, ROT-48 (r1-get-guardada): desligado, o alvo padrão vem da entrada guardada.
#[tokio::test]
async fn rot48_parked_entry_gives_agent_config() {
    let s = Scene::new().await;
    s.write(".claude.json", "{\"numStartups\": 3}");
    s.json_file(".hangar/computer-control.json", json!({"command": "«HOME»/.hangar/computer-control/hangar-computer-control",
        "args": [], "env": {"HCC_AGENT_CONFIG": "/outro/alvo-agent.json"}}));
    s.installed();
    s.json_file(".hangar/computer-control/targets/delphi-02-agent.json",
        json!({"transport": "ssh", "host": "d", "agent_path": "«HOME»/.hangar/computer-control/windows-agent.exe"}));
    let expected = merged(with_ssh_target(installed_state(), "delphi-02", "d"), json!({"agent_config": "/outro/alvo-agent.json"}));
    assert_eq!(s.get().await, (StatusCode::OK, s.h(expected)));
}

/// ROT-14, ROT-47 (r1-get-install-json-invalido).
#[tokio::test]
async fn rot47_install_json_invalid() {
    let s = Scene::new().await;
    s.write(".hangar/computer-control/install.json", "{tag");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.hangar/computer-control/install.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-49 (r1-get-jev-openrouter-e-query): chave sk-or- vai ao OpenRouter; a query do auxiliar fica no fim.
#[tokio::test]
async fn rot49_jev_openrouter_and_aux_query() {
    let s = Scene::new().await;
    s.write(".claude/runtime-config.json", "{\n  \"jev_api_key\": \"sk-or-abcdefgh\",\n  \"jev_aux_base_url\": \"https://x.azure.com/openai/v1?api-version=2024-02-01\",\n  \"jev_aux_modelo\": \"gpt\",\n  \"jev_aux_esforco\": \"\",\n  \"jev_aux_api_key\": \"\"\n}");
    assert_eq!(s.get().await.1, s.h(merged(empty_state(), json!({
        "llm_url": "https://x.azure.com/openai/v1/chat/completions?api-version=2024-02-01", "llm_model": "gpt",
        "jev_key_set": true, "jev_key_tail": "efgh",
        "jev_endpoint": "https://openrouter.ai/api/alpha/decisions", "jev_model": "~typesafe/jev-latest"
    }))));
}

/// ROT-14, ROT-52, HIS-57 (r1-get-main-json-invalido).
#[tokio::test]
async fn his57_main_invalid_json() {
    let s = Scene::new().await;
    s.write(".claude.json", "{invalido");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-15 (r1-get-main-nao-objeto).
#[tokio::test]
async fn rot15_main_not_object() {
    let s = Scene::new().await;
    s.write(".claude.json", "[]");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read", json!({"file": "«HOME»/.claude.json", "error": ""}));
}

/// ROT-14 (r1-get-main-sem-permissao): o `strerror`, sem o conteúdo do arquivo.
#[tokio::test]
async fn rot14_main_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scene::new().await;
    s.write(".claude.json", "{}");
    std::fs::set_permissions(s.home.join(".claude.json"), std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(s.home.join(".claude.json")).is_ok() {
        return; // root lê mesmo assim: o caso não existe aqui.
    }
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "Permission denied"}));
}

/// ROT-14 (r1-get-main-utf8-invalido).
#[tokio::test]
async fn rot14_main_invalid_utf8() {
    let s = Scene::new().await;
    s.write(".claude.json", b"{\"a\": \"\xff\"}");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-14 (r1-get-main-vazio-texto): o principal com 0 bytes continua erro.
#[tokio::test]
async fn rot14_main_empty_file() {
    let s = Scene::new().await;
    s.write(".claude.json", "");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "JSON ou encoding inválido"}));
}

/// ROT-16, HIS-57 (r1-get-mcpservers-nao-objeto).
#[tokio::test]
async fn rot16_main_mcpservers_not_object() {
    let s = Scene::new().await;
    s.write(".claude.json", "{\"mcpServers\": \"x\"}");
    s.assert_error(&s.get().await, 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "mcpServers inválido"}));
}

/// ROT-16 (r1-get-mcpservers-nulo): `null` vale ausente.
#[tokio::test]
async fn rot16_main_mcpservers_null() {
    let s = Scene::new().await;
    s.write(".claude.json", "{\"mcpServers\": null}");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(empty_state())));
}

/// ROT-47, D1.3 (r1-get-pacote-uvx): `uvx` gravado não conta como pacote.
#[tokio::test]
async fn rot47_uvx_package_does_not_count() {
    let s = Scene::new().await;
    s.write(".hangar/computer-control/install.json", "{\n  \"tag\": \"v1.0.0\",\n  \"uvx\": \"/bin/uvx\"\n}");
    std::fs::create_dir_all(s.home.join(".hangar/computer-control/targets")).unwrap();
    s.write(".hangar/computer-control/windows-agent.exe", "MZ-agente-falso");
    assert_eq!(s.get().await, (StatusCode::OK, s.h(merged(empty_state(), json!({
        "agent_exe": {"path": "«HOME»/.hangar/computer-control/windows-agent.exe", "exists": true, "size": 15},
        "installed_tag": "v1.0.0"
    })))));
}

/// ROT-01, D1.1 (r1-get-sem-token, r1-alvo-sem-token): sem token, 401 nas quatro rotas.
#[tokio::test]
async fn rot01_without_token_is_401() {
    let s = Scene::new().await;
    s.installed();
    for (method, uri, body) in [
        ("GET", "/api/computer-control", None),
        ("POST", "/api/computer-control/targets", Some(json!({"project_dir": "", "name": "vm", "host": "vm"}))),
        ("POST", "/api/computer-control/test-host", Some(json!({"host": "h"}))),
        ("GET", "/api/computer-control/windows-setup?host=h", None),
    ] {
        let got = s.call_as(method, uri, body, Who::Nobody).await;
        assert_eq!(got, (StatusCode::UNAUTHORIZED, json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}})), "{uri}");
    }
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
}

/// ROT-01, D1.1: o convidado recebe 403 da própria rota, nas quatro.
#[tokio::test]
async fn rot01_guest_is_403() {
    let s = Scene::new().await;
    s.installed();
    s.ssh("printf 'ok\\n'");
    for (method, uri, body) in [
        ("GET", "/api/computer-control", None),
        ("POST", "/api/computer-control/targets", Some(json!({"project_dir": "", "name": "vm", "host": "vm"}))),
        ("POST", "/api/computer-control/test-host", Some(json!({"host": "h"}))),
        ("GET", "/api/computer-control/windows-setup?host=h", None),
    ] {
        let got = s.call_as(method, uri, body, Who::Guest).await;
        assert_eq!(got.0, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(got.1["detail"]["code"], "erro_fora_do_convite", "{uri}");
    }
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
    assert!(s.ssh_argv().is_empty(), "o convidado não roda ssh");
}

/// ROT-43 (r1-get-ssh-config-diretorio): `~/.ssh/config` ilegível = lista vazia.
#[tokio::test]
async fn rot43_ssh_config_directory() {
    let s = Scene::new().await;
    std::fs::create_dir_all(s.home.join(".ssh/config")).unwrap();
    assert_eq!(s.get().await, (StatusCode::OK, s.h(empty_state())));
}

/// ROT-43, HIS-66 (r1-get-ssh-config-variantes): maiúsculas, vários por linha, sem coringa, sem repetir.
#[tokio::test]
async fn rot43_ssh_config_variants() {
    let s = Scene::new().await;
    s.write(".ssh/config", b"Host a b\nhost c\n  HOST\td\nHost *\nHost e*.x f? !g h\nHost a\nHostName nao-conta\nMatch host x\n  Host  i\xffj\nHost\n#Host k\n");
    assert_eq!(s.get().await.1["ssh_hosts"], json!(["a", "b", "c", "d", "h", "i\u{fffd}j"]));
}

/// ROT-05: o teste SSH lento não segura a leitura do estado.
#[tokio::test]
async fn rot05_slow_ssh_does_not_block_get() {
    let s = Arc::new(Scene::new().await);
    s.ssh("/bin/sleep 3; printf 'ok\\n'");
    let slow = {
        let s = s.clone();
        tokio::spawn(async move { s.test_host(json!({"host": "h"})).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    assert_eq!(s.get().await.0, StatusCode::OK);
    assert!(started.elapsed() < Duration::from_secs(2), "o GET esperou o ssh");
    assert!(!slow.is_finished());
    assert_eq!(slow.await.unwrap(), (StatusCode::OK, json!({"ok": true, "detail": ""})));
}

// ── POST /api/computer-control/targets ───────────────────────────────────────────────────────

/// ROT-09, ROT-32, ROT-35, ROT-36, ROT-37, HIS-05 (r1-alvo-ssh-completo).
#[tokio::test]
async fn rot32_create_ssh_target() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "delphi-03", "transport": "ssh", "host": "delphi-03",
        "proxy_command": "  ssh -W %h:%p salto  ", "request_timeout": 40})).await;
    assert_eq!(got, (StatusCode::OK, s.h(with_ssh_target(installed_state(), "delphi-03", "delphi-03"))));
    s.assert_file(".hangar/computer-control/targets/delphi-03-agent.json",
        "{\n  \"transport\": \"ssh\",\n  \"host\": \"delphi-03\",\n  \"agent_path\": \"«HOME»/.hangar/computer-control/windows-agent.exe\",\n  \"proxy_command\": \"ssh -W %h:%p salto\",\n  \"request_timeout\": 40\n}\n");
    // ROT-37: a resposta é o estado inteiro, o mesmo do GET.
    assert_eq!(s.get().await.1, got.1);
    assert_eq!(s.hits(), 0, "criar alvo não pode ir ao Python");
}

/// ROT-02 (r1-alvo-campo-desconhecido).
#[tokio::test]
async fn rot02_target_unknown_field_is_422() {
    let s = Scene::new().await;
    s.installed();
    assert_eq!(s.target(json!({"project_dir": "", "name": "vm", "host": "vm", "extra": 1})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
}

/// ROT-29, ROT-03, HIS-06 (r1-alvo-existe).
#[tokio::test]
async fn rot29_target_exists() {
    let s = Scene::new().await;
    s.installed();
    s.write(".hangar/computer-control/targets/delphi-03-agent.json", "{\n  \"transport\": \"ssh\"\n}\n");
    let got = s.target(json!({"project_dir": "", "name": "delphi-03", "host": "x"})).await;
    s.assert_error(&got, 409, "erro_computer_control_target_exists", json!({"name": "delphi-03"}));
    s.assert_file(".hangar/computer-control/targets/delphi-03-agent.json", "{\n  \"transport\": \"ssh\"\n}\n");
}

/// Dois POSTs do mesmo alvo ao mesmo tempo: um grava inteiro, o outro recebe 409, sem sobra de temporário.
#[tokio::test]
async fn concurrent_targets_one_wins() {
    let s = Scene::new().await;
    s.installed();
    for _ in 0..5 {
        let _ = std::fs::remove_file(s.home.join(".hangar/computer-control/targets/vm-agent.json"));
        let (a, b) = tokio::join!(
            s.target(json!({"project_dir": "", "name": "vm", "host": "a"})),
            s.target(json!({"project_dir": "", "name": "vm", "host": "b"})),
        );
        let mut statuses = [a.0.as_u16(), b.0.as_u16()];
        statuses.sort();
        assert_eq!(statuses, [200, 409], "{} / {}", a.1, b.1);
        let winner = if a.0 == StatusCode::OK { &a.1 } else { &b.1 };
        let host = winner["targets"][0]["host"].as_str().unwrap().to_owned();
        s.assert_file(".hangar/computer-control/targets/vm-agent.json", &ssh_file(&host));
        let names: Vec<String> = std::fs::read_dir(s.home.join(".hangar/computer-control/targets")).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["vm-agent.json".to_owned()]);
    }
}

/// ROT-39 (r1-alvo-host-espaco, -host-quebra-linha-interna, -host-vazio), HIS-66 (r1-alvo-host-traco).
#[tokio::test]
async fn rot39_target_host_refused() {
    let s = Scene::new().await;
    s.installed();
    for host in [json!("a b"), json!("abc\ndef"), json!("-oProxyCommand=evil"), Value::Null] {
        let mut body = json!({"project_dir": "", "name": "vm"});
        if !host.is_null() {
            body["host"] = host.clone();
        }
        s.assert_error(&s.target(body).await, 400, "erro_computer_control_target_host", json!({}));
    }
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
}

/// ROT-27, ROT-39 (r1-alvo-host-nao-ascii): o nome sai do host, mas o host é recusado.
#[tokio::test]
async fn rot27_target_non_ascii_host() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "", "host": "Usuário@Máquina"})).await;
    s.assert_error(&got, 400, "erro_computer_control_target_host", json!({}));
    assert!(s.read(".hangar/computer-control/targets/m-quina-agent.json").is_none());
}

/// ROT-39 (r1-alvo-host-quebra-linha-final): "abc\n" é aparado e passa.
#[tokio::test]
async fn rot39_target_host_trailing_newline() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "vm", "host": "abc\n"})).await;
    assert_eq!(got, (StatusCode::OK, s.h(with_ssh_target(installed_state(), "vm", "abc"))));
    s.assert_file(".hangar/computer-control/targets/vm-agent.json", &ssh_file("abc"));
}

/// ROT-34, HIS-38 (r1-alvo-local-com-binario): local no Linux roda `[binário, "agent"]` com prazo 15.
#[tokio::test]
async fn his38_create_local_target() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "este", "transport": "local"})).await;
    let path = "«HOME»/.hangar/computer-control/targets/este-agent.json";
    assert_eq!(got, (StatusCode::OK, s.h(merged(installed_state(), json!({
        "targets": [{"name": "este", "path": path, "transport": "local", "host": "", "os": "linux"}],
        "agent_config": path, "agent_configs": [path]
    })))));
    s.assert_file(".hangar/computer-control/targets/este-agent.json",
        "{\n  \"transport\": \"local\",\n  \"command\": [\n    \"«HOME»/.hangar/computer-control/hangar-computer-control\",\n    \"agent\"\n  ],\n  \"request_timeout\": 15\n}\n");
}

/// ROT-35 (r1-alvo-local-prazo-30): o prazo do corpo vence o 15 do local.
#[tokio::test]
async fn rot35_local_target_timeout_from_body() {
    let s = Scene::new().await;
    s.installed();
    assert_eq!(s.target(json!({"project_dir": "", "name": "este", "transport": "local", "request_timeout": 30})).await.0, StatusCode::OK);
    s.assert_file(".hangar/computer-control/targets/este-agent.json",
        "{\n  \"transport\": \"local\",\n  \"command\": [\n    \"«HOME»/.hangar/computer-control/hangar-computer-control\",\n    \"agent\"\n  ],\n  \"request_timeout\": 30\n}\n");
}

/// ROT-34, HIS-07 (r1-alvo-local-sem-binario).
#[tokio::test]
async fn his07_local_target_without_binary() {
    let s = Scene::new().await;
    s.write(".hangar/computer-control/install.json", "{\n  \"tag\": \"v2.0.0\"\n}");
    std::fs::create_dir_all(s.home.join(".hangar/computer-control/targets")).unwrap();
    s.write(".hangar/computer-control/windows-agent.exe", "MZ-agente-falso");
    let got = s.target(json!({"project_dir": "", "name": "este", "transport": "local"})).await;
    s.assert_error(&got, 400, "erro_computer_control_local_only_windows", json!({}));
    assert!(s.read(".hangar/computer-control/targets/este-agent.json").is_none());
}

/// ROT-28 (r1-alvo-local-sem-nome): local sem nome não ganha nome automático.
#[tokio::test]
async fn rot28_local_target_without_name() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "", "transport": "local"})).await;
    s.assert_error(&got, 400, "erro_computer_control_target_name", json!({}));
}

/// ROT-26 (r1-alvo-nome-41, -nome-42, -nome-comeca-hifen, -nome-invalido-espaco, -nome-quebra-linha).
#[tokio::test]
async fn rot26_target_name_rule() {
    let s = Scene::new().await;
    s.installed();
    for name in ["a".repeat(42), "-x".into(), "Bad Name".into()] {
        let got = s.target(json!({"project_dir": "", "name": name, "host": "h"})).await;
        s.assert_error(&got, 400, "erro_computer_control_target_name", json!({}));
    }
    let longest = "x".repeat(41);
    assert_eq!(s.target(json!({"project_dir": "", "name": &longest, "host": "h"})).await.0, StatusCode::OK);
    s.assert_file(&format!(".hangar/computer-control/targets/{longest}-agent.json"), &ssh_file("h"));
    assert_eq!(s.target(json!({"project_dir": "", "name": "a\n", "host": "h"})).await.0, StatusCode::OK);
    s.assert_file(".hangar/computer-control/targets/a-agent.json", &ssh_file("h"));
}

/// ROT-27 (r1-alvo-nome-do-host, -41, -espacos, -hifen-no-corte, -varios-arroba).
#[tokio::test]
async fn rot27_target_name_from_host() {
    let s = Scene::new().await;
    s.installed();
    for (host, name) in [
        ("Administrator@DELPHI-03", "delphi-03"),
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-c-c-c", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-"),
        ("a@b@C", "c"),
    ] {
        let got = s.target(json!({"project_dir": "", "name": "", "host": host})).await;
        assert_eq!(got.0, StatusCode::OK, "{host}: {}", got.1);
        s.assert_file(&format!(".hangar/computer-control/targets/{name}-agent.json"), &ssh_file(host));
    }
    let got = s.target(json!({"project_dir": "", "name": "", "host": "  delphi-04  "})).await;
    assert_eq!(got.0, StatusCode::OK);
    s.assert_file(".hangar/computer-control/targets/delphi-04-agent.json", &ssh_file("delphi-04"));
}

/// ROT-26, ROT-27 (r1-alvo-nome-do-host-vazio): host só de símbolos dá nome vazio.
#[tokio::test]
async fn rot27_target_name_from_symbol_host() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(json!({"project_dir": "", "name": "", "host": "___"})).await;
    s.assert_error(&got, 400, "erro_computer_control_target_name", json!({}));
}

/// ROT-09 (r1-alvo-prazo-0, -prazo-601, -sem-nome-campo, -sem-project-dir, -transporte-invalido).
#[tokio::test]
async fn rot09_target_body_validation() {
    let s = Scene::new().await;
    s.installed();
    for body in [
        json!({"project_dir": "", "name": "vm", "host": "vm", "request_timeout": 0}),
        json!({"project_dir": "", "name": "vm", "host": "vm", "request_timeout": 601}),
        json!({"project_dir": "", "host": "vm"}),
        json!({"name": "vm", "host": "vm"}),
        json!({"project_dir": "", "name": "vm", "host": "vm", "transport": "ftp"}),
    ] {
        assert_eq!(s.target(body.clone()).await.0, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    assert!(s.read(".hangar/computer-control/targets/vm-agent.json").is_none());
}

/// ROT-35 (r1-alvo-prazo-nulo): `null` não grava prazo.
#[tokio::test]
async fn rot35_target_null_timeout() {
    let s = Scene::new().await;
    s.installed();
    assert_eq!(s.target(json!({"project_dir": "", "name": "vm", "host": "vm", "request_timeout": null})).await.0, StatusCode::OK);
    s.assert_file(".hangar/computer-control/targets/vm-agent.json", &ssh_file("vm"));
}

/// D1.6 (r1-alvo-project-dir-ignorado): `project_dir` não escolhe a pasta.
#[tokio::test]
async fn d16_target_project_dir_ignored() {
    let s = Scene::new().await;
    s.installed();
    let got = s.target(s.h(json!({"project_dir": "«HOME»/outro", "name": "vm", "host": "vm"}))).await;
    assert_eq!(got, (StatusCode::OK, s.h(with_ssh_target(installed_state(), "vm", "vm"))));
    s.assert_file(".hangar/computer-control/targets/vm-agent.json", &ssh_file("vm"));
    assert!(!s.home.join("outro").exists());
}

/// ROT-32, ROT-36 (r1-alvo-proxy-nao-ascii): JSON com indentação 2, não-ASCII escapado, newline final.
#[tokio::test]
async fn rot36_target_file_escapes_non_ascii() {
    let s = Scene::new().await;
    s.installed();
    assert_eq!(s.target(json!({"project_dir": "", "name": "vm", "host": "vm", "proxy_command": "ssh -W %h:%p salto-ção"})).await.0, StatusCode::OK);
    s.assert_file(".hangar/computer-control/targets/vm-agent.json",
        "{\n  \"transport\": \"ssh\",\n  \"host\": \"vm\",\n  \"agent_path\": \"«HOME»/.hangar/computer-control/windows-agent.exe\",\n  \"proxy_command\": \"ssh -W %h:%p salto-\\u00e7\\u00e3o\"\n}\n");
}

/// ROT-30, B1, D1.6 (r1-alvo-sem-pasta): sem `targets/`, a pasta nasce (o Python dava 400).
#[tokio::test]
async fn rot30_target_creates_missing_folder() {
    let s = Scene::new().await;
    let got = s.target(json!({"project_dir": "", "name": "vm", "host": "vm"})).await;
    assert_eq!(got, (StatusCode::OK, s.h(with_ssh_target(empty_state(), "vm", "vm"))));
    s.assert_file(".hangar/computer-control/targets/vm-agent.json", &ssh_file("vm"));
}

// ── POST /api/computer-control/test-host ─────────────────────────────────────────────────────

/// ROT-10, ROT-53, ROT-54, HIS-67 (r1-teste-ok): argv do ssh com accept-new, stdin vazio.
#[tokio::test]
async fn rot53_test_host_ok() {
    let s = Scene::new().await;
    // stdin vazio: o `read` acha o fim na hora; herdado, poderia ler algo ou ficar preso.
    s.ssh("if read -r line; then exit 9; fi; printf 'ok\\n'");
    assert_eq!(s.test_host(json!({"host": "delphi-02"})).await, (StatusCode::OK, json!({"ok": true, "detail": ""})));
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o",
        "StrictHostKeyChecking=accept-new", "--", "delphi-02", "echo", "ok"]);
    assert_eq!(s.hits(), 0, "o teste SSH não pode ir ao Python");
}

/// ROT-53, ROT-58 (r1-teste-proxy-e-espacos): host e proxy aparados no argv.
#[tokio::test]
async fn rot53_test_host_proxy_trimmed() {
    let s = Scene::new().await;
    s.ssh("printf 'ok\\n'");
    let got = s.test_host(json!({"host": "  delphi-02  ", "proxy_command": "  ssh -W %h:%p salto  "})).await;
    assert_eq!(got, (StatusCode::OK, json!({"ok": true, "detail": ""})));
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o",
        "StrictHostKeyChecking=accept-new", "-o", "ProxyCommand=ssh -W %h:%p salto", "--", "delphi-02", "echo", "ok"]);
}

/// ROT-57 (r1-teste-bytes-invalidos, -codigo-sem-saida, -ok-com-codigo-1, -ok-substring, -sem-ok).
#[tokio::test]
async fn rot57_test_host_outcomes() {
    let s = Scene::new().await;
    for (body, expected) in [
        ("printf 'falhou \\377\\376 fim' >&2; exit 1", json!({"ok": false, "detail": "falhou \u{fffd}\u{fffd} fim"})),
        ("exit 3", json!({"ok": false, "detail": "saiu com 3"})),
        ("printf 'ok'; exit 1", json!({"ok": false, "detail": "ok"})),
        ("printf 'broken\\n'", json!({"ok": true, "detail": ""})),
        ("printf 'nada\\n'", json!({"ok": false, "detail": "nada"})),
    ] {
        s.ssh(body);
        assert_eq!(s.test_host(json!({"host": "h"})).await, (StatusCode::OK, expected), "{body}");
    }
}

/// ROT-57 (r1-teste-stderr-longo): os últimos 400 code points do stderr aparado.
#[tokio::test]
async fn rot57_test_host_long_stderr() {
    let s = Scene::new().await;
    s.ssh(&format!("printf '%s' '{}\n  ' >&2; exit 255", "falha é ".repeat(100)));
    let got = s.test_host(json!({"host": "h"})).await;
    assert_eq!(got, (StatusCode::OK, json!({"ok": false, "detail": " falha é".repeat(50)})));
}

/// ROT-58, ROT-39 (r1-teste-host-invalido).
#[tokio::test]
async fn rot58_test_host_invalid_host() {
    let s = Scene::new().await;
    s.ssh("printf 'ok'");
    s.assert_error(&s.test_host(json!({"host": "-x"})).await, 400, "erro_computer_control_target_host", json!({}));
    assert!(s.ssh_argv().is_empty());
}

/// ROT-02, ROT-10 (r1-teste-campo-a-mais, r1-teste-sem-host).
#[tokio::test]
async fn rot10_test_host_body_validation() {
    let s = Scene::new().await;
    s.ssh("printf 'ok'");
    assert_eq!(s.test_host(json!({"host": "h", "x": 1})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(s.test_host(json!({"proxy_command": ""})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(s.ssh_argv().is_empty());
}

/// ROT-55, HIS-59 (r1-teste-sem-ssh): sem ssh no PATH é 200 com `ok: false`.
#[tokio::test]
async fn his59_test_host_without_ssh() {
    let s = Scene::new().await;
    assert_eq!(s.test_host(json!({"host": "h"})).await, (StatusCode::OK, json!({"ok": false, "detail": "ssh não encontrado neste servidor"})));
}

/// ROT-56 (r1-teste-prazo): no prazo de 20 s cai o ssh e o que ele deixou no grupo.
#[tokio::test]
async fn rot56_test_host_deadline() {
    let s = Scene::new().await;
    s.hanging_ssh();
    let started = Instant::now();
    assert_eq!(s.test_host(json!({"host": "h"})).await, (StatusCode::OK, json!({"ok": false, "detail": "sem resposta em 20 s"})));
    assert!(started.elapsed() < Duration::from_secs(60));
    s.assert_group_dead().await;
}

/// O ssh filho não herda a autoridade do servidor (`PRIVATE_ENV_KEYS`), mas recebe o PATH dele.
#[tokio::test]
async fn ssh_child_environment_without_server_secrets() {
    let s = Scene::new().await;
    let seen = s.root.path().join("ssh-env");
    s.ssh(&format!(
        "printf '%s\\n' \"${{CP_AUTH_TOKEN-ausente}}\" \"${{HANGAR_INTERNAL_SECRET-ausente}}\" \"$PATH\" > '{}'\nprintf 'ok\\n'",
        seen.display()
    ));
    assert_eq!(s.test_host(json!({"host": "h"})).await, (StatusCode::OK, json!({"ok": true, "detail": ""})));
    let lines: Vec<String> = std::fs::read_to_string(&seen).unwrap().lines().map(str::to_owned).collect();
    assert_eq!(lines, ["ausente".to_owned(), "ausente".to_owned(), s.bin.to_string_lossy().into_owned()]);
}

// ── GET /api/computer-control/windows-setup ──────────────────────────────────────────────────

/// ROT-11, ROT-59, ROT-60, ROT-64, ROT-65, HIS-68 (r1-windows-host-vazio): texto idêntico ao do Python.
#[tokio::test]
async fn rot64_windows_setup_prompt() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.ssh(RESOLVED);
    assert_eq!(s.windows("?host=").await, (StatusCode::OK, json!({"prompt": PROMPT_ADMINISTRATOR, "user": "administrator"})));
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-G", "--", "novo-windows"]);
    assert_eq!(s.hits(), 0, "preparar o Windows não pode ir ao Python");
}

/// ROT-11, ROT-59 (r1-windows-sem-query): sem `host` também vira `novo-windows`.
#[tokio::test]
async fn rot59_windows_setup_without_query() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.ssh(RESOLVED);
    assert_eq!(s.windows("").await, (StatusCode::OK, json!({"prompt": PROMPT_ADMINISTRATOR, "user": "administrator"})));
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-G", "--", "novo-windows"]);
}

/// ROT-59, ROT-39 (r1-windows-host-aparado, r1-windows-host-invalido).
#[tokio::test]
async fn rot59_windows_setup_host_checked() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.ssh(RESOLVED);
    assert_eq!(s.windows("?host=%20%20delphi-02%20%20").await.0, StatusCode::OK);
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-G", "--", "delphi-02"]);
    std::fs::remove_file(s.root.path().join("ssh-args")).unwrap();
    s.assert_error(&s.windows("?host=-x").await, 400, "erro_computer_control_target_host", json!({}));
    assert!(s.ssh_argv().is_empty());
}

/// ROT-60 (r1-windows-identidade-absoluta, r1-windows-varias-identidades): primeira `identityfile` com `.pub`, primeiro `user`.
#[tokio::test]
async fn rot60_windows_setup_identity() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.write(".ssh/id_x.pub", "ssh-rsa XXX\n");
    s.ssh(&format!("printf 'identityfile {}/.ssh/id_ed25519\\n'", s.home.display()));
    let got = s.windows("?host=h").await;
    assert_eq!(got.0, StatusCode::OK);
    assert_eq!(got.1["user"], "");
    assert!(got.1["prompt"].as_str().unwrap().contains("\n   ssh-ed25519 AAAAC3Nza dono@máquina\n"));
    s.ssh("printf 'user primeiro\\nuser segundo\\nidentityfile ~/.ssh/id_rsa\\nidentityfile ~/.ssh/id_ed25519\\nidentityfile ~/.ssh/id_x\\n'");
    let got = s.windows("?host=h").await;
    assert_eq!(got.1["user"], "primeiro");
    let prompt = got.1["prompt"].as_str().unwrap();
    assert!(prompt.contains("4. Autorize esta chave pública pro usuário primeiro (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador logado e me diga qual é):\n   ssh-ed25519 AAAAC3Nza dono@máquina\n"), "{prompt}");
}

/// ROT-62 (r1-windows-sem-chave), ROT-61 (r1-windows-sem-ssh, r1-windows-ssh-falha).
#[tokio::test]
async fn rot62_windows_setup_without_key() {
    let s = Scene::new().await;
    s.ssh(RESOLVED);
    s.assert_error(&s.windows("?host=h").await, 400, "erro_computer_control_no_ssh_key", json!({}));
    s.write(".ssh/id_ed25519.pub", KEY);
    s.ssh("exit 255");
    s.assert_error(&s.windows("?host=h").await, 400, "erro_computer_control_no_ssh_key", json!({}));
    std::fs::remove_file(s.bin.join("ssh")).unwrap();
    s.assert_error(&s.windows("?host=h").await, 400, "erro_computer_control_no_ssh_key", json!({}));
}

/// ROT-61 (r1-windows-ssh-prazo): no prazo de 10 s do `ssh -G` cai o grupo inteiro.
#[tokio::test]
async fn rot61_windows_setup_resolve_deadline() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.hanging_ssh();
    let started = Instant::now();
    s.assert_error(&s.windows("?host=h").await, 400, "erro_computer_control_no_ssh_key", json!({}));
    assert!(started.elapsed() < Duration::from_secs(60));
    s.assert_group_dead().await;
}

/// ROT-65 (r1-windows-sem-usuario), ROT-63 (r1-windows-usuario-explicito).
#[tokio::test]
async fn rot63_windows_setup_user_line() {
    let s = Scene::new().await;
    s.write(".ssh/id_ed25519.pub", KEY);
    s.ssh("printf 'identityfile ~/.ssh/id_ed25519\\n'");
    let got = s.windows("?host=h").await;
    assert_eq!(got.1["user"], "");
    assert!(got.1["prompt"].as_str().unwrap().contains("4. Autorize esta chave pública pro usuário que vai entrar por SSH (me diga qual é):\n"));
    s.ssh(RESOLVED);
    let got = s.windows("?host=Admin@delphi-02").await;
    assert_eq!(got.1["user"], "Admin");
    assert!(got.1["prompt"].as_str().unwrap().contains("4. Autorize esta chave pública pro usuário Admin (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador logado e me diga qual é):\n"));
    assert_eq!(s.ssh_argv(), [s.ssh_path().as_str(), "-G", "--", "Admin@delphi-02"]);
}
