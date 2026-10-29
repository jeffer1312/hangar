//! Leitura da origem congelada e dos turnos novos de uma conversa transferida, sem importar duas vezes.
use std::{
    collections::{HashMap, HashSet},
    io,
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
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn signature(
    metadata: &std::fs::Metadata,
) -> (
    u64,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
    u64,
    i64,
    i64,
) {
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
fn snapshot(path: &Path) -> io::Result<Vec<u8>> {
    let before = signature(&std::fs::metadata(path)?);
    let bytes = std::fs::read(path)?;
    if signature(&std::fs::metadata(path)?) != before {
        return Err(invalid("A fonte da transferência mudou durante a leitura."));
    }
    Ok(bytes)
}

pub(super) fn read(
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
    let source_bytes = snapshot(Path::new(required(source, "path")?))?;
    if digest(&source_bytes) != required(source, "digest")? || !source_bytes.ends_with(b"\n") {
        return Err(invalid(
            "O snapshot da origem da transferência mudou ou contém uma linha incompleta.",
        ));
    }
    let rows = rows(&source_bytes)?;
    let selected = chain(&rows)?;
    let selected_ids: HashSet<_> = selected
        .iter()
        .filter_map(|row| row["uuid"].as_str())
        .collect();
    let expected_ids: HashSet<_> = source["selected_uuids"]
        .as_array()
        .ok_or_else(|| invalid("O ramo da origem está inválido."))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid("O ramo da origem está inválido."))
        })
        .collect::<Result<_, _>>()?;
    if selected_ids != expected_ids {
        return Err(invalid(
            "O ramo da origem diverge do snapshot da transferência.",
        ));
    }
    let active_bytes = snapshot(path)?;
    let offset = boundary["min_offset"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0 && *n <= active_bytes.len())
        .ok_or_else(|| invalid("A fronteira da transferência está inválida."))?;
    let prefix = &active_bytes[..offset];
    if !prefix.ends_with(b"\n") || digest(prefix) != required(boundary, "prefix_digest")? {
        return Err(invalid("O prefixo importado do rollout mudou."));
    }
    let header: Value = serde_json::from_slice(
        prefix
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    )
    .map_err(|_| invalid("O rollout perdeu a identidade da thread."))?;
    if header["type"] != "session_meta"
        || header["payload"]
            .get("id")
            .or_else(|| header["payload"].get("session_id"))
            .and_then(Value::as_str)
            != Some(thread)
    {
        return Err(invalid("A thread do rollout diverge da transferência."));
    }
    let mut old = Vec::new();
    let mut previous = 0.;
    let mut start = 0.;
    let mut parser = LineParser::new(Provider::Claude);
    for row in selected {
        let clock = super::timestamp(row);
        if clock != 0. {
            previous = clock;
        }
        if start == 0. {
            start = previous;
        }
        for mut event in parser.feed(row.to_string().as_bytes(), 0) {
            event.ts = Some(event.ts.filter(|t| *t != 0.).unwrap_or(previous));
            prefix_event(&mut event, id, "claude");
            old.push(event);
        }
    }
    let imported: HashSet<_> = boundary["imported_item_ids"]
        .as_array()
        .ok_or_else(|| invalid("A fronteira perdeu os identificadores importados."))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid("A fronteira possui um identificador inválido."))
        })
        .collect::<Result<_, _>>()?;
    let mut items = Vec::new();
    let mut committed = HashMap::new();
    let mut parser = LineParser::new(Provider::Codex);
    let mut position = offset;
    for line in active_bytes[offset..].split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        let text = std::str::from_utf8(line)
            .map_err(|_| invalid("O rollout ativo contém uma linha inválida."))?;
        let row = pyjson::loads_lossless(text)
            .filter(Value::is_object)
            .ok_or_else(|| invalid("O rollout ativo contém uma linha inválida."))?;
        let imported_row = [&row["id"], &row["payload"]["id"]]
            .into_iter()
            .filter_map(Value::as_str)
            .any(|id| imported.contains(id));
        let current = position;
        position += line.len();
        if imported_row {
            continue;
        }
        let clock = super::timestamp(&row);
        if clock != 0. {
            previous = clock;
        }
        let mut events = parser.feed(line, current as u64);
        for event in &mut events {
            event.ts = Some(event.ts.filter(|t| *t != 0.).unwrap_or(previous));
            prefix_event(event, id, "codex");
        }
        super::absorb(events, previous, current as u64, &mut items, &mut committed);
    }
    super::merge_queue(info, &mut items, &committed, previous, start)?;
    items.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    old.extend(items.into_iter().map(|(_, _, event)| event));
    if old.len() > limit {
        old.drain(..old.len() - limit);
    }
    Ok(Some(old))
}

fn prefix_event(event: &mut ChatEvent, id: &str, provider: &str) {
    event.id = format!("transfer:{id}:{provider}:{}", event.id);
    if let Some(tool) = &mut event.tool_use_id {
        *tool = format!("transfer:{id}:{provider}:{tool}");
    }
}

fn rows(bytes: &[u8]) -> io::Result<Vec<Value>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("O snapshot da origem está ilegível."))?;
    text.split_terminator('\n')
        .map(|line| {
            pyjson::loads_lossless(line)
                .filter(Value::is_object)
                .ok_or_else(|| invalid("O snapshot possui uma linha inválida."))
        })
        .collect()
}

fn chain(rows: &[Value]) -> io::Result<Vec<&Value>> {
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
        selected.push(row);
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
    selected.retain(|row| row["isCompactSummary"] != true);
    if !selected
        .iter()
        .any(|row| matches!(row["type"].as_str(), Some("user" | "assistant")))
    {
        return Err(invalid("A origem não possui conversa."));
    }
    Ok(selected)
}
