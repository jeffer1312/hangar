//! O texto exibido na falha não leva cores ou perde os separadores do original.
#[test]
fn update_error_tail_matches_original_ansi_and_line_boundaries(){
 let corpus:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/error-tail.json")).unwrap();let mut differences=vec![];
 for row in corpus["cases"].as_array().unwrap(){let value=hangar_server::update::process::ProcessOutput{code:1,stdout:row["stdout"].as_str().unwrap().into(),stderr:row["stderr"].as_str().unwrap().into()};let actual=hangar_server::update::git_engine::tail(&value,row["limit"].as_u64().unwrap()as usize);let reason=hangar_server::update::git_engine::failure_reason(&value);if reason!=row["reason"].as_str().unwrap(){differences.push(format!("{}: motivo {:?} != {}",row["name"],reason,row["reason"]));}if actual!=row["expected"].as_str().unwrap(){differences.push(format!("{}: {:?} != {}",row["name"],actual,row["expected"]));}}
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
