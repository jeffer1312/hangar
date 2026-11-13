//! Uma vida do processo conector: lançamento sem shell, negociação, pedidos e fim da árvore.
use crate::manifest::Manifest;
use crate::protocol::{self, ConnectorError, ErrorCode, InitializeResult, PROTOCOL_VERSION};
use crate::rpc;
use hangar_core::terminal_process::{CommandTree, PRIVATE_ENV_KEYS};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// IDs nunca se repetem no servidor: resposta atrasada de outra vida não casa com pedido novo.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// O processo e a árvore dele. Só sai da `Life` para terminar.
struct Owned { child: Child, tree: CommandTree }

impl Owned {
    async fn finish(mut self) {
        hangar_core::terminal_process::finish(&mut self.tree).await;
        let _ = self.child.wait().await;
    }
}

pub(crate) struct Life {
    pub(crate) manifest: Manifest,
    /// `None` só depois que o fim da árvore já foi entregue a uma tarefa própria.
    owned: Option<Owned>,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Pedido em andamento ou transporte quebrado: a vida não pode receber outro pedido.
    pub(crate) poisoned: bool,
}

/// Resposta de erro do conector (vida intacta) ou falha do transporte (vida perdida).
pub(crate) type Reply = Result<Result<Value, ConnectorError>, ConnectorError>;

fn unavailable(message: impl Into<String>) -> ConnectorError { ConnectorError::new(ErrorCode::Unavailable, message) }

impl Life {
    pub(crate) async fn start(manifest: &Manifest, init_timeout: Duration, exit_grace: Duration) -> Result<Life, ConnectorError> {
        let mut command = Command::new(&manifest.command[0]);
        command.args(&manifest.command[1..]).current_dir(&manifest.cwd)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        // O conector é código do dono, mas não herda a autoridade do servidor.
        for key in PRIVATE_ENV_KEYS { command.env_remove(key); }
        let mut tree = CommandTree::configure(&mut command).map_err(|error| unavailable(format!("contenção do conector falhou: {:?}", error.kind())))?;
        hangar_core::terminal_process::die_with_parent(&mut command);
        let mut child = command.spawn().map_err(|error| unavailable(format!("conector não iniciou: {:?}", error.kind())))?;
        if let Err(error) = tree.attach(&child) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(unavailable(format!("contenção do conector falhou: {:?}", error.kind())));
        }
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.start_kill();
            return Err(unavailable("conector sem entrada ou saída"));
        };
        let mut life = Life { manifest: manifest.clone(), owned: Some(Owned { child, tree }), stdin: Some(stdin), stdout: BufReader::new(stdout), poisoned: true };
        let outcome = match tokio::time::timeout(init_timeout, life.initialize()).await {
            Ok(outcome) => outcome,
            Err(_) => Err(ConnectorError::new(ErrorCode::Timeout, "conector não respondeu à negociação no prazo")),
        };
        match outcome {
            Ok(()) => { life.poisoned = false; Ok(life) }
            Err(error) => { life.shutdown(exit_grace).await; Err(error) }
        }
    }

    async fn initialize(&mut self) -> Result<(), ConnectorError> {
        let mut params = protocol::initialize_params();
        if self.manifest.capabilities.contains_key(crate::work_actions::CAPABILITY) {
            params["capabilities"] = json!(self.manifest.capabilities);
        }
        let value = self.request(protocol::INITIALIZE, params).await??;
        let result: InitializeResult = serde_json::from_value(value)
            .map_err(|_| ConnectorError::protocol("resposta de initialize fora do contrato"))?;
        if result.protocol_version != PROTOCOL_VERSION {
            return Err(ConnectorError::new(ErrorCode::UnsupportedVersion, format!("conector fala o protocolo {}", result.protocol_version)));
        }
        if result.id != self.manifest.id {
            return Err(ConnectorError::protocol("conector se apresentou com outro id"));
        }
        if result.capabilities != self.manifest.capabilities {
            return Err(ConnectorError::protocol("capacidades do conector divergem do manifesto"));
        }
        Ok(())
    }

    /// Um pedido e a resposta dele. Qualquer outra coisa na saída quebra o transporte.
    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Reply {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let stdin = self.stdin.as_mut().ok_or_else(|| unavailable("entrada do conector fechada"))?;
        rpc::write_message(stdin, &protocol::request(id, method, params)).await?;
        let line = rpc::read_message(&mut self.stdout).await?.ok_or_else(|| unavailable("conector encerrou sem responder"))?;
        let message: Value = serde_json::from_slice(&line).map_err(|_| ConnectorError::protocol("saída do conector não é JSON-RPC"))?;
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(ConnectorError::protocol("saída do conector não é JSON-RPC 2.0"));
        }
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            return Err(ConnectorError::protocol("resposta não corresponde ao pedido"));
        }
        match (message.get("result"), message.get("error")) {
            (Some(result), None) => Ok(Ok(result.clone())),
            (None, Some(error)) => Ok(Err(ConnectorError::from_rpc(error))),
            _ => Err(ConnectorError::protocol("resposta sem result nem error")),
        }
    }

    /// Pede o fim, espera até `grace` e então termina a árvore inteira (o `finish` sinaliza e confirma).
    pub(crate) async fn shutdown(mut self, grace: Duration) {
        let polite = async {
            if !self.poisoned { let _ = self.request(protocol::SHUTDOWN, json!({})).await; }
            self.stdin = None;
            // Sem recolher o líder: o zumbi segura o número do grupo até o fim da árvore.
            if let Some(owned) = self.owned.as_mut() { let _ = hangar_core::terminal_process::leader_exited(&mut owned.child).await; }
        };
        let _ = tokio::time::timeout(grace, polite).await;
        // Em tarefa própria: quem espera pode ser cancelado (prazo, desligar) sem deixar a árvore viva.
        if let Some(owned) = self.owned.take() { let _ = tokio::spawn(owned.finish()).await; }
    }

}

/// Vida descartada sem `shutdown` (pedido cancelado na negociação ou no próprio encerramento): a
/// árvore termina numa tarefa à parte. O `kill_on_drop` sozinho levaria só o processo líder.
impl Drop for Life {
    fn drop(&mut self) {
        let Some(mut owned) = self.owned.take() else { return };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => { runtime.spawn(owned.finish()); }
            Err(_) => { let _ = owned.child.start_kill(); }
        }
    }
}

