//! Identidade comum às cópias; a raiz de execução continua sendo a cópia da sessão.
use crate::{orchestration::codec::{self, ValueGuard}, routes::AppState};
use axum::response::Response;
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::Mutex, time::Duration};

// A lista inteira é um arquivo só; serializa também projetos diferentes para não perder PUTs.
static STORE: Mutex<()> = Mutex::new(());

pub(super) fn error(status: u16, code: &str, message: &str) -> Response {
    super::guest::error(status, code, message, json!({"detalhe":message}))
}
fn project_error(message: &str) -> Response { error(409, "erro_project_shortcuts_projeto", message) }
fn file_error(path: &Path, message: impl std::fmt::Display) -> Response {
    error(500, "erro_project_shortcuts_arquivo", &format!("{}: {message}", path.display()))
}

pub(super) async fn describe(st: &AppState, cwd: &str) -> Result<Value, Response> {
    let base = Path::new(cwd);
    if !base.is_dir() { return Err(project_error(&format!("pasta da sessao nao existe: {cwd}"))); }
    let env = st.accounts.env.base.clone();
    // O comum (repositório e worktree) sai da `.git` sem abrir o git, que no Windows custa dezenas de ms.
    let physical = hangar_workspace::real(base);
    if !env.keys().any(|name| hangar_workspace::repo_layout::DISCOVERY_VARS.contains(&name.as_str())) {
        let found = { let base = physical.clone(); tokio::task::spawn_blocking(move || hangar_workspace::repo_layout::layout(&base)).await };
        match found {
            Ok(hangar_workspace::repo_layout::Layout::Repo { common, toplevel }) => {
                let common = hangar_workspace::real(&common);
                let common = if common.file_name().is_some_and(|n| n == ".git") { common.parent().unwrap_or(&common).to_path_buf() } else { common };
                return Ok(identity(hangar_workspace::real(&common), hangar_workspace::real(&toplevel)));
            }
            Ok(hangar_workspace::repo_layout::Layout::NotRepo) => return Ok(identity(physical.clone(), physical)),
            _ => {}
        }
    }
    let directory = cwd.to_owned();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = hangar_workspace::process::git_command(Some(&env));
        command.args(["-C", &directory, "rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"]);
        hangar_workspace::process::run_program(&mut command, Duration::from_secs(3))
    }).await;
    let (paths, failure) = match output {
        Ok(Ok(output)) => {
            let lines: Vec<_> = output.stdout.trim().lines().map(str::to_owned).collect();
            (if output.code == 0 && lines.len() == 2 { Some(lines) } else { None }, output.stderr.trim().to_owned())
        }
        Ok(Err(e)) => (None, e.detail.as_str().unwrap_or("git falhou").replace("git não encontrado", "git nao encontrado")),
        Err(_) => return Err(project_error("git falhou")),
    };
    let (common, root) = if let Some(paths) = paths {
        let common = paths[0].strip_suffix("/.git").or_else(|| paths[0].strip_suffix("\\.git")).unwrap_or(&paths[0]);
        (hangar_workspace::real(Path::new(common)), hangar_workspace::real(Path::new(&paths[1])))
    } else {
        if physical.ancestors().any(|p| p.join(".git").exists()) {
            return Err(project_error(&format!("git falhou em {cwd}: {}", if failure.is_empty() { "sem resposta" } else { &failure })));
        }
        (physical.clone(), physical)
    };
    Ok(identity(common, root))
}

fn identity(common: PathBuf, root: PathBuf) -> Value {
    let key = common.to_string_lossy().into_owned();
    #[cfg(windows)]
    let key = key.replace('/', "\\").to_lowercase();
    let name = common.file_name().filter(|v| !v.is_empty()).unwrap_or(common.as_os_str()).to_string_lossy();
    json!({"key":key,"name":name,"root":root})
}

pub(super) async fn folder(st: &AppState, cwd: &str, requested: Option<&str>) -> Result<PathBuf, Response> {
    let Some(requested) = requested else { return Ok(PathBuf::from(cwd)); };
    let requested = Path::new(crate::transcript::py::strip(requested));
    let path = if requested.is_absolute() { requested.to_owned() } else {
        let project = describe(st, cwd).await?;
        Path::new(project["root"].as_str().unwrap()).join(requested)
    };
    if !path.is_dir() { return Err(error(400, "erro_shortcut_pasta", &format!("pasta nao existe: {}", path.display()))); }
    Ok(path)
}

