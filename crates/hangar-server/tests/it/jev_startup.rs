//! Migração do Jev na subida (R5). Estados e resultados vêm das capturas do Python (`r5-*.json`, ponta
//! 7ef9011ba) e dos pytests de `test_computer_control.py`; a chave antiga segue o formato por arquivo de
//! origem (D5.2) e a falha vai ao diário (D5.3).
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};

use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::config_sync::{jev_startup, runtime::RuntimeConfig};
use hangar_server::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};

const MAIN: &str = ".claude.json";
const ACCOUNT: &str = ".claude-x/.claude.json";
const RUNTIME: &str = ".claude/runtime-config.json";
const SETTINGS: &str = ".claude/settings.json";
const PARKED: &str = ".hangar/computer-control.json";
const MARKER: &str = ".hangar/jev-migrated";
const OLD_KEYS: &str = ".hangar/jev-chave-antiga-computer-use.json";
const OLD_LLM: &str = ".hangar/computer-use-llm-antigo.json";

struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("pasta temporária"))
    }
    fn at(&self, rel: &str) -> PathBuf {
        self.0.path().join(rel)
    }
    /// O caminho como o escritor o monta: um componente por vez (no Windows, com `\`).
    fn source(&self, rel: &str) -> String {
        rel.split('/').fold(self.0.path().to_owned(), |path, part| path.join(part)).to_string_lossy().into_owned()
    }
    /// O catálogo de contas dá a pasta da conta já resolvida, pela mesma função da produção.
    fn account_source(&self) -> String {
        let dir = hangar_server::accounts::catalog::resolved(&self.at(".claude-x"));
        dir.join(".claude.json").to_string_lossy().into_owned()
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
    fn entry(&self, env: Value) -> Value {
        let command = self.at(".hangar/computer-control/hangar-computer-control");
        json!({"command": command.to_string_lossy(), "args": [], "env": env})
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
    /// Conta `~/.claude-x` como nas capturas: credencial e um projeto.
    fn account_text(&self, text: &str) {
        self.text(ACCOUNT, text);
        self.text(".claude-x/.credentials.json", "{}");
        self.text(".claude-x/projects/p/s.jsonl", "{}\n");
    }
    fn account(&self, env: Value) {
        self.account_text(&self.account_doc(env).to_string());
    }
    fn mark(&self) {
        self.text(MARKER, "");
    }
    fn env(&self) -> AccountEnvironment {
        let home = self.0.path().to_string_lossy().into_owned();
        AccountEnvironment::from_pairs([
            ("HOME", home.clone()),
            ("USERPROFILE", home),
            ("PATH", String::new()),
        ])
    }
    fn config(&self) -> RuntimeConfig {
        let env = self.env();
        let settings = SettingsSnapshot::capture(
            self.0.path(),
            None,
            SettingsEnvironment::from_pairs(env.ordered_pairs()),
        )
        .expect("configuração do servidor");
        RuntimeConfig::from_snapshot(env, Arc::new(settings)).expect("runtime-config")
    }
    fn start(&self) {
        self.config().startup_jev_migration();
    }
    /// Etapas registradas no diário com o código da migração.
    fn failures(&self) -> Vec<String> {
        let Some(dir) = hangar_server::diag::diary_dir_in(&self.env()) else {
            return Vec::new();
        };
        let Ok(files) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(file.expect("arquivo do diário").path()).expect("diário");
            for line in text.lines() {
                let line: Value = serde_json::from_str(line).expect("linha do diário");
                if line["codigo"] == jev_startup::FAILURE {
                    steps.push(line["etapa"].as_str().unwrap_or("").to_owned());
                }
            }
        }
        steps
    }
    fn mode(&self, rel: &str) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(self.at(rel)).expect(rel).permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            let _ = rel;
            0o600
        }
    }
}

/// O que o repasse acrescenta ao `env` com a chave da Typesafe.
fn with_jev(mut env: Value, key: &str) -> Value {
    env["TYPESAFE_API_KEY"] = json!(key);
    env["JEV_ENDPOINT"] = json!("https://api.typesafe.ai/v1/systemone");
    env["JEV_MODEL"] = json!("jev-latest");
    env
}

