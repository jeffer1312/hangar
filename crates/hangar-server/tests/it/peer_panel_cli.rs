#![cfg(unix)]
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
fn fixture(name: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/tests/fixtures/contract/update-git");
    serde_json::from_slice(&fs::read(root.join(format!("panel{name}.json"))).unwrap()).unwrap()
}
fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
fn setup(port: Option<u16>) -> tempfile::TempDir {
    let temp = crate::physical_tempdir();
    let config = temp.path().join(".hangar/config");
    fs::create_dir_all(&config).unwrap();
    if let Some(port) = port {
        fs::write(
            config.join(".env"),
            format!("CP_PORT={port}\nCP_AUTH_TOKEN=fixture-token\n"),
        )
        .unwrap();
    }
    temp
}
fn command(root: &Path, args: &[&str]) -> Command {
    let binary = std::env::var_os("HANGAR_TEST_SERVER_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hangar-server").into());
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("PATH", root);
    cmd
}
fn run(mut command: Command) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = vec![];
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = vec![];
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI do painel excedeu o prazo")
        };
        std::thread::sleep(Duration::from_millis(5));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}
fn exact(output: &Output, rc: i64, stdout: &[u8]) {
    assert_eq!(
        output.status.code(),
        Some(rc as i32),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, stdout);
}
fn program(root: &Path, name: &str, stdout: &str, code: i64) {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\0' {} \"$@\" '' >> {}\nprintf '%s' {}\nexit {code}\n",
        quote(name),
        quote(&root.join("calls").display().to_string()),
        quote(stdout)
    );
    executable(&root.join(name), &script);
}
fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn calls(root: &Path, count: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let bytes = fs::read(root.join("calls")).unwrap_or_default();
        let mut rows = vec![];
        let mut row = vec![];
        for field in bytes.split(|byte| *byte == 0) {
            if field.is_empty() {
                if !row.is_empty() {
                    rows.push(Value::Array(std::mem::take(&mut row)));
                }
            } else {
                row.push(json!(String::from_utf8(field.to_vec()).unwrap()));
            }
        }
        if rows.len() >= count || Instant::now() >= deadline {
            return json!(rows);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Http {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Http {
    fn new(handler: impl Fn(&str) -> (u16, Vec<u8>) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = vec![];
                let mut buf = [0; 4096];
                let end = loop {
                    let count = stream.read(&mut buf).unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buf[..count]);
                    if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let path = head
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                let (status, body) = handler(path);
                let header = format!(
                    "HTTP/1.1 {status} Synthetic reason\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }
    fn queue(responses: Vec<(u16, String)>) -> Self {
        let queue = Mutex::new(VecDeque::from(responses));
        Self::new(move |_| {
            let (status, text) = queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("chamada HTTP não prevista");
            (status, text.into_bytes())
        })
    }
}
impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn panel_cli_dispatch_and_transport_errors() {
    let temp = setup(None);
    for row in fixture("")["cases"].as_array().unwrap() {
        if row["kind"] != "dispatch" {
            continue;
        }
        let args: Vec<_> = std::iter::once("panel-action")
            .chain(
                row["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .collect();
        let output = run(command(temp.path(), &args));
        assert_eq!(
            output.status.code(),
            Some(row["rc"].as_i64().unwrap() as i32)
        );
        let value = if output.stdout.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&output.stdout).unwrap()
        };
        assert_eq!(value, row["result"]);
    }
    for (status, body, expected) in [
        (200, vec![255], None),
        (503, b"[]".to_vec(), Some("503: Synthetic reason")),
        (503, b"null".to_vec(), Some("503: Synthetic reason")),
        (503, b"\"text\"".to_vec(), Some("503: Synthetic reason")),
        (503, b"1".to_vec(), Some("503: Synthetic reason")),
        (
            503,
            b"{\"detail\":null}".to_vec(),
            Some("503: Synthetic reason"),
        ),
        (
            503,
            b"{\"detail\":{\"reason\":true}}".to_vec(),
            Some("503: {'reason': True}"),
        ),
    ] {
        let server = Http::new(move |_| (status, body.clone()));
        let temp = setup(Some(server.port));
        let output = run(command(
            temp.path(),
            &["panel-action", "fixture", "rename", "renamed"],
        ));
        assert_eq!(output.status.code(), Some(1));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        if let Some(expected) = expected {
            assert_eq!(value["message"], expected)
        } else {
            assert!(value["message"].as_str().unwrap().starts_with(&format!(
                "resposta não-JSON de http://127.0.0.1:{}: ",
                server.port
            )));
        }
    }
}
#[test]
fn panel_cli_projects_and_process_errors() {
    for row in fixture("-projects")["cases"].as_array().unwrap() {
        if row["failure"].as_str().is_some_and(|s| !s.is_empty()) {
            continue;
        }
        let projects = row
            .get("projects")
            .or_else(|| row.get("remote"))
            .cloned()
            .unwrap_or(json!([]));
        let server = Http::new(move |_| (200, serde_json::to_vec(&projects).unwrap()));
        let temp = setup(Some(server.port));
        let root = temp.path();
        let config = root.join(".hangar/config");
        fs::write(
            config.join(".env"),
            format!(
                "CP_PORT={}\nCP_AUTH_TOKEN=fixture-token\nCP_SCAN_ROOTS={}/first,{}/second\n",
                server.port,
                root.display(),
                root.display()
            ),
        )
        .unwrap();
        fs::write(config.join("peers.json"), "{}").unwrap();
        for p in ["first/unique", "first/collision", "second/collision"] {
            fs::create_dir_all(root.join(p)).unwrap();
        }
        let (args, expected) = match row["kind"].as_str().unwrap() {
            "open" => {
                if let Some(term) = row["term"].as_str().filter(|s| !s.is_empty()) {
                    program(root, term, "", 0)
                }
                program(root, "tmux", "", row["code"].as_i64().unwrap());
                (vec!["project", "project", "open"], &row["result"]["value"])
            }
            "pick" => {
                if let Some(term) = row["term"].as_str().filter(|s| !s.is_empty()) {
                    program(
                        root,
                        term,
                        row["stdout"].as_str().unwrap(),
                        row["code"].as_i64().unwrap(),
                    )
                }
                (vec!["fixture", "pick-folder", "/start"], &row["result"])
            }
            _ => (vec!["remote", "import-candidates"], &row["result"]),
        };
        let args: Vec<_> = std::iter::once("panel-action").chain(args).collect();
        let output = run(command(root, &args));
        let text = String::from_utf8(output.stdout)
            .unwrap()
            .replace(root.to_str().unwrap(), "<ROOT>");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            *expected,
            "{}",
            row["name"]
        );
        assert_eq!(
            output.status.code(),
            Some(if expected["ok"] == true { 0 } else { 1 })
        );
        let expected_calls: Vec<_> = row
            .get("calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|v| v["kind"] != "api")
            .map(|v| v["args"].clone())
            .collect();
        assert_eq!(calls(root, expected_calls.len()), json!(expected_calls));
    }
    for row in fixture("-process-errors")["cases"].as_array().unwrap() {
        let server = Http::new(|_| {
            (
                200,
                b"[{\"name\":\"fixture\",\"tmux\":\"runner\"}]".to_vec(),
            )
        });
        let temp = setup(Some(server.port));
        let root = temp.path();
        let name = row["name"].as_str().unwrap();
        executable(
            &root.join("tmux"),
            if name == "run-spawn" {
                "invalid executable"
            } else if name == "run-timeout" {
                "#!/bin/sh\nexec /bin/sleep 6\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            },
        );
        executable(
            &root.join("kitty"),
            if name == "terminal-spawn" {
                "invalid executable"
            } else {
                "#!/bin/sh\nexit 0\n"
            },
        );
        exact(
            &run(command(
                root,
                &["panel-action", "fixture", "project", "open"],
            )),
            row["rc"].as_i64().unwrap(),
            row["stdout"].as_str().unwrap().as_bytes(),
        );
    }
}

#[test]
fn panel_cli_deep_responses() {
    for row in fixture("-depth")["cases"].as_array().unwrap() {
        let depth = row["depth"].as_u64().unwrap() as usize;
        let nested = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let (args, replies) = match row["name"].as_str().unwrap() {
            "pane" => (
                vec!["fixture", "project", "pane"],
                vec![(200, format!("{{\"pane\":{nested}}}"))],
            ),
            "kill" => (
                vec!["fixture", "kill"],
                vec![(200, format!("{{\"ignored\":{nested}}}"))],
            ),
            "start" => (
                vec!["fixture", "project", "start"],
                vec![(200, format!("{{\"state\":\"working\",\"extra\":{nested}}}"))],
            ),
            "info" | "info-error" => (
                vec!["fixture", "info"],
                vec![
                    (
                        200,
                        format!("[{{\"name\":\"fixture\",\"branch\":{nested}}}]"),
                    ),
                    if row["name"] == "info" {
                        (200, "[]".into())
                    } else {
                        (400, "{\"detail\":\"not a git repository\"}".into())
                    },
                ],
            ),
            "commit" => (
                vec!["fixture", "commit"],
                vec![
                    (200, format!("[{{\"path\":{nested}}}]")),
                    (200, "{}".into()),
                ],
            ),
            "import" => (
                vec!["fixture", "import-candidates"],
                vec![(
                    200,
                    format!(
                        "[{{\"name\":{nested},\"cwd\":\"/remote/missing\",\"command\":\"\",\"port\":{nested}}}]"
                    ),
                )],
            ),
            _ => panic!("caso desconhecido: {row}"),
        };
        let server = Http::queue(replies);
        let temp = setup(Some(server.port));
        let args: Vec<_> = std::iter::once("panel-action").chain(args).collect();
        let expected = &row["results"]["default"];
        exact(
            &run(command(temp.path(), &args)),
            expected["rc"].as_i64().unwrap(),
            expected["stdout"].as_str().unwrap().as_bytes(),
        );
    }
    for row in fixture("-snapshot-depth")["cases"].as_array().unwrap() {
        let sessions = row["sessions_json"].as_str().unwrap().to_owned();
        let projects = row["projects_json"].as_str().unwrap().to_owned();
        let server = Http::new(move |path| {
            (
                200,
                if path.starts_with("/api/sessions") {
                    sessions.as_bytes().to_vec()
                } else {
                    projects.as_bytes().to_vec()
                },
            )
        });
        let temp = setup(Some(server.port));
        fs::write(
            temp.path().join(".hangar/config/peers.json"),
            row["peers_json"].as_str().unwrap(),
        )
        .unwrap();
        let original = &row["results"]["original"];
        let expected = if original.get("exception").is_some() {
            &row["results"]["encoder-reference"]
        } else {
            original
        };
        exact(
            &run(command(temp.path(), &["panel-data"])),
            expected["rc"].as_i64().unwrap(),
            expected["stdout"].as_str().unwrap().as_bytes(),
        );
    }
}

#[test]
fn panel_cli_paths_and_stdout() {
    for row in fixture("-paths")["cases"].as_array().unwrap() {
        let cwd = row["cwd"].clone();
        let temp = setup(None);
        let root = temp.path();
        fs::create_dir_all(root.join("real/nested")).unwrap();
        for (link, target) in [
            ("link", "real/nested"),
            ("dangling", "absent/nested"),
            ("cycle-a", "cycle-b"),
            ("cycle-b", "cycle-a"),
        ] {
            std::os::unix::fs::symlink(target, root.join(link)).unwrap();
        }
        let cwd = cwd
            .as_str()
            .unwrap()
            .replace("<ROOT>", root.to_str().unwrap());
        let server = Http::new(move |path| {
            (
                200,
                if path.starts_with("/api/sessions") {
                    serde_json::to_vec(&json!([{"name":"fixture","state":"idle","cwd":cwd}]))
                        .unwrap()
                } else {
                    b"[]".to_vec()
                },
            )
        });
        fs::write(
            root.join(".hangar/config/.env"),
            format!("CP_PORT={}\nCP_AUTH_TOKEN=fixturetoken\n", server.port),
        )
        .unwrap();
        fs::write(root.join(".hangar/config/peers.json"), "{}").unwrap();
        let output = run(command(root, &["panel-data"]));
        assert_eq!(output.status.code(), Some(0));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["sessions"][0]["cwd_real"]
                .as_str()
                .unwrap()
                .replace(root.to_str().unwrap(), "<ROOT>"),
            row["result"].as_str().unwrap()
        );
    }
    for row in fixture("-stdout")["cases"].as_array().unwrap() {
        let value = row["value_json"].as_str().unwrap().to_owned();
        let server = Http::new(move |path| {
            (
                200,
                if path == "/api/sessions" {
                    b"[]".to_vec()
                } else if path == "/api/projects" {
                    format!("[{{\"name\":\"fixture\",\"extra\":{value}}}]").into_bytes()
                } else {
                    format!("{{\"pane\":{value}}}").into_bytes()
                },
            )
        });
        let temp = setup(Some(server.port));
        fs::write(temp.path().join(".hangar/config/peers.json"), "{}").unwrap();
        let output = run(command(
            temp.path(),
            if row["command"] == "action" {
                &["panel-action", "fixture", "project", "pane"]
            } else {
                &["panel-data"]
            },
        ));
        let expected = &row["result"];
        let invalid = expected.get("exception").is_some();
        exact(
            &output,
            expected["rc"].as_i64().unwrap(),
            &if invalid {
                vec![]
            } else {
                unhex(expected["stdout_hex"].as_str().unwrap())
            },
        );
        if invalid {
            assert_eq!(
                output.stderr,
                "erro: saída do painel contém Unicode inválido\n".as_bytes()
            );
        }
    }
}

#[test]
fn panel_cli_peer_writes_preserve_neighbors_and_permissions() {
    for row in fixture("-write")["cases"].as_array().unwrap() {
        let temp = setup(None);
        let root = temp.path();
        let path = root.join(".hangar/config/peers.json");
        fs::write(&path, row["input"].as_str().unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(path.with_file_name("peers.json.stale.tmp"), "stale").unwrap();
        let output = run(command(root, &["panel-action", "peer", "peer", "on"]));
        let expected = &row["result"];
        exact(
            &output,
            expected["rc"].as_i64().unwrap(),
            &unhex(expected["stdout_hex"].as_str().unwrap()),
        );
        let bytes = fs::read(&path).unwrap();
        if let Some(hex) = expected["file_hex"].as_str() {
            assert_eq!(bytes, unhex(hex));
        }
        if let Some(hash) = expected["file_sha256"].as_str() {
            assert_eq!(bytes.len() as u64, expected["file_bytes"].as_u64().unwrap());
            assert_eq!(hangar_server::codex_setup::files::hash(&bytes), hash);
        }
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            expected["mode"].as_u64().unwrap() as u32
        );
        assert!(
            !fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
                .unwrap()
                .path()
                .extension()
                .is_some_and(|s| s == "tmp"))
        );
        if row["name"] == "surrogate" {
            assert_eq!(
                output.stderr,
                "erro: livro de peers contém Unicode inválido\n".as_bytes()
            );
        }
    }
    let temp = setup(None);
    let root = temp.path();
    let path = root.join(".hangar/config/peers.json");
    let initial = json!({"pc":{"base_url":"http://pc","token":"fixture-pc","web_url":"https://pc"},"srv":{"base_url":"http://srv","token":"fixture-srv"}});
    fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let commands = [
        command(root, &["panel-action", "pc", "peer", "off"]),
        command(root, &["panel-action", "srv", "peer", "off"]),
    ];
    let threads: Vec<_> = commands
        .into_iter()
        .map(|cmd| std::thread::spawn(move || run(cmd)))
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap().status.success());
    }
    let mut expected = initial;
    expected["pc"]["enabled"] = json!(false);
    expected["srv"]["enabled"] = json!(false);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        expected
    );
    assert!(
        run(command(root, &["panel-action", "pc", "peer", "on"]))
            .status
            .success()
    );
    expected["pc"]["enabled"] = json!(true);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|s| s == "tmp"))
    );
}

