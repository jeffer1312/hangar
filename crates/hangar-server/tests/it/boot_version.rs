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
