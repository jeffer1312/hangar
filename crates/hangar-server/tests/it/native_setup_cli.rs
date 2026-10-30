//! Contratos dos comandos e bootstraps, em processos isolados e sem serviços reais.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn binary() -> std::ffi::OsString {
    std::env::var_os("HANGAR_TEST_SERVER_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hangar-server").into())
}
fn fixture(name: &str) -> Value {
    serde_json::from_slice(
        &fs::read(repo().join("backend/tests/fixtures/contract").join(name)).unwrap(),
    )
    .unwrap()
}
fn cli(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(binary());
    command
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command
}
fn timed_output(command: Command, seconds: u64) -> Output {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut command = tokio::process::Command::from(command);
            command
                .process_group(0)
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            let pid = child.id().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let mut out = Vec::new();
            let mut err = Vec::new();
            let result = tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
                tokio::try_join!(
                    child.wait(),
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err)
                )
            })
            .await;
            match result {
                Ok(Ok((status, _, _))) => Output {
                    status,
                    stdout: out,
                    stderr: err,
                },
                failure => {
                    // O grupo pertence só à prova; fechar os pipes não depende da saída dos descendentes.
                    unsafe {
                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    }
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    panic!("CLI não terminou corretamente no prazo de {seconds}s: {failure:?}");
                }
            }
        })
}
fn assert_output(output: Output, code: i32, stdout: &[u8], label: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        stdout,
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn doctor_and_pairing_cli_preserve_captured_output_and_explicit_backend() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let backend = home.join("installation/backend");
    fs::create_dir_all(&backend).unwrap();
    fs::write(
        backend.join(".env"),
        "CP_PORT=9876\nCP_AUTH_TOKEN=fixture-token\nCP_LAN_BIND_IP=192.0.2.10\n",
    )
    .unwrap();
    assert_output(
        timed_output(
            cli(
                home,
                &["doctor", "--backend", backend.to_str().unwrap(), "--qr"],
            ),
            10,
        ),
        2,
        b"  Abra no celular: http://192.0.2.10:9876/?token=fixture-token\n\n",
        "backend explícito",
    );
    fs::write(backend.join(".env"), "").unwrap();
    for case in fixture("peers/pairing-print.json")["cases"]
        .as_array()
        .unwrap()
    {
        if case["tty"] == true {
            continue;
        }
        for (args, code, text) in [
            (
                vec!["pairing", "--backend", backend.to_str().unwrap()],
                "rc",
                "stdout",
            ),
            (
                vec!["doctor", "--backend", backend.to_str().unwrap(), "--qr"],
                "doctor_rc",
                "doctor_stdout",
            ),
            (vec!["doctor", "--qr"], "doctor_rc", "doctor_stdout"),
        ] {
            let mut command = cli(home, &args);
            for (key, value) in case["settings"].as_object().unwrap() {
                let mut value = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                if key == "lan_bind_ip"
                    && matches!(
                        value.as_str(),
                        "127.0.0.1" | "localhost" | "::1" | "0.0.0.0" | "auto"
                    )
                {
                    value = "192.0.2.10".into();
                }
                command.env(format!("CP_{}", key.to_uppercase()), value);
            }
            assert_output(
                timed_output(command, 10),
                case[code].as_i64().unwrap() as i32,
                case[text].as_str().unwrap().as_bytes(),
                case["name"].as_str().unwrap(),
            );
        }
    }
}
#[test]
fn pairing_invalid_configuration_never_prints_sensitive_values() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let backend = home.join("backend");
    fs::create_dir(&backend).unwrap();
    for args in [vec!["pairing"], vec!["doctor", "--qr"], vec!["doctor"]] {
        for case in ["port", "front", "file", "environment"] {
            fs::write(
                backend.join(".env"),
                if case == "file" {
                    b"CP_PUBLIC_URL=fixture-sensitive\xff\n".as_slice()
                } else {
                    b""
                },
            )
            .unwrap();
            let mut command = cli(home, &args);
            command
                .args(["--backend", backend.to_str().unwrap()])
                .env("PATH", home)
                .env("CP_AUTH_TOKEN", "fixture-sensitive")
                .env("CP_LAN_BIND_IP", "192.0.2.10");
            let expected = match case {
                "port" => {
                    command.env("CP_PORT", "bad");
                    Some("pairing_config_invalid: CP_PORT\n")
                }
                "front" => {
                    command.env("CP_FRONT_PORT", "bad");
                    Some("pairing_config_invalid: CP_FRONT_PORT\n")
                }
                "environment" => {
                    command.env(
                        "CP_AUTH_TOKEN",
                        std::ffi::OsString::from_vec(b"fixture-sensitive\xff".to_vec()),
                    );
                    Some("pairing_config_invalid: CP_AUTH_TOKEN\n")
                }
                _ => None,
            };
            let output = timed_output(command, 10);
            assert_eq!(output.status.code(), Some(1), "{args:?}/{case}");
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("fixture-sensitive"));
            if let Some(expected) = expected {
                assert_eq!(stderr, expected);
            } else {
                assert!(
                    stderr.starts_with("pairing_config_unavailable: "),
                    "{stderr}"
                );
            }
        }
    }
}
#[test]
#[cfg(target_os = "linux")]
fn invalid_doctor_port_is_rejected_before_any_connection() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let backend = home.join("backend");
    fs::create_dir(&backend).unwrap();
    let source = home.join("deny.c");
    fs::write(&source,r#"#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
int connect(int fd,const struct sockaddr *a,socklen_t n){const char *p=getenv("CONNECT_LOG");FILE *f=fopen(p,"a");if(f){fputs("blocked\n",f);fclose(f);}errno=ECONNREFUSED;return -1;}
"#).unwrap();
    let library = home.join("deny.so");
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(source)
            .arg("-o")
            .arg(&library)
            .status()
            .unwrap()
            .success()
    );
    for port in ["-1", "65536", "1000000000000000000000000000000"] {
        let log = home.join("connections");
        let _ = fs::remove_file(&log);
        let mut command = cli(home, &["doctor", "--backend", backend.to_str().unwrap()]);
        command
            .env("PATH", home)
            .env("CP_PORT", port)
            .env("CP_AUTH_TOKEN", "fixture-token")
            .env("CP_LAN_BIND_IP", "127.0.0.1")
            .env("LD_PRELOAD", &library)
            .env("CONNECT_LOG", &log);
        let output = timed_output(command, 15);
        assert_eq!(
            output.stderr,
            b"doctor_config_invalid: CP_PORT fora de 0..65535\n"
        );
        assert_output(output, 1, b"", port);
        assert!(!log.exists(), "{port}: conexão antes da validação");
    }
}
fn executable(root: &Path, name: &str, body: &str) {
    let path = root.join("bin").join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn bootstrap(root: &Path, branch: bool) -> Command {
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    for name in ["install-server-binaries.sh", "server-binaries-publish.cjs"] {
        fs::copy(
            repo().join("scripts").join(name),
            root.join("scripts").join(name),
        )
        .unwrap();
    }
    executable(
        root,
        "git",
        if branch {
            "#!/bin/sh\necho beta\n"
        } else {
            "#!/bin/sh\necho main\n"
        },
    );
    executable(
        root,
        "uname",
        "#!/bin/sh\n[ \"$1\" = -s ] && echo Linux || echo x86_64\n",
    );
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut command = Command::new("bash");
    command
        .arg(root.join("scripts/install-server-binaries.sh"))
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("HANGAR_SERVER_BIN_DIR", root.join("dest"))
        .env("PROOF_ROOT", root)
        .env_remove("HANGAR_SERVER_RELEASE_URL")
        .env_remove("HANGAR_UPDATE_OWNS_RUST");
    command
}
fn manifest() -> Value {
    let mut files = serde_json::Map::new();
    for name in ["hangar-auth", "hangar-cano", "hangar-server"] {
        let bytes = format!("new-{name}");
        let hash = ring::digest::digest(&ring::digest::SHA256, bytes.as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        files.insert(
            format!("linux-x86_64/{name}"),
            json!({"name":name,"sha256":hash}),
        );
    }
    json!({"files":files})
}
fn tree(root: &Path) -> Value {
    let mut files = BTreeMap::new();
    let mut directories = Vec::new();
    let mut links = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_symlink() {
                links.push(rel.clone());
            }
            if path.is_file() {
                files.insert(rel, fs::read_to_string(&path).unwrap());
            } else if path.is_dir() {
                directories.push(rel);
                if !path.is_symlink() {
                    pending.push(path);
                }
            }
        }
    }
    directories.sort();
    links.sort();
    json!({"files":files,"directories":directories,"links":links})
}
fn lines(path: &Path) -> Value {
    json!(
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .collect::<Vec<_>>()
    )
}

