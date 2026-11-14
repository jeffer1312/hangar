//! Entrega dos avisos dos tickets à sessão pelo handler da rota de entrada do dono, dentro do processo: a
//! mesma fila e a mesma posse da escrita do `POST /api/sessions/{name}/input`.
use crate::routes::AppState;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{OnceLock, Weak};

pub struct SessionNotify {
    /// Referência fraca: o estado guarda este entregador, e uma forte o manteria vivo para sempre.
    pub state: OnceLock<Weak<AppState>>,
}

/// A chave vem da vida atual, pelo sidecar ou ambiente do terminal, nunca de um spec histórico.
pub(crate) fn ticket_groups(home: &std::path::Path, program: &std::ffi::OsStr, sidecars: Vec<(String,crate::groups::model::Sidecar)>) -> Vec<hangar_mission::model::Group> {
    if sidecars.is_empty() {return Vec::new();}
    let mut command=crate::terminal_input::child_command(program);
    command.args(["list-panes","-a","-F","#{session_name}\t#{CP_SESSION_KEY}"]);
    let terminals=hangar_workspace::process::run_program(command.as_std_mut(),std::time::Duration::from_secs(2)).ok().filter(|out|out.code==0)
        .map(|out|out.stdout.lines().filter_map(|line| {
            let (name,key)=line.split_once('\t')?;
            (key.len()==32 && key.bytes().all(|byte|byte.is_ascii_hexdigit())).then(||(name.to_owned(),key.to_owned()))
        }).collect::<std::collections::BTreeMap<_,_>>()).unwrap_or_default();
    let mut found=std::collections::BTreeMap::new();
    for (name,group) in sidecars {
        let members=found.entry(group.gid).or_insert_with(std::collections::BTreeSet::new);
        members.insert(name);members.extend(group.peers);
    }
    found.into_iter().map(|(id,members)|hangar_mission::model::Group {id,
        keys:members.iter().filter(|name|!name.contains("::")).filter_map(|name| {
            let key=match crate::session_open::record::live(home,name) {Ok(Some(spec))=>spec.key,Ok(None)=>terminals.get(name).cloned()?,Err(_)=>return None};
            Some((name.clone(),key))
        }).collect(),members:members.into_iter().collect()}).collect()
}

