//! Orquestração: contratos, histórico e linha do tempo lidos no próprio servidor.
pub mod body;
mod body_encoding;
mod body_numbers;
mod common;
pub mod codec;
pub mod markdown;
pub use common::*;
pub mod consumption;
pub mod context;
mod http;
pub mod panel;
pub mod plan;
pub mod policy;
pub mod roles;
pub mod runs;
pub mod state;
pub mod snapshot;
pub mod timeline;
pub mod triage;
pub use http::{matches, serve};

/// Leitores puros da CLI; stdin e stdout JSON conservam uma implementação para tela e ferramenta.
pub fn command(operation: &str, input: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let home = input["home"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(home);
    let dir = std::path::Path::new(text(&input["directory"]));
    let pair = input["pair_directory"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join(".hangar-pair")
        });
    let name = text(&input["name"]);
    let mux = input["mux"].as_str().unwrap_or("tmux");
    match operation {
        "section" => Ok(json!(plan::section(
            text(&input["text"]),
            text(&input["name"])
        ))),
        "at-or-after" => Ok(json!(state::at_or_after(&input["ts"], &input["reference"]))),
        "until-future" => Ok(json!(
            crate::transcript::py::fromisoformat(text(&input["deadline"]))
                .filter(|(_, offset)| offset.is_some())
                .and_then(|_| runs::when(&input["deadline"]))
                .is_some_and(|t| t.timestamp_micros() as f64 / 1e6 > runs::now())
        )),
        "normalize" => Ok(json!(markdown::normalize(text(&input["text"])))),
        "team-rows" => Ok(json!(roles::cli_rows(text(&input["text"])))),
        "active-gid" => context::active_gid(&home, name, mux).map(|v| json!(v)),
        "context" => {
            let c = context::resolve(&home, &pair, name, mux)?;
            let (content, mtime) = read(&c.path).map_err(|e| e.to_string())?;
            Ok(
                json!({"gid":c.gid,"grouped":c.grouped,"prefix":c.prefix,"path":c.path,
                "text":content,"mtime":mtime,"roles":roles::read(&content),"link":context::link(&pair,name)}),
            )
        }
        "roles" => Ok(json!(roles::read(text(&input["text"])))),
        "roles-file" => {
            let gid = text(&input["gid"]);
            if !runs::valid_id(gid) {
                return Err("grupo inválido".into());
            }
            let path = pair.join(format!("regras-{gid}.md"));
            let (content, mtime) = read(&path).map_err(|e| e.to_string())?;
            Ok(json!({"path":path,"text":content,"mtime":mtime,"roles":roles::read(&content)}))
        }
        "identity" => context::identity(
            &home,
            text(&input["name"]),
            input["mux"].as_str().unwrap_or("tmux"),
        )
        .map(|v| json!(v)),
        "find-plan" => Ok(plan::find(std::path::Path::new(text(&input["repo"])))),
        "plan-tasks" => Ok(json!(plan::tasks(text(&input["text"])))),
        "state" => state::read(dir),
        "events" => runs::lines(&dir.join("eventos.jsonl"), true)
            .map(|v| json!(v))
            .map_err(|e| e.to_string()),
        "closes" => state::closes(dir).map(|v| {
            json!(
                v.into_iter()
                    .map(|(n, v)| (n.to_string(), v))
                    .collect::<std::collections::BTreeMap<_, _>>()
            )
        }),
        "closed" => state::closed(dir).map(|v| {
            json!(
                v.into_iter()
                    .map(|(n, v)| (n.to_string(), v["ts"].clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            )
        }),
        "outcome" => Ok(json!(state::outcome(
            array(&input["events"]),
            integer(&input["task"]).ok_or("Task inválida")?,
            &input["close"]
        ))),
        "plan-sha" => Ok(json!(plan::sha(text(&input["text"])))),
        "triage" => {
            let (verdict, category) = triage::decide(text(&input["text"]));
            Ok(json!([verdict, category]))
        }
        "run-metadata" => Ok(plan::metadata(dir, text(&input["plan"]), &home)),
        _ => Err("operação de orquestração desconhecida".into()),
    }
}
#[cfg(test)]
mod tests;
