use super::*;
#[tokio::test]
async fn codex_service_round_finished_failure_is_not_awaited_when_sync_is_disabled(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/service-finished.json")).unwrap();let mut differences=vec![];
 for case in fixture["cases"].as_array().unwrap(){
  let root=tempfile::tempdir().unwrap();let mut service=super::round_tests::isolated_service(root.path());Arc::get_mut(&mut service).unwrap().config.env.base.insert("CP_CODEX_SYNC_ENABLED".into(),"false".into());service.state.update(|state|state["estado"]=json!("erro"));
  let outcome=if case["kind"]=="ok"{Ok(())}else{Err("fixture".to_owned())};let(_sender,result)=watch::channel(Some(outcome));let(cancel,_signal)=watch::channel(false);
  *service.rounds.lock().unwrap()=Some(Round{forced:false,result,cancel,identity:Arc::new(())});
  let actual=match service.update_and_wait("sessao",false).await{Ok(status)=>json!({"ok":status["estado"]}),Err(message)=>json!({"error":"RuntimeError","message":message})};if actual!=case["result"]{differences.push(json!({"case":case,"actual":actual}));}service.shutdown().await.unwrap();
 }
 assert!(differences.is_empty(),"Rodada concluída divergente: {differences:#?}");
}
