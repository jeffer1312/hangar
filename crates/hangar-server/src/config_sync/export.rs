//! Leitura da origem: cada item falha isoladamente e o manifesto só contém impressões digitais.
use super::{
    bundle::{self, Blob, Bundle, Error},
    describe::describe,
    paths::{self, HEAVY_DIRS, PROGRAMS, Roots},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const OWNED_SETTINGS: [&str; 5] = [
    "hooks",
    "statusLine",
    "enabledPlugins",
    "extraKnownMarketplaces",
    "env",
];
pub const MACHINE_MCP: [&str; 2] = ["hangar", "hangar-computer-control"];
pub const ACCOUNT_SKIPPED: [&str; 6] = [
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "otelHeadersHelper",
    "permissions",
    "skipDangerousModePermissionPrompt",
];
pub const NATIVE_LOCAL: [&str; 11] = [
    "sidebar_width",
    "live_corner",
    "side_width",
    "side_browser_width",
    "terminal_height",
    "language",
    "currency",
    "hands_free",
    "skip_chat_confirmations",
    "keep_in_tray",
    "chrome_autofill",
];

#[derive(Clone)]
pub struct Context {
    pub roots: Roots,
    pub engines: PathBuf,
    pub preferences: PathBuf,
    pub aliases: PathBuf,
    pub native: PathBuf,
    pub progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
}
impl Context {
    pub fn new(roots: Roots) -> Self {
        Self {
            progress: None,
            engines: Path::new(&roots.claude).join("engines.json"),
            preferences: Path::new(&roots.claude).join("runtime-config.json"),
            aliases: Path::new(&roots.claude).join(".hangar-apelidos.json"),
            native: Path::new(&roots.home).join(if cfg!(windows) {
                "AppData/Roaming/hangar-native"
            } else {
                ".config/hangar-native"
            }),
            roots,
        }
    }
    pub fn emit(&self, event: Value) {
        if let Some(progress) = &self.progress {
            let _ = progress.send(event);
        }
    }
}

pub fn read_json(path: &Path) -> Result<Value, Error> {
    let data = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e.into()),
    };
    let value: Value = serde_json::from_slice(&data)
        .map_err(|e| Error::new("config_sync_invalid_json", e.to_string(), json!({})))?;
    if !value.is_object() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        return Err(Error::new(
            "config_sync_invalid_json",
            format!("{name} não é um objeto JSON"),
            json!({"file":name}),
        ));
    }
    Ok(value)
}
pub fn canon(value: &Value, roots: &Roots) -> Value {
    paths::map_strings(value, &|v| paths::canonicalize(v, roots))
}
pub fn uncanon(value: &Value, roots: &Roots) -> Value {
    paths::map_strings(value, &|v| paths::resolve(v, roots))
}
fn pick(value: &Value, keep: Option<&Vec<String>>, prefix: &str) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| keep.is_none_or(|keep| keep.contains(&format!("{prefix}{k}"))))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}
fn read_names(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = std::fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    Ok(paths)
}
fn optional_names(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match read_names(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        result => result,
    }
}
fn names(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}
pub fn own_skills(roots: &Roots) -> BTreeSet<String> {
    names(&Path::new(&roots.hangar).join("skills"))
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}
pub fn own_hooks(roots: &Roots) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = names(&Path::new(&roots.hangar).join("backend/hooks"))
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "py"))
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.insert("pair-context".into());
    names
}
pub fn is_own_hook(command: &str, own: &BTreeSet<String>) -> bool {
    own.iter().any(|name| {
        let pattern = format!(r#"(?:^|[\s/\\"']){}(?:$|[\s"'])"#, regex::escape(name));
        regex::Regex::new(&pattern).unwrap().is_match(command)
    })
}
pub fn split_hooks(groups: &Value, own: &BTreeSet<String>, hangar: bool) -> Value {
    Value::Array(
        groups
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|group| {
                let mut group = group.as_object()?.clone();
                let hooks: Vec<_> = group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|h| {
                        h.is_object()
                            && is_own_hook(h["command"].as_str().unwrap_or(""), own) == hangar
                    })
                    .cloned()
                    .collect();
                if hooks.is_empty() {
                    None
                } else {
                    group.insert("hooks".into(), Value::Array(hooks));
                    Some(Value::Object(group))
                }
            })
            .collect(),
    )
}
fn hook_commands(hooks: &Value) -> Vec<String> {
    hooks
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(_, groups)| groups.as_array().into_iter().flatten())
        .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
        .filter_map(|h| h["command"].as_str().map(str::to_owned))
        .collect()
}
pub fn read_blob(path: &Path, roots: &Roots) -> Result<(Blob, bool), Error> {
    let raw = std::fs::read(path)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::MetadataExt;
        let mode = path.metadata()?.mode();
        if mode & 0o044 == 0 {
            if mode & 0o111 != 0 { 0o700 } else { 0o600 }
        } else if mode & 0o111 != 0 {
            0o755
        } else {
            0o644
        }
    };
    #[cfg(not(unix))]
    let mode = 0o644;
    if !raw.contains(&0) {
        if let Ok(text) = std::str::from_utf8(&raw) {
            let text2 = paths::canonicalize(text, roots);
            if text2 != text {
                return Ok((
                    Blob {
                        data: text2.into_bytes(),
                        mode,
                    },
                    true,
                ));
            }
        }
    }
    Ok((Blob { data: raw, mode }, false))
}
fn walk(
    root: &Path,
    roots: &Roots,
    item: &str,
    entry: &str,
    bundle: &mut Bundle,
) -> Result<(BTreeMap<String, Blob>, Vec<String>), Error> {
    let mut files = BTreeMap::new();
    let mut text = Vec::new();
    if root.is_file() {
        let (blob, marked) = read_blob(root, roots)?;
        files.insert(String::new(), blob);
        if marked {
            text.push(String::new());
        }
        return Ok((files, text));
    }
    let mut pending = vec![root.to_owned()];
    let mut seen = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !seen.insert(std::fs::canonicalize(&current)?) {
            continue;
        }
        let mut dirs = Vec::new();
        for path in read_names(&current)? {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                if HEAVY_DIRS.contains(&name.as_str()) {
                    if name != "__pycache__" {
                        skipped.insert(name);
                    }
                } else {
                    dirs.push(path);
                }
            } else if !name.starts_with(".hangar") {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                match read_blob(&path, roots) {
                    Ok((blob, marked)) => {
                        files.insert(relative.clone(), blob);
                        if marked {
                            text.push(relative);
                        }
                    }
                    Err(error) => {
                        let kind = if !path.exists() {
                            "FileNotFoundError"
                        } else {
                            "PermissionError"
                        };
                        tracing::debug!(%error,"arquivo de configuração não pôde ser lido");
                        bundle.warn(
                            item,
                            "config_sync_unreadable",
                            json!({"entry":format!("{entry}/{relative}"),"error":kind}),
                        );
                    }
                }
            }
        }
        pending.extend(dirs.into_iter().rev());
    }
    for dir in skipped {
        bundle.warn(
            item,
            "config_sync_heavy_dir_skipped",
            json!({"entry":entry,"dir":dir}),
        );
    }
    Ok((files, text))
}
fn directory(
    ctx: &Context,
    item: &str,
    bundle: &mut Bundle,
    keep: Option<&Vec<String>>,
) -> Result<Value, Error> {
    let roots = &ctx.roots;
    let claude = Path::new(&roots.claude);
    let mut entries = BTreeMap::new();
    if item == "claude_instructions" {
        for p in optional_names(claude)? {
            if p.extension().is_some_and(|x| x == "md")
                && p.file_name().unwrap() != "CLAUDE.local.md"
            {
                entries.insert(p.file_name().unwrap().to_string_lossy().into_owned(), p);
            }
        }
    }
    let dirs: &[&str] = match item {
        "claude_instructions" => &["rules"],
        "claude_skills" => &["skills"],
        "claude_agents" => &["agents", "commands", "output-styles"],
        "claude_hooks" => &["hooks"],
        _ => &[],
    };
    let skills = own_skills(roots);
    let hooks = own_hooks(roots);
    for dir in dirs {
        for path in optional_names(&claude.join(dir))? {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with('.')
                || HEAVY_DIRS.contains(&name.as_str())
                || (*dir == "skills" && skills.contains(&name))
            {
                continue;
            }
            if *dir == "hooks"
                && (hooks.contains(&name)
                    || path.is_symlink()
                        && path.canonicalize().is_ok_and(|p| {
                            Path::new(&roots.hangar)
                                .canonicalize()
                                .is_ok_and(|h| p.starts_with(h))
                        }))
            {
                continue;
            }
            let entry = format!("{dir}/{name}");
            if !path.exists() {
                bundle.warn(item, "config_sync_broken_link", json!({"entry":entry}));
                continue;
            }
            entries.insert(entry, path);
        }
    }
    bundle.warnings.entry(item).or_insert_with(|| json!([]));
    let mut data = json!({"entries":{},"hashes":{},"descriptions":{}});
    for (name, path) in entries {
        if keep.is_some_and(|k| !k.contains(&name)) {
            continue;
        }
        ctx.emit(json!({"phase":"read","item":item,"entry":name}));
        let (files, text) = walk(&path, roots, item, &name, bundle)?;
        let hash = bundle::hash(&json!(
            files
                .iter()
                .map(|(rel, b)| json!([rel, bundle::sha256(&b.data), b.mode & 0o111 != 0]))
                .collect::<Vec<_>>()
        ));
        data["entries"][&name] = json!({"kind":if files.contains_key("") {"file"} else {"dir"},"files":files.keys().collect::<Vec<_>>(),"text":text,"hash":hash});
        data["hashes"][&name] = json!(hash);
        let desc = describe(&path);
        if !desc.is_empty() {
            data["descriptions"][&name] = json!(desc);
        }
        for (rel, blob) in files {
            bundle.files.insert(
                format!(
                    "files/{item}/{name}{}",
                    if rel.is_empty() {
                        String::new()
                    } else {
                        format!("/{rel}")
                    }
                ),
                blob,
            );
        }
    }
    Ok(data)
}

