//! Catálogo passivo dos artefatos registrados pelo Claude; não lê fontes nem publica conteúdo.
use crate::{
    routes::{AppState, cors, gate, route_failed},
    side::Binding,
    transcript::Provider,
};
use axum::{
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::SystemTime,
};

#[derive(Clone, Debug, Serialize)]
pub struct Artifact {
    pub id: String,
    pub url: String,
    pub title: String,
    pub version: Option<String>,
}

/// Limitado por sessões, sem índice em disco; uma trava de leitura por catálogo, não por servidor.
#[derive(Default)]
pub struct Catalogs(Mutex<IndexMap<String, Cached>>);
struct Cached {
    binding: Binding,
    entry: Arc<Mutex<Entry>>,
}
impl Catalogs {
    pub fn forget(&self, name: &str) {
        self.0.lock().unwrap().shift_remove(name);
    }

    pub fn read(&self, name: &str, binding: Binding) -> io::Result<Vec<Artifact>> {
        let entry = {
            let mut entries = self.0.lock().unwrap();
            let entry = entries
                .shift_remove(name)
                .filter(|cached| cached.binding == binding)
                .map(|cached| cached.entry)
                .unwrap_or_else(|| Arc::new(Mutex::new(Entry::new(binding.clone()))));
            entries.insert(
                name.to_owned(),
                Cached {
                    binding,
                    entry: entry.clone(),
                },
            );
            while entries.len() > 64 {
                entries.shift_remove_index(0);
            }
            entry
        };
        let mut entry = entry.lock().unwrap();
        if let Err(error) = entry.sync() {
            // Leitura parcial ou erro nunca vira retrato bom; a próxima tentativa relê a fonte.
            entry.reset();
            return Err(error);
        }
        Ok(entry
            .fold
            .items
            .values()
            .map(|record| record.item.clone())
            .collect())
    }
}

