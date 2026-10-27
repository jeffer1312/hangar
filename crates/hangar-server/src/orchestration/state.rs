//! Redução de eventos da execução: fechamento, retomada, trocas e esperas.
use super::{integer, kind, runs, text, truthy};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::Path;
pub fn closes(dir: &Path) -> Result<IndexMap<i128, Value>, String> {
    let mut out = IndexMap::new();
    for e in runs::lines(&dir.join("closed.jsonl"), true).map_err(|e| e.to_string())? {
        if let Some(n) = integer(&e["task"]) {
            if truthy(&e["hash"]) {
                out.shift_remove(&n);
                out.insert(n, e);
            }
        }
    }
    Ok(out)
}
pub fn closed(dir: &Path) -> Result<IndexMap<i128, Value>, String> {
    let mut out = IndexMap::new();
    for value in runs::lines(&dir.join("closed.jsonl"), true).map_err(|e| e.to_string())? {
        if let Some(n) = integer(&value["task"]) {
            out.insert(n, value);
        }
    }
    Ok(out)
}
pub fn at_or_after(at: &Value, reference: &Value) -> bool {
    if reference.is_null() {
        return true;
    }
    match (runs::when(at), runs::when(reference)) {
        (Some(a), Some(b)) => a >= b,
        _ => true,
    }
}
pub fn outcome(events: &[Value], n: i128, close: &Value) -> Option<String> {
    events
        .iter()
        .filter(|e| integer(&e["task"]) == Some(n) && at_or_after(&e["ts"], &close["ts"]))
        .filter(|e| {
            matches!(kind(e), "integrada" | "conflito" | "integracao_vermelha")
                || (kind(e) == "advance_falhou" && e["passo"] == "integrate")
        })
        .last()
        .map(|e| kind(e).to_owned())
}
pub fn read(dir: &Path) -> Result<Value, String> {
    let cfg = runs::json(&dir.join("orq.json")).map_err(|e| e.to_string())?;
    if !cfg.is_object() || cfg.get("arbiter").is_none() {
        return Err("orq.json sem árbitro".into());
    }
    let ev = runs::lines(&dir.join("eventos.jsonl"), true).map_err(|e| e.to_string())?;
    let closed = closed(dir)?;
    Ok(reduce(&cfg, &ev, &closed, runs::now()))
}
pub fn reduce(cfg: &Value, events: &[Value], closed: &IndexMap<i128, Value>, now: f64) -> Value {
    let mut arbiter = cfg["arbiter"].clone();
    let mut roles: IndexMap<i128, Value> = IndexMap::new();
    let mut last: IndexMap<i128, Value> = IndexMap::new();
    let mut waits: IndexMap<String, Value> = IndexMap::new();
    let mut replaced = Vec::new();
    let mut ended = false;
    for e in events {
        let t = kind(e);
        if t == "espera" {
            waits.insert(text(&e["sessao"]).into(), e.clone());
            continue;
        }
        waits.shift_remove(text(&e["sessao"]));
        if let Some(n) = integer(&e["task"]) {
            let row = if t == "task_inicio" {
                e.clone()
            } else {
                roles.get(&n).cloned().unwrap_or_default()
            };
            waits.retain(|who, w| {
                integer(&w["task"]) != Some(n)
                    && !(w["task"].is_null()
                        && (row["executor"] == who.as_str() || row["par"] == who.as_str()))
            });
        }
        match t {
            "task_inicio" => {
                if let Some(n) = integer(&e["task"]) {
                    roles.insert(n, json!({"executor":e["executor"],"par":e["par"]}));
                    last.insert(n, e.clone());
                }
            }
            "entrega" | "veredito" => {
                if let Some(n) = integer(&e["task"]) {
                    last.insert(n, e.clone());
                }
            }
            "sessao_trocada" => {
                waits.shift_remove(text(&e["de"]));
                let was_arbiter = e["de"] == arbiter;
                if was_arbiter {
                    arbiter = e["para"].clone()
                }
                let mut held = false;
                for r in roles.values_mut() {
                    for k in ["executor", "par"] {
                        if r[k] == e["de"] {
                            r[k] = e["para"].clone();
                            held = true
                        }
                    }
                }
                if held && !was_arbiter {
                    replaced.push(json!([e["de"], e["para"]]))
                }
            }
            "execucao_fim" => {
                last.clear();
                waits.clear();
                ended = true
            }
            _ => (),
        }
        if t != "execucao_fim" && integer(&e["task"]).is_some() {
            ended = false
        }
    }
    let mut owners = Vec::<String>::new();
    let mut open = Vec::<i128>::new();
    for (n, e) in last {
        if closed
            .get(&n)
            .is_some_and(|c| at_or_after(&c["ts"], &e["ts"]))
        {
            continue;
        }
        open.push(n);
        let row = roles.get(&n).cloned().unwrap_or_default();
        let owner = if kind(&e) == "entrega" {
            if row["par"] == "subagente" {
                &row["executor"]
            } else {
                &row["par"]
            }
        } else if kind(&e) == "veredito" && e["resultado"] == "devolvido" {
            &Value::Null
        } else {
            &row["executor"]
        };
        let name = text(owner);
        if !name.is_empty() && !owners.iter().any(|s| s == name) {
            owners.push(name.into())
        }
    }
    let waiting: Vec<_> = waits
        .into_iter()
        .filter(|(_, w)| {
            runs::when(&w["ate"]).is_some_and(|t| t.timestamp_micros() as f64 / 1e6 > now)
                && (w["task"].is_null() || integer(&w["task"]).is_some_and(|n| open.contains(&n)))
        })
        .map(|(who, _)| who)
        .collect();
    let ball: Vec<_> = owners.iter().filter(|o| !waiting.contains(o)).collect();
    let role_map: serde_json::Map<String, Value> =
        roles.into_iter().map(|(n, r)| (n.to_string(), r)).collect();
    json!({"roles":role_map,"ball":ball,"owners":owners,"waiting":waiting,"arbiter":arbiter,"open":open,"ended":ended,"replaced":replaced})
}
