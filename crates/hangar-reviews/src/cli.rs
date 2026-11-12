//! `gh` e `glab` com prazo, sem pergunta interativa e sem janela no Windows: o login de cada host é o que o CLI já tem.
use crate::Failure;
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Leitura comum; a lista de um host e o detalhe juntam várias chamadas dentro deste prazo cada.
pub const READ: Duration = Duration::from_secs(25);
/// Mesclar pode esperar o servidor do provedor rodar ganchos.
pub const WRITE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cli { Gh, Glab }

impl Cli {
    pub fn program(self) -> &'static str { match self { Cli::Gh => "gh", Cli::Glab => "glab" } }
}

/// O executável no PATH, pelo resolvedor do projeto (o `PATHEXT` do Windows incluído).
pub fn locate(cli: Cli) -> Option<PathBuf> {
    let env: Vec<(String, String)> = std::env::vars().collect();
    hangar_core::process::resolve_program(cli.program(), env.iter().map(|(k, v)| (k, v)))
}

/// Roda o CLI e devolve a saída padrão; código diferente de zero vira falha com a mensagem dele.
pub async fn run(cli: Cli, args: Vec<String>, timeout: Duration) -> Result<String, Failure> {
    let output = run_raw(cli, args, timeout).await?;
    if output.code != 0 {
        let reason = output.reason("");
        return Err(Failure::new("cli_failed", if reason.is_empty() { format!("{} saiu com {}", cli.program(), output.code) } else { reason }));
    }
    Ok(output.stdout)
}

/// Como `run`, mas devolve o código de saída: `auth status` responde pelo código.
pub async fn status(cli: Cli, args: Vec<String>, timeout: Duration) -> Result<bool, Failure> {
    Ok(run_raw(cli, args, timeout).await?.code == 0)
}

async fn run_raw(cli: Cli, args: Vec<String>, timeout: Duration) -> Result<hangar_workspace::process::Output, Failure> {
    let program = locate(cli).ok_or_else(|| Failure::new("cli_missing", format!("{} não está instalado", cli.program())))?;
    let done = tokio::task::spawn_blocking(move || {
        let mut command = command(&program);
        command.args(&args);
        hangar_workspace::process::run_program(&mut command, timeout)
    }).await;
    match done {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(Failure::new("cli_unavailable", e.detail.as_str().map_or_else(|| e.detail.to_string(), str::to_owned))),
        Err(_) => Err(Failure::new("cli_unavailable", "a chamada ao CLI não terminou")),
    }
}

fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("NO_PROMPT", "1")
        .env("GLAB_CHECK_UPDATE", "false")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0");
    command
}
