//! "Pedir ajuda ao <agente>" (spec "Consertar com o agente"): Claude Code ou Codex, instalado e logado, roda sem terminal
//! com o relatório. Sem sudo e sem senha: o ambiente perde a senha do celular, o askpass e o token do servidor, e cada
//! comando que ele roda vira uma linha de "Ver detalhes".
use std::{io::{BufRead, BufReader, Read, Write}, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};
use serde_json::Value;
use super::run;
use super::system::{find_program, hidden};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Agent { Claude, Codex }

impl Agent {
    /// Ordem dos botões: o Claude Code é o agente padrão do instalador.
    pub(crate) const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];
    pub(crate) fn id(self) -> &'static str { match self { Agent::Claude => "claude", Agent::Codex => "codex" } }
    pub(crate) fn name(self) -> &'static str { match self { Agent::Claude => "Claude Code", Agent::Codex => "Codex" } }
}

/// Nunca chegam ao agente (`CLAUDECODE` faria o claude filho se achar aninhado): senha do celular, código e atalho do askpass, token do servidor e qualquer outro askpass
/// (`SUDO_ASKPASS` por precaução: o app não o põe, o script usa `sudo -S`).
pub(crate) const REMOVED_ENV: [&str; 9] = ["CLAUDECODE", "HANGAR_TOKEN", "HANGAR_ASKPASS", crate::app::ASKPASS_CODE_ENV, "CP_AUTH_TOKEN", "SSH_ASKPASS",
    "SSH_ASKPASS_REQUIRE", "GIT_ASKPASS", "SUDO_ASKPASS"];

/// Sem senha de administrador e sem mexer no histórico da pasta: o app desfaz pelo `git status`, e commit ou reset
/// esconderiam a edição dele.
pub(crate) const CLAUDE_DENIED: [&str; 20] = ["Bash(sudo *)", "Bash(su *)", "Bash(doas *)", "Bash(pkexec *)", "Bash(runas *)",
    "Bash(git commit *)", "Bash(git push *)", "Bash(git reset *)", "Bash(git checkout *)", "Bash(git restore *)", "Bash(git stash *)",
    "Bash(git clean *)", "Bash(git switch *)", "Bash(git rebase *)", "Bash(git merge *)", "Bash(git pull *)",
    // `git -C <pasta> commit` e `bash -c 'sudo …'` escapam das regras que casam só o começo do comando.
    "Bash(git -C *)", "Bash(git -c *)", "Bash(/usr/bin/sudo *)", "Bash(*sudo *)"];

/// Teto do conserto: passou, o app pára o agente e segue com o que houver.
pub(crate) const MAX_RUN: Duration = Duration::from_secs(20 * 60);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);

const RULES: &str = "You were called by the Hangar installer app because installing Hangar on this computer failed. \
A person who is not a programmer is waiting. The installer report is at the end, between <report> tags; passwords, the user \
name, IP addresses and Tailscale names in it were replaced.

What you may do:
- Fix what is outside the Hangar folder and needs no administrator rights: a missing user-level program, the PATH, the \
user's shell or service configuration.
- Edit files inside the Hangar folder to make the installer work. Every edit there is undone when you finish and sent to the \
Hangar maintainer, who will apply the fix for everyone.
- Run the installer again with the command given below to check your fix.

What you must not do:
- Never run sudo, su, doas, pkexec or runas, and never ask for or type a password. If something needs administrator rights, \
stop and write the exact command and why it is needed; the person decides.
- Never commit, push, reset, checkout, restore, stash or clean in the Hangar folder.

Finish with a short explanation for the person: what was wrong, what you changed, and what is still pending.";

pub(crate) fn agent_env(base: impl IntoIterator<Item = (String, String)>, path: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = base.into_iter()
        // Nome de variável no Windows não diferencia maiúsculas: "Path" é o PATH.
        .filter(|(key, _)| !REMOVED_ENV.iter().any(|r| r.eq_ignore_ascii_case(key)) && !key.eq_ignore_ascii_case("PATH"))
        .collect();
    env.push(("PATH".into(), path.into()));
    // Nenhum git do agente espera senha num terminal que não existe.
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    env
}