fn key_env(key: &str) -> Value {
    json!({"TYPESAFE_API_KEY": key})
}

// JEV-20 (r5-aux-legado-diferentes): o auxiliar antigo vira `jev_aux_*`; o modelo do MCP trocado fica guardado.
#[test]
fn jev_20_old_aux_becomes_jev_aux_and_the_replaced_model_is_kept() {
    let home = Home::new();
    let old = json!({"LLM_PROXY_URL": "https://b/v1/chat/completions", "LLM_MODEL": "mb"});
    home.account(old.clone());
    home.main(old.clone());
    home.put(RUNTIME, &json!({"jev_texto_base_url": "https://a/v1", "jev_texto_api_key": "ka", "jev_texto_modelo": "ma"}));
    home.start();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_texto_base_url": "https://a/v1", "jev_texto_api_key": "ka", "jev_texto_modelo": "ma",
            "jev_aux_base_url": "https://a/v1", "jev_aux_api_key": "ka", "jev_aux_modelo": "ma"})
    );
    let new = json!({"LLM_PROXY_URL": "https://a/v1/chat/completions", "LLM_MODEL": "ma", "LLM_PROXY_KEY": "ka"});
    assert_eq!(home.json(MAIN), home.main_doc(new.clone()));
    assert_eq!(home.json(ACCOUNT), home.account_doc(new));
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), old.clone());
    kept.insert(home.account_source(), old);
    assert_eq!(home.json(OLD_LLM), Value::Object(kept));
    assert_eq!(home.mode(OLD_LLM), 0o600);
    assert!(home.exists(MARKER));
}

// JEV-20 (r5-aux-ja-gravado): `jev_aux_*` já gravado não é tocado; o repasse leva o auxiliar ao MCP.
#[test]
fn jev_20_written_aux_is_left_alone() {
    let home = Home::new();
    let old = json!({"LLM_PROXY_URL": "http://127.0.0.1:8317/v1/chat/completions", "LLM_MODEL": "gpt-5",
        "LLM_EFFORT": "high", "LLM_PROXY_KEY": "proxy-key"});
    home.main(old.clone());
    home.text(RUNTIME, "{\n  \"jev_aux_base_url\": \"https://n/v1\",\n  \"jev_aux_modelo\": \"novo\"\n}");
    let runtime = home.bytes(RUNTIME);
    home.start();
    assert_eq!(home.bytes(RUNTIME), runtime);
    assert_eq!(
        home.json(MAIN),
        home.main_doc(json!({"LLM_PROXY_URL": "https://n/v1/chat/completions", "LLM_MODEL": "novo"}))
    );
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), old);
    assert_eq!(home.json(OLD_LLM), Value::Object(kept));
    assert_eq!(home.mode(OLD_LLM), 0o600);
    assert!(home.exists(MARKER));
}

// JEV-20 + JEV-17 (decisão do usuário, corrige o Python): auxiliar antigo, página vazia e chave só no MCP.
// O repasse do auxiliar não apaga a chave: ela vira a do Jev, e a da conta trocada fica guardada (D5.2).
#[test]
fn jev_20_publishing_the_old_aux_keeps_the_mcp_only_key() {
    let home = Home::new();
    let llm = json!({"LLM_PROXY_URL": "https://b/v1/chat/completions", "LLM_MODEL": "mb"});
    let mut main = llm.clone();
    main["TYPESAFE_API_KEY"] = json!("chave-mcp");
    let mut account = llm.clone();
    account["TYPESAFE_API_KEY"] = json!("B-conta");
    home.main(main);
    home.account(account);
    home.start();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_aux_base_url": "https://b/v1", "jev_aux_modelo": "mb", "jev_api_key": "chave-mcp"})
    );
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(llm.clone(), "chave-mcp")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(llm, "chave-mcp")));
    let mut kept = Map::new();
    kept.insert(home.account_source(), json!(["B-conta"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert!(home.exists(MARKER));
    assert!(home.failures().is_empty());
}

// HIS-41 (r5-aux-legado-mcp-completo): navegador só com endereço, MCP completo: vale o do Computer Use inteiro.
#[test]
fn his_41_browser_with_only_an_address_keeps_the_whole_computer_use_model() {
    let home = Home::new();
    home.main(json!({"HCC_AGENT_CONFIG": "a", "LLM_PROXY_URL": "http://127.0.0.1:8317/v1/chat/completions",
        "LLM_MODEL": "gpt-5", "LLM_EFFORT": "high", "LLM_PROXY_KEY": "proxy-key"}));
    home.put(RUNTIME, &json!({"jev_texto_base_url": "http://127.0.0.1:8317/v1"}));
    let main = home.bytes(MAIN);
    home.start();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_texto_base_url": "http://127.0.0.1:8317/v1", "jev_aux_base_url": "http://127.0.0.1:8317/v1",
            "jev_aux_api_key": "proxy-key", "jev_aux_modelo": "gpt-5", "jev_aux_esforco": "high"})
    );
    assert_eq!(home.bytes(MAIN), main);
    assert!(!home.exists(OLD_LLM));
    assert!(home.exists(MARKER));
}

