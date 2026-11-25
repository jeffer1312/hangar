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
                guard(tool, input, value["cwd"].as_str())
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

/// O motivo para barrar a ferramenta, com os nomes e campos do Claude Code; `None` deixa passar. `cwd`: a
/// pasta do agente, que dá lugar ao caminho relativo.
fn guard(tool: &str, input: &Value, cwd: Option<&str>) -> Result<Option<String>, &'static str> {
    // PowerShell: a tool de shell do Claude Code no Windows, quando ligada.
    if !["Bash", "PowerShell", "Read", "Grep"].contains(&tool) {
        return Ok(None);
    }
    let place = Place::new(cwd);
    if !["Bash", "PowerShell"].contains(&tool) {
        let cites = |secret: &Secret| ["file_path", "path", "glob"].iter().any(|k| input[k].as_str().is_some_and(|text| (secret.hits)(&place, text) > 0));
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
    if reads(&VAULT, &place, command, 0, pwsh) {
        return Ok(Some(VAULT_DENIED.to_owned()));
    }
    if reads(&OWNER_KEYS, &place, command, 0, pwsh) {
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
            Some((tool, args)) => guard(tool, &args, value["cwd"].as_str()),
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
/// Programas que trocam a pasta onde o resto do comando roda.
const DIR_CHANGERS: &[&str] = &["cd", "chdir", "pushd", "popd", "set-location", "sl", "push-location", "pop-location"];
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

/// Onde o comando roda: o `cwd` do hook põe o caminho relativo no lugar, e o home e as raízes anotadas
/// dizem qual `backend/.env` é o do servidor.
#[derive(Clone)]
struct Place {
    /// `None`: pasta desconhecida (o hook não mandou, ou um `cd` que não dá para seguir).
    cwd: Option<PathBuf>,
    home: Option<PathBuf>,
    roots: std::cell::OnceCell<Vec<PathBuf>>,
}

impl Place {
    fn new(cwd: Option<&str>) -> Self {
        Place {
            cwd: cwd.filter(|cwd| !cwd.is_empty()).map(PathBuf::from),
            home: std::env::home_dir(),
            roots: std::cell::OnceCell::new(),
        }
    }

    /// A cópia do Hangar que o servidor usa: a do `current`, a anotada ou a acima do binário.
    fn roots(&self) -> &[PathBuf] {
        self.roots.get_or_init(|| {
            let Some(home) = &self.home else { return Vec::new() };
            [hangar_core::install_root::root(home), hangar_core::install_root::noted_root(home)]
                .into_iter()
                .flatten()
                .filter_map(|root| root.canonicalize().ok())
                .collect()
        })
    }

    /// Os nomes como pasta de verdade: `~` vira o home e o relativo junta ao `cwd`; sem `cwd`, `None`.
    fn locate(&self, rooted: bool, names: &[&str]) -> Option<PathBuf> {
        let drive = |name: &str| name.len() == 2 && name.ends_with(':') && name.as_bytes()[0].is_ascii_alphabetic();
        let (mut path, rest) = match names.first() {
            Some(&("~" | "$HOME" | "${HOME}")) if !rooted => (self.home.clone()?, &names[1..]),
            _ if rooted => (PathBuf::from("/"), names),
            Some(&first) if drive(first) => (PathBuf::from(format!("{first}\\")), &names[1..]),
            _ => (self.cwd.clone()?, names),
        };
        for name in rest {
            match *name {
                "." => {}
                ".." => {
                    path.pop();
                }
                other => path.push(other),
            }
        }
        Some(path)
    }

    /// A pasta é a raiz de uma cópia do Hangar: a do servidor ou um checkout dele. Outro projeto com
    /// `backend/.env` não é. Pasta desconhecida conta como cópia: na dúvida o guarda barra.
    fn hangar_copy(&self, rooted: bool, names: &[&str]) -> bool {
        self.locate(rooted, names).is_none_or(|dir| self.hangar_root(&dir))
    }

    fn hangar_root(&self, dir: &Path) -> bool {
        let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        self.roots().contains(&real) || real.join("crates").join("hangar-server").is_dir()
    }

    /// O mesmo lugar, com a pasta desconhecida.
    fn lost(&self) -> Place {
        let mut place = self.clone();
        place.cwd = None;
        place
    }

    /// A pasta depois do `cd` do segmento. Destino montado (variável, subcomando, curinga, `cd -`, `popd`),
    /// `cd` fora da posição de programa (`if cd x`) ou que talvez não valha para o resto (`detached`:
    /// subshell, pipeline, fundo) deixa a pasta desconhecida.
    fn follow(&mut self, segment: &str, pwsh: bool, detached: bool) {
        // No PowerShell a barra invertida é separador, não escape.
        let text = if pwsh { segment.replace('\\', "/") } else { segment.to_owned() };
        let words = tokens(&text);
        let (name, args) = head(&words);
        let name = name.to_ascii_lowercase();
        let targets: Vec<&String> = args.iter().filter(|arg| !arg.starts_with('-')).collect();
        self.cwd = match targets.as_slice() {
            _ if detached || !DIR_CHANGERS.contains(&name.as_str()) || name.starts_with("pop") || args.iter().any(|arg| arg == "-") => None,
            // Só o `cd` do shell sozinho volta ao home; os outros ficam ou trocam pela pilha.
            [] if !pwsh && name == "cd" => self.home.clone(),
            [target] => {
                let (rooted, names) = path_names(target);
                let expands = |index: usize, name: &&str| {
                    let home = index == 0 && ["~", "$HOME", "${HOME}"].contains(name);
                    !home && ((index == 0 && name.starts_with('~')) || name.contains(['$', '`', '%', '*', '?', '[', '{']))
                };
                if names.iter().enumerate().any(|(index, name)| expands(index, name)) { None } else { self.locate(rooted, &names) }
            }
            _ => None,
        };
    }
}

/// O comando tem subshell, substituição, pipeline ou fundo: um `cd` ali pode não valer para o resto.
fn detaches(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut quote = None;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => index += 1,
                '(' | ')' | '`' => return true,
                '|' | '&' if chars.get(index + 1) == Some(&c) => index += 1,
                // `2>&1` e `&>` são redirecionamento.
                '&' if (index > 0 && matches!(chars[index - 1], '<' | '>')) || chars.get(index + 1) == Some(&'>') => {}
                '|' | '&' => return true,
                _ => {}
            },
        }
        index += 1;
    }
    false
}

