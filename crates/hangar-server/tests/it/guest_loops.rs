//! Borda pública dos loops: registros FastAPI capturados, sem upstream Python.
use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
use axum::{body::{Body,to_bytes},extract::ConnectInfo,http::Request};
use serde_json::Value;
use std::{net::SocketAddr,sync::Arc};
use tower::ServiceExt;
fn state(home:&std::path::Path)->Arc<AppState> {
    let mut state=AppState::new(Config{listen:"127.0.0.1:1".parse().unwrap(),upstream:crate::refused_address(),internal_secret:"fixture".into(),auth_token:"owner".into(),log_path:None,trusted:TrustedHosts::parse("")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.to_str().unwrap()),("USERPROFILE",home.to_str().unwrap())]));
    state.login_guests=Arc::new(hangar_server::login_guests::Service::new(hangar_server::login_guests::Settings {
        guests:home.join("guests.json"),vault:home.join("vault.json"),runtime_config:home.join("config.json"),sync_enabled:false,
        bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into(),
    }));
    state.share=None;Arc::new(state)
}
#[tokio::test]
async fn guest_loop_http_routing_matches_original() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/loop-routing.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let router=hangar_server::routes::router(state(home.path()));let mut failures=Vec::new();
    for (index,case) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        if case["status"]==200 {continue;}
        let mut request=Request::builder().method(case["method"].as_str().unwrap()).uri(case["path"].as_str().unwrap())
            .header("host","testserver").header("authorization","Bearer owner").body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response=router.clone().oneshot(request).await.unwrap();let status=response.status().as_u16();
        let headers=response.headers().clone();let body=to_bytes(response.into_body(),1048576).await.unwrap();
        let expected=case["headers"].as_object().unwrap();
        let actual= ["allow","location","content-type","content-length"].into_iter().filter_map(|key|headers.get(key).and_then(|v|v.to_str().ok()).map(|value|(key.to_owned(),Value::String(value.to_owned())))).collect::<serde_json::Map<_,_>>();
        let headers_match=&actual==expected;
        if status!=case["status"].as_u64().unwrap() as u16 || body.as_ref()!=case["body"].as_str().unwrap().as_bytes() || !headers_match {
            failures.push((index,status,String::from_utf8_lossy(&body).into_owned()));
        }
    }
    assert!(failures.is_empty(),"GUE-083/084: roteamento divergente {failures:?}");
}
#[tokio::test]
async fn guest_loop_http_auth_and_body_order_matches_original() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/loop-auth-order.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let router=hangar_server::routes::router(state(home.path()));let mut failures=Vec::new();
    for (index,case) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        // Bloqueio real e efeitos de sucesso têm provas próprias; aqui a entrada e seus erros.
        if case["busy"]==true || case["status"]==200 {continue;}
        let raw=case["body_hex"].as_str().unwrap().as_bytes().chunks_exact(2).map(|pair|u8::from_str_radix(std::str::from_utf8(pair).unwrap(),16).unwrap()).collect::<Vec<_>>();
        let mut builder=Request::post("/api/sessions/fixture/loop").header("authorization",format!("Bearer {}",case["token"].as_str().unwrap()));
        if let Some(media)=case["content_type"].as_str(){builder=builder.header("content-type",media);}
        let mut request=builder.body(Body::from(raw)).unwrap();request.extensions_mut().insert(ConnectInfo(format!("192.0.2.{}:1234",index+1).parse::<SocketAddr>().unwrap()));
        let response=router.clone().oneshot(request).await.unwrap();let status=response.status().as_u16();let raw=to_bytes(response.into_body(),1048576).await.unwrap();
        let body=serde_json::from_slice::<Value>(&raw).unwrap_or_else(|_|Value::String(String::from_utf8_lossy(&raw).into_owned()));
        if status!=case["status"].as_u64().unwrap() as u16 || body!=case["response"] {failures.push((index,status,body));}
    }
    assert!(failures.is_empty(),"GUE-083: ordem de entrada divergente {failures:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn guest_loop_http_login_stays_inside_its_session() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    let home=tempfile::tempdir().unwrap();let program=crate::list_support::sessions(home.path(),0);
    let mut state=Arc::try_unwrap(state(home.path())).ok().unwrap();
    let dirs=parse_dirs(&serde_json::json!({"home":home.path(),"claude":home.path().join(".claude"),"codex_home":home.path().join(".codex"),"pi_sessions":home.path().join(".pi"),"omp_config":home.path().join(".omp"),"omp_agent":home.path().join(".omp/agent"),"kimi_home":home.path().join(".kimi")}).to_string());
    state.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,std::time::Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs},FactsClient::new(crate::refused_address(),"fixture".into())));
    state.login_guests.store.create("Fixture",home.path().to_str().unwrap(),false,true,"guest".into(),"guest-token".into()).unwrap();
    state.login_guests.store.claim("fixture","guest",None).unwrap();
    let router=hangar_server::routes::router(Arc::new(state));
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/loop-gate.json")).unwrap();
    for case in fixture["rows"].as_array().unwrap() {
        let mut request=Request::builder().method(case["method"].as_str().unwrap()).uri(case["path"].as_str().unwrap()).header("authorization","Bearer guest-token").body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response=router.clone().oneshot(request).await.unwrap();let status=response.status().as_u16();let raw=to_bytes(response.into_body(),1048576).await.unwrap();
        assert_eq!(status,case["status"].as_u64().unwrap() as u16,"GUE-083/084 {}: {}",case["path"],String::from_utf8_lossy(&raw));
        assert_eq!(serde_json::from_slice::<Value>(&raw).unwrap(),case["response"],"{}",case["path"]);
    }
}

