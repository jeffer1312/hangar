use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
use std::sync::Arc;
use serde_json::Value;
#[tokio::test]
async fn public_peers_methods_paths_headers_and_redirects_match_python(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/routing.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let state=Arc::new(state);let service=hangar_server::machine_peers::Service::from_state(&state).unwrap();assert!(service.store.path.starts_with(home.path()));assert!(service.env.starts_with(home.path()));
    let router=hangar_server::routes::router(state);
    let task=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap(){
        let path=row["path"].as_str().unwrap();if path.starts_with("/api/update-channel"){continue;}
        let mut request=client.request(row["method"].as_str().unwrap().parse().unwrap(),format!("http://{address}{path}"));if row["authenticated"]==true{request=request.bearer_auth("contract-owner");}
        let response=request.send().await.unwrap();let status=response.status().as_u16();let headers=response.headers().clone();let body=response.text().await.unwrap();
        let fields=[("allow","allow"),("content-type","content_type"),("content-length","content_length")];
        let mismatch=fields.iter().any(|(header,key)|headers.get(*header).and_then(|v|v.to_str().ok())!=row[*key].as_str());
        let location=headers.get("location").and_then(|v|v.to_str().ok()).unwrap_or("").replace(&format!("http://{address}"),"<BASE>");
        if status!=row["status"].as_u64().unwrap() as u16||body!=row["body"].as_str().unwrap()||mismatch||location!=row["location"].as_str().unwrap(){failures.push(format!("{} {path} auth={}: status{status}/{}, headers={headers:?}, body={body}",row["method"],row["authenticated"],row["status"]));}
    }
    task.abort();assert!(failures.is_empty(),"{} diferenças; {}",failures.len(),failures.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
}

#[tokio::test]
async fn public_peers_special_json_matches_python_and_disk_effects(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/peer-json.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let state=Arc::new(state);let service=hangar_server::machine_peers::Service::from_state(&state).unwrap();assert!(service.store.path.starts_with(home.path()));assert!(service.env.starts_with(home.path()));
    let router=hangar_server::routes::router(state);
    let task=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap(){
        std::fs::write(config.join("peers.json"),"{}").unwrap();
        let response=client.post(format!("http://{address}/api/peers")).bearer_auth("contract-owner").header("content-type","application/json").body(row["wire"].as_str().unwrap().to_owned()).send().await.unwrap();
        let status=response.status().as_u16();let mut actual:Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();
        let mut expected:Value=serde_json::from_str(row["response"].as_str().unwrap()).unwrap();
        if row["name"]=="token-\"\\ud800\"" {actual["detail"]["msg"]=Value::Null;expected["detail"]["msg"]=Value::Null;}
        let effect=std::fs::read_to_string(config.join("peers.json")).unwrap();
        if status!=row["status"].as_u64().unwrap() as u16||actual!=expected||effect!=row["effect"].as_str().unwrap(){failures.push(format!("{}: status{status}/{}, response={actual}",row["name"],row["status"]));}
    }
    task.abort();assert!(failures.is_empty(),"{}",failures.join("\n"));
}
