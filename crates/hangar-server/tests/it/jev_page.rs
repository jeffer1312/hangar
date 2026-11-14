//! Gravar a página Jev (`RuntimeConfig::apply`) e o que chega ao Computer Use. O código já era do
//! Rust; a prova era só dos pytests que saíram (`test_runtime_config.py` e `test_computer_control.py`,
//! ponta 7ef9011ba), de onde vêm os valores esperados.
use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::config_sync::{jev_startup, runtime::RuntimeConfig};
use hangar_server::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};

const MAIN: &str = ".claude.json";
const ACCOUNT: &str = ".claude-x/.claude.json";
const RUNTIME: &str = ".claude/runtime-config.json";
const PARKED: &str = ".hangar/computer-control.json";
const MARKER: &str = ".hangar/jev-migrated";
const OLD_KEYS: &str = ".hangar/jev-chave-antiga-computer-use.json";
const OLD_LLM: &str = ".hangar/computer-use-llm-antigo.json";
const OPENROUTER: &str = "https://openrouter.ai/api/alpha/decisions";
const NOT_PUBLISHED: &str = "jev: salvo, mas o Computer Use não recebeu a configuração: ";

struct Home {
    dir: tempfile::TempDir,
    extra: Vec<(String, String)>,
}

impl Home {
    fn new() -> Self {
        Self::with_env(&[])
    }
    fn with_env(extra: &[(&str, &str)]) -> Self {
        let extra = extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        Self { dir: tempfile::tempdir().expect("pasta temporária"), extra }
    }
    fn at(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn source(&self, rel: &str) -> String {
        self.at(rel).to_string_lossy().into_owned()
    }
    fn text(&self, rel: &str, text: &str) {
        let path = self.at(rel);
        std::fs::create_dir_all(path.parent().expect("pasta")).expect("pasta criada");
        std::fs::write(path, text).expect("arquivo escrito");
    }
    fn put(&self, rel: &str, value: &Value) {
        self.text(rel, &value.to_string());
    }
    fn bytes(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.at(rel)).expect(rel)
    }
    fn json(&self, rel: &str) -> Value {
        serde_json::from_slice(&self.bytes(rel)).expect(rel)
    }
    fn exists(&self, rel: &str) -> bool {
        self.at(rel).exists()
    }
    fn command(&self) -> String {
        self.at(".hangar/computer-control/hangar-computer-control").to_string_lossy().into_owned()
    }
    fn entry(&self, env: Value) -> Value {
        json!({"command": self.command(), "args": [], "env": env})
    }
    fn main_doc(&self, env: Value) -> Value {
        json!({"numStartups": 3, "mcpServers": {"outro": {"command": "x"}, "hangar-computer-control": self.entry(env)}})
    }
    fn account_doc(&self, env: Value) -> Value {
        json!({"oauthAccount": {"a": 1}, "mcpServers": {"hangar-computer-control": self.entry(env)}})
    }
    fn main(&self, env: Value) {
        self.put(MAIN, &self.main_doc(env));
    }
    /// Conta `~/.claude-x` que o catálogo acha: credencial e um projeto.
    fn account_text(&self, text: &str) {
        self.text(ACCOUNT, text);
        self.text(".claude-x/.credentials.json", "{}");
        self.text(".claude-x/projects/p/s.jsonl", "{}\n");
    }
    fn account(&self, env: Value) {
        self.account_text(&self.account_doc(env).to_string());
    }
    fn env(&self) -> AccountEnvironment {
        let home = self.dir.path().to_string_lossy().into_owned();
        let mut pairs = vec![("HOME".to_owned(), home.clone()), ("USERPROFILE".to_owned(), home), ("PATH".to_owned(), String::new())];
        pairs.extend(self.extra.iter().cloned());
        AccountEnvironment::from_pairs(pairs)
    }
    fn config(&self) -> RuntimeConfig {
        let env = self.env();
        let settings = SettingsSnapshot::capture(self.dir.path(), None, SettingsEnvironment::from_pairs(env.ordered_pairs()))
            .expect("configuração do servidor");
        RuntimeConfig::from_snapshot(env, Arc::new(settings)).expect("runtime-config")
    }
    /// A gravação da página: `changes` e os campos a remover.
    fn apply(&self, changes: Value, remove: &[&str]) -> Result<Map<String, Value>, String> {
        let changes = changes.as_object().cloned().unwrap_or_default();
        let remove: BTreeSet<String> = remove.iter().map(|key| (*key).to_owned()).collect();
        self.config().apply(&changes, &remove)
    }
    fn start(&self) {
        self.config().startup_jev_migration();
    }
    fn active_env(&self, rel: &str) -> Value {
        self.json(rel)["mcpServers"]["hangar-computer-control"]["env"].clone()
    }
    /// Todo o texto do diário desta pasta.
    fn diary_text(&self) -> String {
        let Some(dir) = hangar_server::diag::diary_dir_in(&self.env()) else { return String::new() };
        let Ok(files) = std::fs::read_dir(dir) else { return String::new() };
        files.map(|file| std::fs::read_to_string(file.expect("arquivo do diário").path()).expect("diário")).collect()
    }
    #[cfg(unix)]
    fn mode(&self, rel: &str) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(self.at(rel)).expect(rel).permissions().mode() & 0o777
    }
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
fn logged(run: impl FnOnce()) -> String {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = log.0.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn with_jev(mut env: Value, key: &str) -> Value {
    env["TYPESAFE_API_KEY"] = json!(key);
    env["JEV_ENDPOINT"] = json!("https://api.typesafe.ai/v1/systemone");
    env["JEV_MODEL"] = json!("jev-latest");
    env
}

/// JEV-02, HIS-46 (`test_jev_aux_cliproxy_usa_a_chave_do_servidor`): sem chave no CLIProxy a gravação
/// recusa e nada é salvo; com chave, ela vira `jev_aux_api_key` e o campo do pedido não persiste.
#[test]
fn jev02_cliproxy_key_comes_from_the_server_config() {
    let home = Home::new();
    let error = home.apply(json!({"jev_aux_cliproxy": true}), &[]).unwrap_err();
    assert!(error.contains("jev_aux_cliproxy"), "{error}");
    assert!(!home.exists(RUNTIME));
    home.text(".cli-proxy-api/config.yaml", "access:\n  api-keys:\n    - proxy-key\n");
    home.apply(json!({"jev_aux_base_url": "http://127.0.0.1:8317/v1", "jev_aux_cliproxy": true}), &[]).unwrap();
    let config = home.config();
    assert_eq!(config.get("jev_aux_api_key"), json!("proxy-key"));
    assert!(config.state().unwrap().get("jev_aux_cliproxy").is_none());
    assert!(home.json(RUNTIME).get("jev_aux_cliproxy").is_none());
}

/// JEV-13, HIS-19, HIS-45 (`test_invalid_computer_use_config_keeps_jev_settings_saved`): arquivo do
/// Computer Use inválido → erro "Computer Use", a chave da página fica salva e o arquivo intacto.
#[test]
fn jev13_invalid_computer_use_files_keep_the_page_saved() {
    for (rel, content) in [
        (MAIN, "{invalid-secret"),
        (MAIN, r#"{"mcpServers": "invalid-secret"}"#),
        (PARKED, r#"["invalid-secret"]"#),
    ] {
        let home = Home::new();
        home.text(MAIN, "{}");
        home.text(rel, content);
        assert!(home.config().state().unwrap().get("upload_retention_days").is_some());
        let error = home.apply(json!({"jev_api_key": "replacement-secret"}), &[]).unwrap_err();
        assert!(error.starts_with(NOT_PUBLISHED), "{rel}: {error}");
        assert!(error.contains("Computer Use"), "{rel}: {error}");
        assert_eq!(home.config().get("jev_api_key"), json!("replacement-secret"), "{rel}");
        assert_eq!(home.bytes(rel), content.as_bytes(), "{rel}");
    }
}

/// JEV-14, HIS-12, HIS-45, HIS-73 (`test_jev_config_reaches_active_and_parked_entries_only_in_jev_vars`):
/// gravar a chave, o modelo ou remover a chave pela página chega à ativa, à conta e à guardada, só
/// nas variáveis do Jev.
#[test]
fn jev14_page_jev_fields_reach_active_and_parked_entries() {
    let home = Home::new();
    let env = json!({"HCC_AGENT_CONFIG": "a", "FOO": "x"});
    home.main(env.clone());
    home.account(env.clone());
    home.put(PARKED, &home.entry(json!({"HCC_AGENT_CONFIG": "a"})));
    home.apply(json!({"jev_api_key": "sk-or-segredo"}), &[]).unwrap();
    let jev = |mut env: Value| {
        env["TYPESAFE_API_KEY"] = json!("sk-or-segredo");
        env["JEV_ENDPOINT"] = json!(OPENROUTER);
        env["JEV_MODEL"] = json!("~typesafe/jev-latest");
        env
    };
    assert_eq!(home.json(MAIN), home.main_doc(jev(env.clone())));
    assert_eq!(home.json(ACCOUNT), home.account_doc(jev(env.clone())));
    assert_eq!(home.json(PARKED), home.entry(jev(json!({"HCC_AGENT_CONFIG": "a"}))));
    let state = home.config().state().unwrap();
    assert!(!state.to_string().contains("sk-or-segredo"));
    assert!(state.get("jev_windows_api_key").is_none());
    home.apply(json!({"jev_model": "typesafe/jev-1.13-20260917"}), &[]).unwrap();
    assert_eq!(home.active_env(MAIN)["JEV_MODEL"], "typesafe/jev-1.13-20260917");
    home.apply(json!({}), &["jev_api_key"]).unwrap();
    assert_eq!(home.json(MAIN), home.main_doc(env.clone()));
    assert_eq!(home.json(ACCOUNT), home.account_doc(env));
    assert_eq!(home.json(PARKED), home.entry(json!({"HCC_AGENT_CONFIG": "a"})));
}

/// JEV-14: o repasse só roda quando muda campo do Jev ou do auxiliar. Com o `.claude.json` ilegível,
/// outro campo grava sem erro (o arquivo nem é lido); campo do Jev, ou a remoção da chave, devolve o
/// erro do repasse.
#[test]
fn jev14_other_fields_do_not_publish() {
    let home = Home::new();
    home.text(MAIN, "{x");
    home.apply(json!({"upload_retention_days": 7}), &[]).unwrap();
    assert_eq!(home.config().get("upload_retention_days"), json!(7));
    let error = home.apply(json!({"jev_model": "typesafe/jev-1.13-20260917"}), &[]).unwrap_err();
    assert!(error.starts_with(NOT_PUBLISHED), "{error}");
    let error = home.apply(json!({}), &["jev_model"]).unwrap_err();
    assert!(error.starts_with(NOT_PUBLISHED), "{error}");
    assert_eq!(home.bytes(MAIN), b"{x");

    // Com a entrada boa, gravar outro campo não regrava o `.claude.json`.
    let home = Home::new();
    home.main(json!({"HCC_AGENT_CONFIG": "a"}));
    let before = home.bytes(MAIN);
    home.apply(json!({"upload_retention_days": 7}), &[]).unwrap();
    assert_eq!(home.bytes(MAIN), before);
}

/// HIS-39 (`test_put_with_a_different_model_is_refused_and_aux_feeds_the_mcp`): trocar
/// `jev_aux_modelo` na página chega na hora ao `LLM_*` do MCP.
#[test]
fn his39_page_aux_model_feeds_the_mcp() {
    let home = Home::new();
    home.put(RUNTIME, &json!({"jev_aux_base_url": "http://x/v1", "jev_aux_modelo": "m1", "jev_aux_esforco": "high",
        "jev_aux_api_key": "chave-do-llm-123"}));
    home.main(json!({"HCC_AGENT_CONFIG": "a", "LLM_PROXY_URL": "http://x/v1/chat/completions", "LLM_MODEL": "m1",
        "LLM_EFFORT": "high", "LLM_PROXY_KEY": "chave-do-llm-123"}));
    home.apply(json!({"jev_aux_modelo": "m2"}), &[]).unwrap();
    let env = home.active_env(MAIN);
    assert_eq!(env["LLM_MODEL"], "m2");
    assert_eq!(env["LLM_PROXY_URL"], "http://x/v1/chat/completions");
}

/// JEV-12, HIS-50, HIS-42 (`test_same_choice_in_both_merges_and_a_replaced_model_is_kept`): a mesma
/// escolha nas duas origens não guarda nada; a página trocar o modelo guarda o antigo do MCP por
/// arquivo de origem, só o primeiro de cada um, e a guardada também é origem.
#[test]
fn his50_replaced_model_is_kept_once_per_source() {
    let home = Home::new();
    home.put(RUNTIME, &json!({"jev_texto_base_url": "https://api.anthropic.com/v1", "jev_texto_api_key": "k",
        "jev_texto_modelo": "claude-haiku-5-5"}));
    let haiku = json!({"LLM_PROXY_URL": "https://api.anthropic.com/v1/chat/completions", "LLM_MODEL": "claude-haiku-5-5",
        "LLM_EFFORT": "low", "LLM_PROXY_KEY": "k"});
    let mut env = haiku.clone();
    env["HCC_AGENT_CONFIG"] = json!("a");
    home.main(env);
    home.start();
    let config = home.config();
    let aux: Vec<Value> = ["jev_aux_base_url", "jev_aux_api_key", "jev_aux_modelo", "jev_aux_esforco"]
        .iter().map(|key| config.get(key)).collect();
    assert_eq!(aux, [json!("https://api.anthropic.com/v1"), json!("k"), json!("claude-haiku-5-5"), json!("low")]);
    assert!(!home.exists(OLD_LLM), "nada mudou no MCP");

    home.apply(json!({"jev_aux_modelo": "claude-sonnet-5-5"}), &[]).unwrap();
    assert_eq!(home.json(OLD_LLM)[home.source(MAIN)]["LLM_MODEL"], "claude-haiku-5-5");
    assert_eq!(home.active_env(MAIN)["LLM_MODEL"], "claude-sonnet-5-5");
    #[cfg(unix)]
    assert_eq!(home.mode(OLD_LLM), 0o600);

    // A segunda troca não sobrescreve o que já foi guardado daquela origem.
    let kept = home.bytes(OLD_LLM);
    home.apply(json!({"jev_aux_modelo": "claude-opus-5-5"}), &[]).unwrap();
    assert_eq!(home.bytes(OLD_LLM), kept);
    assert_eq!(home.active_env(MAIN)["LLM_MODEL"], "claude-opus-5-5");

    // A guardada é outra origem: o modelo dela fica guardado na primeira troca.
    let parked = json!({"LLM_PROXY_URL": "https://p/v1/chat/completions", "LLM_MODEL": "mp"});
    home.put(PARKED, &home.entry(parked.clone()));
    home.apply(json!({"jev_aux_esforco": "high"}), &[]).unwrap();
    let mut expected = Map::new();
    expected.insert(home.source(MAIN), haiku);
    expected.insert(home.source(PARKED), parked);
    assert_eq!(home.json(OLD_LLM), Value::Object(expected));
}

/// JEV-21: origens diferentes com o mesmo número de campos: vence o navegador.
#[test]
fn jev21_tie_between_sources_keeps_the_browser() {
    let home = Home::new();
    home.main(json!({"LLM_PROXY_URL": "https://b/v1/chat/completions", "LLM_MODEL": "mb", "LLM_EFFORT": "high"}));
    home.put(RUNTIME, &json!({"jev_texto_base_url": "https://a/v1", "jev_texto_api_key": "ka", "jev_texto_modelo": "ma"}));
    home.start();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_texto_base_url": "https://a/v1", "jev_texto_api_key": "ka", "jev_texto_modelo": "ma",
            "jev_aux_base_url": "https://a/v1", "jev_aux_api_key": "ka", "jev_aux_modelo": "ma"})
    );
    assert_eq!(
        home.json(MAIN),
        home.main_doc(json!({"LLM_PROXY_URL": "https://a/v1/chat/completions", "LLM_MODEL": "ma", "LLM_PROXY_KEY": "ka"}))
    );
}

