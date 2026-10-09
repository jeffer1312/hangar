//! Catálogo e escolhas por conversa; nenhuma escrita nas configurações do Claude.
use crate::{routes::AppState, workspace_routes::private_ok};
use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;

const MAX_BODY: usize = 256 * 1024;
const MAX_SOURCE: usize = 1024 * 1024;
const MAX_ITEMS: usize = 4096;
const MAX_PLUGINS: usize = 512;

#[derive(Debug, Serialize)]
struct CustomizationError {
    status: u16,
    code: &'static str,
    detail: &'static str,
}
type Result<T> = std::result::Result<T, CustomizationError>;
fn error(status: u16, code: &'static str, detail: &'static str) -> CustomizationError {
    CustomizationError { status, code, detail }
}
fn invalid() -> CustomizationError {
    error(400, "claude_customizations_invalid", "Escolha de plugins ou skills inválida.")
}
fn envelope(result: Result<Value>) -> Response {
    let value = match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

#[derive(Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Catalog(Context),
    Prepare(Prepare),
    Remember(Remember),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    cwd: PathBuf,
    config_dir: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    cwd: PathBuf,
    config_dir: PathBuf,
    session_id: String,
    #[serde(default)]
    selection: Option<Selection>,
    #[serde(default)]
    resume: bool,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    #[serde(default)]
    plugins: BTreeMap<String, bool>,
    #[serde(default)]
    skills: BTreeMap<String, bool>,
    #[serde(default)]
    blocked_skills: Vec<String>,
}
impl Selection {
    fn is_empty(&self) -> bool {
        self.plugins.is_empty() && self.skills.is_empty() && self.blocked_skills.is_empty()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remember {
    session_id: String,
    settings: Value,
}

pub async fn private(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let bytes = match tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return envelope(Err(invalid())),
    };
    let op = match serde_json::from_slice::<Operation>(&bytes) {
        Ok(op) => op,
        Err(_) => return envelope(Err(invalid())),
    };
    let slots = if matches!(&op, Operation::Catalog(_)) { &st.workspace_read_slots } else { &st.workspace_slots };
    let permit = match slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return envelope(Err(error(503, "claude_customizations_busy", "Consulta de plugins ocupada."))),
    };
    let op = match tokio::task::spawn_blocking(move || {
        match &op {
            Operation::Catalog(ctx) => validate_context(&ctx.cwd, &ctx.config_dir)?,
            Operation::Prepare(args) => {
                session_key(&args.session_id)?;
                validate_context(&args.cwd, &args.config_dir)?;
                if let Some(selection) = &args.selection { selection_settings(selection)?; }
            },
            Operation::Remember(args) => {
                session_key(&args.session_id)?;
                restricted_settings(&args.settings)?;
            },
        }
        Ok::<_, CustomizationError>(op)
    }).await {
        Ok(Ok(op)) => op,
        Ok(Err(error)) => return envelope(Err(error)),
        Err(_) => return envelope(Err(error(500, "claude_customizations_failed", "Não deu para validar as escolhas da conversa."))),
    };
    // O CLI só consulta metadados; leituras de disco e gravação ficam fora do executor assíncrono.
    let context = match &op {
        Operation::Catalog(ctx) => Some((&ctx.cwd, &ctx.config_dir)),
        Operation::Prepare(args) if args.selection.as_ref().is_some_and(|selection| !selection.is_empty()) => Some((&args.cwd, &args.config_dir)),
        _ => None,
    };
    let installations = if let Some((cwd, config_dir)) = context {
        match installed_plugins(cwd, config_dir).await {
            Ok(installations) => installations,
            Err(error) => return envelope(Err(error)),
        }
    } else { Vec::new() };
    envelope(match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        execute(op, installations, &store_root()?)
    }).await {
        Ok(result) => result,
        Err(_) => Err(error(500, "claude_customizations_failed", "Não foi possível confirmar as escolhas da conversa.")),
    })
}

fn valid_part(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name != "." && name != ".."
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn valid_skill(name: &str) -> bool {
    name.len() <= 256 && name.split(':').all(valid_part)
}
fn valid_plugin(id: &str) -> bool {
    id.len() <= 256 && id.split_once('@').is_some_and(|(name, origin)| valid_part(name) && valid_part(origin))
}
fn session_key(id: &str) -> Result<String> {
    let bytes = id.as_bytes();
    if bytes.len() != 36 || bytes.iter().enumerate().any(|(i, b)| {
        if [8, 13, 18, 23].contains(&i) { *b != b'-' } else { !b.is_ascii_hexdigit() }
    }) { return Err(invalid()); }
    Ok(id.to_ascii_lowercase())
}
fn validate_context(cwd: &Path, config_dir: &Path) -> Result<()> {
    for path in [cwd, config_dir] {
        if !path.is_absolute() || path.as_os_str().len() > 4096
            || path.to_string_lossy().chars().any(char::is_control)
            || path.components().any(|part| matches!(part, Component::ParentDir)) {
            return Err(error(400, "claude_context_invalid", "Diretório da conversa inválido."));
        }
        if !fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(error(400, "claude_context_invalid", "Diretório da conversa indisponível."));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct Installation {
    id: String,
    version: String,
    scope: String,
    enabled: bool,
    #[serde(rename = "installPath")]
    install_path: PathBuf,
    #[serde(default, rename = "readFromFolder")]
    read_from_folder: Option<PathBuf>,
    #[serde(default, rename = "projectPath")]
    project_path: Option<PathBuf>,
    #[serde(default)]
    errors: Vec<Value>,
    #[serde(default)]
    notes: Vec<Value>,
}
fn configure_plugin_command(command: &mut tokio::process::Command, cwd: &Path, config_dir: &Path) {
    for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
    command.args(["plugin", "list", "--json"]).current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1").env("DISABLE_UPDATES", "1")
        .env_remove("CLAUDE_CODE_PLUGIN_DIRS")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)] { command.creation_flags(0x08000000); }
}
async fn installed_plugins(cwd: &Path, config_dir: &Path) -> Result<Vec<Installation>> {
    let mut command = tokio::process::Command::new("claude");
    configure_plugin_command(&mut command, cwd, config_dir);
    let mut child = command.spawn().map_err(|_| error(503, "claude_plugins_unavailable", "Consulta nativa de plugins indisponível."))?;
    let stdout = child.stdout.take().ok_or_else(|| error(503, "claude_plugins_unavailable", "Consulta nativa de plugins indisponível."))?;
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        let mut bytes = Vec::new();
        stdout.take((2 * MAX_SOURCE + 1) as u64).read_to_end(&mut bytes).await
            .map_err(|_| error(503, "claude_plugins_unavailable", "Não deu para ler a lista de plugins."))?;
        if bytes.len() > 2 * MAX_SOURCE {
            return Err(error(502, "claude_plugins_too_large", "Lista de plugins excedeu o limite."));
        }
        let status = child.wait().await
            .map_err(|_| error(503, "claude_plugins_unavailable", "Consulta nativa de plugins indisponível."))?;
        if !status.success() {
            return Err(error(502, "claude_plugins_failed", "O Claude recusou a consulta de plugins."));
        }
        let rows: Vec<Installation> = serde_json::from_slice(&bytes)
            .map_err(|_| error(502, "claude_plugins_invalid", "O Claude devolveu uma lista de plugins inválida."))?;
        if rows.len() > MAX_PLUGINS { return Err(error(502, "claude_plugins_too_large", "Lista de plugins excedeu o limite.")); }
        Ok(rows)
    }).await;
    match result {
        Ok(result) => result,
        Err(_) => Err(error(504, "claude_plugins_timeout", "A consulta de plugins demorou demais.")),
    }
}

