//! O que o assistente pede ao sistema: o PATH refeito antes de cada chamada (spec "PATH"), achar programa nele e rodar sem
//! janela. O PATH do próprio app nunca muda: cada `Command` recebe o seu.
use std::{path::{Path, PathBuf}, process::Command};

/// Agentes que o instalador conhece (`TODOS_AGENTES` do `install.sh`, `$todosAgentes` do `install.ps1`): id e nome.
pub(crate) const AGENTS: [(&str, &str); 6] = [("claude", "Claude Code"), ("codex", "Codex"), ("pi", "Pi"), ("omp", "omp"), ("kimi", "Kimi Code"),
    ("antigravity", "Antigravity")];

/// O comando de cada id: o Antigravity se chama pelo produto e o binário é `agy` (`programa_agente` dos instaladores).
pub(crate) fn agent_program(id: &str) -> &str {
    match id {
        "antigravity" => "agy",
        other => other,
    }
}

/// Sem isto o Windows abre um console preto por comando.
pub(crate) fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
    command
}

/// `extra` entra no fim, sem repetir; no Windows sem diferenciar maiúsculas (`windows.md`: expandir e acrescentar).
pub(crate) fn merge_path(current: &str, extra: &[String], windows: bool) -> String {
    let sep = if windows { ';' } else { ':' };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for entry in current.split(sep).chain(extra.iter().flat_map(|e| e.split(sep))) {
        let entry = entry.trim();
        if entry.is_empty() { continue; }
        if seen.insert(if windows { entry.to_lowercase() } else { entry.to_owned() }) { out.push(entry); }
    }
    out.join(&sep.to_string())
}

pub(crate) fn refreshed_path() -> String {
    merge_path(&std::env::var("PATH").unwrap_or_default(), &extra_path_entries(), cfg!(windows))
}

/// O PATH do registro, expandido: o winget e os instaladores gravam lá, e o app aberto antes segue com o antigo.
#[cfg(windows)]
fn extra_path_entries() -> Vec<String> {
    powershell("[Environment]::ExpandEnvironmentVariables([Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User'))")
        .map(|path| vec![path]).unwrap_or_default()
}

/// `~/.local/bin`, o fnm e o prefixo do npm: onde o instalador põe uv, Node e os agentes.
#[cfg(not(windows))]
fn extra_path_entries() -> Vec<String> {
    let Some(home) = std::env::home_dir() else { return Vec::new() };
    let fnm = std::env::var_os("FNM_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share/fnm"));
    let mut entries: Vec<String> = [home.join(".local/bin"), fnm.join("aliases/default/bin")].iter()
        .map(|p| p.to_string_lossy().into_owned()).collect();
    let partial = merge_path(&std::env::var("PATH").unwrap_or_default(), &entries, false);
    if let Some(npm) = find_program("npm", &partial)
        && let Ok(out) = hidden(&mut Command::new(npm)).args(["prefix", "-g"]).env("PATH", &partial).output()
        && out.status.success() {
        let prefix = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !prefix.is_empty() { entries.push(Path::new(&prefix).join("bin").to_string_lossy().into_owned()); }
    }
    entries
}

/// Acha `name` no PATH dado. No Windows procura `.exe`, `.cmd` e `.bat` (os atalhos do npm são `.cmd`).
pub(crate) fn find_program(name: &str, path: &str) -> Option<PathBuf> {
    let (sep, names): (char, Vec<String>) = if cfg!(windows) { (';', [".exe", ".cmd", ".bat"].iter().map(|ext| format!("{name}{ext}")).collect()) }
        else { (':', vec![name.to_owned()]) };
    path.split(sep).filter(|dir| !dir.is_empty())
        .find_map(|dir| names.iter().map(|n| Path::new(dir).join(n)).find(|candidate| candidate.is_file()))
}

pub(crate) fn installed_agents(path: &str) -> Vec<&'static str> {
    AGENTS.iter().filter(|(id, _)| find_program(agent_program(id), path).is_some()).map(|(id, _)| *id).collect()
}

/// O 5.1 de todo Windows, pelo caminho fixo: o PATH do app pode não alcançá-lo.
#[cfg(windows)]
pub(crate) fn powershell_exe() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// Uma linha de PowerShell sem janela; a saída sai em UTF-8 (caminho com acento no nome do usuário).
#[cfg(windows)]
pub(crate) fn powershell(script: &str) -> Result<String, String> {
    let script = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {script}");
    let out = hidden(&mut Command::new(powershell_exe())).args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_without_repeating() {
        assert_eq!(merge_path("/usr/bin:/bin", &["/home/dev/.local/bin".into(), "/usr/bin".into(), "".into()], false),
            "/usr/bin:/bin:/home/dev/.local/bin");
        // No Windows a comparação ignora maiúsculas e o separador é `;`.
        assert_eq!(merge_path(r"C:\Windows;C:\Git\cmd", &[r"c:\git\CMD;C:\Users\dev\.local\bin".into()], true),
            r"C:\Windows;C:\Git\cmd;C:\Users\dev\.local\bin");
        assert_eq!(merge_path("", &[], false), "");
    }

    #[test]
    fn finds_programs_and_agents_in_the_given_path() {
        let dir = std::env::temp_dir().join(format!("hangar-system-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = |n: &str| if cfg!(windows) { format!("{n}.cmd") } else { n.to_owned() };
        std::fs::write(dir.join(name("codex")), "").unwrap();
        std::fs::write(dir.join(name("kimi")), "").unwrap();
        std::fs::write(dir.join(name("agy")), "").unwrap();
        let path = dir.to_string_lossy().into_owned();
        assert_eq!(find_program("codex", &path), Some(dir.join(name("codex"))));
        assert_eq!(find_program("claude", &path), None);
        // O `agy` no PATH conta como o id que o instalador aceita.
        assert_eq!(installed_agents(&path), vec!["codex", "kimi", "antigravity"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
