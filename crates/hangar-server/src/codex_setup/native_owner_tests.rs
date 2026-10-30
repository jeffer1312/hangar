use super::*;
use std::{fs,os::unix::fs::PermissionsExt};

#[tokio::test]
async fn native_owner_preserves_memory_flag_and_closes_admitted_commands() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-owner.json")).unwrap();
    let mut differences=Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let root=tempfile::tempdir().unwrap();
        let home=root.path().join("home");let codex=home.join(".codex");let bin=root.path().join("bin");
        fs::create_dir_all(&codex).unwrap();fs::create_dir_all(&bin).unwrap();
        fs::write(codex.join("config.toml"),"model = \"fixture\"\n").unwrap();
        let observed=root.path().join("observed.json");
        let program=fixture["program"].as_str().unwrap().replace("    time.sleep(30)","    Path(str(sys.argv[1])+'.cli').write_text(os.getcwd())\n    time.sleep(30)");
        let script=format!("#!/usr/bin/env python3\nimport sys\nsys.argv.insert(1,{:?})\n{program}",observed.to_str().unwrap());
        fs::write(bin.join("codex"),script).unwrap();fs::set_permissions(bin.join("codex"),fs::Permissions::from_mode(0o700)).unwrap();
        let env=AccountEnvironment::from_map([
            ("HOME".into(),home.to_string_lossy().into_owned()),
            ("PATH".into(),format!("{}:{}",bin.display(),std::env::var("PATH").unwrap())),
        ].into());
        let mut native=Native::open_with_memory(&env,&home,&codex,case["memory"].as_bool().unwrap()).await.unwrap();
        let observation:Value=serde_json::from_slice(&fs::read(&observed).unwrap()).unwrap();
        let cwd=PathBuf::from(observation["cwd"].as_str().unwrap());
        let cli=native.cli();
        let worker=if case["command"]==true {
            let command=cli.clone();
            let worker=tokio::spawn(async move {command.run(vec!["fixture".into()],Duration::from_secs(5)).await});
            let ready=observed.with_file_name("observed.json.cli");
            tokio::time::timeout(Duration::from_secs(2),async {while !ready.exists(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            Some((worker,PathBuf::from(fs::read_to_string(ready).unwrap())))
        } else {None};
        native.close().await.unwrap();
        let empty=cli.active_operations()==0;
        let removed=!cwd.exists() && worker.as_ref().is_none_or(|(_,path)|!path.exists());
        // A prova termina os processos mesmo quando a asserção do dono ainda está vermelha.
        cli.close().await.unwrap();
        let result=if let Some((worker,_))=worker {
            match worker.await.unwrap(){Ok(value)=>json!({"ok":value}),Err(error)=>json!({"error":error})}
        } else {Value::Null};
        let actual=json!({"argv":observation["argv"],"cwd_outside_home":cwd!=home,
            "operations_empty":empty,"directories_removed":removed,
            "config":fs::read_to_string(codex.join("config.toml")).unwrap(),"result":result});
        let expected=json!({"argv":case["argv"],"cwd_outside_home":case["cwd_outside_home"],
            "operations_empty":case["operations_empty"],"directories_removed":case["directories_removed"],
            "config":case["config"],"result":case["result"]});
        if actual!=expected{differences.push(json!({"memory":case["memory"],"command":case["command"],"actual":actual,"expected":expected}));}
    }
    assert!(differences.is_empty(),"Dono nativo divergente: {differences:#?}");
}