const AUTH_CURL: &str = r###"#!/usr/bin/env node
const fs=require('fs'),path=require('path'),root=process.env.PROOF_ROOT,scenario=process.env.PROOF_CASE,args=process.argv.slice(2),url=args.at(-1),out=args[args.indexOf('--output')+1];fs.appendFileSync(path.join(root,'urls'),url+'\n');let data,status=200;
if(url.endsWith('server-latest.json')){let doc=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));if(url.includes('/server-beta/')){if(['branch-keeps','branch-server-download-failure','branch-server-publish-failure','branch-metadata-publish-failure'].includes(scenario))status=404;if(scenario==='branch-missing-auth')delete doc.files['linux-x86_64/hangar-auth'];}data=JSON.stringify(doc);}
else{const n=url.split('/').at(-1),kind=n.replace('hangar-','');if(scenario.replace(/^branch-/, '')===kind+'-download-failure')process.exit(1);data=scenario===kind+'-hash-failure'?'wrong':'new-'+n;}fs.writeFileSync(out,data);if(args.includes('--write-out'))process.stdout.write(String(status));
"###;

const AUTH_FAILURE: &str = r###"const fs=require('fs');const original=fs.renameSync;let failed=false;fs.renameSync=function(src,target){const name=process.env.PROOF_CASE;if(!failed&&process.argv[1]?.endsWith('server-binaries-publish.cjs')&&((name==='branch-server-publish-failure'&&target.endsWith('/hangar-server'))||(name==='branch-metadata-publish-failure'&&target.endsWith('/hangar-server.auth.json')))){failed=true;throw new Error('fixture-publication-failed');}return original(src,target);};"###;