/// Só para provar telas: `HANGAR_SETUP_FAKE_CLAUDE`/`HANGAR_SETUP_FAKE_CODEX` trocam o programa pelo dublê.
pub(crate) fn program(agent: Agent, path: &str) -> Option<PathBuf> {
    let fake = match agent { Agent::Claude => "HANGAR_SETUP_FAKE_CLAUDE", Agent::Codex => "HANGAR_SETUP_FAKE_CODEX" };
    std::env::var_os(fake).map(PathBuf::from).filter(|p| p.is_file()).or_else(|| find_program(agent.id(), path))
}

/// `claude auth status` sai 1 deslogado e imprime o JSON do mesmo jeito: vale o `loggedIn`.
pub(crate) fn claude_logged_in(stdout: &str) -> bool {
    serde_json::from_str::<Value>(stdout).ok().and_then(|v| v.get("loggedIn")?.as_bool()).unwrap_or(false)
}

pub(crate) fn login_args(agent: Agent) -> &'static [&'static str] {
    match agent { Agent::Claude => &["auth", "status", "--json"], Agent::Codex => &["login", "status"] }
}

/// Agentes instalados **e** logados, sem abrir janela: sem login o botão não aparece (spec).
pub(crate) fn available(path: &str) -> Vec<Agent> {
    let env = agent_env(std::env::vars(), path);
    Agent::ALL.into_iter().filter(|agent| program(*agent, path).is_some_and(|program| logged_in(*agent, &program, &env))).collect()
}

fn logged_in(agent: Agent, program: &Path, env: &[(String, String)]) -> bool {
    let mut command = Command::new(program);
    hidden(&mut command).args(login_args(agent)).env_clear().envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null());
    let Some((success, stdout)) = run_within(&mut command, LOGIN_TIMEOUT) else { return false };
    match agent { Agent::Claude => claude_logged_in(&stdout), Agent::Codex => success }
}

/// Saída de um comando curto com teto: um CLI travado não segura o painel da falha.
fn run_within(command: &mut Command, limit: Duration) -> Option<(bool, String)> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || { let mut text = String::new(); let _ = stdout.read_to_string(&mut text); text });
    let started = Instant::now();
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() { break status; }
        if started.elapsed() > limit { let _ = child.kill(); let _ = child.wait(); return None; }
        std::thread::sleep(Duration::from_millis(100));
    };
    Some((status.success(), reader.join().ok()?))
}

/// `home` e a pasta do Hangar são os únicos lugares onde o agente escreve por ferramenta; o prompt vai pela entrada.
pub(crate) fn args(agent: Agent, home: &Path, dest: &Path, work: &Path) -> Vec<String> {
    let (home, dest) = (home.to_string_lossy().into_owned(), dest.to_string_lossy().into_owned());
    match agent {
        Agent::Claude => {
            let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "dontAsk",
                "--no-session-persistence", "--max-turns", "60", "--allowedTools", "Bash", "Read", "Edit", "Write", "Glob", "Grep",
                "--disallowedTools"].map(String::from).into();
            args.extend(CLAUDE_DENIED.map(String::from));
            args.extend(["--add-dir".to_owned(), home, dest]);
            args
        }
        Agent::Codex => vec!["exec".into(), "--json".into(), "--sandbox".into(), "workspace-write".into(),
            "-c".into(), "approval_policy=\"never\"".into(), "-c".into(), "sandbox_workspace_write.network_access=true".into(),
            "--skip-git-repo-check".into(), "--ephemeral".into(), "-C".into(), work.to_string_lossy().into_owned(),
            "--add-dir".into(), home, "--add-dir".into(), dest, "-".into()],
    }
}

/// O comando que o agente usa para conferir. Sem a senha do celular gravada, rodar tudo geraria outra senha (o agente
/// não recebe `HANGAR_TOKEN`): aí ele só confere (`--check` não escreve nada).
pub(crate) fn recheck_command(dest: &Path, options: &run::Options, token_saved: bool, windows: bool) -> String {
    let kind = if token_saved { run::Kind::Install } else { run::Kind::Check };
    // O agente só existe no clone (`detect_agents`).
    let args = run::script_args(kind, options, super::local::Mode::Git, dest, windows, false);
    if windows {
        format!("powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\" {}", dest.join("install.ps1").display(), args.join(" "))
    } else {
        // O primeiro argumento do bootstrap é a pasta; o `install.sh` já roda de dentro dela.
        let script = format!("{}/install.sh", dest.to_string_lossy().trim_end_matches('/')).replace('\'', r"'\''");
        format!("bash '{script}' {}", args[1..].join(" "))
    }
}

