use std::{collections::{BTreeMap, HashSet}, path::{Path,PathBuf}};
use serde_json::{Value,json};

fn read(value: &str, optional: bool, cwd: &Path) -> Result<Value,String> {
    let raw = if value.trim_start().starts_with('{') { value.into() } else {
        let path = Path::new(value); let path = if path.is_absolute() { path.to_owned() } else { cwd.join(path) };
        match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && optional => return Ok(json!({})),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("--settings: arquivo não encontrado".into()),
            Err(_) => return Err("settings: arquivo ilegível ou JSON inválido".into()),
        }
    };
    let data: Value = serde_json::from_str(&raw).map_err(|_|"settings: arquivo ilegível ou JSON inválido")?;
    if !data.is_object() || data.get("env").is_some_and(|v| !v.is_object()) { return Err("settings: esperado um objeto JSON com env como objeto".into()); }
    Ok(data)
}

fn local_root(cwd: &Path, home: &Path) -> PathBuf {
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        for root in cwd.ancestors() {
            if !root.join(".git").exists() { continue; }
            if root == home { return cwd.into(); }
            let mut paths = vec![root.to_owned(),root.join(".git")];
            if root.join(".claude").exists() { paths.push(root.join(".claude")); }
            return if paths.iter().all(|p|std::fs::metadata(p).is_ok_and(|m|m.uid() == unsafe {libc::geteuid()})) { root.into() } else { cwd.into() };
        }
    }
    cwd.into()
}

pub fn resolve(cmd: &[String], env: &mut BTreeMap<String,String>, tier: &str, cwd: &Path, home: &Path) -> Result<(Vec<String>,Value),String> {
    let mut explicit = json!({}); let mut sources: HashSet<String> = ["user","project","local"].into_iter().map(str::to_owned).collect();
    let mut args = vec![cmd.first().ok_or("falta o comando")?.clone()]; let mut i = 1;
    while i < cmd.len() {
        let arg = &cmd[i];
        if arg == "--" { args.extend_from_slice(&cmd[i..]); break; }
        if arg == "--settings" || arg == "--setting-sources" {
            let value = cmd.get(i+1).ok_or_else(||format!("{arg} precisa de um valor"))?;
            if arg == "--settings" { explicit = read(value,false,cwd)?; } else { sources = value.split(',').map(str::to_owned).collect(); args.extend_from_slice(&cmd[i..i+2]); }
            i += 2; continue;
        }
        if let Some(value) = arg.strip_prefix("--settings=") { explicit = read(value,false,cwd)?; }
        else if let Some(value) = arg.strip_prefix("--setting-sources=") { sources = value.split(',').map(str::to_owned).collect(); args.push(arg.clone()); }
        else { args.push(arg.clone()); }
        i += 1;
    }
    let config = env.get("CLAUDE_CONFIG_DIR").filter(|s|!s.is_empty()).map(PathBuf::from).unwrap_or_else(||home.join(".claude"));
    let mut paths = Vec::new();
    if sources.contains("user") { paths.push(config.join("settings.json")); }
    if sources.contains("project") { paths.push(cwd.join(".claude/settings.json")); }
    if sources.contains("local") {
        paths.push(cwd.join(".claude/settings.local.json"));
        let local = local_root(cwd,home); if local != cwd { paths.push(local.join(".claude/settings.local.json")); }
    }
    let mut extra = json!(env.get("CLAUDE_CODE_EXTRA_BODY").map(String::as_str).unwrap_or("{}"));
    for path in paths {
        let source = read(&path.to_string_lossy(),true,cwd)?;
        if let Some(value) = source["env"].get("CLAUDE_CODE_EXTRA_BODY") { extra = value.clone(); }
    }
    if let Some(value) = explicit["env"].get("CLAUDE_CODE_EXTRA_BODY") { extra = value.clone(); }
    let selected = super::environment::service_tier(tier,Some(extra.as_str().ok_or("CLAUDE_CODE_EXTRA_BODY: esperado texto JSON")?))?;
    if explicit.get("env").is_none() { explicit["env"] = json!({}); }
    for (k,v) in selected { explicit["env"][&k] = json!(v); env.insert(k,v); }
    Ok((args,explicit))
}