fn read_source(path: &Path) -> io::Result<Option<String>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "link")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        },
        Err(error) => return Err(error),
    };
    if !metadata.is_file() { return Err(io::Error::new(io::ErrorKind::InvalidData, "tipo")); }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    // Um FIFO colocado entre a conferência e a abertura não pode prender a vaga de consulta.
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() { return Err(io::Error::new(io::ErrorKind::InvalidData, "tipo")); }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take((MAX_SOURCE + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SOURCE { return Err(io::Error::new(io::ErrorKind::InvalidData, "limite")); }
    String::from_utf8(bytes).map(|s| Some(s.trim_start_matches('\u{feff}').to_owned()))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "encoding"))
}
fn project_dirs(cwd: &Path) -> Result<Vec<PathBuf>> {
    let cwd = fs::canonicalize(cwd).map_err(|_| error(400, "claude_context_invalid", "Diretório da conversa indisponível."))?;
    let mut dirs = Vec::new();
    for dir in cwd.ancestors() {
        dirs.push(dir.to_owned());
        match fs::symlink_metadata(dir.join(".git")) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(_) => return Err(error(422, "claude_context_unreadable", "Não deu para determinar a raiz do projeto.")),
        }
    }
    dirs.reverse();
    Ok(dirs)
}

#[derive(Default, Deserialize)]
struct PolicySource {
    #[serde(default, rename = "enabledPlugins")]
    enabled_plugins: BTreeMap<String, bool>,
    #[serde(default, rename = "skillOverrides")]
    skill_overrides: BTreeMap<String, String>,
    #[serde(default)]
    permissions: DenySource,
}
#[derive(Default, Deserialize)]
struct DenySource {
    #[serde(default)]
    deny: Vec<String>,
}
#[derive(Default)]
struct Policy {
    enabled_plugins: BTreeMap<String, bool>,
    skill_overrides: BTreeMap<String, String>,
    deny: BTreeSet<String>,
}
impl Policy {
    fn load(config_dir: &Path, dirs: &[PathBuf]) -> Result<Self> {
        let mut sources = vec![config_dir.join("settings.json")];
        sources.extend(dirs.iter().map(|dir| dir.join(".claude/settings.json")));
        sources.extend(dirs.iter().map(|dir| dir.join(".claude/settings.local.json")));
        let mut policy = Self::default();
        for path in sources {
            let source = read_source(&path).map_err(|_| error(422, "claude_settings_unreadable", "Não deu para ler as restrições de skills."))?;
            let Some(source) = source else { continue; };
            let source: PolicySource = serde_json::from_str(&source)
                .map_err(|_| error(422, "claude_settings_invalid", "As restrições de skills estão inválidas."))?;
            if source.enabled_plugins.len() > MAX_ITEMS || source.skill_overrides.len() > MAX_ITEMS
                || source.permissions.deny.len() > MAX_ITEMS
                || source.permissions.deny.iter().any(|rule| rule.len() > 4096)
                || source.skill_overrides.iter().any(|(name, state)| !valid_skill(name) || !matches!(state.as_str(), "on" | "off" | "name-only" | "user-invocable-only")) {
                return Err(error(422, "claude_settings_invalid", "As restrições de skills estão inválidas."));
            }
            policy.enabled_plugins.extend(source.enabled_plugins);
            policy.skill_overrides.extend(source.skill_overrides);
            policy.deny.extend(source.permissions.deny);
            if policy.enabled_plugins.len() > MAX_ITEMS || policy.skill_overrides.len() > MAX_ITEMS || policy.deny.len() > MAX_ITEMS {
                return Err(error(422, "claude_settings_invalid", "As restrições de skills excedem o limite."));
            }
        }
        Ok(policy)
    }
    fn denied(&self, name: &str) -> bool {
        self.deny.iter().any(|rule| {
            if rule == "Skill" { return true; }
            let Some(target) = rule.strip_prefix("Skill(").and_then(|r| r.strip_suffix(')')) else { return false; };
            let target = target.strip_suffix(" *").unwrap_or(target);
            if target.is_empty() || target == "*" { return true; }
            if !target.contains('*') { return target == name; }
            if let Some(prefix) = target.strip_suffix('*').filter(|prefix| !prefix.contains('*')) {
                return name.starts_with(prefix);
            }
            let pattern = format!("^{}$", regex::escape(target).replace("\\*", ".*"));
            regex::Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(name))
        })
    }
    fn enabled(&self, name: &str) -> bool {
        self.skill_overrides.get(name).is_none_or(|state| state != "off") && !self.denied(name)
    }
    fn skill(&self, name: String, description: String, plugin: bool) -> Skill {
        let blocked = self.denied(&name);
        Skill { enabled: if plugin { !blocked } else { self.enabled(&name) }, name, description, blocked }
    }
}

