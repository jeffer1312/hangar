//! O que cada atualização do servidor trouxe de novidade e até onde cada aparelho já viu, em
//! `~/.hangar/update-news.json`. Aparelho que aparece pela primeira vez recebe só o que chegou nas
//! últimas 24 h.
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_ARRIVALS: usize = 50;
const MAX_DEVICES: usize = 50;
const FIRST_CONTACT_HOURS: i64 = 24;

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

/// De quem é a chegada. Arquivo anterior ao campo é todo do Hangar.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Hangar,
    ComputerControl,
}

#[derive(Debug, Deserialize, Serialize)]
struct Arrival {
    seq: u64,
    from: String,
    to: String,
    at: String,
    news: Vec<Value>,
    // Ausente na chegada do Hangar: o arquivo dele continua igual ao de antes.
    #[serde(default, skip_serializing_if = "Kind::is_hangar")]
    kind: Kind,
}

#[derive(Debug, Deserialize, Serialize)]
struct Device { cursor: u64, at: String }

impl Kind {
    fn is_hangar(&self) -> bool { *self == Kind::Hangar }
}

/// Guarda `to` como a versão conhecida e, havendo novidade, a chegada dela. Sem novidade não há
/// chegada e devolve `None`.
///
/// # Errors
/// Falha ao gravar.
pub fn record(home: &Path, from: &str, to: &str, news: Vec<Value>) -> io::Result<Option<u64>> {
    update(home, |store| {
        store.version = Some(to.to_owned());
        // Volta para a versão de antes (a nova não ficou de pé): o que ela trouxe não está instalado.
        // Chegada de outro componente gravada depois não esconde a última do Hangar.
        if let Some(at) = store.arrivals.iter().rposition(|arrival| arrival.kind == Kind::Hangar)
            && store.arrivals[at].from == to
            && store.arrivals[at].to == from
        {
            store.arrivals.remove(at);
        }
        push(store, Kind::Hangar, from, to, news)
    })
}

/// Chegada de um componente que não é o Hangar (o hcu): não toca a versão conhecida e nunca é
/// desfeita pela volta de versão do Hangar.
///
/// # Errors
/// Falha ao gravar.
pub fn record_component(home: &Path, kind: Kind, from: &str, to: &str, news: Vec<Value>) -> io::Result<Option<u64>> {
    update(home, |store| push(store, kind, from, to, news))
}

/// A última versão com que o servidor gravou; `None` sem arquivo ou em arquivo anterior ao campo.
///
/// # Errors
/// Falha ao ler o arquivo.
pub fn known_version(home: &Path) -> io::Result<Option<String>> { Ok(read(&path(home))?.version) }

/// Novidades das chegadas depois do cursor do aparelho, sem id repetido: a chegada mais nova vem
/// primeiro e vence o id repetido; dentro dela a ordem é a gravada. Aparelho desconhecido é
/// registrado antes das chegadas das últimas 24 h (`first_cursor`).
///
/// # Errors
/// Id de aparelho inválido ou falha ao gravar o aparelho novo.
pub fn unseen(home: &Path, device: &str) -> io::Result<Unseen> {
    check(device)?;
    let path = path(home);
    // Leitura sem trava: a gravação troca o arquivo inteiro. Só o aparelho novo grava, e relê sob a trava.
    let mut store = read(&path)?;
    if !store.devices.contains_key(device) {
        let _lock = lock(home)?;
        store = read(&path)?;
        if !store.devices.contains_key(device) {
            let cursor = first_cursor(&store);
            store.devices.insert(device.to_owned(), Device { cursor, at: now() });
            write(&path, &mut store)?;
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
/// Id de aparelho inválido ou falha ao gravar.
pub fn mark_seen(home: &Path, device: &str, through: u64) -> io::Result<u64> {
    check(device)?;
    update(home, |store| {
        let target = through.min(store.seq);
        // Primeiro contato também por aqui começa como no `unseen`.
        let cursor = first_cursor(store);
        let entry = store.devices.entry(device.to_owned()).or_insert(Device { cursor, at: String::new() });
        entry.cursor = entry.cursor.max(target);
        entry.at = now();
        entry.cursor
    })
}

/// Onde o aparelho desconhecido começa: no `seq` atual, menos as chegadas das últimas 24 h. Na primeira atualização
/// todo aparelho é desconhecido, e sem a janela ninguém veria o que ela trouxe.
fn first_cursor(store: &Store) -> u64 {
    let since = Utc::now() - chrono::Duration::hours(FIRST_CONTACT_HOURS);
    store.arrivals.iter()
        .find(|arrival| DateTime::parse_from_rfc3339(&arrival.at).is_ok_and(|at| at.to_utc() >= since))
        .map_or(store.seq, |arrival| arrival.seq - 1)
}

pub fn valid_device(device: &str) -> bool {
    (1..=64).contains(&device.len()) && device.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn check(device: &str) -> io::Result<()> {
    if valid_device(device) { Ok(()) } else { Err(io::Error::new(io::ErrorKind::InvalidInput, "id de aparelho inválido")) }
}

/// Sem novidade não há chegada.
fn push(store: &mut Store, kind: Kind, from: &str, to: &str, news: Vec<Value>) -> Option<u64> {
    if news.is_empty() { return None; }
    store.seq += 1;
    store.arrivals.push(Arrival { seq: store.seq, from: from.to_owned(), to: to.to_owned(), at: now(), news, kind });
    Some(store.seq)
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
        // Ilegível recomeça vazio, senão nenhuma atualização futura seria gravada; só se perde até onde cada aparelho viu.
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_else(|cause| {
            let kept = path.with_extension("json.unreadable");
            let copied = std::fs::write(&kept, &bytes).is_ok();
            tracing::warn!(code = "update_news_unreadable", %cause, path = %path.display(), copied, kept = %kept.display(),
                "update-news.json ilegível; recomeça vazio");
            Store::default()
        })),
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
