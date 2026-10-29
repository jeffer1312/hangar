//! Comando com saída ao vivo e prazo único, compartilhado pela atualização e pelos instaladores.
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{LazyLock, mpsc},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct ProcessOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
#[derive(Debug)]
pub enum ProcessError {
    Spawn(std::io::Error),
    Timeout { stdout: String },
}
impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "não foi possível executar o comando: {e}"),
            Self::Timeout { .. } => write!(f, "o comando ultrapassou o prazo"),
        }
    }
}
impl std::error::Error for ProcessError {}
impl From<std::io::Error> for ProcessError {
    fn from(error: std::io::Error) -> Self {
        Self::Spawn(error)
    }
}

pub fn run(
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    log: &dyn Fn(&str),
) -> Result<ProcessOutput, ProcessError> {
    run_with_env(args, cwd, timeout, &BTreeMap::new(), log)
}
pub fn run_with_env(
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    env: &BTreeMap<String, String>,
    log: &dyn Fn(&str),
) -> Result<ProcessOutput, ProcessError> {
    let Some(program) = args.first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "comando vazio").into());
    };
    log(&format!("$ {}", args.join(" ")));
    let (reader, writer) = std::io::pipe()?;
    let mut command = Command::new(program);
    command
        .args(&args[1..])
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .envs(env)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    let mut tree = crate::terminal_process::CommandTree::configure_std(&mut command)?;
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn()?;
    #[cfg(windows)]
    if let Err(error) = tree.attach_std(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    drop(command);
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            let mut bytes = Vec::new();
            match reader.read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if send.send(Some(bytes)).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = send.send(None);
    });
    static ANSI: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
    let mut lines = Vec::new();
    let mut pending = Vec::new();
    let mut flushed = Instant::now();
    let timed_out = loop {
        let now = Instant::now();
        if now >= deadline {
            break true;
        }
        match receive.recv_timeout((deadline - now).min(Duration::from_secs(1))) {
            Ok(Some(bytes)) => {
                let text = String::from_utf8_lossy(&bytes);
                let clean = ANSI.replace_all(&text, "");
                let line = clean.trim_end().to_owned();
                if !line.is_empty() {
                    pending.push(line.clone());
                }
                lines.push(line);
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !pending.is_empty() && flushed.elapsed() >= Duration::from_secs(1) {
            log(&pending.join("\n"));
            pending.clear();
            flushed = Instant::now();
        }
    };
    if !pending.is_empty() {
        log(&pending.join("\n"));
    }
    let output = lines.join("\n");
    if timed_out {
        #[cfg(unix)]
        kill_tree(&mut child);
        #[cfg(windows)]
        {
            use crate::terminal_process::OwnedTree;
            tree.terminate()?;
        }
        let _ = child.wait();
        return Err(ProcessError::Timeout { stdout: output });
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                #[cfg(unix)]
        kill_tree(&mut child);
        #[cfg(windows)]
        {
            use crate::terminal_process::OwnedTree;
            tree.terminate()?;
        }
                let _ = child.wait();
                return Err(ProcessError::Timeout { stdout: output });
            }
        }
    };
    let code = exit_code(status);
    if code != 0 {
        log(&format!("[saiu com {code}]"));
    }
    Ok(ProcessOutput {
        code,
        stdout: output,
        stderr: String::new(),
    })
}
#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(1))
}
#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
#[cfg(unix)]
fn kill_tree(child: &mut std::process::Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}