// HIS-52 (r5-aux-legado-mesma-escolha): o esforço que só o MCP tinha chega ao runtime-config antes de o
// repasse reescrever o `LLM_*`; o MCP recebe a chave do navegador sem guardar modelo antigo.
#[test]
fn his_52_old_aux_is_read_before_the_sync_rewrites_llm() {
    let home = Home::new();
    let mcp = json!({"LLM_PROXY_URL": "https://api.anthropic.com/v1/chat/completions", "LLM_MODEL": "haiku", "LLM_EFFORT": "low"});
    home.main(mcp);
    home.put(RUNTIME, &json!({"jev_texto_base_url": "https://api.anthropic.com/v1", "jev_texto_api_key": "k", "jev_texto_modelo": "haiku"}));
    home.start();
    assert_eq!(
        home.json(RUNTIME),
        json!({"jev_texto_base_url": "https://api.anthropic.com/v1", "jev_texto_api_key": "k", "jev_texto_modelo": "haiku",
            "jev_aux_base_url": "https://api.anthropic.com/v1", "jev_aux_api_key": "k", "jev_aux_modelo": "haiku",
            "jev_aux_esforco": "low"})
    );
    assert_eq!(
        home.json(MAIN),
        home.main_doc(json!({"LLM_PROXY_URL": "https://api.anthropic.com/v1/chat/completions", "LLM_MODEL": "haiku",
            "LLM_EFFORT": "low", "LLM_PROXY_KEY": "k"}))
    );
    assert!(!home.exists(OLD_LLM));
}

// JEV-17 (r5-chave-no-mcp-ativo): chave só no MCP ativo vira a do Jev e o MCP recebe o Jev.
#[test]
fn jev_17_key_only_in_the_active_mcp_becomes_the_jev_key() {
    let home = Home::new();
    let env = json!({"TYPESAFE_API_KEY": "chave-antiga-mcp", "HCC_AGENT_CONFIG": "a"});
    home.account(env.clone());
    home.main(env);
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "chave-antiga-mcp"}));
    let expected = with_jev(json!({"HCC_AGENT_CONFIG": "a"}), "chave-antiga-mcp");
    assert_eq!(home.json(MAIN), home.main_doc(expected.clone()));
    assert_eq!(home.json(ACCOUNT), home.account_doc(expected));
    assert!(!home.exists(OLD_KEYS));
    assert!(home.exists(MARKER));
}

