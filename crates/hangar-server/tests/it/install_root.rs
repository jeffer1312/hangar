//! `hangar-server launch backend`: o lançador que o serviço chama. Lê o `~/.hangar/current` uma vez e
//! executa o Python da versão REAL, na `backend/` dela, com o cache de bytecode fora da pasta da versão.
use std::path::Path;

fn launch(home: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .arg("launch").args(args).env_clear().env("HOME", home).env("USERPROFILE", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default()).output().unwrap()
}

/// `current` como a instalação grava: symlink no Unix, junction no Windows.
fn link_current(real: &Path, home: &Path) {
    link_dir(&home.join(".hangar").join("current"), real);
}

fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    // O cmd não entende as aspas que o `arg` do Rust gera: a linha vai crua, como no `update::make_link`.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("cmd.exe")
            .raw_arg(format!("/c mklink /J \"{}\" \"{}\"", link.display(), target.display())).output().unwrap();
        assert!(out.status.success(), "junction não criada: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn launch_without_current_refuses_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let out = launch(dir.path(), &["backend"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("launch_no_current"), "{out:?}");
    assert_eq!(launch(dir.path(), &[]).status.code(), Some(2), "sem o alvo é uso errado");
}

/// O Python falso grava argv, pasta de trabalho e cache de bytecode; o lançador sai com o código dele.
#[cfg(unix)]
#[test]
fn launch_backend_runs_the_real_version_python() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let real = home.join(".hangar/versions/0.1.0.8207");
    std::fs::create_dir_all(real.join("python/bin")).unwrap();
    std::fs::create_dir_all(real.join("backend")).unwrap();
    crate::write_executable(&real.join("python/bin/python3"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$(pwd -P)\" \"$PYTHONPYCACHEPREFIX\" > \"$HOME/record\"\nexit 7\n");
    link_current(&real, &home);
    let out = launch(&home, &["backend", "--log", "/tmp/x.log"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let record = std::fs::read_to_string(home.join("record")).unwrap();
    let real = real.canonicalize().unwrap();
    assert_eq!(record.lines().collect::<Vec<_>>(), [
        "-m app.main --log /tmp/x.log",
        real.join("backend").to_str().unwrap(),
        // Sem prefixo de bytecode: o Python lê os `.pyc` que vêm no pacote.
        "",
    ]);
}

/// Versão sem o Python dela não sobe nada que pareça o backend.
#[test]
fn launch_without_the_version_python_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let real = home.join(".hangar").join("versions").join("v1");
    std::fs::create_dir_all(real.join("backend")).unwrap();
    link_current(&real, home);
    let out = launch(home, &["backend"]);
    assert_eq!(out.status.code(), Some(3));
    // A mensagem aponta o Python que faltou (o último candidato, o venv), não o `current`.
    let venv = if cfg!(windows) { Path::new(".venv").join("Scripts").join("python.exe") } else { Path::new(".venv").join("bin").join("python") };
    let missing = real.canonicalize().unwrap().join("backend").join(venv);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let missing = missing.display().to_string();
    assert!(stderr.contains(&format!("launch_no_python: {}", missing.trim_start_matches(r"\\?\"))), "{stderr}");
}

/// Matar o lançador, como o `Stop-ScheduledTask` ou o `Stop-Process`, leva o backend junto: a porta
/// não fica presa com um órfão. Precisa de um Python de verdade (o `python.exe` com a `Lib` ao lado).
#[cfg(windows)]
#[test]
#[ignore = "rodar na VM Windows com HANGAR_TEST_PYTHON_DIR=<pasta do Python embutido>"]
fn launch_killed_takes_the_backend_along() {
    use std::time::{Duration, Instant};
    let python_dir = std::env::var_os("HANGAR_TEST_PYTHON_DIR").expect("HANGAR_TEST_PYTHON_DIR");
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let real = home.join(".hangar").join("versions").join("v1");
    std::fs::create_dir_all(real.join("backend").join("app")).unwrap();
    link_dir(&real.join("python"), Path::new(&python_dir));
    let pid_file = home.join("backend.pid");
    std::fs::write(real.join("backend").join("app").join("main.py"), format!(
        "import os, time\nopen(r'{}.tmp', 'w').write(str(os.getpid()))\nos.replace(r'{0}.tmp', r'{0}')\ntime.sleep(600)\n",
        pid_file.display())).unwrap();
    link_current(&real, home);
    let mut launcher = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server")).args(["launch", "backend"])
        .env("HOME", home).env("USERPROFILE", home).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.trim().parse::<u32>().ok()) { break pid; }
        assert!(Instant::now() < deadline, "o backend falso não subiu");
        std::thread::sleep(Duration::from_millis(100));
    };
    launcher.kill().unwrap();
    launcher.wait().unwrap();
    let alive = || {
        let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while alive() {
        assert!(Instant::now() < deadline, "o backend {pid} sobreviveu ao lançador");
        std::thread::sleep(Duration::from_millis(200));
    }
}