#[derive(Debug, Serialize)]
struct Skill {
    name: String,
    description: String,
    enabled: bool,
    blocked: bool,
}
#[derive(Debug, Serialize)]
struct Plugin {
    id: String,
    name: String,
    description: String,
    enabled: bool,
    skills: Vec<Skill>,
}
#[derive(Debug, Default, Serialize)]
struct Catalog {
    plugins: Vec<Plugin>,
    skills: Vec<Skill>,
    warnings: Vec<String>,
}
fn warning(warnings: &mut Vec<String>, code: &str, subject: &str) {
    let detail = match code {
        "claude_plugin_multiple_installations" => "Há várias instalações; vale a primeira disponível na ordem do Claude",
        "claude_skill_source_unreadable" => "Não deu para ler uma fonte de skills",
        "claude_skill_scan_limit" => "O catálogo de skills foi limitado; há itens não listados",
        "claude_skill_metadata_invalid" => "Há metadados YAML de skill inválidos",
        "claude_skill_name_invalid" => "Há um nome de skill inválido",
        "claude_plugin_path_outside" => "O plugin declara um caminho fora da própria pasta",
        "claude_plugin_path_missing" => "O plugin declara um caminho que não existe",
        "claude_plugin_source_unreadable" => "Não deu para ler os arquivos de um plugin instalado",
        "claude_plugin_manifest_invalid" => "Há um manifesto de plugin inválido",
        "claude_plugin_name_invalid" => "Há um nome de plugin inválido",
        "claude_plugin_load_error" => "O Claude informou uma falha ao carregar este plugin",
        "claude_plugin_load_warning" => "O Claude informou um aviso ao carregar este plugin",
        _ => "Não deu para completar uma fonte do catálogo",
    };
    let text = format!("{detail} ({subject}).");
    if !warnings.contains(&text) { warnings.push(text); }
}
fn normalize_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.to_string_lossy().chars().any(char::is_control) {
        return Err(error(502, "claude_plugins_invalid", "O Claude devolveu um diretório de plugin inválido."));
    }
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_owned()),
        Err(_) => Err(error(422, "claude_plugins_unreadable", "Não deu para ler um diretório de plugin.")),
    }
}
fn project_identity(path: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = path.to_str() else { return Err(invalid()); };
    let Some(root) = hangar_workspace::worktrees::repo_root_of(path) else { return Ok(None); };
    let dot = Path::new(&root).join(".git");
    let failed = || error(422, "claude_context_unreadable", "Não deu para determinar a raiz do projeto.");
    let git_dir = if fs::metadata(&dot).map_err(|_| failed())?.is_dir() { dot } else {
        let source = read_source(&dot).map_err(|_| failed())?.ok_or_else(failed)?;
        let target = source.trim().strip_prefix("gitdir:").map(str::trim).filter(|target| !target.is_empty()).ok_or_else(failed)?;
        Path::new(&root).join(target)
    };
    let common = read_source(&git_dir.join("commondir")).map_err(|_| failed())?;
    let git_dir = match common {
        Some(common) if !common.trim().is_empty() => git_dir.join(common.trim()),
        Some(_) => return Err(failed()),
        None => git_dir,
    };
    let git_dir = fs::canonicalize(git_dir).map_err(|_| failed())?;
    if !git_dir.is_dir() { return Err(failed()); }
    Ok(Some(git_dir))
}
fn effective_installations(rows: Vec<Installation>, cwd: &Path, warnings: &mut Vec<String>) -> Result<Vec<Installation>> {
    let cwd = normalize_path(cwd)?;
    let identity = project_identity(&cwd)?;
    let mut grouped: BTreeMap<String, Vec<Installation>> = BTreeMap::new();
    for mut row in rows {
        if !valid_plugin(&row.id) { return Err(error(502, "claude_plugins_invalid", "O Claude devolveu um identificador de plugin inválido.")); }
        if matches!(row.scope.as_str(), "project" | "local") {
            let Some(project) = &row.project_path else {
                return Err(error(502, "claude_plugins_invalid", "Plugin de projeto sem diretório de origem."));
            };
            let project = normalize_path(project)?;
            if project != cwd {
                let project_identity = project_identity(&project)?;
                if identity.is_none() || project_identity != identity { continue; }
            }
            row.project_path = Some(project);
        } else if !matches!(row.scope.as_str(), "user" | "managed" | "synced") {
            return Err(error(502, "claude_plugins_invalid", "O Claude devolveu um escopo de plugin desconhecido."));
        }
        row.install_path = normalize_path(row.read_from_folder.as_ref().unwrap_or(&row.install_path))?;
        grouped.entry(row.id.clone()).or_default().push(row);
    }
    let mut result = Vec::new();
    for (id, mut rows) in grouped {
        let Some(first) = rows.first() else { continue; };
        if rows.iter().any(|row| row.enabled != first.enabled) {
            return Err(error(409, "claude_plugins_ambiguous", "Há estados conflitantes do mesmo plugin neste projeto."));
        }
        if rows.iter().any(|row| row.install_path != first.install_path || row.version != first.version) {
            warning(warnings, "claude_plugin_multiple_installations", &id);
        }
        // O loader nativo usa a ordem do registro, não a versão mais nova nem o escopo mais alto.
        let index = rows.iter().position(|row| row.install_path.is_dir()).unwrap_or(0);
        result.push(rows.remove(index));
    }
    Ok(result)
}

