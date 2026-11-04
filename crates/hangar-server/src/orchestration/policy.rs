//! Política de contas em Markdown, preservada como documento do usuário.
use super::{array, markdown as md, runs, text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
const HEADERS: [&str; 5] = ["conta", "provider", "apelido", "modelos", "trocar?"];
fn headers() -> Vec<String> {
    HEADERS.iter().map(|s| (*s).into()).collect()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub conta: String,
    pub provider: String,
    pub apelido: String,
    pub modelos: Vec<String>,
    pub trocar: bool,
}
pub fn path(home: &Path) -> PathBuf {
    home.join(".hangar").join("orquestracao-contas.md")
}

#[cfg(test)]
mod windows_readers_tests {
    #[test]
    fn windows_readers_policy_path_displays_native_path_components() {
        let separator = std::path::MAIN_SEPARATOR;
        assert_eq!(super::path(std::path::Path::new("home")).to_string_lossy(),
            format!("home{separator}.hangar{separator}orquestracao-contas.md"));
    }
}
pub fn read(content: &str) -> Vec<Policy> {
    md::table(content, &headers())
        .into_iter()
        .filter_map(|r| {
            let provider = text(&r["provider"]).to_lowercase();
            let account = text(&r["conta"]);
            if account.is_empty()
                || !matches!(provider.as_str(), "claude" | "kimi" | "pi" | "codex")
            {
                return None;
            }
            let mut models: Vec<_> = text(&r["modelos"])
                .split(',')
                .map(|s| s.trim().trim_matches('`').to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if models.is_empty() {
                models.push("*".into())
            }
            Some(Policy {
                conta: account.into(),
                provider,
                apelido: text(&r["apelido"]).into(),
                modelos: models,
                trocar: !matches!(
                    md::normalize(text(&r["trocar?"])).as_str(),
                    "nao" | "não" | "no" | "n"
                ),
            })
        })
        .collect()
}
pub fn claude_dir(home: &Path, base: &Path, account: &str) -> PathBuf {
    if matches!(account, "padrao" | "" | "~/.claude") {
        return base.into();
    }
    let direct = home.join(format!(".claude-{account}"));
    if !direct.is_dir() {
        if let Some(short) = account.strip_prefix("claude-") {
            let short = home.join(format!(".claude-{short}"));
            if short.is_dir() {
                return short;
            }
        }
    }
    direct
}
pub fn quota_id(home: &Path, base: &Path, provider: &str, account: &str) -> Option<String> {
    match provider {
        "claude" => {
            let p = claude_dir(home, base, account);
            Some(format!(
                "claude:{}",
                fs::canonicalize(&p).unwrap_or(p).display()
            ))
        }
        "kimi" => Some(format!("kimi:{account}")),
        _ => None,
    }
}
pub fn claude_models(dir: &Path, at: f64) -> (Vec<Value>, bool) {
    let cached = runs::json(&dir.join(".hangar-models.json")).ok();
    if let Some(c) = cached {
        let ts = c["ts"]
            .as_f64()
            .or_else(|| c["ts"].as_str().and_then(|s| s.parse().ok()));
        let resp = &c["resp"];
        let truncated = array(&resp["models"]).iter().any(|m| {
            let name = super::py_string(m.get("name").unwrap_or(&json!("")));
            let name = name.trim_end();
            name.ends_with('…') || name.ends_with("...")
        });
        if resp.is_object() && ts.is_some_and(|t| at - t < 30. * 24. * 3600.) && !truncated {
            let mut ids = HashSet::new();
            return (
                array(&resp["models"])
                    .iter()
                    .filter(|m| ids.insert(m["id"].to_string()))
                    .cloned()
                    .collect(),
                false,
            );
        }
    }
    (
        vec![
            json!({"id":"opus"}),
            json!({"id":"sonnet"}),
            json!({"id":"haiku"}),
        ],
        true,
    )
}
pub fn permitted(
    provider: &str,
    account: &str,
    model: &str,
    effort: &str,
    policies: &[Policy],
    kimi: Option<&Value>,
) -> Option<&'static str> {
    let provider = if provider == "omp" { "pi" } else { provider };
    if !policies.is_empty() {
        let Some(rule) = policies
            .iter()
            .find(|p| p.provider == provider && md::normalize(&p.conta) == md::normalize(account))
        else {
            return Some("erro_orq_conta_nao_liberada");
        };
        if !model.is_empty() && !rule.modelos.iter().any(|m| m == "*" || m == model) {
            return Some(if rule.trocar {
                "erro_orq_modelo_nao_liberado"
            } else {
                "erro_orq_conta_travada"
            });
        }
    }
    let levels = match provider {
        "claude" => Some(vec!["low", "medium", "high", "xhigh", "max"]),
        "pi" => Some(vec![
            "off", "minimal", "low", "medium", "high", "xhigh", "max",
        ]),
        "kimi" => kimi
            .and_then(|cat| array(&cat["models"]).iter().find(|m| m["alias"] == model))
            .map(|m| {
                array(&m["efforts"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty()),
        _ => None,
    };
    if !effort.is_empty() && levels.is_some_and(|l| !l.contains(&effort)) {
        Some("erro_orq_esforco_invalido")
    } else {
        None
    }
}
pub fn update(
    content: &str,
    policy: Option<&Policy>,
    account: &str,
    inventory: &[Value],
) -> Result<String, String> {
    let mut content = if let Some(p) = policy {
        if p.provider == "omp" {
            return Err("a política do omp é a do Pi — libere a conta no provider `pi`, que vale pros dois; a tabela não tem linha `omp`".into());
        }
        md::replace_row(
            content,
            &headers(),
            &[p.conta.clone()],
            &json!({"conta":p.conta,"provider":p.provider,"apelido":p.apelido,"modelos":p.modelos.join(", "),"trocar?":if p.trocar{"sim"}else{"não"}}),
            "O que pode",
        )?
    } else {
        md::remove_row(content, &headers(), &[account.into()])
    };
    let allowed: HashSet<_> = read(&content)
        .into_iter()
        .map(|p| (p.provider, p.conta))
        .collect();
    let denied: Vec<_> = inventory
        .iter()
        .filter(|i| {
            !allowed.contains(&(
                text(&i["provider"]).to_owned(),
                text(&i["conta"]).to_owned(),
            ))
        })
        .map(|i| {
            format!(
                "- `{}` ({}) — não liberada",
                text(&i["conta"]),
                text(&i["provider"])
            )
        })
        .collect();
    content = md::replace_section(
        &content,
        "O que NÃO pode",
        &if denied.is_empty() {
            "- (todas as contas conhecidas estão liberadas)".into()
        } else {
            denied.join("\n")
        },
    );
    Ok(content)
}
pub fn migrate(home: &Path, base: &Path) -> bool {
    let old = base.join("orquestracao-contas.md");
    let new = path(home);
    let Ok(meta) = fs::symlink_metadata(&old) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_file() || fs::symlink_metadata(&new).is_ok() {
        return false;
    }
    if fs::create_dir_all(new.parent().unwrap())
        .and_then(|_| fs::rename(&old, &new))
        .is_err()
    {
        tracing::warn!(
            code = "orq_policy_migration_failed",
            "não foi possível mover a política de contas"
        );
        return false;
    }
    true
}
#[derive(Default, Deserialize)]
pub struct InventoryFacts {
    pub base: PathBuf,
    pub aliases: Value,
    #[serde(default)]
    pub claude: Vec<Value>,
    #[serde(default)]
    pub kimi: Value,
    #[serde(default)]
    pub kimi_names: Vec<String>,
    #[serde(default)]
    pub pi: Vec<Value>,
    #[serde(default)]
    pub codex: Vec<Value>,
}
pub fn claude_name(base:&Path,dir:&Path)->String {
    let resolved=fs::canonicalize(dir).unwrap_or_else(|_|dir.to_owned());
    let base=fs::canonicalize(base).unwrap_or_else(|_|base.to_owned());
    if resolved==base {"padrao".into()} else {
        let name=resolved.file_name().unwrap_or_default().to_string_lossy();
        name.strip_prefix(".claude-").unwrap_or(&name).to_owned()
    }
}
/// Os catálogos já resolvidos mantêm a mesma ordem, inclusive quando um provedor não respondeu.
pub fn inventory_from(facts: InventoryFacts) -> Vec<Value> {
    let InventoryFacts {
        base,
        aliases,
        claude,
        kimi,
        kimi_names,
        pi,
        codex,
    } = facts;
    let mut out = Vec::new();
    for row in claude {
        let account = &row["account"];
        let dir = PathBuf::from(text(&account["path"]));
        let resolved = fs::canonicalize(&dir).unwrap_or(dir.clone());
        let name = claude_name(&base,&dir);
        let (models, reduced) = if row["models"].is_array() {
            (array(&row["models"]).to_vec(), row["reduced"] == true)
        } else {
            claude_models(&dir, runs::now())
        };
        out.push(json!({"conta":name,"provider":"claude","apelido":account["label"],"id_cota":format!("claude:{}",resolved.display()),"modelos":models,"reduced":reduced}));
    }
    for name in kimi_names {
        let models:Vec<_>=array(&kimi["models"]).iter().filter(|m|m["provider"]==name.as_str()).map(|m|json!({"id":m["alias"],"name":m["name"],"context_length":m["context_length"],"efforts":m["efforts"]})).collect();
        out.push(json!({"conta":name,"provider":"kimi","apelido":aliases.get(format!("kimi:{name}")).unwrap_or(&json!(name)),"id_cota":format!("kimi:{name}"),"modelos":models,"reduced":false}));
    }
    let mut by = IndexMap::<String, Vec<Value>>::new();
    for model in pi {
        by.entry(text(&model["provider"]).into()).or_default().push(json!({"id":model["id"],"context_length":model["context"],"thinking":model["thinking"]}));
    }
    for (provider, models) in by {
        out.push(json!({"conta":provider,"provider":"pi","apelido":provider,"id_cota":null,"modelos":models,"reduced":false}));
    }
    for row in codex {
        let account = &row["account"];
        let id = text(&account["id"]);
        let default = account["is_default"] == true;
        let key = format!("codex:{}", text(&account["home"]));
        let natural = if default {
            aliases
                .get("codex")
                .cloned()
                .unwrap_or(json!("OpenAI Codex"))
        } else {
            json!(id)
        };
        let label = aliases
            .get(&key)
            .filter(|v| super::truthy(v))
            .unwrap_or(&natural);
        out.push(json!({"conta":if default{"openai-codex"}else{id},"provider":"codex","apelido":label,"id_cota":null,"modelos":row["models"],"reduced":false}));
    }
    out
}
pub async fn inventory(st: &Arc<crate::routes::AppState>) -> Vec<Value> {
    let service = &st.accounts;
    let home = &service.env.home;
    let base = &service.env.claude_base;
    let mut facts = InventoryFacts {
        base: base.clone(),
        aliases: runs::json(&base.join(".hangar-apelidos.json")).unwrap_or_default(),
        ..Default::default()
    };
    if let Ok(accounts) = service.claude_catalog() {
        for account in array(&accounts) {
            let (models, reduced) = claude_models(Path::new(text(&account["path"])), runs::now());
            facts
                .claude
                .push(json!({"account":account,"models":models,"reduced":reduced}));
        }
    }
    facts.kimi = crate::provider_models::kimi_catalog(home).unwrap_or_default();
    let conf = fs::read_to_string(crate::accounts::quotas::other::kimi_config(&service.env))
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok());
    if let Some(providers) = conf
        .as_ref()
        .and_then(|c| c.get("providers"))
        .and_then(toml::Value::as_table)
    {
        facts.kimi_names = providers
            .iter()
            .filter(|(_, provider)| {
                provider.get("type").and_then(toml::Value::as_str) == Some("kimi")
                    && provider
                        .get("api_key")
                        .and_then(toml::Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                    && provider
                        .get("base_url")
                        .and_then(toml::Value::as_str)
                        .is_some_and(|s| !s.is_empty())
            })
            .map(|(name, _)| name.clone())
            .collect();
    }
    facts.pi = crate::provider_models::catalog("pi")
        .await
        .unwrap_or_default();
    if let Ok(accounts) = service.visible_codex_accounts() {
        let model_slots = tokio::sync::Semaphore::new(8);
        let slots = &model_slots;
        use futures_util::StreamExt;
        let catalogs =
            futures_util::stream::iter(accounts.clone().into_iter().map(|account| async move {
                st.dictation_models
                    .get(service, slots, "codex", &account.home, false)
                    .await
                    .map(|(models, _)| {
                        models
                            .into_iter()
                            .map(|m| json!({"id":m["id"],"name":m["name"],"efforts":m["efforts"]}))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }))
            .buffered(8)
            .collect::<Vec<_>>()
            .await;
        for (account, models) in accounts.into_iter().zip(catalogs) {
            facts.codex.push(json!({"account":{"id":account.id,"home":account.home,"is_default":account.is_default},"models":models}));
        }
    }
    inventory_from(facts)
}
use indexmap::IndexMap;
