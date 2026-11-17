//! `hangar-server launch backend`: o lançador que o serviço chama. Lê o `~/.hangar/current` uma vez e
//! executa o Python da versão REAL, na `backend/` dela, com o cache de bytecode fora da pasta da versão.
use std::path::Path;

fn launch(home: &Path, args: &[&str]) -> std::process::Output {
    launch_command(home, args).output().unwrap()
}

/// Sem `current` o lançador cai no checkout acima do próprio binário (modo desenvolvedor). No CI o
/// target fica dentro do repositório: a recusa sem `current` só se prova com uma cópia fora dele.
fn launch_outside_checkout(home: &Path, args: &[&str]) -> std::process::Output {
    let copy = home.join("launcher-copy").join(Path::new(env!("CARGO_BIN_EXE_hangar-server")).file_name().unwrap());
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_hangar-server"), &copy).unwrap();
    launch_program(&copy, home, args).output().unwrap()
}

fn launch_command(home: &Path, args: &[&str]) -> std::process::Command {
    launch_program(Path::new(env!("CARGO_BIN_EXE_hangar-server")), home, args)
}

fn launch_program(program: &Path, home: &Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.arg("launch").args(args).env_clear().env("HOME", home).env("USERPROFILE", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    if cfg!(windows) {
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) { command.env(key, value); }
        }
    }
    command
}

/// O diário do Rust pela pasta de logs do HOME de teste (`LOCALAPPDATA` fica de fora no `env_clear`).
fn diary(home: &Path) -> Vec<serde_json::Value> {
    let base = if cfg!(windows) { home.join("AppData").join("Local").join("hangar").join("logs") } else { home.join(".hangar").join("logs") };
    let Ok(dir) = std::fs::read_dir(base.join("diario").join("servidor-rust")) else { return Vec::new() };
    dir.flatten().flat_map(|entry| std::fs::read_to_string(entry.path()).unwrap().lines()
        .filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<_>>()).collect()
}

/// Versão do pacote que sobe direto (`boot: direct`), sem `backend/` e sem o `bin/hangar-server`.
fn direct_version(home: &Path) -> std::path::PathBuf {
    let real = home.join(".hangar").join("versions").join("v1");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("manifest.json"), r#"{"version":"v1","boot":"direct"}"#).unwrap();
    link_current(&real, home);
    real
}

/// P9: a versão que sobe direto roda na `backend/` dela enquanto a tiver (a anterior, na volta, só
/// acha o `backend/.env` da máquina não migrada pelo cwd) e, sem ela, na raiz.
#[cfg(unix)]
#[test]
fn a_direct_version_runs_its_server_in_its_backend_or_else_in_the_version_root() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let real = direct_version(home);
    let server = home.join("server-falso");
    std::fs::write(&server, "#!/bin/sh\npwd -P > \"$HOME/cwd\"\n").unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = || {
        let out = launch_command(home, &["backend"]).env("CP_RUST_SERVER_BIN", &server).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        std::fs::read_to_string(home.join("cwd")).unwrap().trim().to_owned()
    };
    assert_eq!(cwd(), real.canonicalize().unwrap().display().to_string(), "sem backend/: a raiz");
    std::fs::create_dir_all(real.join("backend")).unwrap();
    assert_eq!(cwd(), real.join("backend").canonicalize().unwrap().display().to_string(), "com backend/: ela");
}

