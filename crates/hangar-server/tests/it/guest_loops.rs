//! Borda pública dos loops, sem upstream Python.
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
    for maximum in [0, 10] {
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
        let mut initial=hangar_server::loops::store::new_loop("objetivo",None,maximum,false,1.0);
        initial.insert("status".into(),serde_json::json!("done_claimed"));
        store.set("s0",&initial).unwrap();
        let path=store.loop_dir().join("s0.json");
        let before=std::fs::read(&path).unwrap();
        let (url,body)=("/api/sessions/s0/loop/resolve",serde_json::json!({"accept":false}));
        let mut request=Request::post(url).header("authorization","Bearer owner").header("content-type","application/json").body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
        request.extensions_mut().insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response=hangar_server::routes::router(st).oneshot(request).await.unwrap();
        let status=response.status().as_u16();
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1048576).await.unwrap()).unwrap();
        assert_eq!((status,body["detail"]["code"].clone()),(503,serde_json::json!("mux_unavailable")),"D59 max_iters={maximum}: {body}");
        assert_eq!(std::fs::read(path).unwrap(),before,"D59: decisão usa o provedor atual sem alterar o vínculo");
    }
}