pub fn never_ref(marked: &str) -> bool {
    let normalized = marked.replace('\\', "/").to_lowercase();
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "." | "" => (),
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let path = parts.join("/");
    let name = parts.last().copied().unwrap_or("");
    [".credentials.json", "auth.json", "claude.local.md"].contains(&name)
        || name.starts_with(".claude.json")
        || parts.iter().any(|p| p.starts_with(".hangar"))
        || path.starts_with("⟦claude⟧/projects/")
        || [
            "⟦claude⟧/settings.json",
            "⟦codex⟧/config.toml",
            "⟦claude⟧/engines.json",
        ]
        .contains(&path.as_str())
}
fn refs(
    ctx: &Context,
    commands: &[String],
    bundle: &mut Bundle,
    wanted: impl Fn(&str) -> bool,
) -> Result<Value, Error> {
    let roots = &ctx.roots;
    let mut refs = json!({});
    for command in commands {
        for marked in paths::marked_paths(command) {
            if marked.starts_with("⟦HANGAR⟧") {
                refs[&marked] = json!({"member":""});
                continue;
            }
            let normal = marked.replace('\\', "/");
            let name = normal.rsplit('/').next().unwrap_or("");
            let name = name.strip_suffix(".exe").unwrap_or(name).to_lowercase();
            let local = PathBuf::from(paths::resolve(&marked, roots));
            if PROGRAMS.contains(&name.as_str())
                || !local.is_file()
                || local.is_symlink()
                || never_ref(&marked)
            {
                continue;
            }
            let mut candidates = BTreeMap::new();
            if normal
                .split_once('⟧')
                .map(|(_, r)| r.split('/').filter(|s| !s.is_empty()).count())
                .unwrap_or(0)
                >= 3
            {
                let mut pending = vec![local.parent().unwrap().to_owned()];
                let mut total = 0u64;
                while let Some(dir) = pending.pop() {
                    let mut dirs = Vec::new();
                    for p in read_names(&dir)? {
                        if p.is_dir() {
                            if !p.is_symlink()
                                && !HEAVY_DIRS
                                    .contains(&p.file_name().unwrap().to_string_lossy().as_ref())
                            {
                                dirs.push(p);
                            }
                        } else if p.is_file() && !p.is_symlink() {
                            total += p.metadata()?.len();
                            candidates.insert(
                                paths::canonicalize(&p.to_string_lossy(), roots).replace('\\', "/"),
                                p,
                            );
                        }
                    }
                    if total > 20 * 1024 * 1024 {
                        candidates.clear();
                        break;
                    }
                    pending.extend(dirs.into_iter().rev());
                }
            }
            if candidates.is_empty() {
                candidates.insert(marked, local);
            }
            for (reference, path) in candidates {
                if refs.get(&reference).is_some() || never_ref(&reference) || !wanted(&reference) {
                    continue;
                }
                let (blob, text) = read_blob(&path, roots)?;
                let member = format!(
                    "refs/{}",
                    sha1_smol::Sha1::from(reference.as_bytes()).digest()
                );
                bundle.files.insert(member.clone(), blob);
                refs[&reference] = json!({"member":member,"text":text});
            }
        }
    }
    Ok(refs)
}
fn add_refs(data: &mut Value, references: Value, bundle: &Bundle, roots: &Roots) {
    if data.get("descriptions").is_none() {
        data["descriptions"] = json!({});
    }
    for (reference, info) in references.as_object().unwrap() {
        let member = info["member"].as_str().unwrap_or("");
        if member.is_empty() {
            continue;
        }
        if let Some(blob) = bundle.files.get(member) {
            data["hashes"][format!("ref:{reference}")] = json!(bundle::sha256(&blob.data));
        }
        let description = describe(Path::new(&paths::resolve(reference, roots)));
        if !description.is_empty() {
            data["descriptions"][format!("ref:{reference}")] = json!(description);
        }
    }
    data["refs"] = references;
}
fn script_names(commands: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for command in commands {
        let mut paths = paths::marked_paths(command);
        if paths.is_empty() {
            paths = command.split_whitespace().map(str::to_owned).collect();
        }
        for path in paths {
            let normal = path.replace('\\', "/");
            let name = normal
                .trim_matches(['\'', '"'])
                .rsplit('/')
                .next()
                .unwrap_or("");
            if !PROGRAMS.contains(
                &name
                    .strip_suffix(".exe")
                    .unwrap_or(name)
                    .to_lowercase()
                    .as_str(),
            ) {
                if !name.is_empty() && !names.contains(&name.to_owned()) {
                    names.push(name.to_owned());
                }
                break;
            }
        }
    }
    names
}