pub async fn route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(name): RoutePath<String>,
    req: Request,
) -> Response {
    let (_, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner {
        return reply(
            &req,
            StatusCode::FORBIDDEN,
            json!({"detail":{"code":"artifacts_owner_only","msg":"Os artefatos são acessíveis somente ao dono."}}),
        );
    }
    let info = match st.info(&name).await {
        Ok(Some(info)) => info,
        Ok(None) => {
            st.artifacts.forget(&name);
            return reply(
                &req,
                StatusCode::NOT_FOUND,
                json!({"detail":{"code":"session_not_found","msg":"Sessão inexistente."}}),
            );
        }
        Err(_) => {
            return route_failed(
                &st,
                req.headers(),
                "rust.artifacts_failed",
                &name,
                "artifacts_info",
                "a descoberta da sessão falhou",
            );
        }
    };
    let fresh = Binding::from_info(&info);
    let binding = fresh
        .as_ref()
        .and_then(|fresh| {
            st.side
                .hubs
                .bound(&name)
                .filter(|bound| !bound.changes_at_once(fresh))
        })
        .or(fresh);
    let Some(binding) =
        binding.filter(|b| matches!(b.provider, Provider::Claude | Provider::ClaudeHeadless))
    else {
        st.artifacts.forget(&name);
        return reply(&req, StatusCode::OK, json!({"items":[]}));
    };
    let catalogs = st.artifacts.clone();
    let session = name.clone();
    match tokio::task::spawn_blocking(move || catalogs.read(&session, binding)).await {
        Ok(Ok(items)) => reply(&req, StatusCode::OK, json!({"items":items})),
        Ok(Err(_)) => route_failed(
            &st,
            req.headers(),
            "rust.artifacts_failed",
            &name,
            "artifacts_io",
            "a leitura dos metadados de artefatos falhou",
        ),
        Err(_) => route_failed(
            &st,
            req.headers(),
            "rust.artifacts_failed",
            &name,
            "artifacts_worker",
            "a leitura dos artefatos caiu no servidor",
        ),
    }
}
fn reply(req: &Request, status: StatusCode, value: Value) -> Response {
    let mut response = (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        value.to_string(),
    )
        .into_response();
    cors(req.headers(), response.headers_mut());
    response
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    changed: (i64, i64),
    size: u64,
}
impl Stamp {
    fn of(_file: &File, metadata: &Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        let (identity, changed) = {
            use std::os::unix::fs::MetadataExt;
            (
                (metadata.dev(), metadata.ino()),
                (metadata.ctime(), metadata.ctime_nsec()),
            )
        };
        #[cfg(windows)]
        let (identity, changed) = {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo,
                GetFileInformationByHandle, GetFileInformationByHandleEx,
            };
            let mut basic = FILE_BASIC_INFO::default();
            // O handle é o mesmo da leitura; consultar o caminho poderia observar outra substituição.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    _file.as_raw_handle(),
                    FileBasicInfo,
                    (&mut basic as *mut FILE_BASIC_INFO).cast(),
                    std::mem::size_of::<FILE_BASIC_INFO>() as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            let ok = unsafe { GetFileInformationByHandle(_file.as_raw_handle(), &mut info) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
            let identity = (u64::from(info.dwVolumeSerialNumber), index);
            (identity, (basic.ChangeTime, 0))
        };
        #[cfg(not(any(unix, windows)))]
        let (identity, changed) = ((0, 0), (0, 0));
        if changed == (0, 0) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sinal de alteração do transcript indisponível",
            ));
        }
        Ok(Self {
            identity,
            created: metadata.created().ok(),
            modified: metadata.modified().ok(),
            changed,
            size: metadata.len(),
        })
    }
    fn same_file(&self, other: &Self) -> bool {
        self.identity == other.identity && self.created == other.created
    }
    /// A mudança mais recente que a assinatura registra (conteúdo ou metadados).
    fn latest_change(&self) -> Option<SystemTime> {
        #[cfg(windows)]
        let changed = u64::try_from(self.changed.0 - 116_444_736_000_000_000).ok()
            .map(|ticks| SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(ticks.saturating_mul(100)));
        #[cfg(not(windows))]
        let changed = u64::try_from(self.changed.0).ok()
            .map(|secs| SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, self.changed.1 as u32));
        changed.max(self.modified)
    }
}
/// O relógio dos arquivos anda em saltos (~15,6 ms no Windows): duas gravações no mesmo salto dão a mesma assinatura.
const STAMP_TICK: std::time::Duration = std::time::Duration::from_millis(50);
struct Entry {
    binding: Binding,
    stamp: Option<Stamp>,
    /// Quando a assinatura guardada foi lida: só prova o conteúdo se foi um salto depois da última mudança.
    read_at: Option<SystemTime>,
    offset: u64,
    prefix_hash: ring::digest::Context,
    fold: Fold,
    /// O transcript já abriu nesta vida; o `reset` não o esquece.
    seen: bool,
}
impl Entry {
    fn new(binding: Binding) -> Self {
        Self {
            binding,
            stamp: None,
            read_at: None,
            offset: 0,
            prefix_hash: ring::digest::Context::new(&ring::digest::SHA256),
            fold: Fold::default(),
            seen: false,
        }
    }
    fn reset(&mut self) {
        self.stamp = None;
        self.read_at = None;
        self.offset = 0;
        self.prefix_hash = ring::digest::Context::new(&ring::digest::SHA256);
        self.fold = Fold::default();
    }
    fn sync(&mut self) -> io::Result<()> {
        let mut file = match File::open(&self.binding.jsonl) {
            Ok(file) => file,
            // Nunca lido nesta vida: o Claude só cria o transcript na primeira mensagem. O que sumiu depois
            // de lido continua erro.
            Err(error) if error.kind() == io::ErrorKind::NotFound && !self.seen => {
                if crate::warn_limit::allow(None, "artifacts_transcript_absent") {
                    tracing::info!(path=%self.binding.jsonl.display(), "artefatos: transcript ainda não criado; catálogo vazio");
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        self.seen = true;
        let stamp = Stamp::of(&file, &file.metadata()?)?;
        let read_at = SystemTime::now();
        let unchanged = self.stamp.as_ref() == Some(&stamp);
        // Assinatura igual lida no mesmo salto da última mudança não prova nada: confere o conteúdo indexado.
        let racy = unchanged && self.read_at.zip(stamp.latest_change()).is_none_or(|(at, changed)| at < changed + STAMP_TICK);
        let mut reusable = self.stamp.as_ref().is_some_and(|old| {
            old.same_file(&stamp) && stamp.size >= old.size && (unchanged || stamp.size > old.size)
        });
        // Crescer não prova append: valida todos os bytes indexados, sem refazer o parsing deles.
        if reusable && (!unchanged || racy) {
            let actual = prefix_digest(&mut file, self.offset)?;
            let expected = self.prefix_hash.clone().finish();
            reusable = actual.as_ref() == expected.as_ref();
        }
        if !reusable {
            self.reset();
        }
        if reusable && unchanged {
            self.read_at = Some(read_at);
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new((&mut file).take(stamp.size - self.offset));
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
                break;
            }
            if !line.iter().all(u8::is_ascii_whitespace) {
                // O Claude Code às vezes grava dois registros colados numa linha e o primeiro cortado; a conversa
                // segue legível, então a linha estragada fica de fora e o resto do catálogo vale.
                match serde_json::from_slice::<Value>(&line) {
                    Ok(value) => self.fold.line(value),
                    Err(_) => {
                        if crate::warn_limit::allow(None, "artifacts_invalid_line") {
                            tracing::warn!(path=%self.binding.jsonl.display(), offset=self.offset, "artefatos: linha de transcript inválida ignorada");
                        }
                    }
                }
            }
            self.prefix_hash.update(&line);
            self.offset += line.len() as u64;
        }
        drop(reader);
        self.stamp = Some(stamp);
        self.read_at = Some(read_at);
        Ok(())
    }
}
fn prefix_digest(file: &mut File, length: u64) -> io::Result<ring::digest::Digest> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hash.finish())
}

