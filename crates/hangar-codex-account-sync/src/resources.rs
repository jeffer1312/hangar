//! Recursos herdados: caminhos confinados, cópia seletiva e retirada apenas do gerenciado.
use crate::{Error, Native, Paths, hash, issue, json_bytes, object};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

pub(crate) type Files = BTreeMap<String, Vec<u8>>;
const ROOTS: &[&str] = &[
    "AGENTS.md",
    "AGENTS.override.md",
    "hooks.json",
    "agents",
    "skills",
    "hooks",
    ".hangar-hooks",
];
const SENSITIVE: &[&str] = &[
    "auth.json",
    ".credentials.json",
    ".claude.json",
    "projects",
    "sessions",
    "archived_sessions",
    "agent.db",
    "models.db",
    "state.sqlite",
    "config.sqlite",
];

pub(crate) fn safe_relative(relative: &str) -> bool {
    !relative.is_empty()
        && relative != "."
        && Path::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
pub(crate) fn safe_destination(
    root: &Path,
    relative: &str,
    leaf_link: bool,
) -> Result<PathBuf, Error> {
    if !safe_relative(relative) || root.is_symlink() {
        return Err(Error::Invalid);
    }
    let path = root.join(relative);
    for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
        if ancestor == root {
            break;
        }
        if ancestor.is_symlink() {
            return Err(Error::Invalid);
        }
    }
    if path.is_symlink() && !leaf_link {
        return Err(Error::Invalid);
    }
    Ok(path)
}
fn forbidden(relative: &Path) -> bool {
    relative.components().any(|c| {
        let part = c.as_os_str().to_string_lossy();
        SENSITIVE.contains(&part.as_ref())
            || (part.starts_with(".hangar-") && part != ".hangar-hooks")
            || [".sqlite", ".sqlite3", ".db"]
                .iter()
                .any(|s| part.ends_with(s))
    })
}
fn account_root(path: &Path, home: &Path) -> bool {
    path.ancestors().any(|p| {
        p.parent() == Some(home)
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".codex-"))
    })
}
fn external_agents(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    let mut any = false;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let path = entry.path();
        any = true;
        if !path.is_file()
            || path
                .extension()
                .is_none_or(|s| s.to_string_lossy().to_lowercase() != "md")
            || path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("readme.md"))
        {
            return false;
        }
    }
    any
}
pub(crate) fn collect(paths: Paths<'_>) -> (Files, Vec<Value>) {
    collect_inner(paths, true)
}
pub(crate) fn collect_names(paths: Paths<'_>) -> (Files, Vec<Value>) {
    collect_inner(paths, false)
}
fn collect_inner(paths: Paths<'_>, contents: bool) -> (Files, Vec<Value>) {
    fn walk(
        paths: Paths<'_>,
        relative: &Path,
        files: &mut Files,
        issues: &mut Vec<Value>,
        active: &mut BTreeSet<PathBuf>,
        contents: bool,
    ) -> Result<(), Error> {
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        if relative
            .components()
            .any(|c| c.as_os_str() == ".hangar-uploads")
        {
            return Ok(());
        }
        let path = paths.source.join(relative);
        let mut reject = |code| issues.push(issue(code, json!({"path":relative_text})));
        if forbidden(relative) {
            reject("codex_account_source_forbidden");
            return Ok(());
        }
        let real = fs::canonicalize(&path)?;
        let source = fs::canonicalize(paths.source)?;
        let home = fs::canonicalize(paths.home)?;
        if real.is_dir() {
            if path.is_symlink() {
                if relative.components().count() == 1 {
                    reject("codex_account_source_root_link");
                    return Ok(());
                }
                let root = relative
                    .components()
                    .next()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .unwrap_or_default();
                if real == source
                    || real == home
                    || source.starts_with(&real)
                    || account_root(&real, &home)
                    || (root == "skills" && !real.join("SKILL.md").is_file())
                    || (root == "agents" && !external_agents(&real))
                    || (!matches!(root.as_ref(), "skills" | "agents")
                        && real.file_name() != relative.file_name())
                {
                    reject("codex_account_source_broad_link");
                    return Ok(());
                }
            }
            if !active.insert(real.clone()) {
                reject("codex_account_source_cycle");
                return Ok(());
            }
            let mut children = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|e| e.file_name());
            for child in children {
                let next = relative.join(child.file_name());
                if let Err(error) = walk(paths, &next, files, issues, active, contents) {
                    issues.push(issue("codex_account_source_unreadable",json!({"path":next.to_string_lossy().replace('\\',"/"),"error":error.kind()})));
                }
            }
            active.remove(&real);
        } else if real.is_file() {
            let target_forbidden = real
                .strip_prefix(&source)
                .map(|r| r == Path::new("config.toml") || forbidden(r))
                .unwrap_or_else(|_| account_root(&real, &home));
            if path.is_symlink()
                && (target_forbidden || forbidden(Path::new(real.file_name().unwrap_or_default())))
            {
                reject("codex_account_source_forbidden");
                return Ok(());
            }
            files.insert(
                relative_text,
                if contents { fs::read(path)? } else { vec![] },
            );
        }
        Ok(())
    }
    let mut files = Files::new();
    let mut issues = Vec::new();
    let mut active = BTreeSet::new();
    for name in ROOTS {
        let path = paths.source.join(name);
        if (path.exists() || path.is_symlink())
            && let Err(error) = walk(
                paths,
                Path::new(name),
                &mut files,
                &mut issues,
                &mut active,
                contents,
            )
        {
            issues.push(issue(
                "codex_account_source_unreadable",
                json!({"path":name,"error":error.kind()}),
            ));
        }
    }
    if contents
        && files.get("hooks.json").is_some_and(|raw| {
            serde_json::from_slice::<Value>(raw)
                .ok()
                .is_none_or(|v| !v.is_object())
        })
    {
        files.remove("hooks.json");
        issues.push(issue(
            "codex_account_source_invalid",
            json!({"path":"hooks.json"}),
        ));
    }
    (files, issues)
}
pub(crate) fn source_relative(value: &str, source: &Path, home: &Path) -> Option<String> {
    let expanded = if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(value)
    };
    let relative = if expanded.is_absolute() {
        expanded
            .strip_prefix(source)
            .ok()
            .map(Path::to_owned)
            .or_else(|| {
                fs::canonicalize(&expanded)
                    .ok()?
                    .strip_prefix(fs::canonicalize(source).ok()?)
                    .ok()
                    .map(Path::to_owned)
            })?
    } else if source.join(&expanded).exists() || source.join(&expanded).is_symlink() {
        expanded
    } else {
        return None;
    };
    let normalized: PathBuf = relative.components().filter(|component| !matches!(component, Component::CurDir)).collect();
    Some(normalized.to_string_lossy().replace('\\', "/"))
}
fn map_path(
    value: &str,
    paths: Paths<'_>,
    available: &BTreeSet<String>,
    issues: &mut Vec<Value>,
    key: &str,
) -> Option<String> {
    let Some(relative) = source_relative(value, paths.source, paths.home) else {
        return Some(value.into());
    };
    if available.contains(&relative) {
        Some(
            paths
                .destination
                .join(relative)
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        issues.push(issue(
            "codex_account_unmapped_reference",
            json!({"key":key,"path":relative}),
        ));
        None
    }
}
fn map_command(
    value: &str,
    paths: Paths<'_>,
    available: &BTreeSet<String>,
    issues: &mut Vec<Value>,
) -> Option<String> {
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let mut i = 0;
    let mut replacements = Vec::new();
    while i < chars.len() {
        while i < chars.len() && chars[i].1.is_whitespace() {
            i += 1;
        }
        if i == chars.len() {
            break;
        }
        let quote = if matches!(chars[i].1, '\'' | '"') {
            Some(chars[i].1)
        } else {
            None
        };
        if quote.is_some() {
            i += 1;
        }
        let start = chars.get(i).map_or(value.len(), |c| c.0);
        while i < chars.len() {
            let c = chars[i].1;
            if quote == Some(c) || (quote.is_none() && c.is_whitespace()) {
                break;
            }
            if quote == Some('"') && c == '\\' && i + 1 < chars.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        if quote.is_some() && i == chars.len() {
            if value.contains(paths.source.to_string_lossy().as_ref()) {
                issues.push(issue(
                    "codex_account_unmapped_reference",
                    json!({"key":"command"}),
                ));
                return None;
            }
            return Some(value.into());
        }
        let end = chars.get(i).map_or(value.len(), |c| c.0);
        let token = &value[start..end];
        let mapped = map_path(token, paths, available, issues, "command")?;
        if mapped != token {
            replacements.push((start, end, mapped));
        }
        if quote.is_some() {
            i += 1;
        }
    }
    let mut result = value.to_owned();
    for (start, end, mapped) in replacements.into_iter().rev() {
        result.replace_range(start..end, &mapped);
    }
    Some(result)
}
pub(crate) fn map_tree(
    value: &Value,
    paths: Paths<'_>,
    available: &BTreeSet<String>,
    issues: &mut Vec<Value>,
    key: &str,
    strict: bool,
) -> Option<Value> {
    match value {
        Value::Object(entries) => {
            let mut out = serde_json::Map::new();
            for (k, v) in entries {
                match map_tree(v, paths, available, issues, k, strict) {
                    Some(mapped) => {
                        out.insert(k.clone(), mapped);
                    }
                    None if strict => return None,
                    None => {}
                }
            }
            Some(Value::Object(out))
        }
        Value::Array(values) => {
            let mut out = Vec::new();
            for v in values {
                match map_tree(v, paths, available, issues, key, strict) {
                    Some(mapped) => out.push(mapped),
                    None if strict => return None,
                    None => {}
                }
            }
            Some(Value::Array(out))
        }
        Value::String(text) if key == "command" && strict => {
            map_command(text, paths, available, issues).map(Value::String)
        }
        Value::String(text)
            if [
                "config_file",
                "model_instructions_file",
                "model_catalog_json",
                "cwd",
                "working_directory",
                "path",
                "file",
                "script",
                "program",
            ]
            .contains(&key) =>
        {
            map_path(text, paths, available, issues, key).map(Value::String)
        }
        _ => Some(value.clone()),
    }
}
pub(crate) fn transform(
    files: Files,
    paths: Paths<'_>,
    native: &impl Native,
    issues: &mut Vec<Value>,
) -> (Files, BTreeMap<String, Value>) {
    let available = files.keys().cloned().collect();
    let mut out = Files::new();
    let mut overrides = BTreeMap::new();
    for (relative, raw) in files {
        if relative.starts_with("agents/") && relative.ends_with(".toml") {
            let parsed = std::str::from_utf8(&raw)
                .map_err(|_| String::new())
                .and_then(|text| native.parse_toml(text));
            match parsed {
                Ok(value) => match map_tree(&value, paths, &available, issues, "", true) {
                    Some(mapped) if mapped != value => {
                        overrides.insert(relative.clone(), mapped);
                    }
                    Some(_) => {}
                    None => {
                        issues.push(issue(
                            "codex_account_unmapped_reference",
                            json!({"path":relative}),
                        ));
                        continue;
                    }
                },
                Err(_) => {
                    issues.push(issue(
                        "codex_account_source_invalid",
                        json!({"path":relative}),
                    ));
                    continue;
                }
            }
        }
        if relative.ends_with(".json")
            && let Ok(value) = serde_json::from_slice::<Value>(&raw)
        {
            if let Some(mapped) = map_tree(&value, paths, &available, issues, "", true) {
                out.insert(relative, crate::json_bytes(&mapped));
            }
            continue;
        }
        out.insert(relative, raw);
    }
    (out, overrides)
}
pub(crate) fn snapshot(
    root: &Path,
    paths: &BTreeSet<String>,
    destination: bool,
) -> Result<(String, Value), Error> {
    let mut entries = serde_json::Map::new();
    for relative in paths {
        let path = root.join(relative);
        let entry = if destination && safe_destination(root, relative, true).is_err() {
            json!({"kind":"path-conflict","hash":""})
        } else if path.is_symlink() {
            let target = fs::read_link(&path)?;
            let mut entry = json!({"kind":"symlink","target":target,"hash":hash(target.to_string_lossy().as_bytes())});
            if !destination && let Ok(raw) = fs::read(&path) {
                entry["hash"] = json!(hash(&raw));
                entry["resolved"] = json!(fs::canonicalize(&path)?);
                entry["executable"] = json!(executable(&path));
            }
            entry
        } else if path.is_file() {
            json!({"kind":"file","hash":hash(&fs::read(&path)?),"executable":executable(&path)})
        } else if path.exists() {
            json!({"kind":"other","hash":""})
        } else {
            json!({"kind":"missing","hash":""})
        };
        entries.insert(relative.clone(), entry);
    }
    let digest = hash(&json_bytes(&Value::Object(entries.clone())));
    Ok((digest, json!({"files":entries})))
}
#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o100 != 0)
}
#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    false
}
pub(crate) fn sync(
    files: &Files,
    paths: Paths<'_>,
    previous: &Value,
    source_snapshot: &Value,
    backups: &Path,
    issues: &mut Vec<Value>,
    allow_removals: bool,
) -> Value {
    let old = object(previous);
    let mut result = serde_json::Map::new();
    for (relative, raw) in files {
        let apply = || -> Result<Value, Error> {
            let path = safe_destination(paths.destination, relative, true)?;
            let prior = old.get(relative).cloned().unwrap_or(Value::Null);
            let old_link = fs::read_link(&path).ok();
            if old_link.as_ref().is_some_and(|link| {
                Some(link.to_string_lossy().as_ref()) != prior["target"].as_str()
            }) {
                return Err(Error::Invalid);
            }
            let current = if old_link.is_some() {
                None
            } else {
                crate::read(&path)?
            };
            if current
                .as_ref()
                .is_some_and(|v| prior["hash"].is_null() && v != raw)
            {
                return Err(Error::Conflict);
            }
            let source_entry = &source_snapshot["files"][relative];
            let target = source_entry["resolved"]
                .as_str()
                .filter(|_| {
                    relative.starts_with("hooks/") || relative.starts_with(".hangar-hooks/")
                })
                .filter(|target| !Path::new(target).starts_with(paths.source))
                .filter(|_| source_entry["kind"] == "symlink" && source_entry["hash"] == hash(raw));
            if let Some(target) = target {
                if current
                    .as_ref()
                    .is_some_and(|v| prior["hash"].as_str() != Some(hash(v).as_str()))
                {
                    return Err(Error::Invalid);
                }
                crate::write_link(
                    &path,
                    Path::new(target),
                    current.as_deref(),
                    old_link.as_deref(),
                    backups,
                )?;
                return Ok(json!({"hash":hash(raw),"target":target}));
            }
            if old_link.is_some() {
                crate::replace_link_with_file(&path, raw, old_link.as_deref(), backups)?;
            } else {
                crate::write(&path, raw, current.as_deref(), Some(backups))?;
            }
            let executable = source_entry["executable"].as_bool().unwrap_or(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&path)?.permissions().mode();
                fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(
                        (mode & !0o100) | if executable { 0o100 } else { 0 },
                    ),
                )?;
            }
            Ok(json!({"hash":hash(raw),"executable":executable}))
        };
        match apply() {
            Ok(entry) => {
                result.insert(relative.clone(), entry);
            }
            Err(Error::Conflict) => issues.push(issue(
                "codex_account_resource_conflict",
                json!({"path":relative}),
            )),
            Err(error) => {
                issues.push(issue(
                    "codex_account_path_conflict",
                    json!({"path":relative,"error":error.kind()}),
                ));
                if let Some(old) = old.get(relative) {
                    result.insert(relative.clone(), old.clone());
                }
            }
        }
    }
    for (relative, prior) in &old {
        if !allow_removals {
            result
                .entry(relative.clone())
                .or_insert_with(|| prior.clone());
            continue;
        }
        if files.contains_key(relative) || !prior.is_object() {
            continue;
        }
        let remove = || -> Result<(), Error> {
            let path = safe_destination(paths.destination, relative, true)?;
            if path.is_symlink() {
                let link = fs::read_link(&path)?;
                if Some(link.to_string_lossy().as_ref()) != prior["target"].as_str() {
                    return Err(Error::Invalid);
                }
                crate::backup(&path, b"", backups)?;
                if fs::read_link(&path)? != link {
                    return Err(Error::Changed);
                }
                fs::remove_file(path)?;
                return Ok(());
            }
            let Some(raw) = crate::read(&path)? else {
                return Ok(());
            };
            if prior["hash"].as_str() != Some(hash(&raw).as_str()) {
                return Err(Error::Conflict);
            }
            crate::backup(&path, &raw, backups)?;
            if crate::read(&path)?.as_deref() != Some(&raw) {
                return Err(Error::Changed);
            }
            fs::remove_file(path)?;
            Ok(())
        };
        if let Err(error) = remove() {
            issues.push(issue(
                if matches!(error, Error::Conflict) {
                    "codex_account_resource_local_change"
                } else {
                    "codex_account_path_conflict"
                },
                json!({"path":relative}),
            ));
            result.insert(relative.clone(), prior.clone());
        }
    }
    Value::Object(result)
}
