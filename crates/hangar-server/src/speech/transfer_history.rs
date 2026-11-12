//! Leitura da origem congelada e dos turnos novos de uma conversa transferida, sem importar duas vezes.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hangar_api::{chat::ChatEvent, session::SessionRow};
use serde_json::Value;

use crate::transcript::{InternalInfo, LineParser, Provider, pyjson};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn required<'a>(value: &'a Value, name: &str) -> io::Result<&'a str> {
    value[name]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid("O registro da transferência perdeu um campo obrigatório."))
}
fn hex_digest(digest: ring::digest::Digest) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
type Signature = (
    u64,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
    u64,
    i64,
    i64,
);

fn signature(metadata: &std::fs::Metadata) -> Signature {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.len(),
            metadata.modified().ok(),
            metadata.created().ok(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    }
    #[cfg(not(unix))]
    {
        (
            metadata.len(),
            metadata.modified().ok(),
            metadata.created().ok(),
            0,
            0,
            0,
        )
    }
}
fn stable_read<T>(
    path: &Path,
    mut read: impl FnMut(&mut BufReader<File>, u64) -> io::Result<T>,
) -> io::Result<T> {
    for _ in 0..3 {
        let file = File::open(path)?;
        let before = signature(&file.metadata()?);
        if signature(&std::fs::metadata(path)?) != before {
            continue;
        }
        let mut reader = BufReader::new(file);
        // O tamanho observado limita cada tentativa; um escritor não prolonga a leitura indefinidamente.
        let result = read(&mut reader, before.0);
        if signature(&reader.get_ref().metadata()?) == before
            && signature(&std::fs::metadata(path)?) == before
        {
            return result;
        }
    }
    Err(invalid("A fonte da transferência mudou durante a leitura."))
}

pub(super) fn read(
    row: &SessionRow,
    home: &Path,
    info: &InternalInfo,
    limit: usize,
) -> io::Result<Option<Vec<ChatEvent>>> {
    if row.provider != "codex" || row.transfer_id.is_none() {
        return Ok(None);
    }
    with_stable_queue(info, || read_once(row, home, info, limit))
}

fn with_stable_queue<T>(
    info: &InternalInfo,
    mut read: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    for _ in 0..3 {
        // Só guardamos commits que reconciliam esta fila; uma troca dela exige refazer essa projeção.
        let before = queue_signature(info)?;
        let result = read();
        if queue_signature(info)? == before {
            return result;
        }
    }
    Err(invalid("A fila da transferência mudou durante a leitura."))
}

fn queue_signature(info: &InternalInfo) -> io::Result<Option<Signature>> {
    let Some(path) = info.history.get("queue").and_then(Value::as_str) else {
        return Ok(None);
    };
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(signature(&metadata))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_once(
    row: &SessionRow,
    home: &Path,
    info: &InternalInfo,
    limit: usize,
) -> io::Result<Option<Vec<ChatEvent>>> {
    if row.provider != "codex" {
        return Ok(None);
    }
    let Some(id) = &row.transfer_id else {
        return Ok(None);
    };
    if id.len() != 36
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(invalid("A identidade da transferência é inválida."));
    }
    let record: Value = serde_json::from_slice(&crate::runtime::queue::read_replaced(
        &home
            .join(".hangar/conversation-transfers")
            .join(format!("{id}.json")),
    )?)
    .map_err(|_| invalid("O registro da transferência está ilegível."))?;
    if record["id"].as_str() != Some(id)
        || record["name"].as_str() != Some(&row.name)
        || record["phase"] != "complete"
    {
        return Err(invalid(
            "A transferência não possui uma conversa concluída desta sessão.",
        ));
    }
    let source = &record["source"];
    let boundary = &record["boundary"];
    if source["provider"] != "claude" {
        return Err(invalid(
            "A fonte da transferência não é uma conversa Claude.",
        ));
    }
    let Some(path) = info.jsonl.as_deref() else {
        return Ok(None);
    };
    let rollout = Path::new(required(boundary, "rollout_path")?);
    let thread = required(boundary, "thread_id")?;
    let same = |a: &Path, b: &Path| {
        a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
    };
    if !same(path, rollout) {
        let account = record["destination_meta"]["codex_home"]
            .as_str()
            .filter(|v| !v.is_empty());
        if crate::locate::session_key(&path.to_string_lossy()) != thread
            || !row
                .codex_home
                .as_deref()
                .zip(account)
                .is_some_and(|(a, b)| same(Path::new(a), Path::new(b)))
        {
            // /clear conserva a vida da sessão, mas a conversa seguinte não herda a origem antiga.
            return Ok(None);
        }
    }
    let (mut old, previous, start) = source_events(source, id, limit)?;
    let queue_keys: HashSet<String> = info
        .history
        .get("queue")
        .and_then(Value::as_str)
        .map(|path| super::load_queue(Path::new(path)))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.get("text").and_then(Value::as_str).map(str::to_owned))
        .flat_map(|text| {
            [
                crate::transcript::py::strip(&text).to_owned(),
                crate::transcript::py::strip(&crate::transcript::history::strip_attach(&text))
                    .to_owned(),
            ]
        })
        .collect();
    let (mut items, committed, previous) =
        active_events(path, boundary, id, previous, limit, &queue_keys)?;
    super::merge_queue(info, &mut items, &committed, previous, start)?;
    retain_latest(&mut items, limit);
    old.extend(items.into_iter().map(|(_, _, event)| event));
    if old.len() > limit {
        old.drain(..old.len() - limit);
    }
    Ok(Some(old))
}

