//! Catálogo de comandos do Claude compartilhado pelos clientes do Hangar.
pub mod cache;
mod frontmatter;
pub mod probe;
mod service;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub(crate) use service::matches;
pub use service::{Context, Service, route, serve};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::SystemTime,
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub display: String,
    pub description: Option<String>,
    #[serde(rename = "argumentHint")]
    pub argument_hint: Option<String>,
    pub source: String,
    pub destructive: bool,
}

#[derive(Default)]
pub struct Catalog {
    frontmatter: HashMap<PathBuf, (Option<SystemTime>, u64, Value)>,
}

fn clean(value: Option<&Value>) -> Option<String> {
    let text = value?
        .as_str()?
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '\u{001c}'..='\u{001f}'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}

fn command(
    name: String,
    description: Option<String>,
    hint: Option<String>,
    source: &str,
    destructive: bool,
) -> CommandInfo {
    CommandInfo {
        display: format!("/{name}"),
        name,
        description,
        argument_hint: hint,
        source: source.into(),
        destructive,
    }
}

fn builtins() -> Vec<CommandInfo> {
    let rows = [
        ("clear", "Limpa o histórico da conversa"),
        ("compact", "Resume e compacta o contexto"),
        ("btw", "Pergunta lateral, sem interromper a conversa"),
        ("context", "Mostra o uso do contexto"),
        ("model", "Troca o modelo do Claude"),
        ("effort", "Ajusta o esforço de raciocínio"),
        ("resume", "Retoma uma conversa anterior"),
        ("rewind", "Volta a conversa a um ponto anterior"),
        ("release-notes", "Mostra as novidades da versão"),
        ("help", "Lista os comandos disponíveis"),
        ("status", "Mostra o estado da sessão e da conta"),
        ("cost", "Mostra o custo e o uso de tokens"),
        ("export", "Exporta a conversa"),
        ("init", "Cria um arquivo CLAUDE.md no projeto"),
        ("agents", "Gerencia subagentes"),
        ("mcp", "Gerencia servidores MCP"),
        ("memory", "Edita os arquivos de memória"),
        ("vim", "Ativa o modo de edição estilo vim"),
        ("config", "Abre as configurações"),
        ("doctor", "Verifica a saúde da instalação"),
        ("quit", "Encerra a sessão"),
        ("login", "Entra com a conta Anthropic"),
        ("permissions", "Gerencia as regras de permissão"),
        ("hooks", "Gerencia os hooks"),
        ("plugin", "Gerencia plugins e marketplaces"),
        ("skills", "Lista as skills disponíveis"),
        ("theme", "Troca o tema do terminal"),
        ("statusline", "Configura a linha de status"),
        ("add-dir", "Adiciona um diretório de trabalho"),
        ("review", "Revisa um pull request"),
        (
            "reload-plugins",
            "Recarrega os plugins para aplicar as alterações pendentes",
        ),
    ];
    rows.into_iter()
        .map(|(name, description)| {
            let hint = match name {
                "btw" => Some("[pergunta]".into()),
                "add-dir" => Some("<caminho>".into()),
                "reload-plugins" => Some("[--force]".into()),
                _ => None,
            };
            command(
                name.into(),
                Some(description.into()),
                hint,
                "builtin",
                matches!(name, "clear" | "compact" | "quit"),
            )
        })
        .collect()
}

fn children(path: &Path) -> Vec<fs::DirEntry> {
    if !path.is_dir() {
        return Vec::new();
    }
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut paths: Vec<_> = entries.filter_map(Result::ok).collect();
            #[cfg(windows)]
            paths.sort_by_cached_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
            #[cfg(not(windows))]
            paths.sort_by_cached_key(fs::DirEntry::file_name);
            paths
        }
        Err(error) => {
            tracing::warn!(code = "claude_commands_directory_unreadable", io_kind = ?error.kind(), "pasta do catálogo de comandos ilegível");
            Vec::new()
        }
    }
}