const EXTRA_CURL: &str = r###"#!/usr/bin/env node
const fs=require('fs'),args=process.argv.slice(2);fs.appendFileSync(process.env.PROOF_LOG,args.at(-1)+'\n');fs.writeFileSync(args[args.indexOf('--output')+1],process.env.PROOF_BODY);process.stdout.write('200');
"###;

const REVIEW_CURL: &str = r###"#!/usr/bin/env node
const fs=require('fs'),path=require('path'),root=process.env.PROOF_ROOT,args=process.argv.slice(2),url=args.at(-1),output=args[args.indexOf('--output')+1];fs.appendFileSync(path.join(root,'calls'),url+'\n');let data;
if(url.endsWith('server-latest.json')) {const m=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));if(url.includes('/server-beta/'))m.files['linux-x86_64/hangar-server']=JSON.parse(fs.readFileSync(path.join(root,'case.json'))).entry;data=JSON.stringify(m);}else data='new-'+url.split('/').at(-1);
fs.writeFileSync(output,data);if(args.includes('--write-out'))process.stdout.write('200');
"###;

const DOCTOR_PREFIX: &str = r###"
use std::{path::{Path,PathBuf},process::Command,time::Duration};
#[derive(Clone,Copy)] enum Mode {Git,Package}
const DOCTOR_TIMEOUT:Duration=Duration::from_secs(40);
fn hidden(command:&mut Command)->&mut Command{command}
fn refreshed_path()->String{std::env::var("PATH").unwrap()}
fn run_with_timeout(command:Command,limit:Duration)->String {
 assert_eq!(limit,Duration::from_secs(40));
 format!("{}|{}|{}",command.get_program().to_string_lossy(),command.get_args().map(|v|v.to_string_lossy()).collect::<Vec<_>>().join(";"),command.get_current_dir().unwrap().display())
}
"###;