/// A fronteira conferida no rollout: o prefixo importado intacto e a thread do cabeçalho; devolve
/// onde os turnos novos começam.
fn check_prefix(reader: &mut BufReader<File>, length: u64, boundary: &Value) -> io::Result<u64> {
    let thread = required(boundary, "thread_id")?;
    let offset = boundary["min_offset"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= length)
        .ok_or_else(|| invalid("A fronteira da transferência está inválida."))?;
    let header = prefix(reader, offset, required(boundary, "prefix_digest")?)?;
    if header["type"] != "session_meta"
        || header["payload"]
            .get("id")
            .or_else(|| header["payload"].get("session_id"))
            .and_then(Value::as_str)
            != Some(thread)
    {
        return Err(invalid("A thread do rollout diverge da transferência."));
    }
    Ok(offset)
}

/// Os turnos novos do rollout depois da fronteira, sem as linhas importadas da origem.
fn active_events(
    path: &Path,
    boundary: &Value,
    id: &str,
    previous: f64,
    limit: usize,
    queue_keys: &HashSet<String>,
) -> io::Result<(Vec<super::Item>, HashMap<String, f64>, f64)> {
    let imported = imported_ids(boundary)?;
    stable_read(path, |reader, length| {
        let offset = check_prefix(reader, length, boundary)?;
        let mut items = Vec::new();
        let mut committed = HashMap::new();
        let mut previous = previous;
        let mut parser = LineParser::new(Provider::Codex);
        let mut position = offset;
        let mut active = reader.take(length - offset);
        let mut line = Vec::new();
        loop {
            line.clear();
            if active.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
                break;
            }
            let row = object(&line, "O rollout ativo contém uma linha inválida.")?;
            let imported_row = [&row["id"], &row["payload"]["id"]]
                .into_iter()
                .filter_map(Value::as_str)
                .any(|id| imported.contains(id));
            let current = position;
            position += line.len() as u64;
            if imported_row {
                continue;
            }
            let clock = super::timestamp(&row);
            if clock != 0. {
                previous = clock;
            }
            let mut events = parser.feed(&line, current);
            for event in &mut events {
                event.ts = Some(event.ts.filter(|t| *t != 0.).unwrap_or(previous));
                prefix_event(event, id, "codex");
            }
            super::absorb(events, previous, current, &mut items, &mut committed);
            committed.retain(|key, _| queue_keys.contains(key));
            retain_latest(&mut items, limit);
        }
        Ok((items, committed, previous))
    })
}

fn prefix_event(event: &mut ChatEvent, id: &str, provider: &str) {
    event.id = format!("transfer:{id}:{provider}:{}", event.id);
    if let Some(tool) = &mut event.tool_use_id {
        *tool = format!("transfer:{id}:{provider}:{tool}");
    }
}

fn object(bytes: &[u8], message: &str) -> io::Result<Value> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(pyjson::loads_lossless)
        .filter(Value::is_object)
        .ok_or_else(|| invalid(message))
}

fn prefix(reader: &mut BufReader<File>, offset: u64, expected: &str) -> io::Result<Value> {
    let mut prefix = reader.take(offset);
    let mut header = Vec::new();
    prefix.read_until(b'\n', &mut header)?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    hash.update(&header);
    let mut last = header.last().copied();
    let mut buffer = [0; 8192];
    loop {
        let n = prefix.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        last = Some(buffer[n - 1]);
    }
    if prefix.limit() != 0 || last != Some(b'\n') || hex_digest(hash.finish()) != expected {
        return Err(invalid("O prefixo importado do rollout mudou."));
    }
    object(&header, "O rollout perdeu a identidade da thread.")
}

