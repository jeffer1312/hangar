//! Hooks autônomos: sinais do Kimi e do Antigravity e guarda do multiplexador, sem servidor ou interpretador.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

pub fn command(home: &Path, mode: &str) -> Result<String, String> {
    crate::codex_setup::hook_command::hook_command(home, mode)
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
    let read = std::io::stdin().read_to_end(&mut input).map_err(|_| "stdin_read_failed");
    match mode {
        "agy-state-hook" => return agy_state(read.map(|_| input.as_slice())),
        "agy-guard-tmux" => return agy_guard(read.map(|_| input.as_slice())),
        _ => {}
    }
    let result = read
        .and_then(|_| {
            let value: Value = serde_json::from_str(&String::from_utf8_lossy(&input))
                .map_err(|_| "invalid_json")?;
            if !value.is_object() {
                return Err("payload_not_object");
            }
            if mode == "kimi-state-hook" {
                let home = std::env::home_dir().ok_or("home_unavailable")?;
                write_kimi(
                    &value,
                    &marker_base(&home),
                    std::env::var("TMUX_PANE").ok().as_deref(),
                    std::env::var("PSMUX_SESSION").ok().as_deref(),
                )?;
                Ok(None)
            } else {
                let tool = value["tool_name"].as_str().unwrap_or("");
                if !["Bash", "PowerShell", "Read", "Grep"].contains(&tool) {
                    return Ok(None);
                }
                let input = match value.get("tool_input") {
                    Some(input) if !input.is_object() => return Err("tool_input_not_object"),
                    Some(input) => input,
                    None => &Value::Null,
                };
                guard(tool, input)
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
            log_failure(if mode == "kimi-state-hook" { "kimi_hook_error.log" } else { "guard_tmux-falhas.log" }, code);
            ExitCode::SUCCESS
        }
    }
}

/// `CLAUDE_CONFIG_DIR` não vazio, senão `~/.claude`: onde moram os marcadores e os grupos.
fn marker_base(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
}

/// O motivo para barrar a ferramenta, com os nomes e campos do Claude Code; `None` deixa passar.
fn guard(tool: &str, input: &Value) -> Result<Option<String>, &'static str> {
    // PowerShell: a tool de shell do Claude Code no Windows, quando ligada.
    if !["Bash", "PowerShell", "Read", "Grep"].contains(&tool) {
        return Ok(None);
    }
    if !["Bash", "PowerShell"].contains(&tool) {
        let cites = |secret: &Secret| ["file_path", "path", "glob"].iter().any(|k| input[k].as_str().is_some_and(|text| (secret.hits)(text) > 0));
        let grep_above = |above: fn(&str) -> bool| tool == "Grep" && input["path"].as_str().is_some_and(above);
        if cites(&VAULT) || grep_above(above_vault) {
            return Ok(Some(VAULT_DENIED.to_owned()));
        }
        return Ok((cites(&OWNER_KEYS) || grep_above(above_owner_keys)).then(|| OWNER_KEYS_DENIED.to_owned()));
    }
    let command = &input["command"];
    if !crate::orchestration::truthy(command) {
        return Ok(None);
    }
    let command = command.as_str().ok_or("command_not_string")?;
    let pwsh = tool == "PowerShell";
    if reads(&VAULT, command, 0, pwsh) {
        return Ok(Some(VAULT_DENIED.to_owned()));
    }
    if reads(&OWNER_KEYS, command, 0, pwsh) {
        return Ok(Some(OWNER_KEYS_DENIED.to_owned()));
    }
    if answers_draft(command) {
        return Ok(Some(DRAFT_ANSWER_DENIED.to_owned()));
    }
    // Caminho do Windows sem aspas (PowerShell, ou o Bash do Codex no Windows): a barra
    // invertida é separador, não escape. Conferido também assim, só para barrar mais; só
    // nas palavras com cara de caminho, para não mexer no escape de um padrão do `pkill -f`.
    let reason = dangerous(command, 0).or_else(|| {
        let slashed = windows_paths_slashed(command);
        (slashed != command).then(|| dangerous(&slashed, 0)).flatten()
    });
    Ok(reason.map(|reason| format!(
        "BLOQUEADO: {reason}.\nPrecisa de tmux limpo para uma sonda? Suba um SERVIDOR SEPARADO com socket próprio:\n  tmux -L probe-x new-session -d -s alvo ...\n  tmux -L probe-x kill-server\nNunca no servidor default, onde vivem as sessões de trabalho (TMUX_TMPDIR não isola: dentro do tmux quem manda é $TMUX).\nPara matar UMA sessão: `tmux kill-session -t '=nome:'`."
    )))
}