// HIS-13 (r5-pagina-vence-sem-marca): sem marca e com chave na página, a página vence e a do MCP fica
// guardada por arquivo de origem (D5.2).
#[test]
fn his_13_page_key_wins_without_the_marker() {
    let home = Home::new();
    home.account(key_env("chave-antiga-mcp"));
    home.main(key_env("chave-antiga-mcp"));
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "chave-pagina"}));
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "chave-pagina")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "chave-pagina")));
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), json!(["chave-antiga-mcp"]));
    kept.insert(home.account_source(), json!(["chave-antiga-mcp"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert_eq!(home.mode(OLD_KEYS), 0o600);
    assert!(home.exists(MARKER));
}

// JEV-17 (r5-chave-na-guardada): chave só na entrada guardada; a guardada recebe o Jev e segue 0600.
#[test]
fn jev_17_key_only_in_the_parked_entry() {
    let home = Home::new();
    home.put(PARKED, &home.entry(key_env("chave-guardada")));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "chave-guardada"}));
    assert_eq!(home.json(PARKED), home.entry(with_jev(json!({}), "chave-guardada")));
    assert_eq!(home.mode(PARKED), 0o600);
    assert!(!home.exists(MAIN));
    assert!(home.exists(MARKER));
}

// HIS-14 (r5-chave-no-settings): chave do `env` do settings.json vira a do Jev; sem MCP nada é criado.
#[test]
fn his_14_settings_key_becomes_the_jev_key_without_creating_an_entry() {
    let home = Home::new();
    home.put(SETTINGS, &json!({"env": {"TYPESAFE_API_KEY": "chave-settings"}}));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "chave-settings"}));
    assert!(!home.exists(PARKED));
    assert!(!home.exists(MAIN));
    assert!(home.exists(MARKER));
}

// JEV-17 (r5-settings-env-nao-objeto): `env` lista não tem chave; só a marca.
#[test]
fn jev_17_settings_env_that_is_not_an_object_only_marks() {
    let home = Home::new();
    home.put(SETTINGS, &json!({"env": ["x"]}));
    home.start();
    assert!(!home.exists(RUNTIME));
    assert!(home.exists(MARKER));
    assert!(home.failures().is_empty());
}

// JEV-17 (r5-pagina-so-espacos): `jev_api_key` só com espaços conta como vazia; a chave entra aparada.
#[test]
fn jev_17_blank_page_key_counts_as_empty() {
    let home = Home::new();
    home.account(key_env("  chave-mcp  "));
    home.main(key_env("  chave-mcp  "));
    home.put(RUNTIME, &json!({"jev_api_key": "   "}));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "chave-mcp"}));
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "chave-mcp")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "chave-mcp")));
    assert!(!home.exists(OLD_KEYS));
    assert!(home.exists(MARKER));
}

// HIS-55 (r5-vazio): nada configurado: a marca é criada, vazia, mesmo sem nada a migrar.
#[test]
fn his_55_nothing_configured_still_marks() {
    let home = Home::new();
    home.start();
    assert_eq!(home.bytes(MARKER), b"");
    assert!(!home.exists(RUNTIME));
    assert!(!home.exists(MAIN));
    assert!(!home.exists(OLD_KEYS));
    assert!(home.failures().is_empty());
}