fn source_index(
    reader: &mut BufReader<File>,
    length: u64,
    expected: &str,
) -> io::Result<(Vec<Value>, Vec<u64>)> {
    let mut scan = (&mut *reader).take(length);
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut metadata = Vec::new();
    let mut offsets = Vec::new();
    let mut position = 0;
    let mut line = Vec::new();
    loop {
        line.clear();
        if scan.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        hash.update(&line);
        if !line.ends_with(b"\n") {
            return Err(invalid(
                "O snapshot da origem da transferência mudou ou contém uma linha incompleta.",
            ));
        }
        let mut row = object(&line, "O snapshot possui uma linha inválida.")?;
        // O ramo precisa da topologia inteira, mas não dos textos, imagens ou resultados de ferramenta.
        row.as_object_mut().unwrap().retain(|key, _| {
            matches!(
                key.as_str(),
                "type"
                    | "uuid"
                    | "parentUuid"
                    | "logicalParentUuid"
                    | "subtype"
                    | "isSidechain"
                    | "isCompactSummary"
                    | "isMeta"
                    | "leafUuid"
            )
        });
        metadata.push(row);
        offsets.push(position);
        position += line.len() as u64;
    }
    if position == 0 || hex_digest(hash.finish()) != expected {
        return Err(invalid(
            "O snapshot da origem da transferência mudou ou contém uma linha incompleta.",
        ));
    }
    Ok((metadata, offsets))
}

/// O ramo selecionado do snapshot conferido: a topologia, onde cada linha começa e os índices do ramo.
fn selected_chain(
    reader: &mut BufReader<File>,
    length: u64,
    source: &Value,
) -> io::Result<(Vec<Value>, Vec<u64>, Vec<usize>)> {
    let (metadata, offsets) = source_index(reader, length, required(source, "digest")?)?;
    let selected = chain(&metadata)?;
    let selected_ids: HashSet<_> = selected
        .iter()
        .filter_map(|index| metadata[*index]["uuid"].as_str())
        .collect();
    let expected_ids: HashSet<_> = source["selected_uuids"]
        .as_array()
        .ok_or_else(|| invalid("O ramo da origem está inválido."))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid("O ramo da origem está inválido."))
        })
        .collect::<Result<_, _>>()?;
    if selected_ids != expected_ids {
        return Err(invalid(
            "O ramo da origem diverge do snapshot da transferência.",
        ));
    }
    Ok((metadata, offsets, selected))
}

fn source_events(source: &Value, id: &str, limit: usize) -> io::Result<(Vec<ChatEvent>, f64, f64)> {
    stable_read(Path::new(required(source, "path")?), |reader, length| {
        let (_, offsets, selected) = selected_chain(reader, length, source)?;
        let mut line = Vec::new();
        let mut old = VecDeque::new();
        let mut seen = HashSet::new();
        let mut previous = 0.;
        let mut start = 0.;
        for index in selected {
            reader.seek(SeekFrom::Start(offsets[index]))?;
            line.clear();
            let end = offsets.get(index + 1).copied().unwrap_or(length);
            (&mut *reader)
                .take(end - offsets[index])
                .read_until(b'\n', &mut line)?;
            let row = object(&line, "O snapshot possui uma linha inválida.")?;
            let clock = super::timestamp(&row);
            if clock != 0. {
                previous = clock;
            }
            if start == 0. {
                start = previous;
            }
            for mut event in crate::transcript::parse_claude_object(row.as_object().unwrap()) {
                if (event.id.starts_with("held:") || event.id.starts_with("delivery:"))
                    && !seen.insert(event.id.clone())
                {
                    continue;
                }
                event.ts = Some(event.ts.filter(|t| *t != 0.).unwrap_or(previous));
                prefix_event(&mut event, id, "claude");
                old.push_back(event);
                if old.len() > limit {
                    old.pop_front();
                }
            }
        }
        Ok((old.into_iter().collect(), previous, start))
    })
}

fn retain_latest(items: &mut Vec<super::Item>, limit: usize) {
    items.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    if items.len() > limit {
        items.drain(..items.len() - limit);
    }
}