const DOCTOR_SUFFIX: &str = r###"
#[test] fn native_doctor_uses_its_installation_without_python(){
 let root=PathBuf::from(std::env::var("FIXTURE_ROOT").unwrap());
 for (mode,windows,expected) in [(Mode::Git,false,"crates/target/release/hangar-server"),(Mode::Git,true,"crates/target/release/hangar-server.exe"),(Mode::Package,false,"current/bin/hangar-server"),(Mode::Package,true,"current/bin/hangar-server.exe")] {
  let (binary,backend)=doctor_paths(mode,&root,windows);
  assert_eq!(binary,root.join(expected));
  assert_eq!(backend,root.join(if matches!(mode,Mode::Package){"current/backend"}else{"backend"}));
 }
 let (binary,backend)=doctor_paths(Mode::Package,&root,false);
 std::fs::create_dir_all(binary.parent().unwrap()).unwrap();std::fs::write(&binary,b"fixture").unwrap();
 assert_eq!(doctor(Mode::Package,&root),Some(format!("{}|doctor;--backend;{}|{}",binary.display(),backend.display(),backend.display())));
}
"###;

#[test]
fn authentication_bootstrap_preserves_codes_urls_files_and_fallback_messages() {
    for row in fixture("update-git/auth-release.json")["cases"]
        .as_array()
        .unwrap()
    {
        let name = row["name"].as_str().unwrap();
        if name.starts_with("hold-")
            || (name.ends_with("publish-failure") && !name.starts_with("branch-"))
        {
            continue;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dest = root.join("dest");
        fs::create_dir(&dest).unwrap();
        for (name, value) in row["initial"].as_object().unwrap() {
            let path = dest.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value.as_str().unwrap()).unwrap();
        }
        let old = (name == "same").then(|| fixture("update-git/bootstrap-old.json"));
        if let Some(old) = &old {
            for (name, value) in old["before"].as_object().unwrap() {
                fs::write(dest.join(name), value.as_str().unwrap()).unwrap();
            }
            fs::create_dir(dest.join("hangar-server.old-directory")).unwrap();
        }
        let mut document = manifest();
        if name == "missing-auth" {
            document["files"]
                .as_object_mut()
                .unwrap()
                .remove("linux-x86_64/hangar-auth");
        }
        if name == "invalid-auth-sha" {
            document["files"]["linux-x86_64/hangar-auth"]["sha256"] = json!("invalid");
        }
        fs::write(root.join("manifest.json"), document.to_string()).unwrap();
        fs::write(root.join("failure.cjs"), AUTH_FAILURE).unwrap();
        let mut command = bootstrap(root, name.starts_with("branch-"));
        executable(root, "curl", AUTH_CURL);
        command.env("PROOF_CASE", name).env(
            "NODE_OPTIONS",
            format!("--require={}", root.join("failure.cjs").display()),
        );
        let output = command.output().unwrap();
        let actual_messages = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains("instalei os da main"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let expected_messages = row["result"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|line| line.contains("instalei os da main"))
            .collect::<Vec<_>>();
        assert_eq!(actual_messages, expected_messages, "{name}");
        assert_eq!(
            output.status.code(),
            Some(i32::from(!row["result"].as_array().unwrap().is_empty())),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected = row["after_publish"].clone();
        if let Some(old) = old {
            expected
                .as_object_mut()
                .unwrap()
                .extend(old["after"].as_object().unwrap().clone());
        }
        assert_eq!(tree(&dest)["files"], expected, "{name}");
        let expected_urls = json!(
            row["urls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().replace(
                    "https://release.invalid",
                    "https://github.com/jeffer1312/hangar/releases/download"
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(lines(&root.join("urls")), expected_urls, "{name}");
    }
}
#[test]
fn bootstrap_rejects_invalid_manifests_without_fallback_or_publication() {
    for case in fixture("update-git/bootstrap-extra.json")["cases"]
        .as_array()
        .unwrap()
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut command = bootstrap(root, true);
        executable(root, "curl", EXTRA_CURL);
        let output = command
            .env("PROOF_LOG", root.join("calls"))
            .env(
                "PROOF_BODY",
                if case["name"] == "branch-invalid-json" {
                    "{"
                } else {
                    "[]"
                },
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(lines(&root.join("calls")), case["urls"]);
        assert_eq!(tree(&root.join("dest"))["files"], json!({}));
    }
}
#[test]
fn bootstrap_preserves_publication_conflicts_and_powershell_encoding() {
    for case in fixture("update-git/bootstrap-review.json")["cases"]
        .as_array()
        .unwrap()
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dest = root.join("dest");
        fs::create_dir(&dest).unwrap();
        let name = case["name"].as_str().unwrap();
        if name == "directory" {
            fs::create_dir(dest.join("hangar-server")).unwrap();
        }
        if name == "symlink-directory" {
            fs::create_dir(dest.join("unchanged")).unwrap();
            std::os::unix::fs::symlink("unchanged", dest.join("hangar-server")).unwrap();
        }
        fs::write(root.join("case.json"), case.to_string()).unwrap();
        fs::write(root.join("manifest.json"), manifest().to_string()).unwrap();
        let mut command = bootstrap(root, name.starts_with("entry-"));
        executable(root, "curl", REVIEW_CURL);
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["code"].as_i64(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(lines(&root.join("calls")), case["urls"], "{name}");
        let actual = tree(&dest);
        for key in ["files", "directories", "links"] {
            assert_eq!(actual[key], case[key], "{name}: {key}");
        }
    }
    assert!(
        fs::read(repo().join("scripts/install-server-binaries.ps1"))
            .unwrap()
            .starts_with(b"\xef\xbb\xbf")
    );
}
#[tokio::test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
async fn native_release_cli_publishes_captured_bytes_using_only_local_http() {
    use axum::{Router, extract::State, http::StatusCode, routing::get};
    use std::sync::{Arc, Mutex};
    let urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut document = manifest();
    document["commit"] = json!("b".repeat(40));
    let router = Router::new()
        .route(
            "/{*path}",
            get(
                |State((urls, document)): State<(Arc<Mutex<Vec<String>>>, Value)>,
                 axum::extract::Path(path): axum::extract::Path<String>| async move {
                    urls.lock().unwrap().push(format!("/{path}"));
                    let name = path.rsplit('/').next().unwrap();
                    if name == "server-latest.json" {
                        (StatusCode::OK, document.to_string())
                    } else if matches!(name, "hangar-auth" | "hangar-cano" | "hangar-server") {
                        (StatusCode::OK, format!("new-{name}"))
                    } else {
                        (StatusCode::NOT_FOUND, String::new())
                    }
                },
            ),
        )
        .with_state((urls.clone(), document));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut command =
        tokio::process::Command::from(cli(root, &["rust-release", root.to_str().unwrap()]));
    command
        .env(
            "HANGAR_SERVER_RELEASE_URL",
            format!("http://{address}/release"),
        )
        .env("HANGAR_UPDATE_OWNS_RUST", "1")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        tree(&root.join(".hangar/bin"))["files"],
        fixture("update-git/auth-release.json")["cases"][0]["after_publish"]
    );
    assert_eq!(
        *urls.lock().unwrap(),
        [
            "/release/server-latest.json",
            "/release/hangar-auth",
            "/release/hangar-cano",
            "/release/hangar-server"
        ]
    );
}
#[test]
fn native_setup_doctor_selects_and_invokes_its_installation_without_python() {
    let source = fs::read_to_string(repo().join("desktop-native/src/app/setup/report.rs")).unwrap();
    let start = source.find("pub(crate) fn doctor_paths(").unwrap();
    let end = source[start..].find("fn run_with_timeout(").unwrap() + start;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let rust = root.join("test.rs");
    let binary = root.join("test");
    fs::write(
        &rust,
        format!("{DOCTOR_PREFIX}{}{DOCTOR_SUFFIX}", &source[start..end]),
    )
    .unwrap();
    assert!(
        Command::new("rustc")
            .args(["--edition=2024", "--test"])
            .arg(rust)
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new(binary)
        .arg("--nocapture")
        .env("FIXTURE_ROOT", root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
