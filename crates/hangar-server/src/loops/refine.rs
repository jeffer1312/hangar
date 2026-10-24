//! Refinar o objetivo do loop por um `claude -p` efêmero (sonnet), sem ferramenta com efeito: o
//! texto vem do usuário e roda com o perfil da máquina. A conta é a de mais folga; bateu o limite,
//! relê as cotas e tenta UMA vez a próxima.
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(60);
pub const GOAL_MAX: usize = 2000;
const LIMIT_EXIT0_MAX: usize = 300;
/// Variádico no CLI: o prompt vai pelo stdin, nunca depois destes no argv.
pub const DISALLOWED: [&str; 6] = ["Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch"];
const REFUSALS: [&str; 7] = ["não consigo", "nao consigo", "não posso", "nao posso", "desculpe", "não é possível", "nao e possivel"];

const SYSTEM: &str = "Você reescreve OBJETIVOS pra um loop autônomo de código. Boas práticas:\n\
- Objetivo é UMA coisa só e VERIFICÁVEL: descreve o RESULTADO final, não os passos.\n\
- Pequeno e concreto (ex: \"migre utils/date.ts pra date-fns mantendo o check verde\"), nunca amplo (\"refatore o projeto\").\n\
- Peça EVIDÊNCIA, não promessa: inclua \"rode o comando de verificação e mostre a saída\".\n\
- Deixe explícito: não editar arquivos de teste nem o comando de verificação; corrigir o código.\n\
A saída é SEMPRE um objetivo reescrito — NUNCA uma pergunta, recusa ou comentário. Se faltar contexto, PRESERVE a intenção e marque a lacuna inline entre <colchetes> (ex: \"Melhore o haiku em <arquivo> até <critério de qualidade>, rode o check e mostre a saída\").\n\
Responda SÓ com o objetivo reescrito em pt-BR, texto puro, sem preâmbulo, sem aspas, sem markdown.";

static LIMIT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)hit your .*limit|usage limit|rate limit").unwrap());
/// Com exit 0 só é limite se a saída inteira FOR o aviso: resposta que cita "limit" fica.
static LIMIT_EXIT0: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)^\s*(?:you'?ve hit your \w+ limit|claude ai usage limit reached|usage limit reached)").unwrap());

pub fn prompt(goal: &str, check_cmd: Option<&str>) -> String {
    let mut parts = vec![SYSTEM.to_owned(), String::new(), "Objetivo do usuário:".into(), goal.to_owned()];
    if let Some(cmd) = check_cmd.filter(|c| !c.is_empty()) {
        parts.push(String::new());
        parts.push(format!("Comando de verificação (check): {cmd}"));
    }
    parts.join("\n")
}

#[derive(Clone, Debug)]
pub struct Output { pub code: i64, pub stdout: String, pub stderr: String }

pub fn limited(o: &Output) -> bool {
    if o.code != 0 { return LIMIT.is_match(&format!("{}\n{}", o.stdout, o.stderr)); }
    let out = o.stdout.trim_matches(super::check::py_space);
    out.chars().count() <= LIMIT_EXIT0_MAX && LIMIT_EXIT0.is_match(out)
}

fn last_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

/// Sem stderr (caso do limite), a causa está no stdout.
fn reason(o: &Output) -> String {
    let err = o.stderr.trim_matches(super::check::py_space);
    last_chars(if err.is_empty() { o.stdout.trim_matches(super::check::py_space) } else { err }, 500)
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
    let text = out.stdout.trim_matches(super::check::py_space);
    if text.is_empty() { return Err("claude -p devolveu saída vazia".into()); }
    Ok(text.to_owned())
}

/// Pergunta ou recusa no lugar do objetivo é falha: o objetivo estava vago.
pub async fn refine(goal: &str, check_cmd: Option<&str>, runner: &dyn Runner) -> Result<String, String> {
    let out = claude_p(&prompt(goal, check_cmd), runner).await?;
    let lower = out.to_lowercase();
    if out.ends_with('?') || REFUSALS.iter().any(|p| lower.starts_with(p)) {
        return Err("refine falhou, tente detalhar o objetivo".into());
    }
    Ok(out.chars().take(GOAL_MAX).collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake { first: Vec<Value>, fresh: Option<Vec<Value>>, answers: Mutex<Vec<Output>>, calls: Mutex<Vec<Option<String>>> }

    impl Runner for Fake {
        fn run<'a>(&'a self, _prompt: &'a str, account: Option<&'a str>) -> BoxFuture<'a, Result<Output, String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(account.map(str::to_owned));
                Ok(self.answers.lock().unwrap().remove(0))
            })
        }
        fn quotas(&self, fresh: bool) -> BoxFuture<'_, Result<Vec<Value>, String>> {
            Box::pin(async move { if fresh { self.fresh.clone().ok_or_else(|| "sem".into()) } else { Ok(self.first.clone()) } })
        }
    }

    fn rows(v: &Value) -> Vec<Value> { v.as_array().cloned().unwrap_or_default() }

    #[test]
    fn prompt_order_and_limit_match_the_python() {
        let g = super::super::tests::golden();
        assert_eq!(prompt("g", Some("make test")), g["prompt"].as_str().unwrap());
        for c in g["orders"].as_array().unwrap() {
            let got: Vec<Value> = order(&rows(&c["rows"])).into_iter().map(|a| a.map_or(Value::Null, Value::String)).collect();
            assert_eq!(Value::Array(got), c["order"], "{c}");
        }
        for c in g["limits"].as_array().unwrap() {
            let o = Output { code: c["exit"].as_i64().unwrap(), stdout: c["stdout"].as_str().unwrap().into(), stderr: c["stderr"].as_str().unwrap().into() };
            assert_eq!(limited(&o), c["limited"].as_bool().unwrap(), "{c}");
        }
    }

    #[tokio::test]
    async fn refine_flows_match_the_python() {
        for c in super::super::tests::golden()["flows"].as_array().unwrap() {
            let answers = c["answers"].as_array().unwrap().iter().map(|a| Output {
                code: a[0].as_i64().unwrap(), stdout: a[1].as_str().unwrap().into(), stderr: a[2].as_str().unwrap().into() }).collect();
            let fake = Fake { first: rows(&c["lidas"]), fresh: c["relidas"].as_array().cloned(), answers: Mutex::new(answers), calls: Mutex::new(Vec::new()) };
            let got = refine("g", Some("make test"), &fake).await;
            match c.get("goal") {
                Some(goal) => assert_eq!(got, Ok(goal.as_str().unwrap().to_owned()), "{}", c["label"]),
                None => assert_eq!(got, Err(c["error"].as_str().unwrap().to_owned()), "{}", c["label"]),
            }
            let calls: Vec<Value> = fake.calls.lock().unwrap().iter().map(|a| a.clone().map_or(Value::Null, Value::String)).collect();
            assert_eq!(Value::Array(calls), c["calls"], "{}", c["label"]);
        }
    }
}
