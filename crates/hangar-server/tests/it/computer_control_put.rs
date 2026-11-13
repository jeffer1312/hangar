//! `PUT /api/computer-control` (R2) pela rota do Rust, com HOME falso. O esperado vem das capturas do
//! Python (`contract/computer-control/r2-*.json`, ponta 7ef9011ba) com as diferenças D2.1–D2.7: arquivos
//! comparados pelo JSON lido (D2.5); recusa = nenhum arquivo muda.
use std::collections::BTreeMap;

use axum::http::StatusCode;
use serde_json::{Value, json};

use hangar_core::pycompat::pyjson::loads_lossless;
use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::config_sync::runtime::RuntimeConfig;

use super::{Scene, Who, installed_state, merged, ssh_file, with_ssh_target, without_running};

const AGENT: &str = "«HOME»/.hangar/computer-control/targets/delphi-02-agent.json";
const MAIN: &str = r#"{"numStartups":3,"zeta":"ação ✓ 😀","oauthAccount":{"emailAddress":"dono@exemplo"},"alpha":1}"#;
const ACCOUNT_X: &str = r#"{"oauthAccount":{"a":1},"ordem":["b","a"]}"#;
const RUNTIME: &str = "{\n  \"jev_api_key\": \"chave-do-jev-456\",\n  \"jev_aux_base_url\": \"http://x/v1\",\n  \"jev_aux_modelo\": \"m1\",\n  \"jev_aux_esforco\": \"high\",\n  \"jev_aux_api_key\": \"chave-do-llm-123\"\n}";
/// Os arquivos que o PUT pode tocar; recusa deixa todos como estavam.
const TOUCHED: [&str; 5] =
    [".claude.json", ".claude-x/.claude.json", ".claude-y/.claude.json", ".claude-z/.claude.json", ".hangar/computer-control.json"];

impl Scene {
    /// O estado comum das capturas R2: conta x com arquivo, y com `{}`, z sem arquivo; principal compacto
    /// com não-ASCII; Jev e auxiliar; pacote instalado e o alvo `delphi-02`.
    fn r2(&self) {
        self.account(".claude-x", 0);
        self.account(".claude-y", 10);
        self.account(".claude-z", 20);
        self.write(".claude-x/.claude.json", ACCOUNT_X);
        self.write(".claude-y/.claude.json", "{}");
        self.write(".claude.json", MAIN);
        self.write(".claude/runtime-config.json", RUNTIME);
        self.installed();
        self.write(".hangar/computer-control/targets/delphi-02-agent.json", self.h(json!(ssh_file("d"))).as_str().unwrap());
    }

    async fn put(&self, body: Value) -> (StatusCode, Value) {
        let (status, body) = self.call("PUT", "/api/computer-control", Some(self.h(body))).await;
        (status, without_running(body))
    }

    fn json_at(&self, rel: &str) -> Value {
        serde_json::from_str(&self.read(rel).unwrap_or_else(|| panic!("{rel} sumiu"))).unwrap()
    }

    fn snapshot(&self) -> Vec<Option<String>> {
        TOUCHED.iter().map(|rel| self.read(rel)).collect()
    }

