//! Raiz da instalação do Hangar, o Python dela e o lançador do backend. A mesma regra de
//! `backend/app/install_root.py`.
//!
//!   hangar-server launch backend [ARGS]   lê o `~/.hangar/current` uma vez, resolve a versão real e
//!                                         abre o `hangar-server` dela (`boot: direct`) ou, na versão
//!                                         que ainda sobe pelo Python, `-m app.main ARGS`
//!
//! Sem o `current` vale o checkout do lançador; sem nenhum dos dois sai com 3 e `launch_no_current`;
//! uso errado, 2. No Windows o lançador fica de pé e religa o `hangar-server` que cai (decisão 38).
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub use hangar_core::install_root::*;

/// `hangar-server launch …`: `args` sem o nome do programa nem o `launch`.
pub fn launch(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("backend") {
        eprintln!("uso: hangar-server launch backend [ARGS...]");
        return ExitCode::from(2);
    }
    let home = crate::accounts::environment::AccountEnvironment::capture().home;
    let log = args.iter().position(|a| a == "--log").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    // A versão vai em toda linha do lançador: na troca, o log recebe a saída da anterior e da nova.
    let version = std::fs::canonicalize(current(&home)).ok()
        .and_then(|real| real.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".to_owned());
    let explicit = std::env::var_os("CP_RUST_SERVER_BIN");
    let mut lines = vec![format!("launch_start: versão {version}, lançador pid {}", std::process::id())];
    let chosen = backend_noted(&home, &args[1..], cfg!(windows), explicit.as_deref(), &mut lines);
    // O direto no Windows gira o log antes de escrever: estas linhas abrem o arquivo da vida nova.
    let tell = |lines: &[String]| { for line in lines { report(log.as_deref(), line); } };
    match chosen {
        Ok(Backend::Direct(command)) => run_direct(command, log.as_deref(), &version, lines),
        Ok(Backend::Python(command)) => { tell(&lines); run(command, log.as_deref(), &version) }
        Err((code, path)) => {
            tell(&lines);
            report(log.as_deref(), &format!("{code}: {}", path.display()));
            failed(log.as_deref(), "erro", code, &path.display().to_string());
            ExitCode::from(3)
        }
    }
}

/// `lancador.falhou` no diário: o gerenciador religa sem fim, e a causa fica no diário que o
/// relatório leva. `nivel` aviso quando o servidor sobe mesmo assim. O lançador não tem log
/// próprio: a falha de gravar vai ao log da tarefa e ao stderr.
fn failed(log: Option<&Path>, level: &str, code: &str, detail: &str) {
    record(log, code, serde_json::json!({"evento": "lancador.falhou", "nivel": level, "codigo": code, "detalhe": detail}));
}

/// Uma linha do lançador no diário; a falha de gravar vai ao log da tarefa e ao stderr, com `what`.
fn record(log: Option<&Path>, what: &str, line: serde_json::Value) {
    let written = match hangar_core::diag::diary_dir() {
        Some(dir) => hangar_core::diag::write_diary(&dir, &[line], chrono::Local::now()).map_err(|error| format!("{:?}", error.kind())),
        None => Err("sem pasta de logs".to_owned()),
    };
    if let Err(cause) = written {
        report(log, &format!("launch_diary_unwritable: {what} não gravado no diário ({cause})"));
    }
}

/// A tarefa do Windows roda o lançador sem console: o erro dele vai também ao log do backend.
fn report(log: Option<&Path>, line: &str) {
    eprintln!("hangar-server launch: {line}");
    if let Some(log) = log {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(log)
            .and_then(|mut f| writeln!(f, "[hangar] hangar-server launch: {line}"));
    }
}

/// No Unix o lançador vira o backend: o serviço acompanha o próprio Python.
#[cfg(unix)]
fn run(mut command: std::process::Command, log: Option<&Path>, version: &str) -> ExitCode {
    use std::os::unix::process::CommandExt;
    if let Err(error) = crate::boot_version::PythonLaunch::capture(&command).and_then(|launch| launch.record_exec()) {
        report(log, &format!("launch_python_version_unavailable: {error}"));
    }
    // A unit é `Type=notify` com `WatchdogSec` para o `hangar-server`; a versão que sobe pelo Python
    // não avisa: o lançador diz pronto e desliga o watchdog antes de virar o Python.
    #[cfg(target_os = "linux")]
    if let Err(error) = notify_from_env("READY=1\nWATCHDOG_USEC=0") {
        report(log, &format!("launch_notify_failed: {error}"));
    }
    let error = command.exec();
    report(log, &format!("launch_exec_failed: versão {version}: {error}"));
    failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
    ExitCode::from(126)
}

/// O lançador vira o `hangar-server`: mesmo pid, então o `sd_notify` dele vale para a unit. Quem
/// religa a queda é o systemd/launchd.
#[cfg(unix)]
fn run_direct(mut command: std::process::Command, log: Option<&Path>, version: &str, lines: Vec<String>) -> ExitCode {
    use std::os::unix::process::CommandExt;
    for line in &lines { report(log, line); }
    let error = command.exec();
    report(log, &format!("launch_exec_failed: versão {version}: {error}"));
    failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
    ExitCode::from(126)
}

/// Sem `NOTIFY_SOCKET` (unit `Type=simple`, rodando na mão) não há a quem avisar.
#[cfg(target_os = "linux")]
fn notify_from_env(state: &str) -> std::io::Result<()> {
    hangar_boot::notify::Notifier::from_lookup(|key| std::env::var(key).ok(), std::process::id()).send(state)
}

/// No Windows: o servidor no Job do lançador, com o log da tarefa como saída (o Python o abria sozinho).
/// O Agendador não religa quem cai depois de subir (decisão 38): o servidor que sai sem parada pedida
/// (`exit_is_requested_stop`) volta depois da espera de `Relaunch`, sem desistir. O lançador só sai com a
/// parada pedida ou morto, e aí o Job leva o servidor.
#[cfg(windows)]
fn run_direct(mut command: std::process::Command, log: Option<&Path>, version: &str, lines: Vec<String>) -> ExitCode {
    let launcher = std::process::id();
    let mut relaunch = Relaunch::new(relaunch_first());
    let (mut pending, mut previous_fall) = (lines, None::<u32>);
    loop {
        // Cada vida gira e abre o log de novo, num handle só e em acréscimo: o lançador escreve antes
        // de abrir o servidor e depois que ele sai, nunca junto com ele.
        let (file, held) = match log.map(|target| (target, open_log(target))) {
            None => (None, None),
            Some((target, Ok((file, used)))) => {
                let held = (used != target).then(|| format!("AVISO: {} está preso por outro processo; a saída vai para {}", target.display(), used.display()));
                (Some(file), held)
            }
            Some((target, Err(error))) => {
                report(log, &format!("launch_log_unavailable: {}: {error}", target.display()));
                failed(log, "aviso", "launch_log_unavailable", &format!("{:?}", error.kind()));
                (None, None)
            }
        };
        // `launch_start` e as notas da escolha abrem o arquivo da primeira vida.
        for line in pending.drain(..).chain(held) { say(file.as_ref(), log, &line); }
        let after = previous_fall.map(|fall| format!(", depois da queda {fall}")).unwrap_or_default();
        say(file.as_ref(), log, &format!("launch_backend_starting: versão {version}, lançador pid {launcher}{after}"));
        if previous_fall.is_none() && let Err(error) = die_with_the_launcher() {
            say(file.as_ref(), log, &format!("launch_job_failed: {error}"));
            failed(log, "aviso", "launch_job_failed", &format!("{:?}", error.kind()));
        }
        match file.as_ref().map(|file| (file.try_clone(), file.try_clone())) {
            Some((Ok(out), Ok(err))) => { command.stdout(out).stderr(err); }
            Some((Err(error), _) | (_, Err(error))) => {
                say(file.as_ref(), log, &format!("launch_log_unavailable: {error}"));
                failed(log, "aviso", "launch_log_unavailable", &format!("{:?}", error.kind()));
                command.stdout(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit());
            }
            None => { command.stdout(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()); }
        }
        let started = std::time::Instant::now();
        let (ended, exit) = match command.spawn() {
            Err(error) => {
                failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
                (format!("launch_exec_failed: versão {version}: {error}"), None)
            }
            Ok(mut child) => {
                let pid = child.id();
                let status = match child.wait() {
                    Ok(status) => status,
                    Err(error) => {
                        say(file.as_ref(), log, &format!("launch_wait_failed: versão {version}, hangar-server pid {pid}: {error}"));
                        return ExitCode::from(126);
                    }
                };
                let exited = format!("launch_backend_exited: versão {version}, hangar-server pid {pid}, {status}, depois de {} s", started.elapsed().as_secs());
                if exit_is_requested_stop(status.code()) {
                    if !status.success() { say(file.as_ref(), log, &exited); }
                    std::process::exit(status.code().unwrap_or(1));
                }
                (exited, status.code())
            }
        };
        // O `Command` guarda as cópias do handle: soltas aqui, o log gira sem arquivo aberto.
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let lived = started.elapsed();
        let (fall, wait) = relaunch.fall(lived);
        say(file.as_ref(), log, &format!("{ended}; religa em {} ms (queda {fall})", wait.as_millis()));
        // A queda vai ao diário que o relatório leva: o servidor só anota a vida que não parou limpa
        // na subida seguinte, sem o código de saída.
        if let Some(code) = exit {
            record(log, "lancador.queda", serde_json::json!({"evento": "lancador.queda", "nivel": "erro", "codigo": "server_exited",
                "saida": format!("0x{:08X}", code as u32), "vida_s": lived.as_secs(), "queda": fall, "espera_ms": wait.as_millis() as u64}));
        }
        drop(file);
        std::thread::sleep(wait);
        previous_fall = Some(fall);
    }
}

/// A primeira espera da religada. `HANGAR_LAUNCH_RELAUNCH_FIRST_MS` só existe para os testes, que
/// não esperam segundos por queda.
#[cfg(windows)]
fn relaunch_first() -> std::time::Duration {
    std::env::var("HANGAR_LAUNCH_RELAUNCH_FIRST_MS").ok().and_then(|ms| ms.parse().ok())
        .map(std::time::Duration::from_millis).unwrap_or(RELAUNCH_FIRST)
}

/// Linha do lançador pelo handle do log; sem ele, pelo `report` de sempre.
#[cfg(windows)]
fn say(file: Option<&std::fs::File>, log: Option<&Path>, line: &str) {
    use std::io::Write;
    match file {
        Some(mut file) => { eprintln!("hangar-server launch: {line}"); let _ = writeln!(file, "[hangar] hangar-server launch: {line}"); }
        None => report(log, line),
    }
}

/// O log da tarefa da subida anterior vai para `<nome>.1` (e os outros andam uma casa, até `.5`): a
/// saída de uma queda fica para ler. Aberto em acréscimo e com escrita compartilhada (o padrão do
/// `std` no Windows), um neto que sobreviveu com a cópia herdada não impede a subida; preso mesmo
/// assim, a saída vai para o primeiro `.N` que abrir.
#[cfg_attr(not(windows), allow(dead_code))]
fn open_log(target: &Path) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    if let Some(parent) = target.parent() { let _ = std::fs::create_dir_all(parent); }
    let (stem, ext) = (target.file_stem().unwrap_or_default().to_string_lossy(), target.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default());
    let numbered = |n: u32| target.with_file_name(format!("{stem}.{n}{ext}"));
    let _ = std::fs::remove_file(numbered(5));
    for n in (1..5).rev() { let _ = std::fs::rename(numbered(n), numbered(n + 1)); }
    let _ = std::fs::rename(target, numbered(1));
    let mut last = std::io::Error::other("sem candidato");
    for candidate in std::iter::once(target.to_path_buf()).chain((1..=5).map(numbered)) {
        match std::fs::OpenOptions::new().append(true).create(true).open(&candidate) {
            Ok(file) => return Ok((file, candidate)),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// No Windows não há `exec`: espera o backend e sai com o código dele, os dois no mesmo Job.
#[cfg(windows)]
fn run(mut command: std::process::Command, log: Option<&Path>, version: &str) -> ExitCode {
    // Sem o Job o backend sobe como antes, só sem a proteção: a falha vai ao log da tarefa.
    if let Err(error) = die_with_the_launcher() {
        report(log, &format!("launch_job_failed: {error}"));
        failed(log, "aviso", "launch_job_failed", &format!("{:?}", error.kind()));
    }
    let snapshot = match crate::boot_version::PythonLaunch::capture(&command) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => { report(log, &format!("launch_python_version_unavailable: {error}")); None }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report(log, &format!("launch_exec_failed: versão {version}: {error}"));
            failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
            return ExitCode::from(126);
        }
    };
    let (pid, started) = (child.id(), std::time::Instant::now());
    report(log, &format!("launch_backend_started: versão {version}, python pid {pid}"));
    if let Some(snapshot) = snapshot {
        let mut birth = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let confirmed = loop {
            if birth.is_none() { birth = crate::terminal_facts::agent_birth(pid); }
            if let Some(birth) = birth {
                match snapshot.record_windows(pid, birth) {
                    Ok(true) => break true,
                    Err(error) => { report(log, &format!("launch_python_version_unavailable: {error}")); break false; }
                    Ok(false) => {}
                }
            }
            if std::time::Instant::now() >= deadline || child.try_wait().ok().flatten().is_some() { break false; }
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        };
        if !confirmed { report(log, "launch_python_version_unavailable: interpretador efetivo não confirmado"); }
    }
    match child.wait() {
        Ok(status) => {
            // 0xffffffff é o `Stop-Process` do reinício: a versão que estava de pé sendo parada.
            if !status.success() {
                report(log, &format!("launch_backend_exited: versão {version}, python pid {pid}, {status}, depois de {} s", started.elapsed().as_secs()));
            }
            std::process::exit(status.code().unwrap_or(1))
        }
        Err(error) => {
            report(log, &format!("launch_wait_failed: versão {version}, python pid {pid}: {error}"));
            ExitCode::from(126)
        }
    }
}

/// Põe o lançador num Job que mata o grupo quando o último handle fecha: matar o lançador
/// (`Stop-ScheduledTask`, `Stop-Process`) leva o backend junto e a porta não fica presa com um órfão.
/// O handle fica aberto até o processo sair. `BREAKAWAY_OK`: o que nasce com
/// `CREATE_BREAKAWAY_FROM_JOB` (terminal de sessão) sobrevive, como no Job do `runtime_process`.
#[cfg(windows)]
fn die_with_the_launcher() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() { return Err(std::io::Error::last_os_error()); }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&info) as u32);
        // O próprio lançador entra antes de criar o filho: ele já nasce dentro, sem janela de corrida.
        if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            let error = std::io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{current, root};

    /// B-36: cada subida começa um log novo e a saída da anterior (a de uma queda) fica em `.1`; as
    /// mais velhas andam uma casa até `.5`.
    #[test]
    fn the_task_log_keeps_the_previous_start_in_the_numbered_files() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let target = logs.join("hangar-backend.log");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(&target, "queda").unwrap();
        for n in 1..=5 { std::fs::write(logs.join(format!("hangar-backend.{n}.log")), format!("velho {n}")).unwrap(); }
        let (mut file, used) = super::open_log(&target).unwrap();
        assert_eq!(used, target);
        file.write_all(b"nova").unwrap();
        file.write_all(b" subida").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nova subida");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.1.log")).unwrap(), "queda");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.2.log")).unwrap(), "velho 1");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.5.log")).unwrap(), "velho 4", "o mais velho sai");
    }

    /// P2: a versão que sobe pelo Python avisa pronto e desliga o watchdog; sem socket, nada a fazer.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launcher_tells_systemd_ready_and_turns_the_watchdog_off_for_the_python() {
        use std::os::linux::net::SocketAddrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let listener = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let to = |socket: Option<String>| hangar_boot::notify::Notifier::from_lookup(move |key| (key == "NOTIFY_SOCKET").then(|| socket.clone()).flatten(), 1);
        to(Some(path.to_string_lossy().into_owned())).send("READY=1\nWATCHDOG_USEC=0").unwrap();
        let mut buf = [0u8; 64];
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\nWATCHDOG_USEC=0");
        let name = format!("hangar-notify-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let abstract_listener = std::os::unix::net::UnixDatagram::bind_addr(&address).unwrap();
        to(Some(format!("@{name}"))).send("READY=1").unwrap();
        let n = abstract_listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        to(None).send("READY=1").unwrap();
    }

    #[test]
    fn install_root_is_the_unresolved_current_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_ne!(root(home), Some(current(home)), "sem o current a raiz é a cópia do Hangar");
        let real = home.join(".hangar").join("versions").join("1.0.0.5");
        std::fs::create_dir_all(&real).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, current(home)).unwrap();
        #[cfg(windows)]
        crate::update::make_link(&real, &current(home)).unwrap();
        assert_eq!(root(home), Some(home.join(".hangar").join("current")));
    }
}