struct Record {
    item: Artifact,
    seq: Option<u64>,
    published: bool,
}
#[derive(Default)]
struct Fold {
    items: IndexMap<String, Record>,
    pending: HashMap<String, Action>,
    deleted: HashSet<String>,
    completed: HashSet<String>,
}
enum Action {
    Publish,
    Delete(String),
}
impl Fold {
    fn line(&mut self, value: Value) {
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if value["type"] == "frame-link" {
            if let Some(url) = value["frameUrl"].as_str()
                && let Some(id) = artifact_id(url)
            {
                if self.deleted.contains(id) {
                    return;
                }
                let record = self.record(id, url);
                // O frame não identifica versão: não rebaixa metadados confirmados pela publicação.
                if !record.published
                    && let Some(title) = value["title"].as_str()
                {
                    record.item.title = title.to_owned();
                }
            }
            return;
        }
        let Some(blocks) = value["message"]["content"].as_array() else {
            return;
        };
        if value["type"] == "assistant" {
            for block in blocks {
                if block["type"] != "tool_use"
                    || !matches!(
                        block["name"].as_str(),
                        Some("Artifact" | "functions.Artifact")
                    )
                {
                    continue;
                }
                let Some(id) = block["id"].as_str() else {
                    continue;
                };
                if self.completed.contains(id) {
                    continue;
                }
                let input = &block["input"];
                let action = match input.get("action") {
                    None => Some(Action::Publish),
                    Some(Value::String(action)) if action == "publish" => Some(Action::Publish),
                    Some(Value::String(action)) if action == "delete" => input["url"]
                        .as_str()
                        .and_then(artifact_id)
                        .map(|id| Action::Delete(id.to_owned())),
                    _ => None,
                };
                if let Some(action) = action {
                    self.pending.insert(id.to_owned(), action);
                }
            }
        } else if value["type"] == "user" {
            let results: Vec<_> = blocks
                .iter()
                .filter(|block| block["type"] == "tool_result")
                .collect();
            // O envelope estruturado pertence a um resultado só; nunca atribuir URL por proximidade.
            let metadata = &value["toolUseResult"];
            for result in &results {
                let Some(call) = result["tool_use_id"].as_str() else {
                    continue;
                };
                let Some(action) = self.pending.remove(call) else {
                    continue;
                };
                self.completed.insert(call.to_owned());
                if results.len() != 1
                    || result["is_error"] == true
                    || metadata.get("error").is_some_and(|v| !v.is_null())
                    || metadata["ok"] == false
                    || metadata["success"] == false
                {
                    continue;
                }
                match action {
                    Action::Publish => self.publish(metadata),
                    // Não deduzir exclusão de texto livre ou da simples ausência de is_error.
                    Action::Delete(id) if metadata["deleted"] == true => {
                        self.items.shift_remove(&id);
                        self.deleted.insert(id);
                    }
                    Action::Delete(_) => {}
                }
            }
        }
    }
    fn record(&mut self, id: &str, url: &str) -> &mut Record {
        self.items.entry(id.to_owned()).or_insert_with(|| Record {
            item: Artifact {
                id: id.to_owned(),
                url: url.to_owned(),
                title: String::new(),
                version: None,
            },
            seq: None,
            published: false,
        })
    }
    fn publish(&mut self, metadata: &Value) {
        let Some(url) = metadata["url"].as_str() else {
            return;
        };
        let Some(id) = artifact_id(url) else {
            return;
        };
        // Exclusão confirmada é definitiva; um registro antigo nunca restaura a identidade.
        if self.deleted.contains(id) {
            return;
        }
        let seq = metadata["seq"].as_u64();
        let previous = self.items.get(id).and_then(|record| record.seq);
        // seq é a revisão ordenável; version é uma identidade opaca, nunca um contador.
        if previous.zip(seq).is_some_and(|(old, new)| new <= old) {
            return;
        }
        let record = self.record(id, url);
        record.item.url = url.to_owned();
        if let Some(title) = metadata["title"].as_str() {
            record.item.title = title.to_owned();
        }
        record.item.version = metadata["version"].as_str().map(str::to_owned);
        record.seq = seq.or(record.seq);
        record.published = true;
    }
}
fn artifact_id(url: &str) -> Option<&str> {
    // A forma literal impede credenciais, portas, query, fragmento e normalização de path/host.
    let path = url.strip_prefix("https://claude.ai/")?;
    let id = path
        .strip_prefix("artifact/")
        .or_else(|| path.strip_prefix("code/artifact/"))?;
    if path.starts_with("code/artifact/")
        && (id.len() != 36
            || !id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }))
    {
        return None;
    }
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
    .then_some(id)
}

