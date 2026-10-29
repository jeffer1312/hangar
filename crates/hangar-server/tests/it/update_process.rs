//! Executor compartilhado: comportamento Python capturado e prazo sobre o grupo criado.
#[cfg(unix)]
#[test]
fn process_matches_python_output_and_log_order() {
    use hangar_server::update::process::run;
    use serde_json::Value;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/peers/process-cases.json"
    ))
    .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("script").is_some())
    {
        let logs = std::cell::RefCell::new(Vec::new());
        let result = run(
            &[
                "sh".into(),
                "-c".into(),
                case["script"].as_str().unwrap().into(),
            ],
            cwd.path(),
            std::time::Duration::from_secs(case["timeout"].as_u64().unwrap_or(3)),
            &|line| logs.borrow_mut().push(line.to_owned()),
        );
        match result {
            Ok(result) => {
                if result.code != case["code"].as_i64().unwrap() as i32
                    || result.stdout != case["stdout"]
                    || result.stderr != case["stderr"]
                    || serde_json::to_value(logs.into_inner()).unwrap() != case["logs"]
                {
                    failures.push(format!(
                        "{}: saída {:?}, esperado {}",
                        case["name"], result.stdout, case["stdout"]
                    ));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", case["name"])),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[test]
fn timeout_kills_its_descendants_without_touching_another_process() {
    use hangar_server::update::process::{ProcessError, run};
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let cwd = tempfile::tempdir().unwrap();
    let mut outside = Command::new("sleep").arg("60").spawn().unwrap();
    let start = Instant::now();
    let result = run(
        &[
            "sh".into(),
            "-c".into(),
            "sleep 60 & echo $! > child.pid; wait".into(),
        ],
        cwd.path(),
        Duration::from_millis(150),
        &|_| {},
    );
    let outside_alive = outside.try_wait().unwrap().is_none();
    outside.kill().unwrap();
    outside.wait().unwrap();
    assert!(outside_alive, "outro grupo não pode ser alcançado");
    assert!(matches!(result, Err(ProcessError::Timeout { .. })));
    assert!(start.elapsed() < Duration::from_secs(3));
    let pid: i32 = std::fs::read_to_string(cwd.path().join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    for _ in 0..100 {
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        #[cfg(target_os = "linux")]
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| s.split_once(") ").is_some_and(|(_, r)| r.starts_with('Z')));
        #[cfg(not(target_os = "linux"))]
        let zombie = false;
        if !exists || zombie {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    panic!("o filho {pid} sobreviveu ao prazo");
}

#[test]
#[ignore = "auxiliar executado somente pelo teste de contenção"]
fn persistent_child_helper() {
    std::thread::sleep(std::time::Duration::from_millis(800));
    std::fs::write("persistent-finished", "ok").unwrap();
}

#[test]
#[ignore = "auxiliar executado somente pelo teste de contenção"]
fn spawn_persistent_child_helper() {
    use std::process::{Command, Stdio};
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "update_process::persistent_child_helper",
            "--ignored",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write("persistent.pid", child.id().to_string()).unwrap();
    if std::env::var_os("HANGAR_PROCESS_TIMEOUT_HELPER").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[test]
fn successful_command_preserves_its_detached_helper() {
    let cwd = tempfile::tempdir().unwrap();
    let result = hangar_server::update::process::run(
        &[
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            "update_process::spawn_persistent_child_helper".into(),
            "--ignored".into(),
        ],
        cwd.path(),
        std::time::Duration::from_secs(3),
        &|_| {},
    )
    .unwrap();
    assert_eq!(result.code, 0);
    for _ in 0..150 {
        if cwd.path().join("persistent-finished").is_file() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("o sucesso do comando encerrou o auxiliar que deveria continuar");
}

#[test]
fn timed_out_command_stops_its_auxiliary() {
    use hangar_server::update::process::{ProcessError, run_with_env};
    let cwd = tempfile::tempdir().unwrap();
    let result = run_with_env(
        &[
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            "update_process::spawn_persistent_child_helper".into(),
            "--ignored".into(),
        ],
        cwd.path(),
        std::time::Duration::from_millis(300),
        &[("HANGAR_PROCESS_TIMEOUT_HELPER".into(), "1".into())].into(),
        &|_| {},
    );
    assert!(matches!(result, Err(ProcessError::Timeout { .. })));
    assert!(
        cwd.path().join("persistent.pid").is_file(),
        "o filho nasceu antes do prazo"
    );
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert!(
        !cwd.path().join("persistent-finished").exists(),
        "o filho executou depois do timeout"
    );
}