#[derive(Default, Deserialize)]
struct Metadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}
fn frontmatter(text: &str) -> Result<Metadata> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else { return Ok(Metadata::default()); };
    let mut yaml = String::new();
    for line in rest.lines() {
        if line == "---" {
            if yaml.trim().is_empty() { return Ok(Metadata::default()); }
            return serde_saphyr::from_str::<Metadata>(&yaml)
                .map_err(|_| error(422, "claude_skill_metadata_invalid", "Metadados de skill inválidos."));
        }
        yaml.push_str(line); yaml.push('\n');
    }
    Err(error(422, "claude_skill_metadata_invalid", "Metadados de skill inválidos."))
}
struct Scan<'a> {
    policy: &'a Policy,
    warnings: &'a mut Vec<String>,
    excluded: &'a [PathBuf],
    visited: HashSet<PathBuf>,
    remaining: &'a mut usize,
}
impl Scan<'_> {
    fn children(&mut self, path: &Path, subject: &str) -> Vec<PathBuf> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(_) => { warning(self.warnings, "claude_skill_source_unreadable", subject); return Vec::new(); },
        };
        let mut paths = Vec::new();
        for entry in entries {
            if *self.remaining == 0 {
                warning(self.warnings, "claude_skill_scan_limit", subject);
                break;
            }
            *self.remaining -= 1;
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(_) => warning(self.warnings, "claude_skill_source_unreadable", subject),
            }
        }
        paths.sort();
        paths
    }
    fn component(&mut self, root: &Path, text: &str, subject: &str) -> Option<PathBuf> {
        let Some(path) = component_path(root, text) else {
            warning(self.warnings, "claude_plugin_path_outside", subject); return None;
        };
        match fs::metadata(&path) {
            Ok(_) => Some(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                warning(self.warnings, "claude_plugin_path_missing", subject); None
            },
            Err(_) => { warning(self.warnings, "claude_skill_source_unreadable", subject); None },
        }
    }
    fn contained(&mut self, path: &Path, root: Option<&Path>, subject: &str) -> Option<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) if root.is_some_and(|root| !path.starts_with(root)) => {
                warning(self.warnings, "claude_plugin_path_outside", subject); None
            },
            Ok(path) if root.is_none() && (self.excluded.iter().any(|root| path.starts_with(root))
                || path.ancestors().any(|dir| dir.join(".claude-plugin/plugin.json").is_file())) => None,
            Ok(path) => Some(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => { warning(self.warnings, "claude_skill_source_unreadable", subject); None },
        }
    }
    fn metadata(&mut self, path: &Path, fallback: &str, prefix: &str, root: Option<&Path>, subject: &str, skill_name: bool) -> Option<Skill> {
        if *self.remaining == 0 { warning(self.warnings, "claude_skill_scan_limit", subject); return None; }
        *self.remaining -= 1;
        let path = self.contained(path, root, subject)?;
        let text = match read_source(&path) {
            Ok(Some(text)) => text,
            Ok(None) => return None,
            Err(_) => { warning(self.warnings, "claude_skill_source_unreadable", subject); return None; },
        };
        let metadata = match frontmatter(&text) {
            Ok(metadata) => metadata,
            Err(_) => { warning(self.warnings, "claude_skill_metadata_invalid", subject); return None; },
        };
        let name = if skill_name { metadata.name.as_deref().unwrap_or(fallback) } else { fallback };
        let name = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}:{name}") };
        if !valid_skill(&name) {
            warning(self.warnings, "claude_skill_name_invalid", subject); return None;
        }
        Some(self.policy.skill(name, metadata.description.unwrap_or_default().chars().take(1536).collect(), root.is_some()))
    }
    fn skill_dirs(&mut self, dir: &Path, prefix: &str, root: Option<&Path>, subject: &str, out: &mut BTreeMap<String, Skill>) {
        let Some(dir) = self.contained(dir, root, subject) else { return; };
        let direct = dir.join("SKILL.md");
        if direct.is_file() {
            let fallback = dir.file_name().unwrap_or_default().to_string_lossy();
            if let Some(skill) = self.metadata(&direct, &fallback, prefix, root, subject, true) { out.insert(skill.name.clone(), skill); }
            return;
        }
        for path in self.children(&dir, subject) {
            if root.is_none() && path.join(".claude-plugin/plugin.json").is_file() { continue; }
            let fallback = path.file_name().unwrap_or_default().to_string_lossy();
            if let Some(skill) = self.metadata(&path.join("SKILL.md"), &fallback, prefix, root, subject, true) { out.insert(skill.name.clone(), skill); }
        }
    }
    fn commands(&mut self, path: &Path, prefix: &str, root: Option<&Path>, subject: &str, depth: usize, out: &mut BTreeMap<String, Skill>) {
        if depth > 16 { warning(self.warnings, "claude_skill_scan_limit", subject); return; }
        let fallback = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let markdown = path.extension().is_some_and(|extension| extension == "md");
        let Some(path) = self.contained(path, root, subject) else { return; };
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => { warning(self.warnings, "claude_skill_source_unreadable", subject); return; },
        };
        if metadata.is_file() {
            if !markdown { return; }
            if let Some(skill) = self.metadata(&path, &fallback, prefix, root, subject, false) { out.insert(skill.name.clone(), skill); }
        } else if metadata.is_dir() {
            if !self.visited.insert(path.clone()) { return; }
            for child in self.children(&path, subject) {
                let child_prefix = if child.is_dir() && root.is_none() {
                    let folder = child.file_name().unwrap_or_default().to_string_lossy();
                    if prefix.is_empty() { folder.into_owned() } else { format!("{prefix}:{folder}") }
                } else { prefix.to_owned() };
                self.commands(&child, &child_prefix, root, subject, depth + 1, out);
            }
        }
    }
}

