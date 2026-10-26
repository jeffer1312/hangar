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
    checkout_from(std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok()), home)
}

/// Cópia acima do binário com `.env` vence a anotada; sem `.env` (worktree de desenvolvimento) vale a
/// anotada, que é a do servidor e tem o token; sem nenhuma das duas, a cópia acima do binário.
fn checkout_from(exe: Option<PathBuf>, home: &Path) -> Option<PathBuf> {
    use crate::session_open::cli::is_backend;
    let above = |found: fn(&Path) -> bool| exe.as_deref()
        .and_then(|exe| exe.ancestors().skip(1).map(|dir| dir.join("backend")).find(|dir| found(dir)));
    let backend = above(|dir| dir.join(".env").is_file())
        .or_else(|| {
            let noted = PathBuf::from(std::fs::read_to_string(crate::session_open::cli::pointer(home)).ok()?.trim());
            is_backend(&noted).then_some(noted)
        })
        .or_else(|| above(is_backend))?;
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
    let log = args.iter().position(|a| a == "--log").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    match backend_command(&home, &args[1..], cfg!(windows)) {
        Ok(command) => run(command, log.as_deref()),
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

/// O backend da versão para onde o `current` aponta agora, pelo caminho real: o processo de pé nunca
/// carrega arquivo da versão seguinte depois da troca. O erro leva o caminho que faltou.
pub fn backend_command(home: &Path, args: &[String], windows: bool) -> Result<std::process::Command, (&'static str, PathBuf)> {
    let no_current = || ("launch_no_current", current(home));
    let real = std::fs::canonicalize(current(home)).map_err(|_| no_current())?;
    // O Python não entende o `\\?\` que o `canonicalize` do Windows devolve.
    let real = crate::list::links::without_verbatim(real);
    let python = python_exe(&real, windows);
    if !python.is_file() { return Err(("launch_no_python", python)); }
    let mut command = std::process::Command::new(python);
    // Sem `PYTHONPYCACHEPREFIX`: o pacote leva os `.pyc` em `unchecked-hash`, que o Python só lê, e o
    // prefixo os faria ignorar e recompilar tudo fora da versão.
    command.args(["-m", "app.main"]).args(args).current_dir(real.join("backend"));
    Ok(command)
}

/// No Unix o lançador vira o backend: o serviço acompanha o próprio Python.
#[cfg(unix)]
fn run(mut command: std::process::Command, log: Option<&Path>) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    report(log, &format!("launch_exec_failed: {error}"));
    ExitCode::from(126)
}

/// No Windows não há `exec`: espera o backend e sai com o código dele, os dois no mesmo Job.
#[cfg(windows)]
fn run(mut command: std::process::Command, log: Option<&Path>) -> ExitCode {
    // Sem o Job o backend sobe como antes, só sem a proteção: a falha vai ao log da tarefa.
    if let Err(error) = die_with_the_launcher() {
        report(log, &format!("launch_job_failed: {error}"));
    }
    match command.status() {
        Ok(status) => {
            if !status.success() { report(log, &format!("launch_backend_exited: {status}")); }
            std::process::exit(status.code().unwrap_or(1))
        }
        Err(error) => {
            report(log, &format!("launch_exec_failed: {error}"));
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
    use super::{checkout_from, current, python_exe, root};

    /// Binário de uma worktree sem `.env` (máquina não migrada): vale a cópia que o servidor anotou,
    /// que tem o token, e não a worktree.
    #[test]
    fn a_worktree_binary_without_env_uses_the_noted_backend() {
        let dir = tempfile::tempdir().unwrap();
        let (home, worktree, main) = (dir.path().join("home"), dir.path().join("wt"), dir.path().join("main"));
        let exe = worktree.join("crates/target/release/hangar-server");
        std::fs::create_dir_all(worktree.join("backend/app")).unwrap();
        std::fs::write(worktree.join("backend/app/rust_server.py"), "").unwrap();
        std::fs::create_dir_all(main.join("backend")).unwrap();
        std::fs::write(main.join("backend/.env"), "CP_AUTH_TOKEN=t\n").unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::fs::write(crate::session_open::cli::pointer(&home), main.join("backend").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(checkout_from(Some(exe.clone()), &home), Some(main.clone()));
        // A worktree com `.env` próprio é a do binário, como antes.
        std::fs::write(worktree.join("backend/.env"), "CP_AUTH_TOKEN=w\n").unwrap();
        assert_eq!(checkout_from(Some(exe.clone()), &home), Some(worktree.clone()));
        // Sem nada anotado, a cópia acima do binário ainda vale sem `.env`.
        std::fs::remove_file(worktree.join("backend/.env")).unwrap();
        std::fs::remove_file(crate::session_open::cli::pointer(&home)).unwrap();
        assert_eq!(checkout_from(Some(exe), &home), Some(worktree));
    }

    /// O pacote leva os `.pyc` prontos: com `PYTHONPYCACHEPREFIX` o Python os ignora e recompila tudo.
    #[cfg(unix)]
    #[test]
    fn the_launcher_uses_the_shipped_bytecode() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = home.join(".hangar/versions/1.0.0.5");
        std::fs::create_dir_all(real.join("python/bin")).unwrap();
        std::fs::write(real.join("python/bin/python3"), "").unwrap();
        std::os::unix::fs::symlink(&real, current(home)).unwrap();
        let command = super::backend_command(home, &[], false).unwrap();
        assert!(command.get_envs().all(|(key, _)| key != "PYTHONPYCACHEPREFIX"), "{command:?}");
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
