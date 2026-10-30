//! Escopos do projeto e exclusão local dos aliases no Git.
use super::{files, instructions_alias};
use crate::accounts::environment::AccountEnvironment;
use files::Error;
use serde_json::Value;
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
        if super::files::matches_json_record(&path, cfg!(windows)) {
            result.push(path);
        }
    }
    Ok(result)
}

pub(super) async fn prepare(
    env: &AccountEnvironment,
    codex: &Path,
    cwd: Option<&Path>,
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
            if let Some(value) = config.get("projects") {
                match value {
                    Value::Object(table) => projects.extend(table.keys().map(PathBuf::from)),
                    Value::Array(items) => {
                        for item in items {
                            projects.insert(project_path(item)?);
                        }
                    }
                    Value::String(text) => {
                        projects.extend(text.chars().map(|ch| PathBuf::from(ch.to_string())))
                    }
                    _ => {
                        return Err(Error::Invalid(format!(
                            "'{}' object is not iterable",
                            type_name(value)
                        )));
                    }
                }
            }
        }
        for path in records(&store)? {
            let record = crate::config_sync::runtime_values::ValueGuard(files::object(&path)?);
            if crate::orchestration::truthy(&record["alvo"]) {
                let target = project_path(&record["alvo"])?;
                if let Some(parent) = target.parent().filter(|parent| *parent != codex) {
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
        if source.is_some() {
            exclude(env, &directory).await?;
        }
        instructions_alias::apply(
            &directory.join("AGENTS.override.md"),
            source.as_deref(),
            &store,
            env,
        )?;
    }
    Ok(())
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn project_path(value: &Value) -> Result<PathBuf, Error> {
    value.as_str().map(PathBuf::from).ok_or_else(|| Error::Invalid(format!(
        "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{}'", type_name(value)
    )))
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
    write()?;
    Ok(())
}

#[cfg(all(test,unix))]
#[tokio::test]
async fn exclude_write_failure_is_visible_before_alias_publication() {
 use std::os::unix::fs::PermissionsExt;
 let dir=tempfile::tempdir().unwrap();let bin=dir.path().join("bin");fs::create_dir(&bin).unwrap();let script=bin.join("git");
 fs::write(&script,"#!/bin/sh\nprintf '.'\n").unwrap();fs::set_permissions(script,fs::Permissions::from_mode(0o700)).unwrap();
 let env=AccountEnvironment::from_pairs([("HOME",dir.path().to_string_lossy().into_owned()),("PATH",bin.to_string_lossy().into_owned())]);
 assert!(exclude(&env,dir.path()).await.is_err());assert!(!dir.path().join("AGENTS.override.md").exists());
}
