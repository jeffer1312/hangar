//! Sessões Claude sem terminal com cano falso, para os testes que só precisam de alguém do Rust que
//! receba o recado (grupos, par, MCP…): o texto de cada tentativa fica anotado por sessão, e o resultado
//! da escrita (`written`, `not_written`, `unknown`) e uma espera antes do ACK são escolhidos pelo teste.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::runtime::protocol::{CanoBinding, RuntimeTarget};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Default)]
struct Inner {
    texts: HashMap<String, Vec<String>>,
    /// Todas as entregas, na ordem de chegada.
    order: Vec<(String, String)>,
    outcome: HashMap<String, &'static str>,
    hold: HashMap<String, Duration>,
}

/// Quem recebeu o quê. Clonável: todas as cópias veem os mesmos recados.
#[derive(Clone, Default)]
pub struct Sink(Arc<Mutex<Inner>>);

impl Sink {
    pub fn texts(&self, name: &str) -> Vec<String> { self.0.lock().unwrap().texts.get(name).cloned().unwrap_or_default() }
    pub fn count(&self, name: &str) -> usize { self.texts(name).len() }
    pub fn total(&self) -> usize { self.0.lock().unwrap().texts.values().map(Vec::len).sum() }
    pub fn last(&self, name: &str) -> Option<String> { self.texts(name).pop() }
    /// O último recado que chegou a qualquer sessão.
    pub fn last_any(&self) -> Option<String> { self.0.lock().unwrap().order.last().map(|(_, text)| text.clone()) }
    /// O ACK das próximas escritas da sessão (`written` por padrão).
    pub fn outcome(&self, name: &str, outcome: &'static str) { self.0.lock().unwrap().outcome.insert(name.into(), outcome); }
    /// Espera antes do ACK (cano lento); zero tira.
    pub fn hold(&self, name: &str, wait: Duration) { self.0.lock().unwrap().hold.insert(name.into(), wait); }

    /// Abre `name` no registro como Claude sem terminal, ligado a um cano falso desta sessão.
    pub async fn open(&self, registry: &RuntimeRegistry, dir: &Path, name: &str) {
        registry.open(RuntimeTarget { key: format!("k-{name}"), generation: 1, name: name.into(), provider: "claude".into(),
            metadata: json!({"name": name, "headless": true, "session_id": "sid-1", "initialized": true}),
            binding: CanoBinding { pid: 42, escuta: self.cano(name).await, token: "secret-test".into(), versao: 2 },
            lease_path: dir.join(format!("{name}.lock")), state_path: dir.join(format!("{name}.queue-state.json")),
            projection_dir: dir.join(format!("{name}-projection")), transcript: dir.join(format!("{name}.jsonl")), created: 0.0 }).await.unwrap();
    }

    async fn cano(&self, name: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sink, name) = (self.clone(), name.to_owned());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (sink, name) = (sink.clone(), name.clone());
                tokio::spawn(async move {
                    let (read, mut write) = tokio::io::split(stream);
                    let mut reader = BufReader::new(read);
                    let mut header = String::new();
                    if reader.read_line(&mut header).await.is_err() || header != "secret-test\n" { return; }
                    let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,"aberto":false,
                        "pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
                    write.write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
                    loop {
                        let mut raw = String::new();
                        if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
                        let envelope: Value = serde_json::from_str(&raw).unwrap();
                        let frame: Value = serde_json::from_str(envelope["frame"].as_str().unwrap_or("{}")).unwrap_or_default();
                        let text = frame["message"]["content"][0]["text"].as_str().unwrap_or_default().to_owned();
                        let (outcome, wait) = {
                            let mut inner = sink.0.lock().unwrap();
                            let outcome = inner.outcome.get(&name).copied().unwrap_or("written");
                            // Conta a tentativa, como o `/input` que o Python falso anotava antes de responder.
                            inner.texts.entry(name.clone()).or_default().push(text.clone());
                            inner.order.push((name.clone(), text));
                            (outcome, inner.hold.get(&name).copied().unwrap_or_default())
                        };
                        if !wait.is_zero() { tokio::time::sleep(wait).await; }
                        let ack = json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":outcome});
                        let _ = write.write_all(format!("{ack}\n").as_bytes()).await;
                    }
                });
            }
        });
        format!("tcp:{address}")
    }
}

/// Registro do runtime com a política falsa que as aberturas pedem e o processo do cano conhecido.
pub async fn registry() -> RuntimeRegistry {
    RuntimeRegistry::new(policy().await, "secret-test".into(), "instance-test".into())
        .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1")))
}

pub async fn policy() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        if line == "\r\n" { break; }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse().unwrap(); }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let reply = json!({"ok":true,"data":{}}).to_string();
                    let response = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{reply}", reply.len());
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    address
}
