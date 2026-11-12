//! Importação de histórico sem iniciar um turno ou publicar a sessão.
#![forbid(unsafe_code)]

use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    path::Path,
    time::Duration,
};

use hangar_conversation_transfer::{ImportBoundary, ImportedContext, TransferError};
use hangar_core::pycompat::pyjson;
use serde_json::{Value, json};

pub const SUPPORTED_VERSION: &str = "0.159.3";
const MAX_REQUEST_BYTES: usize = 7 * 1024 * 1024;
const IMPORT_NOTICE: &str = "Esta conversa contém histórico importado do Claude. Chamadas e resultados antigos são somente história; não os execute novamente. Use as ferramentas nativas do Codex para as próximas ações.";

pub trait NativeImport {
    fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, TransferError>> + Send;
    fn raw_model(
        &mut self,
        config: &Value,
        model: &str,
    ) -> impl Future<Output = Result<Value, TransferError>> + Send;
    fn save_destination(
        &mut self,
        destination: Value,
        boundary: Option<ImportBoundary>,
    ) -> impl Future<Output = Result<(), TransferError>> + Send;
}

pub struct ImportOptions {
    pub cwd: String,
    pub account_home: String,
    pub account_id: String,
    pub transfer_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: String,
    pub collaboration_mode: String,
}

pub struct PreparedCodexThread {
    pub thread_id: String,
    pub rollout_path: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode: String,
    pub tool_output_token_limit: usize,
    pub boundary: ImportBoundary,
}

struct ModelLimits {
    usable: u64,
    compact: u64,
    context: u64,
    modalities: Vec<String>,
}

pub fn import_output_limit(context: &ImportedContext) -> usize {
    context
        .items
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .map(|item| bytes(&item["output"]))
        .fold(context.max_output_bytes.max(1), usize::max)
}

