//! O PTY de um painel: `tmux attach` na sessão, leitor e escritor em threads, desmontagem que
//! solta só o NOSSO cliente e devolve à janela o tamanho de antes.
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot};

use super::resolve::tmux;
use super::{Slot, TermConfig};

/// Leitura do PTY; quadro sempre abaixo do 1 MiB do cliente nativo.
pub(crate) const CHUNK: usize = 64 * 1024;
/// 16 × 64 KiB = 1 MiB parado no canal; cheio, o leitor espera (contrapressão até o `cat`).
pub(crate) const OUTPUT_SLOTS: usize = 16;
const INPUT_SLOTS: usize = 64;
/// Tamanho da janela antes do primeiro painel, para repor na saída ou depois de uma queda.
pub(crate) const SIZE_OPTION: &str = "@hangar_term_size";
/// O `tmux attach` do psmux entrando na tela alternativa: o primeiro byte dele depois do preâmbulo
/// do conhost. Tecla escrita no ConPTY antes disso some.
const READY_MARK: &[u8] = b"\x1b[?1049h";
const READY_WAIT: Duration = Duration::from_secs(5);
const HELD_MAX: usize = 64 * 1024;

pub(crate) struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Vazio no Windows: o psmux não tem identidade de cliente.
    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) tty: String,
}

pub(crate) struct Opened {
    pub(crate) pty: Pty,
    pub(crate) output: mpsc::Receiver<Bytes>,
    pub(crate) input: mpsc::Sender<Bytes>,
    /// Código de quando a entrada segurada saiu sem o sinal de pronto (prazo ou teto).
    pub(crate) held_failure: Option<oneshot::Receiver<&'static str>>,
}

pub(crate) fn clamp(cols: i64, rows: i64) -> (u16, u16) {
    (cols.clamp(20, 500) as u16, rows.clamp(5, 200) as u16)
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

/// Bloqueante (fork + exec): chamar em `spawn_blocking`.
pub(crate) fn open(cfg: &TermConfig, target: &str, cols: u16, rows: u16, slot: Arc<Slot>) -> Result<Opened, &'static str> {
    let mut cmd = CommandBuilder::new(&cfg.program);
    if let Some(socket) = &cfg.socket {
        cmd.arg("-S");
        cmd.arg(socket);
    }
    // A SESSÃO, nunca o pane: `attach -t %N` troca o pane ativo para todos os clientes anexados.
    cmd.args(["attach", "-t", &format!("={target}:")]);
    // Sem TERM o attach nem abre no serviço; os outros dois são o contrato de cor.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("CLAUDE_CODE_TMUX_TRUECOLOR", "1");
    // `TMUX` herdado faz o attach recusar ("sessions should be nested") quando o servidor sobe de
    // dentro de um pane.
    for name in ["NOTIFY_SOCKET", "INVOCATION_ID", "LISTEN_FDS", "LISTEN_PID", "LISTEN_FDNAMES", "PSMUX_SESSION",
                 "TMUX", "TMUX_PANE", "HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE", "CP_AUTH_TOKEN"] {
        cmd.env_remove(name);
    }
    if let Ok(dir) = std::env::current_dir() {
        cmd.cwd(dir);
    }
    spawn(cmd, cols, rows, slot, cfg!(windows))
}