/// JEV-22, HIS-47, HIS-75 (`test_jev_aux_primeira_gravacao_recusa_com_legado_ilegivel`): com o modelo
/// antigo ilegível, ler o auxiliar não derruba (vale vazio) e a primeira gravação de `jev_aux_*`
/// recusa sem gravar.
#[test]
fn jev22_unreadable_old_model_reads_empty_and_refuses_the_first_write() {
    let home = Home::new();
    home.text(MAIN, "{x");
    assert_eq!(home.config().get("jev_aux_modelo"), json!(""));
    let error = home.apply(json!({"jev_aux_modelo": "m"}), &[]).unwrap_err();
    assert!(error.contains("modelo antigo"), "{error}");
    assert!(error.starts_with("jev_aux: não consegui ler o modelo antigo do Computer Use ("), "{error}");
    assert!(error.ends_with("); tente de novo"), "{error}");
    assert!(!home.exists(RUNTIME));
    assert_eq!(home.bytes(MAIN), b"{x");
}

/// JEV-23 (`test_jev_aux_primeira_gravacao_leva_o_legado_e_valida`): a primeira gravação de um
/// `jev_aux_*` leva junto o auxiliar antigo; esforço e endereço continuam validados.
#[test]
fn jev23_first_aux_write_takes_the_old_model_along() {
    let home = Home::new();
    home.put(RUNTIME, &json!({"jev_texto_base_url": "https://llm.exemplo/v1", "jev_texto_api_key": "k", "jev_texto_modelo": "mini"}));
    home.apply(json!({"jev_aux_modelo": "outro"}), &[]).unwrap();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_texto_base_url": "https://llm.exemplo/v1", "jev_texto_api_key": "k", "jev_texto_modelo": "mini",
            "jev_aux_base_url": "https://llm.exemplo/v1", "jev_aux_api_key": "k", "jev_aux_modelo": "outro"})
    );
    assert_ne!(home.config().state().unwrap()["jev_aux_api_key"]["valor"], "k", "segredo mascarado");
    let error = home.apply(json!({"jev_aux_esforco": "max"}), &[]).unwrap_err();
    assert!(error.contains("jev_aux_esforco"), "{error}");
    let error = home.apply(json!({"jev_aux_base_url": "llm.exemplo"}), &[]).unwrap_err();
    assert!(error.contains("jev_aux_base_url"), "{error}");
}

