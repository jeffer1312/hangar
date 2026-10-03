//! Observador tmux: somente display/capture, sem entrada ou respostas ao PTY.
use crate::terminal_state::{self, PaneAnalysis};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, VecDeque}, path::PathBuf, sync::{Arc, atomic::{AtomicU64, Ordering}}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, mpsc, oneshot}, time::{Instant, timeout}};

const MAX_FRAME: usize = 8 * 1024 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const MAX_ACTORS: usize = 64;
const MAX_CONSUMERS: usize = 256;
static NEXT_COMMAND: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    pub consumer: String,
    pub name: String,
    pub provider: String,
    pub binding: String,
    pub target: String,
    pub started: f64,
    pub lines: u32,
    pub colors: bool,
    pub join: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaptureResult {
    pub binding: String,
    pub started: f64,
    pub text: String,
    pub analysis: PaneAnalysis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalError(pub &'static str);
impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for TerminalError {}
type Result<T> = std::result::Result<T, TerminalError>;

fn io_failure(code: &'static str, error: std::io::Error) -> TerminalError {
    tracing::warn!(code, io_kind = ?error.kind(), "observação terminal usa reserva Python");
    TerminalError(code)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    Frame { identity: FrameIdentity, text: String, error: bool },
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameIdentity {
    pub timestamp: u64,
    pub command: u64,
    pub flags: u32,
}

#[derive(Default)]
pub struct ControlParser {
    pending: Vec<u8>,
    frame: Option<(FrameIdentity, Vec<u8>)>,
}
fn marker(line: &[u8], prefix: &[u8]) -> Option<FrameIdentity> {
    let suffix = line.strip_prefix(prefix)?;
    let text = std::str::from_utf8(suffix).ok()?;
    let parts: Vec<_> = text.split(' ').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit())) { return None; }
    Some(FrameIdentity { timestamp: parts[0].parse().ok()?, command: parts[1].parse().ok()?, flags: parts[2].parse().ok()? })
}
impl ControlParser {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<ControlEvent>> {
        if bytes.len() > MAX_FRAME { return Err(TerminalError("control payload too large")); }
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            if end > MAX_LINE { return Err(TerminalError("control line too large")); }
            let mut line: Vec<u8> = self.pending.drain(..=end).collect();
            line.pop();
            if let Some((identity, body)) = &mut self.frame {
                let success = marker(&line, b"%end ").as_ref() == Some(identity);
                let error = marker(&line, b"%error ").as_ref() == Some(identity);
                if success || error {
                    let (identity, body) = self.frame.take().unwrap();
                    let text = String::from_utf8(body).map_err(|_| TerminalError("invalid frame UTF-8"))?;
                    events.push(ControlEvent::Frame { identity, text, error });
                } else {
                    if body.len() + line.len() + 1 > MAX_FRAME { return Err(TerminalError("control frame too large")); }
                    body.extend(line); body.push(b'\n');
                }
            } else if let Some(identity) = marker(&line, b"%begin ") {
                self.frame = Some((identity, Vec::new()));
            } else if std::str::from_utf8(&line).is_err() { return Err(TerminalError("invalid notification UTF-8")); }
            else if line.starts_with(b"%exit") { events.push(ControlEvent::Exit); }
            else if !line.starts_with(b"%") || line.starts_with(b"%begin ") || line.starts_with(b"%end ") || line.starts_with(b"%error ") {
                return Err(TerminalError("invalid control frame"));
            }
        }
        if self.pending.len() > MAX_LINE { return Err(TerminalError("control line too large")); }
        Ok(events)
    }
    pub fn finish(&self) -> Result<()> {
        if self.pending.is_empty() && self.frame.is_none() { Ok(()) }
        else { Err(TerminalError("unfinished control frame")) }
    }
}
fn pane_id(pane: &str) -> bool {
    pane.strip_prefix('%').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
#[derive(Clone)]
pub struct Limits {
    pub startup: Duration,
    pub command: Duration,
    pub lease: Duration,
    pub retry: Duration,
}
impl Default for Limits {
    fn default() -> Self { Self { startup: Duration::from_secs(3), command: Duration::from_secs(2), lease: Duration::from_secs(90), retry: Duration::from_secs(2) } }
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key { name: String, provider: String, binding: String, target: String }
impl From<&CaptureRequest> for Key {
    fn from(r: &CaptureRequest) -> Self { Self { name: r.name.clone(), provider: r.provider.clone(), binding: r.binding.clone(), target: r.target.clone() } }
}
enum Request {
    Capture(CaptureRequest, oneshot::Sender<Result<CaptureResult>>),
    Acquire(String, oneshot::Sender<Result<()>>),
    Release(String, oneshot::Sender<Result<()>>),
}
#[derive(Default)]
struct Entries {
    actors: HashMap<Key, mpsc::Sender<Request>>,
    consumers: HashMap<String, (Key, Instant)>,
    failures: HashMap<Key, Failure>,
    consumer_locks: HashMap<String, Arc<Mutex<()>>>,
}
struct Failure { attempts: u32, until: Instant, error: TerminalError }
#[derive(Clone)]
pub struct TerminalPool {
    entries: Arc<Mutex<Entries>>,
    program: PathBuf,
    socket: Option<PathBuf>,
    limits: Limits,
}
impl Default for TerminalPool { fn default() -> Self { Self::new() } }
impl TerminalPool {
    pub fn new() -> Self { Self::with_program("tmux", None, Limits::default()) }
    pub fn with_program(program: impl Into<PathBuf>, socket: Option<PathBuf>, limits: Limits) -> Self {
        Self { entries: Arc::new(Mutex::new(Entries::default())), program: program.into(), socket, limits }
    }
    async fn consumer_lock(&self, consumer: &str) -> Result<Arc<Mutex<()>>> {
        let mut entries = self.entries.lock().await;
        entries.actors.retain(|_, tx| !tx.is_closed());
        let live: std::collections::HashSet<_> = entries.actors.keys().cloned().collect();
        entries.consumers.retain(|_, (key, renewed)| live.contains(key) && renewed.elapsed() < self.limits.lease);
        let Entries { consumers, consumer_locks, .. } = &mut *entries;
        consumer_locks.retain(|name, lock| consumers.contains_key(name) || Arc::strong_count(lock) > 1);
        if !consumer_locks.contains_key(consumer) && consumer_locks.len() >= MAX_CONSUMERS {
            return Err(TerminalError("too many terminal consumers"));
        }
        Ok(consumer_locks.entry(consumer.into()).or_insert_with(|| Arc::new(Mutex::new(()))).clone())
    }
    async fn enqueue(&self, request: &CaptureRequest, message: Request) -> Result<()> {
        validate(request)?;
        if cfg!(windows) { return Err(TerminalError("terminal control unavailable")); }
        // Migração e release do mesmo consumidor precisam conservar sua ordem sem prender outras sessões.
        let serial = self.consumer_lock(&request.consumer).await?;
        let _consumer = serial.lock().await;
        let key = Key::from(request);
        let mut entries = self.entries.lock().await;
        entries.actors.retain(|_, tx| !tx.is_closed());
        entries.failures.retain(|_, failure| Instant::now() < failure.until + self.limits.lease);
        if let Some(failure) = entries.failures.get(&key).filter(|failure| Instant::now() < failure.until) {
            return Err(failure.error.clone());
        }
        let live: std::collections::HashSet<_> = entries.actors.keys().cloned().collect();
        entries.consumers.retain(|_, (k, renewed)| live.contains(k) && renewed.elapsed() < self.limits.lease);
        if let Some(old) = entries.consumers.get(&request.consumer).filter(|(old, _)| *old != key).map(|(key, _)| key.clone()) {
            let receive = if let Some(tx) = entries.actors.get(&old) {
                let (reply, receive) = oneshot::channel();
                tx.try_send(Request::Release(request.consumer.clone(), reply)).map_err(|_| TerminalError("terminal queue full"))?;
                Some(receive)
            } else { None };
            drop(entries);
            if let Some(receive) = receive {
                timeout(self.limits.command + self.limits.startup, receive).await.map_err(|_| TerminalError("terminal release timeout"))?
                    .map_err(|_| TerminalError("terminal observer closed"))??;
            }
            entries = self.entries.lock().await;
            entries.actors.retain(|_, tx| !tx.is_closed());
            if let Some(failure) = entries.failures.get(&key).filter(|failure| Instant::now() < failure.until) {
                return Err(failure.error.clone());
            }
            entries.consumers.remove(&request.consumer);
        }
        if !entries.consumers.contains_key(&request.consumer) && entries.consumers.len() >= MAX_CONSUMERS { return Err(TerminalError("too many terminal consumers")); }
        if !entries.actors.contains_key(&key) {
            if entries.actors.len() >= MAX_ACTORS { return Err(TerminalError("too many terminal observers")); }
            if !entries.failures.contains_key(&key) && entries.actors.len() + entries.failures.len() >= MAX_CONSUMERS {
                return Err(TerminalError("too many terminal retries"));
            }
            let (tx, rx) = mpsc::channel(32);
            entries.actors.insert(key.clone(), tx);
            let (program, socket, limits) = (self.program.clone(), self.socket.clone(), self.limits.clone());
            let shared = self.entries.clone();
            tokio::spawn(async move { actor(program, socket, key, limits, rx, shared).await; });
        }
        let key = Key::from(request);
        entries.actors[&key].try_send(message).map_err(|_| TerminalError("terminal queue full"))?;
        entries.consumers.insert(request.consumer.clone(), (key, Instant::now()));
        Ok(())
    }
    pub async fn acquire(&self, request: CaptureRequest) -> Result<()> {
        let (reply, receive) = oneshot::channel();
        self.enqueue(&request, Request::Acquire(request.consumer.clone(), reply)).await?;
        timeout(self.limits.startup + self.limits.command * 4, receive).await.map_err(|_| TerminalError("terminal acquire timeout"))?
            .map_err(|_| TerminalError("terminal observer closed"))?
    }
    pub async fn capture(&self, request: CaptureRequest) -> Result<CaptureResult> {
        let (reply, receive) = oneshot::channel();
        self.enqueue(&request, Request::Capture(request.clone(), reply)).await?;
        timeout(self.limits.startup + self.limits.command * 6, receive).await.map_err(|_| TerminalError("terminal capture timeout"))?
            .map_err(|_| TerminalError("terminal observer closed"))?
    }
    pub async fn release(&self, consumer: &str) -> Result<()> {
        {
            let entries = self.entries.lock().await;
            if !entries.consumers.contains_key(consumer)
                && entries.consumer_locks.get(consumer).is_none_or(|lock| Arc::strong_count(lock) == 1) { return Ok(()); }
        }
        let serial = self.consumer_lock(consumer).await?;
        let _consumer = serial.lock().await;
        let (reply, receive) = oneshot::channel();
        {
            let mut entries = self.entries.lock().await;
            let tx = entries.consumers.get(consumer).and_then(|(k, _)| entries.actors.get(k)).filter(|tx| !tx.is_closed());
            if let Some(tx) = tx {
                tx.try_send(Request::Release(consumer.into(), reply)).map_err(|_| TerminalError("terminal queue full"))?;
            } else { entries.consumers.remove(consumer); return Ok(()); }
            entries.consumers.remove(consumer);
        }
        timeout(self.limits.command + self.limits.startup, receive).await.map_err(|_| TerminalError("terminal release timeout"))?
            .map_err(|_| TerminalError("terminal observer closed"))?
    }
}
fn validate(r: &CaptureRequest) -> Result<()> {
    let bounded = |s: &str, max: usize| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control);
    if !bounded(&r.consumer, 256) || !bounded(&r.binding, 1024) || !bounded(&r.target, 256) || !r.started.is_finite() || r.lines > 10000
        || !matches!(r.provider.as_str(), "claude" | "codex") || !bounded(&r.name, 64)
        || !r.name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
        return Err(TerminalError("invalid capture request"));
    }
    if !pane_id(&r.target) {
        let Some(rest) = r.target.strip_prefix(&format!("={}:" , r.name)) else { return Err(TerminalError("invalid terminal target")); };
        if rest.len() > 64 || !rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
            return Err(TerminalError("invalid terminal target"));
        }
    }
    Ok(())
}
fn quote(value: &str) -> String { format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$")) }

#[derive(Clone, PartialEq, Eq)]
struct PaneInfo { id: String, name: String, columns: usize, rows: usize, cursor_x: usize, cursor_y: usize, alternate: bool }
impl PaneInfo {
    fn parse(text: &str, name: &str) -> Result<Self> {
        let fields: Vec<_> = text.trim_end_matches('\n').split('\t').collect();
        if fields.len() != 7 || !pane_id(fields[0]) || fields[1] != name || !matches!(fields[6], "0" | "1") {
            return Err(TerminalError("invalid terminal metadata"));
        }
        let number = |index: usize| fields[index].parse::<usize>().map_err(|_| TerminalError("invalid terminal metadata"));
        let info = Self { id: fields[0].into(), name: fields[1].into(), columns: number(2)?, rows: number(3)?, cursor_x: number(4)?, cursor_y: number(5)?, alternate: fields[6] == "1" };
        if info.columns == 0 || info.rows == 0 || info.cursor_x > info.columns || info.cursor_y >= info.rows { return Err(TerminalError("invalid terminal cursor")); }
        Ok(info)
    }
}
struct Observer {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    parser: ControlParser,
    events: VecDeque<ControlEvent>,
    pane: Option<PaneInfo>,
    limits: Limits,
}
impl Observer {
    async fn spawn(program: PathBuf, socket: Option<PathBuf>, key: &Key, limits: Limits) -> Result<Self> {
        let mut command = Command::new(program);
        if let Some(socket) = socket { command.arg("-S").arg(socket); }
        command.args(["-u", "-C", "-N", "attach-session", "-E", "-f", "read-only,ignore-size,no-output", "-t"]).arg(format!("={}", key.name))
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| io_failure("cannot start terminal observer", e))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut observer = Self { child, stdin, stdout, parser: ControlParser::default(), events: VecDeque::new(), pane: None, limits };
        let initialized = timeout(observer.limits.startup, observer.frame()).await
            .map_err(|_| TerminalError("terminal startup timeout")).and_then(|r| r);
        if let Err(e) = initialized { observer.stop().await; return Err(e); }
        match observer.info(key).await {
            Ok(info) => observer.pane = Some(info),
            Err(e) => { observer.stop().await; return Err(e); }
        }
        Ok(observer)
    }
    fn event(&mut self, event: ControlEvent) -> Result<Option<(FrameIdentity, String)>> {
        match event {
            ControlEvent::Frame { identity, text, error: false } => Ok(Some((identity, text))),
            ControlEvent::Frame { error: true, .. } => Err(TerminalError("tmux command failed")),
            ControlEvent::Exit => Err(TerminalError("terminal observer exited")),

        }
    }
    async fn read(&mut self) -> Result<()> {
        let mut buffer = [0u8; 8192];
        let n = self.stdout.read(&mut buffer).await.map_err(|e| io_failure("terminal read failed", e))?;
        if n == 0 { self.parser.finish()?; return Err(TerminalError("terminal observer EOF")); }
        self.events.extend(self.parser.push(&buffer[..n])?);
        if self.events.len() > 4096 { return Err(TerminalError("terminal event queue full")); }
        Ok(())
    }
    async fn frame(&mut self) -> Result<(FrameIdentity, String)> {
        loop {
            while let Some(event) = self.events.pop_front() {
                if let Some(frame) = self.event(event)? { return Ok(frame); }
            }
            self.read().await?;
        }
    }
    async fn command(&mut self, command: String) -> Result<String> {
        while let Some(event) = self.events.pop_front() {
            if self.event(event)?.is_some() { return Err(TerminalError("unexpected control frame")); }
        }
        if self.parser.frame.is_some() { return Err(TerminalError("unexpected control frame")); }
        let nonce = format!("{}_{}", std::process::id(), NEXT_COMMAND.fetch_add(1, Ordering::Relaxed));
        let start = format!("HG_START_{nonce}");
        let end = format!("HG_END_{nonce}");
        let command = format!("display-message -p {start} ; {} ; display-message -p {end}\n", command.trim_end_matches('\n'));
        timeout(self.limits.command, async {
            self.stdin.write_all(command.as_bytes()).await.map_err(|e| io_failure("terminal write failed", e))?;
            self.stdin.flush().await.map_err(|e| io_failure("terminal write failed", e))?;
            let (first, text) = self.frame().await?;
            if text != format!("{start}\n") { return Err(TerminalError("terminal command marker mismatch")); }
            let (middle, body) = self.frame().await?;
            let (last, text) = self.frame().await?;
            if text != format!("{end}\n") || first.command >= middle.command || middle.command >= last.command
                || first.flags != middle.flags || middle.flags != last.flags {
                return Err(TerminalError("terminal command marker mismatch"));
            }
            Ok(body)
        }).await.map_err(|_| TerminalError("terminal command timeout"))?
    }
    async fn info(&mut self, key: &Key) -> Result<PaneInfo> {
        let text = self.command(format!("display-message -p -t {} \"#{{pane_id}}\\t#{{session_name}}\\t#{{pane_width}}\\t#{{pane_height}}\\t#{{cursor_x}}\\t#{{cursor_y}}\\t#{{alternate_on}}\"\n", quote(&key.target))).await?;
        let info = PaneInfo::parse(&text, &key.name)?;
        if self.pane.as_ref().is_some_and(|old| old.id != info.id) { return Err(TerminalError("terminal target changed")); }
        Ok(info)
    }
    async fn capture(&mut self, key: &Key, request: CaptureRequest) -> Result<CaptureResult> {
        let info = self.info(key).await?;
        self.pane = Some(info.clone());
        let text = self.command(format!("capture-pane -p {}{}-t {} -S -{}\n", if request.colors { "-e " } else { "" }, if request.join { "-J " } else { "" }, quote(&info.id), request.lines)).await?;
        let analysis = terminal_state::analyze(&text);
        Ok(CaptureResult { binding: request.binding, started: request.started, text, analysis })
    }
    async fn stop(&mut self) {
        self.pane = None; self.events.clear();
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
async fn record_failure(entries: &Mutex<Entries>, key: &Key, limits: &Limits, requests: &mut mpsc::Receiver<Request>, error: &TerminalError) {
    let mut entries = entries.lock().await;
    requests.close();
    let attempts = entries.failures.get(key).map_or(0, |failure| failure.attempts.saturating_add(1));
    let pause = limits.retry.saturating_mul(1 << attempts.min(5)).min(Duration::from_secs(60));
    entries.failures.insert(key.clone(), Failure { attempts, until: Instant::now() + pause, error: error.clone() });
}
async fn actor(program: PathBuf, socket: Option<PathBuf>, key: Key, limits: Limits, mut requests: mpsc::Receiver<Request>, entries: Arc<Mutex<Entries>>) {
    let mut observer = match Observer::spawn(program, socket, &key, limits.clone()).await {
        Ok(observer) => observer,
        Err(e) => { record_failure(&entries, &key, &limits, &mut requests, &e).await; while let Some(r) = requests.recv().await { reject(r, e.clone()); } return; }
    };
    let mut consumers: HashMap<String, Instant> = HashMap::new();
    let mut expiry = tokio::time::interval(limits.lease.min(Duration::from_secs(1)).max(Duration::from_millis(10)));
    expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failure = TerminalError("terminal observer closed");
    let mut failure_recorded = false;
    loop {
        while let Some(event) = observer.events.pop_front() {
            match observer.event(event) {
                Ok(None) => {},
                Ok(Some(_)) => { failure = TerminalError("unexpected control frame"); break; },
                Err(e) => { failure = e; break; },
            }
        }
        if failure.0 != "terminal observer closed" { break; }
        tokio::select! {
            _ = expiry.tick() => {
                consumers.retain(|_, renewed| renewed.elapsed() < limits.lease);
                if consumers.is_empty() && requests.is_empty() { break; }
            }
            request = requests.recv() => match request {
                Some(Request::Acquire(consumer, reply)) => { if reply.is_closed() { continue; } consumers.insert(consumer, Instant::now()); let _ = reply.send(Ok(())); }
                Some(Request::Capture(request, reply)) => {
                    if reply.is_closed() { continue; }
                    consumers.insert(request.consumer.clone(), Instant::now());
                    let result = observer.capture(&key, request).await;
                    if let Err(e) = &result {
                        failure = e.clone();
                        record_failure(&entries, &key, &limits, &mut requests, e).await;
                        failure_recorded = true;
                    } else { entries.lock().await.failures.remove(&key); }
                    let _ = reply.send(result);
                    if failure.0 != "terminal observer closed" { break; }
                }
                Some(Request::Release(consumer, reply)) => {
                    consumers.remove(&consumer);
                    if consumers.is_empty() { requests.close(); observer.stop().await; let _ = reply.send(Ok(())); break; }
                    let _ = reply.send(Ok(()));
                }
                None => break,
            },
            read = observer.read() => if let Err(e) = read { failure = e; break; },
        }
    }
    if !failure_recorded && failure.0 != "terminal observer closed" {
        record_failure(&entries, &key, &limits, &mut requests, &failure).await;
    }
    requests.close(); observer.stop().await;
    while let Some(request) = requests.recv().await { reject(request, failure.clone()); }
}
fn reject(request: Request, error: TerminalError) {
    match request {
        Request::Capture(_, reply) => { let _ = reply.send(Err(error)); },
        Request::Acquire(_, reply) | Request::Release(_, reply) => { let _ = reply.send(Err(error)); },
    }
}
