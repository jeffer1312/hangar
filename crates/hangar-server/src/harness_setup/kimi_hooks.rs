//! Reconcilia os comandos do Hangar sem reescrever o TOML pessoal.
use crate::accounts::environment::AccountEnvironment;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, io::Write, ops::Range, path::Path};

pub const ENTRIES: &[(&str, Option<&str>, &str)] = &[
    ("SessionStart", None, "kimi-state-hook"),
    ("UserPromptSubmit", None, "kimi-state-hook"),
    ("TurnStarted", None, "kimi-state-hook"),
    ("Stop", None, "kimi-state-hook"),
    ("Interrupt", None, "kimi-state-hook"),
    ("PermissionRequest", None, "kimi-state-hook"),
    ("PreToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
    ("PostToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
    // No Windows a ferramenta de shell do Kimi também se chama `Bash` (roda no Git Bash).
    ("PreToolUse", Some("Bash"), "guard-tmux"),
];

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    hooks: Vec<Hook>,
}
#[derive(Deserialize)]
struct Hook {
    #[serde(default)]
    event: Option<toml::Value>,
    #[serde(default)]
    matcher: Option<toml::Value>,
    #[serde(default)]
    command: Option<toml::Spanned<toml::Value>>,
}

pub fn installed(home: &Path, command: &str, mode: &str) -> bool {
    crate::provider_hooks::command(home, mode).is_ok_and(|expected| {
        super::hooks::tokens(command, cfg!(windows))
            == super::hooks::tokens(&expected, cfg!(windows))
    })
}
fn native_command(command: &str, mode: &str, windows: bool) -> bool {
    let tokens = super::hooks::tokens(command, windows);
    tokens.len() == 2
        && tokens[1] == mode
        && super::hooks::hangar_program(&tokens[0])
}

/// O modo do Hangar de um comando gravado (Python antigo ou subcomando), ou `None` se é pessoal.
fn ours(command: &str) -> Option<&'static str> {
    [("kimi_state_hook.py", "kimi-state-hook"), ("guard_tmux.py", "guard-tmux")]
        .into_iter()
        .find(|(legacy, mode)| {
            super::hooks::refers_platform(&json!(command), legacy, true, cfg!(windows)) || native_command(command, mode, cfg!(windows))
        })
        .map(|(_, mode)| mode)
}

/// Onde cada `[[hooks]]` começa e termina no texto, com o comentário do Hangar logo acima e a linha
/// em branco que o separa. `None` quando o texto não tem um cabeçalho por entrada (não dá para casar).
fn blocks(raw: &str, count: usize) -> Option<Vec<Range<usize>>> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut at = 0;
    for line in raw.split_inclusive('\n') {
        lines.push((at, line));
        at += line.len();
    }
    fn header(line: &str) -> bool {
        let line = line.trim();
        let line = line.split_once('#').map_or(line, |(head, _)| head).trim_end();
        line.strip_prefix("[[").and_then(|rest| rest.strip_suffix("]]")).is_some_and(|name| name.trim() == "hooks")
    }
    fn table(line: &str) -> bool { line.trim_start().starts_with('[') }
    fn loose(line: &str) -> bool { line.trim().is_empty() || line.trim_start().starts_with('#') }
    let mut ranges = Vec::new();
    for (index, &(start, line)) in lines.iter().enumerate() {
        if !header(line) {
            continue;
        }
        // O bloco vai até a próxima tabela, sem os comentários e linhas em branco logo acima dela:
        // esses são de quem vem depois.
        let mut last = lines[index + 1..].iter().position(|&(_, line)| table(line)).map_or(lines.len(), |offset| index + 1 + offset);
        while last > index + 1 && loose(lines[last - 1].1) {
            last -= 1;
        }
        let end = lines.get(last).map_or(raw.len(), |(start, _)| *start);
        let mut first = index;
        while first > 0 {
            let above = lines[first - 1].1.trim();
            if above.starts_with("# hangar") || above.starts_with("# Remova este bloco") {
                first -= 1;
            } else {
                break;
            }
        }
        if first > 0 && lines[first - 1].1.trim().is_empty() && first != index {
            first -= 1;
        }
        let begin = if first == index { start } else { lines[first].0 };
        ranges.push(begin..end);
    }
    (ranges.len() == count).then_some(ranges)
}

/// Instala o que falta e conserta o que existe (`harness-setup`, reparo `hooks-kimi`).
pub fn sync(env: &AccountEnvironment, _repo: &Path) -> Result<Vec<String>, String> {
    sync_impl(env, true)
}

/// A subida: só conserta e tira as repetidas das entradas do Hangar que já existem; nunca instala
/// numa máquina que não as tinha.
pub fn repair(env: &AccountEnvironment) -> Result<Vec<String>, String> {
    sync_impl(env, false)
}

