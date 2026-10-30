//! Leituras do painel conservam a credencial no servidor e publicam apenas nomes.
use super::health::{self, item, read, read_toml};
use crate::{
    accounts::{AccountService, Provider, environment::AccountEnvironment},
    engines::mirrors,
    orchestration::{codec, truthy},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};
const OAUTH: &str = "openai-codex";
pub fn codex_login(env: &AccountEnvironment) -> bool {
    truthy(&read(&env.codex_default.join("auth.json"))["tokens"]["refresh_token"])
}
pub fn oauth_saved(env: &AccountEnvironment) -> bool {
    let data = read(&env.home.join(".hangar/auth/openai-codex.json"));
    ["access", "refresh", "id_token", "expires_ms", "account_id"]
        .iter()
        .all(|key| data.get(*key).is_some())
}
pub fn present(env: &AccountEnvironment, cli: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let result = (|| -> Result<(), String> {
        match cli {
            "pi" => {
                let dir = env.home.join(".pi/agent");
                let auth =
                    std::fs::read_to_string(dir.join("auth.json")).map_err(|e| e.to_string())?;
                let auth = codec::loads(&auth).ok_or("auth.json inválido")?;
                let auth = crate::config_sync::runtime_values::ValueGuard(auth);
                let map = auth.as_object().ok_or("auth.json não é objeto")?;
                names.extend(map.keys().cloned());
                let models = read(&dir.join("models.json"));
                if let Some(map) = models["providers"].as_object() {
                    names.extend(map.keys().cloned());
                }
            }
            "omp" => {
                let path = env.omp_directories()?.agent_dir.join("agent.db");
                if path.is_file() {
                    let db = rusqlite::Connection::open_with_flags(
                        path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .map_err(|e| e.to_string())?;
                    let mut statement = db
                        .prepare(
                            "select provider from auth_credentials where disabled_cause is null",
                        )
                        .map_err(|e| e.to_string())?;
                    let rows = statement
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(|e| e.to_string())?;
                    for row in rows {
                        names.insert(row.map_err(|e| e.to_string())?);
                    }
                }
            }
            "kimi" | "codex" => {
                let data = read_toml(&super::files::directory(env, cli).join("config.toml"))?;
                if let Some(map) = data
                    .get(if cli == "kimi" {
                        "providers"
                    } else {
                        "model_providers"
                    })
                    .and_then(toml::Value::as_table)
                {
                    names.extend(map.keys().map(|name| {
                        if cli == "kimi" {
                            name.strip_prefix("managed:").unwrap_or(name).to_owned()
                        } else {
                            name.clone()
                        }
                    }));
                }
                if cli == "codex" && codex_login(env) {
                    names.insert(OAUTH.into());
                }
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        if error.ends_with("não é objeto") {
            return Err(error);
        }
        tracing::debug!(%error,"credenciais do harness não puderam ser lidas");
    }
    Ok(names)
}
pub fn available(env: &AccountEnvironment, cli: &str) -> Vec<(String, Option<Value>)> {
    let mut out: Vec<(String, Option<Value>)> = Vec::new();
    let engines = crate::engines::store::state(env).0;
    for (name, data) in &engines {
        let base = data["base_url"].as_str().unwrap_or("");
        if base.is_empty() || !truthy(&data["api_key"]) {
            continue;
        }
        let target = if matches!(cli, "pi" | "omp") {
            mirrors::builtin(base).unwrap_or(name)
        } else {
            name
        };
        let value = Some(json!({"nome":name,"base_url":base,"api_key":data["api_key"]}));
        if let Some((_, old)) = out.iter_mut().find(|(name, _)| name == target) {
            *old = value;
        } else {
            out.push((target.into(), value));
        }
    }
    if cli != "kimi" && (oauth_saved(env) || codex_login(env)) {
        if let Some((_, value)) = out.iter_mut().find(|(name, _)| name == OAUTH) {
            *value = None;
        } else {
            out.push((OAUTH.into(), None));
        }
    }
    out
}
pub fn credentials(env: &AccountEnvironment, cli: &str) -> Result<Value, String> {
    let names = present(env, cli)?;
    let mut absent: Vec<_> = available(env, cli)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|n| !names.contains(n))
        .collect();
    absent.sort();
    let names = if names.is_empty() {
        "—".into()
    } else {
        names.into_iter().collect::<Vec<_>>().join(", ")
    };
    Ok(if absent.is_empty() {
        item(
            "credenciais",
            Some(true),
            "credenciais_ok",
            None,
            false,
            &[("tem", names)],
        )
    } else {
        item(
            "credenciais",
            Some(false),
            "credenciais_faltam",
            Some(&format!("sync:{cli}")),
            false,
            &[("tem", names), ("faltam", absent.join(", "))],
        )
    })
}

pub fn accounts(service: &AccountService) -> Result<Value, String> {
    let names: Vec<_> = service
        .snapshot(Provider::Claude)
        .map_err(|e| e.message)?
        .into_iter()
        .map(|a| a.id)
        .collect();
    Ok(if names.is_empty() {
        item("contas", Some(true), "so_conta_padrao", None, false, &[])
    } else {
        item(
            "contas",
            Some(true),
            "contas_ok",
            Some("contas"),
            false,
            &[("n", names.len().to_string()), ("lista", names.join(", "))],
        )
    })
}
pub fn plugins(env: &AccountEnvironment) -> Result<Value, String> {
    let path = env.home.join(".claude/settings.json");
    let Some(data) = health::read_json(&path).filter(|d| d.is_object()) else {
        return Ok(item("plugins", None, "config_ilegivel", None, false, &[]));
    };
    let mut names = Vec::new();
    if truthy(&data["enabledPlugins"]) {
        let enabled = data["enabledPlugins"]
            .as_object()
            .ok_or("enabledPlugins não é objeto")?;
        names.extend(
            enabled
                .iter()
                .filter(|(_, v)| truthy(v))
                .map(|(n, _)| n.split('@').next().unwrap_or(n).to_owned()),
        );
    }
    names.sort();
    let warnings =
        match crate::accounts::claude_config::check_plugins(&env.home, &env.home.join(".claude")) {
            Ok(v) => v,
            Err(_) => return Ok(item("plugins", None, "config_ilegivel", None, false, &[])),
        };
    Ok(if warnings.is_empty() {
        item(
            "plugins",
            Some(true),
            "plugins_ok",
            None,
            false,
            &[("n", names.len().to_string()), ("lista", names.join(", "))],
        )
    } else {
        item(
            "plugins",
            Some(false),
            "plugins_com_problema",
            None,
            false,
            &[
                ("n", warnings.len().to_string()),
                ("lista", warnings.join("; ")),
            ],
        )
    })
}
pub fn kimi_hooks(env: &AccountEnvironment, repo: &Path) -> Result<Value, String> {
    let data = read_toml(&super::files::directory(env, "kimi").join("config.toml"))?;
    let hooks = data.get("hooks").and_then(toml::Value::as_array);
    let mut missing = 0;
    for (event, matcher, script) in [
        ("SessionStart", None, "kimi_state_hook.py"),
        ("UserPromptSubmit", None, "kimi_state_hook.py"),
        ("TurnStarted", None, "kimi_state_hook.py"),
        ("Stop", None, "kimi_state_hook.py"),
        ("Interrupt", None, "kimi_state_hook.py"),
        ("PermissionRequest", None, "kimi_state_hook.py"),
        ("PreToolUse", Some("AskUserQuestion"), "kimi_state_hook.py"),
        ("PostToolUse", Some("AskUserQuestion"), "kimi_state_hook.py"),
    ] {
        let script = repo.join("backend/hooks").join(script);
        let found = hooks.into_iter().flatten().any(|entry| {
            entry.get("event").and_then(toml::Value::as_str) == Some(event)
                && entry.get("matcher").and_then(toml::Value::as_str) == matcher
                && super::hooks::refers_platform(
                    &json!(
                        entry
                            .get("command")
                            .and_then(toml::Value::as_str)
                            .unwrap_or("")
                    ),
                    &script.to_string_lossy(),
                    false,
                    false,
                )
        });
        if !found {
            missing += 1;
        }
    }
    Ok(if missing == 0 {
        item(
            "hooks",
            Some(true),
            "hooks_ok",
            None,
            false,
            &[("n", "8".into())],
        )
    } else {
        item(
            "hooks",
            Some(false),
            "faltam_n",
            Some("hooks-kimi"),
            false,
            &[("n", missing.to_string())],
        )
    })
}
pub fn skills(env: &AccountEnvironment, repo: &Path, cli: &str) -> Result<Value, String> {
    if !matches!(cli, "pi" | "kimi") {
        return Ok(item("skills", None, "sem_ponte", None, false, &[]));
    }
    let bridge = env.home.join(if cli == "pi" {
        ".pi/agent/skills-bridge"
    } else {
        ".kimi-code/skills-bridge"
    });
    if !bridge.is_dir() {
        return Ok(item(
            "skills",
            Some(false),
            "ponte_ausente",
            Some("skills"),
            false,
            &[],
        ));
    }
    let links = std::fs::read_dir(&bridge)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.is_symlink())
        .collect::<Vec<_>>();
    let dead = links.iter().filter(|p| !p.exists()).count();
    if dead > 0 {
        return Ok(item(
            "skills",
            Some(false),
            "links_pendurados",
            Some("skills"),
            false,
            &[("n", dead.to_string()), ("total", links.len().to_string())],
        ));
    }
    match super::skills::bridge_configured(env, cli) {
        Ok(Some(false)) => {
            return Ok(item(
                "skills",
                Some(false),
                "ponte_fora_da_config",
                None,
                false,
                &[("n", links.len().to_string()), ("cli", cli.into())],
            ));
        }
        Ok(None) | Err(_) => return Ok(item("skills", None, "config_ilegivel", None, false, &[])),
        _ => {}
    }
    let mut counts = [0usize; 4];
    let roots = [
        ("plugins", env.home.join(".claude/plugins")),
        ("pessoais", env.home.join(".claude/skills")),
        ("hangar", repo.join("skills")),
        ("agents", env.home.join(".agents/skills")),
    ];
    for link in &links {
        let target = std::fs::read_link(link).map_err(|e| e.to_string())?;
        for (index, (_, root)) in roots.iter().enumerate() {
            if target.starts_with(root) && target != *root {
                counts[index] += 1;
                break;
            }
        }
    }
    let origin = roots
        .iter()
        .enumerate()
        .filter(|(i, _)| counts[*i] > 0)
        .map(|(i, (name, _))| format!("{} {name}", counts[i]))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(item(
        "skills",
        Some(true),
        "skills_ok",
        None,
        false,
        &[("n", links.len().to_string()), ("origem", origin)],
    ))
}
