//! O `codex app-server` (e o MCP do computer) por JSON-RPC em linhas, sobre o cliente do `hangar-codex`.
use hangar_codex::client::{Client, ClientError, Incoming as CodexIncoming};
use serde_json::{Value, json};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum RpcError { Spawn, Closed, Timeout, Server(String) }

pub enum Incoming { Notification { method: String, params: Value }, Request { id: Value, method: String, params: Value }, Exited }

/// Dono do filho: largar o `Rpc` derruba o processo e o grupo dele (o agente do computer é neto).
pub struct Rpc { client: Client, _child: Option<ChildGuard> }

struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(pid) = self.0.id() {
            let mut kill = std::process::Command::new("taskkill");
            kill.args(["/T", "/F", "/PID", &pid.to_string()]);
            { use std::os::windows::process::CommandExt; kill.creation_flags(0x0800_0000); }
            let _ = kill.output();
        }
        // SIGTERM ao grupo e, após uma folga, SIGKILL em quem ficou; a espera não pode travar quem largou o Rpc.
        #[cfg(unix)]
        if let Some(pgid) = self.0.id().filter(|&p| p > 1 && p <= i32::MAX as u32) {
            unsafe { libc::kill(-(pgid as i32), libc::SIGTERM); }
            std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(500)); unsafe { libc::kill(-(pgid as i32), libc::SIGKILL); } });
        }
    }
}

fn error(e: ClientError) -> RpcError {
    match e { ClientError::Timeout => RpcError::Timeout, ClientError::Rpc { message, .. } => RpcError::Server(message), _ => RpcError::Closed }
}

fn pump(mut from: tokio::sync::mpsc::Receiver<CodexIncoming>) -> async_channel::Receiver<Incoming> {
    let (tx, rx) = async_channel::unbounded();
    tokio::spawn(async move {
        while let Some(item) = from.recv().await {
            let item = match item {
                CodexIncoming::Notification { method, params } => Incoming::Notification { method, params },
                CodexIncoming::Request { id, method, params } =>
                    Incoming::Request { id: serde_json::to_value(id).unwrap_or(Value::Null), method, params },
            };
            if tx.send(item).await.is_err() { return; }
        }
        let _ = tx.send(Incoming::Exited).await;
    });
    rx
}

impl Rpc {
    pub async fn spawn(mut command: tokio::process::Command) -> Result<(Rpc, async_channel::Receiver<Incoming>), RpcError> {
        command.stderr(std::process::Stdio::null()).kill_on_drop(true);
        // CREATE_NO_WINDOW: sem console piscando ao ligar a voz.
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        // Grupo próprio: o Drop derruba os netos (o agente do HCC) junto, não só o filho direto.
        #[cfg(unix)]
        command.process_group(0);
        let (client, incoming, child) = Client::spawn_stdio(command).map_err(|_| RpcError::Spawn)?;
        Ok((Rpc { client, _child: Some(ChildGuard(child)) }, pump(incoming)))
    }

    pub fn over_lines(reader: impl tokio::io::AsyncRead + Unpin + Send + 'static, writer: impl tokio::io::AsyncWrite + Unpin + Send + 'static)
        -> (Rpc, async_channel::Receiver<Incoming>) {
        let (client, incoming) = Client::over_lines(reader, writer);
        (Rpc { client, _child: None }, pump(incoming))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> { self.request_within(method, params, DEADLINE).await }

    pub async fn request_within(&self, method: &str, params: Value, deadline: Duration) -> Result<Value, RpcError> {
        self.client.request_method::<Value>(method, params, deadline).await.map_err(error)
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> { self.client.notify(method, params).await.map_err(error) }

    pub async fn respond(&self, id: Value, result: Value) -> Result<(), RpcError> {
        let id = serde_json::from_value(id).map_err(|_| RpcError::Closed)?;
        self.client.respond(id, Ok(result)).await.map_err(error)
    }
}

pub async fn handshake(rpc: &Rpc) -> Result<Value, RpcError> {
    rpc.request("initialize", json!({"clientInfo": {"name": "hangar_voice", "version": "1"}, "capabilities": {"experimentalApi": true}})).await?;
    rpc.notify("initialized", json!({})).await?;
    Ok(rpc.request("config/read", json!({"includeLayers": false})).await?["config"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn request_reply_server_request_and_exit() {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (read, mut write) = tokio::io::split(theirs);
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let req: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(req["method"], "thread/start");
            write.write_all(format!("{}\n", json!({"id": req["id"], "result": {"thread": {"id": "t1"}}})).as_bytes()).await.unwrap();
            write.write_all(format!("{}\n", json!({"id": 7, "method": "item/tool/call", "params": {"tool": "list_sessions"}})).as_bytes()).await.unwrap();
            let reply: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(reply["id"], 7);
            assert_eq!(reply["result"]["success"], true);
        });
        let (r, w) = tokio::io::split(ours);
        let (rpc, incoming) = Rpc::over_lines(r, w);
        let started = rpc.request("thread/start", json!({})).await.unwrap();
        assert_eq!(started["thread"]["id"], "t1");
        let Ok(Incoming::Request { id, method, .. }) = incoming.recv().await else { panic!("pedido do servidor") };
        assert_eq!(method, "item/tool/call");
        rpc.respond(id, json!({"success": true})).await.unwrap();
        server.await.unwrap();
        assert!(matches!(incoming.recv().await, Ok(Incoming::Exited)), "fim do outro lado vira Exited");
    }

    #[tokio::test]
    async fn server_error_carries_message() {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (read, mut write) = tokio::io::split(theirs);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let req: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            write.write_all(format!("{}\n", json!({"id": req["id"], "error": {"code": -1, "message": "conversation is not running"}})).as_bytes()).await.unwrap();
        });
        let (r, w) = tokio::io::split(ours);
        let (rpc, _incoming) = Rpc::over_lines(r, w);
        assert!(matches!(rpc.request("x", json!({})).await, Err(RpcError::Server(m)) if m == "conversation is not running"));
    }
}