// HIS-55 (r5-marca-presente): com a marca e a página vazia, a chave do MCP não volta; o repasse a tira,
// guardada antes (JEV-18).
#[test]
fn his_55_marker_keeps_the_mcp_key_from_coming_back() {
    let home = Home::new();
    let env = json!({"TYPESAFE_API_KEY": "chave-antiga-mcp", "FOO": "bar"});
    home.account(env.clone());
    home.main(env);
    home.mark();
    home.start();
    assert!(!home.exists(RUNTIME));
    assert_eq!(home.json(MAIN), home.main_doc(json!({"FOO": "bar"})));
    assert_eq!(home.json(ACCOUNT), home.account_doc(json!({"FOO": "bar"})));
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), json!(["chave-antiga-mcp"]));
    kept.insert(home.account_source(), json!(["chave-antiga-mcp"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
}

// JEV-18: página vazia e chave só na conta (o principal sem ela): o repasse a tira, guardada antes.
#[test]
fn jev_18_account_key_removed_by_an_empty_page_is_kept() {
    let home = Home::new();
    home.account(key_env("so-na-conta"));
    home.main(json!({"FOO": "bar"}));
    home.start();
    assert_eq!(home.json(ACCOUNT), home.account_doc(json!({})));
    let mut kept = Map::new();
    kept.insert(home.account_source(), json!(["so-na-conta"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
}

// HIS-18 (test_key_removed_on_the_jev_page_does_not_come_back_on_restart): chave apagada na página não
// volta na subida seguinte, nem do MCP nem do settings.json.
#[test]
fn his_18_key_removed_on_the_jev_page_does_not_come_back_on_restart() {
    let home = Home::new();
    home.put(SETTINGS, &json!({"env": {"TYPESAFE_API_KEY": "legacy-typesafe-key"}}));
    home.main(json!({"HCC_AGENT_CONFIG": "a"}));
    home.start();
    let config = home.config();
    assert_eq!(config.get("jev_api_key"), json!("legacy-typesafe-key"));
    config
        .apply(&Map::new(), &["jev_api_key".to_owned()].into())
        .expect("chave apagada na página");
    home.start();
    let key = home.config().get("jev_api_key");
    assert!(key.is_null() || key == json!(""), "{key}");
    assert!(home.json(MAIN)["mcpServers"]["hangar-computer-control"]["env"].get("TYPESAFE_API_KEY").is_none());
}

// HIS-15 (r5-chave-trocada): chave do MCP diferente da página fica guardada (0600); o MCP fica com a da página.
#[test]
fn his_15_replaced_key_is_kept_for_the_owner() {
    let home = Home::new();
    home.account(key_env("chave-antiga-mcp"));
    home.main(key_env("chave-antiga-mcp"));
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.mark();
    home.start();
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), json!(["chave-antiga-mcp"]));
    kept.insert(home.account_source(), json!(["chave-antiga-mcp"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert_eq!(home.mode(OLD_KEYS), 0o600);
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "chave-pagina")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "chave-pagina")));
}

// JEV-18 (r5-chaves-diferentes-por-conta, D5.2): principal A, conta B, página C: A e B guardadas, cada uma
// no arquivo de origem.
#[test]
fn jev_18_keys_of_every_account_are_kept() {
    let home = Home::new();
    home.account(key_env("B-conta"));
    home.main(key_env("A-principal"));
    home.put(RUNTIME, &json!({"jev_api_key": "C-pagina"}));
    home.mark();
    home.start();
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), json!(["A-principal"]));
    kept.insert(home.account_source(), json!(["B-conta"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "C-pagina")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "C-pagina")));
}

// JEV-18 (r5-chave-trocada-arquivo-existente, D5.2): o formato antigo de uma chave só é mantido, nada é
// sobrescrito.
#[test]
fn jev_18_old_single_key_format_is_kept() {
    let home = Home::new();
    home.account(key_env("segunda"));
    home.main(key_env("segunda"));
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.text(OLD_KEYS, "{\n  \"TYPESAFE_API_KEY\": \"primeira\"\n}");
    home.mark();
    home.start();
    let mut kept = Map::new();
    kept.insert("TYPESAFE_API_KEY".into(), json!("primeira"));
    kept.insert(home.source(MAIN), json!(["segunda"]));
    kept.insert(home.account_source(), json!(["segunda"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert_eq!(home.mode(OLD_KEYS), 0o600);
}

// JEV-18 (r5-chave-igual): chave do MCP igual à da página: nada guardado.
#[test]
fn jev_18_same_key_keeps_nothing() {
    let home = Home::new();
    home.account(key_env("mesma"));
    home.main(key_env("mesma"));
    home.put(RUNTIME, &json!({"jev_api_key": "mesma"}));
    home.mark();
    home.start();
    assert!(!home.exists(OLD_KEYS));
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "mesma")));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "mesma")));
}

