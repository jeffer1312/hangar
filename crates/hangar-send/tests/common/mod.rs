//! Arnês dos testes do `hangar-send`: servidores HTTP falsos, o `Host` falso (livro de peers,
//! `conta --prep` e tmux) e a chamada de `run` com o ambiente de um caso.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_send::{Captured, Host, Invocation};
use serde_json::Value;

/// Uma resposta da rota: corpo cru (o JSON já vem serializado como o Python mandava).
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: f64,
}

#[derive(Clone, Debug)]
pub struct Route {
    pub method: String,
    pub prefix: String,
    pub replies: Vec<Reply>,
}

impl Route {
    pub fn new(method: &str, prefix: &str, status: u16, body: &str) -> Self {
        Self { method: method.into(), prefix: prefix.into(), replies: vec![Reply { status, body: body.into(), delay: 0.0 }] }
    }
}

/// Um pedido como o servidor falso o viu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

/// Servidor HTTP/1.1 falso: a primeira rota com método e prefixo que batem responde; as respostas
/// saem em ordem e a última se repete, como o `FakeHTTP` da captura.
pub struct FakeServer {
    pub port: u16,
    pub log: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl FakeServer {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("porta local");
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let table = Arc::new(Mutex::new(routes.into_iter().map(|route| (route, 0usize)).collect::<Vec<_>>()));
        let (log_c, stop_c) = (log.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_c.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let (log, table) = (log_c.clone(), table.clone());
                std::thread::spawn(move || serve(stream, &log, &table));
            }
        });
        Self { port, log, stop }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        // Acorda o `accept` para a thread ver o pedido de parada.
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<Request>>, table: &Mutex<Vec<(Route, usize)>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_owned();
    let path = first.next().unwrap_or("").to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':').map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned())))
        .collect();
    let length = headers.iter().find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok()).unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(length);
    log.lock().unwrap().push(Request { method: method.clone(), path: path.clone(), headers, body });
    let reply = {
        let mut table = table.lock().unwrap();
        table.iter_mut()
            .find(|(route, _)| (route.method == "*" || route.method == method) && path.starts_with(&route.prefix))
            .map(|(route, used)| {
                let reply = route.replies[(*used).min(route.replies.len() - 1)].clone();
                *used += 1;
                reply
            })
            .unwrap_or(Reply { status: 404, body: r#"{"detail": "no route in fake"}"#.into(), delay: 0.0 })
    };
    if reply.delay > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(reply.delay));
    }
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
    let _ = stream.flush();
}

/// Uma porta que recusa conexão (o servidor "fora do ar").
pub fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Resposta de um programa falso: `stdout`, `stderr`, `rc`, `rcs` (um código por chamada).
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub stdout: String,
    pub stderr: String,
    pub rc: i32,
    pub rcs: Vec<i32>,
}

impl Answer {
    pub fn from_json(value: &Value, fill: &dyn Fn(&str) -> String) -> Self {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(fill).unwrap_or_default();
        Self {
            stdout: text("stdout"),
            stderr: text("stderr"),
            rc: value.get("rc").and_then(Value::as_i64).unwrap_or(0) as i32,
            rcs: value.get("rcs").and_then(Value::as_array)
                .map(|codes| codes.iter().filter_map(Value::as_i64).map(|code| code as i32).collect())
                .unwrap_or_default(),
        }
    }
}

/// O `Host` dos testes: o `hangar-server` falso da captura (casa pelos argumentos inteiros, pelos
/// dois primeiros, pelo primeiro ou `*`, com `{i}` trocado pelo argumento i) e o stub do tmux (casa
/// pelo subcomando, senão `*`).
#[derive(Default)]
pub struct FakeHost {
    pub server: Option<BTreeMap<String, Answer>>,
    pub tmux: BTreeMap<String, Answer>,
    pub calls: Mutex<Vec<Vec<String>>>,
    counters: Mutex<BTreeMap<String, usize>>,
}

impl FakeHost {
    pub fn new(server: Option<BTreeMap<String, Answer>>, tmux: BTreeMap<String, Answer>) -> Self {
        Self { server, tmux, ..Self::default() }
    }

