//! Os argumentos do lançador, os mesmos que o servidor monta (`session_open::codex::launcher_argv`),
//! com as recusas e o texto de uso que o pane mostrava.

/// O uso que acompanha toda recusa de argumento.
pub(crate) const USAGE: &str = "usage: hangar-codex-tui [-h] [--name NAME] [--cwd CWD] [--prompt PROMPT]
                        [--resume RESUME] [--model MODEL] [--effort EFFORT]
                        [--approval-policy {never,on-request}]
                        [--sandbox {read-only,workspace-write,danger-full-access}]
                        [--tool-output-token-limit TOOL_OUTPUT_TOKEN_LIMIT]
";

pub(crate) const HELP: &str = concat!("usage: hangar-codex-tui [-h] [--name NAME] [--cwd CWD] [--prompt PROMPT]
                        [--resume RESUME] [--model MODEL] [--effort EFFORT]
                        [--approval-policy {never,on-request}]
                        [--sandbox {read-only,workspace-write,danger-full-access}]
                        [--tool-output-token-limit TOOL_OUTPUT_TOKEN_LIMIT]

options:
  -h, --help            show this help message and exit
  --name NAME
  --cwd CWD
  --prompt PROMPT
  --resume RESUME       retoma a conversa deste id em vez de abrir uma nova
  --model MODEL         modelo do Codex (vira `-m` na TUI)
  --effort EFFORT       nivel de raciocinio; vira sobrescrita de config, nao flag
  --approval-policy {never,on-request}
  --sandbox {read-only,workspace-write,danger-full-access}
  --tool-output-token-limit TOOL_OUTPUT_TOKEN_LIMIT
");

const APPROVALS: [&str; 2] = ["never", "on-request"];
const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];
const TIERS: [&str; 2] = ["default", "priority"];

/// O que o pane recebeu, sem interpretar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Args {
    pub name: String,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub resume: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub codex_home: Option<String>,
    pub codex_account: Option<String>,
    pub approval: Option<&'static str>,
    pub sandbox: Option<&'static str>,
    pub limit: Option<u64>,
    pub config: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Help,
    Run(Args),
}

/// Recusa antes de qualquer efeito; sai com 2.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Erro de uso: o uso e a frase, como o `argparse` mostrava.
    Usage(String),
    /// Sem nome da sessão: só a frase.
    NoName,
}

impl Refusal {
    pub fn render(&self) -> String {
        match self {
            Self::Usage(message) => format!("{USAGE}hangar-codex-tui: error: {message}\n"),
            Self::NoName => "hangar-codex-tui: --name (ou CP_SESSION_NAME) e obrigatorio\n".into(),
        }
    }
}

