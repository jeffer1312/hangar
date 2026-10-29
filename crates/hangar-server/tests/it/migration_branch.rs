use hangar_server::migration_branch::Branch;
use serde_json::{Value,json};
use std::{collections::VecDeque,time::Duration};
#[test]
fn migration_branch_matches_captured_lifetime_and_retry() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/migration-branch.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut branch=Branch::default();
        let mut responses:VecDeque<_>=case["responses"].as_array().unwrap().iter().collect();
        let mut calls=vec![];
        let values:Vec<_>=case["times"].as_array().unwrap().iter().map(|at| {
            branch.resolve_with(Duration::from_secs(at.as_u64().unwrap()),case["manifest"]["channel"].as_str(),|args,timeout| {
                calls.push(json!({"args":args,"timeout":timeout.as_secs()}));
                let row=responses.pop_front().unwrap();Ok((row[0].as_i64().unwrap() as i32,row[1].as_str().unwrap().into()))
            })
        }).collect();
        assert_eq!(json!(values),case["values"],"{}",case["name"]);
        assert_eq!(json!(calls),case["calls"],"{}",case["name"]);
    }
}

#[test]
fn migration_branch_services_are_isolated_and_freeze_the_first_valid_manifest() {
    let root=tempfile::tempdir().unwrap();
    let manifest=|channel:&str|json!({"version":"1","commit":"fixture","channel":channel}).to_string();
    std::fs::write(root.path().join("manifest.json"),manifest("beta")).unwrap();
    let first=hangar_server::migration_branch::Service::default();
    let env=std::collections::BTreeMap::new();
    assert_eq!(first.get(root.path(),&env,Duration::ZERO),Some("beta".into()));
    std::fs::write(root.path().join("manifest.json"),manifest("changed")).unwrap();
    assert_eq!(first.get(root.path(),&env,Duration::from_secs(120)),Some("beta".into()));
    assert_eq!(hangar_server::migration_branch::Service::default().get(root.path(),&env,Duration::ZERO),Some("changed".into()));
}

proptest::proptest! {
    #[test]
    fn migration_branch_freezes_success_and_strips_python_whitespace(value in "[a-z][a-z0-9/_-]{0,100}") {
        let mut branch=Branch::default();
        let padded=format!("\u{1c} {value}\n\u{85}");
        proptest::prop_assert_eq!(branch.resolve_with(Duration::ZERO,None,|_,_|Ok((0,padded.clone()))),Some(value.clone()));
        proptest::prop_assert_eq!(branch.resolve_with(Duration::from_secs(1000),None,|_,_|panic!("branch congelada não deve executar git")),Some(value));
    }
}

#[tokio::test]
async fn migration_status_reads_owned_branch_and_live_source_without_changing_metrics() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
    use std::sync::Arc;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/migration-source.json")).unwrap();
    let mut failures=vec![];
    for row in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();let root=home.path().join(".hangar/current");let config=home.path().join(".hangar/config");
        std::fs::create_dir_all(root.join("backend")).unwrap();std::fs::create_dir_all(&config).unwrap();
        let env_file=config.join(".env");std::fs::write(&env_file,"CP_UPDATE_BRANCH=fixture-branch\n").unwrap();
        if row["name"]=="package" {
            std::fs::write(root.join("manifest.json"),json!({"version":"0.1.0.1","commit":"fixture","channel":"beta","protocol":1,"api":5,"os":hangar_server::update::platform(),"files":{}}).to_string()).unwrap();
            hangar_server::update::save_channel(&hangar_server::update::Layout::new(home.path()),"latest","beta").unwrap();
        } else {
            for args in [vec!["init","-b","main"],vec!["-c","user.name=Contract","-c","user.email=contract@example.invalid","commit","--allow-empty","-m","fixture"]] {
                let output=std::process::Command::new("git").arg("-C").arg(&root).args(args).env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",if cfg!(windows){"NUL"}else{"/dev/null"}).output().unwrap();
                assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            }
        }
        let mut facts=row["facts"].clone();facts.as_object_mut().unwrap().remove("branch");facts.as_object_mut().unwrap().remove("update_branch");
        let upstream=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let upstream_address=upstream.local_addr().unwrap();
        let upstream_router=axum::Router::new().route("/internal/migration/status",axum::routing::get(move||{let facts=facts.clone();async move{([(axum::http::header::CONTENT_TYPE,"application/json")],facts.to_string())}}));
        let upstream_task=tokio::spawn(async move{axum::serve(upstream,upstream_router).await.unwrap();});
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let mut state=AppState::new(Config{listen:address,upstream:upstream_address,internal_secret:"fixture-internal".into(),auth_token:"fixture-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
        state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
        state.settings_snapshot().unwrap();
        match row["file_hex"].as_str() {
            Some(hex)=>{let bytes:Vec<u8>=hex.as_bytes().chunks_exact(2).map(|pair|u8::from_str_radix(std::str::from_utf8(pair).unwrap(),16).unwrap()).collect();std::fs::write(&env_file,bytes).unwrap();},
            None=>std::fs::remove_file(&env_file).unwrap(),
        }
        let router=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
        let response=reqwest::Client::new().get(format!("http://{address}/api/migration/status")).bearer_auth("fixture-owner").send().await.unwrap();
        let status=response.status();let body:Value=response.json().await.unwrap();
        if row["name"]=="invalid-utf8" {
            if status!=503||body["detail"]["code"]!="migration_status_config_unavailable"{failures.push(format!("{}: {status} {body}",row["name"]));}
        }else {
            let mut expected=row["facts"].clone();if row["name"]!="valid"{expected["update_branch"]=Value::Null;}
            if status!=200||body["python"]!=expected{failures.push(format!("{}: {status} {} != {expected}",row["name"],body["python"]));}
        }
        server.abort();upstream_task.abort();
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}
