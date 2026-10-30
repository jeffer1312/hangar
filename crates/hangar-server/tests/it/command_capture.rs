#[cfg(unix)]
mod posix {
    use hangar_server::{
        accounts::environment::AccountEnvironment,
        command_capture::{CaptureOptions, capture_bytes},
    };
    use serde_json::{Value, json};
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
    fn hex(bytes: &[u8], root: &Path) -> String {
        let root = root.to_string_lossy();
        let mut normalized = bytes.to_vec();
        if bytes.starts_with(root.as_bytes()) {
            normalized = b"$ROOT"
                .iter()
                .chain(&bytes[root.len()..])
                .copied()
                .collect();
        }
        normalized
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    #[tokio::test]
    async fn command_capture_bytes_match_original_channels_cwd_and_timeout() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/command-capture/bytes.json"
        ))
        .unwrap();
        let root = crate::physical_tempdir();
        let cwd = root.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let mut failures = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let actual = match capture_bytes(
                vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    case["script"].as_str().unwrap().into(),
                ],
                environment(root.path()),
                Duration::from_millis(case["budget_ms"].as_u64().unwrap()),
                options(&cwd),
            )
            .await
            {
                Ok(output) => {
                    json!({"success":output.success,"stdout_hex":hex(&output.stdout,root.path()),"stderr_hex":hex(&output.stderr,root.path())})
                }
                Err(error) => {
                    json!({"error":error.kind,"stdout_hex":hex(&error.stdout,root.path()),"stderr_hex":hex(&error.stderr,root.path())})
                }
            };
            if actual != case["expected"] {
                failures.push(format!(
                    "{}: {actual} != {}",
                    case["name"], case["expected"]
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
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

#[cfg(unix)]
mod relative {
    use hangar_server::{
        accounts::environment::AccountEnvironment,
        cli_probe::resolve_program,
        command_capture::{CaptureOptions, capture_bytes},
    };
    use serde_json::{Value, json};
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    #[test]
    fn command_capture_relative_path_matches_python() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/command-capture/relative.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        let interpreter = std::process::Command::new("python3")
            // O launcher do framework macOS reexecuta Python.app e troca argv[0].
            // A prova precisa do interpretador real para observar o argv entregue pelo Hangar.
            .args(["-c", "import pathlib,sys; app=pathlib.Path(sys.base_prefix)/'Resources/Python.app/Contents/MacOS/Python'; print(app if sys.platform == 'darwin' and app.is_file() else sys.executable)"])
            .output().unwrap();
        assert!(interpreter.status.success());
        let interpreter = std::path::PathBuf::from(String::from_utf8(interpreter.stdout).unwrap().trim()).canonicalize().unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let temp = crate::physical_tempdir();
            let root = temp.path();
            std::fs::create_dir(root.join("bin")).unwrap();
            std::fs::create_dir_all(root.join("plugin/bin")).unwrap();
            for (path, text) in [
                ("bin/node", "outer"),
                ("node", "outer"),
                ("plugin/bin/node", "inner"),
                ("plugin/node", "inner"),
            ] {
                let path = root.join(path);
                if case["mode"] == "binary-name" {
                    std::os::unix::fs::symlink(&interpreter, path).unwrap();
                } else {
                    let body = if case["mode"] == "script-name" {
                        "printf %s \"$0\"".to_owned()
                    } else {
                        format!("printf {text}")
                    };
                    std::fs::write(
                        &path,
                        format!(
                            "{}{body}\n",
                            if case["shebang"] == true {
                                "#!/bin/sh\n"
                            } else {
                                ""
                            }
                        ),
                    )
                    .unwrap();
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
                }
            }
            if let Some(layout) = case.get("layout").and_then(Value::as_object) {
                std::fs::remove_file(root.join("plugin/node")).unwrap();
                std::fs::remove_file(root.join("plugin/bin/node")).unwrap();
                for (rel, kind) in layout {
                    let target = root.join("plugin").join(rel);
                    if kind == "directory" {
                        std::fs::create_dir(target).unwrap();
                    } else {
                        std::fs::write(
                            &target,
                            format!(
                                "{}printf allowed\n",
                                if kind == "invalid" { "" } else { "#!/bin/sh\n" }
                            ),
                        )
                        .unwrap();
                        std::fs::set_permissions(
                            target,
                            std::fs::Permissions::from_mode(if kind == "denied" {
                                0o600
                            } else {
                                0o700
                            }),
                        )
                        .unwrap();
                    }
                }
            }
            let path = if case["search"] == "absolute" {
                root.join("bin").to_string_lossy().into_owned()
            } else {
                case["search"].as_str().unwrap().to_owned()
            };
            std::fs::write(
                root.join("args.json"),
                serde_json::to_vec(&case["args"]).unwrap(),
            )
            .unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "command_capture::relative::command_capture_relative_child",
                    "--ignored",
                ])
                .current_dir(root)
                .env("HANGAR_CAPTURE_RELATIVE", &path)
                .env(
                    "HANGAR_CAPTURE_DIRECT",
                    if case["direct"] == true { "1" } else { "0" },
                )
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            let actual: Value = serde_json::from_str(
                &std::fs::read_to_string(root.join("result.json"))
                    .unwrap()
                    .replace(root.to_str().unwrap(), "$ROOT"),
            )
            .unwrap();
            let expected = json!({"program":case["program"],"result":case["result"]});
            if actual != expected {
                failures.push(format!("{case}: {actual}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    #[ignore = "executado pelo teste pai em diretório isolado"]
    fn command_capture_relative_child() {
        let path = std::env::var("HANGAR_CAPTURE_RELATIVE").unwrap();
        let root = std::env::current_dir().unwrap();
        let env = AccountEnvironment::from_pairs([
            ("HOME", root.to_string_lossy().into_owned()),
            ("PATH", path),
        ]);
        let program = if std::env::var("HANGAR_CAPTURE_DIRECT").as_deref() == Ok("1") {
            Some(std::path::PathBuf::from("node"))
        } else {
            resolve_program("node", &env)
        };
        let result = if let Some(program) = &program {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut args = vec![program.to_string_lossy().into_owned()];
            args.extend(
                serde_json::from_slice::<Vec<String>>(
                    &std::fs::read(root.join("args.json")).unwrap(),
                )
                .unwrap(),
            );
            match rt.block_on(capture_bytes(
                args,
                env,
                Duration::from_secs(2),
                CaptureOptions {
                    cwd: Some(root.join("plugin")),
                    ..Default::default()
                },
            )) {
                Ok(out) => {
                    json!({"success":out.success,"stdout":String::from_utf8(out.stdout).unwrap(),"stderr":String::from_utf8(out.stderr).unwrap()})
                }
                Err(error) => json!({"error":error.kind}),
            }
        } else {
            json!({"absent":true})
        };
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec(&json!({"program":program,"result":result})).unwrap(),
        )
        .unwrap();
    }
}
