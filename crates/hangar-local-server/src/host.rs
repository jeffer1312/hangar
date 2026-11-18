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
/// Espera mínima pelo fim da saída depois que o filho terminou.
const PIPE_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// `None`: morto pelo prazo ou por sinal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Morto pelo prazo do `run`, ou com a saída ainda aberta nele (vem a parcial): o comando não
    /// terminou, o que não quer dizer que falhou.
    pub timed_out: bool,
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
    /// Pede a saída do processo (`SIGTERM`) ou, com `force`, encerra (`SIGKILL`, ou o
    /// `TerminateProcess` do Windows); `true` quando o sistema aceitou.
    fn signal(&self, pid: u32, force: bool) -> bool;
    fn pause(&self, duration: Duration);
    /// Segundos desde a época, na mesma escala de `ProcessInfo::start_time`.
    fn now(&self) -> u64;
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
        let stdout = Drain::start(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = Drain::start(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let (code, timed_out) = loop {
            if let Some(status) = child.try_wait()? { break (status.code(), false); }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        // Um descendente que herdou o pipe o mantém aberto depois do filho sair: a leitura tem prazo próprio.
        let until = deadline.max(Instant::now() + PIPE_GRACE);
        let (stdout, stdout_cut) = stdout.finish(until);
        let (stderr, stderr_cut) = stderr.finish(until);
        Ok(Output { code, stdout, stderr, timed_out: timed_out || stdout_cut || stderr_cut })
    }

    fn get(&self, port: u16, path: &str) -> Option<Reply> {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = std::net::TcpStream::connect_timeout(&address, CONNECT_DEADLINE).ok()?;
        let _ = stream.set_read_timeout(Some(HTTP_DEADLINE));
        let _ = stream.set_write_timeout(Some(HTTP_DEADLINE));
        // HTTP/1.0: corpo inteiro até o fim da conexão, sem `chunked`.
        let request = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        if stream.write_all(request.as_bytes()).is_err() { return Some(Reply { status: 0, body: String::new() }); }
        let bytes = read_capped(&mut stream, Instant::now() + HTTP_DEADLINE);
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

    fn signal(&self, pid: u32, force: bool) -> bool {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), true, sysinfo::ProcessRefreshKind::nothing());
        let Some(process) = system.process(pid) else { return false };
        if force { return process.kill(); }
        // O Windows não tem SIGTERM e encerra direto.
        process.kill_with(sysinfo::Signal::Term).unwrap_or_else(|| process.kill())
    }

    fn pause(&self, duration: Duration) { std::thread::sleep(duration); }

    fn now(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
}

/// Leitura de um pipe numa thread, guardando o que já chegou para a saída parcial.
pub(crate) struct Drain {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    pub(crate) fn start(pipe: Option<Box<dyn Read + Send>>) -> Self {
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, done) = std::sync::mpsc::channel();
        let shared = bytes.clone();
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 { break; }
                    if let Ok(mut bytes) = shared.lock() { bytes.extend_from_slice(&chunk[..n]); }
                }
            }
            let _ = tx.send(());
        });
        Self { bytes, done }
    }

    /// O texto lido até `until`; `true` quando o pipe ainda não tinha fechado.
    pub(crate) fn finish(self, until: Instant) -> (String, bool) {
        let cut = self.done.recv_timeout(until.saturating_duration_since(Instant::now())).is_err();
        let text = self.bytes.lock().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        (text, cut)
    }
}

/// Lê até o fim, `HTTP_MAX` bytes ou `deadline`, o que vier antes: o prazo por leitura sozinho deixaria
/// quem manda um byte de cada vez segurar a consulta por horas.
pub(crate) fn read_capped(stream: &mut std::net::TcpStream, deadline: Instant) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    while (bytes.len() as u64) < HTTP_MAX {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() { break; }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = HTTP_MAX as usize - bytes.len();
                bytes.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    bytes
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Um neto que herdou o pipe o mantém aberto: a leitura volta no prazo com o que já chegou.
    #[cfg(unix)]
    #[test]
    fn output_of_a_child_whose_grandchild_keeps_the_pipe_returns_by_the_deadline() {
        let mut child = std::process::Command::new("sh").args(["-c", "sleep 30 & echo oi"])
            .stdout(std::process::Stdio::piped()).spawn().unwrap();
        let drain = Drain::start(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        child.wait().unwrap();
        let begin = Instant::now();
        let (text, cut) = drain.finish(Instant::now() + Duration::from_millis(300));
        assert!(begin.elapsed() < Duration::from_secs(5));
        assert_eq!((text.as_str(), cut), ("oi\n", true));
    }

    /// Quem manda um byte de cada vez não segura a consulta além do prazo total.
    #[test]
    fn a_trickling_reply_stops_at_the_total_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            while peer.write_all(b"x").is_ok() { std::thread::sleep(Duration::from_millis(20)); }
        });
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        let begin = Instant::now();
        let bytes = read_capped(&mut stream, Instant::now() + Duration::from_millis(300));
        assert!(begin.elapsed() < Duration::from_secs(2), "{:?}", begin.elapsed());
        assert!(!bytes.is_empty());
    }
}
