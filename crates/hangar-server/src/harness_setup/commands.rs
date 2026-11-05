//! Comandos conferidos compartilhados pelo painel e pelo instalador local.
use crate::{accounts::environment::AccountEnvironment, cli_probe};
use serde_json::{Value, json};
pub const NAMES: [&str; 4] = ["codex", "pi", "omp", "kimi"];
pub fn command(cli: &str, windows: bool) -> Option<Vec<String>> {
    let args: Vec<&str> = match cli {
        "codex" => vec!["npm", "install", "-g", "@openai/codex"],
        "pi" => vec![
            "npm",
            "install",
            "-g",
            "--ignore-scripts",
            "@earendil-works/pi-coding-agent",
        ],
        "omp" if windows => vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://omp.sh/install.ps1 | iex",
        ],
        "omp" => vec![
            "bash",
            "-o",
            "pipefail",
            "-c",
            "curl -fsSL https://omp.sh/install | sh",
        ],
        "kimi" if !windows => vec![
            "bash",
            "-o",
            "pipefail",
            "-c",
            "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash",
        ],
        _ => return None,
    };
    Some(args.into_iter().map(str::to_owned).collect())
}
pub fn display(args: &[String]) -> String {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "bash" | "powershell"))
    {
        args.last().cloned().unwrap_or_default()
    } else {
        args.join(" ")
    }
}
pub fn resolve(mut args: Vec<String>, env: &AccountEnvironment) -> Result<Vec<String>, String> {
    let program = args.first().ok_or("comando ausente")?;
    let resolved = cli_probe::resolve_program(program, env)
        .ok_or_else(|| format!("{program} não está nesta máquina"))?;
    args[0] = resolved.to_string_lossy().into_owned();
    Ok(args)
}
pub fn public(windows: bool) -> Value {
    Value::Object(
        NAMES
            .into_iter()
            .filter_map(|name| {
                command(name, windows).map(|args| (name.into(), json!(display(&args))))
            })
            .collect(),
    )
}
pub fn manual() -> Value {
    json!({"codex":"https://github.com/openai/codex","pi":"https://pi.dev/docs/latest","omp":"https://github.com/can1357/oh-my-pi","kimi":"https://kimi.com/code"})
}