    #[cfg(unix)]
    fn mode_of(&self, rel: &str) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(self.home.join(rel)).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn chmod(&self, rel: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(self.home.join(rel), std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Recusa: o código, os parâmetros e nenhum arquivo mudado.
    async fn refused(&self, body: Value, status: u16, code: &str, params: Value) {
        let before = self.snapshot();
        let got = self.put(body).await;
        self.assert_error(&got, status, code, params);
        assert_eq!(self.snapshot(), before, "{code}: arquivo mudou");
    }
}

fn on() -> Value {
    json!({"enabled": true, "mode": "package", "agent_config": AGENT})
}

fn entry(env: Value) -> Value {
    json!({"command": "«HOME»/.hangar/computer-control/hangar-computer-control", "args": [], "env": env})
}

/// O env que o PUT grava com o `RUNTIME` das capturas.
fn full_env() -> Value {
    json!({
        "HCC_AGENT_CONFIG": AGENT,
        "HCC_AGENTS_DIR": "«HOME»/.hangar/computer-control/targets",
        "TYPESAFE_API_KEY": "chave-do-jev-456",
        "JEV_ENDPOINT": "https://api.typesafe.ai/v1/systemone",
        "JEV_MODEL": "jev-latest",
        "LLM_PROXY_URL": "http://x/v1/chat/completions",
        "LLM_MODEL": "m1",
        "LLM_EFFORT": "high",
        "LLM_PROXY_KEY": "chave-do-llm-123"
    })
}

fn with_entry(document: &str, env: Value) -> Value {
    let mut data: Value = serde_json::from_str(document).unwrap();
    data["mcpServers"] = json!({"hangar-computer-control": entry(env)});
    data
}

/// A resposta de `r2-ligar-pacote` com D1.2 (`project_dir` "") e D1.5 (`unavailable` null).
fn on_state() -> Value {
    merged(with_ssh_target(installed_state(), "delphi-02", "d"), json!({
        "enabled": true,
        "llm_url": "http://x/v1/chat/completions",
        "llm_model": "m1",
        "llm_effort": "high",
        "llm_key_set": true,
        "llm_key_tail": "-123",
        "jev_key_set": true,
        "jev_key_tail": "-456",
        "jev_endpoint": "https://api.typesafe.ai/v1/systemone",
        "jev_model": "jev-latest",
        "files": [
            {"path": "«HOME»/.claude.json", "enabled": true},
            {"path": "«HOME»/.claude-x/.claude.json", "enabled": true},
            {"path": "«HOME»/.claude-y/.claude.json", "enabled": false}
        ]
    }))
}

fn off_state() -> Value {
    merged(on_state(), json!({
        "enabled": false,
        "files": [
            {"path": "«HOME»/.claude.json", "enabled": false},
            {"path": "«HOME»/.claude-x/.claude.json", "enabled": false},
            {"path": "«HOME»/.claude-y/.claude.json", "enabled": false}
        ]
    }))
}

/// ROT-07, ROT-12, ROT-17, ROT-19, ROT-68, ROT-74, ROT-76, JEV-03, JEV-04, JEV-07, D2.5 (r2-ligar-pacote):
/// principal e conta x recebem a entrada com o binário do pacote; y (`{}`) e z (sem arquivo) ficam.
#[tokio::test]
async fn rot07_turn_on_writes_every_claude_json() {
    let s = Scene::new().await;
    s.r2();
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, s.h(on_state()));
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, full_env())));
    assert_eq!(s.read(".claude-y/.claude.json").as_deref(), Some("{}"));
    assert!(s.read(".claude-z/.claude.json").is_none());
    // ROT-17: indentação 2, sem newline final.
    let text = s.read(".claude.json").unwrap();
    assert!(text.starts_with("{\n  \"numStartups\": 3,\n"), "{text}");
    assert!(text.ends_with('}'), "{text}");
    assert_eq!(s.hits(), 0);
}

