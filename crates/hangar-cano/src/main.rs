//! Cano: dono do processo de uma sessão sem terminal (claude stream-json ou app-server do Codex),
//! separado do backend. Port de `backend/app/adapters/claude_headless/cano.py`, mesmo contrato:
//! sobe o filho, segura stdin/stdout dele e escuta num socket local; o backend conecta,
//! desconecta e reconecta, e quem chega recebe o snapshot do que está em aberto.
//!
//! Uso: hangar-cano --escuta unix:/x.sock|tcp:127.0.0.1:PORT [--token T] [--log F] [--cwd D] -- argv...

mod protocol;
mod stderr_text;

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc, watch};

use protocol::Tracker;

const QUEUE_LIMIT: usize = 5000; // cano.py:43
const STDERR_TAIL: usize = 20; // cano.py:52
const TOKEN_DEADLINE: Duration = Duration::from_secs(10); // cano.py:239
const EXIT_DEADLINE: Duration = Duration::from_secs(5); // cano.py:185
const LINGER: Duration = Duration::from_secs(60); // cano.py:33
// O rc sai com a cauda do stderr completa; filho que deixou um neto segurando o stderr não trava o rc.
const STDERR_GRACE: Duration = Duration::from_secs(1);

// ── log (cano.py:58) ───────────────────────────────────────────────────────────────────────

static LOG: OnceLock<Mutex<Box<dyn Write + Send>>> = OnceLock::new();

fn init_log(path: Option<&OsStr>) -> io::Result<()> {
    let sink: Box<dyn Write + Send> = match path {
        Some(p) => Box::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?),
        None => Box::new(io::stderr()),
    };
    let _ = LOG.set(Mutex::new(sink));
    Ok(())
}

fn log(msg: &str) {
    let Some(sink) = LOG.get() else { return };
    let mut w = sink.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(w, "{} {msg}", chrono::Local::now().format("%H:%M:%S"));
    let _ = w.flush();
}

// ── argumentos (cano.py:372-382) ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct Args {
    listen: String,
    token: Option<String>,
    log: Option<OsString>,
    cwd: Option<OsString>,
    argv: Vec<OsString>,
}

/// Como o argparse com `REMAINDER`: o comando começa no `--` ou no primeiro posicional.
fn parse_args(raw: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let (mut listen, mut token, mut log, mut cwd) = (None, None, None, None);
    let mut argv = Vec::new();
    let mut it = raw.into_iter();
    while let Some(arg) = it.next() {
        let Some(s) = arg.to_str() else {
            argv.push(arg);
            argv.extend(it);
            break;
        };
        if s == "--" {
            argv.extend(it);
            break;
        }
        if !s.starts_with("--") {
            argv.push(arg);
            argv.extend(it);
            break;
        }
        let (name, inline) = match s.split_once('=') {
            Some((n, v)) => (n.to_owned(), Some(OsString::from(v))),
            None => (s.to_owned(), None),
        };
        let slot = match name.as_str() {
            "--escuta" => &mut listen,
            "--token" => &mut token,
            "--log" => &mut log,
            "--cwd" => &mut cwd,
            _ => return Err(format!("opção desconhecida: {name}")),
        };
        let value = match inline {
            Some(v) => v,
            None => it.next().ok_or_else(|| format!("{name} pede um valor"))?,
        };
        *slot = Some(value);
    }
    let text = |v: Option<OsString>, name: &str| -> Result<Option<String>, String> {
        v.map(|v| v.into_string().map_err(|_| format!("{name} não é UTF-8"))).transpose()
    };
    let listen = text(listen, "--escuta")?.ok_or("faltou --escuta")?;
    let token = text(token, "--token")?;
    if argv.is_empty() {
        return Err("faltou o comando do claude depois de --".to_owned());
    }
    Ok(Args { listen, token, log, cwd, argv })
}

// ── estado compartilhado (cano.py:37-54) ───────────────────────────────────────────────────

enum Out {
    Line(String),
    Exit(String),
}

struct Client {
    id: u64,
    tx: mpsc::UnboundedSender<Out>,
    queued: Arc<AtomicUsize>,
    full_warned: bool,
    // Cair do estado (troca ou saída) derruba leitor e escritor dele: os dois esperam este canal fechar.
    _alive: watch::Sender<()>,
}

#[derive(Default)]
struct State {
    tracker: Tracker,
    stderr_tail: VecDeque<String>,
    exited: Option<i64>,
    exit_delivered: bool,
    client: Option<Client>,
    next_client: u64,
}

