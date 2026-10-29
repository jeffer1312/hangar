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

#[tokio::test]
async fn public_peers_sibling_special_fields_preserve_errors_and_effects(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/peer-json-siblings.json")).unwrap();
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let state=Arc::new(state);let service=hangar_server::machine_peers::Service::from_state(&state).unwrap();assert!(service.store.path.starts_with(home.path()));assert!(service.env.starts_with(home.path()));
    let router=hangar_server::routes::router(state);
    let task=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap(){
        std::fs::write(config.join("peers.json"),row["initial"].as_str().unwrap()).unwrap();std::fs::write(config.join(".env"),"").unwrap();
        let response=client.put(format!("http://{address}{}",row["path"].as_str().unwrap())).bearer_auth("contract-owner").header("content-type","application/json").body(row["wire"].as_str().unwrap().to_owned()).send().await.unwrap();
        let status=response.status().as_u16();let mut actual:Value=serde_json::from_str(&response.text().await.unwrap()).unwrap();
        let mut expected:Value=serde_json::from_str(row["response"].as_str().unwrap()).unwrap();
        if expected["detail"]["msg"].as_str().is_some_and(|msg|msg.starts_with("'utf-8' codec")){
            assert_eq!(actual["detail"]["msg"], "texto contém Unicode inválido", "{}", row["name"]);
            actual["detail"]["msg"]=Value::Null;expected["detail"]["msg"]=Value::Null;
        }
        let effect=std::fs::read_to_string(config.join("peers.json")).unwrap();let env=std::fs::read_to_string(config.join(".env")).unwrap();
        if status!=row["status"].as_u64().unwrap() as u16||actual!=expected||effect!=row["effect"].as_str().unwrap()||env!=row["env"].as_str().unwrap(){failures.push(format!("{}: status{status}/{}, response={actual}",row["name"],row["status"]));}
    }
    task.abort();assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[tokio::test]
async fn peers_input_order_and_mime_match_python_without_writes() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
    use std::sync::Arc;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/input-order.json")).unwrap();
    let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
        state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
        let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
        let client=reqwest::Client::new();let mut request=client.request(row["method"].as_str().unwrap().parse().unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap())).body(row["body"].as_str().unwrap().to_owned());
        if row["authorized"]==true {request=request.bearer_auth("contract-owner");}
        if let Some(value)=row["content_type"].as_str(){request=request.header("content-type",value);}
        let response=request.send().await.unwrap();let status=response.status().as_u16();let actual:Value=response.json().await.unwrap();server.abort();
        let expected:Value=if row["status"]==401{serde_json::json!({"detail":{"code":"erro_nao_autorizado","msg":"unauthorized","params":{}}})}
            else {serde_json::from_str(row["text"].as_str().unwrap()).unwrap()};
        if status!=row["status"].as_u64().unwrap() as u16 || actual!=expected {failures.push(format!("{} auth={} type={} body={}: {status} {actual} != {} {expected}",row["path"],row["authorized"],row["content_type"],row["body"],row["status"]));}
        assert_eq!(std::fs::read_to_string(config.join("peers.json")).unwrap(),row["peers"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(config.join(".env")).unwrap(),row["env"].as_str().unwrap());
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[test]
fn peers_model_validation_matches_python_for_all_scalar_kinds() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/models.json")).unwrap();
    let mut failures=vec![];
    for (index,row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","http_peers::peers_model_child","--ignored","--nocapture"]).env("HANGAR_PEER_MODEL_CASE",index.to_string()).output().unwrap();
        if !output.status.success(){failures.push(format!("{} caso{index}: {} {}",row["path"],String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr)));}
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
#[tokio::test]
#[ignore="validação profunda executada em subprocesso"]
async fn peers_model_child() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState,orchestration::codec};
    use std::sync::Arc;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/models.json")).unwrap();let index:usize=std::env::var("HANGAR_PEER_MODEL_CASE").unwrap().parse().unwrap();let row=&fixture["cases"][index];
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"").unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let response=reqwest::Client::new().request(row["method"].as_str().unwrap().parse().unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap())).bearer_auth("contract-owner").header("content-type","application/json").body(row["body"].as_str().unwrap().to_owned()).send().await.unwrap();
    let status=response.status().as_u16();let body=response.text().await.unwrap();server.abort();
    let deep=row["validation"]["exception"]=="RecursionError";
    assert_eq!(status,if deep{422}else{row["status"].as_u64().unwrap() as u16});
    if status==500{assert_eq!(body,row["text"].as_str().unwrap());}else{
        let expected=if deep{row["validation"]["json"].as_str().unwrap()}else{row["text"].as_str().unwrap()};
        let actual=codec::loads(&body).unwrap();let expected=codec::loads(expected).unwrap();
        assert_eq!(codec::dumps(&actual,true),codec::dumps(&expected,true));codec::discard(actual);codec::discard(expected);
    }
    assert_eq!(std::fs::read_to_string(config.join("peers.json")).unwrap(),row["peers"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(config.join(".env")).unwrap(),row["env"].as_str().unwrap());
}
