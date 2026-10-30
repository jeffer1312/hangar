use serde_json::Value;
fn fixture()->Value {serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/channel-special.json")).unwrap()}
#[cfg(unix)]
#[test]
fn update_channel_special_json_matches_original_endpoint() {
    let mut failures=vec![];
    for (index,row) in fixture()["cases"].as_array().unwrap().iter().enumerate() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_channel_http::update_channel_special_child","--ignored","--nocapture"]).env("HANGAR_CHANNEL_CASE",index.to_string()).output().unwrap();
        if !output.status.success(){failures.push(format!("{}: {} {}",row["name"],String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr)));}
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
#[cfg(unix)]
#[tokio::test]
#[ignore="corpos profundos são conferidos em subprocesso"]
async fn update_channel_special_child() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState,orchestration::codec};
    use std::{sync::Arc,os::unix::fs::PermissionsExt};
    let fixture=fixture();let index:usize=std::env::var("HANGAR_CHANNEL_CASE").unwrap().parse().unwrap();let row=&fixture["cases"][index];
    let home=tempfile::tempdir().unwrap();let root=home.path().join(".hangar/current");let config=home.path().join(".hangar/config");let bin=home.path().join("bin");
    for path in [root.join("backend"),config.clone(),bin.clone()]{std::fs::create_dir_all(path).unwrap();}
    let file=config.join(".env");std::fs::write(&file,"CP_UPDATE_BRANCH=\nCP_UPDATE_LAST_BRANCH=\n").unwrap();
    let git=bin.join("git");std::fs::write(&git,"#!/bin/sh\ncase \"$*\" in *ls-remote*) printf 'fixture\\trefs/heads/beta\\n';; *) printf 'main\\n';; esac\n").unwrap();std::fs::set_permissions(&git,std::fs::Permissions::from_mode(0o755)).unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap()),("PATH",bin.to_str().unwrap())]));
    state.settings_snapshot().unwrap();
    let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let response=reqwest::Client::new().put(format!("http://{address}/api/update-channel")).bearer_auth("fixture-owner").header("content-type","application/json").body(row["body"].as_str().unwrap().to_owned()).send().await.unwrap();
    let status=response.status().as_u16();let body=response.text().await.unwrap();server.abort();
    let deep=row["validation"]["exception"]=="RecursionError";
    assert_eq!(status,if deep{422}else{row["status"].as_u64().unwrap() as u16});
    let expected=if deep{row["validation"]["json"].as_str().unwrap()}else{row["text"].as_str().unwrap()};
    if status==500{assert_eq!(body,expected);}else{
        let actual=codec::loads(&body).unwrap();let expected=codec::loads(expected).unwrap();
        assert_eq!(codec::dumps(&actual,true),codec::dumps(&expected,true));codec::discard(actual);codec::discard(expected);
    }
    let written=std::fs::read_to_string(&file).unwrap();
    if row["effects"].as_array().unwrap().is_empty(){assert_eq!(written,"CP_UPDATE_BRANCH=\nCP_UPDATE_LAST_BRANCH=\n");}else{assert!(written.contains("CP_UPDATE_BRANCH=beta\n"));assert!(written.contains("CP_UPDATE_LAST_BRANCH=beta\n"));}
}

