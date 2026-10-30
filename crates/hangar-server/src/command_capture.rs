//! Captura separada dos dois canais; timeout e cancelamento encerram só a árvore criada.
use crate::{
    accounts::environment::AccountEnvironment,
    terminal_process::{self, CommandTree, OwnedTree},
};
use std::{io, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

fn error_name(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    }
    .into()
}

#[derive(Clone, Debug)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn capture(
    args: Vec<String>,
    env: AccountEnvironment,
    budget: Duration,
) -> Result<Output, String> {
    capture_inner(args, env, budget, false).await
}

/// Preserva a conta selecionada sem transmitir os segredos internos do servidor.
pub async fn capture_with_account_env(
    args: Vec<String>,
    env: AccountEnvironment,
    budget: Duration,
) -> Result<Output, String> {
    capture_inner(args, env, budget, true).await
}

async fn capture_inner(
    args: Vec<String>,
    env: AccountEnvironment,
    budget: Duration,
    preserve_account: bool,
) -> Result<Output, String> {
    let output = capture_bytes(
        args,
        env,
        budget,
        CaptureOptions {
            preserve_account,
            ..Default::default()
        },
    )
    .await
    .map_err(|error| error.kind)?;
    let decode = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n")
    };
    Ok(Output {
        success: output.success,
        stdout: decode(&output.stdout),
        stderr: decode(&output.stderr),
    })
}

pub async fn capture_bytes(
    args: Vec<String>,
    env: AccountEnvironment,
    budget: Duration,
    options: CaptureOptions,
) -> Result<ByteOutput, CaptureFailure> {
    let Some(program) = args.first() else {
        return Err(CaptureFailure::empty("FileNotFoundError"));
    };
    let mut command = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().arg0(program);
    }
    command
        .args(&args[1..])
        .env_clear()
        .envs(env.base.iter().filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            (options.preserve_account || key != "CLAUDE_CONFIG_DIR")
                && !terminal_process::PRIVATE_ENV_KEYS.contains(&key.as_str())
        }))
        .stdin(if options.inherit_stdin {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    let mut tree = match CommandTree::configure_detached(&mut command) {
        Ok(tree) => tree,
        Err(error) => return Err(CaptureFailure::empty(error_name(&error))),
    };
    #[cfg(unix)]
    {
        let exec = match StrictExec::new(program, &command, &env) {
            Ok(exec) => exec,
            Err(error) => return Err(CaptureFailure::empty(error_name(&error))),
        };
        // O filho já recebeu cwd, pipes e grupo; aqui só usa buffers preparados antes do fork.
        unsafe {
            command.pre_exec(move || exec.execute());
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Err(CaptureFailure::empty(error_name(&error))),
    };
    if let Err(error) = tree.attach(&child) {
        let _ = tree.terminate();
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(CaptureFailure::empty(error_name(&error)));
    }
    let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
    // O supervisor da captura conserva processo e pipes mesmo se o pedido for cancelado.
    let task = tokio::spawn(async move {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let outcome = {
            let collect = async {
                let (a, b, c) = tokio::join!(
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err),
                    terminal_process::leader_exited(&mut child)
                );
                a?;
                b?;
                c?;
                Ok::<(), io::Error>(())
            };
            tokio::select! {
                result = tokio::time::timeout(budget,collect) => match result {
                    Ok(Ok(())) => Ok(()), Ok(Err(error)) => Err(error_name(&error)), Err(_) => Err("timeout".into()),
                },
                _ = cancelled => Err("interrompido".into()),
            }
        };
        if outcome.is_err() {
            terminal_process::finish(&mut tree).await;
        }
        let status = child.wait().await;
        let status = match outcome.and_then(|()| status.map_err(|error| error_name(&error))) {
            Ok(status) => status,
            Err(kind) => {
                return Err(CaptureFailure {
                    kind,
                    stdout: out,
                    stderr: err,
                });
            }
        };
        Ok(ByteOutput {
            success: status.success(),
            stdout: out,
            stderr: err,
        })
    });
    let result = task
        .await
        .unwrap_or_else(|_| Err(CaptureFailure::empty("falha ao acompanhar o comando")));
    drop(cancel);
    result
}

pub async fn run(args: Vec<String>, env: AccountEnvironment, budget: Duration) -> (bool, String) {
    match capture(args, env, budget).await {
        Err(error) => (false, error),
        Ok(out) => {
            let text = if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            };
            let text = text.trim();
            let skip = text.chars().count().saturating_sub(300);
            (out.success, text.chars().skip(skip).collect())
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CaptureOptions {
    pub cwd: Option<std::path::PathBuf>,
    pub preserve_account: bool,
    pub inherit_stdin: bool,
}
#[derive(Clone, Debug)]
pub struct ByteOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct CaptureFailure {
    pub kind: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl CaptureFailure {
    fn empty(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            stdout: vec![],
            stderr: vec![],
        }
    }
}

#[cfg(unix)]
struct StrictExec {
    paths: Vec<std::ffi::CString>,
    _args: Vec<std::ffi::CString>,
    _env: Vec<std::ffi::CString>,
    argv: Vec<*const libc::c_char>,
    envp: Vec<*const libc::c_char>,
}
// Os ponteiros só leem os buffers CString possuídos e imutáveis desta estrutura.
#[cfg(unix)]
unsafe impl Send for StrictExec {}
#[cfg(unix)]
unsafe impl Sync for StrictExec {}
#[cfg(unix)]
impl StrictExec {
    fn new(program: &str, command: &Command, env: &AccountEnvironment) -> io::Result<Self> {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, path::PathBuf};
        let string = |bytes: &[u8]| {
            CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
        };
        let paths = if program.contains('/') {
            vec![PathBuf::from(program)]
        } else {
            std::env::split_paths(
                env.base
                    .get("PATH")
                    .map(String::as_str)
                    .unwrap_or("/bin:/usr/bin"),
            )
            .map(|directory| directory.join(program))
            .collect()
        }
        .iter()
        .map(|path| string(path.as_os_str().as_bytes()))
        .collect::<io::Result<Vec<_>>>()?;
        let args = std::iter::once(std::ffi::OsStr::new(program))
            .chain(command.as_std().get_args())
            .map(|arg| string(arg.as_bytes()))
            .collect::<io::Result<Vec<_>>>()?;
        let environment = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .map(|(key, value)| {
                let mut bytes = key.as_bytes().to_vec();
                bytes.push(b'=');
                bytes.extend_from_slice(value.as_bytes());
                string(&bytes)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let pointers = |strings: &Vec<CString>| {
            strings
                .iter()
                .map(|value| value.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect()
        };
        let argv = pointers(&args);
        let envp = pointers(&environment);
        Ok(Self {
            paths,
            _args: args,
            _env: environment,
            argv,
            envp,
        })
    }
    fn execute(&self) -> io::Result<()> {
        let mut saved = None;
        let mut last = libc::ENOENT;
        for path in &self.paths {
            unsafe {
                libc::execve(path.as_ptr(), self.argv.as_ptr(), self.envp.as_ptr());
            }
            last = io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
            if saved.is_none() && last != libc::ENOENT && last != libc::ENOTDIR {
                saved = Some(last);
            }
        }
        Err(io::Error::from_raw_os_error(saved.unwrap_or(last)))
    }
}