fn choice(flag: &str, value: &str, allowed: &[&'static str]) -> Result<&'static str, Refusal> {
    allowed.iter().copied().find(|item| *item == value).ok_or_else(|| {
        let list = allowed.iter().map(|item| format!("'{item}'")).collect::<Vec<_>>().join(", ");
        Refusal::Usage(format!("argument {flag}: invalid choice: '{value}' (choose from {list})"))
    })
}

/// `raw` sem o subcomando; `env_name` é o `CP_SESSION_NAME` que o tmux carimbou no pane.
pub(crate) fn parse(raw: &[String], env_name: Option<&str>) -> Result<Parsed, Refusal> {
    let mut args = Args::default();
    let mut name: Option<String> = env_name.map(str::to_owned);
    let mut limit: Option<i64> = None;
    let mut unknown: Vec<&str> = Vec::new();
    let mut at = 0;
    while at < raw.len() {
        let token = raw[at].as_str();
        at += 1;
        if token == "-h" || token == "--help" {
            return Ok(Parsed::Help);
        }
        let (flag, inline) = match token.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (token, None),
        };
        const FLAGS: [&str; 13] = ["--name", "--cwd", "--prompt", "--resume", "--model", "--effort", "--service-tier", "--codex-home",
            "--codex-account", "--approval-policy", "--sandbox", "--tool-output-token-limit", "--config"];
        if !FLAGS.contains(&flag) {
            unknown.push(token);
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None if at < raw.len() => {
                at += 1;
                raw[at - 1].clone()
            }
            None => return Err(Refusal::Usage(format!("argument {flag}: expected one argument"))),
        };
        match flag {
            "--name" => name = Some(value),
            "--cwd" => args.cwd = Some(value),
            "--prompt" => args.prompt = Some(value),
            "--resume" => args.resume = Some(value),
            "--model" => args.model = Some(value),
            "--effort" => args.effort = Some(value),
            "--service-tier" => args.service_tier = Some(choice(flag, &value, &TIERS)?.to_owned()),
            "--codex-home" => args.codex_home = Some(value),
            "--codex-account" => args.codex_account = Some(value),
            "--approval-policy" => args.approval = Some(choice(flag, &value, &APPROVALS)?),
            "--sandbox" => args.sandbox = Some(choice(flag, &value, &SANDBOXES)?),
            "--tool-output-token-limit" => {
                let number = value.trim().parse::<i64>()
                    .map_err(|_| Refusal::Usage(format!("argument {flag}: invalid int value: '{value}'")))?;
                limit = Some(number);
            }
            _ => args.config.push(value),
        }
    }
    if !unknown.is_empty() {
        return Err(Refusal::Usage(format!("unrecognized arguments: {}", unknown.join(" "))));
    }
    match limit {
        Some(number) if number <= 0 => return Err(Refusal::Usage("--tool-output-token-limit precisa ser positivo".into())),
        Some(number) => args.limit = Some(number as u64),
        None => {}
    }
    // Retomar sem id é abrir conversa nova, como o `if args.resume` da TUI.
    if args.resume.as_deref() == Some("") {
        args.resume = None;
    }
    args.name = name.filter(|name| !name.is_empty()).ok_or(Refusal::NoName)?;
    Ok(Parsed::Run(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(raw: &[&str], name: Option<&str>) -> Result<Parsed, Refusal> {
        parse(&raw.iter().map(|s| s.to_string()).collect::<Vec<_>>(), name)
    }

    /// A ordem e os nomes que o servidor manda (`launcher_argv`) chegam inteiros.
    #[test]
    fn the_server_argv_is_read_whole() {
        let Ok(Parsed::Run(args)) = run(&["--cwd", "/w", "--service-tier", "priority", "--codex-home", "/h/.codex", "--codex-account", "b",
            "--resume", "t-1", "--model", "gpt-6", "--effort", "high", "--approval-policy", "on-request", "--sandbox", "read-only",
            "--tool-output-token-limit", "9000", "--prompt", "começa", "--config", "a=1", "--config=b=2"], Some("cx")) else { panic!() };
        assert_eq!(args, Args { name: "cx".into(), cwd: Some("/w".into()), prompt: Some("começa".into()), resume: Some("t-1".into()),
            model: Some("gpt-6".into()), effort: Some("high".into()), service_tier: Some("priority".into()), codex_home: Some("/h/.codex".into()),
            codex_account: Some("b".into()), approval: Some("on-request"), sandbox: Some("read-only"), limit: Some(9000),
            config: vec!["a=1".into(), "b=2".into()] });
        // `--name` vence o do pane; o prompt pode começar com hífen.
        let Ok(Parsed::Run(args)) = run(&["--name", "outra", "--prompt", "-x"], Some("cx")) else { panic!() };
        assert_eq!((args.name.as_str(), args.prompt.as_deref()), ("outra", Some("-x")));
        assert_eq!(run(&["--help"], None), Ok(Parsed::Help));
    }

    #[test]
    fn a_missing_value_or_a_negative_limit_is_a_usage_error() {
        assert_eq!(run(&["--cwd"], Some("cx")), Err(Refusal::Usage("argument --cwd: expected one argument".into())));
        assert_eq!(run(&["--tool-output-token-limit", "-3"], Some("cx")), Err(Refusal::Usage("--tool-output-token-limit precisa ser positivo".into())));
        // O limite vem antes do nome, como no Python.
        assert_eq!(run(&["--tool-output-token-limit", "0"], None), Err(Refusal::Usage("--tool-output-token-limit precisa ser positivo".into())));
        assert_eq!(run(&["--resume", ""], Some("cx")).map(|p| matches!(p, Parsed::Run(Args { resume: None, .. }))), Ok(true));
    }

    proptest::proptest! {
        /// Qualquer argv chega a um desfecho: nunca pânico, e toda recusa leva o uso ou a frase do nome.
        #[test]
        fn any_argv_parses_or_refuses_with_the_usage(raw in proptest::collection::vec("[-a-z=0-9 ]{0,12}", 0..8), name in proptest::option::of("[a-z]{0,4}")) {
            match parse(&raw, name.as_deref()) {
                Ok(_) => {}
                Err(refusal) => {
                    let text = refusal.render();
                    proptest::prop_assert!(text.starts_with("usage: hangar-codex-tui") || text == Refusal::NoName.render(), "{text}");
                }
            }
        }
    }
}
