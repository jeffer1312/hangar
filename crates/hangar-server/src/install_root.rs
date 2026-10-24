//! Raiz da instalação do Hangar, o Python dela e o lançador do backend. A mesma regra de
//! `backend/app/install_root.py`.
//!
//!   hangar-server launch backend [ARGS]   lê o `~/.hangar/current` uma vez, resolve a versão real e
//!                                         executa o Python dela com `-m app.main ARGS`
//!
//! Sem o `current` (máquina sem o pacote) o lançador sai com 3 e `launch_no_current`; uso errado, 2.
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `~/.hangar/current`: link (symlink no Linux, junction no Windows) para `versions/<v>` ou, no modo
/// desenvolvedor, para um checkout.
pub fn current(home: &Path) -> PathBuf { home.join(".hangar").join("current") }

/// `~/.hangar/current` quando existe, SEM resolver o link: o que se grava fora do Hangar sobrevive à
/// troca de versão. Senão, a cópia do Hangar acima do binário ou a que o servidor anotou ao subir.
pub fn root(home: &Path) -> Option<PathBuf> {
    let current = current(home);
    if current.exists() { Some(current) } else { checkout(home) }
}

/// O checkout de onde este binário roda, ou o anotado em `~/.hangar/backend-dir` (binário em `~/.hangar/bin`).
fn checkout(home: &Path) -> Option<PathBuf> {
    use crate::session_open::cli::is_backend;
    let exe = std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok());
    let backend = exe.and_then(|exe| exe.ancestors().skip(1).map(|dir| dir.join("backend")).find(|dir| is_backend(dir)))
        .or_else(|| {
            let noted = PathBuf::from(std::fs::read_to_string(crate::session_open::cli::pointer(home)).ok()?.trim());
            is_backend(&noted).then_some(noted)
        })?;
    backend.parent().map(Path::to_path_buf)
}

/// O Python embutido do pacote; sem ele, o venv do backend (checkout ou modo desenvolvedor).
pub fn python_exe(root: &Path, windows: bool) -> PathBuf {
    let venv = root.join("backend").join(".venv");
    let (embedded, venv) = if windows {
        (root.join("python").join("python.exe"), venv.join("Scripts").join("python.exe"))
    } else {
        (root.join("python").join("bin").join("python3"), venv.join("bin").join("python"))
    };
    if embedded.exists() { embedded } else { venv }
}

/// `hangar-server launch …`: `args` sem o nome do programa nem o `launch`.
pub fn launch(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("backend") {
        eprintln!("uso: hangar-server launch backend [ARGS...]");
        return ExitCode::from(2);
    }
    let home = crate::accounts::environment::AccountEnvironment::capture().home;
    match backend_command(&home, &args[1..], cfg!(windows)) {
        Ok(command) => run(command),
        Err((code, path)) => {
            eprintln!("hangar-server launch: {code}: {}", path.display());
            ExitCode::from(3)
        }
    }
}

/// O backend da versão para onde o `current` aponta agora, pelo caminho real: o processo de pé nunca
/// carrega arquivo da versão seguinte depois da troca. O erro leva o caminho que faltou.
pub fn backend_command(home: &Path, args: &[String], windows: bool) -> Result<std::process::Command, (&'static str, PathBuf)> {
    let no_current = || ("launch_no_current", current(home));
    let real = std::fs::canonicalize(current(home)).map_err(|_| no_current())?;
    // O Python não entende o `\\?\` que o `canonicalize` do Windows devolve.
    let real = crate::list::links::without_verbatim(real);
    let version = real.file_name().ok_or_else(no_current)?.to_owned();
    let python = python_exe(&real, windows);
    if !python.is_file() { return Err(("launch_no_python", python)); }
    let mut command = std::process::Command::new(python);
    // Bytecode fora da pasta da versão: ela nunca muda depois de extraída.
    command.args(["-m", "app.main"]).args(args).current_dir(real.join("backend"))
        .env("PYTHONPYCACHEPREFIX", home.join(".hangar").join("cache").join("pycache").join(version));
    Ok(command)
}

/// No Unix o lançador vira o backend: o serviço acompanha o próprio Python.
#[cfg(unix)]
fn run(mut command: std::process::Command) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    eprintln!("hangar-server launch: launch_exec_failed: {error}");
    ExitCode::from(126)
}

/// No Windows não há `exec`: espera o backend e sai com o código dele, os dois no mesmo Job.
#[cfg(windows)]
fn run(mut command: std::process::Command) -> ExitCode {
    // Sem o Job o backend sobe como antes, só sem a proteção: a falha vai ao log da tarefa.
    if let Err(error) = die_with_the_launcher() {
        eprintln!("hangar-server launch: launch_job_failed: {error}");
    }
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("hangar-server launch: launch_exec_failed: {error}");
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
    use super::{current, python_exe, root};

    #[test]
    fn install_root_is_the_unresolved_current_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_ne!(root(home), Some(current(home)), "sem o current a raiz é a cópia do Hangar");
        let real = home.join(".hangar/versions/1.0.0.5");
        std::fs::create_dir_all(&real).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, current(home)).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(current(home)).arg(&real)
            .output().unwrap().status.success(), "junction não criada");
        assert_eq!(root(home), Some(home.join(".hangar").join("current")));
    }

    #[test]
    fn install_root_python_is_the_embedded_one_then_the_venv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(python_exe(root, false), root.join("backend/.venv/bin/python"));
        assert_eq!(python_exe(root, true), root.join("backend").join(".venv").join("Scripts").join("python.exe"));
        for (windows, embedded) in [(false, root.join("python/bin/python3")), (true, root.join("python").join("python.exe"))] {
            std::fs::create_dir_all(embedded.parent().unwrap()).unwrap();
            std::fs::write(&embedded, "").unwrap();
            assert_eq!(python_exe(root, windows), embedded, "windows={windows}");
        }
    }
}
