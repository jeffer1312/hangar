//! Instalação pelo CLI do destino, preservando os registros e avisos de cada plugin.
use super::{apply::{Apply, truthy}, bundle::Error, export};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, path::Path};

impl Apply {
    pub async fn apply_codex_config(&mut self) -> Result<(), Error> {
        let item = "codex";
        let mut config = export::uncanon(&self.bundle.items[item]["config"], &self.context.roots);
        if !truthy(&config) || !self.programs.contains_key("codex") { return Ok(()); }
        if let Some(servers) = config["mcp_servers"].as_object_mut() {
            for (name,server) in servers {
                if server.is_object() { self.fix_commands(item,server,name); }
            }
        }
        let mut changed = Vec::new();
        crate::codex_setup::files::edit_config(
            &self.accounts.env,
            &Path::new(&self.context.roots.codex).join("config.toml"),
            &self.backups.join("codex"),
            &Path::new(&self.context.roots.home).join(".hangar/config-sync/work"),
            |current| {
                changed.clear();
                let incoming = config.as_object().ok_or("configuração Codex não é objeto")?;
                let mut edits = Vec::new();
                for (key,value) in incoming {
                    let value = if key == "mcp_servers" && value.is_object() && current[key].is_object() {
                        let mut merged = current[key].as_object().unwrap().clone();
                        merged.extend(value.as_object().unwrap().clone());
                        Value::Object(merged)
                    } else { value.clone() };
                    if current.get(key).unwrap_or(&Value::Null) != &value {
                        changed.push(format!("config:{key}"));
                        edits.push(json!({"keyPath":crate::transcript::pyjson::dumps(&json!(key),false),"value":value,"mergeStrategy":"replace"}));
                    }
                }
                Ok(edits)
            },
        ).await.map_err(|error|Error::new("config_sync_codex_failed",error,json!({})))?;
        for key in changed { self.changed(item,key); }
        Ok(())
    }

    pub async fn install_plugins_with<F, Fut>(&mut self, mut run: F) -> Result<(), Error>
    where F: FnMut(Vec<String>) -> Fut, Fut: Future<Output = (bool, String)> {
        let item = "claude_plugins";
        let Some(program) = self.programs.get("claude").cloned() else { return Ok(()); };
        let data = self.bundle.items[item].clone();
        let path = Path::new(&self.context.roots.claude).join("plugins");
        let known = export::read_json(&path.join("known_marketplaces.json"))?;
        let installed = export::read_json(&path.join("installed_plugins.json"))?;
        let sources: BTreeMap<_, _> = data["sources"].as_object().into_iter().flatten().collect();
        for (name, source) in sources {
            if known.get(name).is_some() { continue; }
            let source = export::uncanon(source, &self.context.roots);
            let key = match source["source"].as_str() {
                Some("git" | "url") => "url", Some("github") => "repo",
                Some("directory" | "file") => "path", _ => "",
            };
            let Some(argument) = source[key].as_str().filter(|s| !s.is_empty()) else {
                self.warn(item, "config_sync_marketplace_unknown", json!({"marketplace":name}));
                continue;
            };
            self.context.emit(json!({"phase":"install","item":item,"entry":format!("marketplace:{name}")}));
            let (ok, detail) = run(vec![program.clone(), "plugin".into(), "marketplace".into(), "add".into(), argument.into()]).await;
            if ok { self.changed(item, format!("marketplace:{name}")); }
            else { self.warn(item, "config_sync_marketplace_failed", json!({"marketplace":name,"detail":detail})); }
        }
        let enabled: BTreeMap<_, _> = data["enabledPlugins"].as_object().into_iter().flatten().collect();
        for (plugin, enabled) in enabled {
            if !truthy(enabled) || installed["plugins"].get(plugin).is_some() { continue; }
            self.context.emit(json!({"phase":"install","item":item,"entry":format!("plugin:{plugin}")}));
            let (ok, detail) = run(vec![program.clone(), "plugin".into(), "install".into(), plugin.clone()]).await;
            if ok { self.changed(item, format!("plugin:{plugin}")); }
            else { self.warn(item, "config_sync_plugin_failed", json!({"plugin":plugin,"detail":detail})); }
        }
        Ok(())
    }
}
