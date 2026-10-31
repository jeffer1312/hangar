#[test]
fn peer_file_cli_rejects_the_unconsumed_json_command(){
 let home=tempfile::tempdir().unwrap();let path=home.path().join("peers.json");std::fs::write(&path,r#"{"extra":NaN,"surrogate":"\ud800"}"#).unwrap();
 let(mut out,mut err)=(vec![],vec![]);let code=hangar_server::machine_peers::cli::run(&path,&["json".into()],&mut out,&mut err);
 assert_eq!(code,2);assert!(out.is_empty());assert_eq!(String::from_utf8(err).unwrap(),"uso: hangar-server peers path|config <id>|tsv\n");
}
