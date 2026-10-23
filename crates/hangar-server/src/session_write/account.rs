//! `GET` e `POST /api/sessions/{nome}/conta` do Claude: a mesma conversa continua noutra conta Claude,
//! com o mesmo nome, pela reabertura do spec. O processo antigo sai inteiro antes de a conversa mudar de
//! conta (o `claude` morrendo grava pelo caminho antigo e recriaria o arquivo na origem); a conversa vai
//! por `rename` (`<sid>.jsonl` e a pasta irmã), e volta se a vida nova não subir. A transferência para o
//! Codex (`credential_id`) segue ao Python que a conduz.
use std::net::SocketAddr;
use std::path::{Path,PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo,Request,State};
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::future::BoxFuture;
use serde_json::{Value,json};

use super::{BODY_LIMIT,busy_refusal,detail_body,forward_whole,session_name,too_large};
use crate::mods::http::reply;
use crate::list::discover::real;
use crate::routes::{AppState,gate,pass};
use crate::runtime::claude_reopen::{Between,ReopenFailure};
use crate::runtime::protocol::RuntimeError;
use crate::session_open::{Account,ChangeReason,SessionChange,SessionSpec};

/// A partir daqui a tela avisa e pede confirmação; a partir de `FULL` não aceita.
pub const LOW_PCT:f64 = 95.0;
pub const FULL_PCT:f64 = 99.0;

/// `_account_targets`: as contas Claude, sem a atual, a de mais folga primeiro, sem leitura depois e as
/// cheias e sem login no fim. `pct` é a janela mais cheia (`None` sem leitura).
pub fn targets(catalog:&Value,quotas:&[Value],current:Option<&str>,login:impl Fn(&str)->bool) -> Vec<Value> {
    let same=|a:&str,b:&str|a==b || real(a)==real(b);
    let mut out:Vec<Value>=catalog.as_array().into_iter().flatten().filter_map(|row| {
        let path=row["path"].as_str()?;
        if current.is_some_and(|current|same(current,path)) {return None;}
        let read=quotas.iter().find(|q|q["estado"]=="lida" && q["id"].as_str().and_then(|id|id.strip_prefix("claude:")).is_some_and(|p|same(p,path)));
        let pct=read.and_then(|q|q["janelas"].as_array()?.iter().filter_map(|w|w["pct"].as_f64()).reduce(f64::max));
        Some(json!({"path":path,"label":row["label"],"pct":pct,"low":pct.is_some_and(|p|p>=LOW_PCT),"full":pct.is_some_and(|p|p>=FULL_PCT),"login":login(path)}))
    }).collect();
    out.sort_by(|a,b|{
        let key=|v:&Value|(v["full"]==true || v["login"]==false,v["pct"].is_null(),v["pct"].as_f64().unwrap_or(0.0));
        let (ka,kb)=(key(a),key(b));
        (ka.0,ka.1).cmp(&(kb.0,kb.1)).then(ka.2.total_cmp(&kb.2))
    });
    out
}

fn logged_in(path:&str) -> bool { crate::accounts::claude_refresh::has_login(Path::new(path)) }

/// O pedido de `POST /conta`: exatamente um destino; `config_dir` e `engine_account` são o corpo inteiro.
#[derive(Debug,PartialEq)]
enum Target { ConfigDir(String), EngineAccount(String), Codex }

fn target(bytes:&[u8]) -> Option<Target> {
    let Value::Object(body)=serde_json::from_slice::<Value>(bytes).ok()? else {return None};
    let present=|field:&str|body.get(field).is_some_and(|v|!v.is_null());
    let chosen=["config_dir","credential_id","engine_account"].into_iter().filter(|f|present(f)).count();
    if chosen!=1 {return None;}
    let known=["config_dir","credential_id","engine_account","source_life","source_jsonl","model","effort"];
    if body.keys().any(|k|!known.contains(&k.as_str())) {return None;}
    if present("credential_id") {return Some(Target::Codex);}
    let field=if present("config_dir") {"config_dir"} else {"engine_account"};
    if body.keys().any(|k|k!=field) {return None;}
    let value=body[field].as_str()?.to_owned();
    Some(if field=="config_dir" {Target::ConfigDir(value)} else {Target::EngineAccount(value)})
}