#[tokio::test]
async fn guest_loop_http_transfer_guard_matches_original() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/loop-transfer-order.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path());
    let runtime = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(
        crate::refused_address(), "fixture".into(), "fixture-instance".into()));
    runtime.ingress().hold("fixture", std::time::Duration::from_secs(1)).await.unwrap();
    assert!(state.state.runtime.set(runtime).is_ok());
    let router = hangar_server::routes::router(state);
    let mut failures = Vec::new();
    let mut count = 0;
    for (index, case) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        if case["busy"] != true { continue; }
        count += 1;
        let bytes = case["body_hex"].as_str().unwrap().as_bytes().chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect::<Vec<_>>();
        let mut builder = Request::post("/api/sessions/fixture/loop")
            .header("authorization", format!("Bearer {}", case["token"].as_str().unwrap()));
        if let Some(media) = case["content_type"].as_str() { builder = builder.header("content-type", media); }
        let mut request = builder.body(Body::from(bytes)).unwrap();
        request.extensions_mut().insert(ConnectInfo(format!("192.0.2.{}:1234", index + 1).parse::<SocketAddr>().unwrap()));
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), router.clone().oneshot(request)).await.unwrap().unwrap();
        let status = response.status().as_u16();
        let raw = to_bytes(response.into_body(), 1048576).await.unwrap();
        let body = serde_json::from_slice::<Value>(&raw).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned()));
        if status != case["status"].as_u64().unwrap() as u16 || body != case["response"] { failures.push((index, status, body)); }
    }
    assert_eq!(count, 42);
    assert!(failures.is_empty(), "GUE-083: trava de transferência divergente {failures:?}");
}

#[tokio::test]
async fn guest_loop_http_preflight_and_declared_limit_keep_middleware_order() {
    // Captura do middleware original: caminho irrelevante para preflight/413 antes do handler.
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/cors.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut state = state(home.path());
    Arc::get_mut(&mut Arc::get_mut(&mut state).unwrap().login_guests).unwrap().body_limit = 64;
    let router = hangar_server::routes::router(state);
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let preflight = case["method"] == "OPTIONS" && case["headers"]["origin"].is_string() && case["headers"]["access-control-request-method"].is_string();
        if !preflight && case["expected"]["status"] != 413 { continue; }
        count += 1;
        let mut builder = Request::builder().method(case["method"].as_str().unwrap()).uri("/api/sessions/fixture/loop");
        for (key, value) in case["headers"].as_object().unwrap() { builder = builder.header(key, value.as_str().unwrap()); }
        let mut request = builder.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().iter().map(|(key, value)| (key.as_str().to_owned(), Value::String(value.to_str().unwrap().to_owned()))).collect::<serde_json::Map<_, _>>();
        let raw = to_bytes(response.into_body(), 1048576).await.unwrap();
        assert_eq!(status, case["expected"]["status"].as_u64().unwrap() as u16, "GUE-080/083/084 {case}");
        assert_eq!(Value::Object(headers), case["expected"]["headers"], "GUE-080/083/084 {case}");
        assert_eq!(raw.as_ref(), case["expected"]["body"].as_str().unwrap().as_bytes(), "GUE-080/083/084 {case}");
    }
    assert_eq!(count, 32);
}