impl hangar_mission::http::Notify for SessionNotify {
    fn send_bound(&self, session: String, key: Option<String>, text: String) -> Pin<Box<dyn Future<Output=()> + Send>> {
        let Some(key)=key else {return hangar_mission::http::Notify::send(self,session,text);};
        let state=self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {if let Some(state)=state {
            let (status,_)=crate::session_write::input::write_bound_input(&state,&session,&key,None,&text).await;
            if !status.is_success() {tracing::info!(code="mission_notice_refused",status=status.as_u16());}
        }})
    }
    fn identity(&self, session: String) -> Pin<Box<dyn Future<Output=Result<Option<String>,hangar_mission::store::Error>> + Send>> {
        let state=self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {
            use hangar_mission::store::{Error,Kind};
            let unavailable=||Error {kind:Kind::Unavailable,code:"ticket_session_unavailable",message:"não foi possível conferir a identidade da sessão".into()};
            let state=state.ok_or_else(unavailable)?;
            let rows=state.list.snapshot().await.map_err(|_|unavailable())?;
            let row=rows.rows.iter().find(|row|row.name==session).ok_or_else(unavailable)?;
            if let Some(key)=hangar_api::session::runtime_key(row.lifecycle_id.as_deref()) {return Ok(Some(key.to_owned()));}
            let key=state.list.env().mux.session_env(&session,"CP_SESSION_KEY").await.filter(|key|key.len()==32 && key.bytes().all(|byte|byte.is_ascii_hexdigit())).ok_or_else(unavailable)?;
            Ok(Some(key))
        })
    }

    fn adopt(&self, store: std::sync::Arc<hangar_mission::Store>, connector: String, ticket: String, link: hangar_mission::store::Link, expected: Option<String>) -> Pin<Box<dyn Future<Output=Result<(hangar_mission::model::Item,bool),hangar_mission::store::Error>> + Send + '_>> {
        Box::pin(async move {
            use hangar_mission::store::{Error,Kind};
            let unavailable=||Error {kind:Kind::Unavailable,code:"ticket_groups_unavailable",message:"não foi possível conferir o grupo da sessão".into()};
            let state=self.state.get().and_then(Weak::upgrade).ok_or_else(unavailable)?;
            let key=hangar_mission::http::Notify::identity(self,link.session.clone()).await?;
            if expected.is_some() && expected!=key {return Err(Error {kind:Kind::Gate,code:"ticket_session_changed",message:"a sessão mudou desde a escolha; confira novamente".into()});}
            let _held=if let Some(runtime)=state.state.runtime.get() {
                match crate::session_write::route_write(runtime,&link.session,crate::session_write::WriteRoute::Input,state.write_gate_wait).await {
                    crate::session_write::Write::Rust(pass,target) if key.as_deref()==Some(&target.key)=>Some(pass),
                    crate::session_write::Write::Asleep(current)|crate::session_write::Write::Opening(current) if key.as_ref()==Some(&current)=>None,
                    _=>return Err(Error {kind:Kind::Gate,code:"ticket_session_changed",message:"a sessão está trocando de identidade; confira novamente".into()}),
                }
            } else {None};
            let apply=async {
                let sidecars=match &state.groups {Some(groups)=>groups.sidecars().await.map_err(|_|unavailable())?,None=>Vec::new()};
                let (home,program)=(state.origins_home.clone(),state.list.env().capture_program.clone());
                tokio::task::spawn_blocking(move|| {
                    let mut groups=ticket_groups(&home,&program,sidecars);
                    for group in &mut groups {if group.members.contains(&link.session) && let Some(key)=&key {group.keys.insert(link.session.clone(),key.clone());}}
                    store.link_owner(&connector,&ticket,link,key.as_deref(),&groups)
                }).await.map_err(|_|unavailable())?
            };
            if let Some(groups)=&state.groups {groups.locked(apply).await} else {apply.await}
        })
    }
    fn fetch_evidence(&self, server: String, path: String) -> Pin<Box<dyn Future<Output=Result<(String,Vec<u8>),hangar_mission::store::Error>> + Send>> {
        let state=self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {
            use hangar_mission::store::{Error,Kind};
            let unavailable=||Error {kind:Kind::Unavailable,code:"ticket_remote_evidence_unavailable",message:"não foi possível copiar a evidência da outra máquina".into()};
            let state=state.ok_or_else(unavailable)?;
            let name=path.rsplit(['/', '\\']).next().filter(|name|!name.is_empty()).ok_or_else(unavailable)?.to_owned();
            let query=form_urlencoded::Serializer::new(String::new()).append_pair("path",&path).finish();
            let (_,mut bytes)=state.peers.send_secret(true,&server,reqwest::Method::GET,&format!("/api/mission/source?{query}"),None,512*1024*1024).await.map_err(|_|unavailable())?;
            Ok((name,std::mem::take(&mut *bytes)))
        })
    }
    fn groups(&self) -> Pin<Box<dyn Future<Output = Result<Vec<hangar_mission::model::Group>, hangar_mission::store::Error>> + Send>> {
        let state = self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {
            use hangar_mission::store::{Error,Kind};
            let unavailable = || Error { kind:Kind::Unavailable, code:"ticket_groups_unavailable", message:"não foi possível conferir os participantes dos tickets".into() };
            let state = state.ok_or_else(unavailable)?;
            let Some(groups) = &state.groups else { return Ok(Vec::new()); };
            let sidecars=groups.locked(groups.sidecars()).await.map_err(|_|unavailable())?;
            let home=state.origins_home.clone();
            let program=state.list.env().capture_program.clone();
            let mut found=tokio::task::spawn_blocking(move||ticket_groups(&home,&program,sidecars)).await.map_err(|_|unavailable())?;
            let rows=state.list.snapshot().await.map_err(|_|unavailable())?;
            for group in &mut found {for row in rows.rows.iter() {if group.members.contains(&row.name) && let Some(key)=hangar_api::session::runtime_key(row.lifecycle_id.as_deref()) {group.keys.insert(row.name.clone(),key.to_owned());}}}
            Ok(found)
        })
    }
    fn send(&self, session: String, text: String) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let state = self.state.get().and_then(Weak::upgrade);
        Box::pin(async move {
            let Some(state) = state else { return };
            let path = format!("/api/sessions/{}/input", percent_encoding::utf8_percent_encode(&session, percent_encoding::NON_ALPHANUMERIC));
            let body = serde_json::json!({"text": text}).to_string();
            let Ok(request) = Request::post(path).header("authorization", format!("Bearer {}", state.cfg.auth_token))
                .header("content-type", "application/json").header("user-agent", "hangar-mission").body(Body::from(body)) else { return };
            let peer = SocketAddr::from(([127, 0, 0, 1], 0));
            let response = crate::session_write::input::input(State(state), ConnectInfo(peer), request).await;
            if !response.status().is_success() {
                tracing::info!(code = "mission_notice_refused", status = response.status().as_u16());
            }
        })
    }
}