/// Alguma palavra do comando, também dentro de um `bash -c`, é um `cd`.
fn moves(command: &str) -> bool {
    shell_readings(command)
        .iter()
        .any(|reading| path_words(reading).any(|word| DIR_CHANGERS.contains(&word.to_ascii_lowercase().as_str())))
}

/// Um arquivo que o guarda não deixa ler, e por onde a leitura dele aparece num comando.
struct Secret {
    /// Citações do arquivo como caminho.
    hits: fn(&Place, &str) -> usize,
    /// `< arquivo`.
    redirect: fn(&Place, &str) -> bool,
    /// O `cd` que entra na pasta do arquivo.
    folder: fn(&Place, &str) -> bool,
    /// A pasta corrente é a do arquivo.
    within: fn(&Place) -> bool,
    /// Dentro da pasta, o segmento que lê o arquivo sem citar o caminho.
    bare: fn(&str) -> bool,
}

const VAULT: Secret = Secret { hits: vault_hits_at, redirect: vault_redirect, folder: vault_folder, within: vault_within, bare: any_segment };
const OWNER_KEYS: Secret =
    Secret { hits: owner_key_hits, redirect: owner_key_redirect, folder: owner_key_folder, within: owner_key_within, bare: owner_key_bare };

/// Ler o arquivo, não usá-lo: o caminho como argumento de outro programa passa. `pwsh`: o texto é
/// PowerShell (a tool PowerShell ou o script de um `powershell -c`).
fn reads(secret: &Secret, place: &Place, command: &str, depth: u8, pwsh: bool) -> bool {
    // Com `cd` a pasta de cada segmento é outra: o filtro rápido não vale e a varredura segue a pasta.
    let moves = moves(command);
    if !moves && (secret.hits)(place, command) == 0 && !(secret.folder)(place, command) && !(secret.within)(place) {
        return false;
    }
    let lost;
    let scope = if moves {
        lost = place.lost();
        &lost
    } else {
        place
    };
    // .NET (`[IO.File]::ReadAllText`, `ReadLines`, `OpenText`, `StreamReader`…); fora do PowerShell é só texto.
    let lower = command.to_ascii_lowercase();
    if pwsh && (secret.hits)(scope, command) > 0 && ["::readall", "::readlines", "::opentext", "::openread", "streamreader"].iter().any(|m| lower.contains(m)) {
        return true;
    }
    let command = strip_heredocs(command);
    let detached = detaches(&command);
    let mut here = place.clone();
    let mut inside = (secret.within)(place);
    segments(&command).iter().any(|segment| {
        let place = &here;
        let words = tokens(segment);
        let (_, args) = head(&words);
        let name = program(segment);
        let name = name.as_str();
        if is_shell(name) {
            // O texto cru guarda o caminho do Windows, que `tokens` desfaz; os pedaços pegam o shell atrás de `sudo`/`env`.
            let inner = ["powershell", "pwsh"].contains(&name);
            return depth < 3
                && (shell_script(segment).is_some_and(|s| reads(secret, place, &s, depth + 1, inner))
                    || reads(secret, place, &args.join(" "), depth + 1, inner));
        }
        // Entrar na pasta faz `cat *` ler o arquivo sem citar o caminho.
        if words.iter().any(|word| DIR_CHANGERS.contains(&word.to_ascii_lowercase().as_str())) {
            inside |= (secret.folder)(place, segment);
            here.follow(segment, pwsh, detached);
            inside |= (secret.within)(&here);
            if DIR_CHANGERS.contains(&name) {
                return false;
            }
        }
        let place = &here;
        if (secret.redirect)(place, segment) {
            return true;
        }
        let bare = inside && (secret.bare)(segment);
        if PATTERN_FIRST.contains(&name) {
            let file_option = args.windows(2).any(|w| FILE_OPTIONS.contains(&w[0].to_ascii_lowercase().as_str()) && (secret.hits)(place, &w[1]) > 0);
            // Mais citações do arquivo no texto cru do que no padrão: alguma é arquivo lido.
            let pattern = args.iter().find(|a| !a.starts_with('-')).map_or(0, |p| (secret.hits)(place, p));
            return bare || file_option || (secret.hits)(place, segment) > pattern;
        }
        READERS.contains(&name) && (bare || (secret.hits)(place, segment) > 0)
    })
}

