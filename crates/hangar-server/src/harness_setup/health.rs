//! Diagnóstico de configuração dos harnesses, sem executar o Python.
use crate::{
    accounts::environment::AccountEnvironment,
    orchestration::{codec::{self, ValueGuard}, truthy},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn read_json(path: &Path) -> Option<ValueGuard> {
    let text = std::fs::read_to_string(path).ok()?;
    codec::loads(&text).map(ValueGuard)
}
fn toml_value(value: toml::Value) -> Value {
    match value {
        toml::Value::Float(number) if !number.is_finite() => codec::loads(if number.is_nan() {
            "NaN"
        } else if number.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        })
        .expect("constante JSON conhecida"),
        toml::Value::Datetime(value) => Value::String(value.to_string()),
        toml::Value::Array(values) => Value::Array(values.into_iter().map(toml_value).collect()),
        toml::Value::Table(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_value(value)))
                .collect(),
        ),
        value => serde_json::to_value(value).expect("valor TOML escalar finito"),
    }
}

pub(super) fn read_toml(path: &Path) -> Result<toml::Value, String> {
    let empty = || toml::Value::Table(Default::default());
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(empty()),
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "configuração TOML não está em UTF-8".to_owned())?;
    let value = toml::from_str::<toml::Value>(text).unwrap_or_else(|_| empty());
    let mut pending = vec![&value];
    while let Some(item) = pending.pop() {
        match item {
            toml::Value::Datetime(datetime) => {
                if datetime.date.is_some_and(|date| {
                    date.year == 0
                        || chrono::NaiveDate::from_ymd_opt(
                            i32::from(date.year),
                            u32::from(date.month),
                            u32::from(date.day),
                        )
                        .is_none()
                }) || datetime
                    .time
                    .is_some_and(|time| time.second.is_none_or(|second| second >= 60))
                {
                    return Ok(empty());
                }
            }
            toml::Value::Array(values) => pending.extend(values),
            toml::Value::Table(values) => pending.extend(values.values()),
            _ => (),
        }
    }
    Ok(value)
}
pub(super) fn read(path: &Path) -> ValueGuard {
    let value = read_json(path).unwrap_or_else(|| ValueGuard(json!({})));
    if value.is_object() {
        value
    } else {
        ValueGuard(json!({}))
    }
}
fn temporal_text(datetime: &toml::value::Datetime, nested: bool) -> String {
    let mut args = Vec::new();
    let mut text = String::new();
    if let Some(date) = datetime.date {
        args.extend([
            u32::from(date.year),
            u32::from(date.month),
            u32::from(date.day),
        ]);
        text = format!("{:04}-{:02}-{:02}", date.year, date.month, date.day);
    }
    if let Some(time) = datetime.time {
        let second = time.second.unwrap_or(0);
        let micros = time.nanosecond.unwrap_or(0) / 1000;
        args.extend([u32::from(time.hour), u32::from(time.minute)]);
        if second != 0 || micros != 0 {
            args.push(u32::from(second));
        }
        if micros != 0 {
            args.push(micros);
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!(
            "{:02}:{:02}:{:02}",
            time.hour, time.minute, second
        ));
        if micros != 0 {
            text.push_str(&format!(".{micros:06}"));
        }
    }
    let mut timezone = String::new();
    if let Some(offset) = datetime.offset {
        let minutes = match offset {
            toml::value::Offset::Z => 0,
            toml::value::Offset::Custom { minutes } => i32::from(minutes),
        };
        text.push_str(&format!(
            "{}{:02}:{:02}",
            if minutes < 0 { '-' } else { '+' },
            minutes.abs() / 60,
            minutes.abs() % 60
        ));
        timezone = if minutes == 0 {
            ", tzinfo=datetime.timezone.utc".into()
        } else {
            let seconds = minutes * 60;
            let delta = if seconds < 0 {
                format!("days=-1, seconds={}", seconds.rem_euclid(86400))
            } else {
                format!("seconds={seconds}")
            };
            format!(", tzinfo=datetime.timezone(datetime.timedelta({delta}))")
        };
    }
    if nested {
        let kind = match (datetime.date.is_some(), datetime.time.is_some()) {
            (true, true) => "datetime",
            (true, false) => "date",
            _ => "time",
        };
        format!(
            "datetime.{kind}({}{timezone})",
            args.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        text
    }
}
fn toml_text(value: &toml::Value, nested: bool) -> String {
    match value {
        toml::Value::Datetime(datetime) => temporal_text(datetime, nested),
        toml::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| toml_text(value, true))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        toml::Value::Table(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    toml_text(&toml::Value::String(key.clone()), true),
                    toml_text(value, true)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => {
            let value = toml_value(value.clone());
            if nested && value.is_string() && !codec::contains_nonfinite(&value) {
                let wrapped = codec::python_string(&Value::Array(vec![value]));
                wrapped[1..wrapped.len() - 1].to_owned()
            } else {
                codec::python_string(&value)
            }
        }
    }
}
pub(super) fn item(
    id: &str,
    ok: Option<bool>,
    code: &str,
    repair: Option<&str>,
    info: bool,
    params: &[(&str, String)],
) -> Value {
    let params: serde_json::Map<String, Value> = params
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.clone())))
        .collect();
    json!({"id":id,"ok":ok,"codigo":code,"params":params,"conserto":repair,"info":info})
}
pub(super) fn agent_dir(env: &AccountEnvironment, cli: &str) -> Result<PathBuf, String> {
    if cli == "omp" {
        env.omp_directories()
            .map(|v| v.agent_dir)
            .map_err(str::to_owned)
    } else {
        Ok(super::files::directory(env, cli))
    }
}
fn config(env: &AccountEnvironment, cli: &str, json_name: &str) -> Result<ValueGuard, String> {
    let dir = agent_dir(env, cli)?;
    if matches!(cli, "codex" | "kimi") {
        Ok(ValueGuard(toml_value(read_toml(&dir.join("config.toml"))?)))
    } else {
        Ok(read(&dir.join(json_name)))
    }
}
pub fn mcp(env: &AccountEnvironment, cli: &str) -> Result<Value, String> {
    let doc = match cli {
        "claude" => read(&env.default_claude_json()),
        "agy" => read(&hangar_antigravity::Account::default().gemini_dir(&env.home).join("config/mcp_config.json")),
        _ => config(env, cli, "mcp.json")?,
    };
    let field = match cli {
        "codex" => "mcp_servers",
        "kimi" if truthy(&doc["mcp"]) => "mcp",
        "kimi" => "mcp_servers",
        _ => "mcpServers",
    };
    let servers = &doc[field];
    if !truthy(servers) {
        return Ok(item("mcp", Some(true), "mcp_nenhum", None, true, &[]));
    }
    let Some(servers) = servers.as_object() else {
        return Err("configuração MCP não é um objeto".into());
    };
    let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
    names.sort();
    Ok(item(
        "mcp",
        Some(true),
        "mcp_ok",
        None,
        true,
        &[("n", names.len().to_string()), ("lista", names.join(", "))],
    ))
}
pub fn default_model(env: &AccountEnvironment, cli: &str) -> Result<Value, String> {
    if matches!(cli, "codex" | "kimi") {
        let document = read_toml(&agent_dir(env, cli)?.join("config.toml"))?;
        let field = if cli == "kimi" {
            "default_model"
        } else {
            "model"
        };
        let model = document
            .get(field)
            .filter(|value| truthy(&toml_value((*value).clone())))
            .map(|value| toml_text(value, false))
            .unwrap_or_default();
        return Ok(model_item(model));
    }
    let doc = match cli {
        "claude" => read(&env.home.join(".claude/settings.json")),
        "agy" => read(&hangar_antigravity::Account::default().settings_path(&env.home)),
        _ => config(env, cli, "settings.json")?,
    };
    let model = if matches!(cli, "pi" | "omp") {
        let mut parts = Vec::new();
        for key in ["defaultProvider", "defaultModel"] {
            if truthy(&doc[key]) {
                if codec::contains_nonfinite(&doc[key]) {
                    return Err("modelo padrão não é texto".into());
                }
                parts.push(doc[key].as_str().ok_or("modelo padrão não é texto")?);
            }
        }
        parts.join("/")
    } else {
        let value = &doc[if cli == "kimi" {
            "default_model"
        } else {
            "model"
        }];
        if truthy(value) {
            codec::python_string(value)
        } else {
            String::new()
        }
    };
    Ok(model_item(model))
}
fn model_item(model: String) -> Value {
    if model.is_empty() {
        item(
            "modelo",
            Some(true),
            "modelo_padrao_nenhum",
            None,
            true,
            &[],
        )
    } else {
        item(
            "modelo",
            Some(true),
            "modelo_padrao",
            None,
            true,
            &[("modelo", model)],
        )
    }
}
pub fn codex_hooks(env: &AccountEnvironment) -> Value {
    let doc = read(&env.codex_default.join("hooks.json"));
    let Some(hooks) = doc["hooks"].as_object().filter(|h| !h.is_empty()) else {
        return item("hooks", Some(true), "hooks_nenhum", None, true, &[]);
    };
    let count: usize = hooks
        .values()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .sum();
    let mut events: Vec<&str> = hooks.keys().map(String::as_str).collect();
    events.sort();
    item(
        "hooks",
        Some(true),
        "hooks_codex",
        None,
        true,
        &[("n", count.to_string()), ("eventos", events.join(", "))],
    )
}
pub fn fullscreen(env: &AccountEnvironment, cli: &str) -> Value {
    if cli == "claude" {
        let path = env.home.join(".claude/settings.json");
        if !path.is_file() {
            return item("fullscreen", None, "config_ilegivel", None, false, &[]);
        }
        let doc = read(&path);
        let enabled = doc["tui"].as_str() == Some("fullscreen");
        return item(
            "fullscreen",
            Some(enabled),
            if enabled {
                "fullscreen_ok"
            } else {
                "fullscreen_claude_desligado"
            },
            if enabled {
                None
            } else {
                Some("fullscreen:claude")
            },
            false,
            &[],
        );
    }
    let Ok(dir) = agent_dir(env, cli) else {
        return item("fullscreen", None, "config_ilegivel", None, false, &[]);
    };
    let path = dir.join("fullscreen-tui.json");
    if !path.is_file() {
        return item(
            "fullscreen",
            Some(false),
            "fullscreen_desligado",
            Some(&format!("fullscreen:{cli}")),
            false,
            &[],
        );
    }
    let Some(doc) = read_json(&path).filter(|v| v.is_object()) else {
        return item("fullscreen", None, "config_ilegivel", None, false, &[]);
    };
    item(
        "fullscreen",
        Some(true),
        if doc["enabled"].as_bool() == Some(true) {
            "fullscreen_ok"
        } else {
            "fullscreen_por_escolha"
        },
        None,
        false,
        &[],
    )
}

