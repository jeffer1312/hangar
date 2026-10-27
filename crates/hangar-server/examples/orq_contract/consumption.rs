//! Fontes e executor controlados da prova; a soma e o cache são os usados pelo servidor.
use hangar_server::{costs, orchestration as orq};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
struct NoScopes;
impl costs::collect::ScopeSource for NoScopes {
    fn fetch(&self) -> Result<costs::collect::Scopes, costs::collect::CollectError> {
        Err(costs::collect::CollectError::NoScopes)
    }
}
pub fn collector(root: &Path) -> costs::collect::Collector {
    costs::collect::Collector::new(
        root.join("index"),
        costs::pricing::default_dir(),
        root.join("areas.json"),
        Arc::new(NoScopes),
    )
}
fn strings(v: &Value) -> Vec<String> {
    orq::array(v).iter().map(|v| orq::text(v).into()).collect()
}
fn rows(v: &Value) -> Option<Vec<costs::rows::UsageRow>> {
    if v.is_null() {
        return None;
    }
    Some(
        orq::array(v)
            .iter()
            .map(|r| {
                let mut r = r.clone();
                if let Some(ts) = r["ts"].as_str() {
                    r["ts"] = json!(costs::py::LocalTs::from_iso(ts).unwrap().0)
                }
                for (key, value) in [
                    ("subagente", json!(false)),
                    ("account_id", Value::Null),
                    ("codex_long_context", json!(false)),
                    ("cache_write_1h", json!(0)),
                    ("fast", json!(false)),
                    ("regravado", json!(0)),
                    ("regravado_1h", json!(0)),
                ] {
                    if r.get(key).is_none() {
                        r[key] = value
                    }
                }
                serde_json::from_value(r).unwrap()
            })
            .collect(),
    )
}
pub fn evaluate(action: &str, v: &Value) -> Option<Result<Value, String>> {
    Some(match action {
        "consumption.paths" => Ok(json!(orq::consumption::paths(
            Path::new(orq::text(&v["directory"])),
            Path::new(orq::text(&v["home"])),
            &strings(&v["names"]),
            orq::array(&v["live"])
        ))),
        "consumption.rows" => Ok(json!(orq::consumption::rows_for(
            orq::text(&v["provider"]),
            Path::new(orq::text(&v["path"])),
            orq::runs::when(&v["until"])
        ))),
        "consumption.compute" => {
            let temp = tempfile::tempdir().unwrap();
            let collector = collector(temp.path());
            let mut seen_live = Value::Null;
            let paths: orq::consumption::SourcePaths =
                serde_json::from_value(v["sources"].clone()).unwrap_or_default();
            let resolved = Mutex::new(&mut seen_live);
            let result = orq::consumption::compute_with(
                Path::new(orq::text(&v["directory"])),
                &strings(&v["names"]),
                &v["aux"],
                orq::array(&v["live"]),
                &collector,
                |live| {
                    **resolved.lock().unwrap() = json!(live);
                    paths
                },
                |_provider, path, _until| rows(&v["rows"][path.to_string_lossy().as_ref()]),
            );
            drop(resolved);
            result.map(|(value,unavailable)|json!({"value":value,"unavailable":unavailable,"live":seen_live}))
        }
        "consumption.cache" => {
            let cache = Arc::new(orq::consumption::cache::Cache::default());
            let now = Arc::new(AtomicU64::new(0f64.to_bits()));
            let ticks = now.clone();
            let clock: orq::consumption::cache::Clock =
                Arc::new(move || f64::from_bits(ticks.load(Ordering::SeqCst)));
            let queue = Arc::new(Mutex::new(Vec::<orq::consumption::cache::Work>::new()));
            let count = Arc::new(AtomicUsize::new(0));
            let mut out = Vec::new();
            for step in orq::array(&v["steps"]) {
                if let Some(at) = step["now"].as_f64() {
                    now.store(at.to_bits(), Ordering::SeqCst);
                }
                let key = step["key"].as_str().unwrap_or("run");
                if step["finish"] == true {
                    let work = queue.lock().unwrap().remove(0);
                    work();
                    out.push(
                        json!({"count":count.load(Ordering::SeqCst),"running":cache.running(key)}),
                    );
                    continue;
                }
                let computed = step["result"].clone();
                let unavailable = step["unavailable"] == true;
                let failed = step["compute_error"].as_str().map(str::to_owned);
                let calls = count.clone();
                let pending = queue.clone();
                let (value, error) = cache.request(
                    key.into(),
                    clock.clone(),
                    step["ttl"].as_f64().unwrap_or(60.),
                    5.,
                    move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        if let Some(e) = failed {
                            Err(e)
                        } else {
                            Ok((computed, unavailable))
                        }
                    },
                    |work| {
                        if step["fail_spawn"] == true {
                            return Err(std::io::Error::other("no threads"));
                        }
                        if step["hold"] == true {
                            pending.lock().unwrap().push(work)
                        } else {
                            work()
                        };
                        Ok(())
                    },
                );
                out.push(json!({"value":value,"error":error,"count":count.load(Ordering::SeqCst),"running":cache.running(key),"queued":queue.lock().unwrap().len(),"len":cache.len()}));
            }
            Ok(json!(out))
        }
        _ => return None,
    })
}