fn any_segment(_: &str) -> bool {
    true
}

fn vault_hits_at(_: &Place, text: &str) -> usize {
    vault_hits(text)
}

fn vault_folder(_: &Place, text: &str) -> bool {
    vault_path(text)
}

fn vault_within(place: &Place) -> bool {
    place.cwd.as_ref().is_some_and(|cwd| vault_path(&cwd.to_string_lossy()))
}

fn vault_redirect(_: &Place, segment: &str) -> bool {
    static REDIRECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    REDIRECT.get_or_init(|| regex::Regex::new(r#"(?i)<\s*['"]?[^\s'"]*vault-files"#).unwrap()).is_match(segment)
}

const OWNER_KEYS_DENIED: &str = "BLOQUEADO: o `.env` do Hangar e o `control.key` do WhatsApp guardam as chaves do dono e não se leem.\nPara mandar algo pelo WhatsApp, crie o rascunho com `whatsapp_draft`: quem envia é o dono, no cartão do app.";
const DRAFT_ANSWER_DENIED: &str = "BLOQUEADO: aprovar ou recusar um rascunho de WhatsApp (`/api/whatsapp/outbox/<id>/send` ou `/decline`) é só do dono, no cartão do app.\nCrie o rascunho com `whatsapp_draft` e espere a resposta dele.";

/// As chaves do dono: o `control.key` do WhatsApp, o `.env` de `~/.hangar/config` e o `backend/.env` de uma
/// cópia do Hangar. Conta as palavras que as citam, na leitura que mais acha. Melhor esforço: caminho
/// montado por variável de ambiente passa.
fn owner_key_hits(place: &Place, text: &str) -> usize {
    shell_readings(text)
        .iter()
        .map(|reading| path_words(reading).filter(|word| owner_key_word(place, word)).count())
        .max()
        .unwrap_or(0)
}

fn owner_key_word(place: &Place, word: &str) -> bool {
    let (rooted, names) = path_names(word);
    // Sem o `.hangar` só o nome escrito: `*/*` casaria qualquer pasta com dois níveis.
    let literal = !names.iter().copied().any(is_glob);
    ends_with_glob(&names, &[".hangar", "config", ".env"])
        || ends_with_glob(&names, &[".hangar", "whatsapp", "control.key"])
        || (literal && ends_with_glob(&names, &["whatsapp", "control.key"]))
        || (ends_with_glob(&names, &["backend", ".env"]) && place.hangar_copy(rooted, &names[..names.len() - 2]))
}

/// O texto como o shell o entrega ao programa: sem aspas e, numa segunda leitura, também sem as barras
/// de escape (`control.ke"y"`, `.en\v`). A primeira guarda a barra, que é separador no Windows.
fn shell_readings(text: &str) -> [String; 2] {
    let unquoted: String = text.chars().filter(|c| !matches!(c, '\'' | '"')).collect();
    let mut unescaped = String::with_capacity(unquoted.len());
    let mut chars = unquoted.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            unescaped.extend(chars.next());
        } else {
            unescaped.push(c);
        }
    }
    [unquoted, unescaped]
}