/// JEV-24, HIS-16 (`test_old_clients_sending_a_computer_use_key_get_an_error`): chave digitada em
/// `jev_windows_api_key` recusa sem gravar; a máscara (rascunho inteiro de cliente antigo) e só
/// espaços passam com o resto salvo.
#[test]
fn jev24_typed_computer_use_key_is_refused_and_the_mask_passes() {
    let home = Home::new();
    let error = home.apply(json!({"jev_windows_api_key": "typed-key"}), &[]).unwrap_err();
    assert!(error.contains("página Jev"), "{error}");
    assert!(!home.exists(RUNTIME));
    home.apply(json!({"jev_windows_api_key": "••••1234", "upload_retention_days": 7}), &[]).unwrap();
    assert_eq!(home.config().get("upload_retention_days"), json!(7));
    home.apply(json!({"jev_windows_api_key": "   ", "upload_retention_days": 8}), &[]).unwrap();
    assert_eq!(home.config().get("upload_retention_days"), json!(8));
    assert!(home.json(RUNTIME).get("jev_windows_api_key").is_none());
    assert!(home.config().state().unwrap().get("jev_windows_api_key").is_none());
}

/// HIS-72: o MCP recebe o Jev do runtime-config; `TYPESAFE_API_KEY`, `JEV_ENDPOINT` e `JEV_MODEL` no
/// ambiente do servidor não entram, nem depois de a chave da página ser removida.
#[test]
fn his72_saved_config_wins_over_the_environment() {
    let home = Home::with_env(&[
        ("TYPESAFE_API_KEY", "chave-do-ambiente"),
        ("JEV_ENDPOINT", "https://ambiente.invalid/v1"),
        ("JEV_MODEL", "modelo-do-ambiente"),
    ]);
    home.main(json!({"HCC_AGENT_CONFIG": "a"}));
    home.apply(json!({"jev_api_key": "chave-pagina"}), &[]).unwrap();
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({"HCC_AGENT_CONFIG": "a"}), "chave-pagina")));
    home.apply(json!({}), &["jev_api_key"]).unwrap();
    assert_eq!(home.json(MAIN), home.main_doc(json!({"HCC_AGENT_CONFIG": "a"})));
}

