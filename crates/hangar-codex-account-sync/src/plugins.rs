//! Replica apenas os plugins gerenciados pela conta de origem.
use crate::{Paths, hash, issue};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use crate::ConfigPlan;
pub use futures_util::future::BoxFuture;
/// Trecho do stderr do Codex quando o marketplace já existe com outra origem.
pub const ORPHAN_MARKETPLACE: &str = "is already added from a different source";

#[derive(Debug, Clone)]
pub struct NativeError {
    pub kind: String,
    pub stderr: String,
}
pub struct Inventory {
    pub plugins: Vec<Value>,
    pub marketplaces: Vec<Value>,
}

/// A implementação isola cada comando na conta indicada e conserva o escritor nativo do TOML.
pub trait PluginsNative: Sync {
    fn inventory<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Inventory, NativeError>>;
    fn plugins<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Vec<Value>, NativeError>>;
    fn marketplaces<'a>(
        &'a self,
        account: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Value>, NativeError>>;
    fn cli<'a>(
        &'a self,
        account: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, Result<Value, NativeError>>;
    fn install<'a>(
        &'a self,
        account: &'a Path,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Value, NativeError>>;
    fn parse_toml(&self, text: &str) -> Result<Value, String>;
    fn edit_config<'a>(
        &'a self,
        path: &'a Path,
        backups: &'a Path,
        work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn adapt_hooks<'a>(
        &'a self,
        account: &'a Path,
        root: &'a Path,
        backups: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn trust<'a>(
        &'a self,
        account: &'a Path,
        approve: bool,
    ) -> BoxFuture<'a, Result<Option<bool>, String>>;
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}
fn bundled_path(raw: &str) -> bool {
    let parts: Vec<_> = Path::new(raw).components().map(|p| p.as_os_str()).collect();
    parts.contains(&std::ffi::OsStr::new(".tmp"))
        && parts.contains(&std::ffi::OsStr::new("bundled-marketplaces"))
}
fn bundled_name(raw: &str, item: &Value) -> Option<String> {
    for field in ["marketplace", "marketplaceName", "name"] {
        if let Some(value) = text(item, field) {
            return Some(value.into());
        }
    }
    let mut parts = Path::new(raw).components();
    parts.find(|p| p.as_os_str() == "bundled-marketplaces")?;
    parts
        .next()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
}
fn normalized_manifest(previous: &Value) -> Value {
    let previous = previous.get("manifest").unwrap_or(previous);
    let mut result = json!({"marketplaces":{},"plugins":{}});
    for field in ["marketplaces", "plugins"] {
        let Some(items) = previous.get(field).and_then(Value::as_object) else {
            continue;
        };
        let mut items = items.clone();
        for item in items.values_mut() {
            if !item.is_object() {
                continue;
            }
            if let Some(raw) = item
                .pointer("/marketplaceSource/source")
                .and_then(Value::as_str)
                .filter(|p| bundled_path(p))
            {
                item["marketplaceSource"] =
                    json!({"sourceType":"bundled","name":bundled_name(raw,item)});
            }
            if item.pointer("/origem/type").and_then(Value::as_str) == Some("local") {
                if let Some(raw) = item
                    .pointer("/origem/source")
                    .and_then(Value::as_str)
                    .filter(|p| bundled_path(p))
                {
                    item["origem"] = json!({"type":"bundled","source":bundled_name(raw,item)});
                }
            }
            if text(item, "root").is_some_and(bundled_path) {
                item.as_object_mut().unwrap().remove("root");
            }
        }
        result[field] = Value::Object(items);
    }
    result
}

fn resolved(path: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = std::fs::canonicalize(path) {
        return Ok(hangar_core::fs::without_verbatim(path));
    }
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut tail = Vec::new();
    let mut parent = absolute.as_path();
    while !parent.exists() {
        let name = parent.file_name().ok_or("Caminho inválido")?;
        tail.push(name.to_owned());
        parent = parent.parent().ok_or("Caminho inválido")?;
    }
    let mut result = hangar_core::fs::without_verbatim(
        std::fs::canonicalize(parent).map_err(|e| e.to_string())?,
    );
    for name in tail.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}
