//! A credencial de uma sessão Pi/omp é a do modelo em uso, mesmo com outro nome no Kimi.
use crate::accounts::environment::AccountEnvironment;
use crate::accounts::quotas::other;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

type Signature = Vec<Option<SystemTime>>;
type Mapping = HashMap<String, String>;
type PiCache = HashMap<Vec<PathBuf>, (Signature, Mapping)>;

fn paths(env: &AccountEnvironment) -> Vec<PathBuf> {
    let kimi = env.base.get("KIMI_CODE_HOME").filter(|p| !p.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".kimi-code"));
    let engines = env.base.get("CP_ENGINES_FILE").filter(|p| !p.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude/engines.json"));
    vec![env.home.join(".pi/agent/auth.json"), env.home.join(".pi/agent/models.json"), kimi.join("config.toml"), engines]
}

fn signature(paths: &[PathBuf]) -> Signature {
    paths.iter().map(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok()).collect()
}

fn json(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|raw| crate::transcript::pyjson::loads_lossless(&String::from_utf8(raw).ok()?)).unwrap_or(Value::Null)
}

pub fn pi_account(env: &AccountEnvironment, provider: Option<&str>) -> Option<String> {
    let provider = provider.filter(|provider| !provider.is_empty())?;
    static CACHE: LazyLock<Mutex<PiCache>> = LazyLock::new(Mutex::default);
    let paths = paths(env);
    let signature = signature(&paths);
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((previous, mapping)) = cache.get(&paths) && previous == &signature {
        return mapping.get(provider).cloned();
    }
    let config = other::load(env);
    let mut by_key = HashMap::<String, String>::new();
    for source in config.sources.iter().filter(|source| source.id.starts_with("kimi:")) {
        let key = match &source.reader {
            other::Reader::Kimi { key, .. } | other::Reader::CommandCode { key } => key,
            other::Reader::OpenCode { .. } => continue,
        };
        by_key.entry(key.clone()).or_insert_with(|| source.id.clone());
    }
    for (name, data) in &config.engines {
        if other::engine_name_ok(name) && let Some(key) = data.get("api_key").and_then(Value::as_str).filter(|key| !key.is_empty()) {
            by_key.entry(key.into()).or_insert_with(|| format!("chave:{name}"));
        }
    }
    let mut keys = HashMap::<String, String>::new();
    if let Some(auth) = json(&paths[0]).as_object() {
        for (name, data) in auth {
            if let Some(key) = data.get("key").and_then(Value::as_str).filter(|key| !key.is_empty()) {
                keys.insert(name.clone(), key.into());
            }
        }
    }
    if let Some(providers) = json(&paths[1]).get("providers").and_then(Value::as_object) {
        for (name, data) in providers {
            if let Some(key) = data.get("apiKey").and_then(Value::as_str).filter(|key| !key.is_empty()) {
                keys.entry(name.clone()).or_insert_with(|| key.into());
            }
        }
    }
    let mapping: Mapping = keys.into_iter().filter_map(|(name, key)| by_key.get(&key).cloned().map(|account| (name, account))).collect();
    let result = mapping.get(provider).cloned();
    cache.insert(paths, (signature, mapping));
    result
}