pub async fn prepare_import(
    client: &mut impl NativeImport,
    options: &ImportOptions,
    context: &ImportedContext,
) -> Result<PreparedCodexThread, TransferError> {
    if !matches!(options.collaboration_mode.as_str(), "default" | "plan") {
        return Err(error("session_transfer_invalid_mode"));
    }
    let (approval, sandbox) = match options.permission_mode.as_str() {
        "Ask for approval" => ("on-request", "read-only"),
        "Approve for me" => ("on-request", "workspace-write"),
        "Full Access" => ("never", "danger-full-access"),
        _ => return Err(error("session_transfer_invalid_permission_mode")),
    };
    let mut items = context.items.clone();
    let mut ids = HashSet::new();
    for (index, item) in items.iter_mut().enumerate() {
        if item
            .get("id")
            .is_none_or(|value| value.as_str().is_none_or(str::is_empty))
        {
            item["id"] = json!(format!(
                "import_{}",
                digest(format!("{}:{index}", options.transfer_id).as_bytes())
            ));
        }
        if !ids.insert(item["id"].clone().to_string()) {
            return Err(error("session_transfer_invalid_source_schema"));
        }
    }
    let config = client
        .request(
            "config/read",
            json!({"cwd": options.cwd, "includeLayers": false}),
        )
        .await?["config"]
        .clone();
    if !config.is_object() {
        return Err(unknown());
    }
    let instructions = match config["developer_instructions"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        Some(text) => format!("{text}\n\n{IMPORT_NOTICE}"),
        None => IMPORT_NOTICE.to_owned(),
    };
    // O processo já nasce na pasta correta; cwd no RPC pode promover confiança.
    let mut params = json!({"approvalPolicy": approval, "sandbox": sandbox, "developerInstructions": instructions});
    if let Some(model) = &options.model {
        params["model"] = json!(model);
    }
    let started = client.request("thread/start", params).await?;
    let thread_id = nonempty(
        &started["thread"]["id"],
        "session_transfer_invalid_native_thread",
    )?
    .to_owned();
    let rollout = nonempty(
        &started["thread"]["path"],
        "session_transfer_invalid_native_thread",
    )?
    .to_owned();
    let budget = import_output_limit(context);
    let mut destination = json!({"thread_id":thread_id,"rollout_path":rollout,"codex_home":options.account_home,
        "codex_account":options.account_id,"tool_output_token_limit":budget,"cwd":options.cwd,
        "permission_mode":options.permission_mode,"transfer_id":options.transfer_id});
    // Grava antes do próximo RPC para que a recuperação encontre a thread órfã.
    client.save_destination(destination.clone(), None).await?;
    let cwd = tokio::fs::canonicalize(&options.cwd)
        .await
        .map_err(|_| mismatch())?;
    for value in [&started["cwd"], &started["thread"]["cwd"]] {
        let path = value.as_str().ok_or_else(mismatch)?;
        if tokio::fs::canonicalize(path)
            .await
            .map_err(|_| mismatch())?
            != cwd
        {
            return Err(mismatch());
        }
    }
    let confirmed = client
        .request(
            "config/read",
            json!({"cwd":options.cwd,"includeLayers":false}),
        )
        .await?;
    if confirmed["config"] != config {
        return Err(error("session_transfer_native_config_changed"));
    }
    let model = nonempty(
        &started["model"],
        "session_transfer_native_settings_mismatch",
    )?
    .to_owned();
    if options
        .model
        .as_ref()
        .is_some_and(|wanted| wanted != &model)
        || started["modelProvider"]
            != json!(
                config["model_provider"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("openai")
            )
    {
        return Err(mismatch());
    }
    if let Some(effort) = &options.effort {
        client
            .request(
                "thread/settings/update",
                json!({"threadId":thread_id,"effort":effort}),
            )
            .await?;
    }
    let resolved = client
        .request(
            "thread/read",
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await?;
    let effort = resolved["thread"]["reasoningEffort"]
        .as_str()
        .map(str::to_owned);
    if resolved["thread"]["model"] != model
        || options
            .effort
            .as_ref()
            .is_some_and(|wanted| Some(wanted) != effort.as_ref())
    {
        return Err(mismatch());
    }
    if options.collaboration_mode == "plan" {
        client.request("thread/settings/update", json!({"threadId":thread_id,"collaborationMode":{
            "mode":"plan","settings":{"model":model,"reasoning_effort":effort,"developer_instructions":null}}})).await?;
    }
    let raw = client.raw_model(&config, &model).await?;
    let limits = resolve_limits(&raw, &config)?;
    let instructions =
        instruction_bytes(client, &started, &config, &raw, &options.cwd, &thread_id).await?;
    check_budget(&items, &limits, instructions)?;
    inject(client, &thread_id, &items).await?;
    let resolved = client
        .request(
            "thread/read",
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await?;
    let thread = &resolved["thread"];
    if thread["id"] != thread_id
        || thread["path"] != rollout
        || thread["model"] != model
        || thread["reasoningEffort"] != json!(effort)
    {
        return Err(mismatch());
    }
    let boundary = wait_persisted(Path::new(&rollout), &thread_id, &items).await?;
    destination["model"] = json!(model);
    destination["effort"] = json!(effort);
    client
        .save_destination(destination, Some(boundary.clone()))
        .await?;
    Ok(PreparedCodexThread {
        thread_id,
        rollout_path: rollout,
        model,
        effort,
        mode: options.collaboration_mode.clone(),
        tool_output_token_limit: budget,
        boundary,
    })
}

fn error(code: &'static str) -> TransferError {
    TransferError::new(code)
}
fn unknown() -> TransferError {
    error("session_transfer_model_capacity_unknown")
}
fn mismatch() -> TransferError {
    error("session_transfer_native_settings_mismatch")
}
fn nonempty<'a>(value: &'a Value, code: &'static str) -> Result<&'a str, TransferError> {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error(code))
}
fn bytes(value: &Value) -> usize {
    pyjson::dumps_unicode(value, false).len()
}
fn digest(value: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, value)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn resolve_limits(raw: &Value, config: &Value) -> Result<ModelLimits, TransferError> {
    fn positive(value: &Value) -> Result<u64, TransferError> {
        value.as_u64().filter(|n| *n > 0).ok_or_else(unknown)
    }
    let mut window = positive(if raw["context_window"].is_null() {
        &raw["max_context_window"]
    } else {
        &raw["context_window"]
    })?;
    let maximum = if raw["max_context_window"].is_null() {
        None
    } else {
        Some(positive(&raw["max_context_window"])?)
    };
    if maximum.is_some_and(|n| window > n) {
        return Err(unknown());
    }
    if !config["model_context_window"].is_null() {
        let configured = positive(&config["model_context_window"])?;
        if maximum.is_none() && configured > window {
            return Err(unknown());
        }
        window = configured.min(maximum.unwrap_or(configured));
    }
    let percent = positive(&raw["effective_context_window_percent"])?;
    if percent >= 100 {
        return Err(unknown());
    }
    let ceiling = ((window as u128) * 9 / 10) as u64;
    let compact_value = if config["model_auto_compact_token_limit"].is_null() {
        &raw["auto_compact_token_limit"]
    } else {
        &config["model_auto_compact_token_limit"]
    };
    let compact = if compact_value.is_null() {
        ceiling
    } else {
        positive(compact_value)?.min(ceiling)
    };
    let modalities = raw["input_modalities"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(unknown)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or_else(unknown))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ModelLimits {
        usable: ((window as u128) * percent as u128 / 100) as u64,
        compact,
        context: window,
        modalities,
    })
}

fn image_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(value["type"] == "input_image")
                + map.values().map(image_count).sum::<usize>()
        }
        Value::Array(values) => values.iter().map(image_count).sum(),
        _ => 0,
    }
}