/// A ferramenta do `agy` traduzida para a do Claude Code que a guarda conhece; `None` = não conferida.
fn agy_tool(value: &Value, windows: bool) -> Option<(&'static str, Value)> {
    let args = &value["toolCall"]["args"];
    let text = |key: &str| args[key].clone();
    match value["toolCall"]["name"].as_str()? {
        // No Windows o `run_command` roda no PowerShell.
        "run_command" => Some((if windows { "PowerShell" } else { "Bash" }, json!({"command": text("CommandLine")}))),
        "view_file" => Some(("Read", json!({"file_path": text("AbsolutePath")}))),
        "grep_search" => Some(("Grep", json!({"path": text("SearchPath")}))),
        _ => None,
    }
}

/// O `PreToolUse` do `agy` exige um `decision`: `{}` ou saída diferente de zero barram a ferramenta. Falha da
/// guarda deixa passar, com a linha no log.
fn agy_guard_decision(input: Result<&[u8], &'static str>, windows: bool) -> Value {
    let reason = input
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).map_err(|_| "invalid_json"))
        .and_then(|value| match agy_tool(&value, windows) {
            Some((tool, args)) => guard(tool, &args),
            None => Ok(None),
        });
    match reason {
        Ok(Some(reason)) => json!({"decision": "deny", "reason": reason}),
        Ok(None) => json!({"decision": "allow"}),
        Err(code) => {
            log_failure("guard_tmux-falhas.log", code);
            json!({"decision": "allow"})
        }
    }
}

fn agy_guard(input: Result<&[u8], &'static str>) -> ExitCode {
    let _ = writeln!(std::io::stdout().lock(), "{}", agy_guard_decision(input, cfg!(windows)));
    ExitCode::SUCCESS
}

/// Avisos que só existem no `agy`: dentro do sandbox do terminal o `~/.hangar` some e IP direto é recusado.
const AGY_SANDBOX: &str = "[hangar] Nesta sessão do Antigravity, `hangar-send` e `hangar-preview` só funcionam fora do sandbox do \
terminal: rode-os pelo run_command com BypassSandbox. Prefira as tools do MCP hangar (`sessions`, `send`, `group`, `browser_*`).";