fn sync_impl(env: &AccountEnvironment, install: bool) -> Result<Vec<String>, String> {
    let home = super::files::directory(env, "kimi");
    if !home.is_dir() {
        return Ok(vec![]);
    }
    let path = home.join("config.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut data: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        toml::from_str(&raw).map_err(|_| "config.toml inválido; hooks preservados")?
    };
    // Arrays inline não admitem acrescentar [[hooks]] sem redefinir a mesma chave.
    if data.get("hooks").is_some() && !raw.lines().any(|l| l.trim().starts_with("[[hooks]]")) {
        return Err("hooks inline; configuração preservada".into());
    }
    let parsed: Config =
        toml::from_str(&raw).map_err(|_| "config.toml inválido; hooks preservados")?;
    let ranges = blocks(&raw, parsed.hooks.len());
    let mut seen = BTreeSet::new();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for (index, hook) in parsed.hooks.into_iter().enumerate() {
        let Some(command) = hook.command else {
            continue;
        };
        let Some(text) = command.get_ref().as_str() else {
            continue;
        };
        let Some(mode) = ours(text) else {
            continue;
        };
        let event = hook.event.as_ref().and_then(toml::Value::as_str).unwrap_or_default().to_owned();
        let matcher = hook.matcher.as_ref().and_then(toml::Value::as_str).map(str::to_owned);
        // Cada raiz de venv antiga deixava uma entrada: a primeira de cada evento fica e vira o
        // subcomando, as repetidas saem com o comentário do Hangar acima delas.
        if !seen.insert((event, matcher, mode)) {
            if let Some(ranges) = &ranges {
                edits.push((ranges[index].clone(), String::new()));
                continue;
            }
        }
        // Entrada que já roda o comando certo fica byte a byte (aspas literais do TOML inclusive).
        if !installed(&env.home, text, mode) {
            edits.push((
                command.span(),
                serde_json::to_string(&crate::provider_hooks::command(&env.home, mode)?).map_err(|e| e.to_string())?,
            ));
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = raw.clone();
    for (range, replacement) in edits {
        out.replace_range(range, &replacement);
    }
    if out != raw {
        data = toml::from_str(&out)
            .map_err(|_| "migração de hooks inválida; configuração preservada")?;
    }
    for &(event, matcher, mode) in ENTRIES.iter().filter(|_| install) {
        // Entrada sem filtro vale para toda ferramenta: um guarda antigo sem `matcher` já cobre o Bash.
        let exists = data["hooks"].as_array().into_iter().flatten().any(|h| {
            h["event"] == event
                && (h["matcher"].is_null() || h["matcher"].as_str() == matcher)
                && h["command"].as_str().is_some_and(|c| installed(&env.home, c, mode))
        });
        if exists {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out += &format!("\n# hangar: integração da sessão.\n[[hooks]]\nevent = \"{event}\"\n");
        if let Some(matcher) = matcher {
            out += &format!("matcher = \"{matcher}\"\n");
        }
        out += &format!(
            "command = {}\ntimeout = 5\n",
            serde_json::to_string(&crate::provider_hooks::command(&env.home, mode)?)
                .map_err(|e| e.to_string())?
        );
    }
    if out == raw {
        return Ok(vec![]);
    }
    if !raw.trim().is_empty() {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(home.join("config.toml.bak-hangar")) {
            Ok(mut file) => file.write_all(raw.as_bytes()).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    super::files::write_preserving(&path, &out)?;
    Ok(vec![path.to_string_lossy().into_owned()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No Windows o Kimi roda o hook por `cmd.exe /d /s /c "<comando>"` (Node com `shell: true`): o
    /// `/s` tira só as aspas de fora, e o caminho entre aspas, com espaço e `.exe`, chega inteiro.
    #[test]
    fn a_windows_hook_command_survives_the_toml_and_stays_ours() {
        let command = r#""C:\Users\Ana Maria\.hangar\bin\hangar-hooks.exe" kimi-state-hook"#;
        let raw = format!("[[hooks]]\nevent = \"Stop\"\ncommand = {}\n", serde_json::to_string(command).unwrap());
        let parsed: Config = toml::from_str(&raw).unwrap();
        let written = parsed.hooks[0].command.as_ref().unwrap().get_ref().as_str().unwrap().to_owned();
        assert_eq!(written, command);
        assert_eq!(super::super::hooks::tokens(&written, true), [r"C:\Users\Ana Maria\.hangar\bin\hangar-hooks.exe", "kimi-state-hook"]);
        assert!(native_command(&written, "kimi-state-hook", true));
        assert!(!native_command(&written, "guard-tmux", true));
    }
}