fn local_identity(path: PathBuf) -> String {
    let path = hangar_core::fs::without_verbatim(path)
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) {
        path.replace('/', "\\").to_lowercase()
    } else {
        path
    }
}
fn origin(value: &Value, home: &Path) -> Option<(String, String)> {
    let source = value
        .get("marketplaceSource")
        .filter(|v| v.is_object())
        .unwrap_or(value);
    let kind = text(source, "sourceType").or_else(|| text(source, "source_type"))?;
    if kind == "bundled" {
        return text(source, "name")
            .or_else(|| text(value, "marketplaceName"))
            .or_else(|| text(value, "name"))
            .map(|name| ("bundled".into(), name.into()));
    }
    let raw = text(source, "source")?;
    if bundled_path(raw) {
        return Some((
            "bundled".into(),
            text(value, "marketplaceName")
                .or_else(|| text(value, "name"))
                .unwrap_or("")
                .into(),
        ));
    }
    if matches!(kind, "local" | "directory") {
        let path = if raw == "~" {
            home.to_owned()
        } else if let Some(suffix) = raw.strip_prefix("~/") {
            home.join(suffix)
        } else {
            PathBuf::from(raw)
        };
        return Some(("local".into(), local_identity(resolved(&path).ok()?)));
    }
    if matches!(kind, "git" | "github") {
        let pair = if let Some(raw) = raw.strip_prefix("git@") {
            raw.split_once(':')
                .map(|(h, p)| (h.to_owned(), p.to_owned()))
        } else {
            reqwest::Url::parse(raw).ok().and_then(|url| {
                Some((
                    url.host_str()?.to_owned(),
                    url.path().trim_start_matches('/').to_owned(),
                ))
            })
        };
        if let Some((host, repo)) = pair.filter(|(h, p)| !h.is_empty() && !p.is_empty()) {
            return Some((
                "git".into(),
                format!(
                    "{}/{}",
                    host.to_lowercase(),
                    repo.trim_end_matches('/').trim_end_matches(".git")
                ),
            ));
        }
    }
    Some((kind.into(), raw.into()))
}
fn plugin_origin(value: &Value, home: &Path) -> Option<(String, String)> {
    origin(value, home).or_else(|| {
        let source = value.get("source")?;
        (text(source, "source") == Some("remote"))
            .then(|| text(source, "id").map(|id| ("remote".into(), id.into())))
            .flatten()
    })
}
fn old_origin(value: &Value, home: &Path) -> Option<(String, String)> {
    if let (Some(kind), Some(raw)) = (
        value.pointer("/origem/type").and_then(Value::as_str),
        value.pointer("/origem/source").and_then(Value::as_str),
    ) {
        if kind == "local" && bundled_path(raw) {
            return bundled_name(raw, value).map(|n| ("bundled".into(), n));
        }
        if kind == "local" {
            return Some((kind.into(), local_identity(PathBuf::from(raw))));
        }
        return Some((kind.into(), raw.into()));
    }
    origin(value, home)
}
fn market_name<'a>(id: &'a str, value: &'a Value) -> &'a str {
    text(value, "marketplaceName")
        .unwrap_or_else(|| id.rsplit_once('@').map(|(_, m)| m).unwrap_or("unknown"))
}
fn plugin_name<'a>(id: &'a str, value: &'a Value) -> &'a str {
    text(value, "name").unwrap_or_else(|| id.split_once('@').map(|(n, _)| n).unwrap_or(id))
}
fn plugin_map(items: Vec<Value>) -> Result<BTreeMap<String, Value>, NativeError> {
    let mut result = BTreeMap::new();
    for item in items {
        let id = text(&item, "pluginId").filter(|id| id.contains('@'));
        if id.is_none() || text(&item, "version").is_none() || !item["enabled"].is_boolean() {
            return Err(invalid_inventory());
        }
        if result.insert(id.unwrap().into(), item).is_some() {
            return Err(invalid_inventory());
        }
    }
    Ok(result)
}
fn marketplace_map(items: Vec<Value>) -> Result<BTreeMap<String, Value>, NativeError> {
    let mut result = BTreeMap::new();
    for item in items {
        let name = text(&item, "name")
            .ok_or_else(invalid_inventory)?
            .to_owned();
        if result.insert(name, item).is_some() {
            return Err(invalid_inventory());
        }
    }
    Ok(result)
}
fn invalid_inventory() -> NativeError {
    NativeError {
        kind: "ValueError".into(),
        stderr: String::new(),
    }
}
fn config(native: &impl PluginsNative, account: &Path) -> Result<Value, String> {
    let path = account.join("config.toml");
    if !path.is_file() {
        return Ok(json!({}));
    }
    native.parse_toml(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
}
fn private_dir(paths: &Paths<'_>) -> PathBuf {
    crate::storage::private(*paths, false).unwrap_or_else(|_| {
        let digest = hash(paths.destination.to_string_lossy().as_bytes());
        paths.home.join(".hangar/codex-contas").join(digest)
    })
}
fn cache_path(account: &Path, item: &Value) -> PathBuf {
    if let Some(raw) = item.pointer("/source/path").and_then(Value::as_str) {
        if let (Ok(path), Ok(cache)) = (
            resolved(Path::new(raw)),
            resolved(&account.join("plugins/cache")),
        ) {
            if path.starts_with(cache) {
                return PathBuf::from(raw);
            }
        }
    }
    let id = text(item, "pluginId").unwrap_or("plugin@unknown");
    account
        .join("plugins/cache")
        .join(market_name(id, item))
        .join(plugin_name(id, item))
        .join(text(item, "version").unwrap_or(""))
}
fn bundled(item: &Value) -> bool {
    text(item, "root").is_some_and(bundled_path)
        || item
            .pointer("/marketplaceSource/source")
            .and_then(Value::as_str)
            .is_some_and(bundled_path)
}
fn market_manifest(item: &Value) -> Value {
    if bundled(item) {
        json!({"name":item["name"],"marketplaceSource":{"sourceType":"bundled","name":item["name"]}})
    } else {
        json!({"name":item["name"],"marketplaceSource":item.get("marketplaceSource").filter(|v|v.is_object()).cloned().unwrap_or(json!({})),"root":item.get("root").filter(|v|v.is_string()).cloned().unwrap_or(Value::Null)})
    }
}
fn edit(key: &[&str], value: Value) -> Value {
    json!({"keyPath":key.iter().map(|part|serde_json::to_string(part).unwrap()).collect::<Vec<_>>().join("."),"value":value,"mergeStrategy":"replace"})
}
async fn write_enabled(
    paths: &Paths<'_>,
    choices: &BTreeMap<String, bool>,
    native: &impl PluginsNative,
    issues: &mut Vec<Value>,
) {
    if choices.is_empty() {
        return;
    }
    let private = private_dir(paths);
    let mut plan = |current: &Value| {
        let empty = json!({});
        let plugins = current
            .get("plugins")
            .unwrap_or(&empty)
            .as_object()
            .ok_or("Plugins inválidos")?;
        Ok(choices
            .iter()
            .filter(|(id, enabled)| {
                plugins
                    .get(*id)
                    .and_then(|p| p.get("enabled"))
                    .and_then(Value::as_bool)
                    != Some(**enabled)
            })
            .map(|(id, enabled)| edit(&["plugins", id, "enabled"], json!(enabled)))
            .collect())
    };
    if native
        .edit_config(
            &paths.destination.join("config.toml"),
            &private.join("backups"),
            &private,
            &mut plan,
        )
        .await
        .is_err()
    {
        issues.push(issue(
            "codex_account_plugin_config_failed",
            json!({"error":"RuntimeError"}),
        ));
    }
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    if source.is_symlink() {
        return Err("Link no marketplace embutido".into());
    }
    if source.is_file() {
        std::fs::create_dir_all(target.parent().ok_or("Destino inválido")?)
            .map_err(|e| e.to_string())?;
        std::fs::copy(source, target).map_err(|e| e.to_string())?;
    } else {
        std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
        for item in std::fs::read_dir(source).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            copy_tree(&item.path(), &target.join(item.file_name()))?;
        }
    }
    Ok(())
}
fn verify_tree(source: &Path) -> Result<(), String> {
    if source.is_symlink() {
        return Err("Link no marketplace embutido".into());
    }
    if source.is_dir() {
        for item in std::fs::read_dir(source).map_err(|e| e.to_string())? {
            verify_tree(&item.map_err(|e| e.to_string())?.path())?;
        }
    }
    Ok(())
}
async fn materialize(
    paths: &Paths<'_>,
    entry: &Value,
    plugin: &Value,
    native: &impl PluginsNative,
    issues: &mut Vec<Value>,
) -> bool {
    let name = text(entry, "name").unwrap_or("?");
    let raw = entry.get("root").and_then(Value::as_str).or_else(|| {
        entry
            .pointer("/marketplaceSource/source")
            .and_then(Value::as_str)
    });
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || raw.is_none()
    {
        issues.push(issue(
            "codex_account_plugin_marketplace_invalid",
            json!({"marketplace":name}),
        ));
        return false;
    }
    let target_root = paths
        .destination
        .join(".tmp/bundled-marketplaces")
        .join(name);
    let copy = || -> Result<(), String> {
        let source_root = std::fs::canonicalize(raw.unwrap()).map_err(|e| e.to_string())?;
        let allowed = std::fs::canonicalize(paths.source.join(".tmp/bundled-marketplaces"))
            .map_err(|e| e.to_string())?;
        let plugin_name = plugin_name(text(plugin, "pluginId").unwrap_or("plugin@unknown"), plugin);
        let source_manifest = source_root.join(".agents/plugins/marketplace.json");
        let source_plugin = source_root.join("plugins").join(plugin_name);
        if !source_root.is_dir()
            || !source_root.starts_with(allowed)
            || !source_manifest.is_file()
            || !source_plugin.is_dir()
        {
            return Err("Marketplace inválido".into());
        }
        verify_tree(&source_manifest)?;
        verify_tree(&source_plugin)?;
        let target_manifest = target_root.join(".agents/plugins/marketplace.json");
        if std::fs::read(&target_manifest).ok()
            != Some(std::fs::read(&source_manifest).map_err(|e| e.to_string())?)
        {
            copy_tree(&source_manifest, &target_manifest)?;
        }
        let target_plugin = target_root.join("plugins").join(plugin_name);
        let source_version = std::fs::read(source_plugin.join(".codex-plugin/plugin.json"))
            .map_err(|e| e.to_string())?;
        if std::fs::read(target_plugin.join(".codex-plugin/plugin.json")).ok()
            != Some(source_version)
        {
            copy_tree(&source_plugin, &target_plugin)?;
        }
        Ok(())
    };
    if copy().is_err() {
        issues.push(issue(
            "codex_account_plugin_marketplace_unavailable",
            json!({"marketplace":name}),
        ));
        return false;
    }
    let private = private_dir(paths);
    let mut plan = |current: &Value| {
        let empty = json!({});
        let markets = current
            .get("marketplaces")
            .unwrap_or(&empty)
            .as_object()
            .ok_or("Marketplaces inválidos")?;
        let desired = json!({"source_type":"local","source":target_root});
        if markets.get(name) == Some(&desired) {
            return Ok(Vec::new());
        }
        Ok(vec![
            edit(&["marketplaces", name, "source_type"], json!("local")),
            edit(&["marketplaces", name, "source"], json!(target_root)),
        ])
    };
    if native
        .edit_config(
            &paths.destination.join("config.toml"),
            &private.join("backups"),
            &private,
            &mut plan,
        )
        .await
        .is_err()
    {
        issues.push(issue(
            "codex_account_plugin_config_failed",
            json!({"error":"RuntimeError"}),
        ));
        return false;
    }
    true
}