#[test]
fn panel_cli_literal_environment_and_locale() {
    for row in fixture("-env")["cases"].as_array().unwrap() {
        let temp = setup(None);
        let root = temp.path();
        for dir in ["chosen", "other"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        if let Some(input) = row["input_hex"].as_str() {
            let bytes = unhex(input);
            let marker = b"<ROOT>";
            let mut replaced = Vec::new();
            let mut cursor = 0;
            while cursor < bytes.len() {
                if bytes[cursor..].starts_with(marker) {
                    replaced.extend_from_slice(root.to_str().unwrap().as_bytes());
                    cursor += marker.len();
                } else {
                    replaced.push(bytes[cursor]);
                    cursor += 1;
                }
            }
            fs::write(root.join(".hangar/config/.env"), replaced).unwrap();
        }
        fs::write(root.join(".hangar/config/peers.json"), "{\"peer\":{}}").unwrap();
        executable(&root.join("kdialog"), "#!/bin/sh\nprintf '%s\\n' \"$2\"\n");
        let args: Vec<_> = std::iter::once("panel-action")
            .chain(
                row["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .collect();
        let output = run(command(root, &args));
        let expected = &row["result"];
        assert_eq!(
            output.status.code(),
            Some(expected["rc"].as_i64().unwrap() as i32)
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .replace(root.to_str().unwrap(), "<ROOT>"),
            expected["stdout"].as_str().unwrap()
        );
        assert_eq!(
            fs::read_to_string(root.join(".hangar/config/peers.json")).unwrap(),
            expected["book"].as_str().unwrap()
        );
        if expected.get("exception").is_some() {
            assert_eq!(
                output.stderr,
                "erro: configuração do painel contém Unicode inválido\n".as_bytes()
            );
        }
    }
    let (username, userhome) = current_user();
    for row in fixture("-user-environment")["cases"].as_array().unwrap() {
        let label = row["name"].as_str().unwrap();
        let (name, dialog) = label.rsplit_once('-').unwrap();
        let temp = setup(None);
        let root = temp.path();
        fs::create_dir(root.join("chosen")).unwrap();
        let scan = match name {
            "named-user" => format!("~{username}"),
            "named-user-parent" => format!("~{username}/.."),
            "unknown-user" => "~hangar-fixture-user-does-not-exist/path".into(),
            "home-child" => "~/chosen".into(),
            "home-double-slash" => "~//chosen".into(),
            "named-double-slash" => format!("~{username}//."),
            _ => "~".into(),
        };
        fs::write(
            root.join(".hangar/config/.env"),
            format!("CP_SCAN_ROOTS={scan}\n"),
        )
        .unwrap();
        executable(
            &root.join(dialog),
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$TRACE.args\"\nprintf '%s\\0' \"${LANG-__ABSENT__}\" \"${LC_ALL-__ABSENT__}\" \"${LANGUAGE-__ABSENT__}\" > \"$TRACE.locale\"\nprintf '/chosen\\n'\n",
        );
        let mut cmd = command(root, &["panel-action", "fixture", "pick-folder"]);
        cmd.env("TRACE", root.join("trace"));
        if name == "lang-only" || name == "locale-explicit" {
            cmd.env("LANG", "pt_BR.UTF-8");
        }
        if name == "locale-explicit" {
            cmd.env("LC_ALL", "C.UTF-8").env("LANGUAGE", "pt_BR:pt");
        }
        let output = run(cmd);
        let normalize = |text: &str| {
            text.replace(root.to_str().unwrap(), "<ROOT>")
                .replace(&userhome, "<USER_HOME>")
        };
        let argv = nul_fields(&fs::read(root.join("trace.args")).unwrap())
            .into_iter()
            .map(|v| normalize(&v))
            .collect::<Vec<_>>();
        let fields = nul_fields(&fs::read(root.join("trace.locale")).unwrap());
        let mut locale = serde_json::Map::new();
        for (key, value) in ["LANG", "LC_ALL", "LANGUAGE"].into_iter().zip(fields) {
            locale.insert(
                key.into(),
                if value == "__ABSENT__" {
                    Value::Null
                } else {
                    json!(normalize(&value))
                },
            );
        }
        let observed = json!({"rc":output.status.code(),"stdout":normalize(std::str::from_utf8(&output.stdout).unwrap()),"stderr":normalize(std::str::from_utf8(&output.stderr).unwrap()),"trace":{"argv":argv,"locale":locale}});
        assert_eq!(observed, row["result"], "{label}");
    }
}
fn nul_fields(bytes: &[u8]) -> Vec<String> {
    bytes
        .strip_suffix(&[0])
        .unwrap_or(bytes)
        .split(|b| *b == 0)
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect()
}
fn current_user() -> (String, String) {
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0u8; 65536];
    let mut result = std::ptr::null_mut();
    // O buffer pertence à chamada, sem compartilhar o registro estático de getpwuid.
    unsafe {
        assert_eq!(
            libc::getpwuid_r(
                libc::getuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result
            ),
            0
        );
        assert!(!result.is_null());
        let record = record.assume_init();
        (
            std::ffi::CStr::from_ptr(record.pw_name)
                .to_str()
                .unwrap()
                .to_owned(),
            std::ffi::CStr::from_ptr(record.pw_dir)
                .to_str()
                .unwrap()
                .to_owned(),
        )
    }
}
