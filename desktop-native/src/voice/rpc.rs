//! Cliente do `codex app-server` em stdio: JSON-RPC 2.0, um objeto por linha.
use serde_json::{Value, json};
use crate::app::setup::system::{find_program, refreshed_path};
use std::{collections::HashMap, path::PathBuf, process::Stdio, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}}, time::Duration};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, process::{Child, ChildStdin, Command}, sync::{Mutex as AsyncMutex, oneshot}};

const DEADLINE: Duration = Duration::from_secs(30);

/// O `codex` achado e o PATH com que ele roda: o atalho do npm/fnm precisa do `node` nesse PATH.
#[derive(Clone, Debug)]
pub struct Codex { pub bin: PathBuf, pub path: String }

#[derive(Debug)]
pub enum RpcError { Spawn, Closed, Timeout, Server(String) }

pub enum Incoming {
    Notification { method: String, params: Value },
    Request { id: Value, method: String, params: Value },
    Exited,
}

enum Routed { Reply(u64, Result<Value, RpcError>), Incoming(Incoming) }

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct Rpc { stdin: AsyncMutex<ChildStdin>, next: AtomicU64, pending: Pending, closed: Arc<AtomicBool>, pid: Option<u32>, _child: Child }

// `kill_on_drop` só mata o `cmd.exe` quando o atalho é `codex.cmd`; a árvore inteira cai pelo taskkill.
impl Drop for Rpc {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            let _ = crate::app::setup::system::hidden(std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()])).output();
        }
        // SIGTERM ao grupo e, após uma folga, SIGKILL em quem ficou; a espera não pode travar quem largou o Rpc.
        // ponytail: o grupo pode, em tese, ser reaproveitado na folga; conferir com `group_alive` se isso aparecer.
        #[cfg(target_os = "linux")]
        if let Some(pgid) = self.pid.filter(|&p| p > 1 && p <= i32::MAX as u32) {
            unsafe { libc::kill(-(pgid as i32), libc::SIGTERM); }
            std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(500)); unsafe { libc::kill(-(pgid as i32), libc::SIGKILL); } });
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = self.pid;
    }
}

fn route(line: &str) -> Option<Routed> {
    let value: Value = serde_json::from_str(line).ok()?;
    match (value.get("method").and_then(Value::as_str), value.get("id")) {
        (Some(method), Some(id)) => Some(Routed::Incoming(Incoming::Request {
            id: id.clone(), method: method.to_owned(), params: value["params"].clone() })),
        (Some(method), None) => Some(Routed::Incoming(Incoming::Notification {
            method: method.to_owned(), params: value["params"].clone() })),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let result = match value.get("error") {
                Some(error) => Err(RpcError::Server(error["message"].as_str().unwrap_or("erro do app-server").to_owned())),
                None => Ok(value["result"].clone()),
            };
            Some(Routed::Reply(id, result))
        }
        (None, None) => None,
    }
}

/// Bloqueia (o PATH refeito pode rodar `npm prefix -g`): quem chama o faz fora da thread da tela.
pub fn find_codex() -> Option<Codex> {
    let path = refreshed_path();
    find_program("codex", &path).map(|bin| Codex { bin, path })
}

impl Rpc {
    pub async fn spawn(codex: &Codex, home: Option<&std::path::Path>) -> Result<(Rpc, async_channel::Receiver<Incoming>), RpcError> {
        let mut env = vec![("PATH", std::ffi::OsString::from(&codex.path))];
        if let Some(home) = home { env.push(("CODEX_HOME", home.as_os_str().to_owned())); }
        Self::spawn_program(&codex.bin, &["app-server".as_ref()], &env).await
    }

