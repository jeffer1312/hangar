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

impl hangar_mission::http::Notify for SessionNotify {
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
            let Some(runtime)=state.state.runtime.get() else {return Err(Failure::new(503,"ticket_runtime_unavailable","o runtime ainda não abriu as sessões"));};
            let key=match crate::session_write::route_write(runtime,&name,crate::session_write::WriteRoute::Input,state.write_gate_wait).await {
                crate::session_write::Write::Rust(_held,target)=>target.key,
                crate::session_write::Write::Asleep(key)|crate::session_write::Write::Opening(key)=>key,
                crate::session_write::Write::Busy=>return Err(Failure::new(409,"ticket_session_busy","a sessão está trocando de identidade")),
                crate::session_write::Write::Failed(error)=>return Err(Failure::new(503,error.code,error.message)),
                _=>return Ok(None),
            };
            if expected.as_ref().is_some_and(|expected|expected!=&key) {return Ok(None);}
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