/// O PTY com `cmd` dentro, leitor e escritor nas threads. Bloqueante. `gated` segura a entrada
/// até o sinal de pronto (o psmux joga fora o que chega antes; o tty do Unix guarda).
pub(crate) fn spawn(cmd: CommandBuilder, cols: u16, rows: u16, slot: Arc<Slot>, gated: bool) -> Result<Opened, &'static str> {
    let pair = portable_pty::native_pty_system().openpty(size(cols, rows)).map_err(|_| "pty_open")?;
    let child = pair.slave.spawn_command(cmd).map_err(|_| "pty_spawn")?;
    // Sem soltar o escravo aqui, o leitor nunca vê o fim quando o cliente sai.
    drop(pair.slave);
    #[cfg(unix)]
    let tty = pair.master.tty_name().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    #[cfg(windows)]
    let tty = String::new();
    let pty = Pty { master: pair.master, child, tty };
    // Daqui em diante todo erro mata e colhe o `tmux attach`, que senão ficaria anexado. O filho
    // morre antes do mestre: no Windows soltar o mestre fecha o pseudoconsole.
    let fail = |mut pty: Pty, code| {
        kill(&mut pty);
        let _ = pty.child.wait();
        Err(code)
    };
    // Sem o tty não dá para soltar só o nosso cliente na saída.
    #[cfg(unix)]
    if pty.tty.is_empty() {
        return fail(pty, "pty_tty");
    }
    let Ok(mut reader) = pty.master.try_clone_reader() else { return fail(pty, "pty_reader") };
    let Some(writer) = writer(&pty) else { return fail(pty, "pty_writer") };
    let (out_tx, output) = mpsc::channel(OUTPUT_SLOTS);
    let (input, in_rx) = mpsc::channel(INPUT_SLOTS);
    let (gate, held_failure) = if gated {
        let ready = Arc::new(AtomicBool::new(false));
        reader = Box::new(Marked { inner: reader, ready: ready.clone(), tail: Vec::new() });
        let (failed, rx) = oneshot::channel();
        (Some(Gate { ready, wait: READY_WAIT, max: HELD_MAX, failed }), Some(rx))
    } else {
        (None, None)
    };
    let read_slot = slot.clone();
    let spawned = std::thread::Builder::new().name("term-read".into())
        .spawn(move || { pump(reader, out_tx); drop(read_slot); })
        .and_then(|_| std::thread::Builder::new().name("term-write".into())
            .spawn(move || { write_loop(writer, in_rx, gate); drop(slot); }));
    if spawned.is_err() {
        return fail(pty, "pty_thread");
    }
    Ok(Opened { pty, output, input, held_failure })
}

/// Leitor que acende `ready` quando a saída passa pelo `READY_MARK`, mesmo partido entre leituras.
pub(crate) struct Marked<R> {
    pub(crate) inner: R,
    pub(crate) ready: Arc<AtomicBool>,
    pub(crate) tail: Vec<u8>,
}

impl<R: Read> Read for Marked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 && !self.ready.load(Ordering::Relaxed) {
            self.tail.extend_from_slice(&buf[..n]);
            if self.tail.windows(READY_MARK.len()).any(|w| w == READY_MARK) {
                self.ready.store(true, Ordering::Release);
                self.tail = Vec::new();
            } else {
                let keep = self.tail.len().saturating_sub(READY_MARK.len() - 1);
                self.tail.drain(..keep);
            }
        }
        Ok(n)
    }
}

pub(crate) struct Gate {
    pub(crate) ready: Arc<AtomicBool>,
    pub(crate) wait: Duration,
    pub(crate) max: usize,
    pub(crate) failed: oneshot::Sender<&'static str>,
}

