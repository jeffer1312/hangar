//! Retrato por execução: cada bloco falha isoladamente e o consumo não prende a resposta.
use super::{array, integer, kind, runs, state, text, timeline, truthy};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    sync::{Arc, LazyLock},
    time::SystemTime,
};
static PANELS: LazyLock<
    super::snapshot::SnapshotCache<Vec<Option<(SystemTime, u64)>>, (Value, Value)>,
> = LazyLock::new(|| super::snapshot::SnapshotCache::new(false));
pub fn verdict(e: &Value) -> Option<&'static str> {
    match text(&e["resultado"]) {
        "aprova" | "corrige" => Some("approved"),
        "reprova" | "devolvido" => Some("rejected"),
        _ => None,
    }
}
fn read_rows(dir: &Path, name: &str, errors: &mut Vec<Value>) -> Vec<Value> {
    match runs::lines(&dir.join(name), true) {
        Ok(v) => v,
        Err(e) => {
            errors.push(json!({"file":name,"error":e.to_string()}));
            Vec::new()
        }
    }
}
fn signature(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}
pub fn panel(
    dir: &Path,
    home: &Path,
    live: Vec<Value>,
    costs: Arc<crate::costs::collect::Collector>,
) -> Value {
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.into());
    let cfg = runs::json(&dir.join("orq.json")).unwrap_or_default();
    let files = [
        "orq.json",
        "eventos.jsonl",
        "jev-shadow.jsonl",
        "closed.jsonl",
        "checks.jsonl",
        "sessions.jsonl",
        "plan.snapshot.md",
    ];
    let mut sig: Vec<_> = files.iter().map(|f| signature(&dir.join(f))).collect();
    sig.push(signature(&runs::timeline_path(dir)));
    sig.push(signature(&super::expand(
        Path::new(text(&cfg["plan"])),
        home,
    )));
    let (mut out, aux) = PANELS.get(key, sig, || build(dir, home));
    out = with_timing(out, &aux, runs::now());
    attach_consumption(out, aux, dir, home, live, costs)
}
pub fn attach_consumption(
    mut out: Value,
    aux: Value,
    dir: &Path,
    home: &Path,
    live: Vec<Value>,
    costs: Arc<crate::costs::collect::Collector>,
) -> Value {
    let names: Vec<_> = array(&out["team"])
        .iter()
        .map(|r| text(&r["name"]).to_owned())
        .collect();
    if names.is_empty() && !array(&out["errors"]).is_empty() {
        out["errors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"file":"consumption","error":"sem o time não há como somar o consumo"}));
        return out;
    }
    let (value, error) = super::consumption::cached(dir, home, names, aux, live, costs);
    out["consumption"] = value;
    if let Some(error) = error {
        out["errors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"file":"consumption","error":error}))
    }
    out
}
pub fn task_rows(events: &[Value], plan: &[Value], integrated: &BTreeSet<super::Integer>) -> Vec<Value> {
    let mut titles: BTreeMap<super::Integer, Value> = plan
        .iter()
        .filter_map(|t| Some((integer(&t["n"])?, t["title"].clone())))
        .collect();
    let mut deciding = BTreeMap::<super::Integer, (&str, Value)>::new();
    for e in events {
        let Some(n) = integer(&e["task"]) else {
            continue;
        };
        titles.entry(n.clone()).or_insert(json!(""));
        let rnd = integer(&e["rodada"]).map(|n| json!(n)).unwrap_or_default();
        match kind(e) {
            "task_inicio" => {
                if !truthy(&titles[&n]) {
                    titles.insert(
                        n.clone(),
                        e.get("titulo")
                            .filter(|v| truthy(v))
                            .cloned()
                            .unwrap_or(json!("")),
                    );
                }
                deciding.insert(n, ("executing", Value::Null));
            }
            "entrega" => {
                deciding.insert(n, ("in_review", rnd));
            }
            "veredito" => {
                if let Some(v) = verdict(e) {
                    deciding.insert(n, (v, rnd));
                }
            }
            "integracao_vermelha" | "conflito" => {
                deciding.insert(n, ("integration_red", Value::Null));
            }
            "advance_falhou" if e["passo"] == "integrate" => {
                deciding.insert(n, ("integration_red", Value::Null));
            }
            "integrada" => {
                deciding.insert(n, ("integrated", Value::Null));
            }
            _ => (),
        }
    }
    titles
        .into_iter()
        .map(|(n, title)| {
            let (state, round) = deciding.get(&n).cloned().unwrap_or((
                if integrated.contains(&n) {
                    "integrated"
                } else {
                    "queued"
                },
                Value::Null,
            ));
            json!({"n":n,"title":title,"state":state,"round":round})
        })
        .collect()
}
pub fn team(cfg: &Value, st: &Value, events: &[Value]) -> Vec<Value> {
    let mut roles: BTreeMap<super::Integer, Value> = BTreeMap::new();
    let mut last = IndexMap::<String, Value>::new();
    let mut slots = BTreeMap::<String, (super::Integer, &str)>::new();
    let mut arbiter = cfg["arbiter"].clone();
    let mut before = Vec::new();
    for e in events {
        let n = integer(&e["task"]);
        let rnd = integer(&e["rodada"]).map(|n| json!(n));
        match kind(e) {
            "task_inicio" => {
                if let Some(n) = n {
                    let row = json!({"executor":e["executor"],"par":e["par"]});
                    for k in ["executor", "par"] {
                        if let Some(who) = row[k].as_str() {
                            last.insert(
                                who.into(),
                                json!({"code":"started","round":null,"ts":e["ts"]}),
                            );
                        }
                    }
                    roles.insert(n, row);
                }
            }
            "entrega" => {
                if let Some(who) = n
                    .and_then(|n| roles.get(&n))
                    .and_then(|r| r["executor"].as_str())
                {
                    last.insert(
                        who.into(),
                        json!({"code":"delivered","round":rnd,"ts":e["ts"]}),
                    );
                }
            }
            "veredito" => {
                if let (Some(who), Some(v)) = (e["sessao"].as_str(), verdict(e)) {
                    last.insert(who.into(), json!({"code":v,"round":rnd,"ts":e["ts"]}));
                }
            }
            "sessao_trocada" => {
                if e["de"] == arbiter {
                    before.push(arbiter.clone());
                    arbiter = e["para"].clone()
                }
                for (n, row) in roles.iter_mut() {
                    for (key, role) in [("executor", "executor"), ("par", "reviewer")] {
                        if row[key] == e["de"] {
                            row[key] = e["para"].clone();
                            slots.insert(text(&e["de"]).into(), (n.clone(), role));
                            if let Some(who) = e["para"].as_str() {
                                last.insert(
                                    who.into(),
                                    json!({"code":"swapped_in","round":null,"ts":e["ts"]}),
                                );
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    }
    let mut rows =
        vec![json!({"name":st["arbiter"],"role":"arbiter","task":null,"last":null,"current":true})];
    for n in before.into_iter().rev() {
        rows.push(json!({"name":n,"role":"arbiter","task":null,"last":null,"current":false}))
    }
    let mut replaced = BTreeMap::<super::Integer, Vec<Value>>::new();
    for pair in array(&st["replaced"]) {
        let old = text(&pair[0]);
        if let Some((n, role)) = slots.get(old) {
            replaced.entry(n.clone()).or_default().push(
                json!({"name":old,"role":role,"task":n,"last":last.get(old),"current":false}),
            );
        }
    }
    let mut state_roles: Vec<_> = st["roles"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(n, v)| n.parse::<super::Integer>().ok().map(|n| (n, v)))
        .collect();
    state_roles.sort_by_key(|(n, _)| std::cmp::Reverse(n.clone()));
    for (n, row) in state_roles {
        for (key, role) in [("executor", "executor"), ("par", "reviewer")] {
            if let Some(who) = row[key]
                .as_str()
                .filter(|s| !s.trim().is_empty() && !s.contains(' ') && *s != "subagente")
            {
                rows.push(
                    json!({"name":who,"role":role,"task":n,"last":last.get(who),"current":true}),
                )
            }
        }
        rows.extend(replaced.remove(&n).unwrap_or_default())
    }
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert(r["name"].to_string()));
    rows
}
pub fn decisions(entries: &[(Value, Value)], events: &[Value]) -> Vec<Value> {
    let mut later = BTreeMap::<super::Integer, Vec<chrono::DateTime<chrono::Local>>>::new();
    for e in events {
        if matches!(kind(e), "entrega" | "veredito" | "integrada") {
            if let (Some(n), Some(ts)) = (integer(&e["task"]), runs::when(&e["ts"])) {
                later.entry(n).or_default().push(ts)
            }
        }
    }
    let mut rows = Vec::new();
    for (obj, e) in entries.iter().rev() {
        let orch = e["origin"] == "orchestrator";
        let asks = (matches!(text(&e["kind"]), "woke" | "would_drop")
            && (e["mark"] == "decisao" || orch))
            || (e["kind"] == "failed" && orch);
        let Some(n) = integer(&e["task"]) else {
            continue;
        };
        if !asks {
            continue;
        }
        if runs::when(&obj["ts"])
            .is_some_and(|at| later.get(&n).is_some_and(|all| all.iter().any(|t| *t > at)))
        {
            continue;
        }
        let body = text(&e["body"]);
        let q = if truthy(&e["question"]) {
            e["question"].clone()
        } else {
            json!(timeline::first_sentence(body))
        };
        rows.push(json!({"task":n,"ts":obj["ts"],"question":q,"parecer":e["parecer"],"event_id":timeline::event_id(obj)}));
    }
    rows
}
pub fn automation(cfg: &Value, entries: &[(Value, Value)], shadow: &[Value]) -> Value {
    let mut woke = json!({"total":0,"decisions":0,"alarms":0,"messages":0});
    let mut alone = json!({"total":0,"opened":0,"integrated":0,"dropped":0});
    let (mut dropped, mut would, mut rule) = (0, 0, 0);
    for (_, e) in entries {
        let kind = text(&e["kind"]);
        let orch = e["origin"] == "orchestrator";
        if matches!(kind, "woke" | "would_drop") || (kind == "failed" && orch) {
            super::increment(&mut woke, "total", 1);
            super::increment(
                &mut woke,
                if truthy(&e["mark"]) || orch {
                    "decisions"
                } else if truthy(&e["alarm"]) {
                    "alarms"
                } else {
                    "messages"
                },
                1,
            )
        }
        if kind == "would_drop" {
            would += 1
        }
        let code = text(&e["line"]["code"]);
        if kind == "advance" && matches!(code, "opened" | "integrated") {
            super::increment(&mut alone, code, 1)
        } else if kind == "dropped" {
            super::increment(&mut alone, "dropped", 1);
            dropped += i64::from(e["decided_by"]["source"] == "jev")
        }
        rule += i64::from(matches!(text(&e["decided_by"]["source"]), "rule" | "alarm"));
    }
    alone["total"] = json!(
        alone["opened"].as_i64().unwrap_or(0)
            + alone["integrated"].as_i64().unwrap_or(0)
            + alone["dropped"].as_i64().unwrap_or(0)
    );
    let (mut judged, mut disagree) = (0, 0);
    let mut lowest = Value::Null;
    for row in shadow {
        if truthy(&row["error"]) || !row["text"].is_string() {
            continue;
        }
        judged += 1;
        disagree += i64::from(
            (super::triage::decide(text(&row["text"])).0 == "drop") != truthy(&row["would_drop"]),
        );
        let choice = text(&row["choice"]);
        let p = row["probs"]
            .get(choice)
            .and_then(Value::as_f64)
            .or_else(|| (choice == "nothing").then(|| row["p"].as_f64()).flatten())
            .filter(|p| p.is_finite());
        if let Some(p) = p {
            if lowest["p"].as_f64().is_none_or(|old| p < old) {
                lowest = json!({"p":p,"choice":row["choice"],"ts":row["ts"],"text":text(&row["text"]).chars().take(120).collect::<String>()});
            }
        }
    }
    json!({"mode":{"jev":cfg.get("jev").unwrap_or(&json!("shadow")),"regex":cfg.get("regex").unwrap_or(&json!("shadow"))},"woke":woke,"alone":alone,"dropped_by_jev":dropped,"advanced":{"would_drop":would,"disagree":disagree,"judged":judged,"min_confidence":lowest,"by_rule":rule}})
}
pub fn integration(events: &[Value], closes: &IndexMap<super::Integer, Value>, checks: &[Value]) -> Value {
    let begin = events
        .iter()
        .find(|e| kind(e) == "execucao_inicio")
        .cloned()
        .unwrap_or_default();
    let last = events.iter().rev().find(|e| kind(e) == "integrada");
    let outcome = closes
        .last()
        .and_then(|(n, c)| state::outcome(events, n.clone(), c))
        .and_then(|s| match s.as_str() {
            "integrada" => Some("green"),
            "integracao_vermelha" => Some("red"),
            "conflito" => Some("conflict"),
            "advance_falhou" => Some("failed"),
            _ => None,
        });
    let red = events
        .iter()
        .rev()
        .find(|e| kind(e) == "integracao_vermelha" && e["motivo"].is_string())
        .map(|e| e["motivo"].clone());
    let mut latest = BTreeMap::new();
    for c in checks {
        if let Some(n) = integer(&c["task"]) {
            latest.insert(n, c);
        }
    }
    json!({"branch":begin["branch"],"last":last.map(|e|json!({"task":e["task"],"commit":text(&e["commit"]).chars().take(7).collect::<String>(),"ts":e["ts"]})),"outcome":outcome,"red_log":red,"delivery_checks":{"ok":latest.values().filter(|c|truthy(&c["ok"])).count(),"total":latest.len(),"failing":latest.iter().filter(|(_,c)|!truthy(&c["ok"])).map(|(n,_)|n.clone()).collect::<Vec<_>>()}})
}
pub fn bounds(events: &[Value]) -> Value {
    let begin = events
        .iter()
        .find(|e| kind(e) == "execucao_inicio")
        .map(|e| e["ts"].clone());
    let end = runs::current_end(events).map(|e| e["ts"].clone());
    let mut tasks = serde_json::Map::new();
    for e in events {
        let Some(n) = integer(&e["task"]) else {
            continue;
        };
        let task = tasks
            .entry(n.to_string())
            .or_insert(json!({"started_at":null,"finished_at":null}));
        if kind(e) == "task_inicio" && task["started_at"].is_null() {
            task["started_at"] = e["ts"].clone()
        }
        if kind(e) == "integrada" {
            task["finished_at"] = e["ts"].clone()
        } else if matches!(
            kind(e),
            "task_inicio" | "entrega" | "veredito" | "integracao_vermelha" | "conflito"
        ) || (kind(e) == "advance_falhou" && e["passo"] == "integrate")
        {
            task["finished_at"] = Value::Null
        }
    }
    json!({"started_at":begin,"finished_at":end,"tasks":tasks})
}
pub fn with_timing(mut out: Value, aux: &Value, at: f64) -> Value {
    fn timing(start: &Value, end: &Value, at: f64) -> Value {
        let first = runs::when(start);
        let last = runs::when(end);
        let elapsed = first
            .filter(|_| last.is_some() || end.is_null())
            .and_then(|f| {
                let delta = last
                    .map(|x| x.timestamp_micros() as f64 / 1e6)
                    .unwrap_or(at)
                    - f.timestamp_micros() as f64 / 1e6;
                (delta >= 0.).then_some(delta as i64)
            });
        json!({"started_at":runs::iso(start),"finished_at":runs::iso(end),"elapsed_seconds":elapsed})
    }
    let bounds = &aux["timing"];
    out["timing"] = timing(&bounds["started_at"], &bounds["finished_at"], at);
    if let Some(rows) = out["tasks"]["rows"].as_array_mut() {
        for row in rows {
            let task = &bounds["tasks"][row["n"].to_string()];
            row["timing"] = timing(
                &task["started_at"],
                if truthy(&task["finished_at"]) {
                    &task["finished_at"]
                } else {
                    &bounds["finished_at"]
                },
                at,
            )
        }
    }
    out
}
pub trait Projection {
    fn entry(&self, obj: &Value, dir: &Path) -> Result<Option<Value>, String>;
    fn team(&self, cfg: &Value, st: &Value, events: &[Value]) -> Result<Vec<Value>, String>;
    fn decisions(&self, entries: &[(Value, Value)], events: &[Value])
    -> Result<Vec<Value>, String>;
}
pub struct NativeProjection;
impl Projection for NativeProjection {
    fn entry(&self, obj: &Value, dir: &Path) -> Result<Option<Value>, String> {
        Ok(timeline::entry(obj, Some(dir)))
    }
    fn team(&self, cfg: &Value, st: &Value, events: &[Value]) -> Result<Vec<Value>, String> {
        if events
            .iter()
            .any(|e| kind(e) == "task_inicio" && integer(&e["task"]).is_none())
        {
            return Err(
                "TypeError: '<' not supported between instances of 'int' and 'NoneType'".into(),
            );
        }
        Ok(team(cfg, st, events))
    }
    fn decisions(
        &self,
        entries: &[(Value, Value)],
        events: &[Value],
    ) -> Result<Vec<Value>, String> {
        Ok(decisions(entries, events))
    }
}
fn guarded<T>(
    errors: &mut Vec<Value>,
    name: &str,
    fallback: T,
    read: impl FnOnce() -> Result<T, String>,
) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)) {
        Ok(Ok(value)) => value,
        result => {
            let error = match result {
                Ok(Err(error)) => error,
                _ => "não foi possível ler este bloco".into(),
            };
            errors.push(json!({"file":name,"error":error}));
            fallback
        }
    }
}
pub fn build(dir: &Path, home: &Path) -> (Value, Value) {
    build_with(dir, home, &NativeProjection)
}
pub fn build_with(dir: &Path, home: &Path, projection: &dyn Projection) -> (Value, Value) {
    let mut errors = Vec::new();
    let cfg = match runs::json(&dir.join("orq.json")) {
        Ok(c) if c.is_object() => c,
        Ok(_) => {
            errors.push(json!({"file":"orq.json","error":"orq.json não é um objeto"}));
            Value::Null
        }
        Err(e) => {
            errors.push(json!({"file":"orq.json","error":e.to_string()}));
            Value::Null
        }
    };
    let tl = runs::timeline_path(dir);
    let lines = read_rows(
        dir,
        &tl.file_name().unwrap_or_default().to_string_lossy(),
        &mut errors,
    );
    let events = read_rows(dir, "eventos.jsonl", &mut errors);
    let shadow = read_rows(dir, "jev-shadow.jsonl", &mut errors);
    let checks = read_rows(dir, "checks.jsonl", &mut errors);
    let mut entries = Vec::new();
    for obj in &lines {
        if let Some(entry) = guarded(
            &mut errors,
            &tl.file_name().unwrap_or_default().to_string_lossy(),
            None,
            || projection.entry(obj, dir),
        ) {
            entries.push((obj.clone(), entry));
        }
    }
    let closed = match state::closes(dir) {
        Ok(c) => c,
        Err(e) => {
            errors.push(json!({"file":"closed.jsonl","error":e}));
            IndexMap::new()
        }
    };
    let integrated = closed
        .iter()
        .filter_map(|(n, c)| {
            (state::outcome(&events, n.clone(), c).as_deref() == Some("integrada")).then_some(n.clone())
        })
        .collect();
    let begin = events
        .iter()
        .find(|e| kind(e) == "execucao_inicio")
        .cloned()
        .unwrap_or_default();
    let metadata = super::plan::metadata(dir, text(&begin["plano"]), home);
    let mut plan = None;
    if let Some(path) = cfg["plan"].as_str() {
        match std::fs::read_to_string(super::expand(Path::new(path), home)) {
            Ok(content) => plan = Some(super::plan::tasks(&content)),
            Err(e) => errors.push(json!({"file":"plan","error":e.to_string()})),
        }
    }
    if plan.is_none() && !array(&metadata["tasks"]).is_empty() {
        plan = Some(array(&metadata["tasks"]).to_vec())
    }
    let rows = task_rows(&events, plan.as_deref().unwrap_or(&[]), &integrated);
    let st = if cfg.is_object() {
        Some(state::reduce(
            &cfg,
            &events,
            &state::closed(dir).unwrap_or_default(),
            runs::now(),
        ))
    } else {
        None
    };
    let team = st
        .map(|st| {
            guarded(&mut errors, "team", Vec::new(), || {
                projection.team(&cfg, &st, &events)
            })
        })
        .unwrap_or_default();
    let decisions = guarded(&mut errors, "decisions", Vec::new(), || {
        projection.decisions(&entries, &events)
    });
    let timing = bounds(&events);
    let mut models = serde_json::Map::new();
    for (_, e) in &entries {
        if e["line"]["code"] == "opened" {
            for who in array(&e["line"]["sessions"]) {
                models.insert(text(&who["name"]).into(), who["model"].clone());
            }
        }
    }
    let aux =
        json!({"since":begin["ts"],"until":timing["finished_at"],"models":models,"timing":timing});
    let out = json!({"run":std::fs::canonicalize(dir).unwrap_or_else(|_|dir.into()).file_name().unwrap_or_default().to_string_lossy(),"gid":begin.get("gid").filter(|v|truthy(v)).unwrap_or(&json!("")),"errors":errors,"metadata":metadata,"empty":lines.is_empty()&&!events.iter().any(|e|kind(e)=="task_inicio"),"tasks":{"integrated":rows.iter().filter(|r|r["state"]=="integrated").count(),"total":plan.as_ref().map(|v|json!(v.len())).unwrap_or_else(||json!(rows.iter().filter_map(|r|integer(&r["n"])).max().unwrap_or_default())),"total_known":plan.is_some(),"rows":rows},"team":team,"decisions":decisions,"automation":automation(&cfg,&entries,&shadow),"consumption":null,"integration":integration(&events,&closed,&checks)});
    (out, aux)
}
