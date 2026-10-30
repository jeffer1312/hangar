//! Falha na substituição conserva o destino e não deixa temporários nem derruba o chamador.
#[test]
fn update_state_failed_replacement_preserves_destination_and_cleans_temporary(){
 let captured:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/state-write-failure.json")).unwrap();let temp=tempfile::tempdir().unwrap();let state=hangar_server::update::StateFile::new(temp.path());std::fs::create_dir(state.path()).unwrap();std::fs::write(state.path().join("preserved"),"fixture").unwrap();
 state.write(serde_json::json!({"fase":"rodando","passo":1}));assert!(!state.publish(serde_json::json!({"fase":"rodando","passo":1})),"recusa explícita para quem precisa do resultado");assert_eq!(std::fs::read(state.path().join("preserved")).unwrap(),b"fixture");let temporary=std::fs::read_dir(temp.path()).unwrap().map(|e|e.unwrap().file_name().to_string_lossy().into_owned()).filter(|name|name.ends_with(".tmp")).collect::<Vec<_>>();assert_eq!(serde_json::json!(temporary),captured["cases"][0]["temporary_files"]);
}