/// Segura a entrada até o sinal de pronto e a entrega na ordem. A resposta de posição do cursor
/// passa na hora: é do conhost (`INHERIT_CURSOR`), que sem ela nem sobe o filho, e não chega ao
/// pane. Prazo ou teto estourado entregam o que segurou e avisam. `false` = a escrita acabou.
fn hold(writer: &mut impl Write, rx: &mut mpsc::Receiver<Bytes>, gate: Gate) -> bool {
    let until = Instant::now() + gate.wait;
    let mut held = Vec::new();
    let code = loop {
        if gate.ready.load(Ordering::Acquire) {
            break None;
        }
        match rx.try_recv() {
            Ok(b) => {
                let (cpr, rest) = split_cpr(&b);
                if !cpr.is_empty() && writer.write_all(&cpr).is_err() {
                    let _ = gate.failed.send("input_write_failed");
                    return false;
                }
                held.extend_from_slice(&rest);
                if held.len() > gate.max {
                    break Some("input_held_overflow");
                }
            }
            Err(mpsc::error::TryRecvError::Empty) if Instant::now() >= until => break Some("ready_timeout"),
            // ponytail: espera curta em vez de acordar pelo leitor; só dura a partida do attach.
            Err(mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
            // O painel fechou: não há mais quem receba o que estava segurado.
            Err(mpsc::error::TryRecvError::Disconnected) => return false,
        }
    };
    let delivered = held.is_empty() || writer.write_all(&held).is_ok();
    if let Some(code) = (!delivered).then_some("input_write_failed").or(code) {
        let _ = gate.failed.send(code);
    }
    delivered
}

/// Separa as respostas `ESC [ n ; m R` do resto. Resposta partida entre quadros fica no resto.
fn split_cpr(b: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut cpr, mut rest) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        match cpr_len(&b[i..]) {
            Some(n) => { cpr.extend_from_slice(&b[i..i + n]); i += n; }
            None => { rest.push(b[i]); i += 1; }
        }
    }
    (cpr, rest)
}

fn cpr_len(b: &[u8]) -> Option<usize> {
    let body = b.strip_prefix(b"\x1b[")?;
    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    let row = digits(body);
    if row == 0 || body.get(row) != Some(&b';') {
        return None;
    }
    let col = digits(&body[row + 1..]);
    (col > 0 && body.get(row + 1 + col) == Some(&b'R')).then_some(2 + row + 1 + col + 1)
}

/// Lê até o fim do PTY. Canal cheio segura a leitura; sem ninguém ouvindo, segue lendo e
/// descarta: no Windows o `ClosePseudoConsole` espera o conhost esvaziar a saída.
pub(crate) fn pump(mut reader: impl Read, tx: mpsc::Sender<Bytes>) {
    let mut buf = vec![0u8; CHUNK];
    let mut tx = Some(tx);
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => if tx.as_ref().is_some_and(|t| t.blocking_send(Bytes::copy_from_slice(&buf[..n])).is_err()) {
                tx = None;
            },
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // O fim do mestre no Linux chega como EIO, não como 0.
            Err(_) => return,
        }
    }
}

/// Nunca o `take_writer` no Unix: o `Drop` dele escreve "\n" + EOF no PTY, e o tmux entrega ao
/// pane — fechar o painel mandaria Enter e Ctrl-D ao agente. Um `dup` do mestre só fecha.
#[cfg(unix)]
fn writer(pty: &Pty) -> Option<std::fs::File> {
    pty.master.as_raw_fd()
        // SAFETY: duplica um descritor vivo do mestre.
        .map(|fd| unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) })
        .filter(|fd| *fd >= 0)
        // SAFETY: o descritor acabou de nascer do `fcntl` e ninguém mais é dono dele.
        .map(|fd| unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
}

/// No Windows o `take_writer` é o pipe de entrada do ConPTY, e soltá-lo só fecha o handle.
#[cfg(windows)]
fn writer(pty: &Pty) -> Option<Box<dyn Write + Send>> {
    pty.master.take_writer().ok()
}

pub(crate) fn write_loop(mut writer: impl Write, mut rx: mpsc::Receiver<Bytes>, gate: Option<Gate>) {
    if let Some(gate) = gate {
        if !hold(&mut writer, &mut rx, gate) {
            return;
        }
    }
    while let Some(b) = rx.blocking_recv() {
        if writer.write_all(&b).is_err() {
            return;
        }
    }
}

impl Pty {
    pub(crate) fn resize(&self, cols: u16, rows: u16) {
        if self.master.resize(size(cols, rows)).is_err() {
            tracing::debug!("terminal: resize do pty falhou");
        }
    }
}

