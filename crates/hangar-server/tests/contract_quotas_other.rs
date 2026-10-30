//! Cota das fontes que não são Claude nem Codex: cada caso de `contract_quotas_other_cases.rs` roda
//! pelo leitor contra um servidor que dá a resposta HTTP do caso, e a leitura e o pedido têm de sair
//! como estão escritos. A enumeração das fontes parte dos arquivos do caso. Chave ou cookie com
//! quebra de linha vira `credencial-invalida` sem rede, redirecionamento não é seguido (`http-<3xx>`)
//! e o Kimi recebe `User-Agent: hangar`. A comparação é do JSON serializado, então a ordem das chaves
//! e o `-0.0` contam. Processo próprio porque troca o TZ.
mod contract_quotas_other_cases;

use contract_quotas_other_cases::{NOW, READERS, SOURCES, TZ};
use hangar_server::accounts::environment::AccountEnvironment;
use hangar_server::accounts::quotas::other::{self, Endpoints, Reader};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn table(text: &str, total: usize) -> Vec<Value> {
    let rows: Vec<Value> = serde_json::from_str(text).unwrap();
    assert_eq!(rows.len(), total);
    rows
}

#[derive(Default)]
struct Fake {
    status: u16,
    body: Vec<u8>,
    location: Option<String>,
    seen: Vec<Value>,
}

/// Servidor HTTP mínimo: responde o caso atual e guarda caminho e cabeçalhos de cada pedido.
async fn serve(state: Arc<Mutex<Fake>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let state = state.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let mut lines = head.split("\r\n");
                let path = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_owned();
                let headers: BTreeMap<String, String> = lines
                    .filter_map(|line| line.split_once(": "))
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
                    .collect();
                let response = {
                    let mut fake = state.lock().unwrap();
                    fake.seen.push(json!({"path": path, "authorization": headers.get("authorization"),
                        "accept": headers.get("accept"), "user_agent": headers.get("user-agent"),
                        "cookie": headers.get("cookie")}));
                    let (mut status, mut body, mut location) = (fake.status, fake.body.clone(), fake.location.clone());
                    if path.starts_with("/login") {
                        (status, body, location) = (200, b"<html>Sign in to continue</html>".to_vec(), None);
                    }
                    let mut out = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
                    if let Some(location) = location {
                        out.push_str(&format!("location: {location}\r\n"));
                    }
                    out.push_str("\r\n");
                    let mut out = out.into_bytes();
                    out.extend(body);
                    out
                };
                let _ = stream.write_all(&response).await;
            });
        }
    });
    format!("http://{address}")
}