/// As linhas do lançador no log da tarefa, sem o prefixo.
fn launcher_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap().lines()
        .filter_map(|line| line.strip_prefix("[hangar] hangar-server launch: ")).map(str::to_owned).collect()
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
    let out = launch_outside_checkout(dir.path(), &["backend"]);
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
    // Decisões 1 e 35: versão do pacote sem `boot: direct` sobe pelo Python; sem manifesto seria checkout.
    std::fs::write(real.join("manifest.json"), r#"{"version":"0.1.0.8207"}"#).unwrap();
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
    std::fs::write(real.join("manifest.json"), r#"{"version":"v1"}"#).unwrap();
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

/// B-19: sem um `hangar-server` utilizável o lançador não sobe nada, sai com 3, diz por quê no stderr e
/// grava `lancador.falhou` no diário (o gerenciador religa sem fim; o relatório leva a causa).
#[test]
fn b19_launch_without_a_server_binary_exits_3_and_records_the_failure() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let real = direct_version(home);
    let out = launch(home, &["backend"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let exe = if cfg!(windows) { "hangar-server.exe" } else { "hangar-server" };
    let missing = real.canonicalize().unwrap().join("bin").join(exe).display().to_string();
    let missing = missing.trim_start_matches(r"\\?\");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("hangar-server launch: launch_no_server: {missing}")), "{stderr}");
    let lines = diary(home);
    let failed: Vec<_> = lines.iter().filter(|line| line["evento"] == "lancador.falhou").collect();
    assert_eq!(failed.len(), 1, "{lines:?}");
    assert_eq!(failed[0]["nivel"], "erro");
    assert_eq!(failed[0]["codigo"], "launch_no_server");
    assert_eq!(failed[0]["detalhe"], missing);
}

/// O servidor que não executa sai com 126 e também fica no diário como `lancador.falhou`, com o
/// caminho do binário: o gerenciador religa sem fim e o relatório leva a causa.
#[cfg(unix)]
#[test]
fn launch_exec_failure_exits_126_and_records_the_failure() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    direct_version(home);
    // Interpretador que não existe: o `exec` falha com ENOENT, sem o `/bin/sh` que o `execvp` tenta no ENOEXEC.
    let broken = home.join("hangar-server-quebrado");
    std::fs::write(&broken, "#!/nao/existe/interpretador\n").unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = launch_command(home, &["backend"]).env("CP_RUST_SERVER_BIN", &broken).output().unwrap();
    assert_eq!(out.status.code(), Some(126), "{out:?}");
    let lines = diary(home);
    let failed: Vec<_> = lines.iter().filter(|line| line["evento"] == "lancador.falhou").collect();
    assert_eq!(failed.len(), 1, "{lines:?}");
    assert_eq!((failed[0]["nivel"].as_str(), failed[0]["codigo"].as_str()), (Some("erro"), Some("launch_exec_failed")));
    assert_eq!(failed[0]["detalhe"], broken.display().to_string());
}

/// B-37: a tarefa do Windows roda o lançador sem console; a subida e a recusa vão ao log da tarefa
/// (`--log`), além do stderr.
#[test]
fn b37_launch_start_and_refusal_reach_the_task_log() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let log = home.join("logs").join("hangar-backend.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let out = launch_outside_checkout(home, &["backend", "--log", log.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let lines = launcher_lines(&log);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("launch_start: versão ?, lançador pid "), "{lines:?}");
    assert!(lines[1].starts_with("launch_no_current: "), "{lines:?}");
}

#[cfg(windows)]
fn cmd_exe() -> std::path::PathBuf {
    Path::new(&std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32").join("cmd.exe")
}

/// B-37 (Windows): o lançador abre o servidor e, quando ele é parado pelo `Stop-Process` do reinício
/// (saída `0xFFFFFFFF`), diz no log da tarefa quando começou e como saiu, sem religar; o código de
/// saída é o do servidor. `launch_start` fica no arquivo desta subida, não no `.1` da anterior.
#[cfg(windows)]
#[test]
fn b37_windows_direct_launch_writes_starting_and_exited_to_the_task_log() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    direct_version(home);
    let log = home.join("logs").join("hangar-backend.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, "[hangar] hangar-server launch: subida anterior\n").unwrap();
    let out = launch_command(home, &["backend", "--log", log.to_str().unwrap(), "/c", "exit", "-1"])
        .env("CP_RUST_SERVER_BIN", cmd_exe()).output().unwrap();
    assert_eq!(out.status.code(), Some(-1), "{out:?}");
    let lines = launcher_lines(&log);
    assert!(lines[0].starts_with("launch_start: versão v1, lançador pid "), "{lines:?}");
    assert!(lines.iter().any(|line| line.starts_with("launch_backend_starting: versão v1, lançador pid ")), "{lines:?}");
    let exited = lines.iter().find(|line| line.starts_with("launch_backend_exited: versão v1, hangar-server pid ")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(exited.contains("depois de ") && exited.ends_with(" s"), "parada pedida não religa: {exited}");
    assert_eq!(launcher_lines(&home.join("logs").join("hangar-backend.1.log")), ["subida anterior"], "nada desta subida no .1");
}

/// Decisão 38 (Windows): o servidor que cai é religado pelo lançador com espera crescente (2 s
/// dobrando; aqui 200 ms pela variável dos testes), cada vida num log novo e cada queda no log e no
/// diário; a saída limpa (0) encerra o lançador.
#[cfg(windows)]
#[test]
fn b38_windows_launcher_relaunches_a_fallen_server_with_growing_waits() {
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    direct_version(home);
    let log = home.join("logs").join("hangar-backend.log");
    // Servidor falso: cai com 5 três vezes e na quarta vida para limpo.
    let fake = home.join("fake-server.cmd");
    std::fs::write(&fake, "@echo off\r\nset n=0\r\nif exist \"%~dp0count.txt\" set /p n=<\"%~dp0count.txt\"\r\nset /a n+=1\r\n\
        >\"%~dp0count.txt\" echo %n%\r\nif %n% geq 4 exit 0\r\nexit 5\r\n").unwrap();
    let started = Instant::now();
    let mut launcher = launch_command(home, &["backend", "--log", log.to_str().unwrap(), "/c", fake.to_str().unwrap()])
        .env("CP_RUST_SERVER_BIN", cmd_exe()).env("HANGAR_LAUNCH_RELAUNCH_FIRST_MS", "200")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let status = loop {
        if let Some(status) = launcher.try_wait().unwrap() { break status; }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = launcher.kill();
            panic!("o lançador não saiu depois da parada limpa; vidas: {:?}", std::fs::read_to_string(home.join("count.txt")));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0), "a parada limpa encerra o lançador");
    assert_eq!(std::fs::read_to_string(home.join("count.txt")).unwrap().trim(), "4");
    assert!(started.elapsed() >= Duration::from_millis(200 + 400 + 800), "as esperas não foram cumpridas: {:?}", started.elapsed());
    // Uma vida por arquivo: a mais velha em `.3`, a da parada no log da tarefa.
    let files = ["hangar-backend.3.log", "hangar-backend.2.log", "hangar-backend.1.log", "hangar-backend.log"]
        .map(|name| launcher_lines(&home.join("logs").join(name)));
    assert!(files[0][0].starts_with("launch_start: versão v1, lançador pid "), "{files:?}");
    let pid = files[0][0].rsplit(' ').next().unwrap().to_owned();
    for (life, lines) in files.iter().enumerate() {
        let after = if life == 0 { String::new() } else { format!(", depois da queda {life}") };
        let starting = format!("launch_backend_starting: versão v1, lançador pid {pid}{after}");
        assert!(lines.contains(&starting), "vida {}: {lines:?}", life + 1);
        let exited: Vec<_> = lines.iter().filter(|line| line.starts_with("launch_backend_exited: ")).collect();
        if life == 3 {
            assert!(exited.is_empty(), "a parada limpa não é queda: {lines:?}");
        } else {
            let wait = 200u64 << life;
            assert_eq!(exited.len(), 1, "{lines:?}");
            assert!(exited[0].contains("exit code: 5") && exited[0].ends_with(&format!("; religa em {wait} ms (queda {})", life + 1)), "{}", exited[0]);
        }
    }
    let falls: Vec<_> = diary(home).into_iter().filter(|line| line["evento"] == "lancador.queda").collect();
    assert_eq!(falls.len(), 3, "{falls:?}");
    for (n, fall) in falls.iter().enumerate() {
        assert_eq!((fall["nivel"].as_str(), fall["codigo"].as_str(), fall["saida"].as_str()), (Some("erro"), Some("server_exited"), Some("0x00000005")), "{fall}");
        assert_eq!((fall["queda"].as_u64(), fall["espera_ms"].as_u64()), (Some(n as u64 + 1), Some(200u64 << n)), "{fall}");
    }
}

/// Decisão 38 (Windows): o `Stop-Process -Force` do reinício (`TerminateProcess`, saída `0xFFFFFFFF`)
/// não roda os tratadores do servidor, e a marca de pé dele ficaria. O lançador a fecha como parada
/// limpa: a subida seguinte não conta queda e nada vai ao diário como queda.
#[cfg(windows)]
#[test]
fn d38_a_requested_stop_closes_the_server_life_and_counts_no_fall() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    direct_version(home);
    let hangar = home.join(".hangar");
    // Servidor falso: grava a marca de pé com o próprio pid, como o `record_running`, e sai com -1.
    let script = "$d = Join-Path $env:USERPROFILE '.hangar'; New-Item -ItemType Directory -Force $d | Out-Null; \
        Set-Content -Encoding ascii (Join-Path $d 'server-life.json') ('{\"running\":true,\"unclean_exits\":0,\"pid\":' + $PID + '}'); exit -1";
    let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
    let powershell = Path::new(&std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let out = launch_command(home, &["backend", "-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .env("CP_RUST_SERVER_BIN", &powershell).output().unwrap();
    assert_eq!(out.status.code(), Some(-1), "{out:?}");
    let life: serde_json::Value = serde_json::from_slice(&std::fs::read(hangar.join(hangar_boot::life::FILE)).unwrap()).unwrap();
    assert_eq!(life["running"], false, "{life}");
    let next = hangar_boot::life::begin(&hangar, "agora", |_, _| false).unwrap();
    assert_eq!((next.unclean, next.unclean_exits), (false, 0));
    let lines = diary(home);
    assert!(lines.iter().all(|line| line["evento"] != "lancador.queda"), "{lines:?}");
    let stops: Vec<_> = lines.iter().filter(|line| line["evento"] == "lancador.parada").collect();
    assert_eq!(stops.len(), 1, "{lines:?}");
    assert_eq!((stops[0]["saida"].as_str(), stops[0]["vida"].as_str()), (Some("0xFFFFFFFF"), Some("limpa")));
}

/// B-35 (Windows): o lançador põe o `hangar-server` no Job dele (`KILL_ON_JOB_CLOSE`): matar o
/// lançador, como o `Stop-ScheduledTask`/`Stop-Process`, leva o servidor junto e solta a porta.
#[cfg(windows)]
#[test]
fn b35_killing_the_launcher_takes_the_server_along_and_frees_the_port() {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    direct_version(home);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = home.join(".hangar").join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".env"), format!("CP_PORT={port}\nCP_AUTH_TOKEN=proof\n")).unwrap();
    let health = || {
        let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return false };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        if stream.write_all(b"GET /__hangar_server/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").is_err() { return false; }
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 200")
    };
    let mut launcher = launch_command(home, &["backend"]).env("PATH", home)
        .env("CP_RUST_SERVER_BIN", env!("CARGO_BIN_EXE_hangar-server"))
        .env("CP_RUST_NO_ORPHAN_SWEEP", "1").env("HANGAR_SERVER_GUEST_LISTEN", "127.0.0.1:0")
        .env(hangar_boot::settings::KEEP_INHERITED, "1").stdin(std::process::Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !health() {
        assert!(launcher.try_wait().unwrap().is_none(), "o lançador saiu antes de o servidor subir");
        assert!(Instant::now() < deadline, "o servidor não abriu a porta {port}");
        std::thread::sleep(Duration::from_millis(100));
    }
    launcher.kill().unwrap();
    launcher.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() || std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "o hangar-server sobreviveu ao lançador e segura a porta {port}");
        std::thread::sleep(Duration::from_millis(200));
    }
}
