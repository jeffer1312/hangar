use hangar_server::{accounts::environment::AccountEnvironment,codex_setup::{skills,files}};
use serde_json::json;
proptest::proptest!{
 #![proptest_config(proptest::test_runner::Config::with_cases(48))]
 #[test]
 fn codex_service_skills_manifest_paths_do_not_remove_external_files(relative in "[a-z./]{0,24}"){
  let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let account=home.join(".codex");let source=home.join(".claude/skills/source");std::fs::create_dir_all(&source).unwrap();std::fs::write(source.join("SKILL.md"),b"source").unwrap();
  let destination=account.join("skills/source");std::fs::create_dir_all(&destination).unwrap();let external=account.join("sentinel");std::fs::write(&external,b"personal").unwrap();
  let mut entries=serde_json::Map::new();entries.insert(relative,json!(files::hash(b"personal")));
  let registry=json!({"source":{"path":destination,"mode":"copy","files":entries}});
  let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let _=skills::reconcile(&env,&account,&temp.path().join("repo"),&json!({}),&registry,&home.join("backups"),false,&mut skills::Disk);
  proptest::prop_assert_eq!(std::fs::read(external).unwrap(),b"personal");
 }
}
