//! O que cada atualização do servidor trouxe de novidade e até onde cada aparelho já viu, em
//! `~/.hangar/update-news.json`. Aparelho que aparece pela primeira vez começa no ponto atual: não
//! recebe o que chegou antes dele.
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_ARRIVALS: usize = 50;
const MAX_DEVICES: usize = 50;

/// Novidades que o aparelho ainda não viu; `through` é o que ele devolve ao marcar visto.
#[derive(Debug, Default, Serialize)]
pub struct Unseen { pub news: Vec<Value>, pub through: u64 }

#[derive(Debug, Default, Deserialize, Serialize)]
struct Store {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    seq: u64,
    arrivals: Vec<Arrival>,
    devices: BTreeMap<String, Device>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Arrival { seq: u64, from: String, to: String, at: String, news: Vec<Value> }

#[derive(Debug, Deserialize, Serialize)]
struct Device { cursor: u64, at: String }

/// Guarda `to` como a versão conhecida e, havendo novidade, a chegada dela. Sem novidade não há
/// chegada e devolve `None`.
///
/// # Errors
/// Arquivo ilegível ou falha ao gravar.
pub fn record(home: &Path, from: &str, to: &str, news: Vec<Value>) -> io::Result<Option<u64>> {
    update(home, |store| {
        store.version = Some(to.to_owned());
        if news.is_empty() { return None; }
        store.seq += 1;
        store.arrivals.push(Arrival { seq: store.seq, from: from.to_owned(), to: to.to_owned(), at: now(), news });
        Some(store.seq)
    })
}

/// A última versão com que o servidor gravou; `None` sem arquivo ou em arquivo anterior ao campo.
///
/// # Errors
/// Arquivo ilegível.
pub fn known_version(home: &Path) -> io::Result<Option<String>> { Ok(read(&path(home))?.version) }

/// Novidades das chegadas depois do cursor do aparelho, sem id repetido: a chegada mais nova vem
/// primeiro e vence o id repetido; dentro dela a ordem é a gravada. Aparelho desconhecido é
/// registrado no `seq` atual e não recebe nada.
///
/// # Errors
/// Id de aparelho inválido, arquivo ilegível ou falha ao gravar o aparelho novo.
pub fn unseen(home: &Path, device: &str) -> io::Result<Unseen> {
    check(device)?;
    let path = path(home);
    // Leitura sem trava: a gravação troca o arquivo inteiro. Só o aparelho novo grava, e relê sob a trava.
    let mut store = read(&path)?;
    if !store.devices.contains_key(device) {
        let _lock = lock(home)?;
        store = read(&path)?;
        if !store.devices.contains_key(device) {
            store.devices.insert(device.to_owned(), Device { cursor: store.seq, at: now() });
            write(&path, &mut store)?;
            return Ok(Unseen { news: Vec::new(), through: store.seq });
        }
    }
    let cursor = store.devices[device].cursor;
    let mut ids = HashSet::new();
    let mut news = Vec::new();
    let mut through = cursor;
    for arrival in store.arrivals.iter().rev().filter(|arrival| arrival.seq > cursor) {
        through = through.max(arrival.seq);
        for item in &arrival.news {
            let fresh = item.get("id").and_then(Value::as_str).is_none_or(|id| ids.insert(id.to_owned()));
            if fresh { news.push(item.clone()); }
        }
    }
    Ok(Unseen { news, through })
}

/// Avança o cursor do aparelho até `through` (cortado no `seq`), nunca para trás.
///
/// # Errors
/// Id de aparelho inválido, arquivo ilegível ou falha ao gravar.
pub fn mark_seen(home: &Path, device: &str, through: u64) -> io::Result<u64> {
    check(device)?;
    update(home, |store| {
        let target = through.min(store.seq);
        // Primeiro contato também por aqui começa no ponto atual, como no `unseen`.
        let entry = store.devices.entry(device.to_owned()).or_insert(Device { cursor: store.seq, at: String::new() });
        entry.cursor = entry.cursor.max(target);
        entry.at = now();
        entry.cursor
    })
}

pub fn valid_device(device: &str) -> bool {
    (1..=64).contains(&device.len()) && device.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn check(device: &str) -> io::Result<()> {
    if valid_device(device) { Ok(()) } else { Err(io::Error::new(io::ErrorKind::InvalidInput, "id de aparelho inválido")) }
}

fn update<T>(home: &Path, change: impl FnOnce(&mut Store) -> T) -> io::Result<T> {
    let _lock = lock(home)?;
    let path = path(home);
    let mut store = read(&path)?;
    let outcome = change(&mut store);
    write(&path, &mut store)?;
    Ok(outcome)
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("update-news.json") }

// Servidor e CLI gravam o mesmo arquivo: a trava é entre processos, não só entre threads.
fn lock(home: &Path) -> io::Result<File> {
    let dir = home.join(".hangar");
    std::fs::create_dir_all(&dir)?;
    let file = OpenOptions::new().read(true).append(true).create(true).open(dir.join("update-news.lock"))?;
    file.lock()?;
    Ok(file)
}

fn read(path: &Path) -> io::Result<Store> {
    match hangar_core::fs::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, store: &mut Store) -> io::Result<()> {
    let extra = store.arrivals.len().saturating_sub(MAX_ARRIVALS);
    store.arrivals.drain(..extra);
    if store.devices.len() > MAX_DEVICES {
        let mut seen: Vec<(Option<DateTime<Utc>>, String)> = store.devices.iter()
            .map(|(id, device)| (DateTime::parse_from_rfc3339(&device.at).ok().map(|at| at.to_utc()), id.clone()))
            .collect();
        seen.sort();
        for (_, id) in &seen[..seen.len() - MAX_DEVICES] { store.devices.remove(id); }
    }
    hangar_core::fs::atomic_write(path, &serde_json::to_vec(store)?)
}

fn now() -> String { Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true) }