/// ROT-07, ROT-78 (r2-modo-nulo): `mode` nulo é o pacote.
#[tokio::test]
async fn rot07_null_mode_is_package() {
    let s = Scene::new().await;
    s.r2();
    let (status, body) = s.put(json!({"enabled": true, "mode": null, "agent_config": AGENT})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
}

/// ROT-02, ROT-07 (r2-campo-a-mais, r2-modo-invalido, r2-sem-enabled): corpo fora de forma → 422.
#[tokio::test]
async fn rot07_strict_body() {
    let s = Scene::new().await;
    s.r2();
    let before = s.snapshot();
    for body in [
        json!({"enabled": true, "mode": "package", "agent_config": AGENT, "x": 1}),
        json!({"enabled": true, "mode": "foo", "agent_config": AGENT}),
        json!({"mode": "package"}),
    ] {
        let got = s.put(body.clone()).await;
        assert_eq!(got.0, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(got.1["detail"]["code"], "erro_corpo_invalido", "{body}");
    }
    assert_eq!(s.snapshot(), before);
    assert_eq!(s.hits(), 0);
}

/// ROT-01 (r2-sem-token), D1.1: sem token 401; convidado 403 dado pela rota; nada chega ao Python.
#[tokio::test]
async fn rot01_owner_only() {
    let s = Scene::new().await;
    s.r2();
    let before = s.snapshot();
    let got = s.call_as("PUT", "/api/computer-control", Some(s.h(on())), Who::Nobody).await;
    assert_eq!(got.0, StatusCode::UNAUTHORIZED);
    assert_eq!(got.1["detail"]["code"], "erro_nao_autorizado");
    let got = s.call_as("PUT", "/api/computer-control", Some(s.h(on())), Who::Guest).await;
    assert_eq!(got.0, StatusCode::FORBIDDEN);
    assert_eq!(got.1["detail"]["code"], "erro_fora_do_convite");
    assert_eq!(s.snapshot(), before);
    assert_eq!(s.hits(), 0);
}

/// ROT-66 (r2-jev-key, r2-jev-key-espacos, r2-jev-key-nula): chave digitada recusa antes de tudo;
/// só espaços ou nula passa.
#[tokio::test]
async fn rot66_typed_jev_key_is_refused() {
    let s = Scene::new().await;
    s.r2();
    s.refused(merged(on(), json!({"jev_key": "digitada"})), 400, "erro_computer_control_jev_key_moved", json!({})).await;
    s.refused(json!({"enabled": false, "jev_key": "digitada"}), 400, "erro_computer_control_jev_key_moved", json!({})).await;
    for key in [json!("   "), Value::Null] {
        let (status, body) = s.put(merged(on(), json!({"jev_key": key}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
}

/// ROT-73 (r2-modelo-diferente, r2-modelo-varios-campos, r2-cliproxy-sem-chave, r2-modelo-igual-passa).
#[tokio::test]
async fn rot73_old_client_model() {
    let s = Scene::new().await;
    s.r2();
    s.refused(merged(on(), json!({"llm_model": "outro"})), 400, "erro_computer_control_llm_movido",
        json!({"fields": "llm_model"})).await;
    s.refused(merged(on(), json!({"llm_url": "http://y/v1/chat/completions", "llm_key": "outra", "llm_effort": "high"})),
        400, "erro_computer_control_llm_movido", json!({"fields": "llm_url,llm_key"})).await;
    s.refused(merged(on(), json!({"use_cliproxy_key": true})), 400, "erro_computer_control_llm_movido",
        json!({"fields": "use_cliproxy_key"})).await;
    let same = json!({"llm_url": "http://x/v1/chat/completions", "llm_model": "m1", "llm_effort": "high", "llm_key": "chave-do-llm-123"});
    let (status, body) = s.put(merged(on(), same)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
}

/// ROT-72 (r2-alvo-inexistente, r2-alvo-vazio): o alvo padrão precisa existir.
#[tokio::test]
async fn rot72_default_target_must_exist() {
    let s = Scene::new().await;
    s.r2();
    let missing = "«HOME»/.hangar/computer-control/targets/nao-existe-agent.json";
    s.refused(merged(on(), json!({"agent_config": missing})), 400, "erro_computer_control_agent", json!({"file": missing})).await;
    s.refused(merged(on(), json!({"agent_config": ""})), 400, "erro_computer_control_agent", json!({"file": ""})).await;
}

/// D2.2 (r2-modo-local-com-binario, r2-modo-local-sem-binario): o modo local saiu; nada gravado.
#[tokio::test]
async fn d2_2_local_mode_asks_to_update_the_app() {
    let s = Scene::new().await;
    s.r2();
    let body = merged(on(), json!({"mode": "local", "project_dir": "«HOME»/Projetos/hangar-computer-control"}));
    s.refused(body.clone(), 400, "erro_computer_control_update_app", json!({})).await;
    s.write("Projetos/hangar-computer-control/target/release/hangar-computer-control", "ELF");
    s.refused(body, 400, "erro_computer_control_update_app", json!({})).await;
}

/// ROT-70, D2.3 (r2-nao-instalado, r2-uvx-legado, r2-uvx-gravado-sem-binario): sem o binário do pacote
/// recusa, mesmo com `uvx` no `install.json` ou na entrada gravada.
#[tokio::test]
async fn d2_3_without_package_binary_is_not_installed() {
    let s = Scene::new().await;
    s.r2();
    std::fs::remove_file(s.home.join(".hangar/computer-control/hangar-computer-control")).unwrap();
    s.write(".hangar/computer-control/install.json", "{\n  \"tag\": \"v1.0.0\",\n  \"uvx\": \"/bin/uvx\"\n}");
    s.refused(on(), 400, "erro_computer_control_not_installed", json!({})).await;
    std::fs::remove_file(s.home.join(".hangar/computer-control/install.json")).unwrap();
    s.write(".claude.json", r#"{"numStartups": 3, "mcpServers": {"outro": {"command": "x"}, "hangar-computer-control": {"command": "/custom/uvx", "args": ["--from", "pacote", "hangar-computer-control"], "env": {"LLM_MODEL": "velho"}}}}"#);
    s.refused(on(), 400, "erro_computer_control_not_installed", json!({})).await;
}

/// ROT-14 (r2-main-invalido), ROT-16/D2.1 (r2-mcpservers-nao-objeto-main): principal ruim recusa.
#[tokio::test]
async fn rot14_bad_main_file() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude.json", "{x");
    s.refused(on(), 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "JSON ou encoding inválido"})).await;
    s.write(".claude.json", r#"{"mcpServers": [1]}"#);
    s.refused(on(), 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude.json", "error": "mcpServers inválido"})).await;
}

/// ROT-77, B3, D2.1 (r2-mcpservers-nao-objeto-conta, r2-desligar-mcpservers-nao-objeto-conta): `mcpServers`
/// não objeto numa conta é erro visível ao ligar e ao desligar, e nenhum arquivo muda (o Python trocava
/// calado ou gravava a guardada e o principal antes do 500).
#[tokio::test]
async fn d2_1_account_mcp_servers_not_object_changes_nothing() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude-x/.claude.json", r#"{"mcpServers": "x", "oauthAccount": {}}"#);
    let params = json!({"file": "«HOME»/.claude-x/.claude.json", "error": "mcpServers inválido"});
    s.refused(on(), 500, "erro_computer_control_read", params.clone()).await;
    s.write(".claude.json", &s.h(json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"},
        "hangar-computer-control": entry(json!({"FOO": "a"}))}})).to_string());
    s.refused(json!({"enabled": false}), 500, "erro_computer_control_read", params).await;
    assert!(s.read(".hangar/computer-control.json").is_none());
}

/// ROT-04, JEV-06, D2.6 (r2-env-nao-objeto): `env` da entrada ativa que não é objeto → 500 com código.
#[tokio::test]
async fn d2_6_env_not_object() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude.json", &s.h(json!({"mcpServers": {"hangar-computer-control": {
        "command": "«HOME»/.hangar/computer-control/hangar-computer-control", "env": ["x"]}}})).to_string());
    s.refused(on(), 500, "erro_computer_control_io", json!({"file": "«HOME»/.claude.json", "error": "env inválido"})).await;
}