pub fn wrapper(env: &AccountEnvironment, cli: &str, windows: bool) -> Value {
    let mut missing = Vec::new();
    let mut present = Vec::new();
    for (name, path) in [
        ("fish", env.home.join(".config/fish")),
        ("bash", env.home.join(".bashrc")),
        ("zsh", env.home.join(".zshrc")),
    ] {
        if !path.exists() {
            continue;
        }
        let found = if name == "fish" {
            path.join(format!("functions/{cli}.fish")).is_file()
        } else {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    String::from_utf8_lossy(&bytes).contains(&format!("shell/{cli}.posix.sh"))
                }
                Err(_) => return item("wrapper", None, "config_ilegivel", None, false, &[]),
            }
        };
        if found {
            present.push(name.to_owned());
        } else {
            missing.push(name.to_owned());
        }
    }
    if windows {
        let profile = env
            .base
            .get("USERPROFILE")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.clone())
            .join("Documents");
        let mut texts = Vec::new();
        for folder in ["WindowsPowerShell", "PowerShell"] {
            for name in ["profile.ps1", "Microsoft.PowerShell_profile.ps1"] {
                let path = profile.join(folder).join(name);
                if !path.exists() {
                    continue;
                }
                match std::fs::read(path) {
                    Ok(bytes) => texts.push(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(_) => return item("wrapper", None, "config_ilegivel", None, false, &[]),
                }
            }
        }
        if !texts.is_empty() {
            if texts
                .join("\n")
                .replace('\\', "/")
                .contains(&format!("shell/{cli}.ps1"))
            {
                present.push("PowerShell".into());
            } else {
                missing.push("PowerShell".into());
            }
        }
    }
    if cli == "codex" {
        // O pane do Codex roda `hangar-server codex-tui`; no Windows ele passa pelo `.cmd` do PATH.
        let launchers: &[&str] = if windows { &["hangar-codex", "hangar-codex-tui"] } else { &["hangar-codex"] };
        for name in launchers {
            if !env.home.join(".local/bin").join(name).exists() {
                missing.push(format!("lançador {name}"));
            }
        }
    }
    if missing.is_empty() && present.is_empty() {
        return item("wrapper", None, "wrapper_sem_shell", None, false, &[]);
    }
    if !missing.is_empty() {
        let repair = windows || crate::cli_probe::resolve_program("bash", env).is_some();
        return item(
            "wrapper",
            Some(false),
            "wrapper_falta",
            repair.then_some("wrapper"),
            false,
            &[("lista", missing.join(", "))],
        );
    }
    item(
        "wrapper",
        Some(true),
        "wrapper_ok",
        None,
        false,
        &[("onde", present.join(", "))],
    )
}
pub fn psmux_config(env: &AccountEnvironment) -> PathBuf {
    if let Some(path) = env.base.get("PSMUX_CONFIG_FILE").filter(|s| !s.is_empty()) {
        return PathBuf::from(path);
    }
    [
        ".psmux.conf",
        ".psmuxrc",
        ".tmux.conf",
        ".config/psmux/psmux.conf",
    ]
    .into_iter()
    .map(|p| env.home.join(p))
    .find(|p| p.exists())
    .unwrap_or_else(|| env.home.join(".tmux.conf"))
}
pub fn extensions(env: &AccountEnvironment, repo: &Path, cli: &str) -> Result<Value, String> {
    let names: Vec<&str> = [
        "hangar-state",
        "rich-status-line",
        "claude-bridge",
        "claude-todo",
        "claude-hooks-adapter",
        "git-checkpoint",
        "fullscreen-tui",
    ]
    .into_iter()
    .filter(|name| cli != "omp" || !matches!(*name, "claude-todo" | "fullscreen-tui"))
    .collect();
    let dir = agent_dir(env, cli)?.join("extensions");
    let mut missing = Vec::new();
    let mut other = Vec::new();
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let short = |path: PathBuf| {
        path.strip_prefix(canonical(&env.home))
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|_| path.display().to_string())
    };
    for name in &names {
        let path = dir.join(format!("{name}.ts"));
        let source = repo.join("scripts/pi").join(format!("{name}.ts"));
        if path.exists() {
            if path.is_symlink() && canonical(&path) != canonical(&source) {
                other.push(format!("{name} → {}", short(canonical(&path))));
            }
        } else {
            missing.push((*name).to_owned());
        }
    }
    let lib = dir.join("lib");
    if lib.exists() {
        if lib.is_symlink() && canonical(&lib) != canonical(&repo.join("scripts/pi/lib")) {
            other.push(format!("lib → {}", short(canonical(&lib))));
        }
    } else {
        missing.push("lib".into());
    }
    let repair = format!("extensoes:{cli}");
    Ok(if !other.is_empty() {
        let mut params = vec![("lista", other.join(", "))];
        if !missing.is_empty() {
            params.push(("faltam", missing.join(", ")));
        }
        item(
            "extensoes",
            Some(false),
            "extensoes_outra_fonte",
            Some(&repair),
            false,
            &params,
        )
    } else if !missing.is_empty() {
        item(
            "extensoes",
            Some(false),
            "faltam",
            Some(&repair),
            false,
            &[("lista", missing.join(", "))],
        )
    } else {
        item(
            "extensoes",
            Some(true),
            "extensoes_ok",
            None,
            false,
            &[("n", names.len().to_string())],
        )
    })
}
pub fn claude_hooks(directory: &Path) -> Value {
    let Some(doc) = read_json(&directory.join("settings.json")).filter(|v| v.is_object()) else {
        return item("hooks", None, "config_ilegivel", None, false, &[]);
    };
    let hooks = &doc["hooks"];
    let text = codec::dumps(hooks, false);
    let mut missing: Vec<String> = [
        "state-hook",
        "askq-capture",
        "preview-hook",
        "pair-context",
        "nav-hook",
    ]
    .into_iter()
    .filter(|name| !text.contains(name))
    .map(str::to_owned)
    .collect();
    let mut group = false;
    let mut executable = false;
    if let Some(events) = hooks.as_object() {
        for blocks in events.values().filter_map(Value::as_array) {
            for block in blocks {
                if let Some(entries) = block["hooks"].as_array() {
                    for entry in entries {
                        if let Some(command) = entry["command"]
                            .as_str()
                            .filter(|c| c.contains("pair-context"))
                        {
                            group = true;
                            executable |= crate::loops::check::split(command, true)
                                .ok()
                                .and_then(|args| args.first().map(|s| Path::new(s).is_file()))
                                .unwrap_or(false);
                        }
                    }
                }
            }
        }
    }
    if group && !executable {
        missing.push("pair-context".into());
    }
    if !missing.is_empty() {
        item(
            "hooks",
            Some(false),
            "faltam",
            Some("hooks-claude"),
            false,
            &[("lista", missing.join(", "))],
        )
    } else {
        item(
            "hooks",
            Some(true),
            "hooks_ok",
            None,
            false,
            &[("n", "5".into())],
        )
    }
}

