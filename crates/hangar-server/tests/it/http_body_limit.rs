use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
use std::sync::Arc;

#[tokio::test]
async fn large_ignored_peer_fields_keep_the_python_acceptance_limit(){
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/large-body.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let state=Arc::new(state);let service=hangar_server::machine_peers::Service::from_state(&state).unwrap();assert!(service.store.path.starts_with(home.path()));assert!(service.env.starts_with(home.path()));
    let router=hangar_server::routes::router(state);
    let task=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let client=reqwest::Client::new();
    for row in fixture["cases"].as_array().unwrap(){
        let extra="x".repeat(row["extra_bytes"].as_u64().unwrap() as usize);
        let wire=format!("{{\"id\": \"remote\", \"base_url\": \"https://peer.invalid\", \"token\": \"fixture-token\", \"extra\": \"{extra}\"}}");
        assert_eq!(wire.len(),row["wire_bytes"].as_u64().unwrap() as usize);
        let response=client.post(format!("http://{address}/api/peers")).bearer_auth("contract-owner").header("content-type","application/json").body(wire).send().await.unwrap();
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"{} bytes extras",row["extra_bytes"]);
        assert_eq!(response.json::<serde_json::Value>().await.unwrap(),row["response"]);
        let effect:serde_json::Value=serde_json::from_slice(&std::fs::read(config.join("peers.json")).unwrap()).unwrap();assert_eq!(effect,row["effect"]);
    }
    task.abort();
}

#[tokio::test]
async fn body_limit_matches_original_header_stream_and_auth_order(){
    use axum::{body::{Body,Bytes,to_bytes},http::{HeaderMap,HeaderValue},response::Response};
    use futures_util::StreamExt;
    use hangar_server::http_body_limit as limit;
    use std::sync::atomic::{AtomicUsize,Ordering};
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/body-limit.json")).unwrap();
    assert_eq!(limit::MAX_BYTES,fixture["limit"].as_u64().unwrap() as usize);
    for (index,row) in fixture["cases"].as_array().unwrap().iter().enumerate(){
        let mut headers=HeaderMap::new();if let Some(header)=row["header"].as_str(){headers.insert("content-length",HeaderValue::from_str(header).unwrap());}
        let reads=Arc::new(AtomicUsize::new(0));let mut entered=0;
        let response=if let Some(response)=limit::precheck(&headers){response}else{
            entered+=1;
            if row["authorized"]==true{
                let counts=row["chunks"].as_array().unwrap().iter().map(|n|n.as_u64().unwrap() as usize).collect::<Vec<_>>();let observed=reads.clone();
                let stream=futures_util::stream::iter(counts).map(move|count|{observed.fetch_add(1,Ordering::SeqCst);Ok::<_,std::io::Error>(Bytes::from(vec![b'x';count]))});
                match limit::read(Body::from_stream(stream)).await{
                    Ok(_)=>Response::new(Body::from("OK")),
                    Err(error) if limit::is_limit(&error)=>limit::rejected(),
                    Err(error)=>panic!("falha inesperada no caso {index}: {error}"),
                }
            }else{Response::builder().status(401).body(Body::from("unauthorized")).unwrap()}
        };
        assert_eq!(entered,row["entered"].as_u64().unwrap(),"entrada caso {index}");
        assert_eq!(reads.load(Ordering::SeqCst),row["reads"].as_u64().unwrap() as usize,"leituras caso {index}");
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"status caso {index}");
        let actual:serde_json::Map<String,serde_json::Value>=response.headers().iter().map(|(k,v)|(k.to_string(),v.to_str().unwrap().into())).collect();
        assert_eq!(serde_json::Value::Object(actual),row["headers"],"headers caso {index}");
        assert_eq!(to_bytes(response.into_body(),1024).await.unwrap().as_ref(),row["body"].as_str().unwrap().as_bytes(),"corpo caso {index}");
    }
}
