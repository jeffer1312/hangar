//! Leitura do estado dos plugins no disco da pasta principal, sem executar git nem a CLI.
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::model::{InstalledPlugin, Marketplace, MarketplaceName, PluginsState};

/// Marketplace embutido da CLI (`ptt` na 2.1.319); não aparece no `known_marketplaces.json`.
pub(crate) const BUILTIN: &str = "anthropic-plugin-directory";

/// Conjunto `fbt` menos `rf` da CLI 2.1.319: os que atualizam sozinhos sem `autoUpdate` gravado.
const OFFICIAL: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "anthropic-agent-skills",
    "life-sciences",
    "claude-for-legal",
    "claude-for-financial-services",
    "financial-services-plugins",
    "claude-tag-plugins",
];

/// Um elemento de `claude plugin list --json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstalled {
    pub id: String,
    #[serde(default)]
    pub version: String,
    pub scope: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub install_path: Option<String>,
    #[serde(default)]
    pub installed_at: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub note_details: Option<Value>,
    #[serde(default)]
    pub project_enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoUpdateError {
    SettingsUnreadable,
    AutoUpdateUnavailable,
}

impl AutoUpdateError {
    pub fn code(self) -> &'static str {
        match self {
            Self::SettingsUnreadable => "plugins_settings_unreadable",
            Self::AutoUpdateUnavailable => "plugins_auto_update_unavailable",
        }
    }
}

impl fmt::Display for AutoUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for AutoUpdateError {}

/// Junta o `plugin list --json` (só escopo `user`) ao que o disco da pasta principal diz. Arquivo
/// ausente ou ilegível nunca derruba a lista: o que dependia dele sai desconhecido.
pub fn read_state(main_dir: &Path, settings: &Value, installed_cli: &[CliInstalled]) -> PluginsState {
    let known = read_json(&main_dir.join("plugins/known_marketplaces.json"))
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let declared = settings.get("extraKnownMarketplaces").and_then(Value::as_object);
    let plugins = installed_cli
        .iter()
        .filter(|plugin| plugin.scope == "user")
        .map(|plugin| installed(plugin, &known))
        .collect();
    let mut marketplaces: Vec<Marketplace> = known
        .iter()
        .map(|(name, entry)| marketplace(name, entry, declared.and_then(|d| d.get(name))))
        .collect();
    if !known.contains_key(BUILTIN) {
        marketplaces.push(Marketplace {
            name: BUILTIN.to_owned(),
            source_label: "Anthropic".to_owned(),
            last_updated: None,
            auto_update: true,
            auto_update_editable: false,
            builtin: true,
        });
    }
    // Mesma ordem da CLI: o embutido primeiro, depois por nome.
    marketplaces.sort_by(|a, b| b.builtin.cmp(&a.builtin).then_with(|| a.name.cmp(&b.name)));
    PluginsState { plugins, marketplaces, checked_at: None }
}

/// Versão que o `claude plugin update` compararia: `plugin.json` no clone, senão o `version` da
/// entrada, senão 12 caracteres do `HEAD` do clone. Fonte fora do clone (objeto) dá `None`: a versão
/// dela é a do `plugin.json` no commit remoto, que o disco não tem.
pub fn source_version(marketplace_dir: &Path, entry: &Value) -> Option<String> {
    let plugin_dir = inside(marketplace_dir, entry.get("source")?.as_str()?)?;
    read_json(&plugin_dir.join(".claude-plugin/plugin.json"))
        .and_then(|manifest| text(&manifest, "version"))
        .or_else(|| text(entry, "version"))
        .or_else(|| head_sha(marketplace_dir).map(|sha| sha[..12].to_owned()))
}

/// # Errors
///
/// - [`AutoUpdateError::SettingsUnreadable`] quando o `settings.json` não é objeto.
/// - [`AutoUpdateError::AutoUpdateUnavailable`] quando o marketplace não está em
///   `extraKnownMarketplaces`.
pub fn apply_auto_update(settings: &mut Value, name: &MarketplaceName, enabled: bool) -> Result<(), AutoUpdateError> {
    let entry = settings
        .as_object_mut()
        .ok_or(AutoUpdateError::SettingsUnreadable)?
        .get_mut("extraKnownMarketplaces")
        .and_then(Value::as_object_mut)
        .and_then(|declared| declared.get_mut(name.as_str()))
        .and_then(Value::as_object_mut)
        .ok_or(AutoUpdateError::AutoUpdateUnavailable)?;
    entry.insert("autoUpdate".to_owned(), Value::Bool(enabled));
    Ok(())
}

