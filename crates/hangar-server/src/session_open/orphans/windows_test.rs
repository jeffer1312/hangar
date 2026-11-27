use std::ffi::OsStr;
use std::path::PathBuf;

use super::*;
use crate::session_open::{claude, fallen, record, restore};

#[tokio::test]
#[ignore = "exige o runner Windows descartável com psmux preparado"]
async fn a_real_psmux_orphan_can_be_recovered_and_dismissed() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(std::env::var("RUNNER_ENVIRONMENT").as_deref(), Ok("github-hosted"));
    let subscriber = tracing_subscriber::fmt().with_writer(std::io::stderr).with_ansi(false).finish();
    let _log = tracing::subscriber::set_default(subscriber);
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").unwrap()).canonicalize().unwrap();
    let mux = PathBuf::from(std::env::var_os("HANGAR_TEST_PSMUX").unwrap()).canonicalize().unwrap();
    let registry = PathBuf::from(std::env::var_os("PSMUX_DATA_DIR").unwrap()).canonicalize().unwrap();
    assert!(mux.starts_with(&runner) && registry.starts_with(&runner) && registry != runner);
    assert!(std::fs::read_dir(&registry).unwrap().next().is_none());
    let home = tempfile::tempdir_in(&runner).unwrap();
    let projects = home.path().join("projects");
    let agent = home.path().join("claude.exe");
    std::fs::copy(std::env::var_os("ComSpec").unwrap(), &agent).unwrap();

    for (name, key, reopen) in [
        ("hangar-probe-reopen", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true),
        ("hangar-probe-dismiss", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", false),
    ] {
        let conversation = "8b190b09-d66f-4069-a5d0-331ce61c866c";
        let spec: SessionSpec = serde_json::from_value(serde_json::json!({
            "version":1, "key":key, "name":name, "cwd":home.path(), "provider":"claude", "surface":"terminal",
            "account":{"kind":"default"}, "account_source":"default", "permission_mode":"manual",
            "jev":false, "read_only":false, "created_at":1.0,
        })).unwrap();
        record::write(home.path(), &spec).unwrap();
        let transcript = claude::transcript(&spec, conversation, &projects);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{}\n").unwrap();
        let entry = restore::Entry { name: name.into(), provider: AgentProvider::Claude, conversation: Some(conversation.into()),
            transcript: Some(transcript.to_string_lossy().into_owned()), permission_mode: Some("acceptEdits".into()), missing_since: None, unit: None };
        let snapshot = restore::Snapshot { mux_started: None, sessions: [(key.into(), entry)].into() };
        let key_env = format!("CP_SESSION_KEY={key}");
        let launch = [
            OsStr::new("new-session"), OsStr::new("-d"), OsStr::new("-s"), OsStr::new(name),
            OsStr::new("-c"), home.path().as_os_str(), OsStr::new("-e"), OsStr::new(&key_env),
            agent.as_os_str(), OsStr::new("/d"), OsStr::new("/k"), OsStr::new("ping -t 127.0.0.1"),
        ];
        run(&mux, &launch).await;
        let port = registry.join(format!("{name}.port"));
        let displaced = port.with_extension("probe-backup");
        std::fs::rename(&port, &displaced).expect("sessão de prova gravou a ficha na pasta isolada");

        let found = find_bound(name, key).await;
        register(home.path(), &found, &snapshot, Some(home.path()));
        let fallen = fallen::get(home.path(), name).expect("a sessão de prova apareceu como caída");
        assert!(!fallen.orphan_descendants.is_empty());
        let row = fallen::rows(home.path(), &[]).into_iter().find(|row| row.name == name).unwrap();
        assert!(accepts_action(&fallen, row.lifecycle_id.as_deref()));
        assert!(!accepts_action(&fallen, None));
        if reopen {
            let resolved = restore::resolve_orphan(home.path(), &projects, key, &fallen.entry(name)).unwrap();
            assert_eq!(resolved.resume.as_deref(), Some(conversation));
            assert_eq!(resolved.spec.permission_mode, Some(crate::session_open::PermissionMode::AcceptEdits));
        }
        std::fs::rename(&displaced, &port).unwrap();
        assert_eq!(end_checked(home.path(), name, &fallen, Duration::ZERO), Err("psmux_orphan_unknown"));
        let pid = fallen.orphan_pid.unwrap();
        assert_eq!(hangar_psmux::process_birth(pid).unwrap(), fallen.orphan_birth);
        std::fs::rename(&port, &displaced).unwrap();
        end_checked(home.path(), name, &fallen, Duration::ZERO).expect("encerramento da vida de prova");
        assert!(!holds(name));
        assert_ne!(hangar_psmux::process_birth(pid).unwrap(), fallen.orphan_birth);
        for (pid, birth) in fallen.orphan_descendants {
            assert_ne!(hangar_psmux::process_birth(pid).unwrap(), Some(birth));
        }
        if reopen {
            run(&mux, &launch).await;
            run(&mux, &[OsStr::new("kill-session"), OsStr::new("-t"), OsStr::new(name)]).await;
        }
        restore::forget(home.path(), name);
        assert!(fallen::get(home.path(), name).is_none());
        eprintln!("{name}: vínculo, ficha reaparecida e encerramento conferidos");
    }
}

async fn find_bound(name: &str, key: &str) -> Vec<(Orphan, Option<Binding>)> {
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut ticks = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticks.tick().await;
            let found = tokio::task::spawn_blocking(|| find_after(Duration::ZERO)).await.unwrap().unwrap();
            if found.iter().any(|(orphan, proof)| orphan.name == name && proof.as_ref().is_some_and(|proof| proof.key == key)) {
                return found;
            }
        }
    }).await.expect("o psmux real publicou um processo ligado à sessão de prova")
}

async fn run(mux: &Path, args: &[&OsStr]) {
    let mut cmd = tokio::process::Command::new(mux);
    cmd.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.unwrap().unwrap();
    assert!(output.status.success(), "psmux: {}", String::from_utf8_lossy(&output.stderr));
}
