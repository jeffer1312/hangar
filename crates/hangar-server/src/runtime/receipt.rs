use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchCursor {
    pub conversation: String,
    pub file_identity: Option<String>,
    pub offset: u64,
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absent_since: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Occurrence {
    pub id: String,
    pub conversation: String,
    pub file_identity: String,
    pub offset: u64,
    pub end_offset: u64,
    pub text: String,
    pub kind: String,
    pub timestamp: Option<f64>,
    #[serde(default)]
    pub recorded_conversation: Option<String>,
    /// Rollout do Codex sem `session_meta`: a conversa não tem como ser provada.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub identity_unprovable: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptProof {
    pub cursor: DispatchCursor,
    pub occurrence: Occurrence,
    pub normalized_text: String,
    pub observed_anchor: String,
}

/// Só o necessário para continuar a leitura: identidade, até onde leu e os 256 bytes antes disso.
/// Guardar o transcript inteiro custava o tamanho dele em memória e uma cópia a cada leitura.
pub struct ReceiptIndex {
    provider: String,
    conversation: String,
    identity: Option<String>,
    tail: Vec<u8>,
    occurrences: Vec<Occurrence>,
    scan_offset: u64,
    /// Conversa do `session_meta` do rollout: o Codex não grava a conversa em cada linha.
    meta_conversation: Option<String>,
}

fn anchor(data: &[u8]) -> String {
    sha1_smol::Sha1::from(data).digest().to_string()
}

/// Os até 256 bytes que terminam em `offset`; None se o arquivo é menor que isso.
fn bytes_before(file: &mut File, offset: u64) -> io::Result<Option<Vec<u8>>> {
    if offset > file.metadata()?.len() { return Ok(None); }
    let mut data = vec![0;offset.min(256) as usize];
    file.seek(SeekFrom::Start(offset - data.len() as u64))?;
    file.read_exact(&mut data)?;
    Ok(Some(data))
}

fn identity(file: &File) -> io::Result<String> {
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        let stat = file.metadata()?;
        Ok(format!("{:x}:{:x}",stat.dev(),stat.ino()))
    }
    #[cfg(windows)] {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        struct FileIdInfo { volume:u64, file_id:[u8;16] }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandleEx(handle:*mut std::ffi::c_void, class:i32, buffer:*mut std::ffi::c_void, size:u32) -> i32;
        }
        let mut info = FileIdInfo { volume:0,file_id:[0;16] };
        // A estrutura e o handle permanecem válidos durante a chamada síncrona.
        let result = unsafe { GetFileInformationByHandleEx(file.as_raw_handle(),18,
            (&mut info as *mut FileIdInfo).cast(),std::mem::size_of::<FileIdInfo>() as u32) };
        if result == 0 { return Err(io::Error::last_os_error()); }
        Ok(format!("{:x}:{:x}",info.volume,u128::from_le_bytes(info.file_id)))
    }
    #[cfg(not(any(unix,windows)))] {
        let _ = file;
        Err(io::Error::new(io::ErrorKind::Unsupported,"identidade de arquivo indisponível"))
    }
}

impl ReceiptIndex {
    pub fn new(provider: &str, conversation: &str) -> Self {
        Self { provider:provider.into(),conversation:conversation.into(),identity:None,tail:Vec::new(),occurrences:Vec::new(),scan_offset:0,meta_conversation:None }
    }

    pub fn capture(&self, path: &Path) -> io::Result<DispatchCursor> {
        let (identity, data, offset) = match File::open(path) {
            Ok(mut file) => {
                let id = identity(&file)?;
                let offset = file.seek(SeekFrom::End(0))?;
                let data = bytes_before(&mut file,offset)?.unwrap_or_default();
                (Some(id),data,offset)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (None,Vec::new(),0),
            Err(error) => return Err(error),
        };
        let absent_since = if identity.is_none() { Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?.as_secs_f64()) } else { None };
        Ok(DispatchCursor { conversation:self.conversation.clone(),file_identity:identity,offset,anchor:anchor(&data),absent_since })
    }