fn installed(plugin: &CliInstalled, known: &Map<String, Value>) -> InstalledPlugin {
    let (name, marketplace) = plugin.id.split_once('@').unwrap_or((plugin.id.as_str(), ""));
    let clone = known
        .get(marketplace)
        .and_then(|entry| text(entry, "installLocation"))
        .map(PathBuf::from);
    let entry = clone.as_deref().and_then(|dir| catalog_entry(dir, name));
    let source = clone.as_deref().zip(entry.as_ref()).and_then(|(dir, entry)| source_version(dir, entry));
    InstalledPlugin {
        id: plugin.id.clone(),
        name: name.to_owned(),
        marketplace: marketplace.to_owned(),
        version: plugin.version.clone(),
        last_updated: plugin.last_updated.clone().or_else(|| plugin.installed_at.clone()).unwrap_or_default(),
        enabled: plugin.enabled,
        update_known: source.is_some(),
        available_version: newer(&plugin.version, source),
        description: entry.as_ref().and_then(|entry| text(entry, "description")),
        notes: plugin.notes.clone(),
    }
}

fn marketplace(name: &str, entry: &Value, declared: Option<&Value>) -> Marketplace {
    let source = &entry["source"];
    let declared_auto = declared.and_then(|d| d.get("autoUpdate")).and_then(Value::as_bool);
    Marketplace {
        name: name.to_owned(),
        source_label: ["repo", "url", "path"]
            .iter()
            .find_map(|key| text(source, key))
            .map(|label| without_userinfo(&label))
            .unwrap_or_default(),
        last_updated: text(entry, "lastUpdated"),
        auto_update: auto_update(name, entry, declared_auto),
        auto_update_editable: declared.is_some(),
        builtin: name == BUILTIN,
    }
}

/// Marketplace privado guarda o token na URL (`https://user:token@host/...`); o rótulo vai à tela.
fn without_userinfo(label: &str) -> String {
    let Some((scheme, rest)) = label.split_once("://") else { return label.to_owned() };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => label.to_owned(),
    }
}

/// Cópia de `BVe` da CLI 2.1.319: o `settings.json` vence, depois o `known_marketplaces.json`;
/// sem nenhum, só fonte embutida e marketplace oficial atualizam sozinhos.
fn auto_update(name: &str, entry: &Value, declared: Option<bool>) -> bool {
    declared
        .or_else(|| entry.get("autoUpdate").and_then(Value::as_bool))
        .unwrap_or_else(|| {
            let kind = entry["source"].get("source").and_then(Value::as_str);
            matches!(kind, Some("claudeai" | "pluginDirectory")) || OFFICIAL.contains(&name.to_lowercase().as_str())
        })
}

fn catalog_entry(clone: &Path, name: &str) -> Option<Value> {
    let catalog = read_json(&clone.join(".claude-plugin/marketplace.json"))
        .or_else(|| read_json(&clone.join("marketplace.json")))?;
    catalog
        .get("plugins")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .cloned()
}

fn newer(installed: &str, source: Option<String>) -> Option<String> {
    source.filter(|version| version != installed)
}

/// Caminho relativo da entrada dentro do clone; `..` ou absoluto não saem dele.
fn inside(root: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    path.components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        .then(|| root.join(path))
}

fn head_sha(clone: &Path) -> Option<String> {
    let git = clone.join(".git");
    let head = read_text(&git.join("HEAD"))?;
    let sha = match head.trim().strip_prefix("ref: ") {
        Some(name) => read_text(&git.join(name)).or_else(|| packed_ref(&git, name))?,
        None => head,
    };
    let sha = sha.trim();
    (sha.len() >= 12 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha.to_owned())
}

fn packed_ref(git: &Path, name: &str) -> Option<String> {
    read_text(&git.join("packed-refs"))?
        .lines()
        .find_map(|line| line.split_once(' ').filter(|(_, refname)| *refname == name).map(|(sha, _)| sha.to_owned()))
}

