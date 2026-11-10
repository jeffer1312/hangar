//! Sessões Claude sem terminal com cano falso, para os testes que só precisam de alguém do Rust que
//! receba o recado (grupos, par, MCP…): o texto de cada tentativa fica anotado por sessão, e o resultado
//! da escrita (`written`, `not_written`, `unknown`) e uma espera antes do ACK são escolhidos pelo teste.
//! Cada recado escrito fecha o próprio turno (`result`), para o seguinte não ficar na fila da sessão.
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
    /// Falhas do cano falso (pânico em tarefa some): toda leitura do teste estoura com elas.
    errors: Vec<String>,
}

/// Quem recebeu o quê. Clonável: todas as cópias veem os mesmos recados.
#[derive(Clone, Default)]
pub struct Sink(Arc<Mutex<Inner>>);

impl Sink {
    /// O estado, depois de conferir que o cano falso não falhou.
    fn read(&self) -> std::sync::MutexGuard<'_, Inner> {
        let inner = self.0.lock().unwrap();
        assert!(inner.errors.is_empty(), "o cano falso falhou: {:?}", inner.errors);
        inner
    }
    fn failed(&self, name: &str, error: impl std::fmt::Display) { self.0.lock().unwrap().errors.push(format!("{name}: {error}")); }
    pub fn texts(&self, name: &str) -> Vec<String> { self.read().texts.get(name).cloned().unwrap_or_default() }
    pub fn count(&self, name: &str) -> usize { self.texts(name).len() }
    pub fn total(&self) -> usize { self.read().texts.values().map(Vec::len).sum() }
    pub fn last(&self, name: &str) -> Option<String> { self.texts(name).pop() }
    /// Todos os recados, `(sessão, texto)`, na ordem de chegada.
    pub fn order(&self) -> Vec<(String, String)> { self.read().order.clone() }
    /// O último recado que chegou a qualquer sessão.
    pub fn last_any(&self) -> Option<String> { self.read().order.last().map(|(_, text)| text.clone()) }
    /// O ACK das próximas escritas da sessão (`written` por padrão).
    pub fn outcome(&self, name: &str, outcome: &'static str) { self.0.lock().unwrap().outcome.insert(name.into(), outcome); }
    /// Espera antes do ACK (cano lento); zero tira.
    pub fn hold(&self, name: &str, wait: Duration) { self.0.lock().unwrap().hold.insert(name.into(), wait); }

    /// Abre `name` no registro como Claude sem terminal, ligado a um cano falso desta sessão.
    pub async fn open(&self, registry: &Arc<RuntimeRegistry>, dir: &Path, name: &str) {
        // Com o registro ligado, recado `[de: …]`/`[painel: …]` sem socket nativo cai no cano, como num
        // Claude sem o canal de mensagens.
        registry.bind_lifecycle();
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
                    if let Err(error) = write.write_all(format!("{snapshot}\n").as_bytes()).await { return sink.failed(&name, error); }
                    loop {
                        let mut raw = String::new();
                        if reader.read_line(&mut raw).await.unwrap_or(0) == 0 { return; }
                        let envelope: Value = match serde_json::from_str(&raw) { Ok(envelope) => envelope, Err(error) => return sink.failed(&name, error) };
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
                        let mut out = format!("{ack}\n");
                        // O turno que o recado abriu acaba na hora: sem o `result` a sessão fica "trabalhando" para
                        // sempre, e todo recado seguinte espera na fila dela sem nunca chegar aqui.
                        if outcome == "written" && frame["type"] == "user" {
                            let result = json!({"type":"cano_output","frame":json!({"type":"result","subtype":"success","is_error":false}).to_string()});
                            out.push_str(&format!("{result}\n"));
                        }
                        let _ = write.write_all(out.as_bytes()).await;
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
        .with_processes(Arc::new(RecipeProcesses("sid-1")))
}

/// Cano 42 com o `claude` 43 da conversa dada (`--session-id`). Mora aqui para o módulo servir também
/// à prova de orquestração (`examples/orq_contract`), que o inclui pelo caminho.
pub struct RecipeProcesses(pub &'static str);
impl hangar_server::list::procs::ProcessView for RecipeProcesses {
    fn children(&self,_:Duration)->std::io::Result<Arc<hangar_server::list::procs::ChildrenMap>> {Ok(Arc::new(std::collections::HashMap::from([(42,vec![43])]))) }
    fn argv(&self,_:i64)->Vec<String> {vec![]}
    fn cwd(&self,_:i64)->Option<std::path::PathBuf> {Some(std::env::temp_dir())}
    fn env_var(&self,_:i64,_:&str)->std::io::Result<Option<std::ffi::OsString>> {Ok(None)}
    fn start_time(&self,_:i64)->Option<f64> {Some(1000.0)}
    fn recipe(&self,pid:i64)->std::io::Result<hangar_server::list::procs::ProcessRecipe> {
        let argv=match pid {42=>vec!["hangar-cano".into()],43=>vec!["claude".into(),"--session-id".into(),self.0.into()],_=>return Err(std::io::ErrorKind::NotFound.into())};
        Ok(hangar_server::list::procs::ProcessRecipe {pid,birth:1000.0,executable:if pid==42 {"/fixture/hangar-cano".into()} else {"/fixture/claude".into()},cwd:std::env::temp_dir(),argv,environ:vec!["HOME=/fixture".into()],cgroup:None})
    }
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
