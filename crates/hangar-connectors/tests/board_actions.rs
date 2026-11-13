//! Contrato HTTP do despacho com host/store reais e agente externo isolado na fronteira.
use std::{path::PathBuf, sync::Arc};
use axum::{body::{Body, to_bytes}, extract::ConnectInfo, http::Request};
use hangar_connectors::{host::{Host, HostOptions}, manifest::Manifest, registry::Registry};
use hangar_core::{auth::{Auth, TrustedHosts}, diag::DiagClient, edge::Edge};
use hangar_mission::store::Store;
use hangar_ticket_actions::{Delivery, Engine, Failure, Ledger, Sessions, Task};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Agent { root: PathBuf }
impl Agent {
    fn call(&self, op: &str, name: &str, text: &str) -> Task<Value> {
        let (root,op,name,text) = (self.root.clone(),op.to_owned(),name.to_owned(),text.to_owned());
        Box::pin(async move {
            // estavel: este processo é a fronteira externa do agente; não simula host, store ou dispatcher.
            let output = tokio::process::Command::new("node").args(["--input-type=module", "-e", r#"
import fs from 'node:fs';
import path from 'node:path';
const [root,op,name,text] = process.argv.slice(1);
const file = path.join(root,name+'.json');
let reply;
if (op==='exists') reply=fs.existsSync(file);
if (op==='resolve') {
  let found;
  for(const candidate of fs.readdirSync(root).filter(n=>n.endsWith('.json'))) {
    const record=JSON.parse(fs.readFileSync(path.join(root,candidate),'utf8'));
    if((text && record.key===text) || (!text && candidate===name+'.json')) {
      found={name:candidate.slice(0,-5),key:record.key??candidate.slice(0,-5)+'-life',conversation:record.conversation??null};break;
    }
  }
  reply=found??null;
}
if (op==='open') {
  const record={options:JSON.parse(text),messages:[],key:name+'-life',conversation:name+'-conversation-1'};
  fs.writeFileSync(file,JSON.stringify(record),{flag:'wx'});
  reply=true;
}
if (op==='send') {
  const message=JSON.parse(text);
  const mission=JSON.parse(fs.readFileSync(path.join(root,'mission','mission.json'),'utf8'));
  if (!mission.items.some(i=>i.session===name)) process.exit(3);
  const record=JSON.parse(fs.readFileSync(file,'utf8'));
  if (record.key && record.key!==message.session.key) process.exit(4);
  record.messages.push(message.text);
  fs.writeFileSync(file,JSON.stringify(record));
  if (fs.existsSync(path.join(root,'offline-receipts'))) {
    fs.renameSync(path.join(root,'actions'),path.join(root,'offline-actions'));
    fs.mkdirSync(path.join(root,'actions'));
  }
  reply='queued';
}
process.stdout.write(JSON.stringify(reply));
"#]).arg(root).arg(op).arg(name).arg(text).env_clear().kill_on_drop(true).output().await
                .map_err(|_| Failure::new(503,"agent_unavailable","agente indisponível"))?;
            if !output.status.success() { return Err(Failure::new(503,"agent_failed","agente recusou a operação")); }
            serde_json::from_slice(&output.stdout).map_err(|_| Failure::new(503,"agent_invalid","resposta inválida"))
        })
    }
}
impl Sessions for Agent {
    fn validate_open(&self, options: Value) -> Result<Value, Failure> {
        if options.get("cwd").and_then(Value::as_str).is_none() { return Err(Failure::new(422,"open_invalid","pasta ausente")); }
        Ok(options)
    }
    fn resolve(&self, name: String, key: Option<String>) -> Task<Option<hangar_ticket_actions::Session>> {
        let call=self.call("resolve",&name,key.as_deref().unwrap_or_default());
        Box::pin(async move {serde_json::from_value(call.await?).map_err(|_|Failure::new(503,"agent_invalid","identidade inválida"))})
    }
    fn open(&self, name: String, options: Value) -> Task<()> {
        let call=self.call("open",&name,&options.to_string()); Box::pin(async move { call.await?; Ok(()) })
    }
    fn send(&self, session: hangar_ticket_actions::Session, text: String) -> Task<Delivery> {
        let call=self.call("send",&session.name,&json!({"text":text,"session":session}).to_string()); Box::pin(async move { call.await?; Ok(Delivery::Queued) })
    }
}

struct Bench { dir: tempfile::TempDir, engine: Arc<Engine>, app: axum::Router }
impl Bench {
    fn new(actions: bool) -> Self {
        Self::with_url(actions,None)
    }
    fn with_url(actions: bool, url: Option<&str>) -> Self {
        let dir=tempfile::tempdir().unwrap();
        let host=Host::new(Registry::new(dir.path().join("connectors.json")),HostOptions::default());
        let capabilities=if actions { json!({"work_tracking":1,"work_actions":1}) } else { json!({"work_tracking":1}) };
        let mut command=vec![env!("CARGO_BIN_EXE_hangar-fixture-connector"),"--id","prova"];
        if actions {command.extend(["--actions","yes"]);}
        if let Some(url)=url {command.extend(["--detail-url-size",url]);}
        host.add(Manifest::parse(&json!({"manifest_version":1,"id":"prova","name":"Prova","version":"1","protocol_version":1,
            "capabilities":capabilities,"command":command}).to_string().as_bytes(), dir.path()).unwrap()).unwrap();
        let engine=Engine::new(Ledger::new(dir.path().join("actions")),Arc::new(Store::new(dir.path().join("mission"))),host,
            Arc::new(Agent {root:dir.path().to_owned()}));
        let edge=Edge { trusted:TrustedHosts::parse("127.0.0.1"),auth:Auth::configured("owner"),
            diag:DiagClient::new("127.0.0.1:9".parse().unwrap(),String::new()).with_diary(Some(dir.path().join("diary"))) };
        let app=hangar_ticket_actions::http::router::<()>(edge,engine.clone());
        Self {dir,engine,app}
    }
    async fn send(&self, method: &str, path: &str, token: &str, body: Value) -> (u16, Value) {
        let mut request=Request::builder().method(method).uri(path).header("authorization",format!("Bearer {token}"))
            .body(Body::from(body.to_string())).unwrap();
        request.extensions_mut().insert(ConnectInfo(std::net::SocketAddr::from(([127,0,0,1],40000))));
        let response=self.app.clone().oneshot(request).await.unwrap();
        let status=response.status().as_u16();
        (status,serde_json::from_slice(&to_bytes(response.into_body(),1024*1024).await.unwrap()).unwrap())
    }
    async fn finish(&self, id: &str) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(30),self.engine.completed(id)).await.expect("ação precisa terminar").unwrap();
        let (_,value)=self.send("GET",&format!("/api/mission/actions/{id}"),"owner",Value::Null).await;
        value["attempt"].clone()
    }
}
impl Drop for Bench {
    fn drop(&mut self) {
        let host=self.engine.host.clone();
        std::thread::spawn(move||tokio::runtime::Runtime::new().unwrap().block_on(host.shutdown())).join().unwrap();
    }
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_without_message_records_stage_without_opening_a_session() {
    let bench=Bench::new(false);
    let id="11111111111111111111111111111111";
    let (status,body)=bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento"})).await;
    assert_eq!(status,202,"{body}");
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"recorded","{attempt}");
    let snapshot=bench.engine.store.snapshot().unwrap();
    assert_eq!(snapshot.items[0].stage.as_deref(),Some("planejamento"));
    assert_eq!(snapshot.items[0].session,None);
    assert!(!bench.dir.path().join("t-1.json").exists());
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_links_before_sending_and_repeated_requests_share_the_same_receipt() {
    let bench=Bench::new(true);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Respeite o escopo combinado.".into());
    bench.engine.store.set_stages(stages).unwrap();
    let catalog=bench.engine.host.list_actions("prova",hangar_connectors::work_tracking::GetParams::new("t-1").unwrap()).await.unwrap();
    let id="22222222222222222222222222222222";
    let request=json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","action":"plan","fingerprint":catalog[0].fingerprint,
        "open":{"cwd":bench.dir.path(),"provider":"codex","model":"modelo-da-prova"}});
    let (a,b)=tokio::join!(bench.send("POST","/api/mission/actions","owner",request.clone()),bench.send("POST","/api/mission/actions","owner",request.clone()));
    assert_eq!((a.0,b.0),(202,202));
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"queued","{attempt}");
    let session=attempt["session"].as_str().unwrap();
    let agent:Value=serde_json::from_slice(&std::fs::read(bench.dir.path().join(format!("{session}.json"))).unwrap()).unwrap();
    assert_eq!(agent["options"]["model"],"modelo-da-prova");
    let messages=agent["messages"].as_array().unwrap();
    assert_eq!(messages.len(),1,"tentativa repetida não repete o comando");
    assert!(messages[0].as_str().unwrap().contains("/iniciar #t-1"));
    assert!(messages[0].as_str().unwrap().contains("Respeite o escopo combinado."));
    assert_eq!(bench.engine.store.snapshot().unwrap().items[0].session.as_deref(),Some(session));
    let (_,repeated)=bench.send("POST","/api/mission/actions","owner",request).await;
    assert_eq!(repeated["attempt"]["status"],"queued");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_same_stage_does_not_repeat_but_explicit_run_reuses_the_linked_session() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Faça o planejamento.".into());
    stages[1].prompt=Some("Execute o plano aprovado.".into());
    bench.engine.store.set_stages(stages).unwrap();
    let first="33333333333333333333333333333333";
    let request=json!({"attempt":first,"connector":"prova","ticket_id":"t-1","stage":"planejamento","open":{"cwd":bench.dir.path()}});
    bench.send("POST","/api/mission/actions","owner",request).await;
    let attempt=bench.finish(first).await;
    assert_eq!(attempt["status"],"queued","{attempt}");
    let session=attempt["session"].as_str().unwrap();
    for (id,stage,repeat,expected) in [
        ("44444444444444444444444444444444","planejamento",false,"recorded"),
        ("55555555555555555555555555555555","execucao",false,"queued"),
        ("66666666666666666666666666666666","execucao",true,"queued"),
    ] {
        let request=json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":stage,"repeat":repeat});
        let (status,body)=bench.send("POST","/api/mission/actions","owner",request).await;
        assert_eq!(status,202,"{body}");
        let attempt=bench.finish(id).await;
        assert_eq!(attempt["status"],expected,"{attempt}");
        assert_eq!(attempt["session"],session);
    }
    let agent:Value=serde_json::from_slice(&std::fs::read(bench.dir.path().join(format!("{session}.json"))).unwrap()).unwrap();
    assert_eq!(agent["messages"].as_array().unwrap().len(),3);
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_uncertain_receipt_requires_confirmation_and_recovers_the_opened_session() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje o trabalho.".into());
    bench.engine.store.set_stages(stages).unwrap();
    let old="77777777777777777777777777777777";
    let previous=json!({"attempt":old,"connector":"prova","ticket_id":"t-1","stage":"planejamento"});
    bench.engine.ledger.reserve(old,&previous).unwrap();
    bench.engine.ledger.record(old,hangar_ticket_actions::Status::Uncertain,Some("sessao-recuperada"),Some("ticket_action_timeout")).unwrap();
    std::fs::write(bench.dir.path().join("sessao-recuperada.json"),json!({"messages":[]}).to_string()).unwrap();
    let id="88888888888888888888888888888888";
    let mut request=json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento"});
    let (status,body)=bench.send("POST","/api/mission/actions","owner",request.clone()).await;
    assert_eq!(status,409,"{body}");
    assert_eq!(body["detail"]["code"],"ticket_action_check_required");
    request["confirm_retry"]=json!(true);
    let (status,body)=bench.send("POST","/api/mission/actions","owner",request).await;
    assert_eq!(status,202,"{body}");
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"queued","{attempt}");
    assert_eq!(attempt["session"],"sessao-recuperada");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_refuses_a_stale_choice_missing_folder_and_another_tickets_receipt() {
    let bench=Bench::new(true);
    let id="99999999999999999999999999999999";
    let request=json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","action":"plan","fingerprint":"antigo","open":{"cwd":bench.dir.path()}});
    assert_eq!(bench.send("POST","/api/mission/actions","guest",request.clone()).await.0,401);
    assert!(bench.engine.ledger.latest("prova","t-1").unwrap().is_none());
    assert!(!bench.engine.host.list().unwrap()[0].running);
    bench.send("POST","/api/mission/actions","owner",request.clone()).await;
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"failed");
    assert_eq!(attempt["code"],"ticket_action_stale");
    assert!(attempt["message"].as_str().is_some_and(|text|!text.trim().is_empty()),"a recusa precisa conservar a causa para a tela");
    assert!(bench.engine.store.snapshot().unwrap().items.is_empty());
    let mut different=request;
    different["ticket_id"]=json!("t-2");
    let (status,body)=bench.send("POST","/api/mission/actions","owner",different).await;
    assert_eq!(status,409,"{body}");
    assert_eq!(body["detail"]["code"],"ticket_action_conflict");
    let catalog=bench.engine.host.list_actions("prova",hangar_connectors::work_tracking::GetParams::new("t-1").unwrap()).await.unwrap();
    let id="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","action":"plan","fingerprint":catalog[0].fingerprint})).await;
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"failed");
    assert_eq!(attempt["code"],"ticket_action_folder_required");
    assert!(bench.engine.store.snapshot().unwrap().items.is_empty());
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_follows_the_session_life_after_rename_and_refuses_a_reused_name() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje.".into());stages[1].prompt=Some("Execute.".into());stages[2].prompt=Some("Revise.".into());
    bench.engine.store.set_stages(stages).unwrap();
    let first="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":first,"connector":"prova","ticket_id":"t-1","stage":"planejamento","open":{"cwd":bench.dir.path()}})).await;
    let attempt=bench.finish(first).await;
    let old=attempt["session"].as_str().unwrap();
    std::fs::rename(bench.dir.path().join(format!("{old}.json")),bench.dir.path().join("renamed.json")).unwrap();
    std::fs::write(bench.dir.path().join(format!("{old}.json")),json!({"key":"unrelated-life","messages":[]}).to_string()).unwrap();
    let second="cccccccccccccccccccccccccccccccc";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":second,"connector":"prova","ticket_id":"t-1","stage":"execucao"})).await;
    let attempt=bench.finish(second).await;
    assert_eq!(attempt["status"],"queued","{attempt}");
    assert_eq!(attempt["session"],"renamed");
    std::fs::write(bench.dir.path().join("renamed.json"),json!({"key":"another-life","messages":[]}).to_string()).unwrap();
    let third="dddddddddddddddddddddddddddddddd";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":third,"connector":"prova","ticket_id":"t-1","stage":"revisao"})).await;
    let attempt=bench.finish(third).await;
    assert_eq!(attempt["status"],"failed","{attempt}");
    assert_eq!(attempt["code"],"ticket_session_changed");
    let unrelated:Value=serde_json::from_slice(&std::fs::read(bench.dir.path().join("renamed.json")).unwrap()).unwrap();
    assert!(unrelated["messages"].as_array().unwrap().is_empty());
    let history=bench.engine.store.snapshot().unwrap().items[0].history.len();
    let fourth="789abcdef0123456789abcdef0123456";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":fourth,"connector":"prova","ticket_id":"t-1","stage":"revisao","open":{"cwd":bench.dir.path()}})).await;
    let replacement=bench.finish(fourth).await;
    assert_eq!(replacement["status"],"queued","{replacement}");
    assert_ne!(replacement["session"],"renamed");
    assert!(bench.engine.store.snapshot().unwrap().items[0].history.len()>history,"abrir outra sessão conserva o histórico do ticket");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_recovery_never_takes_a_session_assigned_to_another_ticket() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje.".into());
    bench.engine.store.set_stages(stages).unwrap();
    let old="eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    bench.engine.ledger.reserve(old,&json!({"connector":"prova","ticket_id":"t-1","stage":"planejamento"})).unwrap();
    bench.engine.ledger.record(old,hangar_ticket_actions::Status::Uncertain,Some("assigned"),None).unwrap();
    std::fs::write(bench.dir.path().join("assigned.json"),json!({"key":"other-ticket-life","messages":[]}).to_string()).unwrap();
    bench.engine.store.link("prova","t-2",hangar_mission::store::Link {key:"T-2".into(),title:"Outro ticket".into(),url:"https://example.invalid/T-2".into(),session:"assigned".into()}).unwrap();
    let id="ffffffffffffffffffffffffffffffff";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","confirm_retry":true})).await;
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"failed","{attempt}");
    assert_eq!(attempt["code"],"ticket_session_assigned");
    let items=bench.engine.store.snapshot().unwrap().items;
    assert_eq!(items.iter().find(|i|i.ticket_id=="t-2").unwrap().session.as_deref(),Some("assigned"));
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_completed_receipt_is_still_idempotent_after_a_later_uncertain_attempt() {
    let bench=Bench::new(false);
    let first="0123456789abcdef0123456789abcdef";
    let request=json!({"attempt":first,"connector":"prova","ticket_id":"t-1","stage":"planejamento"});
    bench.send("POST","/api/mission/actions","owner",request.clone()).await;
    assert_eq!(bench.finish(first).await["status"],"recorded");
    let second="123456789abcdef0123456789abcdef0";
    bench.engine.ledger.reserve(second,&json!({"connector":"prova","ticket_id":"t-1","stage":"execucao"})).unwrap();
    bench.engine.ledger.record(second,hangar_ticket_actions::Status::Uncertain,None,None).unwrap();
    let (status,body)=bench.send("POST","/api/mission/actions","owner",request).await;
    assert_eq!(status,202,"{body}");
    assert_eq!(body["attempt"]["status"],"recorded");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_validates_store_metadata_before_opening_the_agent() {
    let bench=Bench::with_url(true,Some("5000"));
    let catalog=bench.engine.host.list_actions("prova",hangar_connectors::work_tracking::GetParams::new("t-1").unwrap()).await.unwrap();
    let id="23456789abcdef0123456789abcdef01";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","action":"plan","fingerprint":catalog[0].fingerprint,"open":{"cwd":bench.dir.path()}})).await;
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"failed","{attempt}");
    assert!(std::fs::read_dir(bench.dir.path()).unwrap().all(|entry|!entry.unwrap().file_name().to_string_lossy().starts_with("ticket-")),"metadados inválidos não abrem uma sessão");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_does_not_stay_sending_when_the_receipt_store_fails_after_delivery() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje.".into());bench.engine.store.set_stages(stages).unwrap();
    std::fs::write(bench.dir.path().join("offline-receipts"),"").unwrap();
    let id="3456789abcdef0123456789abcdef012";
    let request=json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","open":{"cwd":bench.dir.path()}});
    bench.send("POST","/api/mission/actions","owner",request.clone()).await;
    let _=tokio::time::timeout(std::time::Duration::from_secs(30),bench.engine.completed(id)).await.unwrap();
    let (status,body)=bench.send("GET",&format!("/api/mission/actions/{id}"),"owner",Value::Null).await;
    assert_eq!(status,200,"{body}");
    assert_eq!(body["attempt"]["status"],"uncertain","{body}");
    let (status,body)=bench.send("POST","/api/mission/actions","owner",request).await;
    assert_eq!(status,202,"{body}");
    assert_eq!(body["attempt"]["status"],"uncertain");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_move_only_records_the_stage_even_when_it_has_actions_and_a_prompt() {
    let bench=Bench::new(true);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje.".into());bench.engine.store.set_stages(stages).unwrap();
    let previous="56789abcdef0123456789abcdef01234";
    bench.engine.ledger.reserve(previous,&json!({"connector":"prova","ticket_id":"t-1","stage":"planejamento","execute":true})).unwrap();
    bench.engine.ledger.record(previous,hangar_ticket_actions::Status::Uncertain,None,None).unwrap();
    let id="456789abcdef0123456789abcdef0123";
    let (status,body)=bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","execute":false})).await;
    assert_eq!(status,202,"{body}");
    let attempt=bench.finish(id).await;
    assert_eq!(attempt["status"],"recorded","{attempt}");
    let items=bench.engine.store.snapshot().unwrap().items;
    assert_eq!(items[0].stage.as_deref(),Some("planejamento"));
    assert!(items[0].session.is_none());
    assert!(std::fs::read_dir(bench.dir.path()).unwrap().all(|entry|!entry.unwrap().file_name().to_string_lossy().starts_with("ticket-")));
    let (status,latest)=bench.send("GET","/api/mission/actions?connector=prova&ticket_id=t-1","owner",Value::Null).await;
    assert_eq!(status,200,"{latest}");
    assert_eq!(latest["attempt"]["status"],"recorded");
    assert_eq!(latest["execution"]["id"],previous);
    assert_eq!(latest["execution"]["status"],"uncertain");
    let catalog=bench.engine.host.list_actions("prova",hangar_connectors::work_tracking::GetParams::new("t-1").unwrap()).await.unwrap();
    let (status,body)=bench.send("POST","/api/mission/actions","owner",json!({"attempt":"6789abcdef0123456789abcdef012345","connector":"prova","ticket_id":"t-1","stage":"planejamento","action":"plan","fingerprint":catalog[0].fingerprint,"repeat":true})).await;
    assert_eq!(status,409,"mover não elimina o envio incerto anterior: {body}");
    assert_eq!(body["detail"]["code"],"ticket_action_check_required");
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_conversation_can_change_for_a_new_execution_but_not_for_orphan_recovery() {
    let bench=Bench::new(false);
    let mut stages=bench.engine.store.snapshot().unwrap().stages;
    stages[0].prompt=Some("Planeje.".into());stages[1].prompt=Some("Execute.".into());bench.engine.store.set_stages(stages).unwrap();
    let first="89abcdef0123456789abcdef01234567";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":first,"connector":"prova","ticket_id":"t-1","stage":"planejamento","open":{"cwd":bench.dir.path()}})).await;
    let first=bench.finish(first).await;
    let session=first["session"].as_str().unwrap();
    let path=bench.dir.path().join(format!("{session}.json"));
    let mut agent:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    agent["conversation"]=json!("conversation-after-clear");std::fs::write(&path,agent.to_string()).unwrap();
    let next="9abcdef0123456789abcdef012345678";
    bench.send("POST","/api/mission/actions","owner",json!({"attempt":next,"connector":"prova","ticket_id":"t-1","stage":"execucao"})).await;
    let next=bench.finish(next).await;
    assert_eq!(next["status"],"queued","{next}");
    assert_eq!(next["session"],session);
    assert_eq!(bench.engine.store.snapshot().unwrap().items[0].conversation.as_deref(),Some("conversation-after-clear"));

    let orphan=Bench::new(false);
    let mut stages=orphan.engine.store.snapshot().unwrap().stages;stages[1].prompt=Some("Execute.".into());orphan.engine.store.set_stages(stages).unwrap();
    let old="abcdef0123456789abcdef0123456789";
    orphan.engine.ledger.reserve(old,&json!({"connector":"prova","ticket_id":"t-1","stage":"execucao"})).unwrap();
    orphan.engine.ledger.bind(old,&hangar_ticket_actions::Session {name:"orphan".into(),key:"orphan-life".into(),conversation:Some("original".into())}).unwrap();
    orphan.engine.ledger.record(old,hangar_ticket_actions::Status::Uncertain,None,None).unwrap();
    std::fs::write(orphan.dir.path().join("orphan.json"),json!({"key":"orphan-life","conversation":"different","messages":[]}).to_string()).unwrap();
    let retry="bcdef0123456789abcdef0123456789a";
    orphan.send("POST","/api/mission/actions","owner",json!({"attempt":retry,"connector":"prova","ticket_id":"t-1","stage":"execucao","repeat":true,"confirm_retry":true})).await;
    let retry=orphan.finish(retry).await;
    assert_eq!(retry["status"],"failed","{retry}");
    assert_eq!(retry["code"],"ticket_session_changed");
    assert!(orphan.engine.store.snapshot().unwrap().items.is_empty());
}

#[tokio::test(flavor="multi_thread")]
async fn board_action_never_runs_only_the_prompt_when_a_configured_action_is_unavailable() {
    for (actions,code) in [(true,"ticket_action_stale"),(false,"unsupported_capability")] {
        let bench=Bench::new(actions);
        let mut stages=bench.engine.store.snapshot().unwrap().stages;
        stages[0].prompt=Some("Somente depois da ação configurada.".into());
        stages[0].actions=Some(std::collections::BTreeMap::from([("prova".into(),vec!["removed-action".into()])]));
        bench.engine.store.set_stages(stages).unwrap();
        let id="cdef0123456789abcdef0123456789ab";
        bench.send("POST","/api/mission/actions","owner",json!({"attempt":id,"connector":"prova","ticket_id":"t-1","stage":"planejamento","open":{"cwd":bench.dir.path()}})).await;
        let attempt=bench.finish(id).await;
        assert_eq!(attempt["status"],"failed","{attempt}");assert_eq!(attempt["code"],code);
        assert!(bench.engine.store.snapshot().unwrap().items.is_empty());
    }
}
