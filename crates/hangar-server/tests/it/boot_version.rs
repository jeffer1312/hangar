use hangar_server::boot_version::{self,Snapshot};
use serde_json::{Value,json};
#[test]
fn boot_version_matches_python_and_never_recaptures_during_get(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/boot-version.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap(){
        let root=tempfile::tempdir().unwrap();
        if !row["manifest"].is_null(){std::fs::write(root.path().join("manifest.json"),serde_json::to_vec(&row["manifest"]).unwrap()).unwrap();}
        if let Some(version)=row["version_file"].as_str(){std::fs::write(root.path().join("VERSION"),version).unwrap();}
        assert!(boot_version::get(root.path()).is_none());
        let mut replies=row["replies"].as_array().unwrap().iter();let mut calls=vec![];
        let snapshot=boot_version::capture_with(root.path(),|args,timeout|{
            calls.push(json!({"args":std::iter::once("git").chain(args.iter().copied()).collect::<Vec<_>>(),"timeout":timeout.as_secs()}));
            let reply=replies.next().expect("git foi chamado além da captura");
            if reply=="error"{Err(())}else{Ok(reply[1].as_str().unwrap().into())}
        });
        assert_eq!(snapshot,Snapshot{describe:row["describe"].as_str().unwrap().into(),readable:row["readable"].as_str().map(str::to_owned)},"{}",row["name"]);
        assert_eq!(calls,row["calls"].as_array().unwrap().clone(),"{}",row["name"]);
        std::fs::write(root.path().join("manifest.json"),r#"{"version":"changed","commit":"new","channel":"latest"}"#).unwrap();
        assert_eq!(boot_version::get(root.path()),Some(snapshot.clone()));
        assert_eq!(boot_version::capture_with(root.path(),|_,_|panic!("boot repetido consultou git")),snapshot);
    }
}
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