/// A conversa da sessão, onde ela está agora (a pasta de projeto pode não ser a do cwd: `EnterWorktree`).
fn conversation_file(env:&crate::accounts::environment::AccountEnvironment,spec:&SessionSpec,sid:&str) -> PathBuf {
    crate::runtime::claude_launch::transcript(env,spec,sid)
}

fn account_dir(env:&crate::accounts::environment::AccountEnvironment,spec:&SessionSpec) -> PathBuf {
    spec.account.config_dir().map_or_else(||env.claude_base.clone(),PathBuf::from)
}

fn io_refusal(code:&str,message:String) -> RuntimeError { RuntimeError::new(code,&message) }

/// Leva `<sid>.jsonl` e a pasta irmã `<sid>/` de `from` para `to`, tudo ou nada.
fn move_conversation(from:&Path,to:&Path) -> std::io::Result<()> {
    let sid=from.file_stem().ok_or(std::io::ErrorKind::InvalidInput)?.to_owned();
    let (from_dir,to_dir)=(from.parent().ok_or(std::io::ErrorKind::InvalidInput)?,to.parent().ok_or(std::io::ErrorKind::InvalidInput)?);
    std::fs::create_dir_all(to_dir)?;
    std::fs::rename(from,to)?;
    let sibling=from_dir.join(&sid);
    if sibling.is_dir() && let Err(error)=std::fs::rename(&sibling,to_dir.join(&sid)) {
        // Sem a pasta irmã a conversa perde tool-results e subagentes: melhor inteira na origem.
        std::fs::rename(to,from)?;
        return Err(error);
    }
    Ok(())
}

/// A conversa troca de conta entre encerrar o processo antigo e subir o novo.
struct MoveConversation { env:crate::accounts::environment::AccountEnvironment, windows:bool, leave_engine:bool }

impl MoveConversation {
    fn paths(&self,from:&SessionSpec,to:&SessionSpec,sid:&str) -> Option<(PathBuf,PathBuf)> {
        let source=conversation_file(&self.env,from,sid);
        if !source.exists() {return None;}
        let project=source.parent()?.file_name()?.to_owned();
        let destination=account_dir(&self.env,to).join("projects").join(project).join(format!("{sid}.jsonl"));
        (destination!=source).then_some((source,destination))
    }
}