pub(crate) fn prompt(report: &str, dest: &Path, recheck: &str, english: bool) -> String {
    let language = if english { "English" } else { "Brazilian Portuguese" };
    format!("{RULES}\n\nHangar folder: {}\nTo check your fix, run: {recheck}\nWrite the final explanation in {language}.\n\n<report>\n{report}\n</report>\n",
        dest.display())
}

/// O que o agente fez, lido dos eventos (`stream-json` do Claude, `--json` do Codex). Linha que não é JSON é saída crua.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Transcript {
    /// "Ver detalhes": cada comando, cada arquivo editado e a saída crua.
    pub lines: Vec<String>,
    pub commands: Vec<String>,
    pub edits: Vec<String>,
    pub explanation: Option<String>,
    /// O agente terminou com erro (limite de turnos, falha da API): a explicação é o motivo, não um conserto.
    pub failed: bool,
}

impl Transcript {
    pub(crate) fn feed(&mut self, agent: Agent, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            let line = line.trim_end();
            if !line.is_empty() { self.lines.push(line.to_owned()); }
            return;
        };
        match agent { Agent::Claude => self.feed_claude(&event), Agent::Codex => self.feed_codex(&event) }
    }

    fn command(&mut self, command: &str) {
        self.lines.push(format!("$ {command}"));
        self.commands.push(command.to_owned());
    }

    fn edit(&mut self, path: &str) {
        self.lines.push(format!("✎ {path}"));
        if !self.edits.iter().any(|p| p == path) { self.edits.push(path.to_owned()); }
    }

    fn feed_claude(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => for block in event.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
                    self.lines.extend(text.lines().map(str::trim_end).filter(|l| !l.is_empty()).map(str::to_owned));
                    continue;
                }
                if block.get("type").and_then(Value::as_str) != Some("tool_use") { continue; }
                let input = block.get("input");
                match block.get("name").and_then(Value::as_str) {
                    Some("Bash") => if let Some(c) = input.and_then(|i| i.get("command")).and_then(Value::as_str) { self.command(c) },
                    Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") =>
                        if let Some(p) = input.and_then(|i| i.get("file_path")).and_then(Value::as_str) { self.edit(p) },
                    _ => {}
                }
            },
            Some("result") if event.get("is_error").and_then(Value::as_bool) == Some(true) => {
                let reason = event.get("subtype").and_then(Value::as_str).unwrap_or("error");
                let detail = event.get("result").and_then(Value::as_str).unwrap_or_default();
                self.failed = true;
                self.explanation = Some(format!("{reason}: {detail}").trim_end_matches([':', ' ']).to_owned());
            }
            Some("result") => if let Some(text) = event.get("result").and_then(Value::as_str) { self.explanation = Some(text.to_owned()) },
            _ => {}
        }
    }

    fn feed_codex(&mut self, event: &Value) {
        let item = event.get("item");
        let field = |name: &str| item.and_then(|i| i.get(name));
        match (event.get("type").and_then(Value::as_str), field("type").and_then(Value::as_str)) {
            (Some("item.started"), Some("command_execution")) => if let Some(c) = field("command").and_then(Value::as_str) { self.command(c) },
            (Some("item.completed"), Some("file_change")) => for change in field("changes").and_then(Value::as_array).into_iter().flatten() {
                if let Some(p) = change.get("path").and_then(Value::as_str) { self.edit(p) }
            },
            (Some("item.completed"), Some("agent_message")) => if let Some(text) = field("text").and_then(Value::as_str) { self.explanation = Some(text.to_owned()) },
            _ => {}
        }
    }
}

pub(crate) enum Output { Line(String), Exit }

pub(crate) struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub work: PathBuf,
    pub env: Vec<(String, String)>,
    pub prompt: String,
}