fn chain(rows: &[Value]) -> io::Result<Vec<usize>> {
    let mut positions = HashMap::<&str, Vec<usize>>::new();
    for (index, row) in rows.iter().enumerate() {
        required(row, "type")?;
        for flag in ["isSidechain", "isCompactSummary", "isMeta"] {
            if row.get(flag).is_some_and(|value| !value.is_boolean()) {
                return Err(invalid("O ramo da origem possui um marcador inválido."));
            }
        }
        if !row["uuid"].is_null() {
            positions
                .entry(required(row, "uuid")?)
                .or_default()
                .push(index);
        }
    }
    let mut leaf = None;
    for (index, row) in rows.iter().enumerate().rev() {
        if row["type"] == "last-prompt" && row["isSidechain"] != true {
            let reference = required(row, "leafUuid")?;
            leaf = Some(
                *positions
                    .get(reference)
                    .and_then(|values| values.iter().rev().find(|value| **value < index))
                    .ok_or_else(|| invalid("O ramo da origem perdeu a folha selecionada."))?,
            );
            break;
        }
    }
    if leaf.is_none() {
        leaf = rows.iter().rposition(|row| {
            matches!(row["type"].as_str(), Some("user" | "assistant")) && row["isSidechain"] != true
        });
    }
    let mut leaf = leaf.ok_or_else(|| invalid("A origem não possui uma mensagem selecionada."))?;
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(leaf) {
            return Err(invalid("O ramo da origem possui um ciclo."));
        }
        let row = &rows[leaf];
        if row["isSidechain"] == true || row.get("parentUuid").is_none() {
            return Err(invalid("A topologia da origem não pode ser reconstruída."));
        }
        required(row, "uuid")?;
        selected.push(leaf);
        let parent = if row["parentUuid"].is_null() && row["subtype"] == "compact_boundary" {
            Some(required(row, "logicalParentUuid")?)
        } else {
            if !row["logicalParentUuid"].is_null() && row["subtype"] != "compact_boundary" {
                return Err(invalid("A origem possui uma ligação lógica inesperada."));
            }
            row["parentUuid"].as_str()
        };
        let Some(parent) = parent else {
            if !row["parentUuid"].is_null() {
                return Err(invalid("A origem possui uma ligação inválida."));
            }
            break;
        };
        leaf = *positions
            .get(parent)
            .and_then(|values| values.iter().rev().find(|index| **index < leaf))
            .ok_or_else(|| invalid("O ramo da origem perdeu uma mensagem anterior."))?;
    }
    selected.reverse();
    selected.retain(|index| rows[*index]["isCompactSummary"] != true);
    if !selected
        .iter()
        .any(|index| matches!(rows[*index]["type"].as_str(), Some("user" | "assistant")))
    {
        return Err(invalid("A origem não possui conversa."));
    }
    Ok(selected)
}

fn store(home: &Path) -> std::path::PathBuf {
    home.join(".hangar").join("conversation-transfers")
}

/// `load_transfer`: ausência é `None`; registro ilegível ou de outro id é erro.
pub(crate) fn load_record(home: &Path, id: &str) -> io::Result<Option<Value>> {
    let raw = match crate::runtime::queue::read_replaced(&store(home).join(format!("{id}.json"))) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        other => other?,
    };
    let record: Value = serde_json::from_slice(&raw)
        .map_err(|_| invalid("O registro da transferência está ilegível."))?;
    if record["id"].as_str() != Some(id) || !record["phase"].is_string() {
        return Err(invalid("O registro da transferência está ilegível."));
    }
    Ok(Some(record))
}

/// `_thread_key`: a mesma chave da lista, sem o `\\?\` que o canonicalize põe no Windows.
fn thread_key(codex_home: &Path, thread: &str) -> String {
    crate::list::local_transfers::thread_key(&codex_home.to_string_lossy(), thread)
}

/// `_destination`: a conta e a thread de destino gravadas no registro.
fn destination(record: &Value) -> Option<(&str, &str)> {
    fn text(v: &Value) -> Option<&str> { v.as_str().filter(|s| !s.is_empty()) }
    let home = text(&record["destination_meta"]["codex_home"])?;
    let thread = text(&record["destination_meta"]["thread_id"])
        .or_else(|| text(&record["boundary"]["thread_id"]))?;
    Some((home, thread))
}

/// `archive_transfer`: o registro mais novo desta conta e thread, pelo índice; registro achado que não
/// está concluído é erro.
pub(crate) fn archive_record(home: &Path, codex_home: &Path, thread: &str) -> io::Result<Option<Value>> {
    indexed_record(home, codex_home, thread, false)
}

fn indexed_record(home: &Path, codex_home: &Path, thread: &str, require_records: bool) -> io::Result<Option<Value>> {
    let key = thread_key(codex_home, thread);
    let name = format!(
        "thread-{}.json",
        hex_digest(ring::digest::digest(&ring::digest::SHA256, key.as_bytes()))
    );
    let raw = match crate::runtime::queue::read_replaced(&store(home).join("indexes").join(name)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        other => other?,
    };
    let ids: Vec<String> = serde_json::from_slice(&raw)
        .map_err(|_| invalid("O índice de transferência está inválido."))?;
    for id in ids.iter().rev() {
        let Some(record) = load_record(home, id)? else {
            if require_records { return Err(invalid("O índice aponta para um registro de transferência indisponível.")); }
            continue;
        };
        if destination(&record).is_some_and(|(h, t)| thread_key(Path::new(h), t) == key) {
            if record["phase"] != "complete" {
                return Err(invalid("A preparação da thread não é uma conversa concluída."));
            }
            return Ok(Some(record));
        }
    }
    Ok(None)
}

