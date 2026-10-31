//! Contratos dos comandos, em processos isolados e sem serviços reais.
#![cfg(unix)]
use std::{
    fs,
    os::unix::ffi::OsStringExt,
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
#[cfg(target_os = "linux")]
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
