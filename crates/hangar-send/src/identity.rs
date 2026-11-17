//! Quem assina o recado, na ordem do `me()` do script: sidecar sem terminal, pane, spec da
//! abertura, carimbo do nascimento e, por último, o cliente anexado (com aviso).
use std::io::Write;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::Ctx;

const WARNING_HEAD: &str = "aviso: sessão sem CP_SESSION_NAME válido (wrapper antigo ou sessão renomeada) — \
identidade caiu no 'cliente anexado' e pode sair errada";
const WARNING_TAIL: &str = "Reabra a sessão pelo wrapper atualizado pra corrigir.";

/// `quiet` = o `2>/dev/null` do script: o aviso do cliente anexado não sai.
pub(crate) fn me(cx: &Ctx, quiet: bool, err: &mut dyn Write) -> String {
    if let Some(name) = headless_name(cx) {
        return name;
    }
    if let Some(pane) = cx.var("TMUX_PANE")
        && let Some(name) = pane_owner(cx, pane)
    {
        return name;
    }
    if let Some(name) = spec_name(cx) {
        return name;
    }
    if let Some(stamp) = cx.var("CP_SESSION_NAME")
        && cx.host.tmux(&["has-session", "-t", format!("={stamp}").as_str()]).is_some_and(|run| run.code == 0)
    {
        return stamp.to_owned();
    }
    let mut current = String::new();
    if cx.var("TMUX").is_some() {
        current = attached_client(cx).unwrap_or_default();
        let shown = if current.is_empty() { "cli" } else { &current };
        if !quiet {
            let _ = writeln!(err, "{WARNING_HEAD} ('{shown}'). {WARNING_TAIL}");
        }
    }
    if current.is_empty() { "cli".into() } else { current }
}

/// O primeiro `"name"` do arquivo, linha a linha, mesmo aninhado: o grep do script não lê JSON.
pub(crate) fn first_name(text: &str) -> Option<String> {
    let pattern = Regex::new(r#""name": *"([^"]*)""#).ok()?;
    text.lines().find_map(|line| pattern.captures(line)).map(|found| found[1].to_owned())
}

fn headless_name(cx: &Ctx) -> Option<String> {
    let key = cx.var("CP_SESSION_KEY").or_else(|| cx.var("HANGAR_CANO_KEY"))?;
    let dirs: Vec<PathBuf> = match cx.var("HANGAR_HEADLESS_DIR") {
        Some(dir) => vec![PathBuf::from(dir)],
        None => vec![cx.inv.home.join(".hangar/claude-headless"), cx.inv.home.join(".hangar/codex-sessions")],
    };
    // A chave entra crua na expressão, como no grep do script; expressão inválida não casa nada.
    let pattern = Regex::new(&format!(r#""key": ?"{key}""#)).ok();
    for dir in dirs {
        let Some(pattern) = &pattern else { continue };
        let Some(text) = sidecars(&dir).into_iter().find_map(|file| {
            let text = String::from_utf8_lossy(&std::fs::read(&file).ok()?).into_owned();
            text.lines().any(|line| pattern.is_match(line)).then_some(text)
        }) else {
            continue;
        };
        if let Some(name) = first_name(&text).filter(|name| !name.is_empty()) {
            return Some(name);
        }
    }
    None
}

/// Os `*.json` da pasta na ordem do glob.
fn sidecars(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().map(|entry| entry.path())
        .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".json") && !n.starts_with('.')))
        .collect();
    files.sort();
    files
}

/// O pane só identifica quando aparece uma vez: o psmux repete `%N` entre sessões.
fn pane_owner(cx: &Ctx, pane: &str) -> Option<String> {
    let listed = cx.host.tmux(&["list-panes", "-a", "-F", "#{pane_id} #{session_name}"])?;
    let text = String::from_utf8_lossy(&listed.stdout);
    let owners: Vec<String> = text.lines().filter_map(|line| {
        let mut fields = line.split([' ', '\t']).filter(|field| !field.is_empty());
        (fields.next()? == pane).then(|| fields.collect::<Vec<_>>().join(" "))
    }).collect();
    match owners.as_slice() {
        [name] if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// O spec da abertura só vale se a sessão com esse nome carrega a mesma chave.
fn spec_name(cx: &Ctx) -> Option<String> {
    let key = cx.var("CP_SESSION_KEY")?;
    let spec = cx.inv.home.join(".hangar/session-specs").join(format!("{key}.json"));
    if !spec.is_file() {
        return None;
    }
    let name = first_name(&String::from_utf8_lossy(&std::fs::read(&spec).ok()?)).filter(|name| !name.is_empty())?;
    let target = format!("={name}");
    let shown = cx.host.tmux(&["show-environment", "-t", target.as_str(), "CP_SESSION_KEY"])?;
    let expected = format!("CP_SESSION_KEY={key}");
    let text = String::from_utf8_lossy(&shown.stdout).replace('\r', "");
    // Só a linha conta, como no grep do script: o psmux ignora o alvo e lista tudo.
    text.lines().any(|line| line == expected).then_some(name)
}

/// Clientes humanos de um nome só; linha fora do formato anula.
fn attached_client(cx: &Ctx) -> Option<String> {
    let listed = cx.host.tmux(&["list-clients", "-F", "#{client_control_mode}\t#{session_name}"])?;
    if listed.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&listed.stdout);
    let mut names: Vec<&str> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 2 || (fields[0] != "0" && fields[0] != "1") {
            return None;
        }
        if fields[0] == "0" && !fields[1].is_empty() && !names.contains(&fields[1]) {
            names.push(fields[1]);
        }
    }
    match names.as_slice() {
        [name] => Some((*name).to_owned()),
        _ => None,
    }
}