fn same(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .zip(b.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}

/// `_matches_rollout` e o início de `verify_boundary`: registro concluído, com origem Claude, e o
/// rollout é o da fronteira ou o mesmo movido dentro da conta de destino.
fn check_record(record: &Value, path: &Path, owner_home: &Path) -> io::Result<()> {
    let boundary = &record["boundary"];
    if record["phase"] != "complete" || !record["source"].is_object() || !boundary.is_object() {
        return Err(invalid("A transferência não possui história concluída."));
    }
    if record["source"]["provider"] != "claude" {
        return Err(invalid("A fonte da transferência não é uma conversa Claude."));
    }
    if !matches_rollout(record, path, owner_home) {
        return Err(invalid("O rollout não pertence à transferência."));
    }
    Ok(())
}

fn matches_rollout(record: &Value, path: &Path, owner_home: &Path) -> bool {
    let boundary = &record["boundary"];
    boundary["rollout_path"].as_str().is_some_and(|r| same(path, Path::new(r)))
        || (record["destination_meta"]["codex_home"].as_str().is_some_and(|h| same(owner_home, Path::new(h)))
            && boundary["thread_id"].as_str() == Some(crate::locate::session_key(&path.to_string_lossy()).as_str()))
}

fn imported_ids(boundary: &Value) -> io::Result<HashSet<&str>> {
    boundary["imported_item_ids"].as_array()
        .ok_or_else(|| invalid("A fronteira perdeu os identificadores importados."))?
        .iter().map(|v| v.as_str().ok_or_else(|| invalid("A fronteira possui um identificador inválido.")))
        .collect()
}

/// As linhas da origem conferida precedem as do rollout ativo; o bastão conserva o parser de cada trecho.
pub(crate) struct SelectedRows {
    pub rows: Vec<String>,
    pub source_len: usize,
    pub transfer_id: String,
}

/// Citações precisam das linhas originais do ramo, inclusive campos que não viram `ChatEvent`.
pub(crate) fn citation_rows(row: &SessionRow, home: &Path) -> io::Result<Option<Vec<String>>> {
    selected_rows(row, home).map(|selected| selected.map(|selected| selected.rows))
}

fn selected_rows(row: &SessionRow, home: &Path) -> io::Result<Option<SelectedRows>> {
    if row.provider != "codex" { return Ok(None); }
    let Some(path) = row.jsonl.as_deref().map(Path::new) else { return Ok(None) };
    let owner = row.codex_home.as_deref().map(Path::new)
        .ok_or_else(|| invalid("A conta da conversa está indisponível."))?;
    let record = match row.transfer_id.as_deref() {
        Some(id) => Some(load_record(home, id)?.ok_or_else(|| invalid("O registro da transferência desapareceu."))?),
        None => indexed_record(home, owner, &crate::locate::session_key(&path.to_string_lossy()), true)?,
    };
    let Some(record) = record else { return Ok(None) };
    if record["phase"] != "complete" || !record["source"].is_object() || !record["boundary"].is_object() {
        return Err(invalid("A transferência não possui história concluída."));
    }
    // /clear inicia outra conversa; só a thread da transferência herda a origem congelada.
    if !matches_rollout(&record, path, owner) { return Ok(None); }
    check_record(&record, path, owner)?;
    let source = &record["source"];
    let mut rows = stable_read(Path::new(required(source, "path")?), |reader, length| {
        let (_, offsets, selected) = selected_chain(reader, length, source)?;
        let mut rows = Vec::with_capacity(selected.len());
        for index in selected {
            reader.seek(SeekFrom::Start(offsets[index]))?;
            let end = offsets.get(index + 1).copied().unwrap_or(length);
            let mut line = String::new();
            (&mut *reader).take(end - offsets[index]).read_to_string(&mut line)?;
            rows.push(line);
        }
        Ok(rows)
    })?;
    let source_len = rows.len();
    let boundary = &record["boundary"];
    let imported = imported_ids(boundary)?;
    rows.extend(stable_read(path, |reader, length| {
        let offset = check_prefix(reader, length, boundary)?;
        let mut active = reader.take(length - offset);
        let mut rows = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            if active.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") { break; }
            let row = object(&line, "O rollout ativo contém uma linha inválida.")?;
            if [&row["id"], &row["payload"]["id"]].into_iter().filter_map(Value::as_str).any(|id| imported.contains(id)) {
                continue;
            }
            rows.push(String::from_utf8(line.clone()).map_err(|_| invalid("O rollout ativo contém uma linha inválida."))?);
        }
        Ok(rows)
    })?);
    Ok(Some(SelectedRows { rows, source_len, transfer_id: required(&record, "id")?.to_owned() }))
}

/// A mesma seleção alimenta os eventos e os campos crus do bastão, sem reler por seção.
pub(crate) struct BatonWindow {
    pub rows: Vec<String>,
    pub events: Vec<ChatEvent>,
    pub since: Option<f64>,
}

/// O teto de `_plano_citado` do Python: o plano citado e a janela de eventos cabem nele.
const BATON_TAIL: u64 = 8 * 1024 * 1024;

pub(crate) struct BatonReadError {
    pub verification: bool,
    pub cause: io::Error,
}