fn check_budget(
    items: &[Value],
    limits: &ModelLimits,
    instructions: usize,
) -> Result<(), TransferError> {
    let items = Value::Array(items.to_vec());
    let images = image_count(&items);
    if !limits.modalities.iter().any(|m| m == "text")
        || (images > 0 && !limits.modalities.iter().any(|m| m == "image"))
    {
        return Err(error("session_transfer_model_media_unsupported"));
    }
    // Um token por byte e o piso de visão deixam espaço para continuar sem compactar.
    let estimate = bytes(&items) as u128
        + images as u128 * 10000
        + instructions as u128
        + (limits.context - limits.usable) as u128;
    if estimate >= limits.usable.min(limits.compact) as u128 {
        return Err(error("session_transfer_context_budget_exceeded"));
    }
    Ok(())
}

async fn instruction_bytes(
    client: &mut impl NativeImport,
    started: &Value,
    config: &Value,
    raw: &Value,
    cwd: &str,
    thread: &str,
) -> Result<usize, TransferError> {
    let mut paths = started["instructionSources"]
        .as_array()
        .ok_or_else(unknown)?
        .clone();
    let empty = json!({});
    let messages = raw
        .get("model_messages")
        .filter(|v| !v.is_null())
        .unwrap_or(&empty);
    if !messages.is_object() {
        return Err(unknown());
    }
    let template = messages["instructions_template"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| raw["base_instructions"].as_str().filter(|s| !s.is_empty()))
        .ok_or_else(unknown)?;
    let mut size = if messages.as_object().is_some_and(|m| !m.is_empty()) {
        bytes(messages)
    } else {
        bytes(&json!({"base_instructions":template}))
    };
    if config
        .get("model_instructions_file")
        .is_some_and(|v| !v.is_null() && v != "")
    {
        paths.push(config["model_instructions_file"].clone());
    }
    let mut seen_paths = HashSet::new();
    for value in paths {
        let path = value
            .as_str()
            .filter(|p| Path::new(p).is_absolute())
            .ok_or_else(unknown)?;
        if seen_paths.insert(path.to_owned()) {
            size += tokio::fs::read(path).await.map_err(|_| unknown())?.len();
        }
    }
    size += bytes(
        &json!({"instructions":config["instructions"],"developer_instructions":config["developer_instructions"]}),
    ) + IMPORT_NOTICE.len();
    let skills = client
        .request("skills/list", json!({"cwds":[cwd],"forceReload":true}))
        .await?;
    let entries = skills["data"].as_array().ok_or_else(unknown)?;
    if !entries.iter().any(|e| e["cwd"] == cwd) {
        return Err(unknown());
    }
    for entry in entries {
        if !entry.is_object() || truthy(&entry["errors"]) {
            return Err(unknown());
        }
        let skills = entry["skills"].as_array().ok_or_else(unknown)?;
        if skills.iter().any(|v| !v.is_object()) {
            return Err(unknown());
        }
        size += bytes(&Value::Array(
            skills
                .iter()
                .filter(|s| truthy(&s["enabled"]))
                .cloned()
                .collect(),
        ));
    }
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let mut params = json!({"threadId":thread});
        if let Some(value) = cursor {
            params["cursor"] = Value::String(value);
        }
        let inventory = client.request("mcpServerStatus/list", params).await?;
        for server in inventory["data"].as_array().ok_or_else(unknown)? {
            if !server.is_object()
                || truthy(&server["toolsError"])
                || !server["tools"].is_object()
                || !matches!(
                    server["runtimeStatus"].as_str(),
                    Some("connected" | "disabled")
                )
            {
                return Err(unknown());
            }
            size += bytes(&server["tools"]);
        }
        if !truthy(&inventory["nextCursor"]) {
            break;
        }
        let next = inventory["nextCursor"]
            .as_str()
            .ok_or_else(unknown)?
            .to_owned();
        if !seen.insert(next.clone()) {
            return Err(unknown());
        }
        cursor = Some(next);
    }
    Ok(size)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

