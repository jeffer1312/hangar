//! O registro do Hangar nos agentes desta máquina, chamado pelo instalador e pelos passos de
//! atualização: o MCP `hangar` nas contas do Claude, do Codex e do Antigravity (`mcp-register`) e o cabeçalho que o
//! Claude pede a cada conexão (`mcp-headers`), os agentes das skills (`agents-install`) e o bloco
//! Sessões-irmãs do `~/.claude/CLAUDE.md` (`claude-md-block`). Antes eram scripts Python.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

pub mod agents_install;
pub mod claude_md_block;
mod files;
pub mod mcp_headers;
pub mod mcp_register;

pub use files::{env_file, read_env, toml_string};

/// Uma chamada de um dos comandos, com o ambiente lido uma vez.
#[derive(Clone, Debug, Default)]
pub struct Invocation {
    /// `HOME` (ou `USERPROFILE`) de quem chama.
    pub home: PathBuf,
    /// `--home`: a casa onde gravar, quando não é a do processo (testes, como o `--home` antigo).
    pub home_arg: Option<PathBuf>,
    /// `--root`: a raiz da cópia que chamou (`<raiz>/backend/.env` na máquina não migrada, `skills/`).
    pub root: Option<PathBuf>,
    /// O ambiente de quem chama.
    pub vars: BTreeMap<String, String>,
    /// A entrada padrão (o texto do bloco no `claude-md-block`).
    pub stdin: Vec<u8>,
    /// O helper do Claude ganha `.cmd` no Windows.
    pub windows: bool,
}

impl Invocation {
    /// A casa onde os arquivos são gravados.
    pub fn target_home(&self) -> PathBuf {
        self.home_arg.clone().unwrap_or_else(|| self.home.clone())
    }

    /// A raiz da cópia: a passada pelo atalho; sem ela, a da instalação.
    pub fn root(&self) -> Option<PathBuf> {
        self.root.clone().or_else(|| hangar_core::install_root::root(&self.home))
    }
}

/// `--root <dir>` e `--home <dir>`, em qualquer ordem; o resto é erro de uso.
pub fn parse_args(args: &[String]) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let (mut root, mut home) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let slot = match arg.as_str() {
            "--root" => &mut root,
            "--home" => &mut home,
            other => return Err(format!("argumento inesperado '{other}'")),
        };
        let Some(value) = rest.next().filter(|value| !value.is_empty()) else {
            return Err(format!("{arg} pede um caminho"));
        };
        *slot = Some(PathBuf::from(value));
    }
    Ok((root, home))
}

/// A casa do ambiente, pela mesma regra das contas (`AccountEnvironment`): USERPROFILE no Windows.
pub fn home_of(vars: &BTreeMap<String, String>) -> PathBuf {
    hangar_core::account_env::AccountEnvironment::from_map(vars.clone()).home
}

/// O ambiente do processo; `stdin` só para quem o lê.
pub fn from_process(args: &[String], command: &str, read_stdin: bool, err: &mut dyn Write) -> Result<Invocation, u8> {
    let (root, home_arg) = parse_args(args).map_err(|message| {
        let _ = writeln!(err, "uso: hangar-server {command} [--root <raiz>] [--home <casa>] ({message})");
        2
    })?;
    let vars: BTreeMap<String, String> = std::env::vars_os()
        .map(|(key, value)| (key.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()))
        .collect();
    // A casa do `Path.home()`: no Windows o USERPROFILE, nunca o HOME=/c/Users/x do Git Bash.
    let home = home_of(&vars);
    let mut stdin = Vec::new();
    if read_stdin {
        use std::io::Read;
        if let Err(error) = std::io::stdin().read_to_end(&mut stdin) {
            let _ = writeln!(err, "erro: não consegui ler a entrada padrão: {error}");
            return Err(1);
        }
    }
    Ok(Invocation { home, home_arg, root, vars, stdin, windows: cfg!(windows) })
}

/// O comando de terminal: lê o processo, roda e devolve o código.
fn cli(args: &[String], command: &str, read_stdin: bool,
       run: fn(&Invocation, &mut dyn Write, &mut dyn Write) -> u8) -> ExitCode {
    let mut err = std::io::stderr().lock();
    let invocation = match from_process(args, command, read_stdin, &mut err) {
        Ok(invocation) => invocation,
        Err(code) => return ExitCode::from(code),
    };
    let mut out = std::io::stdout().lock();
    let code = run(&invocation, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(code)
}
