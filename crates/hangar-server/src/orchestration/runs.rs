//! Leitores do cofre; uma execução ilegível nunca apaga a informação de outra.
use super::{kind, text, truthy};
use chrono::{DateTime, Local, TimeZone};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::Duration,
};

pub fn root(home: &Path) -> PathBuf {
    home.join(".hangar/orq")
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && !matches!(id, "." | "..") && !id.contains(['/', '\\', ':', '\0'])
}
pub fn exec_dir(home: &Path, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| root(home).join(id))
}
pub fn timeline_path(dir: &Path) -> PathBuf {
    let resolved = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    dir.join(format!(
        "timeline-{}.jsonl",
        resolved.file_name().unwrap_or_default().to_string_lossy()
    ))
}
fn decode_json(raw: &[u8]) -> io::Result<Value> {
    let text = std::str::from_utf8(raw).map_err(|e|io::Error::new(io::ErrorKind::InvalidData,e))?;
    super::codec::loads(text).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData,serde_json::from_slice::<Value>(raw).err().map(|e|e.to_string()).unwrap_or_else(||"JSON inválido".into()))
    })
}
pub fn optional_json(path: &Path) -> io::Result<Option<Value>> {
    match fs::read(path) {
        Ok(raw) => decode_json(&raw).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn json(path: &Path) -> io::Result<Value> { decode_json(&fs::read(path)?) }
pub fn lines(path: &Path, strict: bool) -> io::Result<Vec<Value>> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound || !strict => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(
        crate::transcript::py::splitlines(&String::from_utf8_lossy(&raw))
            .iter()
            .filter_map(|l| super::codec::loads(l.trim()))
            .filter(Value::is_object)
            .collect(),
    )
}
pub fn events(path: &Path, strict: bool) -> io::Result<Vec<Value>> {
    Ok(lines(path, strict)?
        .into_iter()
        .filter(|e| {
            matches!(
                kind(e),
                "execucao_inicio"
                    | "task_inicio"
                    | "entrega"
                    | "veredito"
                    | "sessao_trocada"
                    | "execucao_fim"
            )
        })
        .collect())
}
pub fn current_end(events: &[Value]) -> Option<&Value> {
    let mut end = None;
    for e in events {
        if kind(e) == "execucao_fim" {
            end = Some(e)
        } else if super::integer(&e["task"]).is_some() {
            end = None
        }
    }
    end
}
pub fn now() -> f64 {
    Local::now().timestamp_micros() as f64 / 1e6
}
pub fn when(value: &Value) -> Option<DateTime<Local>> {
    let s = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    let (micros, offset) = crate::transcript::py::fromisoformat(&s)?;
    Local
        .timestamp_micros(
            micros
                - offset.unwrap_or_else(|| {
                    i64::from(Local::now().offset().local_minus_utc()) * 1_000_000
                }),
        )
        .single()
}
pub fn iso(value: &Value) -> Value {
    when(value)
        .map(|t| json!(t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false)))
        .unwrap_or(Value::Null)
}
pub fn pid_alive(value: &Value) -> bool {
    if !cfg!(target_os = "linux") || super::integer(value).is_none() {
        true
    } else {
        PathBuf::from(format!("/proc/{value}")).exists()
    }
}
pub fn units() -> Option<Value> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let output = super::bounded(
        "systemctl",
        &[
            "--user",
            "show",
            "vigia-*",
            "--timestamp=unix",
            "-p",
            "Id,ExecStart,ActiveState,ActiveEnterTimestamp,NRestarts",
        ],
        None,
        Duration::from_secs(3),
    );
    let out = match output {
        Ok(out) if out.status.success() => out,
        _ => return None,
    };
    let mut result = serde_json::Map::new();
    let rx = regex::Regex::new(r"(?:^|\s)(?:-e|--eventos)\s+(\S+)").unwrap();
    for block in String::from_utf8_lossy(&out.stdout).split("\n\n") {
        let mut props = serde_json::Map::new();
        for line in block.lines() {
            if let Some((k, v)) = line.split_once('=') {
                props.insert(k.to_owned(), json!(v));
            }
        }
        if let Some(m) = rx.captures(text(&props.get("ExecStart").cloned().unwrap_or_default())) {
            let key = super::expand(Path::new(m.get(1).unwrap().as_str()), &super::home())
                .to_string_lossy()
                .into_owned();
            if result
                .get(&key)
                .is_none_or(|v: &Value| v["ActiveState"] != "active")
            {
                result.insert(key, Value::Object(props));
            }
        }
    }
    Some(Value::Object(result))
}
pub fn watchdog(dir: &Path, units: Option<&Value>, at: f64) -> Value {
    let hb = json(&dir.join("vigia.json")).ok();
    let unit = units.and_then(|v| v.get(dir.to_string_lossy().as_ref()));
    watchdog_from(hb, unit, units.is_some(), at)
}
/// A lista conserva a última linha boa quando o batimento está no meio da escrita.
pub fn heartbeat(dir: &Path, at: f64) -> io::Result<Option<Value>> {
    let Some(hb) = optional_json(&dir.join("vigia.json"))? else {
        return Ok(None);
    };
    Ok((watchdog_from(Some(hb.clone()), None, false, at)["alive"] == true).then_some(hb))
}
fn watchdog_from(hb: Option<Value>, u: Option<&Value>, units_known: bool, at: f64) -> Value {
    let hb = hb.filter(|h| h.is_object() && when(&h["ts"]).is_some());
    let state = u.map(|u| u["ActiveState"].clone()).unwrap_or_default();
    let (alive, source) = if let Some(h) = &hb {
        let iv = h["interval_s"].as_i64().filter(|i| *i >= 0).unwrap_or(60);
        let fresh =
            at - when(&h["ts"]).unwrap().timestamp_micros() as f64 / 1e6 <= iv as f64 * 2. + 30.;
        (
            fresh
                && (state.is_null() || state == "active" || state == "activating")
                && pid_alive(&h["pid"]),
            "heartbeat",
        )
    } else if let Some(u) = u {
        (u["ActiveState"] == "active", "systemd")
    } else {
        (false, if units_known { "none" } else { "unavailable" })
    };
    let h = hb.unwrap_or_default();
    let since = u
        .and_then(|u| text(&u["ActiveEnterTimestamp"]).strip_prefix('@'))
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(|s| Local.timestamp_opt(s, 0).single())
        .map(|t| json!(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)))
        .unwrap_or_default();
    let restarts = u
        .and_then(|u| u["NRestarts"].as_str())
        .and_then(|s| s.parse::<i64>().ok());
    let unit = u
        .map(|v| text(&v["Id"]))
        .filter(|s| !s.is_empty())
        .map(|s| json!(s.strip_suffix(".service").unwrap_or(s)))
        .unwrap_or_else(|| h["unit"].clone());
    json!({"alive":alive,"last_cycle":h["ts"],"since":since,"restarts":restarts,"unit":unit,"unit_state":state,"arbiter":h["arbiter"],"watching":h["watching"].as_array().map(|a|a.iter().map(super::py_string).collect::<Vec<_>>()).unwrap_or_default(),"source":source})
}
pub fn auto_run(dir: &Path, strict: bool) -> io::Result<Option<(Value, String, bool)>> {
    let cfg = match json(&dir.join("orq.json")) {
        Ok(c) => c,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                || e.kind() == io::ErrorKind::NotADirectory
                || e.kind() == io::ErrorKind::InvalidData
                || !strict =>
        {
            return Ok(None);
        }
        Err(e) => {
            if fs::read(dir.join("orq.json")).is_ok() {
                return Ok(None);
            }
            return Err(e);
        }
    };
    if cfg["auto"] != true {
        return Ok(None);
    }
    let ev = events(&dir.join("eventos.jsonl"), strict)?;
    Ok(auto_entry(cfg, &ev))
}
/// A descoberta da lista distingue JSON incompleto de uma execução encerrada.
pub fn list_run(dir: &Path) -> io::Result<Option<(Value, String, bool)>> {
    let Some(cfg) = optional_json(&dir.join("orq.json"))? else {
        return Ok(None);
    };
    if cfg["auto"] != true {
        return Ok(None);
    }
    let events = events(&dir.join("eventos.jsonl"), true)?;
    Ok(auto_entry(cfg, &events))
}
fn auto_entry(cfg: Value, events: &[Value]) -> Option<(Value, String, bool)> {
    if cfg["auto"] != true {
        return None;
    }
    let gid = events
        .iter()
        .find(|e| kind(e) == "execucao_inicio")
        .map(|e| text(&e["gid"]))
        .unwrap_or("");
    (!gid.is_empty()).then(|| (cfg, gid.to_owned(), current_end(events).is_some()))
}