fn export_item(
    ctx: &Context,
    item: &str,
    bundle: &mut Bundle,
    keep: Option<&Vec<String>>,
) -> Result<Value, Error> {
    let roots = &ctx.roots;
    let claude = Path::new(&roots.claude);
    let selected = |key: &str| keep.is_none_or(|k| k.iter().any(|v| v == key));
    match item {
        "claude_instructions" | "claude_skills" | "claude_agents" => {
            directory(ctx, item, bundle, keep)
        }
        "claude_hooks" => {
            let mut data = directory(ctx, item, bundle, keep)?;
            let settings = read_json(&claude.join("settings.json"))?;
            let own = own_hooks(roots);
            let mut every = json!({});
            for (event, groups) in settings["hooks"].as_object().into_iter().flatten() {
                let mine = split_hooks(groups, &own, false);
                if mine.as_array().is_some_and(|a| !a.is_empty()) {
                    every[event] = canon(&mine, roots);
                }
            }
            let hooks = pick(&every, keep, "hooks:");
            data["hooks"] = hooks.clone();
            data["labels"] = json!({});
            for (event, groups) in hooks.as_object().unwrap() {
                data["hashes"][format!("hooks:{event}")] = json!(bundle::hash(groups));
                data["labels"][format!("hooks:{event}")] =
                    json!(script_names(&hook_commands(&json!({event:groups}))));
            }
            let mut commands = hook_commands(&hooks);
            let mut all = hook_commands(&every);
            if settings["statusLine"].is_object() {
                let status = canon(&settings["statusLine"], roots);
                let command = status["command"].as_str().unwrap_or("").to_owned();
                all.push(command.clone());
                if selected("statusLine") {
                    data["statusLine"] = status.clone();
                    data["hashes"]["statusLine"] = json!(bundle::hash(&status));
                    data["labels"]["statusLine"] = json!(script_names(&[command.clone()]));
                    commands.push(command);
                }
            }
            let mut references = refs(ctx, &commands, bundle, |_| true)?;
            if keep.is_some() {
                let alone = refs(ctx, &all, bundle, |r| {
                    selected(&format!("ref:{r}")) && references.get(r).is_none()
                })?;
                for (r, v) in alone.as_object().unwrap() {
                    if v["member"] != "" {
                        references[r] = v.clone();
                    }
                }
            }
            add_refs(&mut data, references, bundle, roots);
            Ok(data)
        }
        "claude_env" | "claude_settings" | "engines" | "hangar_prefs" => {
            let (key, mut values) = match item {
                "engines" => ("engines", read_json(&ctx.engines)?),
                "hangar_prefs" => (
                    "prefs",
                    read_json(&ctx.preferences).unwrap_or_else(|_| json!({})),
                ),
                "claude_env" => (
                    "env",
                    read_json(&claude.join("settings.json"))?["env"].clone(),
                ),
                _ => ("settings", read_json(&claude.join("settings.json"))?),
            };
            if let Some(values) = values.as_object_mut() {
                values.retain(|k, _| {
                    if item == "claude_settings" {
                        !OWNED_SETTINGS.contains(&k.as_str())
                    } else if item == "hangar_prefs" {
                        super::preferences::editable(k)
                            && !["sync", "term_origins", "scan_roots"].contains(&k.as_str())
                    } else {
                        true
                    }
                });
            }
            let values = canon(&pick(&values, keep, ""), roots);
            Ok(json!({key:values,"hashes":bundle::hashes(&values,"")}))
        }
        "claude_mcp" => {
            let raw = read_json(&Path::new(&roots.home).join(".claude.json"))?;
            let mut all = pick(&raw["mcpServers"], None, "");
            all.as_object_mut()
                .unwrap()
                .retain(|k, _| !MACHINE_MCP.contains(&k.as_str()));
            let all = canon(&all, roots);
            let servers = pick(&all, keep, "");
            let commands = |v: &Value| {
                v.as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(_, s)| s.is_object())
                    .map(|(_, s)| {
                        let mut c = vec![s["command"].as_str().unwrap_or("").to_owned()];
                        c.extend(
                            s["args"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_owned),
                        );
                        c.join(" ")
                    })
                    .collect::<Vec<_>>()
            };
            let mut references = refs(ctx, &commands(&servers), bundle, |_| true)?;
            if keep.is_some() {
                let alone = refs(ctx, &commands(&all), bundle, |r| {
                    selected(&format!("ref:{r}")) && references.get(r).is_none()
                })?;
                for (r, v) in alone.as_object().unwrap() {
                    if v["member"] != "" {
                        references[r] = v.clone();
                    }
                }
            }
            let mut data = json!({"servers":servers,"hashes":bundle::hashes(&servers,"")});
            add_refs(&mut data, references, bundle, roots);
            Ok(data)
        }
        "claude_plugins" => plugins(ctx, bundle, keep),
        "codex" => codex(ctx, bundle, keep),
        "claude_accounts" => accounts(ctx, bundle, keep),
        "hangar_appearance" => appearance(ctx, bundle, keep),
        _ => Err(Error::new(
            "config_sync_unknown_item",
            format!("item desconhecido: {item}"),
            json!({"items":item}),
        )),
    }
}