fn path(st: &AppState) -> Result<PathBuf, Response> {
    let unavailable = |_| super::guest::error(503,"shortcut_context_unavailable","contexto do terminal indisponível",json!({}));
    let settings = st.settings_snapshot().map_err(unavailable)?;
    let projects = settings.value("CP_PROJECTS_DIR").map_err(unavailable)?.map(PathBuf::from)
        .unwrap_or_else(||st.accounts.env.claude_base.join("projects"));
    Ok(projects.parent().unwrap_or(&projects).join(".hangar-project-shortcuts.json"))
}
fn read(path: &Path) -> Result<ValueGuard, Response> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ValueGuard(json!({}))),
        Err(e) => return Err(file_error(path, e)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        let start = e.valid_up_to();
        let message = if let Some(length) = e.error_len() {
            let reason = if bytes[start] < 0xc2 || bytes[start] > 0xf4 { "invalid start byte" } else { "invalid continuation byte" };
            if length == 1 { format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}", bytes[start]) }
            else { format!("'utf-8' codec can't decode bytes in position {start}-{}: {reason}", start + length - 1) }
        } else { format!("'utf-8' codec can't decode bytes in position {start}-{}: unexpected end of data", bytes.len()-1) };
        file_error(path, message)
    })?;
    let value = codec::loads(text).ok_or_else(|| {
        let diagnostic = crate::orchestration::body::parse(&bytes).err().map(|(_,v)| ValueGuard(v));
        let issue = diagnostic.as_ref().map(|v| &v["detail"][0]);
        let position = issue.and_then(|v| v["loc"][1].as_u64()).unwrap_or(0) as usize;
        let before: String = text.chars().take(position).collect();
        let line = before.chars().filter(|c| *c == '\n').count()+1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count()+1;
        file_error(path, format!("{}: line {line} column {column} (char {position})", issue.and_then(|v|v["ctx"]["error"].as_str()).unwrap_or("Expecting value")))
    })?;
    let value = ValueGuard(value);
    if !value.is_object() { return Err(file_error(path, "esperado um objeto JSON")); }
    Ok(value)
}
fn validate(items: &Value) -> Result<(), String> {
    let items = items.as_array().ok_or("items: esperado uma lista de atalhos")?;
    let mut seen = std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let place = format!("item {}", index+1);
        if item["type"] == "internal" { return Err(format!("{place}: atalho interno nao vale por projeto")); }
        crate::config_sync::preferences::validate_shortcut(item, &place)?;
        if item["type"] == "shell" && !item["pasta"].is_null()
            && !item["pasta"].as_str().is_some_and(|v| !crate::transcript::py::strip(v).is_empty()) {
            return Err(format!("{place} (shell) com pasta vazia"));
        }
        let id = item["id"].as_str().unwrap();
        if !seen.insert(id) { return Err(format!("{place}: id '{id}' repetido")); }
    }
    Ok(())
}

pub(super) fn items(st: &AppState, project: Value, items: Option<Value>) -> Result<Value, Response> {
    let mut project = ValueGuard(project);
    let items = items.map(ValueGuard);
    if let Some(items) = &items { validate(items).map_err(|e| error(400, "erro_project_shortcuts", &e))?; }
    let _lock = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let path = path(st)?;
    let mut data = read(&path)?;
    let key = project["key"].as_str().unwrap();
    if let Some(items) = items {
        if items.as_array().unwrap().is_empty() {
            if let Some(old) = data.as_object_mut().unwrap().remove(key) { codec::discard(old); }
        } else {
            let mut entry = json!({});
            entry["items"] = codec::clone_value(&items);
            if let Some(old) = data.as_object_mut().unwrap().insert(key.into(), entry) { codec::discard(old); }
        }
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| file_error(&path,e))?;
        let encoded = codec::dumps_utf8(&data).map_err(|e| file_error(&path,e))?;
        crate::groups::store::replace_atomic(&path, encoded.as_bytes()).map_err(|e| file_error(&path,e))?;
    }
    project["items"] = data[key]["items"].as_array().map(|_|codec::clone_value(&data[key]["items"])).unwrap_or_else(||json!([]));
    Ok(std::mem::take(&mut project.0))
}