/// As palavras do texto, com `.` e `..` resolvidos.
fn path_words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| c.is_whitespace() || ['=', '<', '>', ';', '|', '&', '(', ')', '`'].contains(&c))
        .filter(|word| !word.is_empty())
        .map(lexical_path)
}

/// Se o caminho começa na raiz, e os nomes dele sem as barras vazias.
fn path_names(word: &str) -> (bool, Vec<&str>) {
    (word.starts_with(['/', '\\']), word.split(['/', '\\']).filter(|name| !name.is_empty()).collect())
}

fn is_glob(name: &str) -> bool {
    name.contains(['*', '?', '[', '{'])
}

/// Os últimos nomes do caminho casam, como curinga de shell, com os nomes pedidos.
fn ends_with_glob(names: &[&str], wanted: &[&str]) -> bool {
    let Some(start) = names.len().checked_sub(wanted.len()) else { return false };
    let last = wanted.len() - 1;
    names[start..].iter().zip(wanted).enumerate().all(|(index, (name, want))| glob_match(name, want, index == last))
}

/// Um nome casa um curinga de shell (`*`, `?`, `[..]`, `{a,b}`), sem tocar o disco e sem caixa. Pasta
/// oculta só casa curinga que começa com ponto, como no bash; o arquivo final casa qualquer um, porque
/// o PowerShell e o `dotglob` pegam o `.env` com `*`.
fn glob_match(pattern: &str, name: &str, any_dot: bool) -> bool {
    let name: Vec<char> = name.chars().collect();
    let mut budget = 64;
    expand_braces(&pattern.chars().collect::<Vec<_>>(), &mut budget)
        .iter()
        .any(|pattern| (any_dot || name.first() != Some(&'.') || pattern.first() == Some(&'.')) && glob_one(pattern, &name))
}

/// As alternativas de `{a,b}`, até `budget` delas.
fn expand_braces(pattern: &[char], budget: &mut usize) -> Vec<Vec<char>> {
    let Some(open) = pattern.iter().position(|c| *c == '{') else { return vec![pattern.to_vec()] };
    let (mut depth, mut close, mut commas) = (0usize, None, Vec::new());
    for (index, c) in pattern.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(index),
            _ => {}
        }
    }
    let Some(close) = close.filter(|_| !commas.is_empty()) else {
        // Sem par ou sem vírgula a chave é letra; o resto ainda pode ter alternativas.
        return expand_braces(&pattern[open + 1..], budget)
            .into_iter()
            .map(|rest| pattern[..=open].iter().copied().chain(rest).collect())
            .collect();
    };
    let bounds: Vec<usize> = std::iter::once(open).chain(commas).chain(std::iter::once(close)).collect();
    let mut out = Vec::new();
    for pair in bounds.windows(2) {
        let alternative: Vec<char> = pattern[..open].iter().chain(&pattern[pair[0] + 1..pair[1]]).chain(&pattern[close + 1..]).copied().collect();
        for expanded in expand_braces(&alternative, budget) {
            if *budget == 0 {
                return out;
            }
            *budget -= 1;
            out.push(expanded);
        }
    }
    out
}