/// Sobe o agente destacado (sessão própria no Linux, sem janela no Windows), com o ambiente exatamente o de `env`.
pub(crate) fn spawn(launch: Launch) -> Result<(u32, async_channel::Receiver<Output>), String> {
    std::fs::create_dir_all(&launch.work).map_err(|e| e.to_string())?;
    let mut command = Command::new(&launch.program);
    command.args(&launch.args).current_dir(&launch.work).env_clear().envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    run::detach(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let (tx, rx) = async_channel::unbounded();
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let prompt = launch.prompt;
    // O relatório vai pela entrada: no Windows a linha de comando pára em 32 mil caracteres.
    std::thread::spawn(move || { if let Some(mut stdin) = stdin { let _ = stdin.write_all(prompt.as_bytes()); } });
    let pipes: Vec<Box<dyn Read + Send>> = [stdout.map(|p| Box::new(p) as Box<dyn Read + Send>), stderr.map(|p| Box::new(p) as Box<dyn Read + Send>)]
        .into_iter().flatten().collect();
    let readers: Vec<_> = pipes.into_iter().map(|pipe| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            // Byte inválido vira U+FFFD: parar de ler deixaria o agente travado no cano cheio.
            let (mut reader, mut buf) = (BufReader::new(pipe), Vec::new());
            while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_owned();
                let _ = tx.send_blocking(Output::Line(line));
                buf.clear();
            }
        })
    }).collect();
    std::thread::spawn(move || {
        let _ = child.wait();
        for reader in readers { let _ = reader.join(); }
        let _ = tx.send_blocking(Output::Exit);
    });
    Ok((pid, rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_drops_every_secret_and_sets_the_refreshed_path() {
        let base = [("HANGAR_TOKEN", "t0ken"), ("HANGAR_ASKPASS", "/cfg/askpass.sh"), ("SUDO_ASKPASS", "/cfg/askpass.sh"),
            ("HANGAR_ASKPASS_CODE", "c0de"), ("CP_AUTH_TOKEN", "x"),
            ("ssh_askpass", "y"), ("GIT_ASKPASS", "z"), ("SSH_ASKPASS_REQUIRE", "force"), ("Path", "/velho"), ("HOME", "/home/dev"),
            ("ANTHROPIC_API_KEY", "k")].map(|(k, v)| (k.to_owned(), v.to_owned()));
        let env = agent_env(base, "/novo");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        // O login do próprio agente (chave de API, HOME) fica; senha do Hangar e qualquer askpass saem.
        assert_eq!(keys, vec!["HOME", "ANTHROPIC_API_KEY", "PATH", "GIT_TERMINAL_PROMPT"]);
        assert!(!agent_env([("CLAUDECODE".to_owned(), "1".to_owned())], "/novo").iter().any(|(k, _)| k == "CLAUDECODE"));
        assert!(env.contains(&("PATH".to_owned(), "/novo".to_owned())));
        assert_eq!(crate::app::ASKPASS_CODE_ENV, "HANGAR_ASKPASS_CODE");
    }

    #[test]
    fn claude_runs_print_mode_without_prompts_or_sudo() {
        let args = args(Agent::Claude, Path::new("/home/dev"), Path::new("/home/dev/hangar"), Path::new("/cfg/setup/agent"));
        let joined = args.join(" ");
        for needle in ["-p --output-format stream-json --verbose", "--permission-mode dontAsk", "--no-session-persistence",
            "--allowedTools Bash Read Edit Write Glob Grep", "Bash(sudo *)", "Bash(pkexec *)", "Bash(git commit *)", "Bash(git reset *)", "Bash(git -C *)", "Bash(git -c *)", "Bash(/usr/bin/sudo *)", "Bash(*sudo *)"] {
            assert!(joined.contains(needle), "{needle}: {joined}");
        }
        assert_eq!(&args[args.len() - 3..], ["--add-dir", "/home/dev", "/home/dev/hangar"]);
        assert!(!joined.contains("bypassPermissions") && !joined.contains("dangerously") && !joined.contains("--bare"));
    }

    #[test]
    fn codex_runs_exec_in_the_workspace_sandbox_without_approvals() {
        let args = args(Agent::Codex, Path::new("/home/dev"), Path::new("/home/dev/hangar"), Path::new("/cfg/setup/agent"));
        let joined = args.join(" ");
        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        for needle in ["--json", "--sandbox workspace-write", "approval_policy=\"never\"", "sandbox_workspace_write.network_access=true",
            "--skip-git-repo-check", "--ephemeral", "-C /cfg/setup/agent", "--add-dir /home/dev --add-dir /home/dev/hangar"] {
            assert!(joined.contains(needle), "{needle}: {joined}");
        }
        assert!(!joined.contains("danger-full-access") && !joined.contains("dangerously"));
    }

    #[test]
    fn login_is_read_from_the_cli() {
        assert!(claude_logged_in(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#));
        assert!(!claude_logged_in(r#"{"loggedIn": false, "authMethod": "none"}"#));
        assert!(!claude_logged_in("Error: unknown command"));
        assert_eq!(login_args(Agent::Claude), ["auth", "status", "--json"]);
        assert_eq!(login_args(Agent::Codex), ["login", "status"]);
    }

    #[test]
    fn claude_events_become_commands_edits_and_the_explanation() {
        let mut t = Transcript::default();
        for line in [r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"olhando"},{"type":"tool_use","name":"Bash","input":{"command":"systemctl --user status"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/home/dev/hangar/install.sh"}}]}}"#,
            "aviso cru no stderr",
            r#"{"type":"result","subtype":"success","result":"Consertei o PATH."}"#] { t.feed(Agent::Claude, line); }
        assert_eq!(t.commands, vec!["systemctl --user status"]);
        assert_eq!(t.edits, vec!["/home/dev/hangar/install.sh"]);
        assert_eq!(t.explanation.as_deref(), Some("Consertei o PATH."));
        assert!(!t.failed);
        let mut failed = Transcript::default();
        failed.feed(Agent::Claude, r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert!(failed.failed);
        assert_eq!(failed.explanation.as_deref(), Some("error_max_turns"));
        assert_eq!(t.lines, vec!["olhando", "$ systemctl --user status", "✎ /home/dev/hangar/install.sh", "aviso cru no stderr"]);
    }

    #[test]
    fn codex_events_become_commands_edits_and_the_explanation() {
        let mut t = Transcript::default();
        for line in [r#"{"type":"thread.started","thread_id":"t"}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc 'ls ~/.local/bin'","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"/home/dev/.profile","kind":"update"}],"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"Pus ~/.local/bin no PATH."}}"#] { t.feed(Agent::Codex, line); }
        assert_eq!(t.commands, vec!["bash -lc 'ls ~/.local/bin'"]);
        assert_eq!(t.edits, vec!["/home/dev/.profile"]);
        assert_eq!(t.explanation.as_deref(), Some("Pus ~/.local/bin no PATH."));
    }

    #[test]
    fn recheck_only_checks_until_the_phone_password_is_saved() {
        let options = run::Options { agents: vec!["claude".into()], outside: false, fix_mouse_wheel: false };
        let dest = Path::new("/home/dev/hangar");
        assert_eq!(recheck_command(dest, &options, true, false), "bash '/home/dev/hangar/install.sh' --app --agentes=claude --tailscale=nao --sem-nativo");
        assert!(recheck_command(dest, &options, false, false).ends_with(" --check"));
        let windows = recheck_command(Path::new(r"C:\Users\dev\hangar"), &options, true, true);
        assert!(windows.starts_with("powershell -NoProfile -ExecutionPolicy Bypass -File \""), "{windows}");
        assert!(windows.ends_with("-App -Agentes claude -Tailscale nao -SemNativo"), "{windows}");
    }

    #[test]
    fn prompt_carries_the_report_the_folder_and_the_rules() {
        let text = prompt("relatório limpo", Path::new("/home/dev/hangar"), "bash x --app", false);
        for needle in ["relatório limpo", "/home/dev/hangar", "bash x --app", "Brazilian Portuguese", "Never run sudo"] {
            assert!(text.contains(needle), "{needle}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn spawn_feeds_the_prompt_streams_lines_and_never_passes_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hangar-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.sh");
        std::fs::write(&fake, "#!/bin/sh\nread -r first\necho \"prompt: $first\"\necho \"token: ${HANGAR_TOKEN:-ausente}\"\necho erro >&2\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = agent_env([("HANGAR_TOKEN".to_owned(), "t0ken".to_owned())], "/usr/bin:/bin");
        let (_, output) = spawn(Launch { program: fake, args: vec![], work: dir.join("work"), env, prompt: "conserte\n".into() }).unwrap();
        let mut lines = Vec::new();
        while let Ok(item) = output.recv_blocking() {
            match item { Output::Line(line) => lines.push(line), Output::Exit => break }
        }
        lines.sort();
        assert_eq!(lines, vec!["erro", "prompt: conserte", "token: ausente"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