impl State {
    /// `cano.py:162`. Só enfileira; quem escreve no socket é a tarefa do cliente. Sem cliente, a
    /// linha é descartada: o snapshot carrega o que importa.
    fn send(&mut self, line: String) {
        let Some(c) = self.client.as_mut() else { return };
        if c.queued.load(Ordering::Relaxed) >= QUEUE_LIMIT {
            if !c.full_warned {
                c.full_warned = true;
                log("fila de saída cheia: cliente não lê; descartando");
            }
            return;
        }
        c.queued.fetch_add(1, Ordering::Relaxed);
        let _ = c.tx.send(Out::Line(line));
    }
}

struct Inner {
    state: Mutex<State>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    delivered: Notify,
    token: Option<String>,
    pid: u32,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn drop_client_if(&self, id: u64) {
        let mut st = self.state();
        if st.client.as_ref().is_some_and(|c| c.id == id) {
            st.client = None;
        }
    }
}

/// `cano.py:176`. A saída vai atrás do que já estava na fila do cliente e tem 5 s para chegar;
/// sem cliente, fica para o snapshot de quem chegar.
fn send_exit(st: &mut State, inner: &Arc<Inner>) {
    let Some(c) = st.client.as_ref() else { return };
    if c.tx.send(Out::Exit(protocol::exit_line(st.exited, &st.stderr_tail))).is_err() {
        return;
    }
    let (id, inner) = (c.id, inner.clone());
    tokio::spawn(async move {
        tokio::time::sleep(EXIT_DEADLINE).await;
        let mut st = inner.state();
        if !st.exit_delivered && st.client.as_ref().is_some_and(|c| c.id == id) {
            st.client = None;
        }
    });
}

// ── filho (cano.py:67-101) ─────────────────────────────────────────────────────────────────

async fn pump_stdout(
    inner: Arc<Inner>,
    out: ChildStdout,
    stderr_task: tokio::task::JoinHandle<()>,
    mut rc: watch::Receiver<Option<i64>>,
) {
    let mut r = BufReader::new(out);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                log(&format!("leitura do stdout do claude falhou: {e}"));
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let mut st = inner.state();
        st.tracker.observe_child(line);
        st.send(line.to_owned());
    }
    let code = rc.wait_for(Option::is_some).await.ok().and_then(|v| *v).unwrap_or(-1);
    let _ = tokio::time::timeout(STDERR_GRACE, stderr_task).await;
    let mut st = inner.state();
    st.exited = Some(code);
    st.tracker.turn_open = false;
    log(&format!("claude saiu rc={code}"));
    send_exit(&mut st, &inner);
}

async fn pump_stderr(inner: Arc<Inner>, err: ChildStderr) {
    let mut r = BufReader::new(err);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = stderr_text::stderr_text(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let mut st = inner.state();
        if st.stderr_tail.len() == STDERR_TAIL {
            st.stderr_tail.pop_front();
        }
        st.stderr_tail.push_back(line.to_owned());
        st.send(protocol::stderr_line(line));
    }
}

/// Código de saída como o `Popen.returncode`: sinal vira negativo, e no Windows o DWORD fica sem sinal.
fn exit_code(status: std::process::ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -i64::from(sig);
        }
    }
    #[cfg(windows)]
    if let Some(c) = status.code() {
        return i64::from(c as u32);
    }
    status.code().map(i64::from).unwrap_or(-1)
}

// ── cliente (cano.py:192-292) ──────────────────────────────────────────────────────────────