enum GlobPart {
    Any,
    One,
    Class(bool, Vec<(char, char)>),
    Char(char),
}

fn glob_one(pattern: &[char], name: &[char]) -> bool {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < pattern.len() {
        let part = match pattern[index] {
            '*' => GlobPart::Any,
            '?' => GlobPart::One,
            '[' => match glob_class(pattern, index) {
                Some((end, negated, ranges)) => {
                    parts.push(GlobPart::Class(negated, ranges));
                    index = end;
                    continue;
                }
                None => GlobPart::Char('['),
            },
            other => GlobPart::Char(other),
        };
        parts.push(part);
        index += 1;
    }
    // `reach[j]`: as partes lidas até aqui consomem `name[..j]`.
    let mut reach = vec![false; name.len() + 1];
    reach[0] = true;
    for part in &parts {
        let mut next = vec![false; name.len() + 1];
        for j in (0..=name.len()).filter(|j| reach[*j]) {
            if matches!(part, GlobPart::Any) {
                next[j..].iter_mut().for_each(|slot| *slot = true);
                break;
            }
            if name.get(j).is_some_and(|c| glob_char(part, *c)) {
                next[j + 1] = true;
            }
        }
        reach = next;
    }
    reach[name.len()]
}

fn glob_char(part: &GlobPart, c: char) -> bool {
    let c = c.to_ascii_lowercase();
    match part {
        GlobPart::Any | GlobPart::One => true,
        GlobPart::Char(want) => want.to_ascii_lowercase() == c,
        GlobPart::Class(negated, ranges) => {
            ranges.iter().any(|(low, high)| (low.to_ascii_lowercase()..=high.to_ascii_lowercase()).contains(&c)) != *negated
        }
    }
}

/// `[...]` a partir de `start`: onde termina, se nega (`!` ou `^`) e as faixas; sem `]`, não é classe.
fn glob_class(pattern: &[char], start: usize) -> Option<(usize, bool, Vec<(char, char)>)> {
    let mut index = start + 1;
    let negated = matches!(pattern.get(index), Some('!' | '^'));
    if negated {
        index += 1;
    }
    let first = index;
    let mut ranges = Vec::new();
    loop {
        let &c = pattern.get(index)?;
        if c == ']' && index != first {
            return Some((index + 1, negated, ranges));
        }
        match (pattern.get(index + 1), pattern.get(index + 2)) {
            (Some('-'), Some(&high)) if high != ']' => {
                ranges.push((c, high));
                index += 3;
            }
            _ => {
                ranges.push((c, c));
                index += 1;
            }
        }
    }
}