impl Between for MoveConversation {
    fn adjust(&self,next:&mut SessionSpec) {
        // Voltando do motor à conta Claude: o modelo, a janela e o Fast do motor não valem na Anthropic.
        if self.leave_engine {next.model=None;next.service_tier=None;}
    }
    fn check<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> BoxFuture<'a,Result<(),RuntimeError>> {
        Box::pin(async move {
            if let Some((_,destination))=self.paths(from,to,sid) && (destination.exists() || destination.with_extension("").exists()) {
                return Err(RuntimeError::new("erro_conversa_ja_na_conta","a conta destino ja tem esta conversa"));
            }
            // A confiança na pasta é por conta: sem ela o terminal abre no aviso, em "No, exit". O arquivo
            // é o que o CLI daquela conta lê, pela mesma regra do `-e CLAUDE_CONFIG_DIR`. Antes de
            // encerrar: confiar a mais numa troca recusada não muda nada.
            let terminal=crate::session_open::terminal::TerminalEnv { windows:self.windows,program:String::new(),display:Vec::new(),
                server_config_dir:self.env.base.get("CLAUDE_CONFIG_DIR").filter(|v|!v.is_empty()).cloned(),
                default_config_dir:self.env.home.join(".claude"),birth:std::time::Duration::ZERO };
            let file=crate::session_open::claude::claude_json(&terminal,&self.env.home,to.account.config_dir());
            crate::session_open::claude::pretrust_async(file,to.cwd.clone(),self.windows).await
                .map_err(|e|RuntimeError::new("reopen_pretrust_failed",&format!("a pasta não pôde ser confiada na conta nova ({e}); nada foi reaberto")))
        })
    }
    fn apply<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> BoxFuture<'a,Result<(),RuntimeError>> {
        Box::pin(async move {
            if let Some((source,destination))=self.paths(from,to,sid) {
                tokio::task::spawn_blocking(move||move_conversation(&source,&destination)).await
                    .map_err(|_|io_refusal("erro_mover_conversa","a conversa não foi movida".into()))?
                    .map_err(|e|io_refusal("erro_mover_conversa",format!("nao consegui mover a conversa de conta ({e}); ela ficou na conta de antes")))?;
            }
            Ok(())
        })
    }
    fn undo<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> BoxFuture<'a,Result<(),RuntimeError>> {
        Box::pin(async move {
            let Some((source,destination))=self.paths(to,from,sid) else {return Ok(())};
            let shown=destination.display().to_string();
            tokio::task::spawn_blocking(move||move_conversation(&source,&destination)).await
                .map_err(|_|io_refusal("erro_mover_conversa","a conversa não voltou".into()))?
                .map_err(|e|io_refusal("erro_mover_conversa",format!("a conversa ficou na conta nova e não voltou para {shown} ({e})")))
        })
    }
}


/// As recusas do `_trocar_conta`: ocupada com a frase do app; o resto 409 `erro_troca_conta` com o código
/// de origem (`refused` abre a frase de quando nada mudou), ou o código próprio do conflito da conversa e da cota.
pub(crate) fn account_refusal(failure:&ReopenFailure,refused:&str) -> (StatusCode,Value) {
    let error=&failure.error;
    if let Some(busy)=busy_refusal(error) {return busy;}
    match error.code.as_str() {
        "erro_conversa_ja_na_conta"=>(StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({}))),
        "erro_troca_conta_parada"=>(StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({"erro":error.message}))),
        "erro_mover_conversa" if !failure.stopped=>(StatusCode::INTERNAL_SERVER_ERROR,detail_body(&error.code,&error.message,json!({"erro":error.message}))),
        _=>{
            let message=if failure.stopped {format!("a troca falhou: {}",error.message)} else {format!("{refused}: {}",error.message)};
            (StatusCode::CONFLICT,detail_body("erro_troca_conta",&message,json!({"erro":error.message,"code":error.code})))
        },
    }
}

/// A mesma conversa reaberta na conta Claude `destination`, levando o arquivo dela junto. `None`: a
/// sessão não é do Rust.
pub(crate) async fn reopen_in(st:&Arc<AppState>,runtime:&Arc<crate::runtime::gateway::RuntimeRegistry>,name:&str,spec:&SessionSpec,
    destination:&str,leave_engine:bool,reason:ChangeReason) -> Option<Result<crate::runtime::claude_reopen::Reopened,ReopenFailure>> {
    let surface=spec.parked().then_some(crate::session_open::Surface::Terminal);
    let change=SessionChange { reason,surface,account:Some(Account::Claude { config_dir:destination.to_owned() }),
        permission_mode:None,model:None,effort:None,service_tier:None };
    let mover=MoveConversation { env:runtime.codex_environment(),windows:cfg!(windows),leave_engine };
    let (runtime,owned)=(runtime.clone(),name.to_owned());
    super::relaunch::reopen_detached(st,name,async move {runtime.reopen_claude_session(&owned,change,Some(&mover)).await}).await
}