impl Catalog {
    fn metadata(&mut self, path: &Path, meta: &fs::Metadata) -> Value {
        let stamp = meta.modified().ok();
        if let Some((mtime, size, value)) = self.frontmatter.get(path)
            && stamp.is_some()
            && *mtime == stamp
            && *size == meta.len()
        {
            return value.clone();
        }
        static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"(?s)^\x{feff}?---\s*\n(.*?)\n---\s*(?:\n|$)").unwrap()
        });
        let raw = fs::read(path).unwrap_or_default();
        let text = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
        let value = HEADER
            .captures(&text)
            .and_then(|c| frontmatter::parse(&c[1]))
            .filter(Value::is_object)
            .unwrap_or(Value::Null);
        self.frontmatter
            .insert(path.to_owned(), (stamp, meta.len(), value.clone()));
        value
    }

    fn scan_commands(&mut self, path: &Path) -> Vec<CommandInfo> {
        children(path)
            .into_iter()
            .map(|entry| (entry.path(), entry))
            .filter(|(p, _)| {
                p.extension().is_some_and(|extension| {
                    if cfg!(windows) {
                        extension.to_string_lossy().eq_ignore_ascii_case("md")
                    } else {
                        extension == "md"
                    }
                })
            })
            .filter_map(|(p, entry)| {
                // No Windows a enumeração já trouxe os metadados dos arquivos regulares.
                let file = entry.metadata().ok()?;
                let file = if file.file_type().is_symlink() {
                    fs::metadata(&p).ok()?
                } else {
                    file
                };
                if !file.is_file() {
                    return None;
                }
                let meta = self.metadata(&p, &file);
                let name = clean(meta.get("name")).unwrap_or_else(|| {
                    p.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
                if name.is_empty() {
                    return None;
                }
                let hint = meta
                    .get("argument-hint")
                    .filter(|v| truthy(v))
                    .or_else(|| meta.get("argumentHint"));
                Some(command(
                    name,
                    clean(meta.get("description")),
                    clean(hint),
                    "skill",
                    false,
                ))
            })
            .collect()
    }

    fn scan_skills(&mut self, path: &Path) -> Vec<CommandInfo> {
        children(path)
            .into_iter()
            .map(|entry| entry.path())
            .filter(|p| p.is_dir())
            .filter_map(|p| {
                let path = p.join("SKILL.md");
                let file = fs::metadata(&path).ok()?;
                if !file.is_file() {
                    return None;
                }
                let meta = self.metadata(&path, &file);
                let name = clean(meta.get("name")).unwrap_or_else(|| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
                (!name.is_empty())
                    .then(|| command(name, clean(meta.get("description")), None, "skill", false))
            })
            .collect()
    }

    fn scan_plugins(&mut self, path: &Path) -> Vec<CommandInfo> {
        let registry = fs::read(path.join("installed_plugins.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        let Some(plugins) = registry.as_ref().and_then(|v| v["plugins"].as_object()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, entries) in plugins {
            let plugin = id.split('@').next().unwrap_or(id);
            for entry in entries.as_array().into_iter().flatten() {
                let Some(root) = entry
                    .get("installPath")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let root = Path::new(root);
                for mut info in self
                    .scan_commands(&root.join("commands"))
                    .into_iter()
                    .chain(self.scan_skills(&root.join("skills")))
                {
                    info.name = format!("{plugin}:{}", info.name);
                    info.display = format!("/{}", info.name);
                    info.source = "plugin".into();
                    out.push(info);
                }
            }
        }
        out
    }

    pub fn compose(
        &mut self,
        home: &Path,
        cwd: Option<&Path>,
        cli: Option<&[Value]>,
        excluded: &[String],
        terminal: bool,
    ) -> Vec<CommandInfo> {
        self.compose_for_config(&home.join(".claude"), cwd, cli, excluded, terminal)
    }

    pub fn compose_for_config(
        &mut self,
        config_dir: &Path,
        cwd: Option<&Path>,
        cli: Option<&[Value]>,
        excluded: &[String],
        terminal: bool,
    ) -> Vec<CommandInfo> {
        let mut raw = builtins();
        let mut project = Vec::new();
        if let Some(cwd) = cwd {
            project.extend(self.scan_commands(&cwd.join(".claude/commands")));
            project.extend(self.scan_skills(&cwd.join(".claude/skills")));
        }
        raw.extend(project.clone());
        raw.extend(self.scan_commands(&config_dir.join("commands")));
        raw.extend(self.scan_skills(&config_dir.join("skills")));
        raw.extend(self.scan_plugins(&config_dir.join("plugins")));
        if let Some(cli) = cli {
            let mut local = HashMap::new();
            for item in &raw {
                local.entry(item.name.as_str()).or_insert(item);
            }
            let mut from_cli = Vec::new();
            for item in cli {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let known = local.get(name).copied();
                from_cli.push(command(
                    name.into(),
                    known
                        .and_then(|v| v.description.clone())
                        .or_else(|| clean(item.get("description"))),
                    known
                        .and_then(|v| v.argument_hint.clone())
                        .or_else(|| clean(item.get("argumentHint"))),
                    known
                        .map(|v| v.source.as_str())
                        .unwrap_or(if name.contains(':') {
                            "plugin"
                        } else {
                            "builtin"
                        }),
                    known.is_some_and(|v| v.destructive),
                ));
            }
            if terminal {
                from_cli.extend(builtins());
            }
            from_cli.extend(project);
            raw = from_cli;
        }
        let mut seen: HashSet<String> = excluded.iter().cloned().collect();
        raw.into_iter()
            .filter(|info| seen.insert(info.name.clone()))
            .collect()
    }
}
