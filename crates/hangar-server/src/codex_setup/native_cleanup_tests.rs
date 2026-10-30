use super::*;
use std::{fs,io::Write,os::unix::fs::PermissionsExt};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())}
    fn flush(&mut self)->std::io::Result<()>{Ok(())}
}

#[tokio::test]
async fn native_cli_retries_cleanup_without_hiding_the_primary_error_or_diagnostic() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-cli-cleanup-failure.json")).unwrap();
    let mut differences=vec![];
    for case in fixture["cases"].as_array().unwrap(){
        let log=Arc::new(Mutex::new(Vec::new()));let writer=log.clone();
        let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move||Capture(writer.clone())).finish();
        let _subscriber=tracing::subscriber::set_default(subscriber);
        let root=tempfile::tempdir().unwrap();let bin=root.path().join("bin");let home=root.path().join("home");
        fs::create_dir_all(&bin).unwrap();fs::create_dir_all(&home).unwrap();
        let proceed=root.path().join("proceed");
        let scenario=case["scenario"].as_str().unwrap();
        let script=format!("#!/usr/bin/env python3\nimport os,sys,time\nwhile not os.path.exists({:?}): time.sleep(0.005)\nif {scenario:?} == 'exit': sys.exit(7)\nprint('{{}}')\n",proceed.to_str().unwrap());
        fs::write(bin.join("codex"),script).unwrap();fs::set_permissions(bin.join("codex"),fs::Permissions::from_mode(0o700)).unwrap();
        let env=AccountEnvironment::from_map([
            ("HOME".into(),home.display().to_string()),
            ("PATH".into(),format!("{}:{}",bin.display(),std::env::var("PATH").unwrap())),
        ].into());
        let cli=AdminCli::new(env,&home,&home.join(".codex"));
        let (sender,mut receiver)=tokio::sync::mpsc::unbounded_channel();*cli.inner.observer.lock().unwrap()=Some(sender);
        let worker=cli.clone();let task=tokio::spawn(async move{worker.run(vec!["fixture".into()],Duration::from_secs(5)).await});
        let directory=tokio::time::timeout(Duration::from_secs(3),receiver.recv()).await.unwrap().unwrap();
        let retained=root.path().join("retained");
        fs::rename(&directory,&retained).unwrap();fs::write(&directory,"obstáculo").unwrap();fs::write(&proceed,"").unwrap();
        let result=match task.await.unwrap(){Ok(value)=>json!({"ok":value}),Err(error)=>json!({"error":error,"class":"CodexNativoErro"})};
        let operations_retained=cli.active_operations()!=0;
        fs::remove_file(&directory).unwrap();fs::rename(&retained,&directory).unwrap();cli.close().await.unwrap();
        let message="limpeza da pasta temporária do Codex adiada após falha do comando";
        let log=String::from_utf8(log.lock().unwrap().clone()).unwrap();
        let warnings=vec![message;log.matches(message).count()];
        let actual=json!({"scenario":scenario,"result":result,"operations_empty":cli.active_operations()==0,
            "operations_retained":operations_retained,"directories_removed":!directory.exists(),"warnings":warnings});
        if actual!=*case{differences.push((actual,case.clone()));}
    }
    assert!(differences.is_empty(),"Limpeza administrativa divergente: {differences:?}");
}
