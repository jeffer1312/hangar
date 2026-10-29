//! Leitura do livro pelo shell, sem servidor e sem interpretar Python.
use super::store::{Store, truthy};
use serde_json::Value;
use std::io::Write;
use std::path::Path;

pub fn run(path: &Path, args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let command = args.first().map(String::as_str).unwrap_or("");
    if command == "path" {
        let _ = writeln!(out, "{}", path.display());
        return 0;
    }
    let store = Store::new(path.into());
    if !path.exists() && command == "config" {
        let id = args.get(1).map(String::as_str).unwrap_or("");
        let _ = writeln!(
            err,
            "erro: servidor '{id}' desconhecido — crie {} (ver peers.json.example)",
            path.display()
        );
        return 1;
    }
    let data = match store.read() {
        Ok(data) => data,
        Err(error) => {
            let _ = writeln!(err, "erro: {} inválido ({})", path.display(), error.msg);
            return 1;
        }
    };
    match command {
        "config" => {
            let id = args.get(1).map(String::as_str).unwrap_or("");
            let entry = data.get(id);
            let Some((base, token)) = entry
                .filter(|v| truthy(&v["base_url"]))
                .and_then(|v| Some((v["base_url"].as_str()?, v["token"].as_str()?)))
            else {
                let names = data
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    err,
                    "erro: servidor '{id}' não está em {} (conhecidos: {})",
                    path.display(),
                    if names.is_empty() { "nenhum" } else { &names }
                );
                return 1;
            };
            let _ = writeln!(out, "{}\t{token}", base.trim_end_matches('/'));
            0
        }
        "tsv" => {
            for (id, entry) in &data {
                if entry.get("app").is_some() && !truthy(&entry["base_url"]) {
                    continue;
                }
                if entry["enabled"] == false {
                    let _ = writeln!(
                        err,
                        "({id} desativado no peers.json — enderece direto se precisar)"
                    );
                    continue;
                }
                let (Some(base), Some(token)) =
                    (entry["base_url"].as_str(), entry["token"].as_str())
                else {
                    let _ = writeln!(
                        err,
                        "erro: {} inválido (entrada '{id}' sem endereço/token)",
                        path.display()
                    );
                    return 1;
                };
                let _ = writeln!(out, "{id}\t{}\t{token}", base.trim_end_matches('/'));
            }
            0
        }
        "json" => {
            let _ = writeln!(out, "{}", Value::Object(data));
            0
        }
        _ => {
            let _ = writeln!(err, "uso: hangar-server peers path|config <id>|tsv|json");
            2
        }
    }
}
pub fn cli(args: &[String]) -> std::process::ExitCode {
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let backend = crate::session_open::cli::backend_dir();
    let path = match crate::user_data::path(&env.home, backend.as_deref(), "peers.json") {
        Ok(Some(path)) => path,
        Ok(None) => env.home.join(".hangar/config/peers.json"),
        Err(_) => {
            eprintln!("erro: não foi possível resolver peers.json");
            return 1.into();
        }
    };
    run(&path, args, &mut std::io::stdout(), &mut std::io::stderr()).into()
}