/// A entrada do Hangar em toda conta onde o `agy` já rodou; sem ela o pane não acha a conversa.
pub fn agy_hooks(home: &Path) -> Value {
    use hangar_antigravity::hooks::{ENTRY, EVENTS};
    let mut used = false;
    let mut missing: Vec<&str> = Vec::new();
    for account in hangar_antigravity::paths::known_accounts(home) {
        if !account.data_dir(home).is_dir() {
            continue;
        }
        used = true;
        let doc = match std::fs::read_to_string(account.hooks_path(home)) {
            Ok(text) if text.trim_start_matches('\u{feff}').trim().is_empty() => json!({}),
            Ok(text) => match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
                Ok(doc) if doc.is_object() => doc,
                _ => return item("hooks", None, "config_ilegivel", None, false, &[]),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(_) => return item("hooks", None, "config_ilegivel", None, false, &[]),
        };
        for event in EVENTS {
            let found = doc[ENTRY][event].as_array().into_iter().flatten().any(|handler| {
                handler["command"].as_str().is_some_and(|c| c.contains(super::agy_hooks::MODE))
            });
            if !found && !missing.contains(&event) {
                missing.push(event);
            }
        }
    }
    if !used {
        return item("hooks", None, "agy_hooks_sem_uso", None, true, &[]);
    }
    if missing.is_empty() {
        item("hooks", Some(true), "hooks_ok", None, false, &[("n", EVENTS.len().to_string())])
    } else {
        // A ordem é a de EVENTS, não a de leitura das contas.
        missing.sort_by_key(|event| EVENTS.iter().position(|e| e == event));
        item("hooks", Some(false), "faltam", Some("hooks-agy"), false, &[("lista", missing.join(", "))])
    }
}
pub fn agy_version(text: &str) -> Value {
    let (a, b, c) = hangar_antigravity::version::MIN;
    let minimum = format!("{a}.{b}.{c}");
    let Some((x, y, z)) = hangar_antigravity::version::parse(text) else {
        return item("versao", None, "config_ilegivel", None, false, &[]);
    };
    let params = [("versao", format!("{x}.{y}.{z}")), ("minimo", minimum)];
    if hangar_antigravity::version::supported(text) == Some(true) {
        item("versao", Some(true), "agy_versao_ok", None, false, &params)
    } else {
        item("versao", Some(false), "agy_versao_antiga", None, false, &params)
    }
}
pub fn agy_accounts(home: &Path) -> Value {
    let accounts = hangar_antigravity::paths::known_accounts(home);
    let names: Vec<String> = accounts
        .iter()
        .map(|account| match &account.gemini_home {
            None => "padrão".to_owned(),
            Some(dir) => {
                let name = dir.file_name().unwrap_or_default().to_string_lossy();
                name.strip_prefix(hangar_antigravity::paths::ACCOUNT_PREFIX).unwrap_or(&*name).to_owned()
            }
        })
        .collect();
    item("contas", Some(true), "agy_contas", None, true, &[("n", accounts.len().to_string()), ("lista", names.join(", "))])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks_file(home: &Path, doc: &Value) {
        let path = home.join(".gemini/config/hooks.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(doc).unwrap()).unwrap();
    }

    #[test]
    fn agy_hooks_wait_for_the_first_use() {
        let home = tempfile::tempdir().unwrap();
        let value = agy_hooks(home.path());
        assert_eq!(value["ok"], Value::Null);
        assert_eq!(value["info"], true);
        assert_eq!(value["codigo"], "agy_hooks_sem_uso");
        assert_eq!(value["conserto"], Value::Null);
    }

    #[test]
    fn agy_hooks_name_the_missing_events() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        let value = agy_hooks(home.path());
        assert_eq!(value["ok"], false);
        assert_eq!(value["codigo"], "faltam");
        assert_eq!(value["params"]["lista"], "SessionStart, PreInvocation, PostInvocation, Stop");
        assert_eq!(value["conserto"], "hooks-agy");
        hooks_file(home.path(), &json!({"hangar":{"Stop":[{"type":"command","command":"/x/hangar-server agy-state-hook"}]},"lint":{}}));
        let value = agy_hooks(home.path());
        assert_eq!(value["codigo"], "faltam");
        assert_eq!(value["params"]["lista"], "SessionStart, PreInvocation, PostInvocation");
        assert_eq!(value["conserto"], "hooks-agy");
    }

    #[test]
    fn agy_hooks_complete_and_unreadable() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        let mut doc = json!({"lint":{}});
        doc["hangar"] = hangar_antigravity::hooks::entry("/x/hangar-server agy-state-hook", "/x/hangar-server agy-guard-tmux");
        hooks_file(home.path(), &doc);
        let value = agy_hooks(home.path());
        assert_eq!(value["ok"], true);
        assert_eq!(value["codigo"], "hooks_ok");
        assert_eq!(value["params"]["n"], "4");
        assert_eq!(value["conserto"], Value::Null);
        std::fs::write(home.path().join(".gemini/config/hooks.json"), "{ quebrado").unwrap();
        let value = agy_hooks(home.path());
        assert_eq!(value["ok"], Value::Null);
        assert_eq!(value["codigo"], "config_ilegivel");
    }

    #[test]
    fn agy_version_against_the_minimum() {
        let value = agy_version("1.5.15");
        assert_eq!((value["ok"].clone(), value["codigo"].clone()), (json!(true), json!("agy_versao_ok")));
        assert_eq!(value["params"], json!({"versao":"1.5.15","minimo":"1.5.13"}));
        let value = agy_version("1.5.12");
        assert_eq!((value["ok"].clone(), value["codigo"].clone()), (json!(false), json!("agy_versao_antiga")));
        assert_eq!(value["conserto"], Value::Null);
        assert_eq!(agy_version("sem versão")["codigo"], "config_ilegivel");
    }

    #[test]
    fn agy_accounts_list_the_default_and_the_extra_ones() {
        let home = tempfile::tempdir().unwrap();
        let extra = home.path().join(".antigravity-trabalho");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(extra.join(hangar_antigravity::paths::MARKER), "").unwrap();
        let value = agy_accounts(home.path());
        assert_eq!(value["codigo"], "agy_contas");
        assert_eq!(value["info"], true);
        assert_eq!(value["params"], json!({"n":"2","lista":"padrão, trabalho"}));
    }
}
