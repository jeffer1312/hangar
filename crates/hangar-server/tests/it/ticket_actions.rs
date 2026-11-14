//! Board pela API real, com runtime/fila reais e um cano externo no protocolo público.
#![cfg(unix)]
use std::sync::Arc;
use axum::{body::{Body,to_bytes},extract::ConnectInfo,http::Request};
use hangar_server::{config::Config,routes::AppState,runtime::{gateway::RuntimeRegistry,protocol::{RuntimeTarget,CanoBinding}}};
use serde_json::{Value,json};
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};
use tower::ServiceExt;

#[tokio::test(flavor="multi_thread")]
async fn board_action_uses_the_bound_runtime_input_and_reports_an_uncertain_ack() {
    let temp=tempfile::tempdir().unwrap();
    let policy=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream=policy.local_addr().unwrap();
    let (stop,stopped)=tokio::sync::oneshot::channel();
    let policy_task=tokio::spawn(async move {
        axum::serve(policy,axum::Router::new().fallback(||async {([(axum::http::header::CONTENT_TYPE,"application/json")],json!({"ok":true,"data":{}}).to_string())}))
            .with_graceful_shutdown(async {let _=stopped.await;}).await.unwrap();
    });
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let (sent,text)=tokio::sync::oneshot::channel();
    let cano=tokio::spawn(async move {
        let (stream,_)=listener.accept().await.unwrap();
        let mut reader=BufReader::new(stream);
        let mut raw=String::new();reader.read_line(&mut raw).await.unwrap();
        assert_eq!(raw,"secret-test\n");
        let snapshot=crate::mods_support::cano_snapshot_json();
        reader.get_mut().write_all(format!("{snapshot}\n").as_bytes()).await.unwrap();
        raw.clear();reader.read_line(&mut raw).await.unwrap();
        let envelope:Value=serde_json::from_str(&raw).unwrap();
        let frame:Value=serde_json::from_str(envelope["frame"].as_str().unwrap()).unwrap();
        sent.send(frame["message"]["content"][0]["text"].as_str().unwrap().to_owned()).unwrap();
        // estavel: o ACK desconhecido é uma resposta do protocolo do cano, sem simular o dispatcher.
        let ack=json!({"type":"cano_input_ack","operation_id":envelope["operation_id"],"outcome":"unknown"});
        reader.get_mut().write_all(format!("{ack}\n").as_bytes()).await.unwrap();
        raw.clear();while reader.read_line(&mut raw).await.unwrap_or(0)>0 {raw.clear();}
    });
    let registry=Arc::new(RuntimeRegistry::new(upstream,"secret-test".into(),"ticket-test".into())
        // estavel: somente a leitura do SO é substituída pelos PIDs do processo externo da prova.
        .with_processes(Arc::new(crate::sink::RecipeProcesses("sid-1"))));
    registry.bind_lifecycle();
    registry.open(RuntimeTarget {key:"life-1".into(),generation:1,name:"ticket-session".into(),provider:"claude".into(),
        metadata:json!({"name":"ticket-session","headless":true,"session_id":"sid-1","initialized":true}),
        binding:CanoBinding {pid:42,escuta:format!("tcp:{address}"),token:"secret-test".into(),versao:2},
        lease_path:temp.path().join("session.lock"),state_path:temp.path().join("state.json"),projection_dir:temp.path().join("projection"),
        transcript:temp.path().join("history.jsonl"),created:0.0}).await.unwrap();
    let cfg=Config {listen:"127.0.0.1:0".parse().unwrap(),upstream,internal_secret:"secret-test".into(),auth_token:"owner-ticket".into(),
        log_path:None,trusted:hangar_server::auth::TrustedHosts::parse("127.0.0.1")};
    let mut state=AppState::new(cfg);
    state.hangar_dir=temp.path().join(".hangar");state.origins_home=temp.path().to_owned();
    state.diag=state.diag.with_diary(Some(temp.path().join("diary")));
    state.mission=Arc::new(hangar_mission::Store::new(temp.path().join("mission")));
    let mut stages=state.mission.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje este ticket.".into());
    state.mission.set_stages(stages).unwrap();
    state.mission.link("js-demo","c0ffee-01",hangar_mission::store::Link {key:"OPS/7".into(),title:"Impressora".into(),url:"https://example.invalid/7".into(),session:"ticket-session".into()}).unwrap();
    state.connectors=hangar_connectors::Host::new(hangar_connectors::registry::Registry::new(temp.path().join("connectors.json")),Default::default());
    let script=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/connectors/work_tracking.mjs");
    state.connectors.add(hangar_connectors::manifest::Manifest::parse(json!({"manifest_version":1,"id":"js-demo","name":"Demo","version":"1","protocol_version":1,
        "capabilities":{"work_tracking":1},"command":["node",script]}).to_string().as_bytes(),temp.path()).unwrap()).unwrap();
    state.state.runtime.set(registry.clone()).unwrap_or_else(|_|panic!("runtime já configurado"));
    state.side.runtime.set(registry.clone()).unwrap_or_else(|_|panic!("runtime do hub já configurado"));
    let state=Arc::new(state);
    registry.bind_hubs(state.side.hubs.downgrade());
    let app=hangar_server::routes::router(state.clone());
    let id="abcdef0123456789abcdef0123456789";
    let mut request=Request::post("/api/mission/actions").header("authorization","Bearer owner-ticket").header("content-type","application/json")
        .body(Body::from(json!({"attempt":id,"connector":"js-demo","ticket_id":"c0ffee-01","stage":"planejamento"}).to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(std::net::SocketAddr::from(([127,0,0,1],40000))));
    let response=app.oneshot(request).await.unwrap();
    let status=response.status();
    let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1024*1024).await.unwrap()).unwrap();
    assert_eq!(status,202,"{body}");
    let attempt=tokio::time::timeout(std::time::Duration::from_secs(30),state.ticket_actions.get().unwrap().completed(id)).await.unwrap().unwrap();
    assert_eq!(attempt.status,hangar_ticket_actions::Status::Uncertain,"{attempt:?}");
    assert_eq!(attempt.code.as_deref(),Some("ticket_action_uncertain"));
    let item=state.mission.snapshot().unwrap().items.remove(0);
    let delivered=tokio::time::timeout(std::time::Duration::from_secs(30),text).await.unwrap().unwrap();
    registry.close("life-1",1).await.unwrap();
    state.connectors.shutdown().await;
    tokio::time::timeout(std::time::Duration::from_secs(30),cano).await.unwrap().unwrap();
    stop.send(()).unwrap();policy_task.await.unwrap();
    assert!(delivered.contains("Planeje este ticket."));
    assert_eq!(item.session_key.as_deref(),Some("life-1"));
    assert!(item.stage.is_none(),"o resultado incerto não confirma a mudança de etapa");
}
