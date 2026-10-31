//! Persona e pacotes do instalador, preservando arquivos reais do usuário.
use serde_json::Value;
use std::{fs, path::Path};

fn link(path: &Path, target: &Path, directory: bool) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::read_link(path)? == target {
                return Ok(true);
            }
            #[cfg(windows)]
            if meta.is_dir() {
                fs::remove_dir(path)?;
            } else {
                fs::remove_file(path)?;
            }
            #[cfg(not(windows))]
            fs::remove_file(path)?;
        }
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        let _ = directory;
        std::os::unix::fs::symlink(target, path)?;
    }
    #[cfg(windows)]
    if directory {
        std::os::windows::fs::symlink_dir(target, path)?;
    } else {
        std::os::windows::fs::symlink_file(target, path)?;
    }
    Ok(true)
}
pub fn sync(home: &Path) -> Result<Vec<String>, String> {
    let installed: Value = serde_json::from_slice(
        &fs::read(home.join(".claude/plugins/installed_plugins.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let source = home.join(".claude/CLAUDE.md");
    let mut done = Vec::new();
    let mut warnings = Vec::new();
    for (name, relative, persona) in [
        ("kimi", ".kimi-code", "AGENTS.md"),
        ("pi", ".pi/agent", "CLAUDE.md"),
    ] {
        let root = home.join(relative);
        if !root.is_dir() {
            continue;
        }
        if source.is_file()
            && !link(&root.join(persona), &source, false).map_err(|e| e.to_string())?
        {
            warnings.push(format!(
                "  {name}: {persona} já existe como arquivo de verdade — não mexi"
            ));
        }
        done.push(name);
        if name == "pi" {
            let packages = root.join("packages");
            fs::create_dir_all(&packages).map_err(|e| e.to_string())?;
            for (key, alias) in [
                ("superpowers@claude-plugins-official", "superpowers"),
                ("ponytail@ponytail", "ponytail"),
            ] {
                if let Some(path) = installed["plugins"][key]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v["installPath"].as_str())
                {
                    link(&packages.join(alias), Path::new(path), true)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    let label = if done.is_empty() {
        "nenhum agente instalado, nada a fazer".to_owned()
    } else {
        done.join(", ")
    };
    let mut messages = vec![format!("ponte: {label}")];
    messages.extend(warnings);
    Ok(messages)
}
