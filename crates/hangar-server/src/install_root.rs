//! Raiz da instalação do Hangar, o Python dela e o lançador do backend. A mesma regra de
//! `backend/app/install_root.py`.
//!
//!   hangar-server launch backend [ARGS]   lê o `~/.hangar/current` uma vez, resolve a versão real e
//!                                         executa o Python dela com `-m app.main ARGS`
//!
//! Sem o `current` (máquina sem o pacote) o lançador sai com 3 e `launch_no_current`; uso errado, 2.
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
    report(log.as_deref(), &format!("launch_start: versão {version}, lançador pid {}", std::process::id()));
    match backend_command(&home, &args[1..], cfg!(windows)) {
        Ok(command) => run(command, log.as_deref(), &version),
        Err((code, path)) => {
            report(log.as_deref(), &format!("{code}: {}", path.display()));
            ExitCode::from(3)
        }
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
    let error = command.exec();
    report(log, &format!("launch_exec_failed: versão {version}: {error}"));
    ExitCode::from(126)
}

/// No Windows não há `exec`: espera o backend e sai com o código dele, os dois no mesmo Job.
#[cfg(windows)]
fn run(mut command: std::process::Command, log: Option<&Path>, version: &str) -> ExitCode {
    // Sem o Job o backend sobe como antes, só sem a proteção: a falha vai ao log da tarefa.
    if let Err(error) = die_with_the_launcher() {
        report(log, &format!("launch_job_failed: {error}"));
    }
    let snapshot = match crate::boot_version::PythonLaunch::capture(&command) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => { report(log, &format!("launch_python_version_unavailable: {error}")); None }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report(log, &format!("launch_exec_failed: versão {version}: {error}"));
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