/// ROT-76, ROT-14, D2.7 (r2-conta-vazia-texto): conta com `.claude.json` de 0 bytes é pulada como `{}`;
/// as demais recebem a entrada (o Python dava 500 depois de gravar metade).
#[tokio::test]
async fn d2_7_empty_account_file_is_skipped() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude-y/.claude.json", "");
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, s.h(on_state()));
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, full_env())));
    assert_eq!(s.read(".claude-y/.claude.json").as_deref(), Some(""));
}

/// ROT-76 (r2-ligar-entrada-igual-nao-regrava): arquivo com a entrada igual não é regravado.
#[tokio::test]
async fn rot76_equal_entry_is_not_rewritten() {
    let s = Scene::new().await;
    s.r2();
    let x = s.h(json!({"oauthAccount": {"a": 1}, "mcpServers": {"hangar-computer-control": entry(full_env())}})).to_string();
    s.write(".claude-x/.claude.json", &x);
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.read(".claude-x/.claude.json"), Some(x));
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
}

/// ROT-76 (r2-main-ausente): sem principal, ele nasce só com `mcpServers`.
#[tokio::test]
async fn rot76_missing_main_is_created() {
    let s = Scene::new().await;
    s.r2();
    std::fs::remove_file(s.home.join(".claude.json")).unwrap();
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.json_at(".claude.json"), s.h(json!({"mcpServers": {"hangar-computer-control": entry(full_env())}})));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, full_env())));
    #[cfg(unix)]
    assert_eq!(s.mode_of(".claude.json"), 0o644);
}