    /// Lê só o que foi acrescentado desde a última vez; troca de arquivo ou reescrita relê do início.
    pub fn scan(&mut self, path: &Path) -> io::Result<&[Occurrence]> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.tail.clear(); self.identity = None; self.occurrences.clear(); self.scan_offset = 0; self.meta_conversation = None;
                return Ok(&self.occurrences);
            }
            Err(error) => return Err(error),
        };
        let id = identity(&file)?;
        let unchanged = self.identity.as_deref() == Some(id.as_str())
            && bytes_before(&mut file,self.scan_offset)?.is_some_and(|tail|tail == self.tail);
        if !unchanged { self.occurrences.clear(); self.scan_offset = 0; self.meta_conversation = None; }
        file.seek(SeekFrom::Start(self.scan_offset))?;
        let mut reader = io::BufReader::with_capacity(1 << 16,&mut file);
        let mut parser = crate::transcript::LineParser::new(crate::transcript::Provider::Codex);
        let mut offset = self.scan_offset;
        let mut raw = Vec::new();
        loop {
            raw.clear();
            if reader.read_until(b'\n',&mut raw)? == 0 || raw.last() != Some(&b'\n') { break; }
            let start = offset; offset += raw.len() as u64;
            let Some(obj) = crate::transcript::decode_line(&raw) else { continue };
            if !obj.is_object() { continue; }
            if self.provider == "codex" && obj["type"] == "session_meta" {
                self.meta_conversation = obj["payload"]["id"].as_str().map(str::to_owned);
                continue;
            }
            let (text,kind) = if self.provider == "codex" {
                (parser.feed(&raw,start).into_iter().filter(|e|e.kind == hangar_api::chat::ChatKind::UserMsg)
                    .filter_map(|e|e.text).collect::<Vec<_>>().join("\n"),"user")
            } else {
                match obj["type"].as_str() {
                    Some("user") => (content_text(&obj["message"]["content"]),"user"),
                    Some("queue-operation") if obj["operation"] == "dequeue" => (obj["content"].as_str().unwrap_or("").to_owned(),"dequeue"),
                    Some("attachment") if obj["attachment"]["type"] == "queued_command" => (content_text(&obj["attachment"]["prompt"]),"steer"),
                    _ => continue,
                }
            };
            if text.trim().is_empty() { continue; }
            let provider_id = obj["uuid"].as_str().or_else(||obj["id"].as_str()).or_else(||obj["payload"]["id"].as_str());
            let record = provider_id.filter(|s|!s.is_empty()).map_or_else(||format!("offset:{start}"),|id|format!("id:{id}"));
            let timestamp = obj["timestamp"].as_str().and_then(crate::transcript::ts_of_iso);
            let recorded_conversation = if self.provider == "codex" { self.meta_conversation.clone() }
                else { obj["sessionId"].as_str().map(str::to_owned) };
            let identity_unprovable = self.provider == "codex" && recorded_conversation.is_none();
            // Vários recados num registro só: cada um é uma ocorrência, senão o primeiro gasta o registro e os outros nunca confirmam.
            let peers:Vec<String> = if kind == "user" { crate::transcript::history::peer_bodies(&text).into_iter().map(str::to_owned).collect() } else { Vec::new() };
            let parts:Vec<(String,String)> = if peers.is_empty() { vec![(String::new(),text)] }
                else { peers.into_iter().enumerate().map(|(n,body)|(format!("#peer{n}"),body)).collect() };
            for (suffix,text) in parts {
                self.occurrences.push(Occurrence { id:format!("{}|{id}|{record}{suffix}",self.conversation),conversation:self.conversation.clone(),
                    file_identity:id.clone(),offset:start,end_offset:offset,text,kind:kind.into(),timestamp,recorded_conversation:recorded_conversation.clone(),identity_unprovable });
            }
        }
        drop(reader);
        let current = File::open(path)?;
        if identity(&current)? != id || current.metadata()?.len() < offset {
            self.identity = None;
            return Err(io::Error::new(io::ErrorKind::InvalidData,"transcript mudou durante a leitura"));
        }
        self.tail = bytes_before(&mut file,offset)?.unwrap_or_default();
        self.identity = Some(id); self.scan_offset = offset;
        Ok(&self.occurrences)
    }

    /// A âncora do cursor é relida do arquivo: confere que os bytes antes do despacho não mudaram.
    pub fn match_after(&self, path: &Path, cursor: &DispatchCursor, row: &Value, used: &BTreeMap<String,Value>) -> io::Result<Option<ReceiptProof>> {
        // Cursor sem arquivo: o despacho veio antes de o transcript existir (primeira mensagem da
        // sessão), então tudo no arquivo da mesma conversa é posterior a ele.
        let born_after = cursor.file_identity.is_none() && cursor.offset == 0;
        if cursor.conversation != self.conversation || self.identity.is_none() || !born_after && cursor.file_identity != self.identity { return Ok(None); }
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let current = Some(identity(&file)?);
        if current != self.identity || !born_after && current != cursor.file_identity || cursor.offset > self.scan_offset { return Ok(None); }
        let Some(before) = bytes_before(&mut file,cursor.offset)? else { return Ok(None) };
        let observed = anchor(&before);
        if observed != cursor.anchor { return Ok(None); }
        let candidates = super::queue::entry_lines(row);
        for occurrence in &self.occurrences {
            if used.contains_key(&occurrence.id) || occurrence.offset < cursor.offset { continue; }
            if !cursor_accepts(cursor,occurrence) { continue; }
            let committed = crate::transcript::history::chaves_de_commit(&occurrence.text);
            if let Some(normalized_text) = candidates.iter().find(|c|committed.contains(*c)) {
                return Ok(Some(ReceiptProof { cursor:cursor.clone(),occurrence:occurrence.clone(),normalized_text:normalized_text.clone(),observed_anchor:observed }));
            }
        }
        Ok(None)
    }

    /// Entrada entregue antes de o Rust assumir a sessão não tem cursor: prova pela primeira
    /// ocorrência livre com o texto dela, da mesma conversa e registrada depois do envio.
    pub fn match_legacy(&self, row: &Value, used: &BTreeMap<String,Value>) -> Option<(Occurrence,String)> {
        self.identity.as_ref()?;
        let sent = row["ts"].as_f64()?;
        let candidates = super::queue::entry_lines(row);
        self.occurrences.iter().filter(|o|!used.contains_key(&o.id) && legacy_accepts(o,sent)).find_map(|occurrence| {
            let committed = crate::transcript::history::chaves_de_commit(&occurrence.text);
            candidates.iter().find(|c|committed.contains(*c)).map(|text|(occurrence.clone(),text.clone()))
        })
    }
}

