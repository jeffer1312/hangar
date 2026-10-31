//! Prepara somente as fontes de agentes e memória reconhecidas pelo importador nativo.
use super::{fingerprints, phase::Error};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::OnceLock,
};
fn markdown(path: &Path) -> bool {
    path.extension().is_some_and(|v| {
        if cfg!(windows) {
            v.to_string_lossy().eq_ignore_ascii_case("md")
        } else {
            v == "md"
        }
    })
}
pub fn agents(
    home: &Path,
    settings: &Value,
) -> Result<(BTreeMap<String, String>, Vec<PathBuf>), Error> {
    let mut result = BTreeMap::new();
    let mut unreadable = Vec::new();
    static NAME: OnceLock<regex::Regex> = OnceLock::new();
    let name = NAME.get_or_init(|| {
        regex::Regex::new(r#"(?m)^name:[ \t]*['\"]?([^'\"\s\x1c-\x1f][^'\"\n]*?)['\"]?[ \t]*$"#)
            .unwrap()
    });
    for (plugin, directory) in fingerprints::plugin_agents(home, settings)? {
        let mut entries = match fs::read_dir(&directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| markdown(p))
                .collect::<Vec<_>>(),
            Err(_) => continue,
        };
        entries.sort();
        for path in entries {
            let raw = match fs::read(&path) {
                Ok(v) => v,
                Err(_) => {
                    unreadable.push(path);
                    continue;
                }
            };
            let text = match String::from_utf8(raw) {
                Ok(v) => v,
                Err(_) => {
                    unreadable.push(path);
                    continue;
                }
            };
            let text = text
                .strip_prefix('\u{feff}')
                .unwrap_or(&text)
                .replace("\r\n", "\n")
                .replace('\r', "\n");
            if !text.starts_with("---\n") {
                continue;
            }
            let Some(end) = text[4..].find("\n---").map(|n| n + 4) else {
                continue;
            };
            let Some(found) = name.captures(&text[4..end]).and_then(|c| c.get(1)) else {
                continue;
            };
            let id = format!("{plugin}_{}", found.as_str())
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let mut rewritten = text.clone();
            rewritten.replace_range(found.start() + 4..found.end() + 4, &id);
            result.insert(id, rewritten);
        }
    }
    Ok((result, unreadable))
}
fn copy_stat(source: &Path, target: &Path) -> io::Result<()> {
    let meta = fs::metadata(source)?;
    crate::accounts::claude_config::copy_stat(target, &meta)
        .map_err(|error| io::Error::other(error.to_string()))
}
fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
    if !fs::metadata(source)?.is_file() {
        return Err(io::Error::other("Fonte de memória não é um arquivo comum"));
    }
    fs::copy(source, target)?;
    copy_stat(source, target)
}
pub(super) fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    enum Work {
        Enter(PathBuf, PathBuf, Vec<PathBuf>),
        Finish(PathBuf, PathBuf),
    }
    let mut pending = vec![Work::Enter(
        source.to_path_buf(),
        target.to_path_buf(),
        vec![],
    )];
    while let Some(work) = pending.pop() {
        match work {
            Work::Finish(source, target) => copy_stat(&source, &target)?,
            Work::Enter(source, target, mut ancestors) => {
                let real = fs::canonicalize(&source)?;
                if ancestors.contains(&real) {
                    return Err(io::Error::other("Link circular na fonte de memória"));
                }
                ancestors.push(real);
                fs::create_dir(&target)?;
                pending.push(Work::Finish(source.clone(), target.clone()));
                for entry in fs::read_dir(source)? {
                    let entry = entry?;
                    if entry.file_type()?.is_symlink() { continue; }
                    let name = entry.file_name();
                    let text = name.to_string_lossy();
                    if ["__pycache__", ".git"].contains(&text.as_ref()) || text.ends_with(".pyc") {
                        continue;
                    }
                    let from = entry.path();
                    let to = target.join(name);
                    if from.is_dir() {
                        pending.push(Work::Enter(from, to, ancestors.clone()));
                    } else {
                        copy_file(&from, &to)?;
                    }
                }
            }
        }
    }
    Ok(())
}
fn conversation(path: &Path) -> io::Result<bool> {
    let input = io::BufReader::new(fs::File::open(path)?);
    for line in input.split(b'\n') {
        let line = line?;
        if let Ok(value) = crate::orchestration::body::parse(&line) {
            let value = crate::config_sync::runtime_values::ValueGuard(value);
            if value.get("type").and_then(Value::as_str) == Some("user") { return Ok(true); }
        }
    }
    Ok(false)
}
pub fn copy_memory(source: &Path, target: &Path) -> Vec<String> {
    if source.is_symlink() || !source.is_dir() {
        return vec![];
    }
    let mut errors = Vec::new();
    let mut projects = match fs::read_dir(source) {
        Ok(v) => v
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect::<Vec<_>>(),
        Err(_) => return errors,
    };
    projects.sort();
    for project in projects {
        let memory = project.join("memory");
        if project.is_symlink() || memory.is_symlink() || !memory.is_dir() {
            continue;
        }
        let Some(name) = project.file_name() else {
            continue;
        };
        let destination = target.join(name);
        let copied = (|| -> io::Result<()> {
            let entries = match fs::read_dir(&memory) {
                Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
                Err(_) => return Ok(()),
            };
            if !entries.iter().any(|e| markdown(&e.path())) {
                return Ok(());
            }
            let mut transcripts = Vec::new();
            let entries = match fs::read_dir(&project) {
                Ok(entries) => entries,
                Err(_) => return Ok(()),
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_symlink() && path.extension().is_some_and(|v| {
                    if cfg!(windows) {
                        v.to_string_lossy().eq_ignore_ascii_case("jsonl")
                    } else {
                        v == "jsonl"
                    }
                }) {
                    transcripts.push((fs::metadata(&path)?.len(), path));
                }
            }
            transcripts.sort_by_key(|(size, _)| *size);
            let mut selected = None;
            for (_, path) in transcripts {
                if conversation(&path)? {
                    selected = Some(path);
                    break;
                }
            }
            let Some(transcript) = selected else {
                return Ok(());
            };
            fs::create_dir_all(&destination)?;
            copy_tree(&memory, &destination.join("memory"))?;
            copy_file(
                &transcript,
                &destination.join(transcript.file_name().unwrap()),
            )?;
            Ok(())
        })();
        if copied.is_err() {
            let _ = fs::remove_dir_all(&destination);
            errors.push(name.to_string_lossy().into_owned());
        }
    }
    errors
}