impl SessionNotify {
    fn app(&self) -> Result<std::sync::Arc<AppState>, hangar_ticket_actions::Failure> {
        self.state.get().and_then(Weak::upgrade).ok_or_else(|| hangar_ticket_actions::Failure::new(503,"ticket_runtime_unavailable","servidor indisponível"))
    }
}

impl hangar_ticket_actions::Sessions for SessionNotify {
    fn validate_open(&self, options: serde_json::Value) -> Result<serde_json::Value, hangar_ticket_actions::Failure> {
        use hangar_ticket_actions::Failure;
        let bytes = serde_json::to_vec(&options).map_err(|_|Failure::new(422,"ticket_open_invalid","opções de criação inválidas"))?;
        let request = crate::session_open::SessionOpenRequest::parse(&bytes).map_err(|_|Failure::new(422,"ticket_open_invalid","opções de criação inválidas"))?;
        if request.name.is_some() || request.initial_prompt.is_some() || request.link || request.branch.is_some() || request.new_branch || request.base.is_some() || request.creator.is_some() {
            return Err(Failure::new(422,"ticket_open_invalid","o board escolhe o nome e a primeira mensagem; o plugin prepara a branch"));
        }
        Ok(options)
    }

    fn resolve(&self, mut name: String, expected: Option<String>) -> hangar_ticket_actions::Task<Option<hangar_ticket_actions::Session>> {
        let state = self.app();
        Box::pin(async move {
            use hangar_ticket_actions::Failure;
            let state = state?;
            if let Some(key)=expected.clone() {
                let home = state.hangar_dir.parent().unwrap_or(&state.hangar_dir).to_owned();
                if let Some(spec)=tokio::task::spawn_blocking(move||crate::session_open::record::read(&home,&key)).await
                    .map_err(|_|Failure::new(503,"ticket_session_unavailable","leitura da identidade interrompida"))?
                    .map_err(|_|Failure::new(503,"ticket_session_unavailable","identidade da sessão ilegível"))? {name=spec.name;}
            }
            if let Some(groups)=&state.groups {
                let group=groups.link(&name).await.map_err(|_|Failure::new(503,"ticket_groups_unavailable","não foi possível conferir o grupo da sessão"))?;
                if let Some(group)=group {
                    let store=state.mission.clone();
                    let snapshot=tokio::task::spawn_blocking(move||store.snapshot()).await
                        .map_err(|_|Failure::new(503,"mission_unavailable","leitura dos tickets interrompida"))?
                        .map_err(|_|Failure::new(503,"mission_unavailable","tickets ilegíveis"))?;
                    if snapshot.items.iter().filter(|item|item.delivered_at.is_none() && (item.group_ids.contains(&group.gid)
                        || item.sessions.iter().any(|member|member==&name || group.peers.contains(member)))).count()>1 {
                        return Err(Failure::new(409,"ticket_group_conflict","o grupo está associado a tickets diferentes; remova a associação incorreta no board"));
                    }
                }
            }
            let Some(runtime)=state.state.runtime.get() else {return Err(Failure::new(503,"ticket_runtime_unavailable","o runtime ainda não abriu as sessões"));};
            let key=match crate::session_write::route_write(runtime,&name,crate::session_write::WriteRoute::Input,state.write_gate_wait).await {
                crate::session_write::Write::Rust(_held,target)=>target.key,
                crate::session_write::Write::Asleep(key)|crate::session_write::Write::Opening(key)=>key,
                crate::session_write::Write::Busy=>return Err(Failure::new(409,"ticket_session_busy","a sessão está trocando de identidade")),
                crate::session_write::Write::Failed(error)=>return Err(Failure::new(503,error.code,error.message)),
                _=>return Ok(None),
            };
            if expected.as_ref().is_some_and(|expected|expected!=&key) {return Ok(None);}
            if let Some(groups)=&state.groups && let Some(group)=groups.link(&name).await.map_err(|_|Failure::new(503,"ticket_groups_unavailable","grupo ilegível"))? {
                let mut servers=std::collections::BTreeSet::new();
                servers.extend(group.peers.iter().filter_map(|peer|peer.split_once("::").map(|(server,_)|server.to_owned())));
                if let Some(fed)=&group.fed {servers.insert(fed.owner.clone());}
                servers.remove(&state.server_id);
                let known=state.peers.ids();
                let scoped=format!("{}::{name}",state.server_id);
                let query=form_urlencoded::Serializer::new(String::new()).append_pair("session",&scoped).append_pair("key",&key).append_pair("group_id",&group.gid).finish();
                for server in servers.into_iter().filter(|server|known.contains(server)) {
                    match state.peers.send_secret(false,&server,reqwest::Method::GET,&format!("/api/mission/session?{query}"),None,1024*1024).await {
                        Ok((_,raw))=> {
                            let value:serde_json::Value=serde_json::from_slice(&raw).map_err(|_|Failure::new(503,"ticket_remote_unavailable","resposta de ticket ilegível"))?;
                            if !value["item"].is_object() {return Err(Failure::new(503,"ticket_remote_unavailable","resposta de ticket incompleta"));}
                            if value["item"]["delivered_at"].is_null() {return Err(Failure::new(409,"ticket_group_conflict","o grupo já trabalha um ticket em outra máquina"));}
                        }
                        Err(crate::groups::peers::PeerError::Refused {status:404,detail}) if detail["code"]=="mission_session_not_linked"=>{},
                        Err(_)=>return Err(Failure::new(503,"ticket_remote_unavailable","não foi possível conferir o ticket na outra máquina")),
                    }
                }
            }
            let conversation=state.info(&name).await.map_err(|_|Failure::new(503,"ticket_session_unavailable","não foi possível conferir a conversa"))?
                .map(|info|info.session_key).filter(|key|!key.is_empty());
            Ok(Some(hangar_ticket_actions::Session {name,key,conversation}))
        })
    }

