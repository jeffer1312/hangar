//! As citações autorizam o arquivo; coincidência com um sufixo não autoriza outro caminho.
use crate::{Result, error, real};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

fn lines(path: &Path, mut callback: impl FnMut(&str)) {
    if let Ok(file) = std::fs::File::open(path) {
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => callback(&String::from_utf8_lossy(&buffer)),
            }
        }
    }
}
pub fn cwds(jsonl: &Path, needles: &[String]) -> Value {
    let mut wanted = needles.iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
    wanted.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    wanted.dedup();
    let mut seen = HashSet::new();
    let mut folders = HashMap::<String, Vec<String>>::new();
    lines(jsonl, |line| {
        if !wanted.iter().any(|s| line.contains(s.as_str())) {
            return;
        }
        let mut matched = HashSet::new();
        for (pos, _) in line.char_indices() {
            if let Some(needle) = wanted.iter().find(|s| line[pos..].starts_with(s.as_str())) {
                matched.insert((*needle).clone());
            }
        }
        let cwd = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("cwd").and_then(Value::as_str).map(str::to_owned));
        for needle in matched {
            seen.insert(needle.clone());
            if let Some(cwd) = cwd.as_ref().filter(|s| !s.is_empty()) {
                let list = folders.entry(needle).or_default();
                list.retain(|s| s != cwd);
                list.push(cwd.clone());
            }
        }
    });
    let mut map = serde_json::Map::new();
    for needle in seen {
        let mut list = folders.remove(&needle).unwrap_or_default();
        list.reverse();
        map.insert(needle, json!(list));
    }
    Value::Object(map)
}
fn end_boundary(rest: &str) -> bool {
    let mut cs = rest.chars();
    match cs.next() {
        Some(c) if c.is_alphanumeric() || c == '_' || c == '-' => false,
        Some('.') => !cs.next().is_some_and(|c| c.is_alphanumeric() || c == '_'),
        _ => true,
    }
}
fn beginning(prefix: &str) -> bool {
    prefix.is_empty()
        || prefix.ends_with("\\n")
        || prefix.ends_with("\\t")
        || prefix
            .chars()
            .last()
            .is_some_and(|c| c.is_whitespace() || "\"`'([=:,".contains(c))
}
pub fn elsewhere(jsonl: &Path, path: &str) -> Value {
    let tail = path
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_owned();
    if tail.is_empty() || tail.split('/').any(|p| p == "..") {
        return json!([[], []]);
    }
    let name = tail.rsplit('/').next().unwrap();
    let mut absolute = Vec::new();
    let mut relative = Vec::new();
    let relative_re = regex::Regex::new(&format!(
        r"(?:[\p{{L}}\p{{N}}_.-]+/)+{}",
        regex::escape(name)
    ))
    .unwrap();
    lines(jsonl, |line| {
        if !line.contains(name) {
            return;
        }
        for (start, c) in line.char_indices() {
            if c != '/' || !beginning(&line[..start]) {
                continue;
            }
            let text = &line[start..];
            let allowed = text
                .char_indices()
                .find(|(_, c)| "\\\n\"`'<>|*?".contains(*c))
                .map(|(p, _)| p)
                .unwrap_or(text.len());
            let text = &text[..allowed];
            let suffix = format!("/{tail}");
            for (index, _) in text.match_indices(&suffix) {
                if index == 0 {
                    continue;
                }
                let end = index + suffix.len();
                if text[..index].chars().count() > 401 {
                    break;
                }
                if end_boundary(&text[end..]) {
                    absolute.push(text[..end].into());
                    break;
                }
            }
        }
        if !tail.contains('/') {
            for found in relative_re.find_iter(line) {
                let prefix = &line[..found.start()];
                if prefix
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || "_./-".contains(c))
                {
                    continue;
                }
                if end_boundary(&line[found.end()..]) {
                    relative.push(found.as_str().to_owned());
                }
            }
        }
    });
    fn recent(values: Vec<String>, limit: usize) -> Vec<String> {
        let mut seen = HashSet::new();
        values
            .into_iter()
            .rev()
            .filter(|s| seen.insert(s.clone()))
            .take(limit)
            .collect()
    }
    let absolute = recent(absolute, 50)
        .into_iter()
        .filter(|p| Path::new(p).is_file())
        .collect::<Vec<_>>();
    let relative = recent(relative, 20)
        .into_iter()
        .filter(|p| !p.split('/').any(|s| s == ".."))
        .collect::<Vec<_>>();
    json!([absolute, relative])
}
pub fn find_elsewhere(
    jsonl: &Path,
    cwd: &Path,
    path: &str,
    worked: &[String],
    siblings: bool,
) -> Option<PathBuf> {
    let result = elsewhere(jsonl, path);
    if let Some(p) = result[0].as_array()?.first().and_then(Value::as_str) {
        return Some(PathBuf::from(p));
    }
    let rel = path.replace('\\', "/").trim_start_matches("./").to_owned();
    if rel.split('/').any(|p| p == "..") {
        return None;
    }
    let relatives = if rel.contains('/') {
        vec![rel]
    } else {
        result[1]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let mut folders = worked
        .iter()
        .map(|p| real(Path::new(p)))
        .collect::<Vec<_>>();
    let root = real(cwd);
    if !folders.contains(&root) {
        folders.push(root.clone());
    }
    if siblings
        && let Some(parent) = root.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let mut extra = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !folders.contains(p))
            .collect::<Vec<_>>();
        extra.sort();
        folders.extend(extra.into_iter().take(200));
    }
    for rel in relatives {
        for folder in &folders {
            let candidate = real(&folder.join(&rel));
            if candidate != *folder && candidate.starts_with(folder) && candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
pub fn resolve(cwd: &Path, jsonl: &Path, path: &str, write: bool) -> Result<PathBuf> {
    if path.is_empty() || path.contains('\0') {
        return Err(error(400, "invalid path"));
    }
    let cited = cwds(jsonl, &[path.into()]);
    let Some(bases) = cited.get(path).and_then(Value::as_array) else {
        return Err(crate::WorkspaceError {
            status: 403,
            detail: json!({"code":"erro_arquivo_nao_citado","params":{},"msg":"file not referenced in this conversation"}),
            code: None,
        });
    };
    let expanded = if path.starts_with('~') {
        real(Path::new(path))
    } else {
        PathBuf::from(path)
    };
    let target = if expanded.is_absolute() {
        real(&expanded)
    } else {
        if path.replace('\\', "/").split('/').any(|p| p == "..") {
            return Err(crate::WorkspaceError {
                status: 403,
                detail: json!({"code":"erro_caminho_fora_cwd","params":{},"msg":"path escapes session cwd"}),
                code: None,
            });
        }
        let worked = bases
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut folders = worked
            .iter()
            .map(|s| real(Path::new(s)))
            .collect::<Vec<_>>();
        folders.push(real(cwd));
        folders
            .iter()
            .find_map(|base| {
                let target = real(&base.join(&expanded));
                (target != *base && target.starts_with(base) && target.is_file()).then_some(target)
            })
            .or_else(|| find_elsewhere(jsonl, cwd, path, &worked, !write))
            .ok_or_else(not_found)?
    };
    if !target.is_file() {
        return Err(not_found());
    }
    if target.components().any(|p| p.as_os_str() == ".git") {
        return Err(crate::file_error(
            403,
            "erro_arq_area_do_git",
            "área interna do git",
        ));
    }
    Ok(target)
}
fn not_found() -> crate::WorkspaceError {
    crate::WorkspaceError {
        status: 404,
        detail: json!({"code":"erro_arquivo_nao_encontrado","params":{},"msg":"file not found"}),
        code: None,
    }
}