#[derive(Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Option<Value>,
    #[serde(default)]
    commands: Option<Value>,
}
fn component_paths(value: &Value) -> Option<Vec<&str>> {
    match value {
        Value::String(path) => Some(vec![path]),
        Value::Array(paths) => paths.iter().map(Value::as_str).collect(),
        _ => None,
    }
}
fn component_path(root: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() || path.as_os_str().len() > 4096
        || path.to_string_lossy().chars().any(char::is_control)
        || path.components().any(|part| !matches!(part, Component::Normal(_) | Component::CurDir)) {
        return None;
    }
    Some(root.join(path))
}
fn plugin_catalog(row: &Installation, policy: &Policy, warnings: &mut Vec<String>, remaining: &mut usize) -> Option<Plugin> {
    let root = match fs::canonicalize(&row.install_path) {
        Ok(root) => root,
        Err(_) => { warning(warnings, "claude_plugin_source_unreadable", &row.id); return None; },
    };
    let manifest_path = root.join(".claude-plugin/plugin.json");
    if fs::canonicalize(&manifest_path).is_ok_and(|path| !path.starts_with(&root)) {
        warning(warnings, "claude_plugin_path_outside", &row.id); return None;
    }
    let manifest = match read_source(&manifest_path) {
        Ok(Some(source)) => match serde_json::from_str::<Manifest>(&source) {
            Ok(manifest) => manifest,
            Err(_) => { warning(warnings, "claude_plugin_manifest_invalid", &row.id); return None; },
        },
        Ok(None) => Manifest::default(),
        Err(_) => { warning(warnings, "claude_plugin_source_unreadable", &row.id); return None; },
    };
    let name = manifest.name.unwrap_or_else(|| row.id.split('@').next().unwrap_or_default().to_owned());
    if !valid_part(&name) { warning(warnings, "claude_plugin_name_invalid", &row.id); return None; }
    if !row.errors.is_empty() { warning(warnings, "claude_plugin_load_error", &row.id); }
    if !row.notes.is_empty() { warning(warnings, "claude_plugin_load_warning", &row.id); }
    let mut out = BTreeMap::new();
    let mut scan = Scan { policy, warnings, excluded: &[], visited: HashSet::new(), remaining };
    if let Some(commands) = &manifest.commands {
        if let Some(paths) = component_paths(commands) {
            for path in paths {
                if let Some(path) = scan.component(&root, path, &row.id) { scan.commands(&path, &name, Some(&root), &row.id, 0, &mut out); }
            }
        } else if let Some(commands) = commands.as_object() {
            for (command, value) in commands {
                if *scan.remaining == 0 { warning(scan.warnings, "claude_skill_scan_limit", &row.id); break; }
                *scan.remaining -= 1;
                if !valid_part(command) { warning(scan.warnings, "claude_skill_name_invalid", &row.id); continue; }
                let source = value.get("source").and_then(Value::as_str);
                let content = value.get("content").and_then(Value::as_str);
                if source.is_some() == content.is_some() {
                    warning(scan.warnings, "claude_plugin_manifest_invalid", &row.id); continue;
                }
                let mut skill = if let Some(path) = source {
                    scan.component(&root, path, &row.id).and_then(|path| scan.metadata(&path, command, &name, Some(&root), &row.id, false))
                } else if value.get("content").is_some_and(Value::is_string) {
                    let skill_name = format!("{name}:{command}");
                    Some(policy.skill(skill_name, String::new(), true))
                } else { warning(scan.warnings, "claude_plugin_manifest_invalid", &row.id); None };
                if let Some(skill) = &mut skill {
                    skill.name = format!("{name}:{command}");
                    skill.blocked = policy.denied(&skill.name);
                    skill.enabled = !skill.blocked;
                    if let Some(description) = value.get("description").and_then(Value::as_str) {
                        skill.description = description.chars().take(1536).collect();
                    }
                }
                if let Some(skill) = skill { out.insert(skill.name.clone(), skill); }
            }
        } else { warning(scan.warnings, "claude_plugin_manifest_invalid", &row.id); }
    } else { scan.commands(&root.join("commands"), &name, Some(&root), &row.id, 0, &mut out); }
    scan.skill_dirs(&root.join("skills"), &name, Some(&root), &row.id, &mut out);
    if let Some(skills) = &manifest.skills {
        if let Some(paths) = component_paths(skills) {
            for path in paths {
                if let Some(path) = scan.component(&root, path, &row.id) { scan.skill_dirs(&path, &name, Some(&root), &row.id, &mut out); }
            }
        } else { warning(scan.warnings, "claude_plugin_manifest_invalid", &row.id); }
    } else if !root.join("skills").exists() && root.join("SKILL.md").is_file() {
        scan.skill_dirs(&root, &name, Some(&root), &row.id, &mut out);
    }
    Some(Plugin { id: row.id.clone(), name, description: manifest.description.unwrap_or_default().chars().take(1536).collect(),
        enabled: row.enabled, skills: out.into_values().collect() })
}
fn catalog(cwd: &Path, config_dir: &Path, installations: Vec<Installation>) -> Result<Catalog> {
    validate_context(cwd, config_dir)?;
    let dirs = project_dirs(cwd)?;
    let policy = Policy::load(config_dir, &dirs)?;
    let mut catalog = Catalog::default();
    let rows = effective_installations(installations, cwd, &mut catalog.warnings)?;
    let excluded: Vec<_> = rows.iter().map(|row| row.install_path.clone()).collect();
    let mut remaining = MAX_ITEMS;
    for row in &rows {
        if let Some(plugin) = plugin_catalog(row, &policy, &mut catalog.warnings, &mut remaining) { catalog.plugins.push(plugin); }
    }
    let mut standalone = BTreeMap::new();
    let mut scan = Scan { policy: &policy, warnings: &mut catalog.warnings, excluded: &excluded, visited: HashSet::new(), remaining: &mut remaining };
    let mut sources = vec![config_dir.to_owned()];
    sources.extend(dirs.iter().map(|dir| dir.join(".claude")));
    for source in sources {
        let subject = if source == config_dir { "conta" } else { "projeto" };
        scan.commands(&source.join("commands"), "", None, subject, 0, &mut standalone);
        scan.skill_dirs(&source.join("skills"), "", None, subject, &mut standalone);
    }
    catalog.skills = standalone.into_values().collect();
    Ok(catalog)
}

