//! Comandos conferidos compartilhados pelo painel e pelo instalador local.
use crate::{accounts::environment::AccountEnvironment, cli_probe};
use serde_json::{Value, json};
pub const NAMES: [&str; 6] = ["claude", "codex", "pi", "omp", "kimi", "antigravity"];
/// O programa do id de instalação: o card do Antigravity se chama pelo produto, o binário é `agy`.
pub fn program(id: &str) -> &str {
    match id {
        "antigravity" => "agy",
        other => other,
    }
}
pub fn command(cli: &str, windows: bool) -> Option<Vec<String>> {
    let args: Vec<&str> = match cli {
        "claude" if windows => vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://claude.ai/install.ps1 | iex",
        ],
        "claude" => vec![
            "bash",
            "-o",
            "pipefail",
            "-c",
            "curl -fsSL https://claude.ai/install.sh | bash",
        ],
        "antigravity" if windows => vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://antigravity.google/cli/install.ps1 | iex",
        ],
        "antigravity" => vec![
            "bash",
            "-o",
            "pipefail",
            "-c",
            "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        ],
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
        "kimi" if windows => vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://code.kimi.com/kimi-code/install.ps1 | iex",
        ],
        "kimi" => vec![
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
    json!({"claude":"https://code.claude.com/docs/en/setup","codex":"https://github.com/openai/codex","pi":"https://pi.dev/docs/latest","omp":"https://github.com/can1357/oh-my-pi","kimi":"https://kimi.com/code","antigravity":"https://antigravity.google/docs/getting-started"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cli: &str, windows: bool) -> Vec<String> { command(cli, windows).unwrap() }

    #[test]
    fn the_antigravity_card_installs_and_probes_agy() {
        assert_eq!(program("antigravity"), "agy");
        assert_eq!(program("claude"), "claude");
        assert_eq!(program("codex"), "codex");
        assert!(NAMES.contains(&"antigravity") && NAMES.contains(&"claude"));
        assert_eq!(args("antigravity", false), ["bash", "-o", "pipefail", "-c", "curl -fsSL https://antigravity.google/cli/install.sh | bash"]);
        assert_eq!(args("antigravity", true), ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://antigravity.google/cli/install.ps1 | iex"]);
        assert_eq!(manual()["antigravity"], "https://antigravity.google/docs/getting-started");
    }

    #[test]
    fn claude_installs_by_the_official_script() {
        assert_eq!(args("claude", false), ["bash", "-o", "pipefail", "-c", "curl -fsSL https://claude.ai/install.sh | bash"]);
        assert_eq!(args("claude", true), ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://claude.ai/install.ps1 | iex"]);
        assert_eq!(display(&args("claude", false)), "curl -fsSL https://claude.ai/install.sh | bash");
        assert_eq!(manual()["claude"], "https://code.claude.com/docs/en/setup");
    }

    #[test]
    fn kimi_installs_by_the_official_script_on_both_systems() {
        assert_eq!(args("kimi", false), ["bash", "-o", "pipefail", "-c", "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash"]);
        assert_eq!(args("kimi", true), ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://code.kimi.com/kimi-code/install.ps1 | iex"]);
        assert_eq!(public(true)["kimi"], "irm https://code.kimi.com/kimi-code/install.ps1 | iex");
    }
}
