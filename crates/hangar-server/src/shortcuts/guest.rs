//! Execução compartilhada: o login acrescenta o limite físico da pasta antes do terminal.
use super::terminal::{Service, Spec, Terminal};
use crate::{login_guests::store::Guest, routes::AppState};
use axum::{extract::Request, http::StatusCode, response::Response};
use hangar_api::session::SessionRow;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::Arc, time::Duration};

pub(super) fn error(status: u16, code: &str, msg: &str, params: Value) -> Response {
    crate::http_cors::json_guarded(StatusCode::from_u16(status).unwrap(), json!({"detail":{"code":code,"msg":msg,"params":params}}))
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str { v[key].as_str().unwrap_or("") }
fn fail(code: &str, msg: &str) -> Response { error(400, code, msg, json!({})) }
fn unavailable() -> Response { error(503,"shortcut_context_unavailable","contexto do terminal indisponível",json!({})) }

pub async fn display_environment(env: &crate::accounts::environment::AccountEnvironment) -> BTreeMap<String,String> {
    let mut result=env.base.clone();
    if !cfg!(windows) {
        let mut command=crate::terminal_input::child_command("systemctl");
        command.args(["--user","show-environment"]).env_clear().envs(&env.base).kill_on_drop(true);
        if let Ok(Ok(output))=tokio::time::timeout(Duration::from_secs(3),command.output()).await {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some((key,value))=line.split_once('=')
                    && ["DISPLAY","WAYLAND_DISPLAY","XDG_CURRENT_DESKTOP","HYPRLAND_INSTANCE_SIGNATURE"].contains(&key) {
                    result.entry(key.into()).or_insert_with(||value.into());
                }
            }
        }
    }
    result
}

// O porteiro de login mantém a chamada; a borda comum valida e executa depois da autorização.
pub async fn intercept(_: &Arc<AppState>, _: &Guest, _: &str, request: Request) -> Result<Request,Response> { Ok(request) }

