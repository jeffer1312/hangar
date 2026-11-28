use super::{Action, ClockSample, State, Status, current, row_id, waiting_row};
use serde_json::{Value, json};
use std::{collections::BTreeSet, io};

pub(super) fn claim(state: &mut State, min_ts: f64, clock: ClockSample, call_id: &str) -> io::Result<Value> {
    let mut indices = Vec::new();
    let mut individual = false;
    for (index, row) in state.rows.iter().enumerate() {
        if !current(row, min_ts) || !waiting_row(state, row) || state.protected(row_id(row)) { continue; }
        // Uma tentativa já selada conserva o próprio corte, mesmo se chegaram mensagens depois.
        if row["batch_entries"].is_array() {
            if indices.is_empty() { indices.push(index); individual = true; }
            break;
        }
        let text = row["text"].as_str().unwrap_or("");
        if !super::queue_text_ok(text) || hangar_core::queue_row::message_prefix(text).is_some() {
            if indices.is_empty() { indices.push(index); individual = true; break; }
            continue;
        }
        indices.push(index);
    }
    let Some(&first) = indices.first() else { return Ok(json!([])); };
    if individual {
        let entry_id = Some(row_id(&state.rows[first]).to_owned());
        return super::apply(state, Action::Claim { min_ts, limit: Some(1), entry_id }, clock, call_id);
    }
    let entries: Vec<_> = indices.iter().map(|index| state.rows[*index].clone()).collect();
    let members: BTreeSet<_> = entries.iter().map(row_id).collect();
    let id = if indices.len() == 1 {row_id(&state.rows[first]).to_owned()}
        else {format!("batch-{}", sha1_smol::Sha1::from(format!("{}:{}:{call_id}", state.owner_key, state.generation)).digest())};
    let text = entries.iter().map(|row| row["text"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n");
    if text.len() > hangar_core::net::MAX_FRAME { return Err(super::invalid(super::BATCH_SIZE_MESSAGE)); }
    let mut row = state.rows[first].clone();
    row["id"] = json!(id);
    row["text"] = json!(text);
    row["delivered"] = json!(true);
    row["claimed_ts"] = json!(clock.epoch_s);
    row["pre_transcript"] = json!(entries.iter().any(|entry| entry["pre_transcript"] == true));
    // O lote é fila comum: a marca "Na conversa" do primeiro não vale para as outras.
    if indices.len() > 1 && let Some(fields) = row.as_object_mut() { fields.remove("conversation"); }
    // As intenções originais continuam ligadas ao lote; o Recover não as rematerializa.
    for op in state.operations.values_mut().filter(|op| op.entry_id.as_deref().is_some_and(|entry|entry != id && members.contains(entry))) {
        op.status = Status::Accepted;
        op.result = json!({"operation_id":op.id,"disposition":"accepted","payload":{"queued":true,"batch_id":id}});
        op.entry_materialized = true;
        op.terminal_finalized = true;
    }
    state.rows.retain(|entry| !members.contains(row_id(entry)));
    row["batch_entries"] = json!(entries);
    state.rows.insert(first, row.clone());
    Ok(json!([row]))
}

pub(super) fn confirm_members(state: &mut State, id: &str) {
    let Some(row)=state.rows.iter_mut().find(|row|row_id(row)==id) else {return;};
    if let Some(fields)=row.as_object_mut() {fields.remove("batch_prompt");}
    let Some(entries)=row.get_mut("batch_entries").and_then(Value::as_array_mut) else {return;};
    let mut members = BTreeSet::new();
    for entry in entries {
        members.insert(row_id(entry).to_owned());
        entry["delivered"] = json!(true);
        entry["confirmed"] = json!(true);
    }
    for op in state.operations.values_mut().filter(|op|op.entry_id.as_deref().is_some_and(|entry|members.contains(entry))) {
        op.status = Status::Confirmed;
    }
}
