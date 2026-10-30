use super::*;
fn corpus()->Value{serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/fingerprint-review.json")).unwrap()}
#[test]
fn codex_service_fingerprint_review_glob_includes_dotfile_and_windows_unicode_case(){
 let corpus=corpus();let mut failures=Vec::new();
 for case in corpus["glob_cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();
  for name in case["names"].as_array().unwrap(){let name=name.as_str().unwrap();if name.starts_with("directory"){std::fs::create_dir(temp.path().join(name)).unwrap();}else{std::fs::write(temp.path().join(name),"{}").unwrap();}}
  let actual=records(temp.path(),case["windows"].as_bool().unwrap()).unwrap().iter().map(|path|path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
  if json!(actual)!=case["found"]{failures.push(json!({"windows":case["windows"],"actual":actual,"expected":case["found"]}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
#[test]
fn codex_service_fingerprint_review_utf8_preserves_class_only_in_instructions(){
 let corpus=corpus();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let account=home.join(".codex");let repo=temp.path().join("repo");
  for path in [&account,&repo,&home.join(".claude")]{std::fs::create_dir_all(path).unwrap();}
  std::fs::write(home.join(".claude/CLAUDE.md"),"@./import.md\n").unwrap();
  let hex=case["hex"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();std::fs::write(home.join(case["path"].as_str().unwrap()),raw).unwrap();
  let env=AccountEnvironment::from_map([("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned())].into());
  let actual=match compute(&env,&account,&repo,&json!({}),false,case["phase"].as_str().unwrap(),true){Ok(_)=>json!({"ok":true}),Err(error)=>json!({"error":error.kind,"message":error.message})};
  // D03/D39 conserva a classe; o diagnóstico técnico nativo tem comparação explícita.
  let expected=if case["result"]["error"]=="UnicodeDecodeError"{json!({"error":"UnicodeDecodeError","message":"Arquivo de instruções não está em UTF-8"})}else{case["result"].clone()};
  if actual!=expected{failures.push(json!({"case":case,"actual":actual,"expected":expected}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
