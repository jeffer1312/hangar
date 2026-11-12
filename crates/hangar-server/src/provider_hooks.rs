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
                let tool = value["tool_name"].as_str().unwrap_or("");
                // PowerShell: a tool de shell do Claude Code no Windows, quando ligada.
                if !["Bash", "PowerShell", "Read", "Grep"].contains(&tool) {
                    return Ok(None);
                }
                let input = match value.get("tool_input") {
                    Some(input) if !input.is_object() => return Err("tool_input_not_object"),
                    Some(input) => input,
                    None => &Value::Null,
                };
                if !["Bash", "PowerShell"].contains(&tool) {
                    let read = ["file_path", "path", "glob"].iter().any(|k| input[k].as_str().is_some_and(vault_path))
                        || (tool == "Grep" && input["path"].as_str().is_some_and(above_vault));
                    return Ok(read.then(|| VAULT_DENIED.to_owned()));
                }
                let command = &input["command"];
                if !crate::orchestration::truthy(command) {
                    return Ok(None);
                }
                let command = command.as_str().ok_or("command_not_string")?;
                if reads_vault(command, 0) {
                    return Ok(Some(VAULT_DENIED.to_owned()));
                }
                Ok(dangerous(command, 0).map(|reason| format!(
                    "BLOQUEADO: {reason}.\nPrecisa de tmux limpo para uma sonda? Suba um SERVIDOR SEPARADO com socket próprio:\n  tmux -L probe-x new-session -d -s alvo ...\n  tmux -L probe-x kill-server\nNunca no servidor default, onde vivem as sessões de trabalho (TMUX_TMPDIR não isola: dentro do tmux quem manda é $TMUX).\nPara matar UMA sessão: `tmux kill-session -t '=nome:'`."
                )))
            }
        });
    match result {
        Ok(Some(text)) => {
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
    let mut started = false;
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
            started = true;
        } else if c == '\\' {
            if let Some(next) = chars.next() {
                buf.push(next);
                started = true;
            } else {
                return segment.split_whitespace().map(str::to_owned).collect();
            }
        } else if c.is_whitespace() {
            if started {
                out.push(std::mem::take(&mut buf));
                started = false;
            }
        } else {
            buf.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return segment.split_whitespace().map(str::to_owned).collect();
    }
    if started {
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
const VAULT_DENIED: &str = "BLOQUEADO: o arquivo do `vault_file` guarda uma senha e não se lê, nem para contar, medir ou tirar hash.\nO tamanho já veio na resposta do `vault_file` (`bytes`, `chars`, `ends_with_newline`). Passe o caminho como argumento ao programa que precisa da senha.";

/// A pasta do `vault_file` (`$XDG_RUNTIME_DIR/hangar/vault-files`, `%LOCALAPPDATA%\hangar\vault-files`)
/// como parte de um caminho, absoluto ou relativo, com qualquer barra ou caixa.
fn vault_path(text: &str) -> bool {
    vault_hits(text) > 0
}

/// `Grep` num caminho até dois níveis acima da pasta (`hangar` ou a pasta de execução) a varre inteira.
fn above_vault(path: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase();
    let path = norm(path);
    ["XDG_RUNTIME_DIR", "LOCALAPPDATA"].iter().filter_map(|var| std::env::var(var).ok()).filter(|r| !r.is_empty()).any(|root| {
        let base = format!("{}/hangar/vault-files", norm(&root));
        !path.is_empty() && base.strip_prefix(&format!("{path}/")).is_some_and(|rest| rest.split('/').count() <= 2)
    })
}

/// Programas que mostram, medem ou copiam o conteúdo do arquivo que recebem.
const READERS: &[&str] = &[
    "cat", "tac", "head", "tail", "less", "more", "bat", "nl", "od", "xxd", "hexdump", "strings",
    "base64", "wc", "md5sum", "sha1sum", "sha256sum", "sha512sum", "cksum", "b2sum", "cut", "sort",
    "uniq", "diff", "cmp", "cp", "type", "get-content", "gc", "get-filehash", "format-hex",
    "certutil", "mapfile", "readarray", "read",
];
/// Leitores cujo primeiro argumento é o padrão de busca, que pode citar a pasta sem ler nada.
const PATTERN_FIRST: &[&str] = &["grep", "egrep", "rg", "sed", "awk", "select-string", "sls"];
/// Opções desses leitores cujo valor é um arquivo lido (`-Path`, `grep -f`), não um padrão.
const FILE_OPTIONS: &[&str] = &["-path", "-literalpath", "-f", "--file", "-filepath"];
/// Shells cujo argumento é outro comando; os dois guardas entram neles.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash", "busybox", "eval"];

fn vault_hits(text: &str) -> usize {
    static VAULT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    VAULT
        .get_or_init(|| regex::Regex::new(r#"(?i)(^|[/\\\s'"=<])vault-files([/\\\s'"*]|$)"#).unwrap())
        .find_iter(text)
        .count()
}

/// O comando que um shell recebe, cru (barras invertidas inteiras), sem o nome nem as opções.
fn shell_script(segment: &str) -> Option<String> {
    static SCRIPT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let script = SCRIPT.get_or_init(|| {
        regex::Regex::new(r#"(?is)^\s*(?:\S*[/\\])?(?:sh|bash|zsh|dash|ksh|ash|busybox|eval|powershell|pwsh|cmd)(?:\.exe)?(?:\s+[-/]\S*)*\s+(.*)$"#).unwrap()
    });
    let rest = script.captures(segment)?.get(1)?.as_str().trim();
    let unquoted = ['\'', '"'].iter().find_map(|q| rest.strip_prefix(*q).and_then(|r| r.strip_suffix(*q)));
    Some(unquoted.unwrap_or(rest).to_owned())
}

/// Ler o arquivo, não usá-lo: o caminho como argumento de outro programa passa.
fn reads_vault(command: &str, depth: u8) -> bool {
    if !vault_path(command) {
        return false;
    }
    // .NET pelo PowerShell (`[IO.File]::ReadAllText`, `ReadLines`, `OpenText`, `StreamReader`…).
    let lower = command.to_ascii_lowercase();
    if ["::readall", "::readlines", "::opentext", "::openread", "streamreader"].iter().any(|m| lower.contains(m)) {
        return true;
    }
    // `segments` para no primeiro `<<` (o corpo de um heredoc não é comando); aqui o que vem depois também conta.
    let command = command.replace("<<", "  ");
    static REDIRECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let redirect = REDIRECT.get_or_init(|| regex::Regex::new(r#"(?i)<\s*['"]?[^\s'"]*vault-files"#).unwrap());
    let mut inside = false;
    segments(&command).iter().any(|segment| {
        let words = tokens(segment);
        let (name, args) = head(&words);
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        if SHELLS.contains(&name) || ["powershell", "pwsh", "cmd"].contains(&name) {
            // O texto cru guarda o caminho do Windows, que `tokens` desfaz; os pedaços pegam o shell atrás de `sudo`/`env`.
            return depth < 3
                && (shell_script(segment).is_some_and(|s| reads_vault(&s, depth + 1)) || reads_vault(&args.join(" "), depth + 1));
        }
        // Entrar na pasta faz `cat *` ler a senha sem citar o caminho.
        if ["cd", "pushd", "set-location", "sl", "push-location"].contains(&name) {
            inside |= vault_path(segment);
            return false;
        }
        if redirect.is_match(segment) {
            return true;
        }
        if PATTERN_FIRST.contains(&name) {
            let file_option = args.windows(2).any(|w| FILE_OPTIONS.contains(&w[0].to_ascii_lowercase().as_str()) && w[1].to_ascii_lowercase().contains("vault-files"));
            // Mais citações da pasta no texto cru do que no padrão: alguma é arquivo lido.
            let pattern = args.iter().find(|a| !a.starts_with('-')).map_or(0, |p| vault_hits(p));
            return inside || file_option || vault_hits(segment) > pattern;
        }
        READERS.contains(&name) && (inside || vault_path(segment))
    })
}

fn dangerous(command: &str, depth: u8) -> Option<String> {
    for segment in segments(command) {
        let words = tokens(&segment);
        let (name, args) = head(&words);
        if SHELLS.contains(&name) && depth < 3 {
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
                    if let Some(pattern) = python_regex_pattern(t) {
                        fancy_regex::Regex::new(&pattern)
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
fn python_regex_pattern(pattern: &str) -> Option<String> {
    let chars: Vec<_> = pattern.chars().collect();
    let mut i = 0;
    let mut replacements = Vec::new();
    let mut verbose = false;
    let mut groups = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            if chars.get(i + 1) == Some(&'N') {
                let mut end = i + 2;
                let code = named_character(&chars, &mut end)?;
                replacements.push((i..end, format!("\\x{{{code:x}}}")));
                i = end;
                continue;
            }
            if let Some(&next) = chars.get(i + 1) {
                if "eGhHkKgGORXYy".contains(next) {
                    return None;
                }
            }
            i += 2;
            continue;
        }
        if c == '[' {
            let (end, replacement) = python_class(&chars, i)?;
            replacements.push((i..end, replacement));
            i = end;
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
                        return None;
                    }
                    Some('\'') | Some('&') | Some('R') => return None,
                    Some('P') if !chars.get(i + 3).is_some_and(|c| matches!(c, '<' | '=')) => {
                        return None;
                    }
                    Some('(') if chars.get(i + 3) == Some(&'?') => return None,
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
    let mut out = chars;
    for (span, replacement) in replacements.into_iter().rev() {
        out.splice(span, replacement.chars());
    }
    Some(out.into_iter().collect())
}

#[derive(Clone, Copy)]
enum ClassAtom {
    Literal(u32),
    Category(char),
    Hyphen,
}
impl ClassAtom {
    fn literal(self) -> Option<u32> {
        match self {
            Self::Literal(c) => Some(c),
            Self::Hyphen => Some(u32::from('-')),
            Self::Category(_) => None,
        }
    }
}

// Classes Python são uniões. Escapar os literais impede o motor de interpretar &&, -- ou [ como conjuntos.
fn python_class(chars: &[char], start: usize) -> Option<(usize, String)> {
    let mut i = start + 1;
    let negative = chars.get(i) == Some(&'^');
    if negative {
        i += 1;
    }
    let first = i;
    let mut atoms = Vec::new();
    loop {
        let &c = chars.get(i)?;
        if c == ']' && i != first {
            i += 1;
            break;
        }
        atoms.push(if c == '\\' {
            class_escape(chars, &mut i)?
        } else {
            i += 1;
            if c == '-' {
                ClassAtom::Hyphen
            } else {
                ClassAtom::Literal(u32::from(c))
            }
        });
    }
    let mut body = String::new();
    let mut n = 0;
    while n < atoms.len() {
        if n + 2 < atoms.len() && matches!(atoms[n + 1], ClassAtom::Hyphen) {
            let mut from = atoms[n].literal()?;
            let mut to = atoms[n + 2].literal()?;
            if from > to {
                return None;
            }
            // Surrogates existem no str do Python, mas nunca nos dois alvos ASCII da guarda.
            if (0xd800..=0xdfff).contains(&from) {
                from = 0xe000;
            }
            if (0xd800..=0xdfff).contains(&to) {
                to = 0xd7ff;
            }
            if from <= to {
                body += &format!("\\x{{{from:x}}}-\\x{{{to:x}}}");
            }
            n += 3;
        } else {
            match atoms[n] {
                ClassAtom::Category(category) => {
                    body.push('\\');
                    body.push(category);
                }
                literal => {
                    let code = literal.literal()?;
                    if !(0xd800..=0xdfff).contains(&code) {
                        body += &format!("\\x{{{code:x}}}");
                    }
                }
            }
            n += 1;
        }
    }
    let translated = if body.is_empty() {
        if negative {
            "[\\s\\S]".into()
        } else {
            "(?!)".into()
        }
    } else {
        format!("[{}{body}]", if negative { "^" } else { "" })
    };
    Some((i, translated))
}

fn class_escape(chars: &[char], i: &mut usize) -> Option<ClassAtom> {
    *i += 1;
    let c = *chars.get(*i)?;
    *i += 1;
    if "dDsSwW".contains(c) {
        return Some(ClassAtom::Category(c));
    }
    if ('0'..='7').contains(&c) {
        let mut octal = c.to_digit(8)?;
        for _ in 0..2 {
            let Some(next) = chars.get(*i).and_then(|c| c.to_digit(8)) else {
                break;
            };
            octal = octal * 8 + next;
            *i += 1;
        }
        return (octal <= 0xff).then_some(ClassAtom::Literal(octal));
    }
    let code = match c {
        'a' => 7,
        'b' => 8,
        'f' => 12,
        'n' => 10,
        'r' => 13,
        't' => 9,
        'v' => 11,
        'x' | 'u' | 'U' => {
            let length = match c {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut code = 0u32;
            for _ in 0..length {
                code = code
                    .checked_mul(16)?
                    .checked_add(chars.get(*i)?.to_digit(16)?)?;
                *i += 1;
            }
            if code > 0x10ffff {
                return None;
            }
            code
        }
        'N' => named_character(chars, i)?,
        other if other.is_ascii_alphabetic() || other.is_ascii_digit() => return None,
        other => u32::from(other),
    };
    Some(ClassAtom::Literal(code))
}

fn named_character(chars: &[char], i: &mut usize) -> Option<u32> {
    if chars.get(*i) != Some(&'{') {
        return None;
    }
    let start = *i + 1;
    let end = start + chars.get(start..)?.iter().position(|c| *c == '}')?;
    let name: String = chars[start..end].iter().collect();
    let character = unicode_names2::character(&name)?;
    if let Some(canonical) = unicode_names2::name(character) {
        let canonical = canonical.to_string();
        // A consulta algorítmica também aceita zeros à esquerda que o nome oficial não tem.
        if (canonical.starts_with("CJK UNIFIED IDEOGRAPH-")
            || canonical.starts_with("HANGUL SYLLABLE "))
            && !canonical.eq_ignore_ascii_case(&name)
        {
            return None;
        }
    }
    *i = end + 1;
    Some(u32::from(character))
}