/// O texto que o `agy` põe na conversa: na abertura, o aviso do sandbox e o protocolo do grupo; no primeiro
/// `PreInvocation` de cada mensagem, a dica do navegador. Só em sessão do Hangar.
fn agy_inject(value: &Value, home: &Path) -> Vec<String> {
    if std::env::var("CP_SESSION_KEY").map_or(true, |key| key.is_empty()) {
        return Vec::new();
    }
    match value["hookEventName"].as_str() {
        Some("SessionStart") => {
            let pair_dir = marker_base(home).join(".hangar-pair");
            std::iter::once(AGY_SANDBOX.to_owned())
                .chain(crate::groups::context_cmd::protocol_text(&pair_dir, "antigravity"))
                .collect()
        }
        // O número volta a 0 a cada mensagem da pessoa; as outras chamadas do turno não repetem a dica.
        Some("PreInvocation") if value["invocationNum"] == 0 => {
            let prompt = value["lastUserInput"].as_str().unwrap_or_default();
            hangar_agent_hooks::nav::hint_for(prompt, &hangar_agent_hooks::Context::from_process()).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// Os passos no formato do `agy` (`injectSteps` de `systemMessage`); sem nenhum, `{}`.
fn agy_output(steps: Vec<String>) -> Value {
    if steps.is_empty() {
        return json!({});
    }
    let steps: Vec<Value> = steps.into_iter().map(|text| json!({"systemMessage": {"systemMessage": text}})).collect();
    json!({"injectSteps": steps})
}

fn agy_state(input: Result<&[u8], &'static str>) -> ExitCode {
    let home = std::env::home_dir();
    let value = input
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).map_err(|_| "invalid_json"))
        .and_then(|value| if value.is_object() { Ok(value) } else { Err("payload_not_object") });
    let written = value.as_ref().map_err(|code| *code).and_then(|value| {
        let home = home.as_deref().ok_or("home_unavailable")?;
        write_agy(
            value,
            &marker_base(home),
            std::env::var("TMUX_PANE").ok().as_deref(),
            std::env::var("PSMUX_SESSION").ok().as_deref(),
        )
    });
    if let Err(code) = written {
        log_failure("agy_hook_error.log", code);
    }
    // O bilhete e o texto não dependem um do outro; falha do Hangar nunca prende o agente.
    let steps = match (&value, &home) {
        (Ok(value), Some(home)) => agy_inject(value, home),
        _ => Vec::new(),
    };
    let _ = writeln!(std::io::stdout().lock(), "{}", agy_output(steps));
    ExitCode::SUCCESS
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
        "UserPromptSubmit" => Some("working"),
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
/// Hook do `agy` (`hooks.json`): estado por conversa e o bilhete que liga o pane à conversa. O `agy acp` sem
/// terminal também roda os hooks e herda o `CP_SESSION_KEY` de quem o lançou: sem pane, nada é gravado.
fn write_agy(value: &Value, base: &Path, pane: Option<&str>, psmux: Option<&str>) -> Result<(), &'static str> {
    let key = pane_key(pane, psmux);
    if key.is_empty() { return Ok(()); }
    let conversation = value["conversationId"].as_str().ok_or("conversation_id_missing")?;
    if !hangar_antigravity::paths::valid_conversation_id(conversation) { return Err("conversation_id_invalid"); }
    let state = match value["hookEventName"].as_str().unwrap_or_default() {
        // O `SessionStart` também dispara ao retomar uma conversa parada: só o bilhete, o turno começa no `PreInvocation`.
        "PreInvocation" => Some("working"),
        // `fullyIdle` falso: o turno parou mas ainda há subagente ou tarefa em fundo trabalhando. Sem o campo,
        // o `Stop` já diz que o turno acabou.
        "Stop" => Some(if value["fullyIdle"] == false { "working" } else { "idle" }),
        _ => None,
    };
    if let Some(state) = state {
        marker(base, ".hangar-state", conversation, &json!({"state": state, "ts": timestamp()}))?;
    }
    marker(base, ".hangar-agy", &key, &json!({
        "session_id": conversation,
        "transcript_path": value.get("transcriptPath").unwrap_or(&Value::Null),
        "cwd": value.get("cwd").unwrap_or(&Value::Null),
        "ts": timestamp(),
    }))
}

/// A chave do bilhete: o nome da sessão no psmux, senão o pane do tmux sem o `%`.
fn pane_key(pane: Option<&str>, psmux: Option<&str>) -> String {
    if let Some(name) = psmux.filter(|s| !s.is_empty()) {
        return name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
    }
    pane.unwrap_or_default().trim_start_matches('%').to_owned()
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
/// Troca `\` por `/` só nas palavras que começam como caminho do Windows (`C:\`, `\\`, `.\`, `..\`).
fn windows_paths_slashed(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut in_path = false;
    let mut word_start = true;
    for (index, ch) in command.char_indices() {
        if ch.is_whitespace() {
            in_path = false;
            word_start = true;
            out.push(ch);
            continue;
        }
        if word_start {
            let rest = command[index..].trim_start_matches(['"', '\'', '&', '(']).as_bytes();
            in_path = matches!(rest, [drive, b':', b'\\', ..] if drive.is_ascii_alphabetic())
                || rest.starts_with(b"\\\\")
                || rest.starts_with(b".\\")
                || rest.starts_with(b"..\\");
            word_start = false;
        }
        out.push(if in_path && ch == '\\' { '/' } else { ch });
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
        // A barra invertida que sobra depois das aspas é de caminho do Windows, e o `.exe` não muda o
        // programa: `"C:\tools\tmux.exe" kill-server` é o tmux.
        let name = token_name.rsplit(['/', '\\']).next().unwrap_or_default();
        let cut = name.len().saturating_sub(4);
        let name = match name.get(cut..) {
            Some(tail) if tail.eq_ignore_ascii_case(".exe") => &name[..cut],
            _ => name,
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
    "certutil", "mapfile", "readarray", "read", "source", ".",
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

/// O programa do segmento, sem pasta, caixa nem `.exe`.
fn program(segment: &str) -> String {
    let words = tokens(segment);
    let (name, _) = head(&words);
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
    name.strip_suffix(".exe").map(str::to_owned).unwrap_or(name)
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name) || ["powershell", "pwsh", "cmd"].contains(&name)
}

/// O corpo de um heredoc é texto (mensagem, arquivo), salvo quando vai a um shell (`bash <<EOF`): fora
/// disso sai, e o que vem depois do terminador continua conferido. Here-string (`<<<`) é redirecionamento.
fn strip_heredocs(command: &str) -> String {
    static START: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let start_re = START.get_or_init(|| regex::Regex::new(r#"<<(-?)\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?"#).unwrap());
    let mut text = command.replace("<<<", " < ");
    let mut from = 0;
    while let Some(caps) = start_re.captures(&text[from..]) {
        let whole = caps.get(0).unwrap();
        let (start, end) = (from + whole.start(), from + whole.end());
        let (indented, word) = (!caps[1].is_empty(), caps[2].to_owned());
        let to_shell = segments(&text[..start]).last().is_some_and(|s| is_shell(&program(s)));
        let body = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
        let (mut pos, mut after) = (body, text.len());
        while pos < text.len() {
            let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
            let line = &text[pos..line_end];
            if (if indented { line.trim_start_matches('\t') } else { line }) == word {
                after = line_end;
                break;
            }
            pos = line_end + 1;
        }
        if to_shell {
            text.replace_range(start..end, &" ".repeat(end - start));
            from = end;
        } else {
            let marker_line = text[end..body].to_owned();
            text.replace_range(start..after, &marker_line);
            from = start + marker_line.len();
        }
    }
    text
}

/// O comando que um shell recebe, cru (barras invertidas inteiras): o que vem depois de `-c`, `-Command`,
/// `-File`, `/c` ou `/k`, mesmo com opções de valor antes (`-ExecutionPolicy Bypass`, `-o pipefail`).
fn shell_script(segment: &str) -> Option<String> {
    static SHELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static FLAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let shell = SHELL.get_or_init(|| {
        regex::Regex::new(r#"(?is)^\s*(?:\S*[/\\])?(sh|bash|zsh|dash|ksh|ash|busybox|eval|powershell|pwsh|cmd)(?:\.exe)?(\s.*)?$"#).unwrap()
    });
    let flag = FLAG.get_or_init(|| regex::Regex::new(r#"(?is)^.*?\s(?:-[a-z]*c|-command|-file|/c|/k)\s+(.*)$"#).unwrap());
    let caps = shell.captures(segment)?;
    let rest = caps.get(2).map_or("", |m| m.as_str());
    let script = if caps[1].eq_ignore_ascii_case("eval") { rest.trim() } else { flag.captures(rest)?.get(1)?.as_str().trim() };
    let unquoted = ['\'', '"'].iter().find_map(|q| script.strip_prefix(*q).and_then(|r| r.strip_suffix(*q)));
    Some(unquoted.unwrap_or(script).to_owned())
}

/// Um arquivo que o guarda não deixa ler, e por onde a leitura dele aparece num comando.
struct Secret {
    /// Citações do arquivo como caminho.
    hits: fn(&str) -> usize,
    /// `< arquivo`.
    redirect: fn(&str) -> bool,
    /// O `cd` que entra na pasta do arquivo.
    folder: fn(&str) -> bool,
    /// Dentro da pasta, o segmento que lê o arquivo sem citar o caminho.
    bare: fn(&str) -> bool,
}

const VAULT: Secret = Secret { hits: vault_hits, redirect: vault_redirect, folder: vault_path, bare: any_segment };
const OWNER_KEYS: Secret = Secret { hits: owner_key_hits, redirect: owner_key_redirect, folder: owner_key_folder, bare: owner_key_bare };

/// Ler o arquivo, não usá-lo: o caminho como argumento de outro programa passa. `pwsh`: o texto é
/// PowerShell (a tool PowerShell ou o script de um `powershell -c`).
fn reads(secret: &Secret, command: &str, depth: u8, pwsh: bool) -> bool {
    if (secret.hits)(command) == 0 && !(secret.folder)(command) {
        return false;
    }
    // .NET (`[IO.File]::ReadAllText`, `ReadLines`, `OpenText`, `StreamReader`…); fora do PowerShell é só texto.
    let lower = command.to_ascii_lowercase();
    if pwsh && (secret.hits)(command) > 0 && ["::readall", "::readlines", "::opentext", "::openread", "streamreader"].iter().any(|m| lower.contains(m)) {
        return true;
    }
    let command = strip_heredocs(command);
    let mut inside = false;
    segments(&command).iter().any(|segment| {
        let words = tokens(segment);
        let (_, args) = head(&words);
        let name = program(segment);
        let name = name.as_str();
        if is_shell(name) {
            // O texto cru guarda o caminho do Windows, que `tokens` desfaz; os pedaços pegam o shell atrás de `sudo`/`env`.
            let inner = ["powershell", "pwsh"].contains(&name);
            return depth < 3
                && (shell_script(segment).is_some_and(|s| reads(secret, &s, depth + 1, inner))
                    || reads(secret, &args.join(" "), depth + 1, inner));
        }
        // Entrar na pasta faz `cat *` ler o arquivo sem citar o caminho.
        if ["cd", "pushd", "set-location", "sl", "push-location"].contains(&name) {
            inside |= (secret.folder)(segment);
            return false;
        }
        if (secret.redirect)(segment) {
            return true;
        }
        let bare = inside && (secret.bare)(segment);
        if PATTERN_FIRST.contains(&name) {
            let file_option = args.windows(2).any(|w| FILE_OPTIONS.contains(&w[0].to_ascii_lowercase().as_str()) && (secret.hits)(&w[1]) > 0);
            // Mais citações do arquivo no texto cru do que no padrão: alguma é arquivo lido.
            let pattern = args.iter().find(|a| !a.starts_with('-')).map_or(0, |p| (secret.hits)(p));
            return bare || file_option || (secret.hits)(segment) > pattern;
        }
        READERS.contains(&name) && (bare || (secret.hits)(segment) > 0)
    })
}

fn any_segment(_: &str) -> bool {
    true
}

fn vault_redirect(segment: &str) -> bool {
    static REDIRECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    REDIRECT.get_or_init(|| regex::Regex::new(r#"(?i)<\s*['"]?[^\s'"]*vault-files"#).unwrap()).is_match(segment)
}

const OWNER_KEYS_DENIED: &str = "BLOQUEADO: o `.env` do Hangar e o `control.key` do WhatsApp guardam as chaves do dono e não se leem.\nPara mandar algo pelo WhatsApp, crie o rascunho com `whatsapp_draft`: quem envia é o dono, no cartão do app.";
const DRAFT_ANSWER_DENIED: &str = "BLOQUEADO: aprovar ou recusar um rascunho de WhatsApp (`/api/whatsapp/outbox/<id>/send` ou `/decline`) é só do dono, no cartão do app.\nCrie o rascunho com `whatsapp_draft` e espere a resposta dele.";

/// As chaves do dono: o `control.key` do WhatsApp e o `.env` do Hangar (`~/.hangar/config/.env`, `backend/.env`).
fn owner_key_hits(text: &str) -> usize {
    static KEYS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        regex::Regex::new(r#"(?i)(^|[/\\\s'"=<])(\.hangar[/\\]+config[/\\]+\.env|backend[/\\]+\.env|whatsapp[/\\]+control\.key)([\s'"*;|&)]|$)"#).unwrap()
    })
    .find_iter(text)
    .count()
}

fn owner_key_redirect(segment: &str) -> bool {
    static REDIRECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let redirect = REDIRECT.get_or_init(|| regex::Regex::new(r#"<\s*['"]?([^\s'"<]+)"#).unwrap());
    redirect.captures_iter(segment).any(|c| owner_key_hits(&c[1]) > 0)
}

/// `~/.hangar/whatsapp`, `~/.hangar/config` e `backend` como pasta inteira (destino de um `cd`).
fn owner_key_folder(text: &str) -> bool {
    static FOLDER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    FOLDER
        .get_or_init(|| regex::Regex::new(r#"(?i)(^|[/\\\s'"=])(\.hangar[/\\]+(whatsapp|config)|backend)[/\\]*(['"\s;|&)]|$)"#).unwrap())
        .is_match(text)
}

/// O nome solto (`.env`, `control.key`) ou `*`: na pasta do `backend` o resto continua livre.
fn owner_key_bare(segment: &str) -> bool {
    static BARE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    BARE.get_or_init(|| {
        regex::Regex::new(r#"(?i)(^|[/\\\s'"=<])(\.env|control\.key)([\s'"*;|&)]|$)|(^|\s)\*(\s|$)"#).unwrap()
    })
    .is_match(segment)
}

/// `Grep` em `~/.hangar` ou nas pastas das chaves as varre inteiras.
fn above_owner_keys(path: &str) -> bool {
    static ABOVE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ABOVE.get_or_init(|| regex::Regex::new(r#"(?i)(^|[/\\])\.hangar([/\\]+(whatsapp|config))?[/\\]*$"#).unwrap()).is_match(path)
}

/// Programas que só procuram ou registram texto: citar a rota neles não a chama.
const ROUTE_QUOTERS: &[&str] = &["grep", "egrep", "fgrep", "rg", "git", "select-string", "sls"];

/// `…/api/whatsapp/outbox/<id>/send` ou `/decline` chamado da sessão: a resposta ao rascunho é do dono, no app.
fn answers_draft(command: &str) -> bool {
    static ROUTE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let route = ROUTE.get_or_init(|| regex::Regex::new(r#"(?i)/api/whatsapp/outbox/[^\s'"/?#]+/+(send|decline)([^A-Za-z0-9_-]|$)"#).unwrap());
    if !route.is_match(command) {
        return false;
    }
    segments(&strip_heredocs(command)).iter().any(|segment| {
        // Substituição de comando dentro das aspas roda, mesmo atrás de `git` ou `grep`.
        let quoted = ROUTE_QUOTERS.contains(&program(segment).as_str()) && !segment.contains("$(") && !segment.contains('`');
        route.is_match(segment) && !quoted
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
