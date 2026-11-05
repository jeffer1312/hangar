#[cfg(unix)]
mod posix {
    use hangar_server::{
        accounts::environment::AccountEnvironment,
        command_capture::{CaptureOptions, capture_bytes},
    };
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    fn environment(home: &Path) -> AccountEnvironment {
        AccountEnvironment::from_pairs([("HOME", home.to_string_lossy().into_owned())])
    }
    fn options(cwd: &Path) -> CaptureOptions {
        CaptureOptions {
            cwd: Some(cwd.to_owned()),
            preserve_account: true,
            ..Default::default()
        }
    }
    struct Sentinel(std::process::Child);
    impl Drop for Sentinel {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn alive(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| {
                s.split_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z'))
            })
        {
            return false;
        }
        true
    }
    #[tokio::test]
    async fn command_capture_timeout_and_cancel_keep_other_processes_alive() {
        for cancel in [false, true] {
            let root = crate::physical_tempdir();
            let mut outside = Sentinel(
                std::process::Command::new("/bin/sleep")
                    .arg("60")
                    .spawn()
                    .unwrap(),
            );
            let start = Instant::now();
            let cwd = root.path().to_owned();
            let env = environment(&cwd);
            let task = tokio::spawn(capture_bytes(
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "/bin/sleep 60 & echo $! > child.pid; printf out; printf err >&2; wait".into(),
                ],
                env,
                Duration::from_millis(if cancel { 60000 } else { 350 }),
                options(&cwd),
            ));
            for _ in 0..100 {
                if cwd.join("child.pid").is_file() || task.is_finished() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let pid = std::fs::read_to_string(cwd.join("child.pid"))
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok());
            if cancel {
                task.abort();
                let _ = task.await;
            } else {
                let result = task.await.unwrap();
                assert!(result.is_err());
            }
            let outside_alive = outside.0.try_wait().unwrap().is_none();
            drop(outside);
            assert!(outside_alive, "outro grupo foi alcançado");
            let pid = pid.expect("filho sintético iniciou");
            for _ in 0..100 {
                if !alive(pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let survived = alive(pid);
            if survived {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            assert!(!survived, "filho sobreviveu ao término");
            assert!(start.elapsed() < Duration::from_secs(4));
        }
    }
}
