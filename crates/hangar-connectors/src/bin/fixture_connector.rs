//! Conector de prova dos testes do host. Comportamento escolhido pelos argumentos:
//! `--id <id>`, `--title <texto>`, `--pid-file <arquivo>` (acrescenta o PID de cada vida),
//! `--env-file <arquivo>` (grava os nomes do ambiente), `--grandchild <arquivo>` (lança um neto e
//! grava o PID dele), `--hang <método>`, `--exit <método>`, `--garbage <método>`, `--wrong-id <método>`,
//! `--big <método>`, `--fail <método>=<código>`, `--claim-id <id>`, `--ignore-shutdown`.
//! `--received-file <arquivo>` registra pedidos; `--invalid-result <método>` devolve dados inválidos.
//! `--review on` oferece `work_review` (revisão `r-1` em espera; `sem-revisao` não tem revisão);
//! `--bad-asset on` manda bytes de JPEG como PNG; `--init-file <arquivo>` grava os parâmetros do `initialize`.
//! `--review-file <arquivo>` lê a revisão do arquivo a cada `get` (o `decide` confere o `id` dela);
//! `--delay <método>=<ms>` atrasa a resposta; `--calls-file <arquivo>` grava método e parâmetros de cada
//! pedido, um JSON por linha; `--plain-key on` responde o `work_tracking.get` com `key` igual ao ID pedido.
use base64::Engine as _;
use hangar_connectors::protocol::{self, ConnectorError, ErrorCode};
use hangar_connectors::rpc;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write as _;
use tokio::io::{AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut options: HashMap<String, Vec<String>> = HashMap::new();
    let mut flags = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--ignore-shutdown" { flags.push(arg); continue; }
        let value = iter.next().unwrap_or_default();
        options.entry(arg).or_default().push(value);
    }
    let one = |name: &str| options.get(name).and_then(|values| values.first()).cloned();
    let has = |name: &str, method: &str| options.get(name).is_some_and(|values| values.iter().any(|value| value == method));
    let id = one("--id").unwrap_or_else(|| "fixture".into());
    if let Some(path) = one("--pid-file") {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        writeln!(file, "{}", std::process::id()).unwrap();
    }
    if let Some(path) = one("--env-file") {
        let names: Vec<String> = std::env::vars_os().map(|(name, _)| name.to_string_lossy().into_owned()).collect();
        std::fs::write(path, names.join("\n")).unwrap();
    }
    if let Some(path) = one("--grandchild") {
        #[cfg(unix)]
        let child = std::process::Command::new("sleep").arg("1000").spawn().unwrap();
        #[cfg(windows)]
        let child = std::process::Command::new("ping").args(["-n", "1000", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::fs::write(path, child.id().to_string()).unwrap();
        std::mem::forget(child);
    }
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    while let Ok(Some(line)) = rpc::read_message(&mut stdin).await {
        let request: Value = serde_json::from_slice(&line).unwrap();
        let method = request["method"].as_str().unwrap_or("").to_owned();
        let mut rid = request["id"].clone();
        if let Some(path) = one("--received-file") {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(file, "{method}").unwrap();
        }
        if let Some(path) = one("--calls-file") {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(file, "{}", json!({"method": method, "params": request["params"]})).unwrap();
        }
        let delay = options.get("--delay").and_then(|values| values.iter().find_map(|value| {
            let (name, ms) = value.split_once('=')?;
            (name == method).then(|| ms.parse::<u64>().ok()).flatten()
        }));
        if let Some(ms) = delay { tokio::time::sleep(std::time::Duration::from_millis(ms)).await; }
        let from_file = one("--review-file").map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap());
        if has("--invalid-result", &method) {
            rpc::write_message(&mut stdout, &protocol::success(&rid, json!({}))).await.unwrap();
            continue;
        }
        if has("--hang", &method) { std::future::pending::<()>().await; }
        if has("--exit", &method) { std::process::exit(3); }
        if has("--garbage", &method) {
            stdout.write_all(b"texto solto no stdout\n").await.unwrap();
            stdout.flush().await.unwrap();
            continue;
        }
        if has("--wrong-id", &method) { rid = json!(rid.as_u64().unwrap_or(0) + 1_000_000); }
        if method == "initialize" && let Some(path) = one("--init-file") {
            std::fs::write(path, request["params"].to_string()).unwrap();
        }
        if method=="initialize" && one("--strict-initialize").is_some()
            && request["params"].as_object().is_some_and(|params|params.keys().any(|key|!matches!(key.as_str(),"protocol_version"|"host"))) {
            let reply=protocol::failure(&rid,&ConnectorError::invalid_params("parâmetro de inicialização desconhecido"));
            rpc::write_message(&mut stdout,&reply).await.unwrap();continue;
        }
        let failure = options.get("--fail").and_then(|values| values.iter().find_map(|value| {
            let (name, code) = value.split_once('=')?;
            (name == method).then(|| ErrorCode::parse(code)).flatten()
        }));
        let response = if let Some(code) = failure {
            protocol::failure(&rid, &ConnectorError::new(code, format!("falha simulada: {code}")))
        } else {
            match method.as_str() {
                "initialize" => {
                    let mut capabilities = json!({"work_tracking": 1});
                    if one("--actions").is_some() { capabilities["work_actions"] = json!(1); }
                    if one("--review").is_some() { capabilities["work_review"] = json!(1); }
                    protocol::success(&rid, json!({"protocol_version": 1, "id": one("--claim-id").unwrap_or_else(|| id.clone()), "capabilities": capabilities}))
                }
                "work_review.get" if from_file.is_some() => protocol::success(&rid, json!({"review": from_file})),
                "work_review.decide" if from_file.is_some() => if from_file.as_ref().is_some_and(|current| request["params"]["review_id"] == current["id"]) {
                    protocol::success(&rid, json!({"ok": true}))
                } else {
                    protocol::failure(&rid, &ConnectorError::new(ErrorCode::StaleReview, "a revisão mudou"))
                },
                "work_review.get" if request["params"]["ticket_id"] == "sem-revisao" => protocol::success(&rid, json!({"review": null})),
                "work_review.get" => protocol::success(&rid, json!({"review": review()})),
                "work_review.asset" => {
                    let bytes: &[u8] = if one("--bad-asset").is_some() { b"\xff\xd8\xff\xe0\0\x10JFIF" } else { b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" };
                    protocol::success(&rid, json!({"mime": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(bytes)}))
                }
                // O erro vai cru, como um conector de fora o mandaria.
                "work_review.decide" if request["params"]["review_id"] != "r-1" => json!({"jsonrpc": "2.0", "id": rid,
                    "error": {"code": -32000, "message": "a revisão mudou", "data": {"code": "stale_review"}}}),
                "work_review.decide" => protocol::success(&rid, json!({"ok": true})),
                "work_actions.list" => protocol::success(&rid, json!({"actions": [{"id": "plan", "label": "Planejar o ticket",
                    "description": "Preparar o trabalho", "prompt_template": "/iniciar {key}", "stage": "planejamento"}]})),
                "work_tracking.list" => {
                    let page: u32 = request["params"]["cursor"].as_str().and_then(|c| c.strip_prefix("pagina-")).and_then(|n| n.parse().ok()).unwrap_or(1);
                    let limit = request["params"]["limit"].as_u64().unwrap_or(1).min(3) as u32;
                    let title = if has("--big", &method) { "x".repeat(5 * 1024 * 1024) } else { one("--title").unwrap_or_else(|| "Tarefa de prova".into()) };
                    let items: Vec<Value> = (1..=limit).map(|n| ticket(&format!("cartão:{page}/{n}"), &title)).collect();
                    let next = (page < 2).then(|| format!("pagina-{}", page + 1));
                    protocol::success(&rid, json!({"items": items, "next_cursor": next, "total": null}))
                }
                "work_tracking.get" => {
                    let ticket_id = request["params"]["ticket_id"].as_str().unwrap_or("").to_owned();
                    let mut detail = ticket(&ticket_id, "Detalhe de prova");
                    if one("--plain-key").is_some() { detail["key"] = json!(ticket_id); }
                    if let Some(size)=one("--detail-url-size").and_then(|n|n.parse::<usize>().ok()) {
                        detail["url"]=json!(format!("https://example.invalid/{}","x".repeat(size.min(16384))));
                    }
                    detail["description_markdown"] = json!("**negrito** e `código`");
                    protocol::success(&rid, detail)
                }
                "shutdown" => {
                    if flags.iter().any(|flag| flag == "--ignore-shutdown") { protocol::success(&rid, Value::Null) } else {
                        let _ = rpc::write_message(&mut stdout, &protocol::success(&rid, Value::Null)).await;
                        return;
                    }
                }
                _ => protocol::failure(&rid, &ConnectorError::new(ErrorCode::UnsupportedCapability, "método desconhecido")),
            }
        };
        // A mensagem grande passa direto: o teto é do host conferir.
        if has("--big", &method) {
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            if stdout.write_all(&bytes).await.is_err() { return; }
            let _ = stdout.flush().await;
            continue;
        }
        if rpc::write_message(&mut stdout, &response).await.is_err() { return; }
    }
    if flags.iter().any(|flag| flag == "--ignore-shutdown") { std::future::pending::<()>().await; }
}

fn review() -> Value {
    let side = |asset: &str| json!({"label": "Mock", "asset": asset, "width": 160, "height": 90, "box": [10, 10, 40, 20]});
    json!({
        "id": "r-1", "title": "Revisão de prova", "updated_at": "2026-11-18T10:00:00Z", "state": "waiting",
        "decisions": [{"id": "47", "title": "Diferença de prova", "left": side("a1"), "right": side("a2"),
            "choices": [{"id": "igualar", "label": "Igualar"}, {"id": "manter", "label": "Manter", "requires_reason": true}]}]
    })
}

fn ticket(id: &str, title: &str) -> Value {
    json!({
        "id": id, "key": format!("#{id}"), "title": title, "url": format!("https://fixture.invalid/c/{}", id.len()),
        "status": {"id": "7", "name": "Fazendo", "category": "in_progress"}, "assignee": {"id": "u-1", "name": "Pessoa"},
        "project": null, "updated_at": "2026-11-12T13:00:00Z"
    })
}
