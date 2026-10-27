//! Leitura de planos compartilhada pela tela e pela ferramenta de orquestração.
use super::{array, text};
use regex::Regex;
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};
pub fn section(text: &str, name: &str) -> Vec<String> {
    let mut inside = false;
    let mut result = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            inside = head.trim() == name;
            continue;
        }
        if inside {
            result.push(line.to_owned())
        }
    }
    result
}
pub fn tasks(text: &str) -> Vec<Value> {
    let rows: Vec<_> = section(text, "Tasks")
        .into_iter()
        .filter(|l| l.trim().starts_with('|'))
        .collect();
    if rows.len() < 2 {
        return Vec::new();
    }
    let header: Vec<_> = rows[0]
        .trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_lowercase())
        .collect();
    let backtick = Regex::new("`([^`]+)`").unwrap();
    rows.iter().skip(2).filter_map(|line|{let c:Vec<_>=line.trim().trim_matches('|').split('|').map(str::trim).collect();let get=|key:&str|header.iter().position(|h|h==key).and_then(|i|c.get(i)).copied().unwrap_or("");let n=get("#").parse::<u64>().ok()?;let risk=get("risk").trim_matches('`').to_lowercase();let roteiro=get("roteiro").trim_matches('`');Some(json!({"n":n,"title":get("what it is"),"files":backtick.captures_iter(get("files")).map(|m|m[1].to_owned()).collect::<Vec<_>>(),"verification":get("verification"),"wave":get("wave"),"risk":if matches!(risk.as_str(),""|"-"|"—"){""}else{&risk},"roteiro":if matches!(roteiro,""|"-"|"—"){""}else{roteiro}}))}).collect()
}
pub fn sha(text: &str) -> String {
    let rx = Regex::new(r"(?m)^Preparado: .* · sha ([0-9a-f]{12})[ \t]*(?:\n|$)").unwrap();
    let body = rx.replace_all(text, "");
    super::sha256(&Regex::new("\n{3,}").unwrap().replace_all(&body, "\n\n"))[..12].into()
}
fn without_fences(text: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trim = line.trim_start_matches(' ');
        let indent = line.len() - trim.len();
        let ch = trim.chars().next().unwrap_or(' ');
        let n = trim.chars().take_while(|c| *c == ch).count();
        if let Some((open, len)) = fence {
            if indent <= 3 && ch == open && n >= len && trim[n..].trim().is_empty() {
                fence = None
            }
            out.push('\n')
        } else if indent <= 3 && matches!(ch, '`' | '~') && n >= 3 {
            fence = Some((ch, n));
            out.push('\n')
        } else {
            out.push_str(line)
        }
    }
    out
}
pub fn find(repo: &Path) -> Value {
    let mut problems = Vec::<String>::new();
    let mut paths = Vec::<String>::new();
    for args in [
        vec![
            "-c",
            "core.quotePath=false",
            "ls-files",
            "-co",
            "--exclude-standard",
            "*.orq.md",
        ],
        vec![
            "-c",
            "core.quotePath=false",
            "grep",
            "-l",
            "--untracked",
            "-E",
            r"^### Task [0-9]+:",
            "--",
            "*.md",
        ],
    ] {
        match super::bounded("git", &args, Some(repo), Duration::from_secs(10)) {
            Ok(out) if out.status.success() => paths.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned),
            ),
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                if !(out.status.code() == Some(1) && args[2] == "grep")
                    && !err.contains("not a git repository")
                {
                    problems.push(format!(
                        "git {}: {}",
                        args[2],
                        err.trim().chars().take(160).collect::<String>()
                    ))
                }
            }
            Err(e) => problems.push(format!("git {}: {e}", args[2])),
        }
    }
    if let Ok(entries) = fs::read_dir(repo.join("docs/superpowers/plans")) {
        let mut kept: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        kept.sort();
        paths.extend(kept.into_iter().filter_map(|p| {
            p.strip_prefix(repo)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        }))
    }
    let mut seen = std::collections::HashSet::new();
    let prepared = Regex::new(r"(?m)^Preparado: .* · sha ([0-9a-f]{12})[ \t]*(?:\n|$)").unwrap();
    let open = Regex::new(r"(?m)^\s*- \[ \] \*\*Step").unwrap();
    let done = Regex::new(r"(?m)^\s*- \[[xX]\] \*\*Step").unwrap();
    let task = Regex::new(r"(?m)^### Task \d+:").unwrap();
    let mut found = Vec::new();
    let mut finished = Vec::new();
    for rel in paths {
        if !seen.insert(rel.clone()) {
            continue;
        }
        let path = repo.join(&rel);
        let (content, modified) = match super::read(&path) {
            Ok(x) => x,
            Err(e) => {
                problems.push(format!("{}: {}", path.display(), e));
                continue;
            }
        };
        let body = without_fences(&content);
        if done.is_match(&body) && !open.is_match(&body) {
            finished.push((modified, path));
            continue;
        }
        let stamp = prepared.captures(&content);
        if stamp.is_none() && !rel.ends_with(".orq.md") && !task.is_match(&body) {
            continue;
        }
        let (rank, state) = if let Some(m) = stamp {
            if m[1] == sha(&content) {
                (0, "stamped")
            } else {
                (1, "changed")
            }
        } else if rel.ends_with(".orq.md") {
            (2, "unstamped")
        } else {
            (3, "tasks")
        };
        found.push((rank, -modified, path, state));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    finished.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut result = json!({});
    if let Some((_, _, path, state)) = found.first() {
        result["path"] = json!(path);
        result["state"] = json!(state)
    }
    if !problems.is_empty() {
        result["problems"] = json!(problems)
    }
    if let Some((_, path)) = finished.last() {
        result["finished"] = json!(path)
    }
    result
}
pub fn readiness(cwd: Option<&str>, grouped: bool, roles: bool) -> Value {
    let found = match cwd {
        Some(path) if Path::new(path).is_dir() => find(Path::new(path)),
        _ => {
            json!({"problems":[format!("pasta da sessão desconhecida: {}",cwd.filter(|s|!s.is_empty()).unwrap_or("—"))]})
        }
    };
    let plan = found
        .get("path")
        .map(|p| json!({"path":p,"state":found["state"]}));
    let phase = if plan.is_none() {
        "planner"
    } else if found["state"] != "stamped" {
        "prepare"
    } else if grouped {
        "arbiter"
    } else {
        "launch"
    };
    json!({"phase":phase,"plan":plan,"group":grouped,"roles":roles,"problems":array(&found["problems"]),"finished":found["finished"]})
}
pub fn metadata(dir: &Path, plan: &str, home: &Path) -> Value {
    let cfg = super::runs::json(&dir.join("orq.json"));
    let mut error = cfg
        .as_ref()
        .err()
        .filter(|e| e.kind() != std::io::ErrorKind::NotFound)
        .map(ToString::to_string);
    let cfg = cfg.unwrap_or_default();
    let path = cfg["plan"].as_str().unwrap_or(plan);
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let mut title = Regex::new(r"^\d{4}-\d{2}-\d{2}-")
        .unwrap()
        .replace(&stem, "")
        .strip_suffix(".orq")
        .unwrap_or(
            &Regex::new(r"^\d{4}-\d{2}-\d{2}-")
                .unwrap()
                .replace(&stem, ""),
        )
        .replace('-', " ");
    let mut total = Value::Null;
    let mut planned = Vec::new();
    if !path.is_empty() {
        for source in [
            super::expand(Path::new(path), home),
            dir.join("plan.snapshot.md"),
        ] {
            match fs::read_to_string(source) {
                Ok(content) => {
                    error = None;
                    if let Some(m) = Regex::new(r"(?m)^#\s+(.+)$").unwrap().captures(&content) {
                        title = m[1].trim().to_owned()
                    }
                    planned = tasks(&content)
                        .iter()
                        .map(|t| json!({"n":t["n"],"title":t["title"]}))
                        .collect();
                    if !planned.is_empty() {
                        total = json!(planned.len())
                    }
                    break;
                }
                Err(e) => error = Some(e.to_string()),
            }
        }
    }
    json!({"title":title,"plan":path,"repo":text(&cfg["repo"]),"total_tasks":total,"tasks":planned,"error":error})
}
pub fn kickoff(readiness: &Value, contract: &str) -> String {
    static TEMPLATES: std::sync::LazyLock<Value> =
        std::sync::LazyLock::new(|| serde_json::from_str(include_str!("kickoffs.json")).unwrap());
    let phase = text(&readiness["phase"]);
    let state = if phase == "prepare" {
        text(&readiness["plan"]["state"])
    } else {
        "stamped"
    };
    let key = format!(
        "{phase}:{}:{state}",
        u8::from(super::truthy(&readiness["roles"]))
    );
    let mut result = text(&TEMPLATES[key])
        .replace("⟦PLAN⟧", text(&readiness["plan"]["path"]))
        .replace("⟦CONTRACT⟧", contract);
    if !array(&readiness["problems"]).is_empty() {
        result.push_str(&format!(
            " Não consegui ler tudo ({}): confira se já existe plano antes de escrever outro.",
            array(&readiness["problems"])
                .iter()
                .map(super::py_string)
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
    if phase == "planner" && super::truthy(&readiness["finished"]) {
        result.push_str(&format!(" O plano `{}` está com todos os Steps marcados (concluído); confirme comigo se o trabalho é outro.",text(&readiness["finished"])))
    }
    result
}