/// Folga do relógio entre o carimbo da fila e o do transcript, a mesma do app.
const LEGACY_CLOCK_SLACK_S: f64 = 2.0;

pub(crate) fn legacy_accepts(occurrence: &Occurrence, sent: f64) -> bool {
    matches!(occurrence.kind.as_str(),"user" | "dequeue" | "steer")
        && occurrence.timestamp.is_some_and(|ts|ts + LEGACY_CLOCK_SLACK_S >= sent)
        && (occurrence.identity_unprovable || occurrence.recorded_conversation.as_deref() == Some(occurrence.conversation.as_str()))
}

fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() { return text.into(); }
    content.as_array().map(|blocks| blocks.iter().filter(|b|b["type"] == "text")
        .filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

impl ReceiptProof {
    pub fn validates(&self, cursor: &DispatchCursor, row: &Value) -> bool {
        self.cursor == *cursor && cursor_accepts(cursor,&self.occurrence)
            && self.occurrence.conversation == cursor.conversation && self.occurrence.offset >= cursor.offset
            && self.occurrence.end_offset > self.occurrence.offset && self.observed_anchor == cursor.anchor
            && matches!(self.occurrence.kind.as_str(),"user" | "dequeue" | "steer")
            && super::queue::entry_lines(row).contains(&self.normalized_text)
            && crate::transcript::history::chaves_de_commit(&self.occurrence.text).contains(&self.normalized_text)
    }
}

fn cursor_accepts(cursor:&DispatchCursor,occurrence:&Occurrence)->bool {
    if let Some(identity) = &cursor.file_identity { return identity == &occurrence.file_identity; }
    if cursor.offset != 0 { return false; }
    let Some(since) = cursor.absent_since else {
        // Cursor gravado antes do `absent_since`: tudo no arquivo da conversa é posterior a ele.
        return true;
    };
    // Rollout do Codex sem `session_meta`: não há como provar a conversa; vale o cursor sem arquivo.
    if occurrence.identity_unprovable { return true; }
    // Arquivo novo só comprova a conversa explícita e uma ocorrência posterior ao despacho.
    occurrence.recorded_conversation.as_deref() == Some(cursor.conversation.as_str())
        && occurrence.timestamp.is_some_and(|ts|ts >= since)
}
