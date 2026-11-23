//! `hangar-server agents-install`: os agentes das skills (`skills/*/agents/*.md`) viram link em
//! `~/.claude/agents/` e `.toml` em cada home do Codex. Uma fonte só: o Codex lê o mesmo texto em
//! `developer_instructions`. Idempotente.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hangar_core::pycompat::py;

use crate::{Invocation, files};

pub fn cli(args: &[String]) -> ExitCode {
    crate::cli(args, "agents-install", false, run)
}

pub fn run(invocation: &Invocation, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some(mut root) = invocation.root() else {
        let _ = writeln!(err, "erro: raiz do Hangar desconhecida — passe --root <raiz da cópia>");
        return 2;
    };
    // Pacote: o link aponta para o `current`, não para a versão (a poda a apaga).
    let current = hangar_core::paths::current(&invocation.home);
    if current.exists() && hangar_core::fs::same_file(&current, &root) {
        root = current;
    }
    let home = invocation.target_home();
    let codex_homes = codex_homes(&home);
    let mut all_ok = true;
    for source in agent_sources(&root) {
        if let Err(detail) = install(&source, &home, &codex_homes, out) {
            let _ = writeln!(err, "erro: {detail}");
            all_ok = false;
        }
    }
    if !all_ok {
        let _ = writeln!(err, "erro: algum agente das skills ficou sem instalar (veja acima)");
    }
    u8::from(!all_ok)
}

/// O `glob("skills/*/agents/*.md")` do Python, em ordem: oculto fica de fora.
fn agent_sources(root: &Path) -> Vec<PathBuf> {
    let visible = |entry: &std::fs::DirEntry| !entry.file_name().to_string_lossy().starts_with('.');
    let mut sources: Vec<PathBuf> = std::fs::read_dir(root.join("skills")).into_iter().flatten().flatten()
        .filter(visible)
        .flat_map(|skill| std::fs::read_dir(skill.path().join("agents")).into_iter().flatten().flatten())
        .filter(|entry| visible(entry) && entry.file_name().to_string_lossy().ends_with(".md"))
        .map(|entry| entry.path())
        .collect();
    sources.sort();
    sources
}

/// Cada `.codex*` que já é home do Codex (tem `config.toml` ou `agents/`).
fn codex_homes(home: &Path) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = std::fs::read_dir(home).into_iter().flatten().flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".codex"))
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir() && (dir.join("config.toml").exists() || dir.join("agents").is_dir()))
        .collect();
    homes.sort();
    homes
}

/// O cabeçalho `---\n…\n---\n` e o corpo, sem as linhas em branco do começo. Sem cabeçalho, campos vazios.
pub fn front_matter(text: &str) -> (BTreeMap<String, String>, String) {
    let mut fields = BTreeMap::new();
    let Some(rest) = text.strip_prefix("---\n") else { return (fields, text.to_owned()) };
    let Some(end) = rest.find("\n---\n") else { return (fields, text.to_owned()) };
    for line in py::splitlines(&rest[..end]) {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(py::strip(key).to_owned(), py::strip(value).to_owned());
        }
    }
    (fields, rest[end + 5..].trim_start_matches('\n').to_owned())
}

/// O `.toml` do agente no Codex.
pub fn codex_agent(name: &str, description: &str, body: &str) -> String {
    format!(
        "name = {}\ndescription = {}\nmodel_reasoning_effort = \"high\"\ndeveloper_instructions = {}\n",
        files::toml_string(name), files::toml_string(description), files::toml_string(body)
    )
}

fn install(source: &Path, home: &Path, codex_homes: &[PathBuf], out: &mut dyn Write) -> Result<(), String> {
    let bytes = std::fs::read(source).map_err(|error| format!("{}: {}", source.display(), files::os_error(&error, source)))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{}: o arquivo não é texto UTF-8", source.display()))?;
    let (fields, body) = front_matter(&files::universal_newlines(text));
    let Some(name) = fields.get("name").filter(|name| !name.is_empty()) else { return Ok(()) };
    let destination = home.join(".claude").join("agents").join(format!("{name}.md"));
    let failed = |error: std::io::Error, path: &Path| format!("{}: agente {name} não instalado: {}", destination.display(), files::os_error(&error, path));
    if let Some(dir) = destination.parent() {
        std::fs::create_dir_all(dir).map_err(|error| failed(error, dir))?;
    }
    if std::fs::symlink_metadata(&destination).is_ok() {
        std::fs::remove_file(&destination).map_err(|error| failed(error, &destination))?;
    }
    match link(source, &destination) {
        Ok(()) => {
            let _ = writeln!(out, "ok: {} -> {}", destination.display(), source.display());
        }
        Err(_) => {
            // Windows sem privilégio de symlink: copia, e diz, porque um git pull não a atualiza.
            std::fs::copy(source, &destination).map_err(|error| failed(error, &destination))?;
            let _ = writeln!(out, "ok: {} (COPIA — symlink indisponivel; re-rode apos git pull)", destination.display());
        }
    }
    let agent = codex_agent(name, fields.get("description").map_or("", String::as_str), &body);
    for codex_home in codex_homes {
        let target = codex_home.join("agents").join(format!("{name}.toml"));
        let agents = codex_home.join("agents");
        if !agents.is_dir() {
            std::fs::create_dir(&agents).map_err(|error| failed(error, &agents))?;
        }
        std::fs::write(&target, &agent).map_err(|error| failed(error, &target))?;
        let _ = writeln!(out, "ok: {}", target.display());
    }
    Ok(())
}

#[cfg(unix)]
fn link(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn link(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, destination)
}
