//! Prazos incluem a drenagem dos pipes; um descendente não pode prender o pedido.
use crate::{Result, error};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
impl Output {
    pub fn both(&self) -> String {
        format!("{}{}", self.stdout, self.stderr).trim().into()
    }
    pub fn reason(&self, fallback: &str) -> String {
        if self.stderr.trim().is_empty() {
            fallback.into()
        } else {
            self.stderr.trim().into()
        }
    }
}

pub fn run(cwd: &Path, args: &[&str], timeout: Duration) -> Result<Output> {
    run_program(Command::new("git").arg("-C").arg(cwd).args(args), timeout)
}

pub fn run_program(command: &mut Command, timeout: Duration) -> Result<Output> {
    let guard = crate::process_lifetime::Guard::new()
        .map_err(|_| error(500, "não foi possível proteger a árvore do comando"))?;
    guard.configure(command);
    let mut child = command
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            error(
                500,
                if e.kind() == std::io::ErrorKind::NotFound {
                    "git não encontrado".into()
                } else {
                    format!("git falhou: {e}")
                },
            )
        })?;
    if guard.attach(&child).is_err() {
        guard.kill();
        kill_tree(&mut child);
        return Err(error(500, "não foi possível proteger a árvore do comando"));
    }
    let (tx, rx) = mpsc::channel();
    let drain = |mut pipe: Box<dyn Read + Send>, which: bool, tx: mpsc::Sender<_>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((which, result));
        });
    };
    drain(Box::new(child.stdout.take().unwrap()), true, tx.clone());
    drain(Box::new(child.stderr.take().unwrap()), false, tx);
    let start = Instant::now();
    let mut stdout = None;
    let mut stderr = None;
    let status = loop {
        while let Ok((which, result)) = rx.try_recv() {
            let bytes = match result {
                Ok(b) => b,
                Err(_) => {
                    guard.kill();
                    kill_tree(&mut child);
                    return Err(error(500, "git falhou ao ler a saída"));
                }
            };
            if which {
                stdout = Some(
                    String::from_utf8_lossy(&bytes)
                        .replace("\r\n", "\n")
                        .replace('\r', "\n"),
                );
            } else {
                stderr = Some(
                    String::from_utf8_lossy(&bytes)
                        .replace("\r\n", "\n")
                        .replace('\r', "\n"),
                );
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| error(500, "git falhou ao aguardar o processo"))?
            && stdout.is_some()
            && stderr.is_some()
        {
            break status;
        }
        if start.elapsed() >= timeout {
            guard.kill();
            kill_tree(&mut child);
            return Err(error(504, "git timeout"));
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    Ok(Output {
        code: status.code().unwrap_or(-1),
        stdout: stdout.unwrap(),
        stderr: stderr.unwrap(),
    })
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                    TH32CS_SNAPPROCESS,
                },
                Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
            },
        };
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != INVALID_HANDLE_VALUE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut processes = Vec::new();
            let mut valid = Process32FirstW(snapshot, &mut entry);
            while valid != 0 {
                processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
                valid = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            let mut tree = vec![child.id()];
            loop {
                let before = tree.len();
                for (pid, parent) in &processes {
                    if tree.contains(parent) && !tree.contains(pid) {
                        tree.push(*pid);
                    }
                }
                if tree.len() == before {
                    break;
                }
            }
            for pid in tree.into_iter().rev() {
                let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                if !handle.is_null() {
                    TerminateProcess(handle, 1);
                    CloseHandle(handle);
                }
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn inherited_pipe_does_not_outlive_the_deadline() {
        let start = Instant::now();
        let result = run_program(
            Command::new("sh").args(["-c", "sleep 20 & wait"]),
            Duration::from_millis(60),
        );
        assert_eq!(result.err().unwrap().status, 504);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