pub fn kimi_account(env: &AccountEnvironment) -> Option<String> {
    type KimiCache = HashMap<PathBuf, (SystemTime, Option<String>)>;
    static CACHE: LazyLock<Mutex<KimiCache>> = LazyLock::new(Mutex::default);
    let path = paths(env).remove(2);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((previous, account)) = cache.get(&path) && *previous == modified {
        return account.clone();
    }
    let config = other::load(env);
    let account = config.kimi_text.as_deref().and_then(|text| {
        if text.starts_with('\u{feff}') { return None; }
        let data: toml::Table = text.parse().ok()?;
        let model = data.get("default_model")?.as_str()?;
        let (provider, _) = model.split_once('/')?;
        let account = format!("kimi:{provider}");
        config.sources.iter().any(|source| source.id == account).then_some(account)
    });
    cache.insert(path, (modified, account.clone()));
    account
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn environment(home: &std::path::Path) -> AccountEnvironment {
        AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ]))
    }

    fn at(path: &Path, time: SystemTime) {
        std::fs::OpenOptions::new().write(true).open(path).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
    }

    #[test]
    fn kimi_missing_and_invalid_config_recover_only_when_the_mtime_changes() {
        let home = tempfile::tempdir().unwrap();
        let env = environment(home.path());
        let path = home.path().join(".kimi-code/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(kimi_account(&env), None);
        std::fs::write(&path, "isto não = = toml\n").unwrap();
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(kimi_account(&env), None);
        let config = "default_model = \"apikey/k3\"\n[providers.apikey]\ntype = \"kimi\"\napi_key = \"sk-x\"\nbase_url = \"https://x\"\n";
        std::fs::write(&path, config).unwrap();
        at(&path, stamp);
        assert_eq!(kimi_account(&env), None, "a falha também fica no cache pelo mtime");
        at(&path, stamp + std::time::Duration::from_secs(5));
        assert_eq!(kimi_account(&env).as_deref(), Some("kimi:apikey"));
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "default_model = \"outro/k9\"\n").unwrap();
        at(&path, stamp);
        assert_eq!(kimi_account(&env).as_deref(), Some("kimi:apikey"), "conteúdo novo com mtime igual conserva a conta");
        at(&path, stamp + std::time::Duration::from_secs(5));
        assert_eq!(kimi_account(&env), None);
    }

    #[test]
    fn pi_credential_cache_uses_all_source_mtimes_and_never_invents_an_account() {
        let home = tempfile::tempdir().unwrap();
        let env = environment(home.path());
        let auth = home.path().join(".pi/agent/auth.json");
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::create_dir_all(home.path().join(".kimi-code")).unwrap();
        std::fs::write(home.path().join(".kimi-code/config.toml"), "[providers.apikey]\ntype = \"kimi\"\napi_key = \"sk-kimi-1\"\nbase_url = \"https://x\"\n").unwrap();
        std::fs::write(&auth, r#"{"kimi-coding":{"key":"sk-kimi-1"},"codex":{"type":"oauth","access":"j.w.t"}}"#).unwrap();
        let stamp = std::fs::metadata(&auth).unwrap().modified().unwrap();
        assert_eq!(pi_account(&env, Some("kimi-coding")).as_deref(), Some("kimi:apikey"));
        assert_eq!(pi_account(&env, Some("codex")), None);
        assert_eq!(pi_account(&env, Some("outro")), None);
        assert_eq!(pi_account(&env, None), None);
        std::fs::write(&auth, r#"{"kimi-coding":{"key":"sk-outra"}}"#).unwrap();
        at(&auth, stamp);
        assert_eq!(pi_account(&env, Some("kimi-coding")).as_deref(), Some("kimi:apikey"));
        at(&auth, stamp + std::time::Duration::from_secs(5));
        assert_eq!(pi_account(&env, Some("kimi-coding")), None);
    }

    #[test]
    fn provider_accounts_match_the_python_characterization_including_credential_priority() {
        let fixture: Value = serde_json::from_str(include_str!("../../../backend/tests/fixtures/contract/provider-accounts/accounts.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let home = tempfile::tempdir().unwrap();
            for dir in [".pi/agent", ".kimi-code", ".claude"] {
                std::fs::create_dir_all(home.path().join(dir)).unwrap();
            }
            for (path, value) in [(".pi/agent/auth.json", &case["auth"]), (".pi/agent/models.json", &case["models"]), (".claude/engines.json", &case["engines"])] {
                std::fs::write(home.path().join(path), value.to_string()).unwrap();
            }
            std::fs::write(home.path().join(".kimi-code/config.toml"), case["kimi"].as_str().unwrap()).unwrap();
            let env = environment(home.path());
            assert_eq!(serde_json::to_value(pi_account(&env, case["provider"].as_str())).unwrap(), case["expected"]["pi"], "{}", case["name"]);
            // O leitor Python devolvia o provider; a lista acrescentava o prefixo da conta.
            let provider = kimi_account(&env).map(|account| account.strip_prefix("kimi:").unwrap().to_owned());
            assert_eq!(serde_json::to_value(provider).unwrap(), case["expected"]["kimi"], "{}", case["name"]);
        }
    }
}
