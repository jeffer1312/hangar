//! `claude -p` efêmero (sonnet), sem ferramenta com efeito: o texto vem do usuário e roda com o
//! perfil da máquina. A conta é a de mais folga; bateu o limite, relê as cotas e tenta UMA vez a
//! próxima. Usado pelo refino do loop e pelo Perguntar.
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(60);
const LIMIT_EXIT0_MAX: usize = 300;
/// Variádico no CLI: o prompt vai pelo stdin, nunca depois destes no argv.
pub const DISALLOWED: [&str; 6] = ["Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch"];

static LIMIT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)hit your .*limit|usage limit|rate limit").unwrap());
/// Com exit 0 só é limite se a saída inteira FOR o aviso: resposta que cita "limit" fica.
static LIMIT_EXIT0: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)^\s*(?:you'?ve hit your \w+ limit|claude ai usage limit reached|usage limit reached)").unwrap());

#[derive(Clone, Debug)]
pub struct Output { pub code: i64, pub stdout: String, pub stderr: String }

pub fn limited(o: &Output) -> bool {
    if o.code != 0 { return LIMIT.is_match(&format!("{}\n{}", o.stdout, o.stderr)); }
    let out = o.stdout.trim_matches(crate::loops::check::py_space);
    out.chars().count() <= LIMIT_EXIT0_MAX && LIMIT_EXIT0.is_match(out)
}

fn last_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

/// Sem stderr (caso do limite), a causa está no stdout.
fn reason(o: &Output) -> String {
    let err = o.stderr.trim_matches(crate::loops::check::py_space);
    last_chars(if err.is_empty() { o.stdout.trim_matches(crate::loops::check::py_space) } else { err }, 500)
}

fn exhausted(row: &Value) -> bool {
    row["janelas"].as_array().is_some_and(|w| w.iter().any(|j| j["por_modelo"] != true && j["pct"].as_f64().is_some_and(|p| p >= 100.0)))
}

/// Pastas de config na ordem de tentativa; `None` é a conta padrão (o ambiente do servidor). A
/// padrão vai primeiro salvo leitura dela esgotada; as outras só com leitura boa e folga, pela
/// menor semana gasta.
pub fn order(rows: &[Value]) -> Vec<Option<String>> {
    let claude: Vec<&Value> = rows.iter().filter(|r| r["provedor"] == "claude").collect();
    let default_exhausted = claude.iter().any(|r| r["ativa"] == true && exhausted(r));
    let mut others: Vec<&Value> = claude.into_iter().filter(|r| r["ativa"] != true && r["estado"] == "lida"
        && r["janelas"].as_array().is_some_and(|w| !w.is_empty()) && !exhausted(r)).collect();
    let week = |r: &Value| r["janelas"].as_array().into_iter().flatten().filter(|j| j["rotulo"] == "7d")
        .filter_map(|j| j["pct"].as_f64()).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))).unwrap_or(0.0);
    others.sort_by(|a, b| week(a).total_cmp(&week(b)));
    let mut out: Vec<Option<String>> = if default_exhausted { Vec::new() } else { vec![None] };
    out.extend(others.iter().map(|r| {
        let id = r["id"].as_str().unwrap_or("");
        Some(id.strip_prefix("claude:").unwrap_or(id).to_owned())
    }));
    if out.is_empty() { out.push(None); }
    out
}

fn account_name(account: &Option<String>) -> &str { account.as_deref().unwrap_or("padrão") }

/// O CLI e as cotas; o teste troca os dois.
pub trait Runner: Send + Sync {
    fn run<'a>(&'a self, prompt: &'a str, account: Option<&'a str>) -> BoxFuture<'a, Result<Output, String>>;
    /// Linhas de cota Claude; `fresh` relê em vez de usar o cache.
    fn quotas(&self, fresh: bool) -> BoxFuture<'_, Result<Vec<Value>, String>>;
}

/// O stdout aparado, ou a frase da falha (vira 502).
pub async fn claude_p(prompt: &str, runner: &dyn Runner) -> Result<String, String> {
    let rows = runner.quotas(false).await.unwrap_or_else(|e| {
        tracing::warn!(code = "loop_refine_quotas_failed", "claude -p: cotas ilegíveis ({e}); vai na conta padrão");
        Vec::new()
    });
    let mut account = order(&rows).swap_remove(0);
    let mut out = runner.run(prompt, account.as_deref()).await?;
    if limited(&out) {
        let first = format!("claude -p: conta {} bateu o limite: {}", account_name(&account), reason(&out));
        tracing::warn!(code = "loop_refine_limit", "{first}");
        // A leitura em cache pode ser velha: relê antes de escolher a próxima.
        let rows = runner.quotas(true).await.map_err(|_| first.clone())?;
        let Some(next) = order(&rows).into_iter().find(|a| *a != account) else { return Err(first) };
        account = next;
        out = runner.run(prompt, account.as_deref()).await?;
        if out.code != 0 || limited(&out) {
            return Err(format!("{first}; segunda tentativa na conta {} falhou (exit {}): {}", account_name(&account), out.code, reason(&out)));
        }
    }
    if out.code != 0 { return Err(format!("claude -p falhou (exit {}): {}", out.code, reason(&out))); }
    tracing::info!(conta = account_name(&account), "claude -p atendido");
    let text = out.stdout.trim_matches(crate::loops::check::py_space);
    if text.is_empty() { return Err("claude -p devolveu saída vazia".into()); }
    Ok(text.to_owned())
}

