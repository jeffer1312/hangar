//! Comando com saída ao vivo e prazo único, compartilhado pela atualização e pelos instaladores.
use std::{
    collections::BTreeMap,
    io::Read,
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
    Read {
        source: std::io::Error,
        stdout: String,
    },
    Timeout {
        stdout: String,
    },
}
impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "não foi possível executar o comando: {e}"),
            Self::Read { source, .. } => {
                write!(f, "não foi possível ler a saída do comando: {source}")
            }
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
    run_with_reader(args, cwd, timeout, env, log, |reader| Box::new(reader))
}

/// Captura os dois canais sem misturar diagnósticos ao valor retornado pelo programa.
pub fn run_capture_with_env(args:&[String],cwd:&Path,timeout:Duration,env:&BTreeMap<String,String>)->Result<ProcessOutput,ProcessError>{
    execute(args,cwd,timeout,env,&|_|{},|reader|Box::new(reader),true,false)
}
/// Executa com o retrato completo do ambiente do usuário, sem impor idioma de diagnóstico.
pub fn run_capture_user_environment(args:&[String],cwd:&Path,timeout:Duration,env:&BTreeMap<String,String>)->Result<ProcessOutput,ProcessError>{
    execute(args,cwd,timeout,env,&|_|{},|reader|Box::new(reader),true,true)
}
fn run_with_reader(args:&[String],cwd:&Path,timeout:Duration,env:&BTreeMap<String,String>,log:&dyn Fn(&str),reader:impl FnOnce(std::io::PipeReader)->Box<dyn Read+Send>)->Result<ProcessOutput,ProcessError>{
    execute(args,cwd,timeout,env,log,reader,false,false)
}
fn execute(
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    env: &BTreeMap<String, String>,
    log: &dyn Fn(&str),
    reader: impl FnOnce(std::io::PipeReader) -> Box<dyn Read + Send>,
    capture: bool,
    user_environment: bool,
) -> Result<ProcessOutput, ProcessError> {
    let Some(program) = args.first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "comando vazio").into());
    };
    log(&format!("$ {}", args.join(" ")));
    let (pipe_reader, writer) = std::io::pipe()?;
    let reader = reader(pipe_reader);
    let (error_reader,error_writer)=if capture{let (reader,writer)=std::io::pipe()?;(Some(reader),writer)}else{(None,writer.try_clone()?)};
    let mut command = Command::new(program);
    if user_environment { command.env_clear(); } else { command.env("LC_ALL", "C").env("LANGUAGE", "C"); }
    command
        .args(&args[1..])
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(error_writer);
    drop(writer);
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
        use crate::terminal_process::OwnedTree;
        let _ = tree.terminate();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    drop(command);
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || if capture{read_chunks(reader,send)}else{read_lines(reader, send)});
    let errors=error_reader.map(|mut reader|{
        let (send,receive)=mpsc::channel();
        std::thread::spawn(move||{let mut bytes=Vec::new();let result=reader.read_to_end(&mut bytes).map(|_|bytes);let _=send.send(result);});
        receive
    });
    let mut stdout_bytes=Vec::new();
    let mut stderr_bytes=Vec::new();
    static ANSI: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
    let mut lines = Vec::new();
    let mut pending = Vec::new();
    let mut flushed = Instant::now();
    let mut read_error = None;
    let mut timed_out = loop {
        let now = Instant::now();
        if now >= deadline {
            break true;
        }
        match receive.recv_timeout((deadline - now).min(Duration::from_secs(1))) {
            Ok(Ok(Some(bytes))) => {
                if capture{stdout_bytes.extend(bytes);continue;}
                let text = String::from_utf8_lossy(&bytes);
                let clean = ANSI.replace_all(&text, "");
                let line = clean.trim_end().to_owned();
                if !line.is_empty() {
                    pending.push(line.clone());
                }
                lines.push(line);
            }
            Ok(Ok(None)) => break false,
            Ok(Err(error)) => {
                read_error = Some(error);
                break false;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                read_error = Some(std::io::Error::other("a leitura da saída foi interrompida"));
                break false;
            }
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
    if !timed_out && read_error.is_none() && let Some(errors)=errors{
        match errors.recv_timeout(deadline.saturating_duration_since(Instant::now())){
            Ok(Ok(bytes))=>stderr_bytes=bytes,
            Ok(Err(error))=>read_error=Some(error),
            Err(mpsc::RecvTimeoutError::Timeout)=>timed_out=true,
            Err(mpsc::RecvTimeoutError::Disconnected)=>read_error=Some(std::io::Error::other("a leitura do diagnóstico foi interrompida")),
        }
    }
    let output = if capture{String::from_utf8_lossy(&stdout_bytes).into_owned()}else{lines.join("\n")};
    if timed_out || read_error.is_some() {
        #[cfg(unix)]
        kill_tree(&mut child);
        #[cfg(windows)]
        {
            use crate::terminal_process::OwnedTree;
            tree.terminate()?;
        }
        let _ = child.wait();
        return Err(match read_error {
            Some(source) => ProcessError::Read {
                source,
                stdout: output,
            },
            None => ProcessError::Timeout { stdout: output },
        });
    }
    // O fechamento dos canos inicia a espera final, que sempre dispõe de ao menos um segundo.
    let now = Instant::now();
    let wait_deadline = now
        + deadline
            .saturating_duration_since(now)
            .max(Duration::from_secs(1));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < wait_deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
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
    let decode=|bytes:Vec<u8>|String::from_utf8(bytes).map(|text|text.replace("\r\n","\n").replace('\r',"\n")).map_err(|error|ProcessError::Read{source:std::io::Error::new(std::io::ErrorKind::InvalidData,error),stdout:output.clone()});
    Ok(ProcessOutput {
        code,
        stdout: if capture{decode(stdout_bytes)?}else{output.clone()},
        stderr: if capture{decode(stderr_bytes)?}else{String::new()},
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

fn read_chunks(mut reader:impl Read,send:mpsc::Sender<Result<Option<Vec<u8>>,std::io::Error>>){
    let mut buffer=[0u8;8192];
    loop{match reader.read(&mut buffer){
        Ok(0)=>{let _=send.send(Ok(None));return;},
        Ok(count)=>if send.send(Ok(Some(buffer[..count].to_vec()))).is_err(){return;},
        Err(error) if error.kind()==std::io::ErrorKind::Interrupted=>{},
        Err(error)=>{let _=send.send(Err(error));return;},
    }}
}

fn read_lines(mut reader: impl Read, send: mpsc::Sender<Result<Option<Vec<u8>>, std::io::Error>>) {
    let mut buffer = [0u8; 8192];
    let mut line = Vec::new();
    let mut pending_cr = false;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if pending_cr || !line.is_empty() {
                    let _ = send.send(Ok(Some(line)));
                }
                let _ = send.send(Err(error));
                return;
            }
            Ok(count) => count,
        };
        for &byte in &buffer[..count] {
            if pending_cr {
                pending_cr = false;
                if send.send(Ok(Some(std::mem::take(&mut line)))).is_err() {
                    return;
                }
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => pending_cr = true,
                b'\n' => {
                    if send.send(Ok(Some(std::mem::take(&mut line)))).is_err() {
                        return;
                    }
                }
                byte => line.push(byte),
            }
        }
    }
    if (pending_cr || !line.is_empty()) && send.send(Ok(Some(line))).is_err() {
        return;
    }
    let _ = send.send(Ok(None));
}

#[cfg(test)]
mod read_error_tests {
    use super::*;
    struct Broken {
        first: bool,
    }
    impl Read for Broken {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if self.first {
                self.first = false;
                bytes[..8].copy_from_slice(b"partial\n");
                Ok(8)
            } else {
                Err(std::io::Error::other("fixture-read-failed"))
            }
        }
    }
    #[test]
    fn process_reader_failure_is_not_successful_eof() {
        let (send, receive) = mpsc::channel();
        read_lines(Broken { first: true }, send);
        assert_eq!(receive.recv().unwrap().unwrap(), Some(b"partial".to_vec()));
        assert!(
            receive.recv().unwrap().is_err(),
            "PEE072: saída parcial não comprova leitura completa"
        );
    }
    #[test]
    fn process_read_error_returns_partial_output_and_stops_its_child() {
        let args = vec![
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            "update::process::read_error_tests::read_error_child".into(),
            "--ignored".into(),
        ];
        let temp = tempfile::tempdir().unwrap();
        let began = Instant::now();
        let result = run_with_reader(
            &args,
            temp.path(),
            Duration::from_secs(20),
            &BTreeMap::new(),
            &|_| {},
            |_| Box::new(Broken { first: true }),
        );
        match result {
            Err(ProcessError::Read { source, stdout }) => {
                assert_eq!(source.to_string(), "fixture-read-failed");
                assert_eq!(stdout, "partial");
            }
            other => panic!("erro de leitura não propagado: {other:?}"),
        }
        assert!(began.elapsed() < Duration::from_secs(5));
    }
    #[test]
    #[ignore]
    fn read_error_child() {
        std::thread::sleep(Duration::from_secs(60));
    }
}
