//! Os campos do pedido de abertura (`POST /api/sessions`) numa tabela só. Testes do servidor, do
//! `hangar-send --new` e do MCP `new_session` a conferem: campo novo no pedido que não estiver aqui,
//! ou que o CLI ou o MCP não digam, quebra o teste.

/// Como o `hangar-send --new` diz o campo; os argumentos de exemplo levam o campo ao corpo.
#[derive(Clone, Copy, Debug)]
pub enum Cli {
    /// `--new <nome> [cwd]`.
    Positional,
    /// Os argumentos que põem o campo no corpo.
    Args(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct OpenField {
    /// A chave no corpo do pedido.
    pub body: &'static str,
    /// O nome no `new_session` do MCP.
    pub mcp: &'static str,
    pub cli: Cli,
    /// Omitido, quem decide é o servidor (`resolve_open` com o padrão salvo): o pedido distingue
    /// ausente de explícito, e cliente nenhum preenche o campo com uma preferência guardada nele.
    pub server_default: bool,
}

const fn field(body: &'static str, mcp: &'static str, cli: &'static [&'static str], server_default: bool) -> OpenField {
    OpenField { body, mcp, cli: Cli::Args(cli), server_default }
}

pub const FIELDS: &[OpenField] = &[
    OpenField { body: "name", mcp: "nome", cli: Cli::Positional, server_default: false },
    OpenField { body: "cwd", mcp: "cwd", cli: Cli::Positional, server_default: false },
    field("branch", "branch", &["--branch", "b1"], false),
    field("new_branch", "new_branch", &["--branch", "b1", "--new-branch"], false),
    field("base", "base", &["--branch", "b1", "--new-branch", "--base", "main"], false),
    field("config_dir", "conta", &["--conta", "c1"], true),
    field("provider", "provider", &["--provider", "pi"], true),
    field("remember_provider", "remember_provider", &["--remember-provider"], false),
    field("codex_account", "codex_account", &["--provider", "codex", "--conta", "a1"], true),
    field("initial_prompt", "initial_prompt", &["--prompt", "oi"], false),
    field("engine", "engine", &["--engine", "m1"], false),
    field("engine_account", "engine_account", &["--engine-account", "e1"], false),
    field("model", "model", &["--model", "opus"], true),
    field("effort", "effort", &["--effort", "high"], true),
    field("service_tier", "service_tier", &["--service-tier", "priority"], true),
    field("permission_mode", "permissao", &["--permissao", "manual"], true),
    field("subagent_model", "subagent_model", &["--subagente", "haiku"], true),
    field("code_mode", "code_mode", &["--code-mode"], true),
    field("claude_customizations", "claude_customizations", &["--customizations", "{}"], false),
    field("jev", "jev", &["--jev"], true),
    field("omp_profile", "omp_profile", &["--profile", "p1"], false),
    field("read_only", "read_only", &["--read-only"], false),
    field("headless", "headless", &["--headless"], true),
    field("inherit", "inherit", &["--inherit"], false),
    field("link", "link", &["--link"], false),
];

/// Os campos que uma tarefa agendada guarda para abrir a conversa a cada rodada: sem nome, branch,
/// vínculo nem criadora, que são da abertura de agora.
pub const SCHEDULED_LAUNCH: &[&str] = &[
    "cwd", "provider", "config_dir", "codex_account", "engine", "engine_account", "model", "effort", "service_tier",
    "permission_mode", "subagent_model", "claude_customizations", "jev", "code_mode", "omp_profile", "read_only", "headless",
];

/// Campos do pedido que nem o CLI nem o MCP dizem, com o motivo. Nenhum deles é padrão de abertura.
pub const SERVER_ONLY: &[(&str, &str)] = &[
    ("creator", "quem abre sai da identidade de quem chama (pane, chave), nunca do que ele escreve"),
    ("prewarm", "a reserva pré-aquecida existe só com a tela de criar aberta"),
    ("engine_binding", "confere o vínculo da conta do motor que a tela de criar mostrou; CLI e MCP não mostram vínculo"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scheduled_launch_only_keeps_open_fields() {
        let stray: Vec<&&str> = SCHEDULED_LAUNCH.iter().filter(|name| !FIELDS.iter().any(|f| f.body == **name)).collect();
        assert!(stray.is_empty(), "campos do agendamento fora da abertura: {stray:?}");
    }
}
