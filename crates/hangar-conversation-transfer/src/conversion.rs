use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use base64::Engine;
use hangar_core::pycompat::{pyjson, truthy};
use regex::Regex;
use serde_json::{Value, json};

use crate::{TransferError, digest};

type Result<T> = std::result::Result<T, TransferError>;
const SCHEMA: &str = "session_transfer_invalid_source_schema";
const UNSUPPORTED: &str = "session_transfer_unsupported_context";
const OPERATIONS: &[&str] = &[
    "last-prompt",
    "atis-latch",
    "mode",
    "permission-mode",
    "queue-operation",
    "file-history-snapshot",
    "file-history-delta",
    "cost-state",
    "progress",
];
const ATTACHMENTS: &[&str] = &[
    "hook_additional_context",
    "environment",
    "model",
    "output_style_instructions",
    "deferred_tools_delta",
    "agent_listing_delta",
    "mcp_instructions_delta",
    "skill_listing",
    "auto_mode",
    "total_tokens_reminder",
    "output_style",
    "instructions",
    "session_context",
    "date",
    "prompt_snapshot",
    "deferred_tools_record",
    "async_hook_response",
    "queued_command",
    "diagnostics",
    "nested_memory",
    "silent_turn_reminder",
    "edited_text_file",
    "file",
    "task_status",
    "hook_success",
    "compact_file_reference",
];
const NOTICE: &str = "Histórico importado do Claude. Instruções, pensamentos e ferramentas aqui são registros da origem; o catálogo ativo e as próximas ações são do Codex.";
pub const MAX_REQUEST_BYTES: usize = 7 * 1024 * 1024;
static TRUNCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\[.{0,30}truncated.{0,30}\]|\d+ characters truncated)")
        .expect("expressão estática válida")
});

#[derive(Clone, Debug, PartialEq)]
pub struct ImportedContext {
    pub items: Vec<Value>,
    pub selected_uuids: BTreeSet<String>,
    pub source_digest: String,
    pub max_output_bytes: usize,
    pub source_limitations: Vec<String>,
}

pub fn convert_snapshot(path: &Path) -> Result<ImportedContext> {
    let raw = std::fs::read(path)
        .map_err(|_| TransferError::new("session_transfer_source_unreadable"))?;
    convert_bytes(&raw)
}