    fn open(&self, name: String, mut options: serde_json::Value) -> hangar_ticket_actions::Task<()> {
        let state = self.app();
        Box::pin(async move {
            let state = state?;
            options["name"] = serde_json::json!(name);
            let request = owner_request(&state,"/api/sessions",options)?;
            let response = crate::session_open::create(State(state),ConnectInfo(SocketAddr::from(([127,0,0,1],0))),request).await;
            action_response(response).await.map(|_|())
        })
    }

    fn send(&self, session: hangar_ticket_actions::Session, text: String) -> hangar_ticket_actions::Task<hangar_ticket_actions::Delivery> {
        let state = self.app();
        Box::pin(async move {
            let state = state?;
            let (status,body)=crate::session_write::input::write_bound_input(&state,&session.name,&session.key,session.conversation.as_deref(),&text).await;
            if body["uncertain"] == true {
                return Err(hangar_ticket_actions::Failure::new(503,"ticket_action_uncertain","o envio precisa ser conferido na sessão"));
            }
            check_action_response(status,&body)?;
            if body["ok"]!=true {return Err(hangar_ticket_actions::Failure::new(503,"ticket_action_uncertain","o envio não foi confirmado"));}
            Ok(if body["delivered"] == true {hangar_ticket_actions::Delivery::Sent} else {hangar_ticket_actions::Delivery::Queued})
        })
    }
}

fn owner_request(state: &AppState, path: &str, value: serde_json::Value) -> Result<Request<Body>,hangar_ticket_actions::Failure> {
    Request::post(path).header("authorization",format!("Bearer {}",state.cfg.auth_token))
        .header("content-type","application/json").header("user-agent","hangar-tickets").body(Body::from(value.to_string()))
        .map_err(|_|hangar_ticket_actions::Failure::new(500,"ticket_request_invalid","não foi possível montar o pedido"))
}

async fn action_response(response: axum::response::Response) -> Result<serde_json::Value,hangar_ticket_actions::Failure> {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(),1024*1024).await
        .map_err(|_|hangar_ticket_actions::Failure::new(503,"ticket_runtime_unavailable","resposta do runtime indisponível"))?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_|hangar_ticket_actions::Failure::new(503,"ticket_runtime_unavailable","resposta do runtime inválida"))?;
    check_action_response(status,&body)?;
    Ok(body)
}

fn check_action_response(status: axum::http::StatusCode, body: &serde_json::Value) -> Result<(),hangar_ticket_actions::Failure> {
    if !status.is_success() {
        return Err(hangar_ticket_actions::Failure::new(status.as_u16(),body["detail"]["code"].as_str().unwrap_or("ticket_runtime_refused"),
            body["detail"]["message"].as_str().or_else(||body["detail"]["msg"].as_str()).unwrap_or("o runtime recusou a ação")));
    }
    Ok(())
}
