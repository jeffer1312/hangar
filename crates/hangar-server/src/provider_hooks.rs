//! Hooks autônomos: sinais do Kimi e guarda do multiplexador, sem servidor ou interpretador.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

pub fn command(mode: &str) -> Result<String, String> {
    let (executable, _) = crate::codex_setup::hook_command::allow_wrapper()?;
    Ok(format!("\"{executable}\" {mode}"))
}

fn log_failure(file: &str, code: &str) {
    let Some(logs) = crate::runtime::local_policy::log_base() else {
        return;
    };
    let dir = logs.join("privado");
    let _ = (|| -> std::io::Result<()> {
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let line = format!("{} {code}\n", chrono::Utc::now().to_rfc3339());
        options.open(dir.join(file))?.write_all(line.as_bytes())
    })();
}

pub fn run(mode: &str) -> ExitCode {
    let mut input = Vec::new();
    let result = std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|_| "stdin_read_failed")
        .and_then(|_| {
            let value: Value = serde_json::from_str(&String::from_utf8_lossy(&input))
                .map_err(|_| "invalid_json")?;
            if !value.is_object() {
                return Err("payload_not_object");
            }
            if mode == "kimi-state-hook" {
                let home = std::env::home_dir().ok_or("home_unavailable")?;
                let base = std::env::var_os("CLAUDE_CONFIG_DIR")
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".claude"));
                write_kimi(
                    &value,
                    &base,
                    std::env::var("TMUX_PANE").ok().as_deref(),
                    std::env::var("PSMUX_SESSION").ok().as_deref(),
                )?;
                Ok(None)
            } else {
                if value["tool_name"] != "Bash" {
                    return Ok(None);
                }
                let command = match value.get("tool_input") {
                    Some(input) if !input.is_object() => return Err("tool_input_not_object"),
                    Some(input) => &input["command"],
                    None => &Value::Null,
                };
                if !crate::orchestration::truthy(command) {
                    return Ok(None);
                }
                let command = command.as_str().ok_or("command_not_string")?;
                Ok(dangerous(command, 0))
            }
        });
    match result {
        Ok(Some(reason)) => {
            let text = format!(
                "BLOQUEADO: {reason}.\nPrecisa de tmux limpo para uma sonda? Suba um SERVIDOR SEPARADO com socket próprio:\n  tmux -L probe-x new-session -d -s alvo ...\n  tmux -L probe-x kill-server\nNunca no servidor default, onde vivem as sessões de trabalho (TMUX_TMPDIR não isola: dentro do tmux quem manda é $TMUX).\nPara matar UMA sessão: `tmux kill-session -t '=nome:'`."
            );
            let _ = writeln!(
                std::io::stdout().lock(),
                "{}",
                json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":text}})
            );
            let _ = writeln!(std::io::stderr().lock(), "{text}");
            ExitCode::from(2)
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(code) => {
            log_failure(
                if mode == "kimi-state-hook" {
                    "kimi_hook_error.log"
                } else {
                    "guard_tmux-falhas.log"
                },
                code,
            );
            ExitCode::SUCCESS
        }
    }
}

fn write_kimi(
    value: &Value,
    base: &Path,
    pane: Option<&str>,
    psmux: Option<&str>,
) -> Result<(), &'static str> {
    if value["hook_event_name"].is_array() || value["hook_event_name"].is_object() {
        return Err("event_not_scalar");
    }
    let sid = value.get("session_id");
    let Some(sid) = sid.filter(|sid| crate::orchestration::truthy(sid)) else {
        return Ok(());
    };
    let event = value["hook_event_name"].as_str().unwrap_or_default();
    let state = match event {
        "UserPromptSubmit" | "TurnStarted" => Some("working"),
        "Stop" | "Interrupt" => Some("idle"),
        "PermissionRequest" => Some("awaiting_input"),
        "PreToolUse" if value["tool_name"] == "AskUserQuestion" => Some("awaiting_input"),
        "PostToolUse" if value["tool_name"] == "AskUserQuestion" => Some("working"),
        _ => None,
    };
    if let Some(state) = state {
        let sid = sid.as_str().ok_or("session_id_not_string")?;
        marker(
            base,
            ".hangar-state",
            sid,
            &json!({"state":state,"ts":timestamp()}),
        )?;
    }
    let key: String = if let Some(name) = psmux.filter(|s| !s.is_empty()) {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    } else {
        pane.unwrap_or_default().trim_start_matches('%').to_owned()
    };
    if !key.is_empty() {
        marker(
            base,
            ".hangar-kimi",
            &key,
            &json!({"session_id":sid,"cwd":value.get("cwd").unwrap_or(&Value::Null),"ts":timestamp()}),
        )?;
    }
    Ok(())
}
fn timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn marker(base: &Path, directory: &str, key: &str, payload: &Value) -> Result<(), &'static str> {
    let directory = base.join(directory);
    fs::create_dir_all(&directory).map_err(|_| "marker_directory_failed")?;
    crate::runtime::queue::atomic_write(
        &directory.join(format!("{key}.json")),
        &serde_json::to_vec(payload).map_err(|_| "marker_encoding_failed")?,
    )
    .map_err(|_| "marker_write_failed")
}