/// O `claude` do PATH do servidor, sem a autoridade dele no ambiente.
pub struct Cli { pub accounts: crate::accounts::AccountService, pub bridge: Option<crate::accounts::bridge::AccountsBridge> }

impl Runner for Cli {
    fn run<'a>(&'a self, prompt: &'a str, account: Option<&'a str>) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Some(mut command) = self.accounts.claude_command() else { return Err("claude CLI não encontrado".into()) };
            let failed = || "claude -p excedeu o tempo ou falhou ao iniciar".to_owned();
            command.args(["-p", "--model", "sonnet", "--no-session-persistence", "--disallowedTools"]).args(DISALLOWED)
                .current_dir(std::env::temp_dir()).stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
            for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
            if let Some(dir) = account { command.env("CLAUDE_CONFIG_DIR", dir); }
            let mut tree = crate::terminal_process::CommandTree::configure(&mut command).map_err(|_| failed())?;
            let mut child = command.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "claude CLI não encontrado".into() } else { failed() })?;
            if tree.attach(&child).is_err() {
                crate::terminal_process::finish(&mut tree).await;
                let _ = child.wait().await;
                return Err(failed());
            }
            let (mut stdin, mut out, mut err) = (child.stdin.take().unwrap(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
            let done = tokio::time::timeout(TIMEOUT, async {
                let write = async { let r = stdin.write_all(prompt.as_bytes()).await; drop(stdin); r };
                let (mut o, mut e) = (Vec::new(), Vec::new());
                let (w, ro, re) = tokio::join!(write, out.read_to_end(&mut o), err.read_to_end(&mut e));
                // Pipe fechado cedo pelo CLI não é falha: o código de saída diz.
                let _ = w;
                let status = child.wait().await;
                (ro.and(re).and(status), o, e)
            }).await;
            crate::terminal_process::finish(&mut tree).await;
            match done {
                Ok((Ok(status), o, e)) => Ok(Output {
                    code: status.code().map_or(-1, i64::from),
                    stdout: String::from_utf8_lossy(&o).replace("\r\n", "\n").replace('\r', "\n"),
                    stderr: String::from_utf8_lossy(&e).replace("\r\n", "\n").replace('\r', "\n"),
                }),
                _ => { let _ = child.wait().await; Err(failed()) }
            }
        })
    }

    fn quotas(&self, fresh: bool) -> BoxFuture<'_, Result<Vec<Value>, String>> {
        Box::pin(async move {
            let rows = self.accounts.quotas(self.bridge.clone(), false, !fresh).await.map_err(|e| e.code.to_owned())?;
            Ok(rows.as_array().cloned().unwrap_or_default())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn cli(path: &Path) -> Cli {
        let text = path.to_string_lossy().into_owned();
        let env = crate::accounts::environment::AccountEnvironment::from_map(BTreeMap::from([
            ("PATH".to_owned(), text.clone()), ("HOME".to_owned(), text)]));
        Cli { accounts: crate::accounts::AccountService::new(env), bridge: None }
    }

    #[tokio::test]
    async fn cli_runs_the_claude_of_the_path_with_fixed_argv_and_the_prompt_on_stdin() {
        // ASK-18..23 e ASK-25: argv da captura (`loop.py:293-296`), prompt só no stdin, cwd na pasta
        // temporária, CLAUDE_CONFIG_DIR só fora da conta padrão, saída lida como UTF-8.
        let dir = tempfile::tempdir().unwrap();
        let o = dir.path().display();
        let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{o}/argv'\npwd -P > '{o}/cwd'\ncat > '{o}/stdin'\n\
            printf '%s' \"${{CLAUDE_CONFIG_DIR-unset}}\" > '{o}/env'\nprintf 'ol\\303\\241\\r\\n'\nprintf 'aviso' >&2\nexit 3\n");
        let source = dir.path().join("claude.conteudo");
        std::fs::write(&source, script).unwrap();
        // `install` grava e fecha noutro processo: sem ETXTBSY com os outros testes criando processos.
        assert!(std::process::Command::new("install").arg("-m").arg("755").arg(&source).arg(dir.path().join("claude")).status().unwrap().success());
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        let cli = cli(dir.path());
        let got = cli.run("pergunta\nlinha 2", None).await.unwrap();
        assert_eq!((got.code, got.stdout.as_str(), got.stderr.as_str()), (3, "olá\n", "aviso"));
        assert_eq!(read("argv").lines().collect::<Vec<_>>(), ["-p", "--model", "sonnet", "--no-session-persistence", "--disallowedTools",
            "Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch"]);
        assert_eq!(read("stdin"), "pergunta\nlinha 2");
        assert_eq!(Path::new(read("cwd").trim_end()), std::env::temp_dir().canonicalize().unwrap());
        // Conta padrão: o ambiente do servidor como está (`env=None` no Python).
        assert_eq!(read("env"), std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "unset".into()));
        cli.run("p", Some("/conta/b")).await.unwrap();
        assert_eq!(read("env"), "/conta/b");
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(self::cli(empty.path()).run("p", None).await.unwrap_err(), "claude CLI não encontrado");
    }
}