pub fn group_phase(home: &Path, gid: &str) -> Option<String> {
    static WARNED: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);
    let unknown=|path:PathBuf|{if WARNED.lock().unwrap().insert(path.clone()){
        tracing::warn!(code="orq_run_unreadable",run=%path.file_name().unwrap_or_default().to_string_lossy(),"orq: execuções ilegíveis, grupo mantido");
    }Some("unknown".into())};
    let root=root(home);
    let paths=match fs::read_dir(&root){Ok(d)=>d,Err(e)if e.kind()==io::ErrorKind::NotFound=>return None,Err(_)=>return unknown(root)};
    let mut paths=match paths.map(|d|d.map(|e|e.path())).collect::<io::Result<Vec<_>>>(){Ok(p)=>p,Err(_)=>return unknown(root)};
    paths.sort();let mut ended=false;let mut live=false;
    for d in paths {match auto_run(&d,true){
        Ok(Some((_,g,end))) if g==gid=>{ended|=end;live|=!end},
        Err(_)=>return unknown(d),_=>(),
    }}
    WARNED.lock().unwrap().clear();
    if live {Some("live".into())}else{ended.then(||"ended".into())}
}

pub fn activity(timeline: &Path, at: f64) -> (&'static str, Option<f64>) {
    let lock = timeline
        .parent()
        .unwrap_or(Path::new(""))
        .join("advance.lock");
    let last = [timeline, &lock]
        .iter()
        .filter_map(|p| {
            fs::metadata(p)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64())
        })
        .reduce(f64::max);
    let mut held = false;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(meta), Ok(raw)) = (fs::metadata(&lock), fs::read_to_string("/proc/locks")) {
            held = raw.lines().any(|l| {
                let f: Vec<_> = l.split_whitespace().collect();
                f.len() > 5 && f[1] == "FLOCK" && f[5].ends_with(&format!(":{}", meta.ino()))
            })
        }
    }
    (
        if held || last.is_some_and(|t| at - t < 120.) {
            "working"
        } else {
            "idle"
        },
        last,
    )
}
pub fn active(home: &Path, at: f64) -> io::Result<Vec<Value>> {
    let mut out = Vec::new();
    let dirs = match fs::read_dir(root(home)) {
        Ok(x) => x,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for d in dirs {
        let d = d?.path();
        if let Some((cfg, gid, false)) = auto_run(&d, false)? {
            let wd = watchdog(&d, None, at);
            if wd["alive"] == true {
                let tl = timeline_path(&d);
                let (state, last) = activity(&tl, at);
                out.push(json!({"name":format!("{gid}-orq"),"gid":gid,"repo":cfg["repo"],"arbiter":if truthy(&wd["arbiter"]){wd["arbiter"].clone()}else{cfg["arbiter"].clone()},"timeline":tl,"state":state,"last_activity":last}));
            }
        }
    }
    out.sort_by(|a, b| text(&a["timeline"]).cmp(text(&b["timeline"])));
    Ok(out)
}
pub fn summary(id: &str, ev: &[Value]) -> Value {
    let begin = ev
        .iter()
        .find(|e| kind(e) == "execucao_inicio")
        .cloned()
        .unwrap_or_default();
    let end = current_end(ev);
    let mut by: BTreeMap<super::Integer, Vec<Value>> = BTreeMap::new();
    let mut loose = Vec::new();
    for e in ev {
        if let Some(n) = super::integer(&e["task"]) {
            by.entry(n).or_default().push(e.clone());
        } else if !matches!(kind(e), "execucao_inicio" | "execucao_fim") {
            loose.push(e.clone())
        }
    }
    let mut tasks = Vec::new();
    let mut returns = 0;
    let mut firsts = 0;
    for (n, items) in by {
        let start = items
            .iter()
            .find(|e| kind(e) == "task_inicio")
            .cloned()
            .unwrap_or_default();
        let verdicts: Vec<_> = items.iter().filter(|e| kind(e) == "veredito").collect();
        let final_v = verdicts.iter().rev().find(|e| e["fase"] != "codigo");
        let rounds = items
            .iter()
            .filter(|e| kind(e) == "entrega" || (kind(e) == "veredito" && e["fase"] != "codigo"))
            .filter_map(|e| super::integer(&e["rodada"]))
            .max()
            .unwrap_or_default();
        returns += verdicts
            .iter()
            .filter(|e| matches!(text(&e["resultado"]), "devolvido" | "reprova" | "corrige"))
            .count();
        if final_v.is_some_and(|e| e["resultado"] == "aprova") && rounds == 1 {
            firsts += 1
        }
        tasks.push(json!({"task":n,"titulo":super::py_string(start.get("titulo").unwrap_or(&json!(""))),"executor":super::py_string(start.get("executor").unwrap_or(&json!(""))),"par":super::py_string(start.get("par").unwrap_or(&json!(""))),"rodadas":rounds,"resultado":final_v.map(|e|e["resultado"].clone()),"inicio":start["ts"],"fim":final_v.map(|e|e["ts"].clone()),"eventos":items}));
    }
    json!({"id":id,"plano":super::py_string(begin.get("plano").unwrap_or(&json!(""))),"branch":super::py_string(begin.get("branch").unwrap_or(&json!(""))),"gid":super::py_string(begin.get("gid").unwrap_or(&json!(""))),"inicio":begin["ts"],"fim":end.map(|e|e["ts"].clone()),"resultado":end.map(|e|e["resultado"].clone()),"tasks":tasks,"eventos_execucao":loose,"voltas":returns,"aprovadas_primeira":firsts,"reconstruida":truthy(&begin["reconstruido"])})
}
pub fn list(home:&Path)->Vec<Value>{list_directory(&root(home))}
pub fn list_directory(root:&Path)->Vec<Value>{
    list_directory_with(root,|path|match fs::metadata(path){Ok(m)=>Ok(m.is_dir()),Err(e)if e.kind()==io::ErrorKind::NotFound=>Ok(false),Err(e)=>Err(e)})
}
/// Uma entrada ilegível não oculta as demais execuções.
pub fn list_directory_with(root:&Path,inspect:impl Fn(&Path)->io::Result<bool>)->Vec<Value>{
    let mut out=Vec::new();
    if let Ok(dirs)=fs::read_dir(root){for d in dirs.flatten(){let path=d.path();
        match inspect(&path){Ok(true)=>{},Ok(false)=>continue,Err(_)=>{tracing::warn!(code="orq_run_stat_failed",run=%d.file_name().to_string_lossy(),"não foi possível inspecionar a execução");continue}}
        let event_path=path.join("eventos.jsonl");
        let ev=events(&event_path,false).unwrap_or_default();
        if !ev.is_empty(){out.push(summary(&d.file_name().to_string_lossy(),&ev));}
        else if event_path.exists(){tracing::warn!(code="orq_run_empty",run=%d.file_name().to_string_lossy(),"eventos sem linha válida; execução fora da lista");}
    }}
    out.sort_by(|a,b|text(&b["id"]).cmp(text(&a["id"])));out
}
pub fn detail(home:&Path,id:&str)->Option<Value>{detail_directory(&root(home),id)}
pub fn detail_directory(root:&Path,id:&str)->Option<Value>{
    if !valid_id(id){return None}let ev=events(&root.join(id).join("eventos.jsonl"),false).ok()?;
    (!ev.is_empty()).then(||summary(id,&ev))
}

pub fn cards(runs: &[Value]) -> Vec<Value> {
    let mut cards: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
    for r in runs {
        for t in super::array(&r["tasks"]) {
            let reviewer = text(&t["par"]);
            if reviewer.is_empty() {
                continue;
            }
            let c=cards.entry(reviewer.to_owned()).or_insert(json!({"par":reviewer,"aceitas":0,"nao_aceitas":0,"aprovadas_primeira":0,"_rod":0,"_rod_n":0}));
            let accepted = t["resultado"] == "aprova";
            super::increment(c, if accepted { "aceitas" } else { "nao_aceitas" }, 1);
            let rnd = super::integer(&t["rodadas"]).unwrap_or_default();
            if accepted && rnd == 1 {
                super::increment(c, "aprovadas_primeira", 1)
            }
            if rnd.0 >= num_bigint::BigInt::from(1) {
                c["_rod"] = json!(super::Integer(super::integer(&c["_rod"]).unwrap_or_default().0 + rnd.0));
                super::increment(c,"_rod_n",1);
            }
        }
    }
    let mut out: Vec<_> = cards
        .into_values()
        .map(|mut c| {
            let n = c["_rod_n"].as_u64().unwrap_or(0);
            c["rodadas_media"] = json!(if n > 0 {
                super::round(super::positive_mean(&super::integer(&c["_rod"]).unwrap_or_default(),n), 1)
            } else {
                0.
            });
            c.as_object_mut().unwrap().remove("_rod");
            c.as_object_mut().unwrap().remove("_rod_n");
            c
        })
        .collect();
    out.sort_by_key(|c| {
        -(c["aceitas"].as_i64().unwrap_or(0) + c["nao_aceitas"].as_i64().unwrap_or(0))
    });
    out
}
