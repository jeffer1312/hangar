use hangar_server::{
    accounts::environment::AccountEnvironment,
    codex_setup::fingerprints,
};
use serde_json::json;

#[test]
fn codex_service_native_wrapper_fingerprint_ignores_obsolete_python_copies() {
    let temp = crate::physical_tempdir();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    let account = home.join(".codex");
    let env = AccountEnvironment::from_pairs([
        ("HOME", home.to_string_lossy().into_owned()),
        ("USERPROFILE", home.to_string_lossy().into_owned()),
    ]);
    let before = fingerprints::compute(&env, &account, &repo, &json!({}), false, "fragments", false).unwrap();
    for path in [repo.join("scripts/codex-hook-json.py"), account.join(".hangar-hooks/codex-hook-json.py")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "cópia antiga sem consumidor").unwrap();
    }
    let after = fingerprints::compute(&env, &account, &repo, &json!({}), false, "fragments", false).unwrap();
    assert_eq!(before, after);
}

proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(64))]
 #[test]
 fn codex_service_fingerprint_ignores_unselected_external_text(text in ".{0,1024}"){
  let dir=crate::physical_tempdir();let home=dir.path().join("home");let account=home.join(".codex");let repo=dir.path().join("repo");std::fs::create_dir_all(&account).unwrap();std::fs::create_dir_all(&repo).unwrap();let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
  let before=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();std::fs::write(account.join("config.toml"),format!("extra = {}\n",serde_json::to_string(&text).unwrap().replace('\u{7f}',"\\u007f"))).unwrap();let after=fingerprints::compute(&env,&account,&repo,&json!({}),false,"fragments",false).unwrap();proptest::prop_assert_eq!(before,after);
 }
}

#[test]
fn codex_service_fingerprint_detects_same_metadata_edits_and_plugin_selection() {
 let dir=crate::physical_tempdir();let home=dir.path().join("home");let account=home.join(".codex");let repo=dir.path().join("repo");
 std::fs::create_dir_all(&account).unwrap();std::fs::create_dir_all(home.join(".claude/skills/a")).unwrap();
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().into_owned()),("USERPROFILE",home.to_string_lossy().into_owned())]);
 let path=home.join(".claude/skills/a/SKILL.md");std::fs::write(&path,"first").unwrap();let modified=std::fs::metadata(&path).unwrap().modified().unwrap();
 let signature=||fingerprints::compute(&env,&account,&repo,&json!({}),false,"skills",false).unwrap();let before=signature();
 std::fs::write(&path,"other").unwrap();std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();assert_ne!(before,signature());
 let before=signature();std::fs::write(account.join("config.toml"),"[plugins.'a@m']\nenabled=true\n").unwrap();assert_ne!(before,signature());
}