/// ROT-75, HIS-03 (r2-ligar-env-antigo): env alheio fica, `PYTHONPATH` sai, gerenciadas trocadas.
#[tokio::test]
async fn rot75_foreign_env_stays_and_pythonpath_goes() {
    let s = Scene::new().await;
    s.r2();
    let old = json!({"VIRTUAL_ENV": "", "PYTHONPATH": "/x", "FOO": "bar", "HCC_AGENT_CONFIG": "/velho", "LLM_MODEL": "velho", "TYPESAFE_API_KEY": "velha"});
    s.write(".claude.json", &s.h(json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"},
        "hangar-computer-control": entry(old)}})).to_string());
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let env = merged(json!({"VIRTUAL_ENV": "", "FOO": "bar"}), full_env());
    assert_eq!(s.json_at(".claude.json"), s.h(json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"},
        "hangar-computer-control": entry(env.clone())}})));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, env)));
}

/// ROT-20, JEV-27 (r2-religar-da-guardada): religar parte do env da guardada; a guardada fica.
#[tokio::test]
async fn rot20_turning_on_again_uses_the_parked_env() {
    let s = Scene::new().await;
    s.r2();
    let parked = s.h(entry(json!({"FOO": "bar", "LLM_MODEL": "velho"}))).to_string();
    s.write(".hangar/computer-control.json", &parked);
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let env = merged(json!({"FOO": "bar"}), full_env());
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, env.clone())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, env)));
    assert_eq!(s.read(".hangar/computer-control.json"), Some(parked));
}

/// JEV-03, ROT-74 (r2-sem-jev): sem Jev nem auxiliar, só as `HCC_*`.
#[tokio::test]
async fn jev03_without_jev_only_hcc_vars() {
    let s = Scene::new().await;
    s.r2();
    std::fs::remove_file(s.home.join(".claude/runtime-config.json")).unwrap();
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, s.h(merged(on_state(), json!({
        "llm_url": "", "llm_model": "", "llm_effort": "", "llm_key_set": false, "llm_key_tail": "",
        "jev_key_set": false, "jev_key_tail": "", "jev_endpoint": "", "jev_model": ""
    }))));
    let env = json!({"HCC_AGENT_CONFIG": AGENT, "HCC_AGENTS_DIR": "«HOME»/.hangar/computer-control/targets"});
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, env.clone())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, env)));
}

/// JEV-03, JEV-04 (r2-jev-openrouter-query): Jev pelo OpenRouter (chave `sk-or-`) e base com query.
#[tokio::test]
async fn jev04_openrouter_and_query() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude/runtime-config.json", r#"{"jev_api_key": "sk-or-k", "jev_aux_base_url": "https://a.b/v1?x=1", "jev_aux_modelo": "m"}"#);
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let env = json!({
        "HCC_AGENT_CONFIG": AGENT,
        "HCC_AGENTS_DIR": "«HOME»/.hangar/computer-control/targets",
        "TYPESAFE_API_KEY": "sk-or-k",
        "JEV_ENDPOINT": "https://openrouter.ai/api/alpha/decisions",
        "JEV_MODEL": "~typesafe/jev-latest",
        "LLM_PROXY_URL": "https://a.b/v1/chat/completions?x=1",
        "LLM_MODEL": "m"
    });
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, env.clone())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(ACCOUNT_X, env)));
}

/// ROT-21, ROT-67, JEV-09, HIS-01 (r2-desligar): desligar guarda a ativa (0600) e tira de todos os que a
/// tinham; os demais MCP ficam.
#[tokio::test]
async fn rot67_turn_off_parks_and_removes() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude.json", &s.h(json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"},
        "hangar-computer-control": entry(full_env())}})).to_string());
    s.write(".claude-x/.claude.json", &s.h(json!({"oauthAccount": {"a": 1},
        "mcpServers": {"hangar-computer-control": entry(full_env())}})).to_string());
    let (status, body) = s.put(json!({"enabled": false})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, s.h(off_state()));
    assert_eq!(s.json_at(".hangar/computer-control.json"), s.h(entry(full_env())));
    #[cfg(unix)]
    assert_eq!(s.mode_of(".hangar/computer-control.json"), 0o600);
    assert_eq!(s.json_at(".claude.json"), json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"}}}));
    assert_eq!(s.json_at(".claude-x/.claude.json"), json!({"oauthAccount": {"a": 1}, "mcpServers": {}}));
    assert_eq!(s.read(".claude-y/.claude.json").as_deref(), Some("{}"));
}