fn segments(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut quote = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            buf.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            buf.push(c);
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            buf.push(c);
            buf.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '<' && chars.get(i + 1) == Some(&'<') {
            break;
        }
        if ";|&\n()`".contains(c) || (c == '$' && chars.get(i + 1) == Some(&'(')) {
            out.push(std::mem::take(&mut buf));
            i += if c == '$' || ((c == '&' || c == '|') && chars.get(i + 1) == Some(&c)) {
                2
            } else {
                1
            };
            continue;
        }
        buf.push(c);
        i += 1;
    }
    out.push(buf);
    out
}
fn tokens(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut quote = None;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && c == '\\' {
                if let Some(next) = chars.next() {
                    if next != '"' && next != '\\' {
                        buf.push('\\');
                    }
                    buf.push(next);
                }
            } else {
                buf.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == '\\' {
            if let Some(next) = chars.next() {
                buf.push(next);
            }
        } else if c.is_whitespace() {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
        } else {
            buf.push(c);
        }
    }
    if quote.is_some() {
        return segment.split_whitespace().map(str::to_owned).collect();
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}
fn head(tokens: &[String]) -> (&str, &[String]) {
    const ENVELOPES: &[&str] = &[
        "sudo",
        "doas",
        "env",
        "nohup",
        "setsid",
        "time",
        "timeout",
        "command",
        "exec",
        "builtin",
        "stdbuf",
        "nice",
        "ionice",
        "systemd-run",
        "rtk",
        "proxy",
        "--",
    ];
    for (i, token) in tokens.iter().enumerate() {
        let token_name = token.trim_matches(['\'', '"']);
        let name = if cfg!(windows) {
            token_name.rsplit(['/', '\\']).next().unwrap_or_default()
        } else {
            token_name.rsplit('/').next().unwrap_or_default()
        };
        let duration = {
            static DURATION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            DURATION
                .get_or_init(|| regex::Regex::new(r"^\d+(\.\d+)?[smhd]?$").unwrap())
                .is_match(name)
        };
        if (token.contains('=') && !token.starts_with('-'))
            || ENVELOPES.contains(&name)
            || token.starts_with('-')
            || duration
        {
            continue;
        }
        return (name, &tokens[i + 1..]);
    }
    ("", &[])
}
fn dangerous(command: &str, depth: u8) -> Option<String> {
    for segment in segments(command) {
        let words = tokens(&segment);
        let (name, args) = head(&words);
        if ["sh", "bash", "zsh", "dash", "ksh", "ash", "busybox", "eval"].contains(&name)
            && depth < 3
        {
            let script = if name == "eval" {
                Some(args.join(" "))
            } else {
                args.iter()
                    .position(|t| t.starts_with('-') && !t.starts_with("--") && t.contains('c'))
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            if let Some(reason) = script.and_then(|script| dangerous(&script, depth + 1)) {
                return Some(reason);
            }
            continue;
        }
        if name == "xargs" {
            let (target, _) = head(args);
            if ["tmux", "psmux", "pkill", "killall"].contains(&target) {
                return Some(format!(
                    "`xargs {target}` monta os argumentos pela entrada; o comando real pode ser um kill-server"
                ));
            }
            continue;
        }
        if ["pkill", "killall"].contains(&name) {
            let regex = args
                .iter()
                .any(|t| t.starts_with('-') && !t.starts_with("--") && t.contains('f'));
            let kills = args.iter().filter(|t| !t.starts_with('-')).any(|t| {
                if regex {
                    if python_regex_extensions(t) {
                        fancy_regex::Regex::new(t)
                            .and_then(|r| Ok(r.is_match("tmux server")? || r.is_match("psmux")?))
                            .unwrap_or_else(|_| t.contains("tmux"))
                    } else {
                        t.contains("tmux")
                    }
                } else {
                    t.contains("tmux") || t.contains("psmux")
                }
            });
            if kills {
                return Some(format!(
                    "`{name}` casando tmux mata o servidor e todas as sessões junto"
                ));
            }
            continue;
        }
        if !["tmux", "psmux"].contains(&name)
            || args
                .iter()
                .any(|t| t.starts_with("-L") || t.starts_with("-S"))
        {
            continue;
        }
        if args.iter().any(|t| t == "kill-server") {
            return Some(
                "`tmux kill-server` sem -L/-S derruba TODAS as sessões do socket default".into(),
            );
        }
        if args.iter().any(|t| t == "kill-session") {
            if args.iter().any(|t| t == "-a") {
                return Some(
                    "`tmux kill-session -a` sem -L/-S mata todas as outras sessões do default"
                        .into(),
                );
            }
            if !args.iter().any(|t| t.starts_with("-t")) {
                return Some(
                    "`tmux kill-session` sem -t mata a sessão ATUAL (o pane de quem chamou)".into(),
                );
            }
        }
    }
    None
}

// O padrão vem do argumento de pkill/killall no payload Bash, não de configuração do Hangar.
// Extensões de outros dialetos precisam seguir o mesmo fallback do re.error anterior.
fn python_regex_extensions(pattern: &str) -> bool {
    let chars: Vec<_> = pattern.chars().collect();
    let mut i = 0;
    let mut class_start = None;
    let mut verbose = false;
    let mut groups = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            if let Some(&next) = chars.get(i + 1) {
                if "eGhHkKgGORXYy".contains(next) {
                    return false;
                }
                if next == 'N' && chars.get(i + 2) != Some(&'{') {
                    return false;
                }
            }
            i += 2;
            continue;
        }
        if let Some(start) = class_start {
            if c == ']' && i > start + 1 && !(i == start + 2 && chars.get(start + 1) == Some(&'^'))
            {
                class_start = None;
            }
            i += 1;
            continue;
        }
        if c == '[' {
            class_start = Some(i);
            i += 1;
            continue;
        }
        if verbose && c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == ')' {
            if let Some(old) = groups.pop() {
                verbose = old;
            }
            i += 1;
            continue;
        }
        if c == '(' {
            if chars.get(i + 1) == Some(&'?') {
                match chars.get(i + 2) {
                    Some('#') => {
                        i += 3;
                        while i < chars.len() {
                            if chars[i] == '\\' {
                                i += 2;
                                continue;
                            }
                            if chars[i] == ')' {
                                i += 1;
                                break;
                            }
                            i += 1;
                        }
                        continue;
                    }
                    Some('<') if !chars.get(i + 3).is_some_and(|c| matches!(c, '=' | '!')) => {
                        return false;
                    }
                    Some('\'') | Some('&') | Some('R') => return false,
                    Some('P') if !chars.get(i + 3).is_some_and(|c| matches!(c, '<' | '=')) => {
                        return false;
                    }
                    Some('(') if chars.get(i + 3) == Some(&'?') => return false,
                    _ => {}
                }
                let mut j = i + 2;
                let mut enable = true;
                let mut new_verbose = verbose;
                while let Some(&flag) = chars.get(j) {
                    if flag == '-' {
                        enable = false;
                    } else if "aiLmsux".contains(flag) {
                        if flag == 'x' {
                            new_verbose = enable;
                        }
                    } else {
                        break;
                    }
                    j += 1;
                }
                if j > i + 2 && chars.get(j).is_some_and(|c| matches!(c, ':' | ')')) {
                    if chars[j] == ':' {
                        groups.push(verbose);
                    }
                    verbose = new_verbose;
                    i = j + 1;
                    continue;
                }
            }
            groups.push(verbose);
        }
        i += 1;
    }
    true
}
