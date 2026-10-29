//! Aplicação ordenada; erros são por item, e todo arquivo substituído tem recuperação.
use super::{
    bundle::{Blob, Bundle, Error, warning},
    export::{self, Context, MACHINE_MCP, OWNED_SETTINGS},
    files, paths,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct Apply {
    pub context: Context,
    pub backups: PathBuf,
    pub bundle: Bundle,
    pub report: serde_json::Map<String, Value>,
    pub programs: BTreeMap<String, String>,
    pub accounts: crate::accounts::AccountService,
}
impl Apply {
    pub fn new(
        context: Context,
        bundle: Bundle,
        stamp: &str,
        programs: BTreeMap<String, String>,
    ) -> Self {
        let backups = Path::new(&context.roots.home)
            .join(".hangar/config-sync/backups")
            .join(stamp);
        let accounts = crate::accounts::AccountService::new(
            crate::accounts::environment::AccountEnvironment::from_map(
                [
                    ("HOME".into(), context.roots.home.clone()),
                    ("USERPROFILE".into(), context.roots.home.clone()),
                ]
                .into(),
            ),
        );
        Self {
            accounts,
            context,
            backups,
            bundle,
            report: Default::default(),
            programs,
        }
    }
    pub fn result(&mut self, item: &str) -> &mut Value {
        self.report.entry(item).or_insert_with(||json!({"status":"same","changed":[],"warnings":self.bundle.warnings.get(item).cloned().unwrap_or_else(||json!([]))}))
    }
    pub fn warn(&mut self, item: &str, code: &str, params: Value) {
        self.result(item)["warnings"]
            .as_array_mut()
            .unwrap()
            .push(warning(code, params));
    }
    pub fn changed(&mut self, item: &str, key: impl Into<String>) {
        self.result(item)["changed"]
            .as_array_mut()
            .unwrap()
            .push(json!(key.into()));
    }
    pub fn blob(&self, member: &str, text: bool) -> Result<Blob, Error> {
        let blob = self
            .bundle
            .files
            .get(member)
            .ok_or_else(|| Error::invalid(format!("arquivo ausente no pacote: {member}")))?;
        if !text {
            return Ok(blob.clone());
        }
        let text = std::str::from_utf8(&blob.data).map_err(|e| Error::invalid(e.to_string()))?;
        Ok(Blob {
            data: paths::resolve(text, &self.context.roots).into_bytes(),
            mode: blob.mode,
        })
    }
    fn write(
        &mut self,
        item: &str,
        name: &str,
        dest: &Path,
        kind: &str,
        files: BTreeMap<String, Blob>,
    ) -> Result<bool, Error> {
        let (changed, link) =
            files::write_entry(dest, kind, &files, &self.context.roots, &self.backups)?;
        if let Some(link) = link {
            self.warn(
                item,
                "config_sync_link_replaced",
                json!({"entry":name,"target":link.to_string_lossy()}),
            );
        }
        Ok(changed)
    }
    fn directory(&mut self, item: &str) -> Result<(), Error> {
        let data = self.bundle.items[item].clone();
        let roots = self.context.roots.clone();
        let skills = export::own_skills(&roots);
        let hooks = export::own_hooks(&roots);
        let allowed: &[&str] = match item {
            "claude_instructions" => &["rules", ""],
            "claude_skills" => &["skills"],
            "claude_agents" => &["agents", "commands", "output-styles"],
            "claude_hooks" => &["hooks"],
            _ => &[],
        };
        let entries: BTreeMap<_, _> = data["entries"].as_object().into_iter().flatten().collect();
        for (name, entry) in entries {
            let (folder, base) = name.rsplit_once('/').unwrap_or(("", name));
            let rels: Vec<_> = entry["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !allowed.contains(&folder)
                || name.contains('\\')
                || base.is_empty()
                || [".", ".."].contains(&base)
                || (folder.is_empty() && !base.ends_with(".md"))
                || escapes(name)
                || rels.iter().any(|r| escapes(r))
            {
                self.warn(item, "config_sync_invalid_entry", json!({"entry":name}));
                continue;
            }
            if folder == "skills" && skills.contains(base) {
                self.warn(item, "config_sync_hangar_skill_kept", json!({"entry":name}));
                continue;
            }
            if folder == "hooks" && hooks.contains(base) {
                continue;
            }
            let text: Vec<_> = entry["text"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mut files = BTreeMap::new();
            for rel in rels {
                let member = format!(
                    "files/{item}/{name}{}",
                    if rel.is_empty() {
                        String::new()
                    } else {
                        format!("/{rel}")
                    }
                );
                files.insert(rel.to_owned(), self.blob(&member, text.contains(&rel))?);
            }
            if self.write(
                item,
                name,
                &Path::new(&roots.claude).join(name),
                entry["kind"].as_str().unwrap_or("dir"),
                files,
            )? {
                self.changed(item, name);
            }
        }
        Ok(())
    }
    fn references(&mut self, item: &str) -> Result<(), Error> {
        let data = self.bundle.items[item].clone();
        let roots = self.context.roots.clone();
        let refs: BTreeMap<_, _> = data["refs"].as_object().into_iter().flatten().collect();
        for (marked, reference) in refs {
            let local = PathBuf::from(paths::resolve(marked, &roots));
            if marked.starts_with("⟦HANGAR⟧") {
                if !local.exists() {
                    self.warn(
                        item,
                        "config_sync_hangar_outdated",
                        json!({"file":local.to_string_lossy()}),
                    );
                }
                continue;
            }
            let rest = marked
                .split_once('⟧')
                .map(|(_, r)| r)
                .unwrap_or(marked)
                .replace('\\', "/");
            if !["⟦CLAUDE⟧", "⟦CODEX⟧", "⟦HOME⟧"]
                .iter()
                .any(|p| marked.starts_with(p))
                || rest.split('/').any(|p| p == "..")
                || export::never_ref(marked)
            {
                self.warn(item, "config_sync_invalid_entry", json!({"entry":marked}));
                continue;
            }
            let blob = self.blob(
                reference["member"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("referência sem membro"))?,
                truthy(&reference["text"]),
            )?;
            if self.write(item, marked, &local, "file", [(String::new(), blob)].into())? {
                self.changed(item, local.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    pub(super) fn fix_commands(&mut self, item: &str, value: &mut Value, place: &str) {
        if let Some(command) = value["command"].as_str() {
            let (command, missing) = paths::fix_programs(
                command,
                |name| self.programs.get(name).cloned(),
                |path| Path::new(path).exists(),
            );
            value["command"] = json!(command);
            for program in missing {
                self.warn(
                    item,
                    "config_sync_missing_program",
                    json!({"program":program,"where":place}),
                );
            }
        }
    }
    fn merge_at(
        &mut self,
        item: &str,
        path: &Path,
        key: Option<&str>,
        incoming: Value,
        prefix: &str,
    ) -> Result<(), Error> {
        let mut changed = Vec::new();
        files::edit_json(path, &self.backups, |current| {
            changed.clear();
            if let Some(key) = key {
                if incoming.as_object().is_some_and(|m| !m.is_empty()) {
                    if !current[key].is_object() {
                        current[key] = json!({});
                    }
                    merge(&mut current[key], &incoming, prefix, &mut changed);
                }
            } else {
                merge(current, &incoming, prefix, &mut changed);
            }
            Ok(())
        })?;
        for key in changed {
            self.changed(item, key);
        }
        Ok(())
    }
    fn hooks(&mut self) -> Result<(), Error> {
        let item = "claude_hooks";
        self.directory(item)?;
        self.references(item)?;
        let data = self.bundle.items[item].clone();
        let roots = self.context.roots.clone();
        let own = export::own_hooks(&roots);
        let mut incoming = export::uncanon(&data["hooks"], &roots);
        for (event, groups) in incoming.as_object_mut().into_iter().flatten() {
            for group in groups.as_array_mut().into_iter().flatten() {
                for hook in group["hooks"].as_array_mut().into_iter().flatten() {
                    self.fix_commands(item, hook, event);
                }
            }
        }
        let mut status = data
            .get("statusLine")
            .filter(|v| v.is_object())
            .map(|v| export::uncanon(v, &roots));
        if let Some(status) = &mut status {
            self.fix_commands(item, status, "statusLine");
        }
        let mut changed = Vec::new();
        let mut replaced = Vec::new();
        files::edit_json(
            &Path::new(&roots.claude).join("settings.json"),
            &self.backups,
            |settings| {
                changed.clear();
                replaced.clear();
                let mut current = if settings["hooks"].is_object() {
                    settings["hooks"].clone()
                } else {
                    json!({})
                };
                let ordered: BTreeMap<_, _> = incoming.as_object().into_iter().flatten().collect();
                for (event, groups) in ordered {
                    let mut new = export::split_hooks(&current[event], &own, true);
                    new.as_array_mut()
                        .unwrap()
                        .extend(groups.as_array().into_iter().flatten().cloned());
                    let commands = |groups: &Value| {
                        groups
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
                            .filter_map(|h| h["command"].as_str())
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    };
                    let arriving = commands(groups);
                    let leaving = commands(&export::split_hooks(&current[event], &own, false));
                    for command in leaving {
                        if !arriving.contains(&command) {
                            replaced.push(warning("config_sync_hook_replaced",json!({"event":event,"command":command.chars().take(120).collect::<String>()})));
                        }
                    }
                    let old = if truthy(&current[event]) {
                        current[event].clone()
                    } else {
                        json!([])
                    };
                    if new != old {
                        changed.push(format!("hooks:{event}"));
                    }
                    if new.as_array().is_some_and(|a| !a.is_empty()) {
                        current[event] = new;
                    } else {
                        current.as_object_mut().unwrap().shift_remove(event);
                    }
                }
                if !current.as_object().unwrap().is_empty() || settings.get("hooks").is_some() {
                    settings["hooks"] = current;
                }
                if let Some(status) = &status {
                    if settings.get("statusLine") != Some(status) {
                        settings["statusLine"] = status.clone();
                        changed.push("statusLine".into());
                    }
                }
                Ok(())
            },
        )?;
        for key in changed {
            self.changed(item, key);
        }
        self.result(item)["warnings"]
            .as_array_mut()
            .unwrap()
            .extend(replaced);
        Ok(())
    }
    pub fn apply_item(&mut self, item: &str) -> Result<(), Error> {
        let data = self.bundle.items[item].clone();
        let roots = self.context.roots.clone();
        let settings = Path::new(&roots.claude).join("settings.json");
        match item {
            "claude_instructions" | "claude_skills" | "claude_agents" => self.directory(item),
            "claude_hooks" => self.hooks(),
            "claude_env" => self.merge_at(
                item,
                &settings,
                Some("env"),
                export::uncanon(&data["env"], &roots),
                "",
            ),
            "claude_settings" => {
                let mut incoming = export::uncanon(&data["settings"], &roots);
                if let Some(map) = incoming.as_object_mut() {
                    map.retain(|k, _| !OWNED_SETTINGS.contains(&k.as_str()));
                }
                self.merge_at(item, &settings, None, incoming, "")
            }
            "engines" => self.merge_at(
                item,
                &self.context.engines.clone(),
                None,
                export::uncanon(&data["engines"], &roots),
                "",
            ),
            "claude_mcp" => {
                self.references(item)?;
                let mut incoming = export::uncanon(&data["servers"], &roots);
                if let Some(map) = incoming.as_object_mut() {
                    map.retain(|k, _| !MACHINE_MCP.contains(&k.as_str()));
                    for (name, server) in map {
                        self.fix_commands(item, server, name);
                    }
                }
                self.merge_at(
                    item,
                    &Path::new(&roots.home).join(".claude.json"),
                    Some("mcpServers"),
                    incoming,
                    "",
                )
            }
            "claude_plugins" => {
                let mut changed = Vec::new();
                files::edit_json(&settings, &self.backups, |current| {
                    changed.clear();
                    for (key, prefix) in [
                        ("enabledPlugins", "plugin:"),
                        ("extraKnownMarketplaces", "marketplace:"),
                    ] {
                        let incoming = export::uncanon(&data[key], &roots);
                        if incoming.as_object().is_some_and(|m| !m.is_empty()) {
                            if !current[key].is_object() {
                                current[key] = json!({});
                            }
                            merge(&mut current[key], &incoming, prefix, &mut changed);
                        }
                    }
                    Ok(())
                })?;
                for key in changed {
                    self.changed(item, key);
                }
                if !self.programs.contains_key("claude") {
                    self.warn(
                        item,
                        "config_sync_missing_program",
                        json!({"program":"claude","where":"plugins"}),
                    );
                }
                Ok(())
            }
            "hangar_appearance" => self.appearance(),
            "hangar_prefs" => self.preferences(),
            "codex" => {
                if let Some(agents) = data.get("agents_md").filter(|v| v.is_object()) {
                    let blob = self.blob(
                        agents["member"]
                            .as_str()
                            .ok_or_else(|| Error::invalid("AGENTS.md sem membro"))?,
                        truthy(&agents["text"]),
                    )?;
                    if self.write(
                        item,
                        "AGENTS.md",
                        &Path::new(&roots.codex).join("AGENTS.md"),
                        "file",
                        [(String::new(), blob)].into(),
                    )? {
                        self.changed(item, "AGENTS.md");
                    }
                }
                if truthy(&data["config"]) && !self.programs.contains_key("codex") {
                    self.warn(
                        item,
                        "config_sync_missing_program",
                        json!({"program":"codex","where":"config.toml"}),
                    );
                }
                Ok(())
            }
            "claude_accounts" => self.apply_accounts(),
            _ => Err(Error::invalid("item desconhecido")),
        }
    }
    fn apply_accounts(&mut self) -> Result<(), Error> {
        let item = "claude_accounts";
        let data = self.bundle.items[item]["accounts"].clone();
        let ordered: BTreeMap<_, _> = data.as_object().into_iter().flatten().collect();
        for (name, entry) in ordered {
            if !crate::accounts::catalog::valid_name(name) || !entry.is_object() {
                self.warn(item, "config_sync_invalid_entry", json!({"entry":name}));
                continue;
            }
            if let Err(error) = self.apply_account(name, entry) {
                self.warn(item, "config_sync_account_failed", json!({"account":name,"error":error.message.chars().take(200).collect::<String>()}));
            }
        }
        Ok(())
    }
    fn apply_account(&mut self, name: &str, entry: &Value) -> Result<(), Error> {
        use crate::accounts::{Provider, aliases, catalog, claude_config};
        let item = "claude_accounts";
        let path = self.accounts.target(Provider::Claude, name);
        let mut touched = false;
        if !self.accounts.managed(Provider::Claude, &path, name) {
            if std::fs::symlink_metadata(&path).is_ok() {
                self.warn(
                    item,
                    "config_sync_account_not_hangar",
                    json!({"account":name}),
                );
                return Ok(());
            }
            let home = self.accounts.env.home.clone();
            self.accounts
                .create(Provider::Claude, name, |path| {
                    let result = claude_config::seed(&home, path);
                    claude_config::write_cli_diary(&self.accounts.env, &result.diary);
                    result.result.map(|_| ()).map_err(|error| {
                        catalog::AccountError::new(
                            500,
                            "account_prepare_failed",
                            error.cause(),
                            json!({}),
                        )
                    })
                })
                .map_err(|error| Error::new(error.code, error.message, error.params))?;
            touched = true;
            self.changed(item, name);
            self.warn(
                item,
                "config_sync_account_needs_login",
                json!({"account":name}),
            );
        }
        if let Some(alias) = entry["alias"].as_str().filter(|s| !s.trim().is_empty()) {
            let id = format!("claude:{}", catalog::resolved(&path).display());
            let clean: String = alias.trim().chars().take(40).collect();
            if aliases::read(&self.accounts.env)
                .get(&id)
                .and_then(Value::as_str)
                != Some(&clean)
            {
                aliases::set(&self.accounts.env, &id, Some(alias))?;
                touched = true;
            }
        }
        let mut incoming = export::uncanon(&entry["settings"], &self.context.roots);
        if let Some(map) = incoming.as_object_mut() {
            map.retain(|k, _| {
                !OWNED_SETTINGS.contains(&k.as_str())
                    && !export::ACCOUNT_SKIPPED.contains(&k.as_str())
            });
        }
        if incoming.as_object().is_some_and(|m| !m.is_empty()) {
            let mut changed = Vec::new();
            files::edit_json(&path.join("settings.json"), &self.backups, |current| {
                changed.clear();
                merge(current, &incoming, "", &mut changed);
                Ok(())
            })?;
            touched |= !changed.is_empty();
        }
        if touched
            && !self.result(item)["changed"]
                .as_array()
                .unwrap()
                .contains(&json!(name))
        {
            self.changed(item, name);
        }
        Ok(())
    }
    fn preferences(&mut self) -> Result<(), Error> {
        let item = "hangar_prefs";
        let incoming = export::uncanon(&self.bundle.items[item]["prefs"], &self.context.roots);
        let path = self.context.preferences.clone();
        let mut current = export::read_json(&path).unwrap_or_else(|_| json!({}));
        let pending: BTreeMap<_, _> = incoming
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, v)| {
                super::preferences::editable(k)
                    && !["sync", "term_origins", "scan_roots"].contains(&k.as_str())
                    && current.get(*k) != Some(*v)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !pending.is_empty() && path.exists() {
            std::fs::copy(
                &path,
                files::backup_target(&path, &self.context.roots, &self.backups)?,
            )?;
        }
        for (key, value) in pending {
            match super::preferences::coerce(&key, &value) {
                Ok(value) => {
                    current[&key] = value;
                    files::write_file(
                        &path,
                        &Blob {
                            data: serde_json::to_vec_pretty(&current).unwrap(),
                            mode: 0o600,
                        },
                    )?;
                    self.changed(item, key);
                }
                Err(error) => self.warn(
                    item,
                    "config_sync_pref_rejected",
                    json!({"key":key,"error":error.chars().take(200).collect::<String>()}),
                ),
            }
        }
        Ok(())
    }
    fn appearance(&mut self) -> Result<(), Error> {
        let item = "hangar_appearance";
        let data = self.bundle.items[item].clone();
        let folder = self.context.native.clone();
        let target = folder.join("appearance.json");
        let mut image_changed = false;
        if data["background"].is_object() {
            if let Some(blob) = self
                .bundle
                .files
                .get("files/hangar_appearance/background-image")
                .cloned()
            {
                let name =
                    export::background_name(data["background"]["name"].as_str().unwrap_or(""));
                for (name, blob) in [
                    ("background-image", blob),
                    (
                        "background-name",
                        Blob {
                            data: name.into_bytes(),
                            mode: 0o644,
                        },
                    ),
                ] {
                    image_changed |= self.write(
                        item,
                        name,
                        &folder.join(name),
                        "file",
                        [(String::new(), blob)].into(),
                    )?;
                }
                if image_changed {
                    self.changed(item, "background-image");
                }
            }
        }
        let mut incoming = data["native"].clone();
        if let Some(map) = incoming.as_object_mut() {
            map.retain(|k, _| !export::NATIVE_LOCAL.contains(&k.as_str()));
        }
        let mut changed = Vec::new();
        if incoming.as_object().is_some_and(|m| !m.is_empty()) {
            files::edit_json(&target, &self.backups, |current| {
                changed.clear();
                merge(current, &incoming, "", &mut changed);
                Ok(())
            })?;
            if !changed.is_empty() {
                self.changed(item, "appearance.json");
            }
        }
        if image_changed && changed.is_empty() && target.exists() {
            std::fs::File::open(&target)?.set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now())
                    .set_accessed(std::time::SystemTime::now()),
            )?;
        }
        Ok(())
    }
    pub fn finish_item(&mut self, item: &str, result: Result<(), Error>) {
        if let Err(error) = result {
            self.result(item)["status"] = json!("failed");
            self.warn(
                item,
                "config_sync_item_failed",
                json!({"item":item,"error":error.message.chars().take(300).collect::<String>()}),
            );
        } else {
            let changed = self.result(item)["changed"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
            self.result(item)["status"] = json!(if changed { "applied" } else { "same" });
        }
    }
    pub fn report(&self) -> Value {
        json!({"items":self.report,"backup":if self.backups.exists(){self.backups.to_string_lossy().into_owned()}else{String::new()}})
    }
}
pub fn escapes(path: &str) -> bool {
    path.contains('\\')
        || path.starts_with('/')
        || path.split('/').any(|p| p == "..")
        || path.as_bytes().get(1) == Some(&b':')
}
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}
fn merge(current: &mut Value, incoming: &Value, prefix: &str, changed: &mut Vec<String>) {
    for (key, value) in incoming.as_object().into_iter().flatten() {
        if current.get(key).unwrap_or(&Value::Null) != value {
            changed.push(format!("{prefix}{key}"));
            current[key] = value.clone();
        }
    }
}