pub async fn targets_route(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request) -> Response {
    let (fwd,owner)=gate(&st,peer,&req);
    let runtime=st.state.runtime.get().cloned();
    let (Some(runtime),true,Some(name))=(runtime,owner,session_name(req.uri().path())) else {return pass(&st,req,&fwd).await};
    let env=runtime.codex_environment();
    // No motor, mesmo a conta atual é destino: a sessão volta para a conta Claude sem mover nada. Sessão
    // que não é Claude não tem conta Claude atual: todas são destino.
    let current=match runtime.claude_spec(&name).await {
        Some(Ok(spec))=>(!matches!(spec.account,Account::Engine{..})).then(||account_dir(&env,&spec).to_string_lossy().into_owned()),
        Some(Err(error))=>return reply(Some(req.headers()),StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({}))),
        None if runtime.has_entry(None,&name).await=>None,
        None=>return reply(Some(req.headers()),StatusCode::NOT_FOUND,detail_body("erro_sessao_inexistente","sessão não encontrada",json!({}))),
    };
    let catalog=match st.accounts.claude_catalog() {
        Ok(catalog)=>catalog,
        Err(error)=>return reply(Some(req.headers()),StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),detail_body(error.code,&error.message,json!({}))),
    };
    let quotas=st.accounts.quotas(None,false,true).await.unwrap_or(Value::Null);
    let quotas:Vec<Value>=quotas.as_array().into_iter().flatten().filter(|row|row["provedor"]=="claude").cloned().collect();
    reply(Some(req.headers()),StatusCode::OK,Value::Array(targets(&catalog,&quotas,current.as_deref(),logged_in)))
}

pub async fn switch_route(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request) -> Response {
    let (fwd,owner)=gate(&st,peer,&req);
    let runtime=st.state.runtime.get().cloned();
    let (Some(runtime),true,Some(name))=(runtime,owner,session_name(req.uri().path())) else {return pass(&st,req,&fwd).await};
    let (parts,body)=req.into_parts();
    let Ok(bytes)=to_bytes(body,BODY_LIMIT).await else {return too_large(&parts.headers)};
    let destination=match target(&bytes) {
        Some(Target::ConfigDir(dir))=>dir,
        // A transferência para o Codex e o corpo torto seguem ao Python, que os conduz e recusa.
        Some(Target::Codex) | None=>return forward_whole(&st,parts,bytes,&fwd).await,
        Some(Target::EngineAccount(account))=>return super::engine::account_route(st,runtime,name,account,parts.headers).await,
    };
    let catalog=match st.accounts.claude_catalog() {
        Ok(catalog)=>catalog,
        Err(error)=>return reply(Some(&parts.headers),StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),detail_body(error.code,&error.message,json!({}))),
    };
    let quotas=st.accounts.quotas(None,false,true).await.unwrap_or(Value::Null);
    let quotas:Vec<Value>=quotas.as_array().into_iter().flatten().filter(|row|row["provedor"]=="claude").cloned().collect();
    let Some(chosen)=targets(&catalog,&quotas,None,logged_in).into_iter().find(|t|t["path"]==destination.as_str()) else {
        return reply(Some(&parts.headers),StatusCode::BAD_REQUEST,detail_body("erro_config_dir_invalido","config_dir invalido",json!({})));
    };
    // Sem login o Claude morre ao subir: recusa antes de parar a sessão.
    if chosen["login"]==false {
        return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_conta_sem_login",&format!("a conta {} não está logada; faça login nela antes de trocar",chosen["label"].as_str().unwrap_or_default()),
            json!({"conta":chosen["label"]})));
    }
    if chosen["full"]==true {
        let pct=chosen["pct"].as_f64().unwrap_or(0.0);
        return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_conta_cheia",&format!("a conta {} está em {pct:.0}% da cota",chosen["label"].as_str().unwrap_or_default()),
            json!({"conta":chosen["label"],"pct":pct})));
    }
    let spec=match runtime.claude_spec(&name).await {
        None=>{
            let provider=st.list.discover(None).await.map(|rows|rows.iter().find(|r|r.name==name).map(|r|r.provider.clone())).map_err(|e|e.code);
            return match without_claude_spec(provider.as_ref().map(|p|p.as_deref()).map_err(|e|*e)) {
                Some((status,body))=>reply(Some(&parts.headers),status,body),
                None=>forward_whole(&st,parts,bytes,&fwd).await,
            };
        }
        Some(Err(error))=>return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_troca_conta",&format!("não troquei de conta: {}",error.message),json!({"erro":error.message,"code":error.code}))),
        Some(Ok(spec))=>spec,
    };
    let env=runtime.codex_environment();
    let leave_engine=match &spec.account {
        Account::Engine { name:engine,.. } if !super::engine::local_cliproxy(&env,engine) =>
            return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_conta_so_claude","só conta Claude ou motor CLIProxyAPI local troca de conta",json!({}))),
        Account::Engine{..}=>true,
        _=>false,
    };
    if !leave_engine && !spec.parked() && real(&account_dir(&env,&spec).to_string_lossy())==real(&destination) {
        return reply(Some(&parts.headers),StatusCode::OK,json!({"ok":true,"config_dir":destination}));
    }
    let outcome=reopen_in(&st,&runtime,&name,&spec,&destination,leave_engine,ChangeReason::AccountSwitch).await;
    let (status,body)=match outcome {
        None=>return forward_whole(&st,parts,bytes,&fwd).await,
        Some(Ok(reopened))=>(StatusCode::OK,reopened.answer(json!({"ok":true,"config_dir":destination}))),
        Some(Err(failure))=>account_refusal(&failure,"não troquei de conta"),
    };
    reply(Some(&parts.headers),status,body)
}