async fn serve_client<S>(inner: Arc<Inner>, stream: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (r, w) = tokio::io::split(stream);
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    if let Some(token) = &inner.token {
        // TCP em loopback: qualquer processo local alcança a porta; o token faz o cano ser só do backend.
        let read = tokio::time::timeout(TOKEN_DEADLINE, r.read_until(b'\n', &mut buf)).await;
        if !matches!(read, Ok(Ok(_))) || String::from_utf8_lossy(&buf).trim() != token {
            return;
        }
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let (alive_tx, alive_rx) = watch::channel(());
    let queued = Arc::new(AtomicUsize::new(0));
    let (id, snapshot) = {
        let mut st = inner.state();
        st.next_client += 1;
        let id = st.next_client;
        // Um cliente por vez: o antigo cai aqui, e o que sobrou na fila dele já está no snapshot.
        st.client = Some(Client { id, tx, queued: queued.clone(), full_warned: false, _alive: alive_tx });
        let snapshot = st.tracker.snapshot(inner.pid, &st.stderr_tail, st.exited);
        if st.exited.is_some() {
            // Já saiu: quem chegou leva o rc como se acontecesse agora, e aí o cano pode morrer.
            send_exit(&mut st, &inner);
        }
        (id, snapshot)
    };
    log("cliente conectado");
    tokio::spawn(write_client(inner.clone(), id, w, snapshot, rx, queued, alive_rx.clone()));
    read_client(&inner, r, buf, alive_rx).await;
    inner.drop_client_if(id);
    log("cliente saiu");
}

async fn write_client<W: AsyncWrite + Unpin>(
    inner: Arc<Inner>,
    id: u64,
    mut w: W,
    snapshot: String,
    mut rx: mpsc::UnboundedReceiver<Out>,
    queued: Arc<AtomicUsize>,
    mut alive: watch::Receiver<()>,
) {
    let work = async {
        write_line(&mut w, &snapshot).await?;
        while let Some(out) = rx.recv().await {
            match out {
                Out::Line(line) => {
                    queued.fetch_sub(1, Ordering::Relaxed);
                    write_line(&mut w, &line).await?;
                }
                Out::Exit(line) => {
                    write_line(&mut w, &line).await?;
                    inner.state().exit_delivered = true;
                    inner.delivered.notify_one();
                }
            }
        }
        Ok::<(), io::Error>(())
    };
    let failed = tokio::select! {
        r = work => r.is_err(),
        _ = alive.changed() => false,
    };
    if failed {
        inner.drop_client_if(id);
    }
    let _ = w.shutdown().await;
}

async fn write_line<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    w.write_all(&bytes).await
}

async fn read_client<R: AsyncRead + Unpin>(
    inner: &Inner,
    mut r: BufReader<R>,
    mut buf: Vec<u8>,
    mut alive: watch::Receiver<()>,
) {
    loop {
        buf.clear();
        let read = tokio::select! {
            r = r.read_until(b'\n', &mut buf) => r,
            _ = alive.changed() => return,
        };
        if !matches!(read, Ok(n) if n > 0) {
            return;
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let exited = {
            let mut st = inner.state();
            st.tracker.observe_client(line);
            st.exited.is_some()
        };
        if exited {
            continue;
        }
        let mut stdin = inner.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else { continue };
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let wrote = match pipe.write_all(&bytes).await {
            Ok(()) => pipe.flush().await,
            Err(e) => Err(e),
        };
        if let Err(e) = wrote {
            log(&format!("stdin do claude falhou: {e}"));
        }
    }
}

// ── escuta (cano.py:294-312) ───────────────────────────────────────────────────────────────

enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    Tcp(tokio::net::TcpListener),
}

fn listen(spec: &str) -> io::Result<Listener> {
    if let Some(path) = spec.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let sock = tokio::net::UnixSocket::new_stream()?;
            sock.bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            return Ok(Listener::Unix(sock.listen(2)?));
        }
        #[cfg(not(unix))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("socket unix indisponível: {path}")));
    }
    // Como o cano.py: o que não é unix: é tcp:host:porta.
    let rest = spec.get(4..).unwrap_or("");
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "endereço sem porta"))?;
    let port: u16 = port.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "porta inválida"))?;
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "host sem IPv4"))?;
    let sock = tokio::net::TcpSocket::new_v4()?;
    sock.set_reuseaddr(true)?;
    sock.bind(addr)?;
    Ok(Listener::Tcp(sock.listen(2)?))
}

/// Uma tarefa por cliente: em série, quem chega só recebe o snapshot quando o ligado sai, e o
/// backend que não recebe snapshot a tempo mata o cano como mudo.
async fn accept_loop(listener: Listener, inner: Arc<Inner>) {
    loop {
        let accepted = match &listener {
            #[cfg(unix)]
            Listener::Unix(l) => l.accept().await.map(|(s, _)| {
                tokio::spawn(serve_client(inner.clone(), s));
            }),
            Listener::Tcp(l) => l.accept().await.map(|(s, _)| {
                tokio::spawn(serve_client(inner.clone(), s));
            }),
        };
        if let Err(e) = accepted {
            log(&format!("accept falhou, parei de escutar: {e}"));
            return;
        }
    }
}

fn remove_socket(spec: &str) {
    if let Some(path) = spec.strip_prefix("unix:") {
        let _ = std::fs::remove_file(path);
    }
}

// ── ciclo de vida (cano.py:314-420) ────────────────────────────────────────────────────────