async fn ensure_marketplace(
    paths: &Paths<'_>,
    entry: &Value,
    plugin: &Value,
    target_plugin: Option<&Value>,
    target: &mut BTreeMap<String, Value>,
    source_config: &Value,
    native: &impl PluginsNative,
    issues: &mut Vec<Value>,
) -> bool {
    let name = text(entry, "name").unwrap_or("?");
    let Some(expected) = origin(entry, paths.home) else {
        issues.push(issue(
            "codex_account_plugin_marketplace_invalid",
            json!({"marketplace":name}),
        ));
        return false;
    };
    if bundled(entry) {
        if target
            .get(name)
            .is_some_and(|v| origin(v, paths.home).as_ref() == Some(&expected))
            && target_plugin.is_some_and(|p| p["version"] == plugin["version"])
        {
            return true;
        }
        if !materialize(paths, entry, plugin, native, issues).await {
            return false;
        }
        if let Some(current) = target.get(name) {
            if origin(current, paths.home).as_ref() != Some(&expected) {
                issues.push(issue(
                    "codex_account_plugin_origin_conflict",
                    json!({"marketplace":name}),
                ));
                return false;
            }
            return true;
        }
        match native
            .marketplaces(paths.destination)
            .await
            .and_then(marketplace_map)
        {
            Ok(refreshed) => target.extend(refreshed),
            Err(_) => {
                issues.push(issue(
                    "codex_account_plugin_marketplace_unavailable",
                    json!({"marketplace":name}),
                ));
                return false;
            }
        }
        if !target
            .get(name)
            .is_some_and(|v| origin(v, paths.home).as_ref() == Some(&expected))
        {
            issues.push(issue(
                "codex_account_plugin_marketplace_unavailable",
                json!({"marketplace":name}),
            ));
            return false;
        }
        return true;
    }
    if let Some(current) = target.get(name) {
        if origin(current, paths.home).as_ref() != Some(&expected) {
            issues.push(issue(
                "codex_account_plugin_origin_conflict",
                json!({"marketplace":name}),
            ));
            return false;
        }
        return true;
    }
    let source = entry
        .pointer("/marketplaceSource/source")
        .and_then(Value::as_str)
        .unwrap_or("");
    if source.is_empty() || (expected.0 == "local" && !Path::new(source).is_dir()) {
        issues.push(issue(
            "codex_account_plugin_marketplace_unavailable",
            json!({"marketplace":name}),
        ));
        return false;
    }
    let mut args: Vec<String> = ["plugin", "marketplace", "add", source]
        .map(String::from)
        .to_vec();
    if let Some(sparse) = source_config
        .get("marketplaces")
        .and_then(|m| m.get(name))
        .and_then(|m| m.get("sparse_paths"))
        .and_then(Value::as_array)
    {
        for path in sparse
            .iter()
            .filter_map(Value::as_str)
            .filter(|p| !p.is_empty())
        {
            args.extend(["--sparse".into(), path.into()]);
        }
    }
    args.push("--json".into());
    let mut added = native.cli(paths.destination, &args).await;
    if added.as_ref().is_err_and(|e| {
        e.stderr
            .contains(ORPHAN_MARKETPLACE)
    }) {
        let undeclared = config(native, paths.destination)
            .ok()
            .and_then(|v| v.get("marketplaces").cloned().or(Some(json!({}))))
            .and_then(|v| v.as_object().map(|m| !m.contains_key(name)))
            .unwrap_or(false);
        if undeclared {
            let path = paths.destination.join("config.toml");
            let backup = (|| -> Result<(), String> {
                if path.is_file() {
                    let raw = std::fs::read(&path).map_err(|e| e.to_string())?;
                    let directory = private_dir(paths).join("backups");
                    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                    let digest = hash(&raw);
                    std::fs::write(directory.join(format!("config.toml.{digest}.bak")), raw)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            })();
            if backup.is_ok() {
                let remove = ["plugin", "marketplace", "remove", name, "--json"].map(String::from);
                added = match native.cli(paths.destination, &remove).await {
                    Ok(_) => native.cli(paths.destination, &args).await,
                    Err(error) => Err(error),
                };
            }
        }
    }
    if added.is_err() {
        issues.push(issue(
            "codex_account_plugin_marketplace_add_failed",
            json!({"marketplace":name}),
        ));
        return false;
    }
    match native
        .marketplaces(paths.destination)
        .await
        .and_then(marketplace_map)
    {
        Ok(current)
            if current
                .get(name)
                .is_some_and(|v| origin(v, paths.home).as_ref() == Some(&expected)) =>
        {
            target.extend(current);
            true
        }
        Ok(_) => {
            issues.push(issue(
                "codex_account_plugin_marketplace_unavailable",
                json!({"marketplace":name}),
            ));
            false
        }
        Err(_) => {
            issues.push(issue(
                "codex_account_plugin_marketplace_add_failed",
                json!({"marketplace":name}),
            ));
            false
        }
    }
}

/// O inventário é relido depois das instalações e das habilitações; sucesso da CLI não prova o efeito.
pub async fn sync(paths: Paths<'_>, previous: &Value, native: &impl PluginsNative) -> Value {
    let manifest = normalized_manifest(previous);
    if resolved(paths.source).ok() == resolved(paths.destination).ok() {
        return json!({"manifest":manifest,"issues":[issue("codex_account_source_destination_conflict",json!({}))],"trust_pending":false});
    }
    let (source_inventory, target_inventory) = futures_util::join!(
        native.inventory(paths.source),
        native.inventory(paths.destination)
    );
    let inventory = (|| {
        let source = source_inventory?;
        let target = target_inventory?;
        Ok::<_, NativeError>((
            plugin_map(source.plugins)?,
            plugin_map(target.plugins)?,
            marketplace_map(source.marketplaces)?,
            marketplace_map(target.marketplaces)?,
            config(native, paths.source).map_err(|_| invalid_inventory())?,
        ))
    })();
    let (source_items, mut target_items, source_markets, mut target_markets, source_config) =
        match inventory {
            Ok(value) => value,
            Err(error) => {
                return json!({"manifest":manifest,"issues":[issue("codex_account_plugin_inventory_failed",json!({"error":error.kind}))],"trust_pending":null});
            }
        };
    let mut issues = Vec::new();
    let previous_plugins = manifest["plugins"].as_object().unwrap();
    let mut desired = BTreeMap::new();
    let mut managed = Map::new();
    let mut installs = BTreeMap::new();
    let preserve = |id: &str, managed: &mut Map<String, Value>| {
        if let Some(value) = previous_plugins.get(id) {
            managed.insert(id.into(), value.clone());
        }
    };
    for (id, plugin) in &source_items {
        let target_item = target_items.get(id);
        let source_origin = plugin_origin(plugin, paths.home);
        if source_origin.as_ref().is_some_and(|o| o.0 == "remote") && target_item.is_some() {
            let target_item = target_item.unwrap();
            let code = if plugin_origin(target_item, paths.home) != source_origin {
                Some("codex_account_plugin_origin_conflict")
            } else if target_item["version"] != plugin["version"] {
                Some("codex_account_plugin_version_conflict")
            } else {
                None
            };
            if let Some(code) = code {
                issues.push(issue(code, json!({"plugin":id})));
                preserve(id, &mut managed);
            } else {
                desired.insert(id.clone(), plugin["enabled"].as_bool().unwrap());
            }
            continue;
        }
        let name = market_name(id, plugin);
        let fallback = plugin
            .get("marketplaceSource")
            .filter(|v| v.is_object())
            .map(|s| json!({"name":name,"marketplaceSource":s}));
        let Some(market) = source_markets.get(name).or(fallback.as_ref()) else {
            issues.push(issue(
                "codex_account_plugin_marketplace_missing",
                json!({"plugin":id}),
            ));
            preserve(id, &mut managed);
            continue;
        };
        let expected = origin(market, paths.home);
        if expected.is_none() || origin(plugin, paths.home).is_some_and(|o| Some(o) != expected) {
            issues.push(issue(
                "codex_account_plugin_origin_conflict",
                json!({"plugin":id}),
            ));
            preserve(id, &mut managed);
            continue;
        }
        if !ensure_marketplace(
            &paths,
            market,
            plugin,
            target_item,
            &mut target_markets,
            &source_config,
            native,
            &mut issues,
        )
        .await
        {
            preserve(id, &mut managed);
            continue;
        }
        if let Some(item) = target_item {
            let current = origin(item, paths.home);
            if current.is_none() || current != expected {
                issues.push(issue(
                    if current.is_none() {
                        "codex_account_plugin_origin_unknown"
                    } else {
                        "codex_account_plugin_origin_conflict"
                    },
                    json!({"plugin":id}),
                ));
                preserve(id, &mut managed);
                continue;
            }
        }
        if target_item.is_none_or(|p| p["version"] != plugin["version"]) {
            match native.install(paths.destination, id).await {
                Ok(value) => {
                    installs.insert(id.clone(), value);
                }
                Err(error) => {
                    issues.push(issue(
                        if target_item.is_none() {
                            "codex_account_plugin_install_failed"
                        } else {
                            "codex_account_plugin_update_failed"
                        },
                        json!({"plugin":id,"error":error.kind}),
                    ));
                    preserve(id, &mut managed);
                    continue;
                }
            }
        }
        desired.insert(id.clone(), plugin["enabled"].as_bool().unwrap());
    }
    target_items = match native.plugins(paths.destination).await.and_then(plugin_map) {
        Ok(items) => items,
        Err(error) => {
            issues.push(issue(
                "codex_account_plugin_inventory_failed",
                json!({"error":error.kind}),
            ));
            BTreeMap::new()
        }
    };
    for id in desired.keys() {
        let plugin = &source_items[id];
        let Some(item) = target_items.get(id) else {
            issues.push(issue(
                "codex_account_plugin_not_installed",
                json!({"plugin":id}),
            ));
            preserve(id, &mut managed);
            continue;
        };
        if item["version"] != plugin["version"] {
            issues.push(issue(
                "codex_account_plugin_version_conflict",
                json!({"plugin":id}),
            ));
            preserve(id, &mut managed);
            continue;
        }
        let Some(source_origin) = plugin_origin(plugin, paths.home).or_else(|| {
            source_markets
                .get(market_name(id, plugin))
                .and_then(|m| origin(m, paths.home))
        }) else {
            issues.push(issue(
                "codex_account_plugin_origin_unknown",
                json!({"plugin":id}),
            ));
            preserve(id, &mut managed);
            continue;
        };
        let source_info = if source_origin.0 == "bundled" {
            json!({"sourceType":"bundled","name":market_name(id,plugin)})
        } else {
            plugin
                .get("marketplaceSource")
                .cloned()
                .unwrap_or(json!({}))
        };
        let target_path = installs
            .get(id)
            .and_then(|i| i.get("installedPath"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_path(paths.destination, item));
        managed.insert(id.clone(),json!({"pluginId":id,"version":plugin["version"],"enabled":plugin["enabled"],"marketplace":market_name(id,plugin),"marketplaceSource":source_info,"origem":{"type":source_origin.0,"source":source_origin.1},"cache_fonte":cache_path(paths.source,plugin),"cache_destino":target_path}));
        let root = cache_path(paths.destination, item);
        if root.is_dir()
            && native
                .adapt_hooks(
                    paths.destination,
                    &root,
                    &private_dir(&paths).join("backups"),
                )
                .await
                .is_err()
        {
            issues.push(issue(
                "codex_account_plugin_hooks_failed",
                json!({"error":"RuntimeError"}),
            ));
        }
    }
    for (id, old) in previous_plugins
        .iter()
        .filter(|(id, _)| !source_items.contains_key(*id))
    {
        // A intenção permanece até o inventário confirmar a desativação ou a ausência.
        preserve(id, &mut managed);
        if let Some(item) = target_items.get(id) {
            let current = origin(item, paths.home);
            if current.is_some() && current == old_origin(old, paths.home) {
                desired.insert(id.clone(), false);
            } else {
                issues.push(issue(
                    "codex_account_plugin_origin_conflict",
                    json!({"plugin":id}),
                ));
            }
        }
    }
    write_enabled(&paths, &desired, native, &mut issues).await;
    match native.plugins(paths.destination).await.and_then(plugin_map) {
        Ok(items) => {
            for (id, old) in previous_plugins
                .iter()
                .filter(|(id, _)| !source_items.contains_key(*id))
            {
                let confirmed = items.get(id).is_none_or(|item| {
                    let current = origin(item, paths.home);
                    item["enabled"] == false
                        && current.is_some()
                        && current == old_origin(old, paths.home)
                });
                if confirmed {
                    managed.remove(id);
                }
            }
            target_items = items;
        }
        Err(error) => issues.push(issue(
            "codex_account_plugin_inventory_failed",
            json!({"error":error.kind}),
        )),
    }
    for (id, entry) in &mut managed {
        if let Some(enabled) = target_items
            .get(id)
            .and_then(|i| i.get("enabled"))
            .filter(|v| v.is_boolean())
        {
            entry["enabled"] = enabled.clone();
        }
    }
    let trust = native.trust(paths.destination, true).await.ok().flatten();
    if trust.is_none() {
        issues.push(issue("codex_account_plugin_trust_unavailable", json!({})));
    }
    let markets: Map<String, Value> = source_markets
        .iter()
        .filter(|(name, _)| {
            source_items
                .iter()
                .any(|(id, p)| market_name(id, p) == name.as_str())
        })
        .map(|(name, item)| (name.clone(), market_manifest(item)))
        .collect();
    json!({"manifest":{"marketplaces":markets,"plugins":managed},"issues":issues,"trust_pending":trust})
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
