//! A abertura com o servidor fora do ar (`hangar-server open`): a mesma biblioteca do servidor,
//! sem o que só ele guarda (fila, grupos, lista, diário). A sessão nasce igual, com o spec gravado, e
//! o servidor a adota quando voltar, pela varredura de sempre.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;

use super::OpenError;
use super::claude::PluginBridge;
use super::open::{Born, OpenHost};
use super::terminal::{Mux, MuxOutput, ProcessMux, TerminalEnv};

/// `CHAVE=valor` por linha do `backend/.env`; a última vale, como o `install-native` lê.
pub fn dotenv(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        let (key, value) = line.strip_prefix("export ").unwrap_or(line).split_once('=')?;
        let value = value.trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(value);
        (!key.trim().starts_with('#')).then(|| (key.trim().to_owned(), value.to_owned()))
    }).collect()
}

pub struct LocalHost { home: PathBuf, env: BTreeMap<String, String>, terminal: TerminalEnv, mux: ProcessMux, bridge: Option<PluginBridge>,
    reconcile_limit: Duration }

impl LocalHost {
    /// `env`: o ambiente que o servidor teria; `bridge`: a porta e o token do `.env` dele, para a
    /// ponte do plugin valer quando ele voltar (o token é derivado, não sorteado).
    pub fn new(env: BTreeMap<String, String>, bridge: Option<PluginBridge>) -> Self {
        let accounts = crate::accounts::environment::AccountEnvironment::from_map(env.clone());
        let terminal = TerminalEnv::local(&accounts.home, &env);
        Self { home: accounts.home, env, terminal, mux: ProcessMux::default(), bridge, reconcile_limit: super::open::RECONCILE_LIMIT }
    }

    #[cfg(test)]
    pub(crate) fn with_reconcile_limit(self, reconcile_limit: Duration) -> Self { Self { reconcile_limit, ..self } }
}

impl OpenHost for LocalHost {
    fn home(&self) -> &Path { &self.home }
    fn env(&self) -> &BTreeMap<String, String> { &self.env }
    fn terminal(&self) -> &TerminalEnv { &self.terminal }
    fn mux(&self) -> &dyn Mux { &self.mux }
    fn bridge(&self) -> Option<PluginBridge> { self.bridge.clone() }
    fn server_projects(&self) -> PathBuf {
        crate::accounts::environment::AccountEnvironment::from_map(self.env.clone()).claude_base.join("projects")
    }
    /// A `backend/` da cópia do Hangar (`cli::backend_dir`).
    fn backend_dir(&self) -> Option<PathBuf> { super::cli::backend_dir() }
    fn run(&self, argv: Vec<String>, timeout: Duration) -> BoxFuture<'_, MuxOutput> {
        Box::pin(super::server::run_program(&self.env, argv, timeout))
    }
    fn customizations(&self, _args: Value) -> BoxFuture<'_, Result<Value, OpenError>> {
        Box::pin(async { Err(OpenError::new(503, "claude_customizations_unavailable", "plugins e skills por sessão precisam do servidor no ar")) })
    }
    /// Sem o servidor não há quem limpe grupo, `then` e fila do nome reusado: com algum deles no disco
    /// a sessão nova nasceria no grupo da antiga, com a fila e o encadeamento dela. Recusa.
    fn leave_old_group<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, OpenError>> {
        Box::pin(async move {
            let claude = crate::accounts::environment::AccountEnvironment::from_map(self.env.clone()).claude_base;
            match leftover(&claude, name) {
                Some(path) => Err(OpenError::new(409, "session_open_name_needs_server", format!(
                    "o nome '{name}' ainda tem estado da sessão antiga ({}); com o servidor fora ninguém o limpa — suba o servidor ou use outro nome",
                    path.display())).with_params(serde_json::json!({"nome": name}))),
                None => Ok(Vec::new()),
            }
        })
    }
    fn release_name<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Result<(), OpenError>> { Box::pin(async { Ok(()) }) }
    fn born<'a>(&'a self, _born: &'a Born) -> BoxFuture<'a, Vec<String>> { Box::pin(async { Vec::new() }) }
    /// O servidor sobe o Codex sem terminal na primeira escrita depois de voltar; a primeira
    /// mensagem não tem quem a guarde até lá.
    fn warm_codex<'a>(&'a self, _name: &'a str, initial_prompt: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            initial_prompt.filter(|t| !t.is_empty()).map(|_| "A sessão foi criada com o servidor fora do ar; a primeira mensagem não foi \
                entregue: envie de novo quando ele voltar.".to_owned()).into_iter().collect()
        })
    }
    fn diary(&self, _events: Vec<Value>) {}
    /// No arquivo dos comandos de conta, fora do runtime: a trava do diário pode esperar até 5 s. O runtime
    /// espera a tarefa ao terminar, então o evento chega ao disco antes de o comando sair.
    fn account_diary(&self, events: Vec<Value>) {
        let env = crate::accounts::environment::AccountEnvironment::from_map(self.env.clone());
        tokio::task::spawn_blocking(move || crate::accounts::claude_config::write_cli_diary(&env, &events));
    }
    fn reconcile_limit(&self) -> Duration { self.reconcile_limit }
    fn new_session_id(&self) -> String { super::open::uuid4(&crate::mods::state::random_hex(16)) }
    fn new_bridge_key(&self) -> String { crate::mods::state::random_hex(16) }
    // O comando local só abre sessão com terminal.
    fn wake(&self, _name: &str) {}
    /// As reservas são do servidor; fora dele não há nenhuma.
    fn prewarm(&self) -> Option<&crate::prewarm::Store> { None }
    // Sem o servidor não há lista: o comando local não retoma conversa.
    fn live_sessions(&self) -> BoxFuture<'_, Result<Vec<(String, String)>, OpenError>> {
        Box::pin(async { Err(OpenError::new(503, "erro_lista_indisponivel", "retomar conversa precisa do servidor no ar")) })
    }
}

/// O primeiro arquivo que a sessão antiga deixou no nome; ilegível conta como presente.
pub(super) fn leftover(claude: &Path, name: &str) -> Option<PathBuf> {
    let stem = crate::groups::store::file_stem(name);
    [claude.join(".hangar-pair").join(format!("{stem}.json")), claude.join(".hangar-chain").join(format!("{stem}.json")),
        crate::locate::queue_path(claude, name)]
        .into_iter().find(|path| probe(path).unwrap_or(true))
}

/// `try_exists` em que a pasta-mãe que existe e não é pasta é erro em todo sistema: no Windows o
/// caminho sob um arquivo responde "não existe", e o nome passaria por livre.
pub(super) fn probe(path: &Path) -> std::io::Result<bool> {
    if let Some(parent) = path.parent()
        && parent.try_exists()?
        && !parent.is_dir()
    {
        return Err(std::io::ErrorKind::NotADirectory.into());
    }
    path.try_exists()
}