pub(crate) fn baton_window(row: &SessionRow, home: &Path, claude: &Path) -> Result<BatonWindow, BatonReadError> {
    let info = crate::locate::info_of(row, claude);
    let mut verification = false;
    with_stable_queue(&info, || {
        verification = false;
        let selected = if row.provider == "codex" && row.codex_home.is_none() && row.transfer_id.is_none() {None} else {
            selected_rows(row, home).map_err(|cause| { verification = true; cause })?
        };
        let transferred = selected.is_some();
        let (rows, source_len, id, base) = match selected {
            Some(selected) => (selected.rows, selected.source_len, selected.transfer_id, 0),
            None => {
                let Some(path) = row.jsonl.as_deref() else { return Ok(BatonWindow { rows:Vec::new(), events:Vec::new(), since:None }); };
                let (rows, base) = match stable_read(Path::new(path), |reader, length| {
                    // Só o fim: o bastão quer a janela recente e o último plano citado, como o Python.
                    // ponytail: teto fixo; 600 eventos além dele ficam de fora, crescer a janela se aparecer.
                    // Um byte antes do corte: se ele já fecha uma linha, a seguinte entra inteira.
                    let mut base = length.saturating_sub(BATON_TAIL + 1);
                    reader.seek(SeekFrom::Start(base))?;
                    let mut raw = Vec::new();
                    reader.by_ref().take(length - base).read_to_end(&mut raw)?;
                    if base > 0 {
                        match raw.iter().position(|b| *b == b'\n') {
                            Some(end) if end + 1 < raw.len() => { raw.drain(..=end); base += end as u64 + 1; }
                            // A última linha sozinha passa da janela: lê o arquivo todo em vez de nada.
                            _ => {
                                reader.seek(SeekFrom::Start(0))?;
                                raw.clear();
                                reader.take(length).read_to_end(&mut raw)?;
                                base = 0;
                            }
                        }
                    }
                    Ok((String::from_utf8_lossy(&raw).split_inclusive('\n').map(str::to_owned).collect::<Vec<_>>(), base))
                }) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), 0),
                    other => other?,
                };
                (rows, 0, String::new(), base)
            }
        };
        let provider = Provider::parse(&row.provider).filter(|provider| *provider != Provider::Orq).unwrap_or(Provider::Claude);
        let mut parser = LineParser::new(provider);
        let (mut parsed, mut position, mut previous, mut start, mut seen) = (Vec::new(), base, 0., 0., HashSet::new());
        for (index, raw) in rows.iter().enumerate() {
            let offset = position;
            position += raw.len() as u64;
            let Some(value) = pyjson::loads_lossless(raw).filter(Value::is_object) else { continue; };
            let clock = super::timestamp(&value);
            if clock > 0. { previous = clock; if start == 0. { start = clock; } }
            let source = transferred && index < source_len;
            let mut events = if source { crate::transcript::parse_claude_object(value.as_object().unwrap()) }
                else { parser.feed(raw.as_bytes(), offset) };
            events.retain(|event| !(event.id.starts_with("held:") || event.id.starts_with("delivery:")) || seen.insert(event.id.clone()));
            if transferred { for event in &mut events { prefix_event(event, &id, if source {"claude"} else {"codex"}); } }
            for event in events { parsed.push((event.ts.filter(|t| *t != 0.).unwrap_or(previous), event.offset.unwrap_or(offset), event)); }
        }
        for event in parser.flush() { parsed.push((event.ts.unwrap_or(previous), event.offset.unwrap_or(position), event)); }
        // `merged_history(limit=600)` conserva a janela inteira, não só seiscentos eventos.
        let mut span = crate::transcript::TAIL_WINDOW;
        let offset = if transferred { 0 } else { loop {
            let lower = position.saturating_sub(span);
            let mut cursor = base;
            let offset = if lower == 0 {0} else { rows.iter().find_map(|raw| { let old=cursor; cursor+=raw.len() as u64; (old>=lower).then_some(old) }).unwrap_or(position) };
            if offset == 0 || parsed.iter().filter(|(_,pos,_)| *pos >= offset).count() >= 600 { break offset; }
            span = span.saturating_mul(4);
        }};
        let mut items = Vec::new(); let mut committed = HashMap::new();
        for (ts,pos,event) in parsed.into_iter().filter(|(_,pos,_)| *pos >= offset) {
            super::absorb(vec![event], ts, pos, &mut items, &mut committed);
        }
        super::merge_queue(&info, &mut items, &committed, previous, start)?;
        items.sort_by(|a,b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if transferred { retain_latest(&mut items, 600); }
        Ok(BatonWindow { rows, events:items.into_iter().map(|(_,_,event)|event).collect(), since:(start>0.).then_some(start) })
    }).map_err(|cause| BatonReadError {verification,cause})
}

