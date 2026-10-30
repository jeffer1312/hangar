use super::*;
use std::{fs,os::unix::fs::PermissionsExt};

fn logs(path:&Path)->Vec<Value> {
    let mut result = Vec::new();
    if !path.exists() {return result;}
    let mut entries = fs::read_dir(path).unwrap().map(|entry|entry.unwrap().path()).collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {result.extend(logs(&path));}
        else if path.extension().is_some_and(|extension|extension=="log") {
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777,0o600);
            result.push(serde_json::from_slice(&fs::read(path).unwrap()).unwrap());
        }
    }
    result
}

#[tokio::test]
async fn native_cli_matches_original_process_results_and_cleanup() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-cli.json")).unwrap();
    assert_fixture(fixture).await;
}

#[tokio::test]
async fn native_cli_waits_for_output_owned_by_descendants() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-cli-descendants.json")).unwrap();
    assert_fixture(fixture).await;
}

#[tokio::test]
async fn native_cli_preserves_python_json_values_and_encodings() {
    let fixture = crate::orchestration::codec::loads(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-cli-json.json")).unwrap();
    assert_fixture(fixture).await;
}

async fn assert_fixture(fixture:Value) {
    let mut differences = Vec::new();
    for row in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir_in(crate::list::links::without_verbatim(std::env::temp_dir().canonicalize().unwrap())).unwrap();
        let base = root.path().join("base");
        let home = root.path().join("home");
        let codex = home.join(".codex");
        let bin = root.path().join("bin");
        fs::create_dir_all(&codex).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let script = format!("#!/usr/bin/env python3\nimport sys\nsys.argv.insert(1,{scenario:?})\n{}",fixture["program"].as_str().unwrap());
        fs::write(bin.join("codex"),script).unwrap();
        fs::set_permissions(bin.join("codex"),fs::Permissions::from_mode(0o700)).unwrap();
        let env = AccountEnvironment::from_map([
            ("HOME".into(),base.to_string_lossy().into_owned()),
            ("PATH".into(),format!("{}:{}",bin.display(),std::env::var("PATH").unwrap())),
        ].into());
        let cli = AdminCli::new(env,&home,&codex);
        let (observer,mut receive) = tokio::sync::mpsc::unbounded_channel();
        *cli.inner.observer.lock().unwrap() = Some(observer);
        if row["name"] == "closed" { cli.close().await.unwrap(); }
        let worker = cli.clone();
        let operation = row["operation"].as_str().unwrap().to_owned();
        let timeout = Duration::from_millis(if scenario.contains("timeout") { 150 } else { 5000 });
        let task = tokio::spawn(async move {
            match operation.as_str() {
                "add"=>worker.add("a@m",timeout).await,
                "list"=>worker.plugins(timeout).await.map(Value::Array),
                "markets"=>worker.marketplaces(timeout).await.map(Value::Array),
                "upgrade"=>worker.upgrade("m",timeout).await,
                _=>worker.run(vec!["fixture".into(),"ação com espaço".into()],timeout).await,
            }
        });
        let mut directories = Vec::new();
        if matches!(scenario,"cancel"|"close") {
            if let Ok(Some(path)) = tokio::time::timeout(Duration::from_millis(500),receive.recv()).await {
                directories.push(path);
                if scenario=="cancel" {task.abort();} else {cli.close().await.unwrap();}
            }
        }
        let result = match task.await {
            Ok(Ok(value))=>json!({"ok":value}),
            Ok(Err(error))=>json!({"error":error,"class":"CodexNativoErro"}),
            Err(error) if error.is_cancelled()=>json!({"cancelled":true}),
            Err(error)=>panic!("tarefa da CLI falhou: {error}"),
        };
        cli.close().await.unwrap();
        while let Ok(path) = receive.try_recv() {directories.push(path);}
        let actual = json!({"result":result,"logs":logs(&base.join(".hangar/logs")),
            "operations_empty":cli.active_operations()==0,
            "directories_removed":directories.iter().all(|path|!path.exists()),
            "directories_outside_home":directories.iter().all(|path|path!=&home)});
        let mut encoded = crate::orchestration::codec::dumps(&actual,false).replace(root.path().to_str().unwrap(),"$ROOT");
        for directory in &directories {
            let physical = directory.parent().unwrap().canonicalize().unwrap().join(directory.file_name().unwrap());
            encoded=encoded.replace(physical.to_str().unwrap(),"$ADMIN").replace(directory.to_str().unwrap(),"$ADMIN");
        }
        let actual = crate::orchestration::codec::loads(&encoded).unwrap();
        let expected = json!({"result":row["result"],"logs":row["logs"],"operations_empty":row["operations_empty"],"directories_removed":row["directories_removed"],"directories_outside_home":row["directories_outside_home"]});
        if actual != expected {differences.push((row["name"].clone(),actual,expected));}
        if row["name"] != "closed" && directories.is_empty() {differences.push((row["name"].clone(),json!("nenhum processo observado"),json!("processo próprio")));}
    }
    assert!(differences.is_empty(),"CLI divergente: {differences:?}");
}