async fn inject(
    client: &mut impl NativeImport,
    thread: &str,
    items: &[Value],
) -> Result<(), TransferError> {
    let mut pending = VecDeque::from(hangar_conversation_transfer::serialized_batches(
        items,
        MAX_REQUEST_BYTES,
    )?);
    while let Some(batch) = pending.pop_front() {
        let params = json!({"threadId":thread,"items":batch});
        // Reserva o maior id do transporte e confere o envelope com a thread real.
        if pyjson::dumps(
            &json!({"jsonrpc":"2.0","id":u64::MAX,"method":"thread/inject_items","params":params}),
            false,
        )
        .len()
            + 1
            > MAX_REQUEST_BYTES
        {
            if batch.len() <= 1 {
                return Err(error("session_transfer_source_item_too_large"));
            }
            let middle = batch.len() / 2;
            pending.push_front(batch[middle..].to_vec());
            pending.push_front(batch[..middle].to_vec());
        } else if !batch.is_empty() {
            client.request("thread/inject_items", params).await?;
        }
    }
    Ok(())
}

fn matches_projection(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(k, v)| matches_projection(a.get(k).unwrap_or(&Value::Null), v)),
        (Value::Array(a), Value::Array(e)) => {
            a.len() == e.len() && a.iter().zip(e).all(|(a, e)| matches_projection(a, e))
        }
        _ => actual == expected,
    }
}