fn closed_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn readers_answer_each_case() {
    // SAFETY: processo próprio deste teste; ninguém mais lê o ambiente em paralelo.
    unsafe { std::env::set_var("TZ", TZ) };
    let state = Arc::new(Mutex::new(Fake::default()));
    let base = serve(state.clone()).await;
    let closed = closed_port();
    let client = other::client().unwrap();
    let mut failures = vec![];
    for case in &table(READERS, 56) {
        let name = case["name"].as_str().unwrap();
        // O Windows ignora o TZ do processo: data sem fuso sairia na hora local da máquina.
        if cfg!(windows) && name == "kimi_reset_formatos" {
            continue;
        }
        let response = &case["response"];
        {
            let mut fake = state.lock().unwrap();
            fake.status = response["status"].as_u64().unwrap_or(0) as u16;
            fake.body = match response["raw_hex"].as_str() {
                Some(raw) => hex(raw),
                None => response["body"].as_str().unwrap_or("").as_bytes().to_vec(),
            };
            fake.location = response["location"].as_str().map(str::to_owned);
            fake.seen.clear();
        }
        let origin = case["base"].as_str().map(str::to_owned).unwrap_or_else(|| {
            if case["refused"] == true { closed.clone() } else { base.clone() }
        });
        let text = |field: &str| case[field].as_str().unwrap().to_owned();
        let reader = match case["reader"].as_str().unwrap() {
            "kimi" => Reader::Kimi { key: text("key"), base: format!("{origin}{}", text("path")) },
            "commandcode" => Reader::CommandCode { key: text("key") },
            _ => Reader::OpenCode { workspace: text("workspace"), cookie: text("cookie") },
        };
        let endpoints = Endpoints {
            commandcode: format!("{origin}/alpha/billing/credits"),
            opencode: format!("{origin}/workspace/{{}}/go"),
        };
        let outcome = other::read(&client, &reader, &endpoints, NOW).await;
        let (value, unreachable) = (outcome.value, outcome.contact == other::Contact::Refused);
        let got = json!({"estado": value["estado"], "janelas": value["janelas"], "motivo": value["motivo"]});
        let seen = std::mem::take(&mut state.lock().unwrap().seen);
        let expected = &case["expected"];
        let requests = case["requests"].as_array().unwrap();
        if got.to_string() != expected.to_string() {
            failures.push(format!("{name}: leitura\n  esperado {expected}\n  rust   {got}"));
        }
        if unreachable != (case["refused"] == true) {
            failures.push(format!("{name}: recusa marcada {unreachable}"));
        }
        let fields = ["path", "authorization", "accept", "cookie", "user_agent"];
        let pick = |rows: &[Value]| -> Vec<Value> {
            rows.iter().map(|row| fields.iter().map(|f| (f.to_string(), row[*f].clone())).collect::<serde_json::Map<_, _>>().into()).collect()
        };
        if pick(&seen) != pick(requests) {
            failures.push(format!("{name}: pedido\n  esperado {:?}\n  rust   {:?}", pick(requests), pick(&seen)));
        }
    }
    assert!(failures.is_empty(), "{} diferença(s):\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn sources_load_each_case() {
    let mut failures = vec![];
    for case in &table(SOURCES, 12) {
        let name = case["name"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let mut base: BTreeMap<String, String> = BTreeMap::new();
        base.insert("HOME".into(), home.to_string_lossy().into());
        base.insert("USERPROFILE".into(), home.to_string_lossy().into());
        let kimi = match case["kimi_config_at"].as_str() {
            Some(at) => {
                base.insert("KIMI_CODE_HOME".into(), home.join(at).to_string_lossy().into());
                home.join(at)
            }
            None => home.join(".kimi-code"),
        };
        let engines = match case["engines_at"].as_str() {
            Some(at) => {
                base.insert("CP_ENGINES_FILE".into(), home.join(at).to_string_lossy().into());
                home.join(at)
            }
            None => claude.join("engines.json"),
        };
        for (key, value) in case["env"].as_object().unwrap() {
            base.insert(key.clone(), value.as_str().unwrap().into());
        }
        let files = [
            ("kimi_config", kimi.join("config.toml")),
            ("engines", engines),
            ("opencode", claude.join(".hangar-opencode.json")),
            ("opencode_legacy", claude.join(".claude-pocket-opencode.json")),
            ("apelidos", claude.join(".hangar-apelidos.json")),
            ("apelidos_legacy", claude.join(".claude-pocket-apelidos.json")),
        ];
        for (field, path) in files {
            if let Some(text) = case["files"][field].as_str() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        }
        let env = AccountEnvironment::from_map(base);
        let config = other::load(&env);
        let got = json!({
                "sources": config.sources.iter().map(|s| {
                    let mut row = s.row();
                    row["leitor"] = json!({"tipo": s.reader.kind(), "args": s.reader.args()});
                    row
                }).collect::<Vec<_>>(),
                "aliases": config.aliases,
            });
        // O motor que não é objeto sai sozinho; os outros continuam fontes.
        if name == "motor_nao_objeto_derruba" {
            let ids: Vec<&Value> = got["sources"].as_array().into_iter().flatten().map(|s| &s["id"]).collect();
            if ids != [&json!("chave:a")] {
                failures.push(format!("{name}: esperava só chave:a, rust {got}"));
            }
            continue;
        }
        let want = &case["expected"];
        if got.to_string() != want.to_string() {
            failures.push(format!("{name}:\n  esperado {want}\n  rust   {got}"));
        }
    }
    assert!(failures.is_empty(), "{} diferença(s):\n{}", failures.len(), failures.join("\n"));
}
