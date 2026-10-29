//! Executor compartilhado: comportamento Python capturado e prazo sobre o grupo criado.
#[cfg(unix)]
#[test]
fn process_matches_python_output_and_log_order() {
    use hangar_server::update::process::run;
    use serde_json::Value;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/process-cases.json")).unwrap();
    let cwd=tempfile::tempdir().unwrap();
    for case in fixture["cases"].as_array().unwrap().iter().filter(|c|c.get("script").is_some()) {
        let logs=std::cell::RefCell::new(Vec::new());
        let result=run(&["sh".into(),"-c".into(),case["script"].as_str().unwrap().into()],cwd.path(),std::time::Duration::from_secs(3),&|line|logs.borrow_mut().push(line.to_owned())).unwrap();
        assert_eq!(result.code,case["code"].as_i64().unwrap() as i32);
        assert_eq!(result.stdout,case["stdout"]);assert_eq!(result.stderr,case["stderr"]);
        assert_eq!(serde_json::to_value(logs.into_inner()).unwrap(),case["logs"]);
    }
}

#[cfg(unix)]
#[test]
fn timeout_kills_its_descendants_without_touching_another_process() {
    use hangar_server::update::process::{run,ProcessError};
    use std::{time::{Instant,Duration},process::Command};
    let cwd=tempfile::tempdir().unwrap();
    let mut outside=Command::new("sleep").arg("60").spawn().unwrap();
    let start=Instant::now();
    let result=run(&["sh".into(),"-c".into(),"sleep 60 & echo $! > child.pid; wait".into()],cwd.path(),Duration::from_millis(150),&|_|{});
    let outside_alive=outside.try_wait().unwrap().is_none();outside.kill().unwrap();outside.wait().unwrap();
    assert!(outside_alive,"outro grupo não pode ser alcançado");assert!(matches!(result,Err(ProcessError::Timeout{..})));assert!(start.elapsed()<Duration::from_secs(3));
    let pid:i32=std::fs::read_to_string(cwd.path().join("child.pid")).unwrap().trim().parse().unwrap();
    for _ in 0..100 {let exists=unsafe{libc::kill(pid,0)}==0;
        #[cfg(target_os="linux")] let zombie=std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().is_some_and(|s|s.split_once(") ").is_some_and(|(_,r)|r.starts_with('Z')));
        #[cfg(not(target_os="linux"))] let zombie=false;
        if !exists||zombie{return}std::thread::sleep(Duration::from_millis(10));
    }
    unsafe{libc::kill(pid,libc::SIGKILL);}panic!("o filho {pid} sobreviveu ao prazo");
}
