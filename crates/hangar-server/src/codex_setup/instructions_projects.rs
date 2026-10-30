//! Escopos do projeto e exclusão local dos aliases no Git.
use super::{files, instructions::Links, instructions_alias};
use crate::accounts::environment::AccountEnvironment;
use files::Error;
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn records(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            result.push(path);
        }
    }
    Ok(result)
}

pub(super) async fn prepare(
    env: &AccountEnvironment,
    codex: &Path,
    cwd: Option<&Path>,
    links: &impl Links,
) -> Result<(), Error> {
    let store = codex.join(".hangar-instrucoes");
    let mut projects = BTreeSet::new();
    if let Some(cwd) = cwd {
        projects.insert(cwd.to_owned());
    } else {
        let config = codex.join("config.toml");
        if config.exists() {
            let config = crate::config_sync::runtime_values::ValueGuard(
                super::instructions_toml::read(&config)?,
            );
            if let Some(table) = config
                .get("projects")
                .and_then(serde_json::Value::as_object)
            {
                projects.extend(table.keys().map(PathBuf::from));
            }
        }
        for path in records(&store)? {
            let record = crate::config_sync::runtime_values::ValueGuard(files::object(&path)?);
            if let Some(target) = record["alvo"].as_str().filter(|target| !target.is_empty()) {
                if let Some(parent) = Path::new(target).parent().filter(|parent| *parent != codex) {
                    projects.insert(parent.to_owned());
                }
            }
        }
    }
    let mut scopes = BTreeSet::new();
    for project in projects
        .into_iter()
        .filter(|project| project.is_absolute() && project.is_dir())
    {
        let project = fs::canonicalize(project)?;
        let root = project
            .ancestors()
            .find(|path| path.join(".git").exists())
            .unwrap_or(&project);
        for path in project.ancestors() {
            scopes.insert(path.to_owned());
            if path == root {
                break;
            }
        }
    }
    for directory in scopes {
        let source = instructions_alias::source(&directory)?;
        instructions_alias::apply(
            &directory.join("AGENTS.override.md"),
            source.as_deref(),
            &store,
            &env.home,
            links,
        )?;
        if source.is_some() {
            exclude(env, &directory).await?;
        }
    }
    Ok(())
}

async fn exclude(env: &AccountEnvironment, directory: &Path) -> Result<(), Error> {
    let result = crate::command_capture::capture(
        vec![
            "git".into(),
            "-C".into(),
            directory.to_string_lossy().into_owned(),
            "rev-parse".into(),
            "--git-path".into(),
            "info/exclude".into(),
        ],
        env.clone(),
        Duration::from_secs(5),
    )
    .await;
    let Ok(result) = result else {
        return Ok(());
    };
    let path = crate::transcript::py::strip(&result.stdout);
    if !result.success || path.is_empty() {
        return Ok(());
    }
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    };
    let write = || -> Result<(), std::io::Error> {
        let text = if path.exists() {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines = crate::transcript::py::splitlines(&text);
        if lines.contains(&"AGENTS.override.md") {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        if !lines.is_empty() && lines.last() != Some(&"") {
            file.write_all(b"\n")?;
        }
        file.write_all(b"AGENTS.override.md\n")
    };
    if let Err(error) = write() {
        if error.kind() == std::io::ErrorKind::InvalidData {
            return Err(error.into());
        }
    }
    Ok(())
}
