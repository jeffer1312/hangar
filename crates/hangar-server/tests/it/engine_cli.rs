//! O CLI resolve o motor sem serviço nem interpretador Python disponível.
use std::process::Command;
use serde_json::json;
fn command(home: &std::path::Path) -> Command {
    let mut command=Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.arg("engine").env_clear().env("HOME",home).env("USERPROFILE",home).env("PATH","");
    command
}
#[test]
fn engine_cli_without_python_or_service() {
    let home=tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/engines.json"),json!({"motor":{"base_url":"https://provider.invalid","api_key":"fixture-secret","model":"m"}}).to_string()).unwrap();
    let list=command(home.path()).arg("--list").output().unwrap();
    assert!(list.status.success(),"{}",String::from_utf8_lossy(&list.stderr));
    assert!(String::from_utf8_lossy(&list.stdout).contains("motor"));
    assert!(!String::from_utf8_lossy(&list.stdout).contains("fixture-secret"));
    let env=command(home.path()).args(["--env","motor"]).output().unwrap();
    assert!(env.status.success(),"{}",String::from_utf8_lossy(&env.stderr));
    assert!(String::from_utf8_lossy(&env.stdout).contains("ANTHROPIC_BASE_URL"));
    let absent=command(home.path()).args(["--env","inexistente"]).output().unwrap();
    assert!(!absent.status.success());
    #[cfg(unix)] {
        let run=command(home.path()).args(["--exec","motor","--","/bin/sh","-c","printf '%s\\n' \"$ANTHROPIC_BASE_URL\" \"$ANTHROPIC_AUTH_TOKEN\" \"$ANTHROPIC_MODEL\""]).output().unwrap();
        assert!(run.status.success(),"{}",String::from_utf8_lossy(&run.stderr));
        assert_eq!(String::from_utf8(run.stdout).unwrap(),"https://provider.invalid\nfixture-secret\nm\n");
    }
}

proptest::proptest! {
    #[test]
    fn engine_names_never_escape_store(name in ".{0,96}") {
        use hangar_server::engines::store;
        let result=store::normalize(&name,&json!({"base_url":"https://provider.invalid","api_key":"fixture"}).as_object().unwrap().clone());
        if result.is_ok() {
            proptest::prop_assert!(name.len()<=32);
            proptest::prop_assert!(name.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'_'||b==b'-'));
            proptest::prop_assert!(!name.starts_with('-')&&!name.starts_with('_'));
        }
    }
}
