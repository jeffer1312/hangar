//! Leitura do livro pelo shell, sem servidor e sem interpretar Python.
use super::store::Store;
use crate::orchestration::truthy;
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
            let Some(entry) = entry.filter(|value|truthy(&value["base_url"]))            else {
                let names = data
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                write_error(err,&format!("erro: servidor '{id}' não está em {} (conhecidos: {})",path.display(),if names.is_empty(){"nenhum"}else{&names}));
                return 1;
            };
            let (base,token)=match fields(entry){Ok(fields)=>fields,Err(unicode)=>return field_error(path,id,unicode,err)};
            let _ = writeln!(out, "{base}\t{token}");
            0
        }
        "tsv" => {
            for (id, entry) in &data {
                if entry.get("app").is_some() && !truthy(&entry["base_url"]) {
                    continue;
                }
                if entry["enabled"] == false {
                    write_error(err,&format!("({id} desativado no peers.json — enderece direto se precisar)"));
                    continue;
                }
                let (base,token)=match fields(entry){Ok(fields)=>fields,Err(unicode)=>return field_error(path,id,unicode,err)};
                if crate::orchestration::codec::contains_surrogate(&Value::String(id.clone())){return field_error(path,id,true,err);}
                let _ = writeln!(out, "{id}\t{base}\t{token}");
            }
            0
        }
        _ => {
            let _ = writeln!(err, "uso: hangar-server peers path|config <id>|tsv");
            2
        }
    }
}
fn write_error(err:&mut dyn Write,text:&str){
    let mut bytes=crate::orchestration::codec::encode_utf8_backslashreplace(text);bytes.push(b'\n');let _=err.write_all(&bytes);
}
fn fields(entry:&Value)->Result<(String,String),bool>{
    use crate::orchestration::codec;
    let base=entry.get("base_url").filter(|value|!codec::contains_nonfinite(value)).and_then(Value::as_str).ok_or(false)?;
    let token=entry.get("token").ok_or(false)?;
    if codec::contains_surrogate(&entry["base_url"]) || (token.is_string()&&codec::contains_surrogate(token)){return Err(true);}
    Ok((base.trim_end_matches('/').into(),codec::python_string(token)))
}
fn field_error(path:&Path,id:&str,unicode:bool,err:&mut dyn Write)->u8{
    let detail=if unicode{"texto contém Unicode inválido".into()}else{format!("entrada '{id}' sem endereço/token")};
    write_error(err,&format!("erro: {} inválido ({detail})",path.display()));1
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