/// Sem spec Claude: sessão inexistente é 404 e de outro provedor é 409, como o `_trocar_conta`; a lista
/// fora do ar é 503 com o código dela. `None` = Claude, segue.
fn without_claude_spec(provider: Result<Option<&str>, &'static str>) -> Option<(StatusCode, Value)> {
    match provider {
        Err(code) => Some((StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "a lista de sessões não respondeu", json!({})))),
        Ok(None) => Some((StatusCode::NOT_FOUND, detail_body("erro_sessao_inexistente", "sessão não encontrada", json!({})))),
        Ok(Some("claude")) => None,
        Ok(Some(_)) => Some((StatusCode::CONFLICT, detail_body("erro_conta_so_claude", "só sessão Claude troca de conta por esta rota", json!({})))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sem spec Claude, a sessão que não existe é 404 e a de outro provedor (Pi, omp, Kimi,
    /// Codex com `config_dir`) é 409, como o Python; lista fora do ar é 503 com o código dela.
    #[test]
    fn without_a_claude_spec_the_refusal_is_the_python_one() {
        let code = |r: Option<(StatusCode, Value)>| r.map(|(s, b)| (s, b["detail"]["code"].clone()));
        assert_eq!(code(without_claude_spec(Ok(None))), Some((StatusCode::NOT_FOUND, json!("erro_sessao_inexistente"))));
        for provider in ["pi", "omp", "kimi", "codex"] {
            assert_eq!(code(without_claude_spec(Ok(Some(provider)))), Some((StatusCode::CONFLICT, json!("erro_conta_so_claude"))), "{provider}");
        }
        assert_eq!(code(without_claude_spec(Err("erro_lista_indisponivel"))), Some((StatusCode::SERVICE_UNAVAILABLE, json!("erro_lista_indisponivel"))));
        assert_eq!(without_claude_spec(Ok(Some("claude"))), None, "Claude sem spec segue como antes");
    }

    #[test]
    fn only_one_destination_and_the_whole_legacy_body() {
        assert_eq!(target(br#"{"config_dir":"/h/.claude-b"}"#),Some(Target::ConfigDir("/h/.claude-b".into())));
        assert_eq!(target(br#"{"engine_account":"acc"}"#),Some(Target::EngineAccount("acc".into())));
        assert_eq!(target(br#"{"credential_id":"codex:x","source_life":"k:a","source_jsonl":"/j"}"#),Some(Target::Codex));
        for bad in [&br#"{"config_dir":"/a","engine_account":"b"}"#[..],br#"{"config_dir":"/a","model":"m"}"#,br#"{}"#,br#"{"x":1}"#] {
            assert_eq!(target(bad),None,"{}",String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn targets_put_the_most_room_first_unread_after_and_full_last_without_the_current() {
        let catalog=json!([{"path":"/a","label":"a"},{"path":"/b","label":"b"},{"path":"/c","label":"c"},{"path":"/d","label":"d"},{"path":"/e","label":"e"}]);
        let quota=|path:&str,pct:f64|json!({"id":format!("claude:{path}"),"estado":"lida","janelas":[{"pct":pct},{"pct":pct/2.0}]});
        let quotas=vec![quota("/a",99.5),quota("/b",40.0),quota("/c",96.0),quota("/e",10.0)];
        let out=targets(&catalog,&quotas,Some("/e"),|_|true);
        let order:Vec<&str>=out.iter().map(|t|t["path"].as_str().unwrap()).collect();
        assert_eq!(order,["/b","/c","/d","/a"]);
        assert_eq!((out[1]["low"].as_bool(),out[1]["full"].as_bool()),(Some(true),Some(false)));
        assert_eq!((out[3]["full"].as_bool(),out[2]["pct"].is_null()),(Some(true),true));
    }

    /// Conta sem login não serve de destino: vem marcada e vai para o fim, junto das cheias.
    #[test]
    fn an_account_without_login_is_marked_and_goes_last() {
        let catalog=json!([{"path":"/a","label":"a"},{"path":"/b","label":"b"}]);
        let out=targets(&catalog,&[],None,|path|path!="/a");
        let order:Vec<(&str,bool)>=out.iter().map(|t|(t["path"].as_str().unwrap(),t["login"].as_bool().unwrap())).collect();
        assert_eq!(order,[("/b",true),("/a",false)]);
    }

    /// Login vale com o token de acesso no prazo ou com o de renovação; sem arquivo, não.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn login_needs_a_live_credential() {
        let dir=tempfile::tempdir().unwrap();
        assert!(!crate::accounts::claude_refresh::has_login(dir.path()));
        let write=|v:Value|std::fs::write(dir.path().join(".credentials.json"),json!({"claudeAiOauth":v}).to_string()).unwrap();
        let future=(crate::accounts::quotas::now()+3600.0)*1000.0;
        write(json!({"accessToken":"a","expiresAt":future}));
        assert!(crate::accounts::claude_refresh::has_login(dir.path()));
        write(json!({"accessToken":"a"}));
        assert!(crate::accounts::claude_refresh::has_login(dir.path()), "sem prazo informado não é falta de login");
        write(json!({"accessToken":"a","expiresAt":1000,"refreshToken":"r"}));
        assert!(crate::accounts::claude_refresh::has_login(dir.path()));
        write(json!({"accessToken":"a","expiresAt":1000,"refreshToken":"r","refreshTokenExpiresAt":1000}));
        assert!(!crate::accounts::claude_refresh::has_login(dir.path()));
        write(json!({"accessToken":"a","expiresAt":1000}));
        assert!(!crate::accounts::claude_refresh::has_login(dir.path()));
    }

    #[test]
    fn the_conversation_moves_with_its_sibling_folder_or_not_at_all() {
        let dir=tempfile::tempdir().unwrap();
        let (from,to)=(dir.path().join("a/projects/p/s.jsonl"),dir.path().join("b/projects/p/s.jsonl"));
        std::fs::create_dir_all(from.parent().unwrap().join("s")).unwrap();
        std::fs::write(&from,"{}\n").unwrap();
        std::fs::write(from.parent().unwrap().join("s/tool.txt"),"x").unwrap();
        move_conversation(&from,&to).unwrap();
        assert!(to.exists() && to.parent().unwrap().join("s/tool.txt").exists() && !from.exists());
    }
}