#[cfg(unix)]
#[tokio::test]
async fn guest_loop_selected_before_transfer_cannot_run_after_its_life_disappears() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    use hangar_server::loops::watch::Host;
    use std::os::unix::fs::PermissionsExt;
    for held in [false,true] {
        let home=tempfile::tempdir().unwrap();
        let program=crate::list_support::sessions(home.path(),0);
        let mut st=Arc::try_unwrap(state(home.path())).ok().unwrap();
        let dirs=parse_dirs(&serde_json::json!({"home":home.path(),"claude":home.path().join(".claude"),"codex_home":home.path().join(".codex"),"pi_sessions":home.path().join(".pi"),"omp_config":home.path().join(".omp"),"omp_agent":home.path().join(".omp/agent"),"kimi_home":home.path().join(".kimi")}).to_string());
        st.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,std::time::Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs},FactsClient::new(crate::refused_address(),"fixture".into())));
        let st=Arc::new(st);
        let runtime=Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(crate::refused_address(),"fixture".into(),"fixture-instance".into()));
        if held {runtime.ingress().hold("fixture",std::time::Duration::from_secs(1)).await.unwrap();}
        assert!(st.state.runtime.set(runtime).is_ok());
        let command=home.path().join("check");
        std::fs::write(&command,"#!/bin/sh\nprintf changed > called\n").unwrap();
        std::fs::set_permissions(&command,std::fs::Permissions::from_mode(0o700)).unwrap();
        let store=hangar_server::loops::store(&st);
        let mut initial=hangar_server::loops::store::new_loop("objetivo",command.to_str(),10,false,1.0);
        initial.insert("goal_delivered_ts".into(),serde_json::json!(1.0));
        store.set("fixture",&initial).unwrap();
        let path=store.loop_dir().join("fixture.json");
        let before=std::fs::read(&path).unwrap();
        let selected=serde_json::from_value(serde_json::json!({"name":"fixture","provider":"claude","jsonl":"vida-antiga.jsonl","lifecycle_id":"k:antiga","cwd":home.path()})).unwrap();
        let host=hangar_server::loops::watch::Live {st};
        tokio::time::timeout(std::time::Duration::from_secs(5),host.tick(selected)).await.unwrap();
        assert!(!home.path().join("called").exists(),"D59 held={held}: seleção antiga não autoriza comando");
        assert_eq!(std::fs::read(path).unwrap(),before,"D59: preserva o vínculo");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn guest_loop_live_tick_revalidates_life_and_keeps_the_valid_cycle() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    use hangar_server::loops::watch::Host;
    for changed in [false,true] {
        let root=tempfile::tempdir().unwrap();
        let home=root.path().join("home");
        let program=crate::list_support::sessions(root.path(),1);
        let script=std::fs::read_to_string(&program).unwrap().replace("\\t\\t\\t\\t0\\t0", "\\t\\tclaude\\t100\\t0\\t0");
        std::fs::write(&program,&script).unwrap();
        let mut st=Arc::try_unwrap(state(&home)).ok().unwrap();
        let dirs=parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi")}).to_string());
        st.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,std::time::Duration::from_secs(5)),capture_program:program.clone().into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs},FactsClient::new(crate::refused_address(),"fixture".into())));
        let st=Arc::new(st);
        let runtime=Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(crate::refused_address(),"fixture".into(),"fixture-instance".into()));
        assert!(st.state.runtime.set(runtime).is_ok());
        let selected=st.list.refresh().await.unwrap().rows.iter().find(|r|r.name=="s0").cloned().expect("sessão sintética descoberta");
        assert_eq!(selected.lifecycle_id.as_deref(),Some("t:100"));
        if changed {std::fs::write(&program,script.replace("\\t100\\t", "\\t200\\t")).unwrap();}
        let command=root.path().join("check");
        crate::write_executable(&command,"#!/bin/sh\nprintf changed > called\n");
        let store=hangar_server::loops::store(&st);
        let mut initial=hangar_server::loops::store::new_loop("objetivo",command.to_str(),10,false,1.0);
        initial.insert("goal_delivered_ts".into(),serde_json::json!(1.0));
        store.set("s0",&initial).unwrap();
        let path=store.loop_dir().join("s0.json");
        let before=std::fs::read(&path).unwrap();
        let host=hangar_server::loops::watch::Live {st};
        tokio::time::timeout(std::time::Duration::from_secs(5),host.tick(selected)).await.unwrap();
        let called=root.path().join("w0/called").exists();
        assert_eq!(called,!changed,"D59: comando só na vida selecionada");
        if changed {assert_eq!(std::fs::read(path).unwrap(),before);}
        else {assert_eq!(store.get("s0").unwrap()["status"],"done");}
    }
}

