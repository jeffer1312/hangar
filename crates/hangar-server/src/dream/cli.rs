//! O modelo do sonho pelo `claude -p`: só ferramentas de arquivo, presas à pasta de trabalho, sem
//! nada da conta (CLAUDE.md, hooks, MCP) no contexto. Flags e respostas medidas no Claude Code 2.1.323.

use std::sync::LazyLock;
use std::time::Duration;

use hangar_dream::run::{Auth, BoxFut, Call, Model, ModelError, Reply};
use serde_json::Value;

pub const CALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);

static NO_CREDIT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)credit balance (?:is )?too low").unwrap());

pub struct ClaudeModel {
    pub accounts: crate::accounts::AccountService,
    pub api_key: String,
}

impl Model for ClaudeModel {
    fn call<'a>(&'a self, call: Call<'a>) -> BoxFut<'a, Result<Reply, ModelError>> {
        Box::pin(async move {
            let out = match self.run(&call).await {
                Ok(out) => out,
                Err(Failure::Timeout) => return Err(ModelError::Timeout { cost_usd: call.budget_usd }),
                Err(Failure::Start(e)) => return Err(ModelError::Other(e)),
            };
            // A chave só vai no ambiente; se o CLI a ecoar num erro, ela não sai daqui.
            classify(&out).map_err(|e| match e {
                ModelError::Other(text) if !self.api_key.is_empty() => ModelError::Other(text.replace(&self.api_key, "[chave]")),
                other => other,
            })
        })
    }
}

enum Failure {
    Start(String),
    Timeout,
}

impl ClaudeModel {
    async fn run(&self, call: &Call<'_>) -> Result<crate::claude_p::Output, Failure> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Some(mut command) = self.accounts.claude_command() else { return Err(Failure::Start("claude CLI não encontrado".into())) };
        command
            .args(["-p", "--model", call.model, "--output-format", "json", "--max-budget-usd"])
            .arg(format!("{:.4}", call.budget_usd))
            .args(["--safe-mode", "--setting-sources", "", "--strict-mcp-config", "--mcp-config", r#"{"mcpServers":{}}"#])
            .args(["--no-session-persistence", "--tools", "Read,Write,Edit,Glob,Grep", "--permission-mode", "acceptEdits"])
            .current_dir(call.workdir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Nada da autoridade do servidor nem de outra conta: só o que esta chamada escolheu.
        for key in crate::terminal_process::PRIVATE_ENV_KEYS {
            command.env_remove(key);
        }
        for (key, _) in std::env::vars_os() {
            let k = key.to_string_lossy();
            if k.starts_with("ANTHROPIC_") || k.starts_with("CLAUDE_") {
                command.env_remove(&key);
            }
        }
        let _empty_config;
        match &call.auth {
            Auth::Key => {
                // Pasta de conta vazia: a chamada nunca cai no login de uma conta.
                _empty_config = tempfile::Builder::new().prefix("hangar-dream-cfg-").tempdir().map_err(|e| Failure::Start(e.to_string()))?;
                command.env("ANTHROPIC_API_KEY", &self.api_key).env("CLAUDE_CONFIG_DIR", _empty_config.path());
            }
            Auth::Account(dir) => {
                command.env("CLAUDE_CONFIG_DIR", dir);
            }
        }
        let failed = |e: std::io::Error| Failure::Start(format!("claude -p falhou ao iniciar: {}", e.kind()));
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command).map_err(failed)?;
        let mut child = command.spawn().map_err(failed)?;
        if let Err(e) = tree.attach(&child) {
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return Err(failed(e));
        }
        let (mut stdin, mut out, mut err) = (child.stdin.take().unwrap(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let done = tokio::time::timeout(CALL_TIMEOUT, async {
            let write = async {
                let r = stdin.write_all(call.prompt.as_bytes()).await;
                drop(stdin);
                r
            };
            let (mut o, mut e) = (Vec::new(), Vec::new());
            let (_, ro, re) = tokio::join!(write, out.read_to_end(&mut o), err.read_to_end(&mut e));
            let status = child.wait().await;
            (ro.and(re).and(status), o, e)
        })
        .await;
        crate::terminal_process::finish(&mut tree).await;
        match done {
            Ok((Ok(status), o, e)) => Ok(crate::claude_p::Output {
                code: status.code().map_or(-1, i64::from),
                stdout: String::from_utf8_lossy(&o).into_owned(),
                stderr: String::from_utf8_lossy(&e).into_owned(),
            }),
            Ok((Err(e), _, _)) => {
                let _ = child.wait().await;
                Err(Failure::Start(format!("claude -p falhou ao ler a saída: {}", e.kind())))
            }
            Err(_) => {
                let _ = child.wait().await;
                Err(Failure::Timeout)
            }
        }
    }
}

/// Lê o JSON final do `claude -p --output-format json`.
fn classify(out: &crate::claude_p::Output) -> Result<Reply, ModelError> {
    let result: Option<Value> = out.stdout.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().filter(|v| v["type"] == "result"));
    let cost = result.as_ref().and_then(|r| r["total_cost_usd"].as_f64()).unwrap_or(0.0);
    let text = result.as_ref().and_then(|r| r["result"].as_str()).unwrap_or("").to_string();
    let errors = result.as_ref().map(|r| r["errors"].to_string()).unwrap_or_default();
    let all = format!("{text}\n{errors}\n{}", out.stderr);
    // Sucesso primeiro: uma resposta boa pode citar "credit balance too low" de uma transcrição.
    match result.as_ref() {
        Some(r) if r["is_error"] == false && out.code == 0 => Ok(Reply { cost_usd: cost, text }),
        Some(r) if r["subtype"] == "error_max_budget_usd" => Err(ModelError::Budget { cost_usd: cost }),
        _ if NO_CREDIT.is_match(&all) => Err(ModelError::NoCredit),
        _ if crate::claude_p::limited(out) => Err(ModelError::Limited),
        // Sem a linha de resultado não há custo nem motivo confiável; só o código sai daqui.
        None => Err(ModelError::Other("dream_unparsed_output".into())),
        _ => Err(ModelError::Other(last_line(&all))),
    }
}

fn last_line(text: &str) -> String {
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("sem saída");
    line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i64, stdout: &str) -> crate::claude_p::Output {
        crate::claude_p::Output { code, stdout: stdout.into(), stderr: String::new() }
    }

    #[test]
    fn reads_the_measured_answers() {
        let ok = out(0, r#"{"type":"result","subtype":"success","is_error":false,"result":"feito","total_cost_usd":0.00286507}"#);
        let reply = classify(&ok).unwrap();
        assert_eq!(reply.text, "feito");
        assert!((reply.cost_usd - 0.00286507).abs() < 1e-12);
        let budget = out(1, r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"terminal_reason":"budget_exhausted","total_cost_usd":0.00024536}"#);
        assert_eq!(classify(&budget).err(), Some(ModelError::Budget { cost_usd: 0.00024536 }));
        let credit = out(1, r#"{"type":"result","subtype":"success","is_error":true,"result":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.","total_cost_usd":0}"#);
        assert_eq!(classify(&credit).err(), Some(ModelError::NoCredit));
        assert_eq!(classify(&out(1, "lixo")).err(), Some(ModelError::Other("dream_unparsed_output".into())));
        assert_eq!(
            hangar_dream::run::DreamError::Model(ModelError::Other("dream_unparsed_output".into())).to_string(),
            "dream_unparsed_output"
        );
    }
}
