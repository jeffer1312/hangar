use std::sync::Arc;
use hangar_connectors::{Host,HostOptions,manifest::Manifest,registry::Registry};
use hangar_help_connector::provided::Provided;
use serde_json::json;

#[test]
fn provided_help_tracks_the_server_without_rewriting_the_registered_command_or_enabling_it() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("connectors.json");
    let registry=Registry::new(path.clone());
    let original=Manifest::parse(json!({"manifest_version":1,"id":"help-config","name":"Help","version":"1","protocol_version":1,
        "capabilities":{"work_tracking":1},"start_prompt":"/iniciar {key}",
        "command":["./hangar-help-test-connector","--id","help-config","--help-url","http://127.0.0.1:9","--config-dir","./credentials"]}).to_string().as_bytes(),dir.path()).unwrap();
    registry.add(original.clone()).unwrap();registry.set_enabled("help-config",false).unwrap();
    let before=std::fs::read(&path).unwrap();
    let executable=dir.path().join(if cfg!(windows) {"hangar-server.exe"} else {"hangar-server"});
    let host=Host::with_resolver(Registry::new(path.clone()),HostOptions::default(),Arc::new(Provided::new(executable.clone())));
    let effective=host.entry("help-config").unwrap();
    assert_eq!(effective.capabilities.get("work_actions"),Some(&1));
    assert!(!effective.enabled);assert_eq!(effective.cwd,dir.path());
    assert_eq!(effective.command[0],executable.to_string_lossy());assert_eq!(effective.command[1],"help-connector");
    assert_eq!(&effective.command[2..],&original.command[1..]);
    assert_eq!(effective.start_prompt.as_deref(),Some("/iniciar {key}"));
    assert_eq!(host.list().unwrap()[0].entry.capabilities.get("work_actions"),Some(&1));
    assert_eq!(Registry::new(path.clone()).get("help-config").unwrap().manifest,original);
    // estavel: o contrato é não modificar o cadastro, para permitir voltar ao servidor anterior.
    assert_eq!(std::fs::read(path).unwrap(),before);
}