#[cfg(unix)]
#[tokio::test]
async fn guest_loop_http_rechecks_provider_after_transfer_before_any_effect() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    for resolve in [false,true] {
        let root=tempfile::tempdir().unwrap();
        let home=root.path().join("home");
        let program=crate::list_support::sessions(root.path(),1);
        let script=std::fs::read_to_string(&program).unwrap().replace("\\t\\t\\t\\t0\\t0", "\\t\\tclaude\\t100\\t0\\t0");
        std::fs::write(&program,&script).unwrap();
        let mut st=Arc::try_unwrap(state(&home)).ok().unwrap();
        let dirs=parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi")}).to_string());
        st.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,std::time::Duration::from_secs(5)),capture_program:program.clone().into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs},FactsClient::new(crate::refused_address(),"fixture".into())));
        let st=Arc::new(st);
        let runtime=Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(crate::refused_address(),"fixture".into(),"fixture-instance".into()));
        assert!(st.state.runtime.set(runtime).is_ok());
        assert_eq!(st.list.refresh().await.unwrap().rows.iter().find(|r|r.name=="s0").unwrap().provider,"claude");
        std::fs::write(&program,script.replace("\\tclaude\\t", "\\tcodex\\t")).unwrap();
        let store=hangar_server::loops::store(&st);
        let mut initial=hangar_server::loops::store::new_loop("objetivo",None,10,false,1.0);
        initial.insert("status".into(),serde_json::json!(if resolve {"done_claimed"} else {"done"}));
        store.set("s0",&initial).unwrap();
        let path=store.loop_dir().join("s0.json");
        let before=std::fs::read(&path).unwrap();
        let (url,body)=if resolve {("/api/sessions/s0/loop/resolve",serde_json::json!({"accept":false}))} else {("/api/sessions/s0/loop",serde_json::json!({"goal":"novo","require_branch":false}))};
        let mut request=Request::post(url).header("authorization","Bearer owner").header("content-type","application/json").body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response=hangar_server::routes::router(st).oneshot(request).await.unwrap();
        let status=response.status().as_u16();
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1048576).await.unwrap()).unwrap();
        assert_eq!((status,body["detail"]["code"].clone()),(409,serde_json::json!("erro_loop_provider_invalido")),"D59 resolve={resolve}: {body}");
        assert_eq!(std::fs::read(path).unwrap(),before,"D59: decisão usa o provedor atual sem alterar o vínculo");
    }
}
#[cfg(unix)]
#[tokio::test]
async fn guest_loop_http_discovery_failure_preserves_waiting_confirmation() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    for resolve in [true] {
        let root=tempfile::tempdir().unwrap();
        let home=root.path().join("home");
        let program=crate::list_support::sessions(root.path(),1);
        let script=std::fs::read_to_string(&program).unwrap().replace("\\t\\t\\t\\t0\\t0", "\\t\\tclaude\\t100\\t0\\t0");
        std::fs::write(&program,&script).unwrap();
        let mut st=Arc::try_unwrap(state(&home)).ok().unwrap();
        let dirs=parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi")}).to_string());
        st.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,std::time::Duration::from_secs(5)),capture_program:program.clone().into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs},FactsClient::new(crate::refused_address(),"fixture".into())));
        let st=Arc::new(st);
        let runtime=Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(crate::refused_address(),"fixture".into(),"fixture-instance".into()));
        assert!(st.state.runtime.set(runtime).is_ok());
        assert_eq!(st.list.refresh().await.unwrap().rows.iter().find(|r|r.name=="s0").unwrap().provider,"claude");
        std::fs::write(root.path().join("fail"), "fixture").unwrap();
        let store=hangar_server::loops::store(&st);
        let mut initial=hangar_server::loops::store::new_loop("objetivo",None,10,false,1.0);
        initial.insert("status".into(),serde_json::json!(if resolve {"done_claimed"} else {"done"}));
        store.set("s0",&initial).unwrap();
        let path=store.loop_dir().join("s0.json");
        let before=std::fs::read(&path).unwrap();
        let (url,body)=if resolve {("/api/sessions/s0/loop/resolve",serde_json::json!({"accept":false}))} else {("/api/sessions/s0/loop",serde_json::json!({"goal":"novo","require_branch":false}))};
        let mut request=Request::post(url).header("authorization","Bearer owner").header("content-type","application/json").body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response=hangar_server::routes::router(st).oneshot(request).await.unwrap();
        let status=response.status().as_u16();
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1048576).await.unwrap()).unwrap();
        assert_eq!((status,body["detail"]["code"].clone()),(503,serde_json::json!("mux_unavailable")),"D59 resolve={resolve}: {body}");
        assert_eq!(std::fs::read(path).unwrap(),before,"D59: decisão usa o provedor atual sem alterar o vínculo");
    }
}