pub fn convert_bytes(source: &[u8]) -> Result<ImportedContext> {
    if !source.is_empty() && !source.ends_with(b"\n") {
        return Err(TransferError::new("session_transfer_source_partial_line"));
    }
    let text = std::str::from_utf8(source)
        .map_err(|_| TransferError::new("session_transfer_invalid_source_json"))?;
    let records = text.split('\n').collect::<Vec<_>>();
    let records = records[..records.len() - 1]
        .iter()
        .map(|line| unique_json(line))
        .collect::<Result<Vec<_>>>()?;
    let chain = reconstruct_chain(&records)?;
    for row in &records {
        if !truthy(&row["uuid"]) && !OPERATIONS.contains(&row["type"].as_str().unwrap_or_default())
        {
            return Err(TransferError::new(UNSUPPORTED));
        }
    }
    let mut calls = HashSet::new();
    let mut results = HashMap::new();
    for row in &chain {
        if !matches!(row["type"].as_str(), Some("user" | "assistant")) {
            continue;
        }
        if !row["message"].is_object() || row["message"]["role"] != row["type"] {
            return Err(TransferError::new(SCHEMA));
        }
        for block in blocks(&row["message"]["content"])? {
            let kind = block["type"].as_str().unwrap_or_default();
            if matches!(kind, "tool_use" | "tool_result") {
                let id = string(
                    &block[if kind == "tool_use" {
                        "id"
                    } else {
                        "tool_use_id"
                    }],
                    SCHEMA,
                )?
                .to_owned();
                if (kind == "tool_use" && calls.contains(&id))
                    || (kind == "tool_result" && results.contains_key(&id))
                {
                    return Err(TransferError::new(
                        "session_transfer_source_tool_id_ambiguous",
                    ));
                }
                if kind == "tool_use" {
                    calls.insert(id);
                } else {
                    results.insert(id.clone(), calls.contains(&id));
                }
            }
        }
    }
    let mut items = Vec::new();
    let mut limitations = Vec::new();
    let mut max_output_bytes = 0;
    for row in &chain {
        let kind = string(&row["type"], SCHEMA)?;
        if OPERATIONS.contains(&kind) {
            continue;
        }
        if kind == "system" {
            let subtype = string(&row["subtype"], SCHEMA)?;
            if matches!(subtype, "compact_boundary" | "turn_duration") {
                if subtype == "turn_duration"
                    && (row.get("content").is_some() || row.get("message").is_some())
                {
                    return Err(TransferError::new(UNSUPPORTED));
                }
                continue;
            }
        }
        if matches!(kind, "attachment" | "system") {
            let payload = if kind == "attachment" {
                &row["attachment"]
            } else {
                row
            };
            if !payload.is_object() {
                return Err(TransferError::new(SCHEMA));
            }
            let subtype = string(
                &payload[if kind == "attachment" {
                    "type"
                } else {
                    "subtype"
                }],
                SCHEMA,
            )?;
            if kind == "attachment" && subtype == "credential_org" {
                continue;
            }
            if (kind == "attachment" && !ATTACHMENTS.contains(&subtype))
                || (kind == "system" && subtype != "stop_hook_summary")
            {
                return Err(TransferError::new(UNSUPPORTED));
            }
            let content = if kind == "attachment" {
                attachment(payload)?
            } else {
                vec![history(subtype, payload)]
            };
            items.push(message("user", content, row, 0));
            continue;
        }
        if !matches!(kind, "user" | "assistant") {
            return Err(TransferError::new(UNSUPPORTED));
        }
        let text_type = if kind == "user" {
            "input_text"
        } else {
            "output_text"
        };
        let mut pending = if truthy(&row["isMeta"]) {
            vec![json!({"type":text_type,"text":"[Histórico Claude: mensagem isMeta]"})]
        } else {
            Vec::new()
        };
        let content = blocks(&row["message"]["content"])?;
        for (index, block) in content.iter().enumerate() {
            let block_type = block["type"].as_str().unwrap_or_default();
            if block_type == "text" && block["text"].is_string() {
                pending.push(json!({"type":text_type,"text":block["text"]}));
                continue;
            }
            if !pending.is_empty() {
                items.push(message(kind, std::mem::take(&mut pending), row, index));
            }
            match block_type {
                "tool_use" => {
                    let id = string(&block["id"], SCHEMA)?;
                    let name = string(&block["name"], SCHEMA)?;
                    if !block["input"].is_object() {
                        return Err(TransferError::new(SCHEMA));
                    }
                    if results.get(id) == Some(&true) {
                        items.push(json!({"type":"function_call","call_id":id,"name":name,"arguments":pyjson::dumps(&block["input"],false)}));
                    } else {
                        items.push(message(
                            "user",
                            vec![history(
                                "chamada sem resultado gravado; resultado desconhecido",
                                block,
                            )],
                            row,
                            index,
                        ));
                    }
                }
                "tool_result" => {
                    let id = string(&block["tool_use_id"], SCHEMA)?;
                    let output = tool_output(block)?;
                    let recorded = output.as_str().map(str::to_owned).unwrap_or_else(|| {
                        output
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|p| p["text"].as_str().unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                    if truthy(&block["isTruncated"])
                        || truthy(&block["truncated"])
                        || TRUNCATION.is_match(&recorded)
                    {
                        limitations.push(format!(
                            "Claude {}/{id}: saída com indicação de corte na fonte",
                            row["uuid"].as_str().unwrap_or_default()
                        ));
                    }
                    max_output_bytes = max_output_bytes.max(
                        output
                            .as_str()
                            .map(str::len)
                            .unwrap_or_else(|| pyjson::dumps(&output, false).len()),
                    );
                    if results.get(id) == Some(&true) {
                        items.push(
                            json!({"type":"function_call_output","call_id":id,"output":output}),
                        );
                    } else {
                        let mut parts = vec![
                            json!({"type":"input_text","text":format!("[Histórico Claude: resultado órfão {id}]")}),
                        ];
                        if output.is_string() {
                            parts.push(json!({"type":"input_text","text":output}));
                        } else if let Some(a) = output.as_array() {
                            parts.extend(a.clone());
                        }
                        items.push(message("user", parts, row, index));
                    }
                }
                "thinking" if block["thinking"].is_string() => items.push(message(
                    "user",
                    vec![history("pensamento textual, não raciocínio nativo", block)],
                    row,
                    index,
                )),
                "redacted_thinking" => {
                    string(&block["data"], SCHEMA)?;
                    let limitation = format!(
                        "Claude {}: pensamento redigido/cifrado não recuperável",
                        row["uuid"].as_str().unwrap_or_default()
                    );
                    items.push(message(
                        "user",
                        vec![history(&limitation, block)],
                        row,
                        index,
                    ));
                    limitations.push(limitation);
                }
                "image" => items.push(message("user", vec![image(block)?], row, index)),
                _ => return Err(TransferError::new(UNSUPPORTED)),
            }
        }
        if !pending.is_empty() {
            items.push(message(kind, pending, row, content.len()));
        }
    }
    if items.is_empty() {
        return Err(TransferError::new("session_transfer_no_source_context"));
    }
    if items[0]["type"] == "message" {
        let kind = if items[0]["role"] == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        items[0]["content"]
            .as_array_mut()
            .expect("mensagem construída com conteúdo")
            .push(json!({"type":kind,"text":NOTICE}));
        let hash = digest(pyjson::dumps(&items[0]["content"], false).as_bytes());
        let previous = items[0]["id"]
            .as_str()
            .unwrap_or_default()
            .rsplit_once('_')
            .map(|(p, _)| p)
            .unwrap_or_default();
        items[0]["id"] = Value::String(format!("{previous}_{}", &hash[..16]));
    } else {
        items.insert(
            0,
            message(
                "user",
                vec![json!({"type":"input_text","text":NOTICE})],
                chain[0],
                0,
            ),
        );
    }
    Ok(ImportedContext {
        items,
        selected_uuids: chain
            .iter()
            .map(|r| r["uuid"].as_str().unwrap_or_default().to_owned())
            .collect(),
        source_digest: digest(source),
        max_output_bytes,
        source_limitations: limitations,
    })
}

pub fn reconstruct_chain(records: &[Value]) -> Result<Vec<&Value>> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, row) in records.iter().enumerate() {
        if !row.is_object() {
            return Err(TransferError::new(SCHEMA));
        }
        string(&row["type"], SCHEMA)?;
        for flag in ["isSidechain", "isCompactSummary", "isMeta"] {
            if row.get(flag).is_some_and(|v| !v.is_boolean()) {
                return Err(TransferError::new(SCHEMA));
            }
        }
        if !row["uuid"].is_null() {
            positions
                .entry(string(&row["uuid"], SCHEMA)?)
                .or_default()
                .push(i);
        }
    }
    let explicit = records
        .iter()
        .enumerate()
        .rev()
        .find(|(_, r)| r["type"] == "last-prompt" && !truthy(&r["isSidechain"]));
    let mut leaf = if let Some((index, row)) = explicit {
        let reference = string(&row["leafUuid"], "session_transfer_source_leaf_invalid")?;
        positions
            .get(reference)
            .and_then(|v| v.iter().rev().find(|&&i| i < index))
            .copied()
            .ok_or_else(|| TransferError::new("session_transfer_source_leaf_invalid"))?
    } else {
        records
            .iter()
            .rposition(|r| {
                matches!(r["type"].as_str(), Some("user" | "assistant"))
                    && !truthy(&r["isSidechain"])
            })
            .ok_or_else(|| TransferError::new("session_transfer_no_source_leaf"))?
    };
    let mut indices = Vec::new();
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(leaf) {
            return Err(TransferError::new("session_transfer_source_cycle"));
        }
        let row = &records[leaf];
        if truthy(&row["isSidechain"]) || row.get("parentUuid").is_none() {
            return Err(TransferError::new(
                "session_transfer_unsupported_source_topology",
            ));
        }
        string(&row["uuid"], SCHEMA)?;
        indices.push(leaf);
        let mut parent = &row["parentUuid"];
        if parent.is_null() && row["subtype"] == "compact_boundary" {
            string(
                &row["logicalParentUuid"],
                "session_transfer_source_parent_missing",
            )?;
            parent = &row["logicalParentUuid"];
        } else if !row["logicalParentUuid"].is_null() && row["subtype"] != "compact_boundary" {
            return Err(TransferError::new(
                "session_transfer_unsupported_source_topology",
            ));
        }
        if parent.is_null() {
            break;
        }
        let versions = positions.get(string(parent, SCHEMA)?);
        leaf = versions
            .and_then(|v| v.iter().rev().find(|&&i| i < leaf))
            .copied()
            .ok_or_else(|| {
                TransferError::new(
                    if versions.is_some_and(|v| v.iter().any(|i| seen.contains(i))) {
                        "session_transfer_source_cycle"
                    } else {
                        "session_transfer_source_parent_missing"
                    },
                )
            })?;
    }
    let chain: Vec<_> = indices
        .into_iter()
        .rev()
        .map(|i| &records[i])
        .filter(|r| !truthy(&r["isCompactSummary"]))
        .collect();
    if !chain
        .iter()
        .any(|r| matches!(r["type"].as_str(), Some("user" | "assistant")))
    {
        return Err(TransferError::new("session_transfer_no_source_leaf"));
    }
    Ok(chain)
}