/// Associação da sessão ao histórico importado, resolvida uma vez por lote de leitura.
#[derive(Clone)]
pub(crate) struct LiveContext { home:std::path::PathBuf, name:String, cache:Arc<Mutex<Option<(PathBuf,(u64,u64),Arc<LiveFilter>)>>> }

pub(crate) struct LiveFilter { id:String, offset:u64, imported:HashSet<String> }
impl LiveContext {
    pub(crate) fn new(home:PathBuf,name:String)->Self { Self {home,name,cache:Default::default()} }
    /// Resolvido uma vez por arquivo de rollout: o registro concluído e o prefixo importado não mudam
    /// enquanto o arquivo só cresce. Arquivo substituído ou encolhido refaz. Só o filtro achado fica
    /// guardado: a ausência pode ser o sidecar ou o registro ainda não gravados.
    pub(crate) fn filter(&self,path:&Path)->io::Result<Option<Arc<LiveFilter>>> {
        let meta=std::fs::metadata(path)?;
        let file=file_id(&meta);
        if let Some(file)=file && let Some((cached,id,filter))=&*self.cache.lock().unwrap_or_else(|e|e.into_inner())
            && cached==path && *id==file && meta.len()>=filter.offset {return Ok(Some(filter.clone()));}
        let filter=self.resolve(path)?.map(Arc::new);
        if let (Some(file),Some(found))=(file,&filter) {*self.cache.lock().unwrap_or_else(|e|e.into_inner())=Some((path.to_owned(),file,found.clone()));}
        Ok(filter)
    }
    fn resolve(&self,path:&Path)->io::Result<Option<LiveFilter>> {
        let sidecar=crate::runtime::codex_sidecar::path(&crate::runtime::codex_sidecar::dir(&self.home),&self.name);
        let Some(meta)=crate::runtime::codex_sidecar::read(&sidecar)? else {return Ok(None)};
        let owner=meta.get("codex_home").and_then(Value::as_str).map(Path::new);
        let record=match meta.get("transfer_id").and_then(Value::as_str).filter(|s|!s.is_empty()) {
            Some(id)=>Some(load_record(&self.home,id)?.ok_or_else(||invalid("O registro da transferência desapareceu."))?),
            // Sessão que nunca foi transferida não perde a cauda por um índice ilegível.
            None=>match owner {
                Some(owner)=>indexed_record(&self.home,owner,&crate::locate::session_key(&path.to_string_lossy()),false).unwrap_or_else(|error| {
                    tracing::warn!(session=%self.name,kind=?error.kind(),code="transfer_index_unreadable","índice de transferência ilegível; cauda sem filtro");
                    None
                }),
                None=>None,
            },
        };
        let Some(record)=record else {return Ok(None)};
        let owner=owner.ok_or_else(||invalid("A conta da conversa transferida está indisponível."))?;
        // Uma conversa nova por /clear não herda a associação antiga conservada no sidecar.
        if !matches_rollout(&record,path,owner) {return Ok(None)};
        check_record(&record,path,owner)?;
        let offset=stable_read(path,|reader,length|check_prefix(reader,length,&record["boundary"]))?;
        Ok(Some(LiveFilter {id:required(&record,"id")?.into(),offset,imported:imported_ids(&record["boundary"])?.into_iter().map(str::to_owned).collect()}))
    }
}
#[cfg(unix)]
fn file_id(meta:&std::fs::Metadata)->Option<(u64,u64)> {use std::os::unix::fs::MetadataExt;Some((meta.dev(),meta.ino()))}
#[cfg(not(unix))]
fn file_id(meta:&std::fs::Metadata)->Option<(u64,u64)> {
    meta.created().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d|(d.as_secs(),u64::from(d.subsec_nanos())))
}
impl LiveFilter {
    pub(crate) fn includes(&self,line:&[u8],offset:u64)->io::Result<bool> {
        if offset<self.offset {return Ok(false)};
        let row=object(line,"O rollout ativo contém uma linha inválida.")?;
        Ok(![&row["id"],&row["payload"]["id"]].into_iter().filter_map(Value::as_str).any(|id|self.imported.contains(id)))
    }
    pub(crate) fn decorate(&self,event:&mut ChatEvent) {prefix_event(event,&self.id,"codex");}
}

/// `verify_boundary`: o rollout ainda tem o prefixo importado e a thread da transferência.
pub(crate) fn verify_boundary(record: &Value, path: &Path, owner_home: &Path) -> io::Result<()> {
    check_record(record, path, owner_home)?;
    stable_read(path, |reader, length| check_prefix(reader, length, &record["boundary"]).map(|_| ()))
}