/// O texto com `.` e `..` resolvidos em cada caminho, sem tocar o disco: `config/./.env` e
/// `backend/../backend/.env` abrem o mesmo arquivo que o caminho direto.
fn lexical_paths(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("/.") && !text.contains("\\.") {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() || ['\'', '"', '=', '<', '>', ';', '|', '&', '(', ')', '`'].contains(&c) {
            out.push_str(&lexical_path(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&lexical_path(&word));
    std::borrow::Cow::Owned(out)
}

fn lexical_path(word: &str) -> String {
    let parts: Vec<&str> = word.split(['/', '\\']).collect();
    if parts.len() < 2 || !parts.iter().any(|p| *p == "." || *p == "..") {
        return word.to_owned();
    }
    let absolute = parts[0].is_empty();
    let mut kept: Vec<&str> = Vec::new();
    for part in parts {
        match part {
            "" | "." => {}
            ".." => match kept.last() {
                Some(&last) if last != ".." => {
                    kept.pop();
                }
                None if absolute => {}
                _ => kept.push(".."),
            },
            other => kept.push(other),
        }
    }
    let mut path = kept.join("/");
    if absolute {
        path.insert(0, '/');
    } else if path.is_empty() {
        path.push('.');
    }
    if word.ends_with(['/', '\\']) && !kept.is_empty() {
        path.push('/');
    }
    path
}

fn owner_key_redirect(place: &Place, segment: &str) -> bool {
    static REDIRECT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let redirect = REDIRECT.get_or_init(|| regex::Regex::new(r#"<\s*['"]?([^\s'"<]+)"#).unwrap());
    redirect.captures_iter(segment).any(|c| owner_key_hits(place, &c[1]) > 0)
}

/// `~/.hangar/whatsapp`, `~/.hangar/config` e a `backend/` de uma cópia do Hangar como pasta inteira (destino de um `cd`).
fn owner_key_folder(place: &Place, text: &str) -> bool {
    shell_readings(text).iter().any(|reading| {
        path_words(reading).any(|word| {
            let (rooted, names) = path_names(&word);
            ends_with_glob(&names, &[".hangar", "config"])
                || ends_with_glob(&names, &[".hangar", "whatsapp"])
                || (ends_with_glob(&names, &["backend"]) && place.hangar_copy(rooted, &names[..names.len() - 1]))
        })
    })
}

/// A pasta corrente é `~/.hangar/config`, `~/.hangar/whatsapp` ou a `backend/` de uma cópia do Hangar.
/// Desconhecida não conta: `cat *` numa pasta qualquer segue livre.
fn owner_key_within(place: &Place) -> bool {
    let Some(cwd) = &place.cwd else { return false };
    let names: Vec<String> = cwd.iter().map(|name| name.to_string_lossy().to_ascii_lowercase()).collect();
    let ends = |wanted: &[&str]| names.len() >= wanted.len() && names[names.len() - wanted.len()..].iter().zip(wanted).all(|(name, want)| name == want);
    ends(&[".hangar", "config"])
        || ends(&[".hangar", "whatsapp"])
        || (ends(&["backend"]) && cwd.parent().is_some_and(|parent| place.hangar_root(parent)))
}

/// Dentro da pasta, o nome solto (`.env`, `control.key`) ou um curinga que case com ele (`*`, `.en?`).
fn owner_key_bare(segment: &str) -> bool {
    shell_readings(segment).iter().any(|reading| {
        path_words(reading).any(|word| {
            let (_, names) = path_names(&word);
            names.len() == 1 && [".env", "control.key"].iter().any(|key| glob_match(names[0], key, true))
        })
    })
}

/// `Grep` em `~/.hangar` ou nas pastas das chaves as varre inteiras.
fn above_owner_keys(path: &str) -> bool {
    static ABOVE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ABOVE.get_or_init(|| regex::Regex::new(r#"(?i)(^|[/\\])\.hangar([/\\]+(whatsapp|config))?[/\\]*$"#).unwrap()).is_match(&lexical_paths(path))
}

/// Programas que só procuram ou registram texto: citar a rota neles não a chama.
const ROUTE_QUOTERS: &[&str] = &["grep", "egrep", "fgrep", "rg", "git", "select-string", "sls"];

/// `…/api/whatsapp/outbox/<id>/send` ou `/decline` chamado da sessão: a resposta ao rascunho é do dono, no app.
/// Confere o texto como o shell o entrega (sem aspas nem escapes) e a ação escrita como curinga (`{send,x}`).
fn answers_draft(command: &str) -> bool {
    static ROUTE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static GLOB: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let route = ROUTE.get_or_init(|| regex::Regex::new(r#"(?i)/api/whatsapp/outbox/[^\s'"/?#]+/+(send|decline)([^A-Za-z0-9_-]|$)"#).unwrap());
    let glob = GLOB.get_or_init(|| regex::Regex::new(r#"(?i)/api/whatsapp/outbox/[^\s'"/?#]+/+([A-Za-z0-9_*?\[\]{},!^-]+)"#).unwrap());
    let calls = |text: &str| {
        shell_readings(text).iter().any(|reading| {
            route.is_match(reading)
                || glob.captures_iter(reading).any(|c| ["send", "decline"].iter().any(|action| glob_match(&c[1], action, true)))
        })
    };
    if !calls(command) {
        return false;
    }
    segments(&strip_heredocs(command)).iter().any(|segment| {
        // Substituição de comando dentro das aspas roda, mesmo atrás de `git` ou `grep`.
        let quoted = ROUTE_QUOTERS.contains(&program(segment).as_str()) && !segment.contains("$(") && !segment.contains('`');
        calls(segment) && !quoted
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