pub fn serialized_batches(items: &[Value], max_request_bytes: usize) -> Result<Vec<Vec<Value>>> {
    if max_request_bytes == 0 {
        return Err(TransferError::new(
            "session_transfer_invalid_transport_limit",
        ));
    }
    let limit = max_request_bytes.min(MAX_REQUEST_BYTES);
    let envelope = json!({"jsonrpc":"2.0","id":u64::MAX,"method":"thread/inject_items","params":{"threadId":"f".repeat(36),"items":[]}});
    let overhead = pyjson::dumps(&envelope, false).len() + 1;
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut size = overhead;
    for item in items {
        let item_size = pyjson::dumps(item, false).len();
        if overhead + item_size > limit {
            return Err(TransferError::new("session_transfer_source_item_too_large"));
        }
        if size + item_size + if batch.is_empty() { 0 } else { 2 } > limit {
            batches.push(std::mem::take(&mut batch));
            size = overhead;
        }
        size += item_size + if batch.is_empty() { 0 } else { 2 };
        batch.push(item.clone());
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

fn string<'a>(value: &'a Value, code: &str) -> Result<&'a str> {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TransferError::new(code))
}
fn message(role: &str, content: Vec<Value>, row: &Value, index: usize) -> Value {
    let hash = digest(pyjson::dumps(&Value::Array(content.clone()), false).as_bytes());
    json!({"type":"message","id":format!("claude_{}_{index}_{}",row["uuid"].as_str().unwrap_or_default(),&hash[..16]),"role":role,"content":content})
}
fn history(label: &str, value: &Value) -> Value {
    json!({"type":"input_text","text":format!("[Histórico Claude: {label}]\n{}",pyjson::dumps(value,false))})
}
fn blocks(value: &Value) -> Result<Vec<Value>> {
    if let Some(s) = value.as_str() {
        return Ok(vec![json!({"type":"text","text":s})]);
    }
    let values = value.as_array().ok_or_else(|| TransferError::new(SCHEMA))?;
    for block in values {
        if !block.is_object() {
            return Err(TransferError::new(SCHEMA));
        }
        string(&block["type"], SCHEMA)?;
    }
    Ok(values.clone())
}
fn image(block: &Value) -> Result<Value> {
    let source = &block["source"];
    if !source.is_object() {
        return Err(TransferError::new("session_transfer_invalid_media_schema"));
    }
    if source["type"] != "base64" {
        return Err(TransferError::new("session_transfer_source_media_missing"));
    }
    let mime = source["media_type"]
        .as_str()
        .filter(|m| matches!(*m, "image/png" | "image/jpeg" | "image/gif" | "image/webp"))
        .ok_or_else(|| TransferError::new("session_transfer_unsupported_media_mime"))?;
    let data = string(&source["data"], "session_transfer_source_media_missing")?;
    let padding_start = data.find('=').unwrap_or(data.len());
    let decoded_data = if padding_start > 0
        && padding_start % 4 == 0
        && data[padding_start..].bytes().all(|b| b == b'=')
    {
        &data[..padding_start]
    } else {
        data
    };
    let decoder = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    let bytes = decoder
        .decode(decoded_data)
        .map_err(|_| TransferError::new("session_transfer_invalid_media_schema"))?;
    if bytes.is_empty() {
        return Err(TransferError::new("session_transfer_source_media_missing"));
    }
    Ok(json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")}))
}
fn tool_output(block: &Value) -> Result<Value> {
    let mut output = if block["content"].is_string() {
        block["content"].clone()
    } else {
        let mut parts = Vec::new();
        for part in blocks(&block["content"])? {
            if part["type"] == "text" && part["text"].is_string() {
                parts.push(json!({"type":"input_text","text":part["text"]}));
            } else if part["type"] == "image" {
                parts.push(image(&part)?);
            } else {
                return Err(TransferError::new(UNSUPPORTED));
            }
        }
        Value::Array(parts)
    };
    if block.get("is_error").is_some_and(|v| !v.is_boolean()) {
        return Err(TransferError::new(SCHEMA));
    }
    if truthy(&block["is_error"]) {
        let marker = "[Claude tool_result: is_error=true]\n";
        if let Some(s) = output.as_str() {
            output = Value::String(format!("{marker}{s}"));
        } else if let Some(parts) = output.as_array_mut() {
            parts.insert(0, json!({"type":"input_text","text":marker}));
        }
    }
    Ok(output)
}
fn attachment(payload: &Value) -> Result<Vec<Value>> {
    let kind = payload["type"].as_str().unwrap_or_default();
    let missing = || TransferError::new("session_transfer_source_media_missing");
    match kind {
        "compact_file_reference" => {
            string(&payload["filename"], SCHEMA)?;
            string(&payload["displayPath"], SCHEMA)?;
        }
        "instructions" => {
            let files = payload["files"]
                .as_array()
                .ok_or_else(|| TransferError::new(SCHEMA))?;
            if files
                .iter()
                .any(|f| !f.is_object() || !f["content"].is_string())
            {
                return Err(missing());
            }
        }
        "nested_memory" => {
            let c = &payload["content"];
            if !(c.is_string() || c.is_object() && c["content"].is_string()) {
                return Err(missing());
            }
        }
        "file" => {
            let c = &payload["content"];
            if c.is_object() {
                if c["type"] == "image" {
                    let mut metadata = payload.clone();
                    metadata
                        .as_object_mut()
                        .expect("objeto validado")
                        .remove("content");
                    return Ok(vec![
                        history("arquivo/imagem gravado", &metadata),
                        image(c)?,
                    ]);
                }
                if c["type"] != "text" {
                    return Err(TransferError::new(UNSUPPORTED));
                }
                if !c["file"].is_object() || !c["file"]["content"].is_string() {
                    return Err(missing());
                }
            } else if !c.is_string() {
                return Err(missing());
            }
        }
        _ => {}
    }
    Ok(vec![history(kind, payload)])
}

// A leitura em RawValue conserva inteiros arbitrários e permite recusar chaves repetidas antes
// de construir Value, que por si só conservaria apenas a última ocorrência.
fn unique_json(text: &str) -> Result<Value> {
    fn validate(raw: &serde_json::value::RawValue) -> std::result::Result<(), String> {
        let text = raw.get().trim_start();
        if text.starts_with('{') {
            struct Fields;
            impl<'de> serde::de::Visitor<'de> for Fields {
                type Value = ();
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("objeto sem chaves repetidas")
                }
                fn visit_map<M: serde::de::MapAccess<'de>>(
                    self,
                    mut map: M,
                ) -> std::result::Result<(), M::Error> {
                    let mut keys = HashSet::new();
                    while let Some((key, value)) =
                        map.next_entry::<String, Box<serde_json::value::RawValue>>()?
                    {
                        if !keys.insert(key) {
                            return Err(serde::de::Error::custom("chave repetida"));
                        }
                        validate(&value).map_err(serde::de::Error::custom)?;
                    }
                    Ok(())
                }
            }
            let mut de = serde_json::Deserializer::from_str(text);
            serde::de::Deserializer::deserialize_map(&mut de, Fields).map_err(|e| e.to_string())?;
        } else if text.starts_with('[') {
            let values: Vec<Box<serde_json::value::RawValue>> =
                serde_json::from_str(text).map_err(|e| e.to_string())?;
            for v in values {
                validate(&v)?;
            }
        }
        Ok(())
    }
    let error = || TransferError::new("session_transfer_invalid_source_json");
    let raw: Box<serde_json::value::RawValue> = serde_json::from_str(text).map_err(|_| error())?;
    validate(&raw).map_err(|_| error())?;
    pyjson::loads_lossless(text).ok_or_else(error)
}
