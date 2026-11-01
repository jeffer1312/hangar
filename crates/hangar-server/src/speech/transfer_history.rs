//! Leitura da origem congelada e dos turnos novos de uma conversa transferida, sem importar duas vezes.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
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
    let imported: HashSet<_> = boundary["imported_item_ids"]
        .as_array()
        .ok_or_else(|| invalid("A fronteira perdeu os identificadores importados."))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid("A fronteira possui um identificador inválido."))
        })
        .collect::<Result<_, _>>()?;
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
        let Some(record) = load_record(home, id)? else { continue };
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
    let matches = boundary["rollout_path"].as_str().is_some_and(|r| same(path, Path::new(r)))
        || (record["destination_meta"]["codex_home"].as_str().is_some_and(|h| same(owner_home, Path::new(h)))
            && boundary["thread_id"].as_str() == Some(crate::locate::session_key(&path.to_string_lossy()).as_str()));
    if !matches {
        return Err(invalid("O rollout não pertence à transferência."));
    }
    Ok(())
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
