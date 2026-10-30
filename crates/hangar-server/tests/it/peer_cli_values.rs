use serde_json::Value;
#[test]
fn peer_cli_values_match_python_stdout_and_error_codes(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/cli-values.json")).unwrap();let mut failures=vec![];
 for row in fixture["cases"].as_array().unwrap(){
  let home=tempfile::tempdir().unwrap();let path=home.path().join("peers.json");std::fs::write(&path,row["initial"].as_str().unwrap()).unwrap();let(mut out,mut err)=(vec![],vec![]);
  let code=hangar_server::machine_peers::cli::run(&path,&[row["command"].as_str().unwrap().into(),"a".into()],&mut out,&mut err);
  let stdout=out.iter().map(|byte|format!("{byte:02x}")).collect::<String>();let stderr=String::from_utf8(err).unwrap().replace(&path.display().to_string(),"<FILE>");
  let expected_error=if row["field"]=="id" && row["kind"]!="active"{row["stderr"].as_str().unwrap().to_owned()}else if row["rc"]==0{String::new()}else if row["value"]=="\"\\ud800\""{"erro: <FILE> inválido (texto contém Unicode inválido)\n".into()}else{"erro: <FILE> inválido (entrada 'a' sem endereço/token)\n".into()};
  if u64::from(code)!=row["rc"].as_u64().unwrap()||stdout!=row["stdout_hex"].as_str().unwrap()||stderr!=expected_error{failures.push(format!("{} {} {} {}: rc={code} stdout={stdout} stderr={stderr}",row["field"],row["value"],row["position"],row["command"]));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