#[cfg(unix)]
fn kill(pty: &mut Pty) {
    signal(pty, libc::SIGKILL);
}

#[cfg(windows)]
fn kill(pty: &mut Pty) {
    // Filho que já saiu devolve erro de acesso; quem decide é a espera depois.
    let _ = pty.child.kill();
}

#[cfg(unix)]
fn signal(pty: &Pty, sig: i32) {
    if let Some(pid) = pty.child.process_id() {
        // SAFETY: kill(2) só envia sinal; o pid é do nosso filho ainda não colhido.
        unsafe { libc::kill(pid as i32, sig) };
    }
}

#[cfg(unix)]
async fn reaped(pty: &mut Pty, within: Duration) -> bool {
    let until = tokio::time::Instant::now() + within;
    loop {
        match pty.child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) if tokio::time::Instant::now() >= until => return false,
            Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

fn parse_size(text: &str, sep: char) -> Option<(u32, u32)> {
    let (w, h) = text.trim().split_once(sep)?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Tamanho a repor na saída. Uma opção que ficou de uma queda vale mais que a janela de agora,
/// que pode estar no tamanho do painel que caiu.
pub(crate) async fn remember_size(cfg: &TermConfig, target: &str) -> Option<(u32, u32)> {
    // No psmux a janela segue o cliente anexado, e `resize-window`/`setw` voltam 0 sem fazer
    // nada: não há o que repor (`termsock._desmontar_windows`).
    if cfg!(windows) {
        return None;
    }
    let t = format!("={target}:");
    if let Ok(out) = tmux(cfg, &["show-options", "-v", "-t", &t, SIZE_OPTION]).await {
        if let Some(saved) = out.status.success().then(|| parse_size(&String::from_utf8_lossy(&out.stdout), 'x')).flatten() {
            return Some(saved);
        }
    }
    // Com `:`: só `={name}` deixa window_width vazio no `display -p`.
    let size = match tmux(cfg, &["display", "-p", "-t", &t, "#{window_width}\t#{window_height}"]).await {
        Ok(out) if out.status.success() => parse_size(&String::from_utf8_lossy(&out.stdout), '\t'),
        _ => None,
    };
    let Some((w, h)) = size else {
        tracing::warn!(session = %target, "terminal: tamanho da janela não lido; não será reposto na saída");
        return None;
    };
    if !tmux(cfg, &["set-option", "-t", &t, SIZE_OPTION, &format!("{w}x{h}")]).await.is_ok_and(|o| o.status.success()) {
        tracing::warn!(session = %target, "terminal: tamanho não guardado no tmux; uma queda do Rust não o repõe");
    }
    Some((w, h))
}

/// `resize-window` sozinho deixa a janela em tamanho manual; o par com `setw latest` devolve o
/// normal. A opção só sai depois dos dois: é a pista que a subida seguinte usaria.
pub(crate) async fn restore_size(cfg: &TermConfig, target: &str, (w, h): (u32, u32)) -> Result<(), &'static str> {
    let s = format!("={target}");
    let ok = |r: Result<std::process::Output, super::resolve::MuxDown>| r.is_ok_and(|o| o.status.success());
    if !ok(tmux(cfg, &["resize-window", "-t", &s, "-x", &w.to_string(), "-y", &h.to_string()]).await)
        || !ok(tmux(cfg, &["setw", "-t", &s, "window-size", "latest"]).await) {
        return Err("size_restore_failed");
    }
    let _ = tmux(cfg, &["set-option", "-u", "-t", &format!("={target}:"), SIZE_OPTION]).await;
    Ok(())
}

/// Ordem do `termsock._desmontar`: soltar o nosso cliente, fechar, colher, esperar ele sair da
/// lista e só então repor o tamanho (antes disso o tmux reimpõe o do cliente). `Err` = o código
/// do que ficou para trás (cliente vivo ou janela no tamanho do painel).
#[cfg(unix)]
pub(crate) async fn teardown(cfg: &TermConfig, target: &str, mut pty: Pty, saved: Option<(u32, u32)>) -> Result<(), &'static str> {
    // `-t <tty>`, nunca `-s`: `-s` derruba também o `tmux attach` nativo do dono.
    let _ = tmux(cfg, &["detach-client", "-t", &pty.tty]).await;
    signal(&pty, libc::SIGHUP);
    let mut gone = reaped(&mut pty, Duration::from_secs(3)).await;
    if !gone {
        signal(&pty, libc::SIGKILL);
        gone = reaped(&mut pty, Duration::from_secs(1)).await;
    }
    let Pty { master, tty, .. } = pty;
    drop(master);
    if !gone {
        return Err("client_not_reaped");
    }
    let Some(saved) = saved else { return Ok(()) };
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let Ok(out) = tmux(cfg, &["list-clients", "-t", &format!("={target}"), "-F", "#{client_tty}"]).await else {
            return Err("size_restore_failed");
        };
        // Sessão que acabou não tem janela a repor.
        if !out.status.success() {
            return Ok(());
        }
        if !String::from_utf8_lossy(&out.stdout).split_whitespace().any(|t| t == tty) {
            break;
        }
        if tokio::time::Instant::now() >= until {
            return Err("client_still_attached");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    restore_size(cfg, target, saved).await
}

/// Ao subir: sessões que guardaram o tamanho e não têm painel são de um Rust que caiu no meio.
pub(crate) async fn restore_after_crash(cfg: &TermConfig, open: impl Fn(&str) -> bool) {
    let format = format!("#{{session_name}}\t#{{{SIZE_OPTION}}}");
    let Ok(out) = tmux(cfg, &["list-sessions", "-F", &format]).await else {
        tracing::warn!("terminal: multiplexador sem resposta ao repor tamanhos");
        return;
    };
    if !out.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((name, value)) = line.split_once('\t') else { continue };
        if let Some(saved) = parse_size(value, 'x') {
            if !open(name) && restore_size(cfg, name, saved).await.is_err() {
                tracing::warn!(session = %name, "terminal: tamanho deixado por um Rust anterior não reposto");
            }
        }
    }
}

