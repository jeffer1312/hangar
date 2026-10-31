use hangar_server::update::StateFile;
use serde_json::json;
#[test]
fn update_state_refusal_is_visible_before_package_spawn() {
    use hangar_server::update::{executor, state_dir};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Recorder(AtomicUsize);
    impl executor::Spawner for Recorder {
        fn spawn(&self, _: &[String]) -> std::io::Result<u32> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(std::process::id())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let claude = temp.path().join(".claude");
    let dir = state_dir(&claude);
    std::fs::create_dir_all(&dir).unwrap();
    let file = StateFile::new(&dir);
    let original = br#"{"extra":"\ud800"}"#;
    std::fs::write(file.path(), original).unwrap();
    let recorder = Recorder(AtomicUsize::new(0));
    let args = executor::Args::parse(&["--run".into()]).unwrap();
    let outcome = executor::start(temp.path(), &claude, &args, &recorder);
    assert!(matches!(outcome, Err(executor::StartError::Launch(ref message)) if message == "update_state_invalid_unicode: texto contém surrogate Unicode isolado"));
    assert_eq!(recorder.0.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
}

#[test]
fn update_state_returns_specific_refusal_without_replacing_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = StateFile::new(temp.path());
    for (raw, expected, operation) in [
        (r#"{"extra":"\ud800"}"#, "update_state_invalid_unicode: texto contém surrogate Unicode isolado", 0),
        (r#"{"log":true}"#, "update_state_invalid_log", 1),
        (r#"{"avisos":true}"#, "update_state_invalid_warnings", 2),
    ] {
        std::fs::write(file.path(), raw).unwrap();
        let result = match operation { 0 => file.try_write(json!({"ok":true})), 1 => file.try_log("linha"), _ => file.try_warn("aviso") };
        assert_eq!(result.unwrap_err(), expected);
        assert_eq!(std::fs::read(file.path()).unwrap(), raw.as_bytes());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn update_state_restart_never_announces_success_after_write_refusal() {
    use hangar_server::{accounts::environment::AccountEnvironment, update::{self, executor}};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Restart { path: std::path::PathBuf, calls: AtomicUsize }
    impl update::Restarter for Restart {
        async fn restart(&self) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&self.path, br#"{"extra":"\ud800"}"#).unwrap();
            Ok(())
        }
    }
    struct Health<'a>(&'a Restart);
    impl update::HealthProbe for Health<'_> {
        async fn read(&self) -> update::Reading {
            update::Reading::Rust { protocol: 1, pid: Some(10 + self.0.calls.load(Ordering::SeqCst) as u32), commit: None }
        }
    }
    for initially_invalid in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_pairs([("HOME", temp.path().to_str().unwrap()), ("USERPROFILE", temp.path().to_str().unwrap())]);
        let args = executor::Args::parse(&["--restart".into(), "--request-id".into(), "fixture".into()]).unwrap();
        let parts = executor::Parts::new(&env, &args, "http://127.0.0.1:1");
        let file = StateFile::new(&parts.state_dir);
        std::fs::create_dir_all(&parts.state_dir).unwrap();
        if initially_invalid { std::fs::write(file.path(), br#"{"extra":"\ud800"}"#).unwrap(); }
        let restart = Restart { path: file.path().into(), calls: AtomicUsize::new(0) };
        let settings = update::Settings { health_deadline: std::time::Duration::from_millis(100), health_interval: std::time::Duration::from_millis(1), ..Default::default() };
        assert!(!executor::restart_only(&parts, "fixture", &restart, &Health(&restart), &settings).await, "inicialmente inválido={initially_invalid}");
        assert_eq!(restart.calls.load(Ordering::SeqCst), usize::from(!initially_invalid));
        assert_eq!(std::fs::read(file.path()).unwrap(), br#"{"extra":"\ud800"}"#);
    }
}

#[test]
fn update_state_checkout_returns_failure_without_reusing_previous_success() {
    use hangar_server::update::git_engine::{Service, releases::{Source, DownloadError}};
    struct Unused;
    impl Source for Unused {
        fn get(&self, _: &str, _: std::time::Duration) -> Result<Vec<u8>, DownloadError> { panic!("não pode baixar antes de gravar o estado"); }
    }
    let temp = tempfile::tempdir().unwrap();
    let service = Service::new(temp.path().into(), temp.path().join("repo"), temp.path().join(".claude"), Default::default());
    std::fs::create_dir_all(&service.state_dir).unwrap();
    let original = br#"{"ok":true,"extra":"\ud800"}"#;
    std::fs::write(service.state().path(), original).unwrap();
    for result in [service.execute_restart(1, "fixture", &Unused), service.execute(1, &Unused, None)] {
        assert_eq!(result["ok"], false);
        assert_eq!(result["fase"], "pronto");
        assert_eq!(result["erro"], "update_state_invalid_unicode: texto contém surrogate Unicode isolado");
        assert_eq!(std::fs::read(service.state().path()).unwrap(), original);
    }
}