#[cfg(unix)]
#[tokio::test]
async fn update_channel_methods_match_original_endpoint() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/channel-methods.json")).unwrap();
    let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
    use std::{sync::Arc,os::unix::fs::PermissionsExt};
    let home=tempfile::tempdir().unwrap();let root=home.path().join(".hangar/current");let config=home.path().join(".hangar/config");let bin=home.path().join("bin");
    for path in [root.join("backend"),config.clone(),bin.clone()]{std::fs::create_dir_all(path).unwrap();}
    let file=config.join(".env");std::fs::write(&file,"CP_UPDATE_BRANCH=\nCP_UPDATE_LAST_BRANCH=\n").unwrap();
    let git=bin.join("git");std::fs::write(&git,"#!/bin/sh\ncase \"$*\" in *ls-remote*) printf 'fixture\\trefs/heads/beta\\n';; *) printf 'main\\n';; esac\n").unwrap();std::fs::set_permissions(&git,std::fs::Permissions::from_mode(0o755)).unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap()),("PATH",bin.to_str().unwrap())]));
    state.settings_snapshot().unwrap();
    let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});

        let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let response=client.request(reqwest::Method::from_bytes(row["method"].as_str().unwrap().as_bytes()).unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap())).bearer_auth("fixture-owner").header("host","fixture").header("content-type","application/json").body("{\"branch\":\"beta\"}").send().await.unwrap();
        let status=response.status().as_u16();let headers=response.headers().clone();let body=response.text().await.unwrap();server.abort();
        if status!=row["status"].as_u64().unwrap() as u16 {failures.push(format!("{} {}: status{status} != {}",row["method"],row["path"],row["status"]));continue;}
        for key in ["allow","content-type","location"] {
            if headers.get(key).and_then(|v|v.to_str().ok())!=row["headers"][key].as_str(){failures.push(format!("{} {}: cabeçalho{key}",row["method"],row["path"]));}
        }
        if status!=200 {
            if body!=row["text"].as_str().unwrap(){failures.push(format!("{} {}: corpo{body}",row["method"],row["path"]));}
            if headers.get("content-length").and_then(|v|v.to_str().ok())!=row["headers"]["content-length"].as_str(){failures.push(format!("{} {}: content-length",row["method"],row["path"]));}
        }else {
            let actual:Value=serde_json::from_str(&body).unwrap();let expected:Value=serde_json::from_str(row["text"].as_str().unwrap()).unwrap();
            if actual!=expected{failures.push(format!("{} {}: JSON{actual} != {expected}",row["method"],row["path"]));}
        }
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[cfg(unix)]
#[tokio::test]
async fn update_channel_auth_and_content_type_follow_original_pipeline() {
    let mut cases=vec![];
    for (raw,authorized) in [(include_str!("../../../../backend/tests/fixtures/contract/update-git/channel-content-type.json"),true),(include_str!("../../../../backend/tests/fixtures/contract/update-git/channel-auth-order.json"),false)] {
        let fixture:Value=serde_json::from_str(raw).unwrap();
        for row in fixture["cases"].as_array().unwrap() {let mut row=row.clone();row["authorized"]=Value::Bool(authorized);if authorized{row["body"]=Value::String("{\"branch\":\"beta\"}".into());}else{row["content_type"]=Value::String("application/json".into());}cases.push(row);}
    }
    let mut failures=vec![];
    for row in &cases {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
    use std::{sync::Arc,os::unix::fs::PermissionsExt};
    let home=tempfile::tempdir().unwrap();let root=home.path().join(".hangar/current");let config=home.path().join(".hangar/config");let bin=home.path().join("bin");
    for path in [root.join("backend"),config.clone(),bin.clone()]{std::fs::create_dir_all(path).unwrap();}
    let file=config.join(".env");std::fs::write(&file,"CP_UPDATE_BRANCH=\nCP_UPDATE_LAST_BRANCH=\n").unwrap();
    let git=bin.join("git");std::fs::write(&git,"#!/bin/sh\ncase \"$*\" in *ls-remote*) printf 'fixture\\trefs/heads/beta\\n';; *) printf 'main\\n';; esac\n").unwrap();std::fs::set_permissions(&git,std::fs::Permissions::from_mode(0o755)).unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap()),("PATH",bin.to_str().unwrap())]));
    state.settings_snapshot().unwrap();
    let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});


        let client=reqwest::Client::new();let mut request=client.put(format!("http://{address}/api/update-channel")).body(row["body"].as_str().unwrap().to_owned());
        if row["authorized"]==true{request=request.bearer_auth("fixture-owner");}
        if let Some(value)=row["content_type"].as_str(){request=request.header("content-type",value);}
        let response=request.send().await.unwrap();let status=response.status().as_u16();let body=response.text().await.unwrap();server.abort();
        if status!=row["status"].as_u64().unwrap() as u16 {failures.push(format!("auth={} type={} body={}: status{status} != {}",row["authorized"],row["content_type"],row["body"],row["status"]));continue;}
        let actual:Value=serde_json::from_str(&body).unwrap();
        if status==401 {assert_eq!(actual["detail"]["code"],"erro_nao_autorizado");}
        else {let expected:Value=serde_json::from_str(row["text"].as_str().unwrap()).unwrap();if actual!=expected{failures.push(format!("body{actual} != {expected}"));}}
        let written=std::fs::read_to_string(&file).unwrap();
        if row["effects"].as_array().is_some_and(|effects|!effects.is_empty()){assert!(written.contains("CP_UPDATE_BRANCH=beta\n"));}else{assert_eq!(written,"CP_UPDATE_BRANCH=\nCP_UPDATE_LAST_BRANCH=\n");}
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}

#[tokio::test]
async fn update_checkout_methods_and_query_match_python(){
 use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
 use std::sync::Arc;
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/update-http.json")).unwrap();
 let home=tempfile::tempdir().unwrap();let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
 let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
 state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap()),("PATH","")]));state.settings_snapshot().unwrap();
 let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
 let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();let mut failures=vec![];
 for row in fixture["cases"].as_array().unwrap(){
  let path=row["path"].as_str().unwrap();
  if row["status"]==200 {
   if let Some((_,query))=path.split_once('?') {let expected:Value=serde_json::from_str(row["body"].as_str().unwrap()).unwrap();let actual=hangar_server::update::git_engine::http::refresh_query(Some(query)).unwrap();if actual!=expected["procurar"].as_bool().unwrap(){failures.push(format!("consulta {query}: {actual}"));}}
   continue;
  }
  let response=client.request(reqwest::Method::from_bytes(row["method"].as_str().unwrap().as_bytes()).unwrap(),format!("http://{address}{path}")).header("host","fixture").bearer_auth("fixture-owner").send().await.unwrap();
  let status=response.status().as_u16();let headers=response.headers().clone();let body=response.text().await.unwrap();
  if status!=row["status"].as_u64().unwrap() as u16||body!=row["body"].as_str().unwrap(){failures.push(format!("{} {path}: status{status}, corpo{body}",row["method"]));}
  for key in ["content-type","content-length","allow","location"]{if headers.get(key).and_then(|v|v.to_str().ok())!=row["headers"][key].as_str(){failures.push(format!("{} {path}: cabeçalho {key}",row["method"]));}}
 }
 server.abort();assert!(failures.is_empty(),"{}",failures.join("\n"));
}