async fn wait_persisted(
    path: &Path,
    thread: &str,
    items: &[Value],
) -> Result<ImportBoundary, TransferError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::fs::read(path).await {
            Ok(raw) => {
                if let Some(boundary) = persisted(&raw, path, thread, items)? {
                    return Ok(boundary);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(error("session_transfer_native_import_failed")),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(error("session_transfer_import_persistence_unconfirmed"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn persisted(
    raw: &[u8],
    path: &Path,
    thread: &str,
    items: &[Value],
) -> Result<Option<ImportBoundary>, TransferError> {
    if !raw.ends_with(b"\n") {
        return Ok(None);
    }
    let rows = raw
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| error("session_transfer_native_import_failed"))?;
    if rows
        .first()
        .is_none_or(|row| row["type"] != "session_meta" || row["payload"]["id"] != thread)
    {
        return Err(error("session_transfer_import_persistence_mismatch"));
    }
    let ids: Vec<String> = items
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default().to_owned())
        .collect();
    let imported: Vec<&Value> = rows
        .iter()
        .filter(|row| row["type"] == "response_item")
        .map(|row| &row["payload"])
        .filter(|item| {
            item["id"]
                .as_str()
                .is_some_and(|id| ids.iter().any(|wanted| wanted == id))
        })
        .collect();
    if imported.len() < items.len() {
        return Ok(None);
    }
    if imported.len() != items.len()
        || !imported
            .iter()
            .zip(items)
            .all(|(a, e)| matches_projection(a, e))
    {
        return Err(error("session_transfer_import_persistence_mismatch"));
    }
    if rows.iter().any(|row| {
        row["type"] == "compacted"
            || (row["type"] == "event_msg"
                && matches!(
                    row["payload"]["type"].as_str(),
                    Some("task_started" | "user_message" | "agent_message")
                ))
    }) {
        return Err(error("session_transfer_unexpected_import_turn"));
    }
    Ok(Some(ImportBoundary {
        thread_id: thread.to_owned(),
        rollout_path: path.to_string_lossy().into_owned(),
        min_offset: raw.len() as u64,
        imported_item_ids: ids,
        prefix_digest: Some(digest(raw)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ImportProbe {
        cwd: String,
        rollout: String,
        calls: Vec<String>,
        saved: Vec<(Value, Option<ImportBoundary>)>,
        corrupt: bool,
    }

    impl NativeImport for ImportProbe {
        async fn request(&mut self, method: &str, params: Value) -> Result<Value, TransferError> {
            self.calls.push(method.to_owned());
            let thread = json!({"id":"thread","path":self.rollout,"cwd":self.cwd,"model":"test-model","reasoningEffort":"medium"});
            match method {
                "config/read" => Ok(json!({"config":{}})),
                "thread/start" => {
                    assert!(params.get("cwd").is_none());
                    assert_eq!(params["sandbox"], "read-only");
                    std::fs::write(
                        &self.rollout,
                        format!(
                            "{}\n",
                            json!({"type":"session_meta","payload":{"id":"thread"}})
                        ),
                    )
                    .unwrap();
                    Ok(
                        json!({"thread":thread,"model":"test-model","modelProvider":"openai","cwd":self.cwd,"instructionSources":[]}),
                    )
                }
                "thread/read" => Ok(json!({"thread":thread})),
                "thread/settings/update" => Ok(json!({})),
                "skills/list" => Ok(json!({"data":[{"cwd":self.cwd,"skills":[],"errors":[]}]})),
                "mcpServerStatus/list" => Ok(json!({"data":[],"nextCursor":null})),
                "thread/inject_items" => {
                    use std::io::Write;
                    assert_eq!(
                        self.saved.len(),
                        1,
                        "a thread precisa estar registrada antes da injeção"
                    );
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&self.rollout)
                        .unwrap();
                    for mut item in params["items"].as_array().unwrap().clone() {
                        if self.corrupt {
                            item["content"] = json!([]);
                        }
                        writeln!(file, "{}", json!({"type":"response_item","payload":item}))
                            .unwrap();
                    }
                    Ok(json!({}))
                }
                _ => panic!("RPC inesperado: {method}"),
            }
        }

        async fn raw_model(&mut self, _: &Value, _: &str) -> Result<Value, TransferError> {
            Ok(
                json!({"context_window":200000,"effective_context_window_percent":95,
                "input_modalities":["text","image"],"base_instructions":"Instruções do modelo."}),
            )
        }

        async fn save_destination(
            &mut self,
            destination: Value,
            boundary: Option<ImportBoundary>,
        ) -> Result<(), TransferError> {
            self.saved.push((destination, boundary));
            Ok(())
        }
    }

    #[tokio::test]
    async fn import_commits_only_verified_history_without_starting_inference() {
        for corrupt in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let cwd = dir.path().to_string_lossy().into_owned();
            let mut probe = ImportProbe {
                cwd: cwd.clone(),
                rollout: dir
                    .path()
                    .join("rollout.jsonl")
                    .to_string_lossy()
                    .into_owned(),
                calls: vec![],
                saved: vec![],
                corrupt,
            };
            let context = ImportedContext {
                items: vec![
                    json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Continue a ação."}]}),
                ],
                selected_uuids: Default::default(),
                source_digest: String::new(),
                max_output_bytes: 0,
                source_limitations: vec![],
            };
            let options = ImportOptions {
                cwd,
                account_home: dir.path().join("account").to_string_lossy().into_owned(),
                account_id: "account".into(),
                transfer_id: "transfer".into(),
                model: Some("test-model".into()),
                effort: Some("medium".into()),
                permission_mode: "Ask for approval".into(),
                collaboration_mode: "plan".into(),
            };
            let result = prepare_import(&mut probe, &options, &context).await;
            if corrupt {
                assert_eq!(
                    result.err().unwrap().code,
                    "session_transfer_import_persistence_mismatch"
                );
                assert_eq!(probe.saved.len(), 1);
                assert!(probe.saved[0].1.is_none());
            } else {
                let prepared = result.unwrap();
                assert_eq!(prepared.mode, "plan");
                assert_eq!(probe.saved.len(), 2);
                assert_eq!(probe.saved[1].1, Some(prepared.boundary));
                assert_eq!(probe.saved[1].0["model"], "test-model");
            }
            assert!(!probe.calls.iter().any(|call| call.starts_with("turn/")));
        }
    }

    #[test]
    fn capacity_preserves_native_ceiling_and_reserves_continuation() {
        let raw = json!({"context_window":200000,"max_context_window":400000,
            "effective_context_window_percent":95,"input_modalities":["text","image"]});
        let limits = resolve_limits(&raw, &json!({"model_context_window":100000})).unwrap();
        assert_eq!(
            (limits.context, limits.usable, limits.compact),
            (100000, 95000, 90000)
        );
        assert!(
            check_budget(
                &[json!({"type":"input_image","image_url":"data:image/png;base64,AA=="})],
                &limits,
                0
            )
            .is_ok()
        );
        assert!(check_budget(&[], &limits, 85000).is_err());
        for invalid in [json!(0), json!(-1), json!(true), json!(1.5)] {
            assert!(resolve_limits(&raw, &json!({"model_context_window":invalid})).is_err());
        }
    }

    #[test]
    fn persistence_requires_complete_ordered_history_without_a_turn() {
        let items = vec![json!({"id":"a","type":"message","content":[{"text":"ação"}]})];
        let metadata = json!({"type":"session_meta","payload":{"id":"thread"}});
        let imported = json!({"type":"response_item","payload":{"id":"a","type":"message","content":[{"text":"ação","extra":true}],"extra":1}});
        let raw = format!("{metadata}\n{imported}\n");
        let result = persisted(raw.as_bytes(), Path::new("rollout"), "thread", &items)
            .unwrap()
            .unwrap();
        assert_eq!(result.imported_item_ids, vec!["a"]);
        assert_eq!(result.prefix_digest, Some(digest(raw.as_bytes())));
        assert!(
            persisted(
                raw.trim_end().as_bytes(),
                Path::new("rollout"),
                "thread",
                &items
            )
            .unwrap()
            .is_none()
        );
        assert!(
            persisted(
                format!("{raw}{imported}\n").as_bytes(),
                Path::new("rollout"),
                "thread",
                &items
            )
            .is_err()
        );
        assert!(persisted(raw.as_bytes(), Path::new("rollout"), "other", &items).is_err());
        for event in [
            json!({"type":"compacted"}),
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
        ] {
            assert!(
                persisted(
                    format!("{raw}{event}\n").as_bytes(),
                    Path::new("rollout"),
                    "thread",
                    &items
                )
                .is_err()
            );
        }
    }
}
