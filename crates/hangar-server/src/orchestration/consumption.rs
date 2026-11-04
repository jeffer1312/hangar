//! Soma fora da resposta HTTP, com uma tarefa por execução e repetição curta para fonte indisponível.
use super::{array, runs, text, truthy};
use crate::costs::{self, index::Fold, rows::UsageRow};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Instant,
};
pub mod cache;
static CACHE:LazyLock<Arc<cache::Cache>>=LazyLock::new(||Arc::new(cache::Cache::default()));
fn clock()->f64 {static START:LazyLock<Instant>=LazyLock::new(Instant::now);START.elapsed().as_secs_f64()}
pub fn cached(dir:&Path,home:&Path,names:Vec<String>,aux:Value,live:Vec<Value>,costs:Arc<costs::collect::Collector>)->(Value,Option<String>){
    let key=format!("{}{}",fs::canonicalize(dir).unwrap_or_else(|_|dir.into()).display(),if truthy(&aux["until"]){format!("@{}",text(&aux["until"]))}else{String::new()});
    let dir=dir.to_owned();let home=home.to_owned();
    CACHE.request(key,Arc::new(clock),60.,5.,move||compute(&dir,&home,&names,&aux,&live,&costs),|work|{
        std::thread::Builder::new().name("orq-consumption".into()).spawn(work).map(|_|())
    })
}
fn walk(dir: &Path, filter: &impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        if let Ok(entries) = fs::read_dir(dir) {
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries.into_iter().rev() {
                let p = e.path();
                if e.file_type().is_ok_and(|m| m.is_dir()) {
                    pending.push(p)
                } else if filter(&p) {
                    out.push(p)
                }
            }
        }
    }
    out.sort();
    out
}
fn claude_path(home: &Path, config: &str, id: &str) -> Option<PathBuf> {
    let roots = if config.is_empty() {
        let mut roots = vec![home.join(".claude")];
        if let Ok(entries) = fs::read_dir(home) {
            roots.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with(".claude-")
            }))
        }
        roots
    } else {
        vec![PathBuf::from(config)]
    };
    for root in roots {
        let target = format!("{id}.jsonl");
        if let Some(p) = walk(&root.join("projects"), &|p| {
            p.file_name().is_some_and(|n| n == target.as_str())
        })
        .into_iter()
        .next()
        {
            return Some(p);
        }
    }
    None
}
fn codex_path(home: &Path, config: &str, id: &str) -> Option<PathBuf> {
    for root in [PathBuf::from(config), home.join(".codex")] {
        if root.as_os_str().is_empty() {
            continue;
        }
        let suffix = format!("-{id}.jsonl");
        if let Some(p) = walk(&root.join("sessions"), &|p| {
            p.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with("rollout-")
                    && n.to_string_lossy().ends_with(&suffix)
            })
        })
        .into_iter()
        .next()
        {
            return Some(p);
        }
    }
    None
}
pub fn paths(
    dir: &Path,
    home: &Path,
    names: &[String],
    live: &[Value],
) -> BTreeMap<String, IndexMap<PathBuf, String>> {
    let mut out: BTreeMap<String, IndexMap<PathBuf, String>> =
        names.iter().map(|n| (n.clone(), IndexMap::new())).collect();
    let mut add = |name: &str, provider: &str, path: Option<PathBuf>| {
        if let (Some(rows), Some(path)) = (out.get_mut(name), path) {
            let path = fs::canonicalize(&path).unwrap_or(path);
            rows.entry(path).or_insert(provider.into());
        }
    };
    let mut last = IndexMap::new();
    for row in runs::lines(&dir.join("sessions.jsonl"), false).unwrap_or_default() {
        if let Some(name) = row["name"].as_str() {
            last.insert(name.to_owned(), row);
        }
    }
    for (name, row) in last {
        match text(&row["provider"]) {
            "claude" if truthy(&row["session_id"]) => add(
                &name,
                "claude",
                claude_path(home, text(&row["config_dir"]), text(&row["session_id"])),
            ),
            "codex" if truthy(&row["thread_id"]) => add(
                &name,
                "codex",
                codex_path(home, text(&row["codex_home"]), text(&row["thread_id"])),
            ),
            _ => (),
        }
    }
    for p in walk(&dir.join("medicao"), &|p| {
        p.extension().is_some_and(|x| x == "json")
    }) {
        if let Ok(snap) = runs::json(&p) {
            if let Some(path) = snap["source"]["path"]
                .as_str()
                .filter(|p| Path::new(p).is_file())
            {
                add(
                    text(&snap["session"]),
                    text(&snap["provider"]),
                    Some(path.into()),
                )
            }
        }
    }
    for row in live {
        add(
            text(&row["name"]),
            text(&row["provider"]),
            row["jsonl"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        )
    }
    out
}
fn folded<F: Fold>(
    path: &Path,
    mut fold: F,
    until: Option<chrono::DateTime<chrono::Local>>,
    provider: &str,
) -> Option<Vec<UsageRow>> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).split(b'\n') {
        let raw = line.ok()?;
        if let Some(until) = until {
            if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
                let at = runs::when(&v["timestamp"]);
                if provider == "codex"
                    && at.is_none()
                    && (v["type"] == "token_usage_record"
                        || (v["type"] == "event_msg" && v["payload"]["type"] == "token_count"))
                {
                    return None;
                }
                if at.is_some_and(|t| t > until) {
                    continue;
                }
            }
        }
        fold.line(&raw)
    }
    Some(fold.close().costs)
}
pub fn rows_for(
    provider: &str,
    path: &Path,
    until: Option<chrono::DateTime<chrono::Local>>,
) -> Option<Vec<UsageRow>> {
    if until.is_some() && !matches!(provider, "claude" | "codex") {
        return None;
    }
    match provider {
        "claude" => {
            let mut files = vec![path.to_owned()];
            let dir = path.parent()?.join(path.file_stem()?).join("subagents");
            files.extend(walk(&dir, &|p| p.extension().is_some_and(|x| x == "jsonl")));
            let mut rows = Vec::new();
            for file in files {
                let sub = file != path;
                rows.extend(folded(
                    &file,
                    costs::claude::ClaudeFold::new(
                        file.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        sub,
                        sub,
                    ),
                    until,
                    provider,
                )?)
            }
            Some(rows)
        }
        "codex" => folded(path, costs::codex::new_fold(path), until, provider),
        "pi" | "omp" => folded(
            path,
            costs::simple::new_pi_fold(path.parent()?, provider)(path),
            None,
            provider,
        ),
        "kimi" => folded(path, costs::simple::new_kimi_fold(path), None, provider),
        _ => None,
    }
}
pub fn before_start_day<Tz: chrono::TimeZone>(
    ts: costs::py::LocalTs,
    since: chrono::DateTime<Tz>,
) -> bool
where
    Tz::Offset: std::fmt::Display,
{
    ts.day() < costs::py::LocalTs(since.timestamp_micros()).day()
}
pub fn compute(
    dir: &Path,
    home: &Path,
    names: &[String],
    aux: &Value,
    live: &[Value],
    collector: &costs::collect::Collector,
) -> Result<(Value, bool), String> {
    compute_with(dir,names,aux,live,collector,|live|paths(dir,home,names,live),rows_for)
}
pub type SourcePaths=BTreeMap<String,IndexMap<PathBuf,String>>;
pub fn compute_with(
    dir:&Path,names:&[String],aux:&Value,live:&[Value],collector:&costs::collect::Collector,
    resolve:impl FnOnce(&[Value])->SourcePaths,
    read:impl Fn(&str,&Path,Option<chrono::DateTime<chrono::Local>>)->Option<Vec<UsageRow>>,
)->Result<(Value,bool),String>{
    let since = runs::when(&aux["since"]);
    let until = runs::when(&aux["until"]);
    let sources = resolve(if until.is_some() { &[] } else { live });
    let mut hints = aux["models"].as_object().cloned().unwrap_or_default();
    for p in walk(&dir.join("medicao"), &|p| {
        p.extension().is_some_and(|x| x == "json")
    }) {
        if let Ok(snap) = runs::json(&p) {
            if snap["session"].is_string() && truthy(&snap["model_observed"]) {
                hints.insert(
                    text(&snap["session"]).into(),
                    snap["model_observed"].clone(),
                );
            }
        }
    }
    let mut seen = HashMap::<PathBuf, bool>::new();
    let mut readable = HashSet::<String>::new();
    let mut groups = IndexMap::<(String, String), Value>::new();
    let mut missing_prices = BTreeSet::new();
    let mut subagents = false;
    for name in names {
        for (path, provider) in sources.get(name).into_iter().flatten() {
            if let Some(known) = seen.get(path) {
                if *known {
                    readable.insert(name.clone());
                }
                continue;
            }
            let rows = read(provider, path, until);
            seen.insert(path.clone(), rows.is_some());
            let Some(rows) = rows else { continue };
            readable.insert(name.clone());
            for mut row in rows {
                if until.is_some_and(|u| {
                    row.ts.0 > u.timestamp_micros()
                        || since.is_some_and(|s| row.ts.0 < s.timestamp_micros())
                }) {
                    continue;
                }
                subagents |= row.subagente;
                if let Some(since) = since {
                    let before = if row.source == "claude" && until.is_none() {
                        before_start_day(row.ts, since)
                    } else {
                        row.ts.0 < since.timestamp_micros()
                    };
                    if before {
                        continue;
                    }
                }
                if row.model.is_empty() {
                    if let Some(hint) = hints.get(name) {
                        row.model = text(hint).into()
                    }
                }
                let pricing = collector.pricing();
                let cost = costs::report_costs::row_cost(&row, &pricing);
                let model = pricing.canonizar(&row.model);
                let model = if model.is_empty() {
                    "unknown".into()
                } else {
                    model
                };
                drop(pricing);
                let key = (row.source.clone(), model.clone());
                let group = groups
                    .entry(key)
                    .or_insert(json!({"names":[],"new":0,"cache_read":0,"usd":0.}));
                if !array(&group["names"]).iter().any(|n| n == name) {
                    group["names"].as_array_mut().unwrap().push(json!(name))
                }
                super::increment(group, "new", row.input + row.output + row.cache_write);
                super::increment(group, "cache_read", row.cache_read);
                if let Some(cost) = cost {
                    group["usd"] =
                        json!(group["usd"].as_f64().unwrap_or(0.) + cost.iter().sum::<f64>())
                } else {
                    missing_prices.insert(model);
                }
            }
        }
    }
    let mut by = IndexMap::<String, Vec<(String, Value)>>::new();
    for ((source, model), g) in groups {
        by.entry(source).or_default().push((model, g));
    }
    let mut providers = Vec::new();
    for (provider, mut models) in by {
        models.sort_by(|a, b| {
            b.1["usd"]
                .as_f64()
                .unwrap_or(0.)
                .total_cmp(&a.1["usd"].as_f64().unwrap_or(0.))
                .then(a.0.cmp(&b.0))
        });
        let names: BTreeSet<_> = models
            .iter()
            .flat_map(|(_, g)| array(&g["names"]).iter().map(|n| text(n).to_owned()))
            .collect();
        providers.push(json!({"provider":provider,"sessions":names.len(),"new":models.iter().map(|(_,g)|g["new"].as_i64().unwrap_or(0)).sum::<i64>(),"cache_read":models.iter().map(|(_,g)|g["cache_read"].as_i64().unwrap_or(0)).sum::<i64>(),"usd":super::round(models.iter().map(|(_,g)|g["usd"].as_f64().unwrap_or(0.)).sum(),4),"models":models.iter().map(|(m,g)|json!({"model":m,"sessions":array(&g["names"]).len(),"new":g["new"],"cache_read":g["cache_read"],"usd":super::round(g["usd"].as_f64().unwrap_or(0.),4)})).collect::<Vec<_>>()}));
    }
    providers.sort_by(|a, b| {
        b["usd"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&a["usd"].as_f64().unwrap_or(0.))
            .then(text(&a["provider"]).cmp(text(&b["provider"])))
    });
    let unavailable = names
        .iter()
        .any(|n| sources.get(n).is_some_and(|s| !s.is_empty()) && !readable.contains(n));
    Ok((
        json!({"computed_at":chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs,false),"since":aux["since"],"until":aux["until"],"sessions":{"team":names.len(),"measured":readable.len(),"missing":names.iter().filter(|n|!readable.contains(*n)).collect::<Vec<_>>()},"totals":{"new":providers.iter().map(|p|p["new"].as_i64().unwrap_or(0)).sum::<i64>(),"cache_read":providers.iter().map(|p|p["cache_read"].as_i64().unwrap_or(0)).sum::<i64>(),"usd":super::round(providers.iter().map(|p|p["usd"].as_f64().unwrap_or(0.)).sum(),4),"usd_partial":!missing_prices.is_empty()||readable.len()<names.len()},"providers":providers,"missing_prices":missing_prices,"subagents":subagents}),
        unavailable,
    ))
}
