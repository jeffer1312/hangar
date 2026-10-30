use hangar_server::boot_version::{self,Snapshot};
proptest::proptest!{
    #[test]
    fn boot_version_injection_is_scoped_and_first_publication_wins(describe in ".{0,50}",readable in proptest::option::of(".{0,30}")){
        let root=tempfile::tempdir().unwrap();let other=tempfile::tempdir().unwrap();
        let snapshot=Snapshot{describe,readable};
        proptest::prop_assert_eq!(boot_version::publish(root.path(),snapshot.clone()),snapshot.clone());
        proptest::prop_assert_eq!(boot_version::get(root.path()),Some(snapshot.clone()));
        proptest::prop_assert_eq!(boot_version::get(other.path()),None);
        proptest::prop_assert_eq!(boot_version::publish(root.path(),Snapshot{describe:"later".into(),readable:None}),snapshot);
    }
}
#[test]
fn boot_version_get_does_not_wait_for_capture_or_block_other_roots(){
    use std::{sync::mpsc,time::Duration};
    let root=tempfile::tempdir().unwrap();let ready=tempfile::tempdir().unwrap();
    let expected=Snapshot{describe:"ready".into(),readable:None};
    boot_version::publish(ready.path(),expected.clone());
    let (entered_tx,entered_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
    let capture_root=root.path().to_owned();
    let capture=std::thread::spawn(move||{let mut first=true;boot_version::capture_with(&capture_root,|_,_|{if first{first=false;entered_tx.send(()).unwrap();release_rx.recv().unwrap();}Err(())})});
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (read_tx,read_rx)=mpsc::channel();let reading=root.path().to_owned();let ready_path=ready.path().to_owned();
    let reader=std::thread::spawn(move||read_tx.send((boot_version::get(&reading),boot_version::get(&ready_path))).unwrap());
    let observed=read_rx.recv_timeout(Duration::from_millis(300));
    release_tx.send(()).unwrap();capture.join().unwrap();reader.join().unwrap();
    assert_eq!(observed.unwrap(),(None,Some(expected)),"a consulta não pode aguardar os subprocessos da captura");
}
#[test]
fn capture_process_preserves_separate_channels_environment_and_timeout(){
    use std::{collections::BTreeMap,time::{Duration,Instant}};
    let root=tempfile::tempdir().unwrap();let args=vec![std::env::current_exe().unwrap().display().to_string(),"--exact".into(),"boot_version::capture_process_child".into(),"--ignored".into(),"--nocapture".into()];
    let mut environment=BTreeMap::from([("HANGAR_CAPTURE_FIXTURE".into(),"valor sintético".into())]);
    let output=hangar_server::update::process::run_capture_with_env(&args,root.path(),Duration::from_secs(3),&environment).unwrap();
    assert_eq!(output.code,0);assert!(output.stdout.contains("valor sintético\n\u{1b}[31mvalor\u{1b}[0m  \n"));assert!(!output.stdout.contains("diagnóstico"));assert_eq!(output.stderr,"diagnóstico\n");
    environment.insert("HANGAR_CAPTURE_WAIT".into(),"1".into());let began=Instant::now();
    assert!(matches!(hangar_server::update::process::run_capture_with_env(&args,root.path(),Duration::from_millis(150),&environment),Err(hangar_server::update::process::ProcessError::Timeout{..})));
    assert!(began.elapsed()<Duration::from_secs(3));
}
#[test]
#[ignore]
fn capture_process_child(){
    use std::io::Write;
    if std::env::var_os("HANGAR_CAPTURE_WAIT").is_some(){std::thread::sleep(std::time::Duration::from_secs(30));return;}
    std::io::stdout().write_all(format!("{}\r\n\u{1b}[31mvalor\u{1b}[0m  \r",std::env::var("HANGAR_CAPTURE_FIXTURE").unwrap()).as_bytes()).unwrap();
    std::io::stderr().write_all("diagnóstico\r\n".as_bytes()).unwrap();
}