pub fn export(
    ctx: &Context,
    items: &[String],
    keep: Option<&serde_json::Map<String, Value>>,
    limit: bool,
) -> Result<Bundle, Error> {
    let mut bundle = Bundle::default();
    for (index, item) in items.iter().enumerate() {
        ctx.emit(
            json!({"phase":"read","item":item,"entry":null,"index":index,"total":items.len()}),
        );
        let keep = keep
            .and_then(|k| k.get(item))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
        match export_item(ctx, item, &mut bundle, keep.as_ref()) {
            Ok(data) => {
                bundle.items.insert(item.clone(), data);
            }
            Err(error) => {
                bundle
                    .files
                    .retain(|m, _| !m.starts_with(&format!("files/{item}/")));
                bundle.warn(item,"config_sync_item_failed",json!({"item":item,"error":error.message.chars().take(300).collect::<String>()}));
            }
        }
    }
    if limit && bundle.files.values().map(|b| b.data.len()).sum::<usize>() > bundle::MAX_BUNDLE {
        let mut sizes: Vec<_> = bundle
            .items
            .keys()
            .map(|i| (i, bundle.item_bytes(i)))
            .collect();
        sizes.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
        sizes.truncate(3);
        return Err(Error::new(
            "config_sync_bundle_too_big",
            format!(
                "pacote maior que o teto: {}",
                sizes
                    .iter()
                    .map(|(i, n)| format!("{i} ({n} bytes)"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            json!({"largest":sizes.iter().map(|(i,n)|format!("{i} ({} MB)",n/(1024*1024))).collect::<Vec<_>>().join(", ")}),
        ));
    }
    Ok(bundle)
}

fn plugins(ctx: &Context, bundle: &mut Bundle, keep: Option<&Vec<String>>) -> Result<Value, Error> {
    let roots = &ctx.roots;
    let claude = Path::new(&roots.claude);
    let settings = read_json(&claude.join("settings.json"))?;
    bundle
        .warnings
        .entry("claude_plugins")
        .or_insert_with(|| json!([]));
    let enabled = pick(&settings["enabledPlugins"], keep, "plugin:");
    let mut extra = pick(&settings["extraKnownMarketplaces"], None, "");
    let needed: BTreeSet<_> = enabled
        .as_object()
        .unwrap()
        .keys()
        .filter_map(|p| p.split_once('@').map(|(_, m)| m.to_owned()))
        .collect();
    if let Some(keep) = keep {
        extra
            .as_object_mut()
            .unwrap()
            .retain(|k, _| needed.contains(k) || keep.contains(&format!("marketplace:{k}")));
    }
    let known = read_json(&claude.join("plugins/known_marketplaces.json"))?;
    let mut sources = json!({});
    let mut all = needed;
    all.extend(extra.as_object().unwrap().keys().cloned());
    for name in all {
        let source = if known[&name]["source"].is_object() {
            &known[&name]["source"]
        } else {
            &extra[&name]["source"]
        };
        if source.is_object() {
            sources[&name] = canon(source, roots);
        } else {
            bundle.warn(
                "claude_plugins",
                "config_sync_marketplace_unknown",
                json!({"marketplace":name}),
            );
        }
    }
    let mut hashes = bundle::hashes(&enabled, "plugin:");
    hashes.as_object_mut().unwrap().extend(
        bundle::hashes(&sources, "marketplace:")
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut descriptions = json!({});
    for folder in names(&claude.join("plugins/marketplaces")) {
        let Ok(manifest) = read_json(&folder.join(".claude-plugin/marketplace.json")) else {
            continue;
        };
        let fallback = folder.file_name().unwrap_or_default().to_string_lossy();
        let market = manifest["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&fallback);
        for plugin in manifest["plugins"].as_array().into_iter().flatten() {
            if let (Some(name), Some(description)) =
                (plugin["name"].as_str(), plugin["description"].as_str())
            {
                let key = format!("plugin:{name}@{market}");
                if hashes.get(&key).is_some() && !description.is_empty() {
                    descriptions[&key] = json!(super::describe::short(description));
                }
            }
        }
    }
    Ok(
        json!({"enabledPlugins":enabled,"extraKnownMarketplaces":canon(&extra,roots),"sources":sources,"hashes":hashes,"descriptions":descriptions}),
    )
}

fn codex(ctx: &Context, bundle: &mut Bundle, keep: Option<&Vec<String>>) -> Result<Value, Error> {
    const PREFS: &[&str] = &[
        "model",
        "model_reasoning_effort",
        "model_reasoning_summary",
        "model_verbosity",
        "service_tier",
        "model_context_window",
        "model_auto_compact_token_limit",
        "model_catalog_json",
        "personality",
        "features",
        "tui",
        "desktop",
        "project_doc_fallback_filenames",
        "project_doc_max_bytes",
        "model_instructions_file",
        "agents",
        "mcp_servers",
        "shell_environment_policy",
        "model_providers",
        "approval_policy",
        "approvals_reviewer",
        "sandbox_mode",
        "sandbox_workspace_write",
        "permissions",
        "auto_review",
        "web_search",
        "notify",
        "otel",
        "analytics",
    ];
    let roots = &ctx.roots;
    let codex = Path::new(&roots.codex);
    let mut data = json!({"hashes":{}});
    bundle.warnings.entry("codex").or_insert_with(|| json!([]));
    let agents = codex.join("AGENTS.md");
    if agents.is_file() && keep.is_none_or(|k| k.iter().any(|s| s == "AGENTS.md")) {
        let (blob, text) = read_blob(&agents, roots)?;
        data["agents_md"] = json!({"member":"files/codex/AGENTS.md","text":text});
        data["hashes"]["AGENTS.md"] = json!(bundle::sha256(&blob.data));
        bundle.files.insert("files/codex/AGENTS.md".into(), blob);
    }
    let path = codex.join("config.toml");
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let config: toml::Value = toml::from_str(&source)
        .map_err(|e| Error::new("config_sync_invalid_toml", e.to_string(), json!({})))?;
    fn has_date(v: &toml::Value) -> bool {
        match v {
            toml::Value::Datetime(_) => true,
            toml::Value::Array(a) => a.iter().any(has_date),
            toml::Value::Table(t) => t.values().any(has_date),
            _ => false,
        }
    }
    let mut prefs = json!({});
    for (key, value) in config.as_table().into_iter().flatten() {
        if !PREFS.contains(&key.as_str()) {
            continue;
        }
        if has_date(value) {
            bundle.warn("codex", "config_sync_unsupported_value", json!({"key":key}));
            continue;
        }
        let mut value = serde_json::to_value(value)
            .map_err(|e| Error::new("config_sync_invalid_toml", e.to_string(), json!({})))?;
        if key == "mcp_servers" {
            if let Some(servers) = value.as_object_mut() {
                servers.retain(|k, _| !MACHINE_MCP.contains(&k.as_str()));
            }
        }
        prefs[key] = value;
    }
    let config = canon(&pick(&prefs, keep, "config:"), roots);
    data["hashes"].as_object_mut().unwrap().extend(
        bundle::hashes(&config, "config:")
            .as_object()
            .unwrap()
            .clone(),
    );
    data["config"] = config;
    Ok(data)
}

fn accounts(
    ctx: &Context,
    bundle: &mut Bundle,
    keep: Option<&Vec<String>>,
) -> Result<Value, Error> {
    let roots = &ctx.roots;
    let principal = read_json(&Path::new(&roots.claude).join("settings.json"))?;
    let aliases = read_json(&ctx.aliases).unwrap_or_else(|_| json!({}));
    let mut accounts = json!({});
    for path in names(Path::new(&roots.home)) {
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let Some(name) = filename.strip_prefix(".claude-") else {
            continue;
        };
        if !crate::accounts::catalog::valid_name(name)
            || !crate::accounts::claude_config::is_account(&path)
            || keep.is_some_and(|k| !k.iter().any(|s| s == name))
        {
            continue;
        }
        let own = match read_json(&path.join("settings.json")) {
            Ok(v) => v,
            Err(error) => {
                bundle.warn("claude_accounts","config_sync_unreadable",json!({"entry":format!("{name}/settings.json"),"error":if error.code=="config_sync_invalid_json"{"BundleError"}else{"OSError"}}));
                continue;
            }
        };
        let own: serde_json::Map<_, _> = own
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| {
                principal.get(*k).is_none()
                    && !OWNED_SETTINGS.contains(&k.as_str())
                    && !ACCOUNT_SKIPPED.contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut entry = json!({"settings":canon(&Value::Object(own),roots)});
        let id = format!(
            "claude:{}",
            crate::list::links::without_verbatim(path.canonicalize()?).display()
        );
        if let Some(alias) = aliases[&id].as_str().filter(|v| !v.is_empty()) {
            entry["alias"] = json!(alias);
        }
        accounts[name] = entry;
    }
    Ok(json!({"accounts":accounts,"hashes":bundle::hashes(&accounts,"")}))
}

pub fn background_name(name: &str) -> String {
    if name.chars().count() <= 255 && !name.contains('\n') {
        name.to_owned()
    } else {
        String::new()
    }
}
fn appearance(
    ctx: &Context,
    bundle: &mut Bundle,
    keep: Option<&Vec<String>>,
) -> Result<Value, Error> {
    let mut data = json!({"hashes":{}});
    let mut native = read_json(&ctx.native.join("appearance.json"))?;
    native
        .as_object_mut()
        .unwrap()
        .retain(|k, _| !NATIVE_LOCAL.contains(&k.as_str()));
    if !native.as_object().unwrap().is_empty()
        && keep.is_none_or(|k| k.iter().any(|s| s == "appearance.json"))
    {
        data["hashes"]["appearance.json"] = json!(bundle::hash(&native));
        data["native"] = native;
    }
    let path = ctx.native.join("background-image");
    if path.is_file() && keep.is_none_or(|k| k.iter().any(|s| s == "background-image")) {
        let raw = std::fs::read(path)?;
        let name_path = ctx.native.join("background-name");
        let name = if name_path.is_file() {
            background_name(&String::from_utf8_lossy(&std::fs::read(name_path)?))
        } else {
            String::new()
        };
        data["hashes"]["background-image"] =
            json!(bundle::hash(&json!([bundle::sha256(&raw), name])));
        data["background"] = json!({"name":name});
        bundle.files.insert(
            "files/hangar_appearance/background-image".into(),
            Blob {
                data: raw,
                mode: 0o644,
            },
        );
    }
    Ok(data)
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn listing_failure_is_not_an_empty_directory() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("not-a-directory");
        std::fs::write(&file, b"content").unwrap();
        assert!(read_names(&file).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn referenced_parent_does_not_export_secret_file_symlinks() {
        let home = tempfile::tempdir().unwrap();
        let scripts = home.path().join("scripts/package");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("tool.sh"), b"echo safe").unwrap();
        std::fs::write(home.path().join("credentials.json"), b"fixture-secret").unwrap();
        std::os::unix::fs::symlink(
            home.path().join("credentials.json"),
            scripts.join("innocent.txt"),
        )
        .unwrap();
        let roots = Roots {
            home: home.path().display().to_string(),
            claude: home.path().join(".claude").display().to_string(),
            codex: home.path().join(".codex").display().to_string(),
            hangar: home.path().join("checkout").display().to_string(),
        };
        let context = Context::new(roots);
        let mut bundle = Bundle::default();
        let refs = refs(
            &context,
            &["sh ⟦HOME⟧/scripts/package/tool.sh".into()],
            &mut bundle,
            |_| true,
        )
        .unwrap();
        assert!(refs.get("⟦HOME⟧/scripts/package/tool.sh").is_some());
        assert!(refs.get("⟦HOME⟧/scripts/package/innocent.txt").is_none());
        assert!(
            bundle
                .files
                .values()
                .all(|blob| blob.data != b"fixture-secret")
        );
    }
}