/// O CLI troca esses arquivos por tmp + rename; no Windows a leitura no instante da troca falha.
fn read_text(path: &Path) -> Option<String> {
    hangar_core::fs::read_replaced_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&read_text(path)?).ok()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_label_drops_credentials() {
        assert_eq!(super::without_userinfo("https://user:tok3n@git.example.com/a/b.git"), "https://git.example.com/a/b.git");
        assert_eq!(super::without_userinfo("https://git.example.com/team/acme.git"), "https://git.example.com/team/acme.git");
        assert_eq!(super::without_userinfo("JuliusBrussee/caveman"), "JuliusBrussee/caveman");
        assert_eq!(super::without_userinfo("https://host/path@x"), "https://host/path@x");
    }

    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::*;

    const SHA: &str = "a3829a90c85c7ac558ce67c425dbb613fca8b085";

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("pai")).expect("pasta");
        fs::write(path, text).expect("escrita");
    }

    #[test]
    fn source_version_prefers_plugin_json() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join("plugins/x/.claude-plugin/plugin.json"), r#"{"name":"x","version":"1.1.0"}"#);
        let entry = json!({"name": "x", "source": "./plugins/x", "version": "0.9.0"});
        assert_eq!(source_version(dir.path(), &entry).as_deref(), Some("1.1.0"));
    }

    #[test]
    fn source_version_falls_back_to_entry_version() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join(".claude-plugin/plugin.json"), r#"{"name":"ui-ux-pro-max"}"#);
        let entry = json!({"name": "ui-ux-pro-max", "source": "./", "version": "2.15.0"});
        assert_eq!(source_version(dir.path(), &entry).as_deref(), Some("2.15.0"));
    }

    #[test]
    fn source_version_uses_loose_head_ref() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join("plugin/.claude-plugin/plugin.json"), r#"{"name":"acme"}"#);
        write(&dir.path().join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&dir.path().join(".git/refs/heads/main"), &format!("{SHA}\n"));
        let entry = json!({"name": "acme", "source": "./plugin"});
        assert_eq!(source_version(dir.path(), &entry).as_deref(), Some("a3829a90c85c"));
    }

    #[test]
    fn source_version_reads_packed_refs_without_loose_ref() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &dir.path().join(".git/packed-refs"),
            &format!("# pack-refs with: peeled fully-peeled sorted \n1111111111111111111111111111111111111111 refs/remotes/origin/main\n{SHA} refs/heads/main\n"),
        );
        let entry = json!({"name": "acme", "source": "./"});
        assert_eq!(source_version(dir.path(), &entry).as_deref(), Some("a3829a90c85c"));
    }

    #[test]
    fn source_version_reads_detached_head() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join(".git/HEAD"), &format!("{SHA}\n"));
        let entry = json!({"name": "x", "source": "./"});
        assert_eq!(source_version(dir.path(), &entry).as_deref(), Some("a3829a90c85c"));
    }

    #[test]
    fn source_version_is_unknown_for_external_sources() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join(".git/HEAD"), &format!("{SHA}\n"));
        let url = json!({"name": "superpowers", "version": "7.0.2",
            "source": {"source": "url", "url": "https://github.com/obra/superpowers.git", "sha": "bb92a77741419a4ab5f06e711a283343f1ada0c3"}});
        let subdir = json!({"name": "adobe", "version": "1.0.0",
            "source": {"source": "git-subdir", "url": "https://github.com/adobe/skills.git", "path": "plugins/x"}});
        assert_eq!(source_version(dir.path(), &url), None);
        assert_eq!(source_version(dir.path(), &subdir), None);
    }

    #[test]
    fn source_version_refuses_paths_outside_the_clone() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join("fora/.claude-plugin/plugin.json"), r#"{"version":"9.9.9"}"#);
        let clone = dir.path().join("clone");
        write(&clone.join(".git/HEAD"), &format!("{SHA}\n"));
        let entry = json!({"name": "x", "source": "../fora"});
        assert_eq!(source_version(&clone, &entry), None);
    }

    /// Trechos de `claude plugin list --json` (2.1.319) desta máquina.
    fn cli_list() -> Vec<CliInstalled> {
        serde_json::from_value(json!([
            {"id": "caveman@caveman", "version": "4.1.0", "scope": "user", "enabled": true,
             "installPath": "/x/cache/caveman/caveman/4.1.0", "installedAt": "2026-04-23T13:07:40.388Z",
             "lastUpdated": "2026-11-07T04:43:45.891Z",
             "notes": ["The packages it lists were not installed, because it uses a yarn or pnpm lockfile, which plugin installs do not support"],
             "noteDetails": [{"type": "dependencies-refused", "plugin": "caveman"}], "projectEnabled": true},
            {"id": "caveman@caveman", "version": "4.1.0", "scope": "project", "enabled": true,
             "installPath": "/x/cache/caveman/caveman/4.1.0", "installedAt": "2026-08-22T12:12:02.261Z",
             "lastUpdated": "2026-11-07T14:19:19.203Z", "projectPath": "/home/x", "projectEnabled": true},
            {"id": "superpowers@claude-plugins-official", "version": "7.0.0", "scope": "user", "enabled": true,
             "installPath": "/x", "installedAt": "2026-10-16T02:34:02.764Z", "lastUpdated": "2026-10-16T02:34:02.764Z"},
            {"id": "pyright-lsp@claude-plugins-official", "version": "1.0.0", "scope": "user", "enabled": false,
             "installPath": "/x", "installedAt": "2026-09-17T12:50:16.463Z", "lastUpdated": "2026-09-17T12:50:16.463Z"},
            {"id": "acme@acme-marketplace", "version": "a3829a90c85c", "scope": "user", "enabled": true,
             "installPath": "/x", "installedAt": "2026-04-16T14:14:12.426Z", "lastUpdated": "2026-11-09T11:40:28.525Z"}
        ]))
        .expect("formato do list --json")
    }

    fn main_dir_fixture(root: &Path) -> std::path::PathBuf {
        let main = root.join("main");
        let caveman = root.join("mk/caveman");
        let official = root.join("mk/claude-plugins-official");
        write(&caveman.join(".claude-plugin/plugin.json"), r#"{"name":"caveman","version":"4.2.0"}"#);
        write(
            &caveman.join(".claude-plugin/marketplace.json"),
            r#"{"name":"caveman","plugins":[{"name":"caveman","source":"./","description":"Ultra-compressed communication mode."}]}"#,
        );
        write(&official.join("plugins/pyright-lsp/.claude-plugin/plugin.json"), r#"{"name":"pyright-lsp"}"#);
        write(
            &official.join(".claude-plugin/marketplace.json"),
            r#"{"name":"claude-plugins-official","plugins":[
                {"name":"pyright-lsp","version":"1.0.0","source":"./plugins/pyright-lsp","description":"Python language server (Pyright)"},
                {"name":"superpowers","description":"Superpowers teaches Claude brainstorming.",
                 "source":{"source":"url","url":"https://github.com/obra/superpowers.git","sha":"bb92a77741419a4ab5f06e711a283343f1ada0c3"}}]}"#,
        );
        let known = json!({
            "acme-marketplace": {"source": {"source": "git", "url": "https://git.example.com/team/acme.git"},
                "installLocation": root.join("mk/sumiu"), "lastUpdated": "2026-11-09T11:52:19.272Z", "autoUpdate": true},
            "caveman": {"source": {"source": "github", "repo": "JuliusBrussee/caveman"},
                "installLocation": caveman, "lastUpdated": "2026-11-09T11:52:20.473Z", "autoUpdate": true},
            "claude-plugins-official": {"source": {"source": "github", "repo": "anthropics/claude-plugins-official"},
                "installLocation": official, "lastUpdated": "2026-11-09T11:52:20.500Z"},
            "ponytail": {"source": {"source": "directory", "path": "/srv/ponytail"},
                "installLocation": root.join("mk/ponytail"), "lastUpdated": "2026-11-09T11:52:20.448Z"}
        });
        write(&main.join("plugins/known_marketplaces.json"), &known.to_string());
        main
    }

    fn settings() -> Value {
        json!({
            "enabledPlugins": {"caveman@caveman": true},
            "extraKnownMarketplaces": {
                "caveman": {"source": {"source": "github", "repo": "JuliusBrussee/caveman"}, "autoUpdate": false},
                "acme-marketplace": {"source": {"source": "git", "url": "https://git.example.com/team/acme.git"}}
            }
        })
    }

    #[test]
    fn read_state_joins_cli_list_with_disk() {
        let dir = tempfile::tempdir().expect("pasta");
        let state = read_state(&main_dir_fixture(dir.path()), &settings(), &cli_list());
        let plugin = |id: &str| state.plugins.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("{id} ausente"));

        assert_eq!(state.plugins.len(), 4, "só escopo user");
        assert_eq!(
            plugin("caveman@caveman"),
            &InstalledPlugin {
                id: "caveman@caveman".into(),
                name: "caveman".into(),
                marketplace: "caveman".into(),
                version: "4.1.0".into(),
                last_updated: "2026-11-07T04:43:45.891Z".into(),
                enabled: true,
                available_version: Some("4.2.0".into()),
                update_known: true,
                description: Some("Ultra-compressed communication mode.".into()),
                notes: vec!["The packages it lists were not installed, because it uses a yarn or pnpm lockfile, which plugin installs do not support".into()],
            }
        );
        let superpowers = plugin("superpowers@claude-plugins-official");
        assert_eq!((superpowers.available_version.as_deref(), superpowers.update_known), (None, false));
        assert_eq!(superpowers.description.as_deref(), Some("Superpowers teaches Claude brainstorming."));
        let pyright = plugin("pyright-lsp@claude-plugins-official");
        assert_eq!((pyright.available_version.as_deref(), pyright.update_known, pyright.enabled), (None, true, false));
        let acme = plugin("acme@acme-marketplace");
        assert_eq!((acme.available_version.as_deref(), acme.update_known, acme.description.as_deref()), (None, false, None));
        assert_eq!(state.checked_at, None);
    }

    #[test]
    fn read_state_lists_marketplaces_with_auto_update_rule() {
        let dir = tempfile::tempdir().expect("pasta");
        let state = read_state(&main_dir_fixture(dir.path()), &settings(), &cli_list());
        fn row(m: &Marketplace) -> (&str, &str, Option<&str>, bool, bool, bool) {
            (m.name.as_str(), m.source_label.as_str(), m.last_updated.as_deref(), m.auto_update, m.auto_update_editable, m.builtin)
        }
        assert_eq!(
            state.marketplaces.iter().map(row).collect::<Vec<_>>(),
            vec![
                ("anthropic-plugin-directory", "Anthropic", None, true, false, true),
                ("acme-marketplace", "https://git.example.com/team/acme.git", Some("2026-11-09T11:52:19.272Z"), true, true, false),
                ("caveman", "JuliusBrussee/caveman", Some("2026-11-09T11:52:20.473Z"), false, true, false),
                ("claude-plugins-official", "anthropics/claude-plugins-official", Some("2026-11-09T11:52:20.500Z"), true, false, false),
                ("ponytail", "/srv/ponytail", Some("2026-11-09T11:52:20.448Z"), false, false, false),
            ]
        );
    }

    #[test]
    fn read_state_survives_unreadable_marketplace_registry() {
        let dir = tempfile::tempdir().expect("pasta");
        write(&dir.path().join("plugins/known_marketplaces.json"), "{ quebrado");
        let state = read_state(dir.path(), &json!("não é objeto"), &cli_list());
        assert_eq!(state.plugins.len(), 4);
        assert!(state.plugins.iter().all(|p| p.available_version.is_none() && !p.update_known));
        assert_eq!(state.marketplaces.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["anthropic-plugin-directory"]);
    }

    #[test]
    fn apply_auto_update_changes_only_that_key() {
        let name = MarketplaceName::parse("caveman").expect("nome");
        let mut current = settings();
        current["model"] = json!("opus");
        apply_auto_update(&mut current, &name, true).expect("marketplace declarado");
        let mut expected = settings();
        expected["model"] = json!("opus");
        expected["extraKnownMarketplaces"]["caveman"]["autoUpdate"] = json!(true);
        assert_eq!(current, expected);

        let acme = MarketplaceName::parse("acme-marketplace").expect("nome");
        apply_auto_update(&mut current, &acme, false).expect("marketplace declarado sem a chave");
        assert_eq!(current["extraKnownMarketplaces"]["acme-marketplace"]["autoUpdate"], json!(false));
    }

    #[test]
    fn apply_auto_update_refuses_what_it_cannot_change() {
        let ponytail = MarketplaceName::parse("ponytail").expect("nome");
        let mut current = settings();
        assert_eq!(apply_auto_update(&mut current, &ponytail, true), Err(AutoUpdateError::AutoUpdateUnavailable));
        assert_eq!(current, settings());
        let mut broken = json!(["lista"]);
        assert_eq!(apply_auto_update(&mut broken, &ponytail, true), Err(AutoUpdateError::SettingsUnreadable));
        let mut no_extra = json!({"model": "opus"});
        assert_eq!(apply_auto_update(&mut no_extra, &ponytail, true), Err(AutoUpdateError::AutoUpdateUnavailable));
        assert_eq!(AutoUpdateError::SettingsUnreadable.code(), "plugins_settings_unreadable");
        assert_eq!(AutoUpdateError::AutoUpdateUnavailable.code(), "plugins_auto_update_unavailable");
    }

    #[test]
    fn newer_reports_only_a_different_version() {
        assert_eq!(newer("1.0.0", Some("1.1.0".into())).as_deref(), Some("1.1.0"));
        assert_eq!(newer("1.1.0", Some("1.1.0".into())), None);
        assert_eq!(newer("1.1.0", None), None);
    }
}