pub(super) async fn execute(st: &Arc<AppState>, guest: Option<&Guest>, is_guest: bool, name: &str, row: Option<SessionRow>, run_code: bool, value: &Value) -> Result<Response,Response> {
    let command=text(value,"command");
    let mut shell=None;
    if run_code {
        if command.chars().count()>4096 { return Err(fail("erro_run_code_longo","comando longo demais")); }
        if command.contains('\0') { return Err(fail("erro_run_code_invalido","comando invalido")); }
        let key=text(value,"key");
        if !key.is_empty() && (!key.starts_with("run-code:") || !key.chars().all(|c|c.is_ascii_alphanumeric() || matches!(c,':'|'-'))) {
            return Err(fail("erro_run_code_invalido","identificador invalido"));
        }
        let language=crate::transcript::py::strip(text(value,"language")).to_lowercase();
        let unix=["bash","sh","zsh","fish"].contains(&language.as_str());
        let powershell=["powershell","ps1","pwsh"].contains(&language.as_str());
        if !language.is_empty() && !unix && !powershell && language!="shell" { return Err(fail("erro_run_code_linguagem","linguagem de terminal invalida")); }
        if cfg!(windows) && unix || !cfg!(windows) && powershell {
            return Err(error(409,"erro_run_code_shell_incompativel","o bloco nao combina com o sistema deste servidor",json!({"linguagem":language,"sistema":if cfg!(windows){"Windows"}else{"Linux"}})));
        }
        if !cfg!(windows) && !matches!(language.as_str(),""|"shell") || cfg!(windows) && language=="pwsh" {
            shell=find_shell(if cfg!(windows){"pwsh.exe"}else{&language},&st.accounts.env.base);
            if shell.is_none() { return Err(error(409,"erro_run_code_shell_ausente","interpretador nao instalado",json!({"linguagem":language}))); }
        }
    } else if text(value,"runs_in")=="hangar" && is_guest {
        return Err(error(403,"erro_shortcut_hangar_convidado","convidado nao roda atalho No Hangar",json!({})));
    }
    let cwd=row.and_then(|row|row.cwd).filter(|p|!p.is_empty()).ok_or_else(||error(404,"erro_sessao_inexistente","sessao nao encontrada",json!({})))?;
    let command=crate::transcript::py::strip(command).to_owned();
    if command.is_empty() { return Err(fail("erro_shortcut_vazio","comando vazio")); }
    let hangar=!run_code && text(value,"runs_in")=="hangar";
    let cwd=if hangar && value["home"].as_bool().unwrap_or(true) { st.accounts.env.home.clone() }
        else { super::project::folder(st,&cwd,if run_code {None}else{value["pasta"].as_str()}).await? };
    let cwd=match guest { Some(guest)=>confined_folder(&cwd,Path::new(&guest.root))?, None=>cwd };
    if let Some(missing)=super::has_placeholder(&command) {
        return Err(error(if run_code{400}else{422},"erro_shortcut_segredo",&format!("preencha a credencial {missing} antes de usar"),json!({"nome":missing})));
    }
    let service=service(st).await;
    let spec=Spec {
        owner:name.into(),cwd:cwd.to_string_lossy().into_owned(),command,
        label:if run_code{"Terminal"}else{text(value,"label")}.into(),key:text(value,"key").into(),origin:if hangar{name.into()}else{String::new()},
        ask:if run_code{true}else{value["ask"].as_bool().unwrap_or(true)},powershell:run_code,shell,
    };
    if hangar {
        if crate::transcript::py::strip(&spec.key).is_empty() { return Err(fail("erro_shortcut_sem_chave","atalho No Hangar sem chave")); }
        let (terminal,reused)=service.clone().start_hangar(spec).await.map_err(terminal_error)?;
        return hangar_started(st,&service,terminal.ok_or_else(shell_error)?,reused).await;
    }
    let terminal=service.clone().start(spec).await.map_err(|_|crate::http_cors::internal_error())?.ok_or_else(shell_error)?;
    started(&service,&terminal).await.map(|value|crate::http_cors::json_guarded(StatusCode::ACCEPTED,value))
}
pub(super) async fn restart(st:&AppState,id:String)->Result<Response,Response> {
    let service=service(st).await;
    let (terminal,reused)=service.clone().restart_hangar(id).await.map_err(terminal_error)?;
    let terminal=terminal.ok_or_else(||error(404,"erro_hangar_terminal_inexistente","terminal No Hangar nao encontrado",json!({})))?;
    hangar_started(st,&service,terminal,reused).await
}
fn terminal_error(error_value:super::terminal::Error)->Response {
    match error_value {
        super::terminal::Error::MuxUnavailable=>error(500,"erro_shortcut_mux_indisponivel","o multiplexador nao respondeu; tente de novo",json!({})),
        super::terminal::Error::Restart=>error(500,"erro_hangar_terminal_rodar_de_novo","nao foi possivel recuperar o comando do terminal No Hangar",json!({})),
        super::terminal::Error::Io(_)=>crate::http_cors::internal_error(),
    }
}
async fn hangar_started(st:&AppState,service:&Service,terminal:Terminal,reused:bool)->Result<Response,Response> {
    if reused {
        let focused=super::focus::focus(st,service,&terminal).await;
        return Ok(crate::http_cors::json_guarded(StatusCode::ACCEPTED,json!({"ok":true,"reused":true,"focused":focused,"terminal":{"id":terminal.id,"label":terminal.label,"alive":true,"exit_code":null}})));
    }
    let mut result=started(service,&terminal).await?;
    result["reused"]=json!(false);result["focused"]=json!(false);
    Ok(crate::http_cors::json_guarded(StatusCode::ACCEPTED,result))
}
fn shell_error()->Response { error(500,"erro_shortcut_shell","o multiplexador recusou criar o terminal",json!({})) }
pub(super) async fn service(st:&AppState)->Arc<Service> {
    Arc::new(Service {
        mux:Arc::new(crate::session_open::terminal::ProcessMux::default()),home:st.accounts.env.home.clone(),env:display_environment(&st.accounts.env).await,
        program:vec![st.list.env().capture_program.to_string_lossy().into_owned()],windows:cfg!(windows),encode:super::terminal_windows::encode,
        id:||crate::mods::state::random_hex(3),sequence:||std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
    })
}
async fn started(service:&Service,terminal:&Terminal)->Result<Value,Response> {
    let limit=tokio::time::Instant::now()+Duration::from_secs(2);
    let (alive,code)=loop {
        let state=service.status(&terminal.tmux).await;
        if !state.0 || tokio::time::Instant::now()>=limit { break state; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let public=json!({"id":terminal.id,"label":terminal.label,"alive":alive,"exit_code":code});
    if alive || code==Some(0) { return Ok(json!({"ok":true,"terminal":public})); }
    let output=service.output(&terminal.tmux).await;
    let lines:Vec<_>=output.trim().lines().rev().take(5).collect();
    let tail=lines.into_iter().rev().map(str::trim).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" | ");
    let tail=if tail.chars().count()>400 {format!("…{}",tail.chars().skip(tail.chars().count()-400).collect::<String>())}else{tail};
    let mut msg=format!("o comando saiu com o código {}",code.map(|v|v.to_string()).unwrap_or_else(||"None".into()));
    if !tail.is_empty() { msg.push_str(&format!(": {tail}")); }
    Err(error(422,"erro_shortcut_falhou",&msg,json!({"codigo":code,"saida":tail,"terminal":public})))
}
fn confined_folder(cwd:&Path,root:&Path)->Result<PathBuf,Response> {
    let real=std::fs::canonicalize(cwd).map_err(|_|unavailable())?;
    let root=std::fs::canonicalize(root).map_err(|_|unavailable())?;
    if !real.starts_with(&root) { return Err(error(403,"erro_fora_da_pasta","o convidado só abre sessão dentro da pasta dele",json!({}))); }
    Ok(real)
}
pub(super) fn find_shell(name:&str,env:&BTreeMap<String,String>)->Option<String> {
    std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or("")).map(|p|p.join(name)).find(|p| {
        if !p.is_file() { return false; }
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; p.metadata().is_ok_and(|m|m.permissions().mode()&0o111!=0) }
        #[cfg(not(unix))] { true }
    }).map(|p|p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod root_tests {
    #[test]
    fn registered_root_and_canonical_path_share_the_same_physical_boundary() {
        let home=tempfile::tempdir().unwrap();
        let root=home.path().join("convidado");let child=root.join("projeto");
        std::fs::create_dir_all(&child).unwrap();
        let registered=crate::login_guests::store::valid_root(root.to_str().unwrap()).unwrap();
        let real=std::fs::canonicalize(&child).unwrap();
        assert_eq!(super::confined_folder(&real,std::path::Path::new(&registered)).unwrap(),real);
        assert_eq!(super::confined_folder(home.path(),std::path::Path::new(&registered)).unwrap_err().status(),403);
    }
}