// JEV-18 (D5.2, "de qualquer conta"): sem marca, a chave do principal vira a do Jev e a da conta, que o
// repasse troca, fica guardada. O Python a perdia; não há captura.
#[test]
fn jev_18_account_key_replaced_by_the_migrated_key_is_kept() {
    let home = Home::new();
    home.account(key_env("B-conta"));
    home.main(key_env("A-principal"));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "A-principal"}));
    let mut kept = Map::new();
    kept.insert(home.account_source(), json!(["B-conta"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    assert_eq!(home.json(ACCOUNT), home.account_doc(with_jev(json!({}), "A-principal")));
}

// HIS-53: a antiga é guardada ANTES de o MCP ser sobrescrito; sem conseguir guardar, o MCP fica como estava.
#[test]
fn his_53_mcp_is_not_overwritten_when_the_old_key_cannot_be_kept() {
    let home = Home::new();
    home.account(key_env("chave-antiga-mcp"));
    home.main(key_env("chave-antiga-mcp"));
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.text(OLD_KEYS, "{x");
    home.mark();
    let (main, account) = (home.bytes(MAIN), home.bytes(ACCOUNT));
    home.start();
    assert_eq!(home.bytes(MAIN), main);
    assert_eq!(home.bytes(ACCOUNT), account);
    assert_eq!(home.bytes(OLD_KEYS), b"{x");
    assert_eq!(home.failures(), ["chave"]);
}

// HIS-73 (r5-guardada-e-ativa): na subida a ativa e a guardada recebem o Jev; a guardada passa a 0600.
#[test]
fn his_73_active_and_parked_entries_receive_the_jev_on_startup() {
    let home = Home::new();
    home.main(key_env("chave-pagina"));
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.put(PARKED, &home.entry(json!({"FOO": "guardada"})));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.at(PARKED), std::fs::Permissions::from_mode(0o644)).expect("modo");
    }
    home.mark();
    home.start();
    assert_eq!(home.json(MAIN), home.main_doc(with_jev(json!({}), "chave-pagina")));
    assert_eq!(home.json(PARKED), home.entry(with_jev(json!({"FOO": "guardada"}), "chave-pagina")));
    assert_eq!(home.mode(PARKED), 0o600);
}

// JEV-19 (r5-principal-compacto-nao-ascii): o principal editado por fora recebe o Jev na subida e o resto
// do arquivo continua o mesmo JSON.
#[test]
fn jev_19_main_file_edited_outside_receives_the_jev() {
    let home = Home::new();
    home.text(MAIN, "{\"zeta\":\"ação 😀\",\"mcpServers\":{\"hangar-computer-control\":{\"command\":\"x\",\"env\":{\"Z\":\"1\",\"A\":\"é\"}}},\"alpha\":1}");
    home.put(RUNTIME, &json!({"jev_api_key": "chave-pagina"}));
    home.mark();
    home.start();
    assert_eq!(
        home.json(MAIN),
        json!({"zeta": "ação 😀", "mcpServers": {"hangar-computer-control": {"command": "x",
            "env": with_jev(json!({"Z": "1", "A": "é"}), "chave-pagina")}}, "alpha": 1})
    );
}

// JEV-25 (r5-main-invalido): principal ilegível: as duas etapas falham, vão ao diário, a subida segue e
// não há marca.
#[test]
fn jev_25_unreadable_main_file_fails_both_steps() {
    let home = Home::new();
    home.text(MAIN, "{x");
    home.start();
    assert_eq!(home.bytes(MAIN), b"{x");
    assert!(!home.exists(MARKER));
    assert!(!home.exists(RUNTIME));
    assert_eq!(home.failures(), ["auxiliar", "chave"]);
}

// JEV-25 (r5-settings-invalido): settings.json ilegível sem chave em outro lugar: sem marca.
#[test]
fn jev_25_unreadable_settings_leaves_no_marker() {
    let home = Home::new();
    home.text(SETTINGS, "{x");
    home.start();
    assert!(!home.exists(MARKER));
    assert!(!home.exists(RUNTIME));
    assert_eq!(home.failures(), ["chave"]);
}

// JEV-25 (r5-conta-mcpservers-nao-objeto): a chave vira a do Jev, o repasse recusa a conta inválida e
// nenhum `.claude.json` muda; sem marca.
#[test]
fn jev_25_account_with_invalid_mcp_servers_keeps_the_saved_key() {
    let home = Home::new();
    home.account_text("{\"mcpServers\": \"x\"}");
    home.main(key_env("k-mcp"));
    let (main, account) = (home.bytes(MAIN), home.bytes(ACCOUNT));
    home.start();
    assert_eq!(home.json(RUNTIME), json!({"jev_api_key": "k-mcp"}));
    assert_eq!(home.bytes(MAIN), main);
    assert_eq!(home.bytes(ACCOUNT), account);
    assert!(!home.exists(MARKER));
    assert_eq!(home.failures(), ["chave"]);
}