/// `composed_history(..., include_queue=False)` do Arquivo: a origem congelada e só os turnos novos.
pub(crate) fn archived_history(record: &Value, path: &Path, owner_home: &Path) -> io::Result<Vec<ChatEvent>> {
    check_record(record, path, owner_home)?;
    let id = required(record, "id")?;
    let (mut old, previous, _) = source_events(&record["source"], id, usize::MAX)?;
    let (items, _, _) = active_events(path, &record["boundary"], id, previous, usize::MAX, &HashSet::new())?;
    old.extend(items.into_iter().map(|(_, _, event)| event));
    Ok(old)
}

/// `historical_image`: a `index`-ésima imagem base64 da mensagem da origem congelada que o id aponta.
pub(crate) fn historical_image(record: &Value, event_id: &str, index: i64) -> io::Result<Option<(Vec<u8>, String)>> {
    let id = required(record, "id")?;
    let Some(uuid) = event_id.strip_prefix(&format!("transfer:{id}:claude:")) else { return Ok(None) };
    let source = &record["source"];
    let row = stable_read(Path::new(required(source, "path")?), |reader, length| {
        let (metadata, offsets, selected) = selected_chain(reader, length, source)?;
        let Some(&index) = selected.iter().find(|i| metadata[**i]["uuid"] == uuid) else { return Ok(None) };
        reader.seek(SeekFrom::Start(offsets[index]))?;
        let end = offsets.get(index + 1).copied().unwrap_or(length);
        let mut line = Vec::new();
        (&mut *reader).take(end - offsets[index]).read_until(b'\n', &mut line)?;
        object(&line, "O snapshot possui uma linha inválida.").map(Some)
    })?;
    let Some(row) = row else { return Ok(None) };
    let images: Vec<&Value> = row["message"]["content"].as_array().into_iter().flatten()
        .filter(|block| block["type"] == "image").collect();
    let Some(image) = usize::try_from(index).ok().and_then(|i| images.get(i)) else { return Ok(None) };
    let source = &image["source"];
    let Some(data) = source["data"].as_str().filter(|_| source["type"] == "base64") else { return Ok(None) };
    let Ok(raw) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data) else { return Ok(None) };
    Ok(Some((raw, crate::session_media::served_media(source["media_type"].as_str()).to_owned())))
}

#[cfg(test)]
#[path = "transfer_history_tests.rs"]
mod tests;

#[cfg(test)]
mod citation_index_contract {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_indexed_record_is_strict_only_for_citation_authorization() {
        let dir=tempfile::tempdir().unwrap(); let home=dir.path(); let owner=home.join("codex"); std::fs::create_dir(&owner).unwrap();
        let thread="019f99de-9572-7221-89c0-80c62f883d44";
        let key=format!("[{}, {}]",json!(crate::accounts::catalog::resolved(&owner)),json!(thread));
        let indexes=home.join(".hangar/conversation-transfers/indexes"); std::fs::create_dir_all(&indexes).unwrap();
        std::fs::write(indexes.join(format!("thread-{}.json",hex_digest(ring::digest::digest(&ring::digest::SHA256,key.as_bytes())))),json!(["a563a017-af37-42e8-a8e5-2bd837584f8e"]).to_string()).unwrap();
        assert!(archive_record(home,&owner,thread).unwrap().is_none(),"Arquivo mantém a seleção anterior que pula ausência");
        let row:SessionRow=serde_json::from_value(json!({"name":"resumed","provider":"codex","codex_home":owner,
            "jsonl":home.join(format!("rollout-fixture-{thread}.jsonl"))})).unwrap();
        assert_eq!(citation_rows(&row,home).unwrap_err().kind(),io::ErrorKind::InvalidData);
    }

    #[test]
    fn unreadable_index_leaves_a_never_transferred_tail_unfiltered() {
        let dir=tempfile::tempdir().unwrap(); let home=dir.path(); let owner=home.join("codex"); std::fs::create_dir(&owner).unwrap();
        let thread="019f99de-9572-7221-89c0-80c62f883d44";
        let path=owner.join(format!("rollout-fixture-{thread}.jsonl")); std::fs::write(&path,b"{}\n").unwrap();
        assert_eq!(crate::locate::session_key(&path.to_string_lossy()),thread);
        let key=format!("[{}, {}]",json!(crate::accounts::catalog::resolved(&owner)),json!(thread));
        let indexes=home.join(".hangar/conversation-transfers/indexes"); std::fs::create_dir_all(&indexes).unwrap();
        std::fs::write(indexes.join(format!("thread-{}.json",hex_digest(ring::digest::digest(&ring::digest::SHA256,key.as_bytes())))),b"{").unwrap();
        assert!(indexed_record(home,&owner,thread,false).is_err());
        crate::runtime::codex_sidecar::put(&crate::runtime::codex_sidecar::dir(home),"session",json!({"name":"session","codex_home":owner}).as_object().unwrap()).unwrap();
        assert!(LiveContext::new(home.into(),"session".into()).filter(&path).unwrap().is_none());
    }
}