/// SIGTERM do backend (encerrar sessão): derruba o filho e não deixa socket velho.
#[cfg(unix)]
fn watch_sigterm(inner: Arc<Inner>, rc: watch::Receiver<Option<i64>>, spec: String) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sig = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("sem tratador de SIGTERM: {e}"));
            return;
        }
    };
    tokio::spawn(async move {
        sig.recv().await;
        log(&format!("sinal {}: encerrando", libc::SIGTERM));
        // rc ainda vazio = filho não foi colhido, então o pid ainda é dele.
        if rc.borrow().is_none() {
            // SAFETY: kill(2) com pid e sinal válidos.
            unsafe { libc::kill(inner.pid as libc::pid_t, libc::SIGTERM) };
        }
        remove_socket(&spec);
        std::process::exit(0);
    });
}

async fn run(args: Args) -> i32 {
    // ANTES de subir o filho: escuta que falha (caminho unix > 107 bytes, porta ocupada) derruba o
    // cano com log e código de saída, nunca deixa um filho órfão sem porta.
    let listener = match listen(&args.listen) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("não consegui escutar em {}: {e}", args.listen));
            return 1;
        }
    };
    // Mesmo grupo de processos que o cano: matar o grupo do cano mata os dois.
    let mut cmd = Command::new(&args.argv[0]);
    cmd.args(&args.argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = &args.cwd {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(&format!("claude não subiu: {e}"));
            drop(listener);
            remove_socket(&args.listen);
            return 1;
        }
    };
    let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take().expect("stdout em pipe");
    let stderr = child.stderr.take().expect("stderr em pipe");
    let inner = Arc::new(Inner {
        state: Mutex::new(State::default()),
        stdin: tokio::sync::Mutex::new(child.stdin.take()),
        delivered: Notify::new(),
        token: args.token.clone(),
        pid,
    });
    let (rc_tx, rc_rx) = watch::channel(None);
    tokio::spawn(async move {
        let code = match child.wait().await {
            Ok(status) => exit_code(status),
            Err(e) => {
                log(&format!("espera do claude falhou: {e}"));
                -1
            }
        };
        let _ = rc_tx.send(Some(code));
    });
    let stderr_task = tokio::spawn(pump_stderr(inner.clone(), stderr));
    tokio::spawn(pump_stdout(inner.clone(), stdout, stderr_task, rc_rx.clone()));
    log(&format!("claude pid={pid}"));
    tokio::spawn(accept_loop(listener, inner.clone()));
    #[cfg(unix)]
    watch_sigterm(inner.clone(), rc_rx.clone(), args.listen.clone());

    // Vive enquanto o filho viver; depois espera um cliente levar o rc (ou desiste).
    let mut rc = rc_rx;
    let _ = rc.wait_for(Option::is_some).await;
    let _ = tokio::time::timeout(LINGER, inner.delivered.notified()).await;
    remove_socket(&args.listen);
    0
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("cano: {msg}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = init_log(args.log.as_deref()) {
        eprintln!("cano: não abri o log: {e}");
        return ExitCode::from(1);
    }
    // O stderr do cano é DEVNULL: sem isto, pânico numa tarefa sumia sem rastro.
    std::panic::set_hook(Box::new(|info| log(&format!("tarefa estourou: {info}"))));
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log(&format!("runtime não subiu: {e}"));
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(run(args));
    // Sai na hora: tarefa presa em leitura de socket não segura o processo.
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Result<Args, String> {
        parse_args(v.iter().map(OsString::from))
    }

    #[test]
    fn parses_the_backend_command_line() {
        let a = args(&["--escuta", "unix:/x.sock", "--log", "/l", "--cwd", "/c", "--token", "t", "--", "claude", "--", "x"])
            .unwrap();
        assert_eq!(a.listen, "unix:/x.sock");
        assert_eq!(a.token.as_deref(), Some("t"));
        assert_eq!(a.log, Some(OsString::from("/l")));
        assert_eq!(a.cwd, Some(OsString::from("/c")));
        assert_eq!(a.argv, ["claude", "--", "x"].map(OsString::from));
    }

    #[test]
    fn remainder_starts_at_first_positional_and_accepts_equals() {
        let a = args(&["--escuta=tcp:127.0.0.1:1", "claude", "--log", "z"]).unwrap();
        assert_eq!(a.listen, "tcp:127.0.0.1:1");
        assert_eq!(a.log, None);
        assert_eq!(a.argv, ["claude", "--log", "z"].map(OsString::from));
    }

    #[test]
    fn missing_command_or_listen_is_an_error() {
        assert!(args(&["--escuta", "unix:/x"]).is_err());
        assert!(args(&["--escuta", "unix:/x", "--"]).is_err());
        assert!(args(&["--", "claude"]).is_err());
        assert!(args(&["--escuta", "unix:/x", "--outra", "v", "--", "claude"]).is_err());
    }
}
