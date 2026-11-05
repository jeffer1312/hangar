//! Autoria do alias registrada antes da troca, para permitir retomada após interrupção.
use super::files;
use crate::accounts::environment::AccountEnvironment;
use files::Error;
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(source, target)
    }
}

fn resolve(path: &Path) -> Result<PathBuf, Error> {
    super::plugins::resolved(path).map_err(Error::Invalid)
}

pub(super) fn source(directory: &Path) -> Result<Option<PathBuf>, Error> {
    for name in ["CLAUDE.md", "CLAUDE.MD"] {
        let path = directory.join(name);
        if path.is_file() && !crate::transcript::py::strip(&fs::read_to_string(&path)?).is_empty() {
            return Ok(Some(std::path::absolute(path)?));
        }
    }
    Ok(None)
}

fn lines(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (offset, character) in text.char_indices() {
        if matches!(
            character,
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            let end = offset + character.len_utf8();
            result.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}

fn text(path: &Path) -> Result<String, Error> {
    Ok(fs::read_to_string(path)?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

fn render(source: &Path, env: &AccountEnvironment) -> Result<Vec<u8>, Error> {
    struct Frame {
        path: PathBuf,
        lines: Vec<String>,
        next: usize,
        output: String,
    }
    let frame = |path: &Path| -> Result<Frame, Error> {
        Ok(Frame {
            path: path.into(),
            lines: lines(&text(path)?).into_iter().map(str::to_owned).collect(),
            next: 0,
            output: String::new(),
        })
    };
    let mut stack = vec![frame(source)?];
    let mut seen = BTreeSet::from([resolve(source)?]);
    loop {
        let current = stack.last_mut().unwrap();
        if current.next == current.lines.len() {
            let finished = stack.pop().unwrap();
            seen.remove(&resolve(&finished.path)?);
            if let Some(parent) = stack.last_mut() {
                parent.output.push_str(&finished.output);
                if !finished.output.ends_with('\n') {
                    parent.output.push('\n');
                }
                continue;
            }
            return Ok(finished.output.into_bytes());
        }
        let line = current.lines[current.next].clone();
        current.next += 1;
        if let Some(path) =
            super::instructions::imported(&line, current.path.parent().unwrap(), env)
        {
            let resolved = resolve(&path)?;
            if !seen.contains(&resolved) {
                let child = frame(&path)?;
                seen.insert(resolved);
                stack.push(child);
                continue;
            }
        }
        stack.last_mut().unwrap().output.push_str(&line);
    }
}

fn relative(source: &Path, parent: &Path) -> PathBuf {
    let source: Vec<_> = source.components().collect();
    let parent: Vec<_> = parent.components().collect();
    let common = source
        .iter()
        .zip(&parent)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return source.iter().collect();
    }
    let mut result = PathBuf::new();
    for _ in common..parent.len() {
        result.push("..");
    }
    for part in &source[common..] {
        result.push(part);
    }
    result
}

pub(super) fn apply(
    target: &Path,
    source: Option<&Path>,
    records: &Path,
    env: &AccountEnvironment,
) -> Result<(), Error> {
    let record = records.join(format!(
        "{}.json",
        files::hash(target.to_string_lossy().as_bytes())
    ));
    let previous = crate::config_sync::runtime_values::ValueGuard(files::object(&record)?);
    if source.is_none() && previous.as_object().is_some_and(|value| value.is_empty()) {
        return Ok(());
    }
    let exists = target.exists() || target.is_symlink();
    let raw = if target.exists() {
        Some(fs::read(target)?)
    } else {
        None
    };
    let link = if target.is_symlink() {
        Some(fs::read_link(target)?)
    } else {
        None
    };
    let resolved = resolve(target)?;
    let owned = [&previous.0, &previous["anterior"]]
        .into_iter()
        .any(|state| {
            (link.is_some() && state["fonte"].as_str() == resolved.to_str())
                || (link.is_none()
                    && raw.as_ref().is_some_and(|raw| {
                        state["hash"].as_str() == Some(files::hash(raw).as_str())
                    }))
        });
    if exists
        && !owned
        && link.is_some()
        && source.is_some_and(|source| resolve(source).ok().as_ref() == Some(&resolved))
    {
        return Ok(());
    }
    let Some(source) = source else {
        if exists && !owned {
            return Err(Error::Invalid(format!(
                "Instruções pessoais preservadas em {}; não é um alias gerenciado.",
                target.display()
            )));
        }
        if owned {
            fs::remove_file(target)?;
        }
        match fs::remove_file(record) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    };
    let data = render(source, env)?;
    let mirror = data == fs::read(source)?;
    if owned && mirror && link.is_some() && resolved == resolve(source)? {
        return Ok(());
    }
    if owned && link.is_none() && raw.as_deref() == Some(data.as_slice()) {
        return Ok(());
    }
    let new = json!({"alvo":target,"fonte":resolve(source)?,"hash":files::hash(&data)});
    let parent = target.parent().unwrap();
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".hangar-instrucoes-")
        .tempfile_in(parent)?
        .into_temp_path();
    fs::remove_file(&temporary)?;
    if !mirror || symlink(&relative(source, parent), &temporary).is_err() {
        fs::write(&temporary, &data)?;
    }
    if exists && !owned {
        files::backup(
            target,
            raw.as_deref().unwrap_or_default(),
            &records
                .join("backups")
                .join(files::hash(raw.as_deref().unwrap_or_default())),
        )?;
    }
    let mut transition = new.clone();
    transition["anterior"] = json!({"fonte":previous["fonte"],"hash":previous["hash"]});
    let transition = files::bytes(&transition)?;
    files::write(&record, &transition, files::read(&record)?.as_deref(), None)?;
    let current_raw = if target.exists() {
        Some(fs::read(target)?)
    } else {
        None
    };
    let current_link = if target.is_symlink() {
        Some(fs::read_link(target)?)
    } else {
        None
    };
    if (target.exists() || target.is_symlink()) != exists
        || current_raw != raw
        || current_link != link
    {
        return Err(Error::Invalid(format!(
            "Instruções alteradas durante a integração: {}",
            target.display()
        )));
    }
    files::replace_preserving(&temporary, target, raw.as_deref(), link.as_deref())?;
    files::write(&record, &files::bytes(&new)?, Some(&transition), None)?;
    Ok(())
}