// JEV-25 (r5-env-nao-objeto): `env` da entrada ativa é lista: nada muda, sem marca, falha no diário.
#[test]
fn jev_25_env_that_is_not_an_object() {
    let home = Home::new();
    let main = json!({"mcpServers": {"hangar-computer-control": {
        "command": home.at(".hangar/computer-control/hangar-computer-control").to_string_lossy(), "env": ["x"]}}});
    home.put(MAIN, &main);
    let before = home.bytes(MAIN);
    home.start();
    assert_eq!(home.bytes(MAIN), before);
    assert!(!home.exists(MARKER));
    assert!(!home.exists(RUNTIME));
    assert!(home.failures().contains(&"chave".to_owned()));
}

// JEV-18: `.claude.json` com o surrogate solto que o Claude Code grava e chave diferente da do Jev, sem
// marca: a chave antiga vai para a lista de trocadas antes de o repasse sobrescrevê-la, e a migração
// conclui (o leitor estrito pulava a origem e a chave se perdia calada).
#[test]
fn jev_18_lone_surrogate_file_keeps_the_replaced_key() {
    let home = Home::new();
    home.text(MAIN, r#"{"a": "x\ud800", "mcpServers": {"hangar-computer-control": {"command": "c", "env": {"TYPESAFE_API_KEY": "velha"}}}}"#);
    home.put(RUNTIME, &json!({"jev_api_key": "pagina"}));
    home.start();
    assert!(home.failures().is_empty(), "{:?}", home.failures());
    assert!(home.exists(MARKER));
    let mut kept = Map::new();
    kept.insert(home.source(MAIN), json!(["velha"]));
    assert_eq!(home.json(OLD_KEYS), Value::Object(kept));
    let text = String::from_utf8(home.bytes(MAIN)).expect("utf-8");
    assert!(text.contains(r#""a": "x\ud800""#), "{text}");
    let data = hangar_core::pycompat::pyjson::loads_lossless(&text).expect("json");
    assert_eq!(data["mcpServers"]["hangar-computer-control"]["env"]["TYPESAFE_API_KEY"], "pagina");
}

fn json_text() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;
    prop_oneof![
        "[{}\\[\\]\":,a-zA-Z0-9 _]{0,40}",
        "\\PC{0,24}".prop_map(|text| serde_json::to_string(&text).expect("texto")),
        Just("null".to_owned()),
        Just("[\"x\"]".to_owned()),
        Just("{\"TYPESAFE_API_KEY\": 1}".to_owned()),
        Just("{\"TYPESAFE_API_KEY\": \"  \"}".to_owned()),
    ]
}

proptest::proptest! {
    #[test]
    fn readers_of_settings_env_and_old_keys_never_panic(
        raw in proptest::collection::vec(proptest::num::u8::ANY, 0..128),
        fragment in json_text(),
    ) {
        let home = tempfile::tempdir().expect("pasta temporária");
        let settings = home.path().join("settings.json");
        let kept = home.path().join("kept.json");
        let source = "/origem/.claude.json";
        let documents = [
            raw.clone(),
            fragment.clone().into_bytes(),
            format!("{{\"env\": {fragment}}}").into_bytes(),
            format!("{{\"{source}\": {fragment}, \"TYPESAFE_API_KEY\": {fragment}}}").into_bytes(),
        ];
        for document in documents {
            std::fs::write(&settings, &document).expect("settings");
            let _ = jev_startup::settings_key(&settings);
            std::fs::write(&kept, &document).expect("chaves antigas");
            if jev_startup::keep_old_keys(&kept, &[(source.to_owned(), json!("chave"))]).is_ok() {
                let data: Value = serde_json::from_slice(&std::fs::read(&kept).expect("lido")).expect("objeto");
                let slot = &data[source];
                proptest::prop_assert!(
                    slot == &json!("chave") || slot.as_array().is_some_and(|list| list.contains(&json!("chave"))),
                    "{data}"
                );
            }
        }
    }
}