/// JEV-06: entrada sem `env` (ausente ou `null`) vale `{}` e recebe só as variáveis do Jev.
#[test]
fn jev06_entry_without_env_gets_only_the_jev() {
    let home = Home::new();
    let command = home.command();
    home.put(MAIN, &json!({"mcpServers": {"hangar-computer-control": {"command": command, "args": []}}}));
    home.account_text(&json!({"mcpServers": {"hangar-computer-control": {"command": command, "args": [], "env": null}}}).to_string());
    home.apply(json!({"jev_api_key": "chave-pagina"}), &[]).unwrap();
    let expected = json!({"mcpServers": {"hangar-computer-control": {"command": command, "args": [], "env": with_jev(json!({}), "chave-pagina")}}});
    assert_eq!(home.json(MAIN), expected);
    assert_eq!(home.json(ACCOUNT), expected);
}

/// HIS-53: a migração que falha registra a etapa; nem o diário nem o log levam pedaço de chave
/// (antes o log levava os 4 finais).
#[test]
fn his53_diary_and_log_never_carry_a_key_piece() {
    let (old, new) = ("velha-ZQXW", "nova-JKVH");
    // Chave antiga sem poder ser guardada.
    let home = Home::new();
    home.account(json!({"TYPESAFE_API_KEY": old}));
    home.main(json!({"TYPESAFE_API_KEY": old}));
    home.put(RUNTIME, &json!({"jev_api_key": new}));
    home.text(OLD_KEYS, "{x");
    home.text(MARKER, "");
    let log = logged(|| home.start());
    let diary = home.diary_text();
    assert!(diary.contains(jev_startup::FAILURE), "a falha vai ao diário: {diary}");
    for piece in [old, new, "ZQXW", "JKVH"] {
        assert!(!diary.contains(piece), "diário com {piece}: {diary}");
        assert!(!log.contains(piece), "log com {piece}: {log}");
    }

    // `env` que não é objeto, com a chave dentro.
    let home = Home::new();
    home.put(MAIN, &json!({"mcpServers": {"hangar-computer-control": {"command": home.command(), "env": [old]}}}));
    home.put(RUNTIME, &json!({"jev_api_key": new}));
    let log = logged(|| home.start());
    let diary = home.diary_text();
    assert!(diary.contains(jev_startup::FAILURE), "a falha vai ao diário: {diary}");
    for piece in [old, new, "ZQXW", "JKVH"] {
        assert!(!diary.contains(piece), "diário com {piece}: {diary}");
        assert!(!log.contains(piece), "log com {piece}: {log}");
    }
}
