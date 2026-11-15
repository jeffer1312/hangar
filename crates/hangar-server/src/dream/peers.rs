//! A máquina que sonha conversa com as outras por rotas de dono (o token do `peers.json` é o de dono
//! de lá): pede o inventário, o texto das transcrições e a memória, e devolve a memória consolidada.
//! O corpo de cada chamada fica abaixo de 1 MiB, o teto do `PeerClient`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::{json, Value};

use super::inventory::SessionRef;
use crate::routes::AppState;

fn enc(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn failed(machine: &str, e: impl std::fmt::Debug) -> String {
    format!("dream_peer_failed: {machine}: {e:?}")
}

pub async fn inventory(st: &AppState, machine: &str, since: Option<DateTime<Utc>>) -> Result<Vec<SessionRef>, String> {
    let since = since.map(|t| enc(&t.to_rfc3339())).unwrap_or_default();
    let value = st.peers.call(machine, Method::GET, &format!("/api/dream/inventory?since={since}"), None).await.map_err(|e| failed(machine, e))?;
    serde_json::from_value(value.unwrap_or(Value::Null)["sessions"].clone()).map_err(|e| failed(machine, e))
}

/// Todas as partes do texto novo de uma sessão de lá, e até onde ela foi lida.
pub async fn transcript(st: &AppState, machine: &str, path: &str, from: usize) -> Result<(Vec<String>, usize), String> {
    let mut parts = Vec::new();
    let mut part = 0;
    // A primeira resposta fixa até onde ler; as outras partes param no mesmo evento.
    let mut until: Option<u64> = None;
    loop {
        let to = until.map(|u| format!("&to={u}")).unwrap_or_default();
        let url = format!("/api/dream/transcript?path={}&from={from}&part={part}{to}", enc(path));
        let value = st.peers.call(machine, Method::GET, &url, None).await.map_err(|e| failed(machine, e))?.unwrap_or(Value::Null);
        let total = value["parts"].as_u64().unwrap_or(0) as usize;
        let end = value["end_event"].as_u64().unwrap_or(from as u64);
        until.get_or_insert(end);
        if let Some(text) = value["text"].as_str() {
            parts.push(text.to_string());
        }
        part += 1;
        if part >= total {
            return Ok((parts, until.unwrap_or(end) as usize));
        }
    }
}

pub async fn memory_get(st: &AppState, machine: &str, root: &str) -> Result<hangar_dream::memory::Snapshot, String> {
    let value = st.peers.call(machine, Method::GET, &format!("/api/dream/memory?root={}", enc(root)), None).await.map_err(|e| failed(machine, e))?.unwrap_or(Value::Null);
    let files: BTreeMap<String, String> = serde_json::from_value(value["files"].clone()).map_err(|e| failed(machine, e))?;
    let digest = value["digest"].as_str().unwrap_or_default().to_string();
    Ok(hangar_dream::memory::Snapshot { files, digest })
}

pub async fn memory_put(st: &AppState, machine: &str, root: &str, files: &BTreeMap<String, String>, expect: &str, from: &str) -> Result<(), String> {
    let body = json!({"root": root, "files": files, "expect_digest": expect, "from_machine": from});
    st.peers.call(machine, Method::PUT, "/api/dream/memory", Some(&body)).await.map(|_| ()).map_err(|e| failed(machine, e))
}

/// `expect`: o peer só grava se o arquivo de lá ainda for este (`dream_memory_changed`, senão).
pub async fn user_put(st: &AppState, machine: &str, content: &str, expect: Option<&str>, from: &str) -> Result<(), String> {
    let body = json!({"content": content, "expect": expect, "from_machine": from});
    st.peers.call(machine, Method::PUT, "/api/dream/user-memory", Some(&body)).await.map(|_| ()).map_err(|e| failed(machine, e))
}