/// ROT-21, ROT-67 (r2-desligar-guardada-existente): a guardada anterior é sobrescrita e vira 0600.
#[tokio::test]
async fn rot67_turn_off_overwrites_the_parked_entry() {
    let s = Scene::new().await;
    s.r2();
    s.write(".claude.json", &s.h(json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"},
        "hangar-computer-control": entry(json!({"FOO": "novo"}))}})).to_string());
    s.write(".hangar/computer-control.json", &s.h(entry(json!({"FOO": "antigo"}))).to_string());
    #[cfg(unix)]
    s.chmod(".hangar/computer-control.json", 0o644);
    let (status, body) = s.put(json!({"enabled": false})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["agent_config"], s.h(json!(AGENT)));
    assert_eq!(s.json_at(".hangar/computer-control.json"), s.h(entry(json!({"FOO": "novo"}))));
    #[cfg(unix)]
    assert_eq!(s.mode_of(".hangar/computer-control.json"), 0o600);
    assert_eq!(s.json_at(".claude.json"), json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"}}}));
}

/// ROT-67 (r2-desligar-sem-entrada): desligar o que já está desligado não grava nada.
#[tokio::test]
async fn rot67_turn_off_without_entry_changes_nothing() {
    let s = Scene::new().await;
    s.r2();
    let before = s.snapshot();
    let (status, body) = s.put(json!({"enabled": false})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(s.snapshot(), before);
}

/// JEV-16, D2.4 (r2-link-e-modo): o principal fica com o modo que tinha (o Python o deixava 0644); a conta
/// que é link para fora é gravada pelo escritor do Rust: o arquivo de fora não muda e o modo vem dele.
#[cfg(unix)]
#[tokio::test]
async fn d2_4_mode_and_link_are_kept() {
    let s = Scene::new().await;
    s.r2();
    s.chmod(".claude.json", 0o640);
    std::fs::remove_file(s.home.join(".claude-x/.claude.json")).unwrap();
    s.write("real-x.json", r#"{"oauthAccount":{"a":1}}"#);
    s.chmod("real-x.json", 0o600);
    std::os::unix::fs::symlink(s.home.join("real-x.json"), s.home.join(".claude-x/.claude.json")).unwrap();
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.mode_of(".claude.json"), 0o640);
    assert_eq!(s.json_at(".claude.json"), s.h(with_entry(MAIN, full_env())));
    assert_eq!(s.json_at(".claude-x/.claude.json"), s.h(with_entry(r#"{"oauthAccount":{"a":1}}"#, full_env())));
    assert_eq!(s.mode_of(".claude-x/.claude.json"), 0o600);
    assert_eq!(s.read("real-x.json").as_deref(), Some(r#"{"oauthAccount":{"a":1}}"#));
    // Nenhuma trava nem versão deslocada fica para trás.
    let leftovers: Vec<_> = std::fs::read_dir(&s.home).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock") || name.starts_with(".hangar-recovery-")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// D2.4, JEV-15: a gravação espera a trava do Claude Code no `.claude.json`; ocupada, recusa sem gravar.
#[tokio::test]
async fn d2_4_put_takes_the_claude_lock() {
    let s = Scene::new().await;
    s.r2();
    let lock = hangar_core::config_lock::lock_config(&s.home.join(".claude.json")).unwrap();
    let before = s.snapshot();
    let got = s.put(on()).await;
    assert_eq!(got.0, StatusCode::INTERNAL_SERVER_ERROR, "{}", got.1);
    assert_eq!(got.1["detail"]["code"], "erro_computer_control_io", "{}", got.1);
    assert_eq!(s.snapshot(), before);
    drop(lock);
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// D2.4: a trava de uma conta ocupada também recusa sem gravar nada, nem o principal.
#[tokio::test]
async fn d2_4_put_takes_the_account_lock() {
    let s = Scene::new().await;
    s.r2();
    let lock = hangar_core::config_lock::lock_config(&s.home.join(".claude-x/.claude.json")).unwrap();
    let before = s.snapshot();
    let got = s.put(on()).await;
    assert_eq!(got.0, StatusCode::INTERNAL_SERVER_ERROR, "{}", got.1);
    assert_eq!(got.1["detail"]["code"], "erro_computer_control_io", "{}", got.1);
    assert_eq!(s.snapshot(), before);
    drop(lock);
}

/// ROT-16, D2.1: só `null` vale como `mcpServers` ausente; `[]`, `0` e `""` recusam sem gravar,
/// no principal ou numa conta, ao ligar e ao desligar.
#[tokio::test]
async fn d2_1_only_null_mcp_servers_counts_as_absent() {
    let s = Scene::new().await;
    s.r2();
    for servers in ["[]", "0", "\"\""] {
        s.write(".claude.json", format!(r#"{{"mcpServers": {servers}}}"#));
        let params = json!({"file": "«HOME»/.claude.json", "error": "mcpServers inválido"});
        s.refused(on(), 500, "erro_computer_control_read", params.clone()).await;
        s.refused(json!({"enabled": false}), 500, "erro_computer_control_read", params).await;
    }
    s.write(".claude.json", MAIN);
    s.write(".claude-x/.claude.json", r#"{"mcpServers": ""}"#);
    s.refused(on(), 500, "erro_computer_control_read",
        json!({"file": "«HOME»/.claude-x/.claude.json", "error": "mcpServers inválido"})).await;
    s.write(".claude-x/.claude.json", ACCOUNT_X);
    s.write(".claude.json", r#"{"mcpServers": null, "a": 1}"#);
    let (status, body) = s.put(on()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s.json_at(".claude.json"), s.h(json!({"mcpServers": {"hangar-computer-control": entry(full_env())}, "a": 1})));
}

/// Escritor único: o repasse do Jev do `/api/config` (`RuntimeConfig::sync_jev`) e a migração da subida
/// (R5) gravam pelo escritor do crate, que lê o surrogate solto que o Claude Code grava e o devolve igual
/// (o gravador antigo do servidor recusava esse arquivo).
#[tokio::test]
async fn jev_publish_and_startup_go_through_the_single_writer() {
    let s = Scene::new().await;
    s.r2();
    let main = |key: &str| {
        format!(r#"{{"a": "x\ud800y", "mcpServers": {{"hangar-computer-control": {{"command": "c", "env": {{"FOO": "1", "TYPESAFE_API_KEY": "{key}"}}}}}}}}"#)
    };
    s.write(".claude.json", main("velha"));
    s.chmod_if_unix(".claude.json", 0o640);
    let home = s.home.to_string_lossy().into_owned();
    let env = AccountEnvironment::from_map(BTreeMap::from([
        ("HOME".into(), home.clone()),
        ("USERPROFILE".into(), home),
        ("PATH".into(), s.bin.to_string_lossy().into_owned()),
    ]));
    let config = RuntimeConfig::new(env).unwrap();
    config.sync_jev().unwrap();
    let text = s.read(".claude.json").unwrap();
    assert!(text.contains(r#""a": "x\ud800y""#), "{text}");
    // O `serde_json` recusa o surrogate solto; o leitor do Python o aceita.
    let data = loads_lossless(&text).unwrap();
    assert_eq!(data["mcpServers"]["hangar-computer-control"]["env"]["TYPESAFE_API_KEY"], "chave-do-jev-456");
    assert_eq!(data["mcpServers"]["hangar-computer-control"]["env"]["FOO"], "1");
    #[cfg(unix)]
    assert_eq!(s.mode_of(".claude.json"), 0o640);
    // R5: com a marca, a subida só repassa o Jev, pelo mesmo escritor.
    s.write(".claude.json", main("velha"));
    s.write(".hangar/jev-migrated", "");
    config.startup_jev_migration();
    let text = s.read(".claude.json").unwrap();
    assert!(text.contains(r#""a": "x\ud800y""#), "{text}");
    let data = loads_lossless(&text).unwrap();
    assert_eq!(data["mcpServers"]["hangar-computer-control"]["env"]["TYPESAFE_API_KEY"], "chave-do-jev-456");
}

impl Scene {
    fn chmod_if_unix(&self, rel: &str, mode: u32) {
        #[cfg(unix)]
        self.chmod(rel, mode);
        #[cfg(not(unix))]
        let _ = (rel, mode);
    }
}