/// No psmux matar o NOSSO `tmux attach` é o desmonte: não há `detach-client -t <tty>`, e `-s`
/// derrubaria também o cliente nativo do dono.
#[cfg(windows)]
pub(crate) async fn teardown(_cfg: &TermConfig, _target: &str, pty: Pty, _saved: Option<(u32, u32)>) -> Result<(), &'static str> {
    close(pty).await.map(|_| ())
}

/// Mata o filho e só então fecha o pseudoconsole: `ClosePseudoConsole` com o cliente vivo pode
/// travar esperando ele sair (microsoft/terminal#17716).
#[cfg(windows)]
pub(crate) async fn close(mut pty: Pty) -> Result<portable_pty::ExitStatus, &'static str> {
    kill(&mut pty);
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    let status = loop {
        match pty.child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if tokio::time::Instant::now() < until => tokio::time::sleep(Duration::from_millis(50)).await,
            _ => break None,
        }
    };
    let Some(status) = status else {
        // Vazar um conhost é o mal menor: fechar com o filho vivo prenderia a thread para sempre.
        std::mem::forget(pty.master);
        return Err("client_not_reaped");
    };
    // Fechar pode esperar o conhost esvaziar a saída, e o mestre ainda segura a ponta de leitura:
    // fora das threads do runtime e com prazo, para o painel não ficar preso sem código.
    match tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(move || drop(pty))).await {
        Ok(Ok(())) => Ok(status),
        Ok(Err(_)) => Err("pty_close_panic"),
        Err(_) => Err("pty_close_timeout"),
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(b); Ok(b.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    impl Sink {
        fn text(&self) -> String { String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned() }
    }

    struct Gated {
        sink: Sink,
        tx: mpsc::Sender<Bytes>,
        ready: Arc<AtomicBool>,
        failed: oneshot::Receiver<&'static str>,
        writer: std::thread::JoinHandle<()>,
    }

    fn gated(wait: Duration, max: usize) -> Gated {
        let sink = Sink::default();
        let (tx, rx) = mpsc::channel(INPUT_SLOTS);
        let ready = Arc::new(AtomicBool::new(false));
        let (failed_tx, failed) = oneshot::channel();
        let gate = Gate { ready: ready.clone(), wait, max, failed: failed_tx };
        let out = sink.clone();
        let writer = std::thread::spawn(move || write_loop(out, rx, Some(gate)));
        Gated { sink, tx, ready, failed, writer }
    }

    fn settle() { std::thread::sleep(Duration::from_millis(200)); }

    #[test]
    fn input_waits_for_ready_in_order_and_cursor_reply_passes() {
        let mut g = gated(Duration::from_secs(30), HELD_MAX);
        for b in [&b"ab"[..], b"\x1b[3;7R", b"c\x1b[12;1Rd"] {
            g.tx.blocking_send(Bytes::copy_from_slice(b)).unwrap();
        }
        settle();
        assert_eq!(g.sink.text(), "\x1b[3;7R\x1b[12;1R", "antes do sinal só a resposta do cursor passa");
        g.ready.store(true, Ordering::Release);
        g.tx.blocking_send(Bytes::from_static(b"e")).unwrap();
        drop(g.tx);
        g.writer.join().unwrap();
        assert_eq!(g.sink.text(), "\x1b[3;7R\x1b[12;1Rabcde");
        assert!(g.failed.try_recv().is_err(), "com o sinal no prazo não há o que avisar");
    }

    #[test]
    fn deadline_delivers_held_input_and_reports() {
        let mut g = gated(Duration::from_millis(400), HELD_MAX);
        g.tx.blocking_send(Bytes::from_static(b"k")).unwrap();
        settle();
        assert_eq!(g.sink.text(), "");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(g.sink.text(), "k", "passado o prazo a tecla segurada sai");
        assert_eq!(g.failed.try_recv(), Ok("ready_timeout"));
        drop(g.tx);
        g.writer.join().unwrap();
    }

    #[test]
    fn over_the_cap_delivers_without_waiting_and_reports() {
        let mut g = gated(Duration::from_secs(30), 4);
        g.tx.blocking_send(Bytes::from_static(b"123")).unwrap();
        g.tx.blocking_send(Bytes::from_static(b"456")).unwrap();
        settle();
        assert_eq!(g.sink.text(), "123456");
        assert_eq!(g.failed.try_recv(), Ok("input_held_overflow"));
        drop(g.tx);
        g.writer.join().unwrap();
    }

    #[test]
    fn ready_mark_split_between_reads() {
        let ready = Arc::new(AtomicBool::new(false));
        let chunks = [&b"\x1b[6n\x1b]0;tmux\x07\x1b[?10"[..], b"49h\x1b[H"];
        let inner = std::io::Read::chain(chunks[0], chunks[1]);
        let mut r = Marked { inner, ready: ready.clone(), tail: Vec::new() };
        let mut buf = [0u8; 64];
        let n = r.read(&mut buf).unwrap();
        assert_eq!(n, chunks[0].len());
        assert!(!ready.load(Ordering::Acquire), "o preâmbulo do conhost não é o sinal");
        let n = r.read(&mut buf).unwrap();
        assert_eq!(n, chunks[1].len());
        assert_eq!(&buf[..n], chunks[1]);
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn cpr_split() {
        assert_eq!(split_cpr(b"a\x1b[1;1Rb\x1b[;1R\x1b[2;3"), (b"\x1b[1;1R".to_vec(), b"ab\x1b[;1R\x1b[2;3".to_vec()));
    }
}