    /// Qualquer servidor JSON-RPC de um objeto por linha (o MCP do HCC também); `env` soma ao herdado.
    pub async fn spawn_program(program: impl AsRef<std::ffi::OsStr>, args: &[&std::ffi::OsStr], env: &[(&str, std::ffi::OsString)]) -> Result<(Rpc, async_channel::Receiver<Incoming>), RpcError> {
        let mut command = Command::new(program);
        command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        for (key, value) in env { command.env(key, value); }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: sem console piscando ao ligar a voz
        // Grupo próprio: o Drop derruba os netos (o agente do HCC) junto, não só o filho direto.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|_| RpcError::Spawn)?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or(RpcError::Spawn)?;
        let stdout = child.stdout.take().ok_or(RpcError::Spawn)?;
        let pending: Pending = Default::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let (readers, reader_closed) = (pending.clone(), closed.clone());
        tokio::spawn(async move {
            // Bytes, não `lines()`: uma linha fora de UTF-8 não pode encerrar a leitura.
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                if !matches!(reader.read_until(b'\n', &mut buf).await, Ok(n) if n > 0) { break; }
                match route(&String::from_utf8_lossy(&buf)) {
                    Some(Routed::Reply(id, result)) => {
                        if let Some(waiter) = readers.lock().unwrap().remove(&id) { let _ = waiter.send(result); }
                    }
                    Some(Routed::Incoming(incoming)) => { if tx.send(incoming).await.is_err() { break; } }
                    None => {}
                }
            }
            // A bandeira sobe antes do esvaziamento: `request` confere depois de se registrar, então nenhuma fica sem resposta.
            reader_closed.store(true, Ordering::SeqCst);
            for (_, waiter) in readers.lock().unwrap().drain() { let _ = waiter.send(Err(RpcError::Closed)); }
            let _ = tx.send(Incoming::Exited).await;
        });
        Ok((Rpc { stdin: AsyncMutex::new(stdin), next: AtomicU64::new(0), pending, closed, pid, _child: child }, rx))
    }

    // O prazo cobre a trava e a escrita: pipe parado não pode prender o stdin para sempre.
    async fn write(&self, value: Value) -> Result<(), RpcError> {
        let mut line = value.to_string();
        line.push('\n');
        let send = async { self.stdin.lock().await.write_all(line.as_bytes()).await.map_err(|_| RpcError::Closed) };
        tokio::time::timeout(DEADLINE, send).await.unwrap_or(Err(RpcError::Timeout))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> { self.request_within(method, params, DEADLINE).await }

    pub async fn request_within(&self, method: &str, params: Value, deadline: Duration) -> Result<Value, RpcError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if self.closed.load(Ordering::SeqCst) {
            self.pending.lock().unwrap().remove(&id);
            return Err(RpcError::Closed);
        }
        if let Err(error) = self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => { self.pending.lock().unwrap().remove(&id); Err(RpcError::Timeout) }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params})).await
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<(), RpcError> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "result": result})).await
    }
}

pub async fn handshake(rpc: &Rpc) -> Result<Value, RpcError> {
    rpc.request("initialize", json!({"clientInfo": {"name": "hangar_native_voice", "version": "1"},
        "capabilities": {"experimentalApi": true}})).await?;
    rpc.notify("initialized", json!({})).await?;
    Ok(rpc.request("config/read", json!({"includeLayers": false})).await?["config"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn reply_notification_and_server_request_are_told_apart() {
        assert!(matches!(route(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#), Some(Routed::Reply(3, _))));
        assert!(matches!(route(r#"{"jsonrpc":"2.0","method":"thread/realtime/sdp","params":{"sdp":"v=0"}}"#),
            Some(Routed::Incoming(Incoming::Notification { .. }))));
        // Pedido do servidor: tem `method` E `id`; o id do servidor é independente dos nossos.
        assert!(matches!(route(r#"{"jsonrpc":"2.0","id":3,"method":"item/tool/call","params":{}}"#),
            Some(Routed::Incoming(Incoming::Request { .. }))));
        assert!(route("warning: not json").is_none());
    }

    #[test]
    fn error_reply_carries_message() {
        let Some(Routed::Reply(1, Err(RpcError::Server(message)))) =
            route(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"conversation is not running"}}"#) else { panic!() };
        assert_eq!(message, "conversation is not running");
    }

    #[tokio::test]
    async fn exited_child_fails_pending_requests() {
        // Um "codex" que sai na hora: a request pendente falha com Closed em vez de esperar o prazo.
        let (program, args): (&str, Vec<&std::ffi::OsStr>) = if cfg!(windows) { ("cmd", vec!["/c".as_ref(), "exit".as_ref()]) } else { ("true", vec![]) };
        let (rpc, incoming) = Rpc::spawn_program(program, &args, &[]).await.unwrap();
        let result = rpc.request("initialize", serde_json::json!({})).await;
        assert!(matches!(result, Err(RpcError::Closed)));
        assert!(matches!(incoming.recv().await, Ok(Incoming::Exited)));
    }
}