fn excluded(command: &Value) -> bool {
    let Some(command) = command.as_str() else {
        return false;
    };
    let command = command.replace('\\', "/");
    if command.contains("backend/hooks/") {
        return true;
    }
    static GUARD: OnceLock<regex::Regex> = OnceLock::new();
    static CLAUDE: OnceLock<regex::Regex> = OnceLock::new();
    GUARD.get_or_init(||regex::Regex::new(r#"(?:^|/)guard_tmux\.py(?:["'\s\x1c-\x1f;]|$)"#).unwrap()).is_match(&command)
 || CLAUDE.get_or_init(||regex::Regex::new(r#"(?:^|[/"'\s\x1c-\x1f])(?:skill-suggester|jev-command-gate|jev-answer-check)\.py(?:["'\s\x1c-\x1f;|&]|$)"#).unwrap()).is_match(&command)
}
pub fn without_app_hooks(value: &Value) -> Value {
    use crate::orchestration::codec;
    let Some(events) = value.as_object() else {
        return codec::clone_value(value);
    };
    let mut out = serde_json::Map::new();
    for (event, groups) in events {
        let Some(groups) = groups.as_array() else {
            out.insert(event.clone(), codec::clone_value(groups));
            continue;
        };
        let mut remaining = Vec::new();
        for group in groups {
            let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                remaining.push(codec::clone_value(group));
                continue;
            };
            let own: Vec<_> = hooks
                .iter()
                .filter(|hook| !hook.get("command").is_some_and(excluded))
                .map(codec::clone_value)
                .collect();
            if !own.is_empty() || hooks.is_empty() {
                let mut next = codec::clone_value(group);
                if let Some(old) = next
                    .as_object_mut()
                    .unwrap()
                    .insert("hooks".into(), Value::Array(own))
                {
                    codec::discard(old);
                }
                remaining.push(next);
            }
        }
        if !remaining.is_empty() {
            out.insert(event.clone(), Value::Array(remaining));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn transcript_requires_a_top_level_user_message() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("transcript.jsonl");
        for raw in [r#"{"type":"assistant","nested":{"type":"user"}}"#,r#"{"type":"assistant","text":"user"}"#,"not JSON"] {
            fs::write(&path,raw).unwrap();assert!(!conversation(&path).unwrap());
        }
        fs::write(&path,"{\"type\" : \"user\"}\n").unwrap();assert!(conversation(&path).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn memory_copy_skips_links_without_touching_their_targets() {
        let dir=tempfile::tempdir().unwrap();let source=dir.path().join("source");let target=dir.path().join("target");
        fs::create_dir(&source).unwrap();let outside=dir.path().join("private.md");fs::write(&outside,"private").unwrap();
        fs::write(source.join("public.md"),"public").unwrap();std::os::unix::fs::symlink(&outside,source.join("linked.md")).unwrap();
        std::os::unix::fs::symlink(dir.path(),source.join("linked-directory")).unwrap();
        copy_tree(&source,&target).unwrap();assert_eq!(fs::read(target.join("public.md")).unwrap(),b"public");
        assert!(!target.join("linked.md").exists());assert!(!target.join("linked-directory").exists());assert_eq!(fs::read(outside).unwrap(),b"private");
    }
}
