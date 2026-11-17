//! Tudo o que sai do processo (comandos, HTTP, lista de processos, sinal) passa por `Host`: os
//! testes trocam por um dublê e nada da máquina de quem roda o teste é lido ou encerrado.
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// O processo filho nasce sem console: o nativo é um app de janela, e o PowerShell abriria uma.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Teto de um comando: `systemctl`, PowerShell e `journalctl` respondem em segundos.
const COMMAND_DEADLINE: Duration = Duration::from_secs(20);
const CONNECT_DEADLINE: Duration = Duration::from_millis(1500);
const HTTP_DEADLINE: Duration = Duration::from_secs(3);
const HTTP_MAX: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// `None`: morto pelo prazo ou por sinal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// 0 quando quem respondeu não fala HTTP.
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub cmd: Vec<String>,
    /// Uid no Unix, SID no Windows; `None` quando o sistema não deixa ler (outro dono).
    pub user: Option<String>,
    /// Segundos desde a época (inteiros também no Windows).
    pub start_time: u64,
}

pub trait Host {
    /// Roda `program` sem janela e com prazo; `Err` quando ele nem abriu.
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output>;
    /// `GET http://127.0.0.1:<port><path>`; `None` quando ninguém aceitou a conexão.
    fn get(&self, port: u16, path: &str) -> Option<Reply>;
    fn processes(&self) -> Vec<ProcessInfo>;
    fn process(&self, pid: u32) -> Option<ProcessInfo>;
    /// O usuário deste processo, no formato de `ProcessInfo::user`.
    fn user(&self) -> Option<String>;
    /// Encerra o processo; `true` quando o sistema aceitou.
    fn terminate(&self, pid: u32) -> bool;
    fn pause(&self, duration: Duration);
}

/// A máquina de verdade.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealHost;

impl Host for RealHost {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<Output> {
        let mut command = std::process::Command::new(program);
        command.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; command.creation_flags(CREATE_NO_WINDOW); }
        let mut child = command.spawn()?;
        // Lidos em paralelo: um pipe cheio travaria o filho antes de ele sair.
        let drain = |pipe: Option<Box<dyn Read + Send>>| std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe { let _ = pipe.read_to_end(&mut bytes); }
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let code = loop {
            if let Some(status) = child.try_wait()? { break status.code(); }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        Ok(Output { code, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
    }

    fn get(&self, port: u16, path: &str) -> Option<Reply> {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = std::net::TcpStream::connect_timeout(&address, CONNECT_DEADLINE).ok()?;
        let _ = stream.set_read_timeout(Some(HTTP_DEADLINE));
        let _ = stream.set_write_timeout(Some(HTTP_DEADLINE));
        // HTTP/1.0: corpo inteiro até o fim da conexão, sem `chunked`.
        let request = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        if stream.write_all(request.as_bytes()).is_err() { return Some(Reply { status: 0, body: String::new() }); }
        let mut bytes = Vec::new();
        let _ = stream.take(HTTP_MAX).read_to_end(&mut bytes);
        Some(parse_reply(&String::from_utf8_lossy(&bytes)))
    }

    fn processes(&self) -> Vec<ProcessInfo> {
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind());
        system.processes().values().map(info).collect()
    }

    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, refresh_kind());
        system.process(pid).map(info)
    }

    fn user(&self) -> Option<String> { self.process(std::process::id())?.user }

    fn terminate(&self, pid: u32) -> bool {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
        let Some(process) = system.process(pid) else { return false };
        // SIGTERM deixa o servidor gravar a parada; o Windows não tem sinal e encerra direto.
        process.kill_with(sysinfo::Signal::Term).unwrap_or_else(|| process.kill())
    }

    fn pause(&self, duration: Duration) { std::thread::sleep(duration); }
}

fn refresh_kind() -> sysinfo::ProcessRefreshKind {
    sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always).with_user(sysinfo::UpdateKind::Always)
}

fn info(process: &sysinfo::Process) -> ProcessInfo {
    ProcessInfo {
        pid: process.pid().as_u32(),
        parent: process.parent().map(|p| p.as_u32()),
        name: process.name().to_string_lossy().into_owned(),
        cmd: process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(),
        user: process.user_id().map(|uid| (**uid).to_string()),
        start_time: process.start_time(),
    }
}

/// Status e corpo de uma resposta HTTP/1.x; o que não começa com `HTTP/` é status 0.
pub fn parse_reply(text: &str) -> Reply {
    let status = text.strip_prefix("HTTP/").and_then(|rest| rest.split_whitespace().nth(1)).and_then(|code| code.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body).to_owned();
    Reply { status, body }
}