#[cfg(all(test, windows))]
mod windows_identity_tests {
    use super::Stamp;
    use std::{fs::OpenOptions, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle, SetFileTime,
    };

    #[test]
    fn preserved_creation_does_not_hide_a_different_handle_identity() {
        let dir = tempfile::tempdir().unwrap();
        let first = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.path().join("first.jsonl"))
            .unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.path().join("second.jsonl"))
            .unwrap();
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        assert_ne!(
            unsafe { GetFileInformationByHandle(first.as_raw_handle(), &mut info) },
            0
        );
        // Preserva a criação no sistema de arquivos de verdade, sem montar um Stamp fictício.
        assert_ne!(
            unsafe {
                SetFileTime(
                    second.as_raw_handle(),
                    &info.ftCreationTime,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let first_meta = first.metadata().unwrap();
        let second_meta = second.metadata().unwrap();
        assert_eq!(
            first_meta.created().unwrap(),
            second_meta.created().unwrap()
        );
        let first_stamp = Stamp::of(&first, &first_meta).unwrap();
        let second_stamp = Stamp::of(&second, &second_meta).unwrap();
        assert_ne!(first_stamp.identity, second_stamp.identity);
        assert!(!first_stamp.same_file(&second_stamp));
    }
}

#[cfg(test)]
mod same_tick_tests {
    use super::*;

    fn frame(id: &str) -> String {
        format!("{}\n", json!({"type":"frame-link","frameUrl":format!("https://claude.ai/artifact/{id}"),"title":"Novo"}))
    }

    /// Duas gravações do mesmo tamanho no mesmo tique do relógio (~15,6 ms no Windows) dão a mesma assinatura:
    /// logo depois de uma gravação ela não prova que o conteúdo é o mesmo.
    #[test]
    fn a_same_size_rewrite_with_the_same_stamp_right_after_a_write_is_reread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, frame("c")).unwrap();
        let mut entry = Entry::new(Binding { provider: Provider::Claude, jsonl: path.clone(), key: "k".into(), headless: false, life: None });
        entry.sync().unwrap();
        assert!(entry.fold.items.contains_key("c"));
        std::fs::write(&path, frame("d")).unwrap();
        // O tique de Windows, forçado: a assinatura lida antes é a mesma de agora.
        let file = File::open(&path).unwrap();
        entry.stamp = Some(Stamp::of(&file, &file.metadata().unwrap()).unwrap());
        entry.sync().unwrap();
        assert!(entry.fold.items.contains_key("d"), "serviu o conteúdo velho");
        assert!(!entry.fold.items.contains_key("c"));
    }
}