    fn hangar_server(&self, args: &[&str]) -> Captured {
        self.calls.lock().unwrap().push(args.iter().map(|arg| (*arg).to_owned()).collect());
        let Some(table) = &self.server else {
            return Captured { code: 127, stdout: Vec::new(), stderr: b"sem hangar-server neste caso\n".to_vec() };
        };
        let keys = [args.join(" "), args[..args.len().min(2)].join(" "), args.first().copied().unwrap_or("").to_owned(), "*".into()];
        let Some(answer) = keys.iter().find_map(|key| table.get(key)) else {
            return Captured { code: 1, stdout: Vec::new(), stderr: b"capture fake hangar-server: not configured for this case\n".to_vec() };
        };
        let fill = |text: &str| {
            args.iter().enumerate().fold(text.to_owned(), |text, (index, arg)| text.replace(&format!("{{{index}}}"), arg))
        };
        Captured { code: answer.rc, stdout: fill(&answer.stdout).into_bytes(), stderr: fill(&answer.stderr).into_bytes() }
    }
}

impl Host for FakeHost {
    fn peers(&self, args: &[&str]) -> Captured {
        let mut full = vec!["peers"];
        full.extend_from_slice(args);
        self.hangar_server(&full)
    }

    fn account_prep(&self, name: &str) -> Captured {
        self.hangar_server(&["conta", "--prep", name])
    }

    fn tmux(&self, args: &[&str]) -> Option<Captured> {
        let sub = args.first().copied().unwrap_or("");
        self.calls.lock().unwrap().push(std::iter::once("tmux").chain(args.iter().copied()).map(str::to_owned).collect());
        let answer = self.tmux.get(sub).or_else(|| self.tmux.get("*")).cloned().unwrap_or_default();
        let call = {
            let mut counters = self.counters.lock().unwrap();
            let count = counters.entry(sub.to_owned()).or_insert(0);
            *count += 1;
            *count
        };
        let code = if answer.rcs.is_empty() { answer.rc } else { answer.rcs[(call - 1).min(answer.rcs.len() - 1)] };
        Some(Captured { code, stdout: answer.stdout.into_bytes(), stderr: answer.stderr.into_bytes() })
    }
}

/// Saída de uma chamada de `run`.
pub struct Outcome {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

pub fn invoke(args: &[String], root: Option<PathBuf>, home: &Path, vars: BTreeMap<String, String>, host: &dyn Host) -> Outcome {
    // O caso roda no processo do teste: a pasta "real" de quem chama é o PWD do caso.
    let cwd = vars.get("PWD").map(PathBuf::from).unwrap_or_default();
    let invocation = Invocation { args: args.to_vec(), root, home: home.to_path_buf(), vars, cwd };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = hangar_send::run(&invocation, host, &mut out, &mut err);
    Outcome { code, stdout: String::from_utf8_lossy(&out).into_owned(), stderr: String::from_utf8_lossy(&err).into_owned() }
}

/// Uma casa mínima: `.env` com token e porta do servidor falso, sem `peers.json`.
pub struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    pub fn new(port: u16, extra_env: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("home/.hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(dir.path().join("inst/backend")).unwrap();
        std::fs::write(config.join(".env"), format!("CP_AUTH_TOKEN=tok\nCP_PORT={port}\n{extra_env}")).unwrap();
        Self { dir }
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().join("inst")
    }

    /// Fora de qualquer sessão: só HOME.
    pub fn vars(&self, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::from([("HOME".to_owned(), self.home().to_string_lossy().into_owned())]);
        vars.extend(extra.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())));
        vars
    }

    pub fn run(&self, args: &[&str], extra: &[(&str, &str)], host: &dyn Host) -> Outcome {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        invoke(&args, Some(self.root()), &self.home(), self.vars(extra), host)
    }
}

/// Host sem livro de peers e com o tmux que falha em tudo (fora de qualquer sessão).
pub fn plain_host() -> FakeHost {
    let tmux = BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() })]);
    FakeHost::new(Some(BTreeMap::new()), tmux)
}
