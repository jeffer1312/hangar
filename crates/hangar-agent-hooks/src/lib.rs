//! Hooks de marcador dos agentes (Claude e Codex) como subcomandos do `hangar-server`: leem o evento
//! JSON no stdin e gravam `<config>/.hangar-{state,active,prompt,preview,askq}` sem servidor, sem
//! interpretador e sem rede (só o Jev do `nav-hook` sai dela). Todo hook sai 0: um hook nosso nunca
//! barra o agente.
#![deny(unsafe_code)]

pub mod askq;
mod input;
pub mod log;
mod marker;
pub mod nav;
pub mod preview;
pub mod state;
pub mod tree;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Os subcomandos deste crate, na forma gravada nos `settings.json`/`hooks.json`.
pub const MODES: &[&str] = &["state-hook", "preview-hook", "nav-hook", "askq-capture"];

pub fn is_mode(name: &str) -> bool {
    MODES.contains(&name)
}

/// O que o hook lê do processo, separado para o teste montar o seu.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// A casa do usuário (`HOME`; no Windows, `USERPROFILE`).
    pub home: Option<PathBuf>,
    /// Variáveis que os hooks consultam; ausente e vazia valem o mesmo onde o Python usava `or`.
    pub vars: BTreeMap<String, String>,
}

/// As variáveis que algum hook lê. `PATH` vai junto para o `tmux` do `nav-hook` ser achado pelo mesmo.
const VARS: &[&str] = &[
    "CLAUDE_CONFIG_DIR", "CP_SESSION_KEY", "TMUX_PANE", "HANGAR_JEV", "TYPESAFE_API_KEY", "JEV_ENDPOINT",
    "JEV_MODEL", "LOCALAPPDATA", "PATH",
];

impl Context {
    pub fn from_process() -> Self {
        let vars = VARS.iter()
            .filter_map(|key| std::env::var_os(key).map(|value| ((*key).to_owned(), value.to_string_lossy().into_owned())))
            .collect();
        Self { home: std::env::home_dir(), vars }
    }

    /// O valor não vazio: o `os.environ.get(x) or …` do Python.
    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|value| !value.is_empty())
    }

    /// Pasta base dos marcadores: `CLAUDE_CONFIG_DIR` não vazio, senão `~/.claude`.
    pub fn config_base(&self) -> Option<PathBuf> {
        self.var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| Some(self.home.as_ref()?.join(".claude")))
    }
}

/// O que o processo do hook devolve ao agente.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub code: u8,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Um hook pelo nome, com stdin e contexto dados. `None` = nome que não é deste crate.
pub fn handle(mode: &str, stdin: &[u8], context: &Context) -> Option<Outcome> {
    Some(match mode {
        "state-hook" => state::handle(stdin, context, &tree::System, tree::parent_pid()),
        "preview-hook" => preview::handle(stdin, context),
        "nav-hook" => nav::handle(stdin, context),
        "askq-capture" => askq::handle(stdin, context),
        _ => return None,
    })
}

/// O subcomando inteiro: stdin do agente, ambiente do processo, saídas e código.
pub fn run(mode: &str) -> ExitCode {
    let mut stdin = Vec::new();
    let read = std::io::stdin().lock().read_to_end(&mut stdin);
    let outcome = match read {
        Ok(_) => handle(mode, &stdin, &Context::from_process()).unwrap_or_else(|| Outcome { code: 2, ..Outcome::default() }),
        // Só a prévia avisava a falha de leitura; os outros calavam.
        Err(_) if mode == "preview-hook" => preview::failure("OSError"),
        Err(_) => Outcome::default(),
    };
    if !outcome.stdout.is_empty() {
        let _ = std::io::stdout().lock().write_all(&outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        let _ = std::io::stderr().lock().write_all(&outcome.stderr);
    }
    ExitCode::from(outcome.code)
}