fn selection_settings(selection: &Selection) -> Result<Value> {
    if selection.plugins.len() + selection.skills.len() + 2 * selection.blocked_skills.len() > MAX_ITEMS
        || selection.plugins.keys().any(|id| !valid_plugin(id))
        || selection.skills.keys().any(|name| !valid_skill(name))
        || selection.blocked_skills.iter().any(|name| !valid_skill(name) || !name.contains(':')) {
        return Err(invalid());
    }
    let mut settings = serde_json::Map::new();
    if !selection.plugins.is_empty() { settings.insert("enabledPlugins".into(), json!(selection.plugins)); }
    if !selection.skills.is_empty() {
        settings.insert("skillOverrides".into(), json!(selection.skills.iter().map(|(name, enabled)|
            (name.clone(), if *enabled { "on" } else { "off" })).collect::<BTreeMap<_, _>>()));
    }
    if !selection.blocked_skills.is_empty() {
        // O Claude distingue a invocação sem argumentos daquela com argumentos.
        let deny: BTreeSet<_> = selection.blocked_skills.iter().flat_map(|name|
            [format!("Skill({name})"), format!("Skill({name} *)")]).collect();
        settings.insert("permissions".into(), json!({"deny":deny}));
    }
    Ok(Value::Object(settings))
}
fn validate_targets(selection: &Selection, catalog: &Catalog) -> Result<()> {
    for id in selection.plugins.keys() {
        if !catalog.plugins.iter().any(|plugin| &plugin.id == id) {
            return Err(error(409, "claude_plugin_unknown", "Um plugin escolhido não está mais instalado neste projeto."));
        }
    }
    for (name, enabled) in &selection.skills {
        let Some(skill) = catalog.skills.iter().find(|skill| &skill.name == name) else {
            return Err(error(409, "claude_skill_unknown", "Uma skill escolhida não está mais disponível neste projeto."));
        };
        if *enabled && skill.blocked {
            return Err(error(403, "claude_skill_restricted", "Esta skill está desativada nas configurações herdadas."));
        }
    }
    for name in &selection.blocked_skills {
        if !catalog.plugins.iter().flat_map(|plugin| &plugin.skills).any(|skill| &skill.name == name) {
            return Err(error(409, "claude_skill_unknown", "Uma skill escolhida não está mais disponível neste projeto."));
        }
    }
    Ok(())
}
fn restricted_settings(settings: &Value) -> Result<()> {
    let object = settings.as_object().ok_or_else(invalid)?;
    let mut items = 0;
    for (key, value) in object {
        match key.as_str() {
            "enabledPlugins" => {
                let plugins = value.as_object().ok_or_else(invalid)?;
                items += plugins.len();
                if plugins.iter().any(|(id, enabled)| !valid_plugin(id) || !enabled.is_boolean()) { return Err(invalid()); }
            },
            "skillOverrides" => {
                let skills = value.as_object().ok_or_else(invalid)?;
                items += skills.len();
                if skills.iter().any(|(name, state)| !valid_skill(name) || !matches!(state.as_str(), Some("on" | "off"))) { return Err(invalid()); }
            },
            "permissions" => {
                let permissions = value.as_object().ok_or_else(invalid)?;
                if permissions.len() != 1 { return Err(invalid()); }
                let deny = permissions.get("deny").and_then(Value::as_array).ok_or_else(invalid)?;
                items += deny.len();
                for rule in deny {
                    let name = rule.as_str().and_then(|rule| rule.strip_prefix("Skill(")).and_then(|rule| rule.strip_suffix(')')).ok_or_else(invalid)?;
                    let name = name.strip_suffix(" *").unwrap_or(name);
                    if !valid_skill(name) || !name.contains(':') { return Err(invalid()); }
                }
            },
            _ => return Err(invalid()),
        }
    }
    if items > MAX_ITEMS { return Err(invalid()); }
    Ok(())
}
fn store_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty()).ok_or_else(|| error(500, "claude_customizations_store_unavailable", "Diretório das escolhas da conversa indisponível."))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() { return Err(error(500, "claude_customizations_store_unavailable", "Diretório das escolhas da conversa indisponível.")); }
    Ok(home.join(".hangar/claude-customizations"))
}
fn save_settings(root: &Path, id: &str, settings: &Value) -> Result<()> {
    let key = session_key(id)?;
    restricted_settings(settings)?;
    let bytes = serde_json::to_vec(&json!({"settings":settings})).map_err(|_| invalid())?;
    if bytes.len() > MAX_BODY { return Err(invalid()); }
    let write = || -> io::Result<()> {
        fs::create_dir_all(root)?;
        let mut temp = tempfile::NamedTempFile::new_in(root)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(root.join(format!("{key}.json"))).map_err(|error| error.error)?;
        #[cfg(unix)] { fs::File::open(root)?.sync_all()?; }
        Ok(())
    };
    write().map_err(|_| error(500, "claude_customizations_write_failed", "Não deu para guardar as escolhas da conversa."))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSettings { settings: Value }
fn load_settings(root: &Path, id: &str) -> Result<Value> {
    let key = session_key(id)?;
    let Some(text) = read_source(&root.join(format!("{key}.json")))
        .map_err(|_| error(500, "claude_customizations_read_failed", "Não deu para ler as escolhas desta conversa."))? else { return Ok(json!({})); };
    let saved: SavedSettings = serde_json::from_str(&text)
        .map_err(|_| error(422, "claude_customizations_record_invalid", "As escolhas guardadas desta conversa estão inválidas."))?;
    restricted_settings(&saved.settings)
        .map_err(|_| error(422, "claude_customizations_record_invalid", "As escolhas guardadas desta conversa estão inválidas."))?;
    Ok(saved.settings)
}
fn merge_settings(mut saved: Value, delta: Value) -> Result<Value> {
    restricted_settings(&saved)?;
    restricted_settings(&delta)?;
    let saved_object = saved.as_object_mut().ok_or_else(invalid)?;
    for key in ["enabledPlugins", "skillOverrides"] {
        if let Some(values) = delta.get(key).and_then(Value::as_object) {
            saved_object.entry(key.to_owned()).or_insert_with(|| json!({}))
                .as_object_mut().ok_or_else(invalid)?.extend(values.clone());
        }
    }
    if let Some(deny) = delta.pointer("/permissions/deny").and_then(Value::as_array) {
        let mut rules: BTreeSet<String> = saved_object.get("permissions")
            .and_then(|permissions| permissions.get("deny")).and_then(Value::as_array)
            .into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        rules.extend(deny.iter().filter_map(Value::as_str).map(str::to_owned));
        saved_object.insert("permissions".into(), json!({"deny":rules}));
    }
    restricted_settings(&saved)?;
    Ok(saved)
}
fn settings_result(settings: Value) -> Value {
    json!({"settings":if settings.as_object().is_some_and(|settings| settings.is_empty()) { Value::Null } else { settings }})
}
fn execute(op: Operation, installations: Vec<Installation>, root: &Path) -> Result<Value> {
    match op {
        Operation::Catalog(ctx) => Ok(json!(catalog(&ctx.cwd, &ctx.config_dir, installations)?)),
        Operation::Prepare(args) => {
            session_key(&args.session_id)?;
            validate_context(&args.cwd, &args.config_dir)?;
            let settings = if let Some(selection) = args.selection {
                let delta = selection_settings(&selection)?;
                if !selection.is_empty() {
                    let catalog = catalog(&args.cwd, &args.config_dir, installations)?;
                    validate_targets(&selection, &catalog)?;
                }
                let settings = if args.resume { merge_settings(load_settings(root, &args.session_id)?, delta)? } else { delta };
                save_settings(root, &args.session_id, &settings)?;
                settings
            } else if args.resume { load_settings(root, &args.session_id)? } else { json!({}) };
            Ok(settings_result(settings))
        },
        Operation::Remember(args) => {
            save_settings(root, &args.session_id, &args.settings)?;
            Ok(settings_result(args.settings))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "12345678-1234-4123-8123-123456789abc";
    const SECOND: &str = "87654321-1234-4123-8123-123456789abc";

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn row(root: &Path, id: &str, enabled: bool) -> Installation {
        Installation { id: id.into(), version: "1.0".into(), scope: "user".into(), enabled,
            install_path: root.into(), read_from_folder: None, project_path: None, errors: vec![], notes: vec![] }
    }

    #[test]
    fn metadata_command_removes_private_environment_without_changing_account_credentials() {
        use std::ffi::OsStr;
        let secrets = ["HANGAR_INTERNAL_SECRET", "CP_AUTH_TOKEN", "HANGAR_RUNTIME_INSTANCE", "HANGAR_PLUGIN_TOKEN"];
        let mut command = tokio::process::Command::new("claude");
        for key in secrets { command.env(key, "must-not-inherit"); }
        command.env("ANTHROPIC_AUTH_TOKEN", "account-only");
        configure_plugin_command(&mut command, Path::new("/chosen-project"), Path::new("/chosen-account"));
        let env: BTreeMap<_, _> = command.as_std().get_envs().collect();
        for key in secrets { assert_eq!(env.get(OsStr::new(key)), Some(&None), "{key}"); }
        assert_eq!(env.get(OsStr::new("ANTHROPIC_AUTH_TOKEN")), Some(&Some(OsStr::new("account-only"))));
        assert_eq!(env.get(OsStr::new("CLAUDE_CONFIG_DIR")), Some(&Some(OsStr::new("/chosen-account"))));
        let args: Vec<_> = command.as_std().get_args().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(args, vec!["plugin", "list", "--json"]);
    }

    #[test]
    fn delta_only_emits_chosen_keys() {
        let selection: Selection = serde_json::from_value(json!({"plugins":{"one@market":false},
            "skills":{"falar":true},"blocked_skills":["tools:deploy","tools:deploy"]})).unwrap();
        assert_eq!(selection_settings(&selection).unwrap(), json!({"enabledPlugins":{"one@market":false},
            "skillOverrides":{"falar":"on"},"permissions":{"deny":["Skill(tools:deploy *)","Skill(tools:deploy)"]}}));
        assert_eq!(selection_settings(&Selection::default()).unwrap(), json!({}));
        let delta: Selection = serde_json::from_value(json!({"plugins":{"one@market":true}})).unwrap();
        assert_eq!(selection_settings(&delta).unwrap(), json!({"enabledPlugins":{"one@market":true}}));
        assert!(serde_json::from_value::<Selection>(json!({"env":{"TOKEN":"x"}})).is_err());
    }

    #[test]
    fn chosen_plugin_skill_emits_exact_and_argument_rules_without_name_prefix_wildcard() {
        let selection: Selection = serde_json::from_value(json!({"blocked_skills":["tools:deploy"]})).unwrap();
        let settings = selection_settings(&selection).unwrap();
        assert_eq!(settings, json!({"permissions":{"deny":["Skill(tools:deploy *)","Skill(tools:deploy)"]}}));
        restricted_settings(&settings).unwrap();
        assert!(restricted_settings(&json!({"permissions":{"deny":["Skill(tools:deploy*)"]}})).is_err());
        let mut policy = Policy::default();
        for rule in ["Skill(tools:deploy)", "Skill(tools:deploy *)"] {
            policy.deny = [rule.into()].into();
            assert!(policy.denied("tools:deploy"));
            assert!(!policy.denied("tools:deploy-extra"));
        }
        let dir = tempfile::tempdir().unwrap();
        save_settings(dir.path(), FIRST, &settings).unwrap();
        assert_eq!(load_settings(dir.path(), FIRST).unwrap(), settings);
    }

    #[test]
    fn records_are_isolated_and_resume_does_not_inherit_another_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let settings = json!({"permissions":{"deny":["Skill(tools:deploy)"]}});
        save_settings(dir.path(), FIRST, &settings).unwrap();
        assert_eq!(load_settings(dir.path(), FIRST).unwrap(), settings);
        assert_eq!(load_settings(dir.path(), SECOND).unwrap(), json!({}));
        save_settings(dir.path(), SECOND, &json!({"skillOverrides":{"falar":"off"}})).unwrap();
        assert_eq!(load_settings(dir.path(), FIRST).unwrap(), settings);
        save_settings(dir.path(), FIRST, &json!({})).unwrap();
        assert_eq!(settings_result(load_settings(dir.path(), FIRST).unwrap()), json!({"settings":null}));
    }

    #[test]
    fn resume_applies_only_delta_and_keeps_chosen_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project"); let account = dir.path().join("account");
        fs::create_dir_all(project.join(".git")).unwrap(); fs::create_dir_all(&account).unwrap();
        let root = dir.path().join("records");
        let settings = json!({"enabledPlugins":{"one@market":false},"skillOverrides":{"falar":"off"},
            "permissions":{"deny":["Skill(tools:deploy)"]}});
        save_settings(&root, FIRST, &settings).unwrap();
        let prepare = |resume, selection| Operation::Prepare(Prepare { cwd: project.clone(), config_dir: account.clone(),
            session_id: FIRST.into(), resume, selection });
        assert_eq!(execute(prepare(true, Some(Selection::default())), vec![], &root).unwrap(), json!({"settings":settings}));
        assert_eq!(execute(prepare(false, None), vec![], &root).unwrap(), json!({"settings":null}));
        assert_eq!(execute(prepare(true, None), vec![], &root).unwrap(), json!({"settings":settings}));
        let delta = json!({"enabledPlugins":{"one@market":true},"permissions":{"deny":["Skill(tools:review)"]}});
        assert_eq!(merge_settings(settings, delta).unwrap(), json!({"enabledPlugins":{"one@market":true},
            "skillOverrides":{"falar":"off"},"permissions":{"deny":["Skill(tools:deploy)","Skill(tools:review)"]}}));
    }

    #[test]
    fn project_scopes_use_common_git_directory_not_any_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main"); let linked = dir.path().join("linked");
        let other = dir.path().join("other"); let admin = dir.path().join("git-data/repo");
        fs::create_dir_all(admin.join("worktrees/w")).unwrap();
        fs::create_dir_all(dir.path().join("git-data/other")).unwrap();
        write(&main.join(".git"), &format!("gitdir: {}", admin.display()));
        write(&linked.join(".git"), &format!("gitdir: {}", admin.join("worktrees/w").display()));
        write(&admin.join("worktrees/w/commondir"), "../..");
        write(&admin.join("worktrees/w/gitdir"), &linked.join(".git").to_string_lossy());
        write(&other.join(".git"), &format!("gitdir: {}", dir.path().join("git-data/other").display()));
        assert_eq!(project_identity(&main).unwrap(), project_identity(&linked).unwrap());
        assert_ne!(project_identity(&main).unwrap(), project_identity(&other).unwrap());
        let mut applicable = row(dir.path(), "one@market", true);
        applicable.scope = "project".into(); applicable.project_path = Some(main);
        let mut irrelevant = row(dir.path(), "two@market", true);
        irrelevant.scope = "local".into(); irrelevant.project_path = Some(other);
        let selected = effective_installations(vec![applicable, irrelevant], &linked, &mut vec![]).unwrap();
        assert_eq!(selected.len(), 1); assert_eq!(selected[0].id, "one@market");
        let mut ancestral = row(dir.path(), "old@market", true);
        ancestral.scope = "project".into(); ancestral.project_path = Some(dir.path().into());
        assert!(effective_installations(vec![ancestral], &linked, &mut vec![]).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_sources_cannot_hold_a_catalog_slot() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("SKILL.md");
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(read_source(&pipe).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_source(dir.path()).is_err());
    }

    #[test]
    fn malicious_input_is_rejected_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-created");
        for settings in [json!({"env":{"KEY":"secret"}}), json!({"permissions":{"allow":[]}}),
            json!({"model":"opus"}), json!({"hooks":{}}), json!({"skillOverrides":{"x)":"off"}}),
            json!({"permissions":{"deny":["Skill(tools:deploy*)"]}}),
            json!({"permissions":{"deny":["Skill(tools:deploy  *)"]}})] {
            assert!(save_settings(&root, FIRST, &settings).is_err());
        }
        assert!(save_settings(&root, "../../settings", &json!({})).is_err());
        assert!(!root.exists());
        for name in ["x/y", "x y", "x(*)", "x\\y", "x\n", "x:*", ":bad"] {
            assert!(!valid_skill(name));
        }
    }

    #[test]
    fn scope_deduplication_rejects_unknown_effective_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = row(dir.path(), "same@market", true);
        project.scope = "project".into(); project.project_path = Some(dir.path().into());
        let user = row(dir.path(), "same@market", true);
        assert_eq!(effective_installations(vec![user.clone(), project.clone()], dir.path(), &mut vec![]).unwrap().len(), 1);
        project.version = "2.0".into();
        let mut warnings = vec![];
        let selected = effective_installations(vec![user.clone(), project.clone()], dir.path(), &mut warnings).unwrap();
        assert_eq!(selected[0].version, "1.0");
        assert_eq!(warnings, vec!["Há várias instalações; vale a primeira disponível na ordem do Claude (same@market)."]);
        project.enabled = false;
        assert_eq!(effective_installations(vec![user, project], dir.path(), &mut vec![]).unwrap_err().code, "claude_plugins_ambiguous");
        let mut foreign = row(dir.path(), "other@market", true);
        foreign.scope = "local".into(); foreign.project_path = Some(dir.path().join("other"));
        assert!(effective_installations(vec![foreign], dir.path(), &mut vec![]).unwrap().is_empty());
    }

    #[test]
    fn catalog_uses_manifest_namespace_yaml_and_inherited_restrictions() {
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().join("account"); let project = dir.path().join("project"); let plugin = dir.path().join("plugin");
        fs::create_dir_all(project.join(".git")).unwrap();
        write(&account.join("settings.json"), r#"{"enabledPlugins":{"alias@market":true,"other@market":true},"skillOverrides":{"falar":"off"},"permissions":{"deny":["Skill(tools:deploy *)"]}}"#);
        write(&project.join(".claude/settings.json"), r#"{"enabledPlugins":{"alias@market":false},"skillOverrides":{"falar":"on"}}"#);
        write(&project.join(".claude/settings.local.json"), r#"{"skillOverrides":{"falar":"off"}}"#);
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"tools","description":"Ferramentas","skills":["./extras"],"commands":"./extra-commands"}"#);
        write(&plugin.join("skills/deploy/SKILL.md"), "\u{feff}---\r\nname: deploy\r\ndescription: >-\r\n  Enviar a\r\n  aplicação\r\n---\r\ncorpo");
        write(&plugin.join("extras/review/SKILL.md"), "---\nname: review\ndescription: |\n  Linha um\n  Linha dois\n---\ncorpo");
        write(&plugin.join("extra-commands/status.md"), "---\nname: ignored\ndescription: Ver estado\n---\ncorpo");
        write(&plugin.join("commands/ignored.md"), "ignorado");
        write(&account.join("skills/falar/SKILL.md"), "---\ndescription: Conta\n---\ncorpo");
        write(&project.join(".claude/skills/falar/SKILL.md"), "---\ndescription: Projeto\n---\ncorpo");
        let policy = Policy::load(&account, &[project.clone()]).unwrap();
        assert_eq!(policy.enabled_plugins.get("alias@market"), Some(&false));
        assert_eq!(policy.enabled_plugins.get("other@market"), Some(&true));
        let result = catalog(&project, &account, vec![row(&plugin, "alias@market", false)]).unwrap();
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert!(!result.plugins[0].enabled);
        assert_eq!(result.plugins[0].name, "tools");
        assert_eq!(result.plugins[0].skills.len(), 3);
        let deploy = result.plugins[0].skills.iter().find(|skill| skill.name == "tools:deploy").unwrap();
        assert_eq!(deploy.description, "Enviar a aplicação"); assert!(!deploy.enabled);
        assert_eq!(result.skills[0].description, "Projeto"); assert!(!result.skills[0].enabled);
        assert!(!result.skills[0].blocked);
        let selection: Selection = serde_json::from_value(json!({"skills":{"falar":true}})).unwrap();
        validate_targets(&selection, &result).unwrap();
        write(&account.join("settings.json"), r#"{"permissions":{"deny":["Skill(falar)"]},"skillOverrides":{"tools:review":"off"}}"#);
        let result = catalog(&project, &account, vec![row(&plugin, "alias@market", false)]).unwrap();
        assert!(result.skills[0].blocked);
        assert_eq!(validate_targets(&selection, &result).unwrap_err().code, "claude_skill_restricted");
        assert!(result.plugins[0].skills.iter().find(|skill| skill.name == "tools:review").unwrap().enabled);
    }

    #[test]
    fn unreadable_settings_and_bad_yaml_are_not_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("settings.json"), "not json");
        assert_eq!(Policy::load(dir.path(), &[]).err().unwrap().code, "claude_settings_invalid");
        assert!(frontmatter("---\ndescription: [\n---\nbody").is_err());
        let mut policy = Policy::default();
        for rule in ["Skill", "Skill(*)", "Skill(tools:*)", "Skill(tools:deploy *)"] {
            policy.deny = [rule.into()].into();
            assert!(policy.denied("tools:deploy"));
        }
    }
}
