//! Adaptador de prova: somente serialização e chamadas às interfaces usadas pelo servidor.
use hangar_server::orchestration::{
    self as orq, markdown as md, panel, policy, roles, runs, timeline,
};
use serde_json::{Value, json};
#[path = "orq_contract/http.rs"]
mod http;
#[path="orq_contract/consumption.rs"]
mod consumption;
use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::Path,
};
fn txt<'a>(v: &'a Value, k: &str) -> &'a str {
    orq::text(&v[k])
}
fn strings(v: &Value) -> Vec<String> {
    orq::array(v).iter().map(|v| orq::text(v).into()).collect()
}
fn keys(v: &Value) -> Vec<String> {
    if let Some(s) = v.as_str() {
        vec![s.into()]
    } else {
        strings(v)
    }
}
fn entries(v: &Value) -> Vec<(Value, Value)> {
    orq::array(v)
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}
fn evaluate(action: &str, v: &Value) -> Result<Value, String> {
    let text = txt(v, "texto");
    let headers = strings(&v["cabecalho"]);
    Ok(match action {
        "identity.tmux"=>json!(orq::context::tmux_identity(&orq::py_string(&v["pid"]),v["started"].as_f64().unwrap_or_default(),txt(v,"session"),txt(v,"created"))),
        "context.promote-failure"=>{let source=Path::new(txt(v,"source"));let destination=Path::new(txt(v,"destination"));let (content,version)=orq::read(source).map_err(|e|e.to_string())?;
            orq::context::promote_source_with(source,destination,&content,version,version,|path,_text,_expected|{
                if v["external"]==true{std::fs::write(path,"external editor").map_err(|e|e.to_string())?;}Err("unable to write pointer".into())
            }).map_err(|e|e.message)?;Value::Null},
        "snapshot.cache"=>{let cache=orq::snapshot::SnapshotCache::<Value,Value>::new(v["evict_updated"]==true);let mut count=0;json!(orq::array(&v["steps"]).iter().map(|step|{let value=cache.get(Path::new(txt(step,"key")).to_owned(),step["signature"].clone(),||{count+=1;step["value"].clone()});json!({"value":value,"count":count,"len":cache.len()})}).collect::<Vec<_>>())},
        "runs.when"=>json!(runs::when(&v["value"]).map(|at|at.timestamp_micros()as f64/1e6)),
        "timeline.history"=>json!(runs::lines(Path::new(txt(v,"path")),false).map_err(|e|e.to_string())?.iter().flat_map(|obj|timeline::parse_obj(obj,Path::new(txt(v,"path")).parent())).collect::<Vec<_>>()),
        "context.resolve"=>orq::command("context",v)?,
        "context.write"=>{let c=&v["context"];let context=orq::context::Context{gid:txt(c,"gid").into(),grouped:c["grouped"]==true,prefix:txt(c,"session_prefix").into(),path:txt(c,"path").into()};json!(orq::context::write(Path::new(txt(v,"home")),Path::new(txt(v,"pair_directory")),&context,text,v["mtime"].as_f64().unwrap_or(0.))?)},
        "context.promote"=>{orq::context::promote(Path::new(txt(v,"home")),Path::new(txt(v,"pair_directory")),txt(v,"name"),txt(v,"gid"),v["mtime"].as_f64(),"tmux")?;Value::Null},
        "command" => orq::command(txt(v,"operation"), &v["arguments"])?,
        "timeline.path"=>json!(runs::timeline_path(Path::new(txt(v,"directory")))),
        "timeline.parse" => json!(timeline::parse_obj(&v["obj"], v["directory"].as_str().map(Path::new))),
        "panel.parts"=>{let(out,aux)=panel::build(Path::new(txt(v,"directory")),Path::new(txt(v,"home")));json!({"out":out,"aux":aux})},
        "panel.build" => {let (out,aux)=panel::build(Path::new(txt(v,"directory")),Path::new(txt(v,"home")));panel::with_timing(out,&aux,v["at"].as_f64().unwrap_or_else(runs::now))},
        "runs.root"=>json!(runs::root(Path::new(txt(v,"home")))),
        "runs.list"=>json!(runs::list_directory_with(Path::new(txt(v,"directory")),|path|{if path.file_name().is_some_and(|n|n==txt(v,"stat_error")){Err(io::Error::other("stat negado"))}else{std::fs::metadata(path).map(|m|m.is_dir())}})),
        "runs.detail"=>json!(runs::detail_directory(Path::new(txt(v,"directory")),txt(v,"name"))),
        "runs.cards"=>json!(runs::cards(orq::array(&v["runs"]))),
        "runs.phases"=>json!(strings(&v["gids"]).iter().map(|gid|runs::group_phase(Path::new(txt(v,"home")),gid)).collect::<Vec<_>>()),
        "runs.phase"=>json!(runs::group_phase(Path::new(txt(v,"home")),txt(v,"gid"))),
        "markdown.normalizar" => json!(md::normalize(txt(v, "celula"))),
        "markdown.limpar" => json!(md::clean(txt(v, "celula"))),
        "markdown.validar_celula" => json!(md::validate(txt(v, "valor"))?),
        "markdown.ler_tabela" => json!(md::table(text, &headers)),
        "markdown.trocar_linha" => json!(md::replace_row(
            text,
            &headers,
            &keys(&v["chave"]),
            &v["valores"],
            txt(v, "titulo_se_ausente")
        )?),
        "markdown.remover_linha" => json!(md::remove_row(text, &headers, &keys(&v["chave"]))),
        "markdown.inserir_coluna" => json!(md::insert_column(
            text,
            &headers,
            txt(v, "nome"),
            v["em"].as_u64().unwrap_or(0) as usize,
            txt(v, "valor")
        )),
        "markdown.trocar_secao" => {
            json!(md::replace_section(text, txt(v, "titulo"), txt(v, "corpo")))
        }
        "file.read" => json!(orq::read(Path::new(txt(v, "path"))).map_err(|e| e.to_string())?),
        "file.read-after-replace" => {
            let file = std::fs::File::open(txt(v, "path")).map_err(|e| e.to_string())?;
            std::fs::rename(txt(v, "replacement"), txt(v, "path")).map_err(|e| e.to_string())?;
            json!(orq::read_open(file).map_err(|e| e.to_string())?)
        }
        "file.write" => {
            let _lock = orq::context::WRITE_LOCK.lock().unwrap();
            json!(orq::write(
                Path::new(txt(v, "path")),
                text,
                v["mtime"].as_f64()
            )?)
        }
        "roles.cabecalho_atual" => json!(roles::headers(text)),
        "roles.ler" => json!(roles::read(text)),
        "roles.abertura_texto" => json!(roles::opening(
            &serde_json::from_value(v["p"].clone()).map_err(|e| e.to_string())?
        )),
        "roles.escrever_papel" => json!(roles::write(
            text,
            &serde_json::from_value(v["p"].clone()).map_err(|e| e.to_string())?
        )?),
        "roles.validar_janela" => {
            let r = roles::Role {
                janela: txt(v, "valor").into(),
                ..Default::default()
            };
            roles::write("", &r)?;
            json!(r.janela)
        }
        "roles.casar_viva" => json!(roles::live(
            &serde_json::from_value(v["papel"].clone()).map_err(|e| e.to_string())?,
            orq::array(&v["sessoes"])
        )),
        "roles.path" => orq::command("roles-file", v)?["path"].clone(),
        "policy.ler" => json!(policy::read(text)),
        "policy.name"=>json!(policy::claude_name(Path::new(txt(v,"base")),Path::new(txt(v,"path")))),
        "policy.read-file" => json!(policy::read(
            &orq::read(Path::new(txt(v, "path")))
                .map_err(|e| e.to_string())?
                .0
        )),
        "policy.inventory" => json!(policy::inventory_from(
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?
        )),
        "policy.quota" => json!(policy::quota_id(
            Path::new(txt(v, "home")),
            Path::new(txt(v, "base")),
            txt(v, "provider"),
            txt(v, "conta")
        )),
        "policy.migrate" => json!(policy::migrate(
            Path::new(txt(v, "home")),
            Path::new(txt(v, "base"))
        )),
        "policy.write" => {
            let _lock = orq::context::WRITE_LOCK.lock().unwrap();
            let path = Path::new(txt(v, "path"));
            let (content, _) = orq::read(path).map_err(|e| e.to_string())?;
            let row: Option<policy::Policy> =
                serde_json::from_value(v["policy"].clone()).map_err(|e| e.to_string())?;
            let next = policy::update(
                &content,
                row.as_ref(),
                txt(v, "conta"),
                orq::array(&v["inventory"]),
            )?;
            json!(orq::write(path, &next, v["mtime"].as_f64())?)
        }
        "policy.permitido" => {
            let rules: Vec<policy::Policy> =
                serde_json::from_value(v["politica"].clone()).map_err(|e| e.to_string())?;
            let kimi = json!({"models":[{"alias":v["modelo"],"efforts":v["levels"]}]});
            json!(policy::permitted(
                txt(v, "provider"),
                txt(v, "conta"),
                txt(v, "modelo"),
                txt(v, "esforco"),
                &rules,
                Some(&kimi)
            ))
        }
        "timeline.entry" => {
            if let Some(dir)=v["directory"].as_str(){json!(timeline::entry(&v["obj"],Some(Path::new(dir))))}
            else if v.get("shadow").is_some(){let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            std::fs::write(dir.path().join("jev-shadow.jsonl"), txt(v, "shadow")).map_err(|e| e.to_string())?;
            json!(timeline::entry(&v["obj"], Some(dir.path())))}
            else {json!(timeline::entry(&v["obj"],None))}
        }
        "panel._task_rows" => json!(panel::task_rows(
            orq::array(&v["evs"]),
            orq::array(&v["plan"]),
            &orq::array(&v["integrated"])
                .iter()
                .filter_map(orq::integer)
                .collect::<BTreeSet<_>>()
        )),
        "panel._timing_bounds" => panel::bounds(orq::array(&v["evs"])),
        "panel._team" => json!(panel::team(&v["cfg"], &v["st"], orq::array(&v["evs"]))),
        "panel._decision_rows" => json!(panel::decisions(
            &entries(&v["entries"]),
            orq::array(&v["evs"])
        )),
        "panel._automation" => {
            panel::automation(&v["cfg"], &entries(&v["entries"]), orq::array(&v["shadow"]))
        }
        "panel._integration" => {
            let mut closes = indexmap::IndexMap::new();
            for (k, value) in v["closes"].as_object().into_iter().flatten() {
                closes.insert(k.parse().map_err(|_| "Task inválida")?, value.clone());
            }
            panel::integration(orq::array(&v["evs"]), &closes, orq::array(&v["checks"]))
        }
        "panel._with_timing" => panel::with_timing(
            v["out"].clone(),
            &v["aux"],
            runs::when(&v["now"])
                .ok_or("relógio inválido")?
                .timestamp_micros() as f64
                / 1e6,
        ),
        "panel._first_sentence" => json!(timeline::first_sentence(txt(v, "body"))),
        _ => return http::evaluate(action,v).or_else(||consumption::evaluate(action,v)).unwrap_or_else(|| Err(format!("operação de prova desconhecida: {action}"))),
    })
}
fn answer(action: &str, value: &Value) -> Value {
    match evaluate(action, value) {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    }
}
fn main() {
    if std::env::args().nth(1).is_some_and(|arg|matches!(arg.as_str(),"list-panes"|"display-message")){eprintln!("no server running");std::process::exit(1)}
    let _=tracing_subscriber::fmt().with_writer(std::io::stderr).with_ansi(false).without_time().try_init();
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let value: Value = serde_json::from_str(&input).unwrap();
    let out = if let Some(cases) = value["cases"].as_array() {
        json!(
            cases
                .iter()
                .map(|case| answer(orq::text(&case["action"]), &case["input"]))
                .collect::<Vec<_>>()
        )
    } else {
        answer(orq::text(&value["action"]), &value["input"])
    };
    println!("{out}");
}
