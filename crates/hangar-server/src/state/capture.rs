//! Captura avulsa do pane: um processo do multiplexador por quadro. Serve a lista (sessão sem
//! chat aberto não tem cliente `-C`) e o `Monitor` no Windows, onde o `-C` segue desligado.
use std::ffi::OsString;
use std::time::Duration;

use super::monitor::{CaptureFailed, Frame, PoolCapture};
use crate::terminal_control::TerminalPool;
use crate::terminal_state::analyze;

pub const TIMEOUT: Duration = Duration::from_secs(5);

/// O multiplexador chamado como processo: `capture-pane` e `has-session`.
pub struct MuxProcess { program: OsString, timeout: Duration }

/// Quadro da saída do `capture-pane`, não do código de retorno: o psmux sai com código ≠ 0 tendo
/// escrito o quadro. Sem saída, o código decide entre recusa e pane vazio. Byte que não decodifica
/// não é o pane; no Windows nem U+FFFD, que é o que o psmux põe no lugar do byte que ele não leu.
fn read_output(success: bool, stdout: &[u8]) -> Result<String, &'static str> {
    if stdout.is_empty() {
        return if success { Ok(String::new()) } else { Err("capture_refused") };
    }
    match std::str::from_utf8(stdout) {
        Ok(text) if !(cfg!(windows) && text.contains('\u{fffd}')) => Ok(text.to_owned()),
        _ => Err("capture_undecodable"),
    }
}

impl MuxProcess {
    pub fn new(program: impl Into<OsString>, timeout: Duration) -> Self { Self { program: program.into(), timeout } }

    async fn output(&self, args: &[&str]) -> Result<std::process::Output, &'static str> {
        let mut command = crate::terminal_input::child_command(&self.program);
        command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
        match tokio::time::timeout(self.timeout, command.output()).await {
            Err(_) => Err("capture_timeout"),
            Ok(Err(_)) => Err("capture_spawn_failed"),
            Ok(Ok(out)) => Ok(out),
        }
    }

    /// Só o `capture-pane`: quem chama decide se confere a sessão.
    pub async fn capture(&self, target: &str) -> Result<String, &'static str> {
        let out = self.output(&["capture-pane", "-p", "-t", target, "-S", "-200"]).await?;
        read_output(out.status.success(), &out.stdout)
    }

    /// `None` = o multiplexador não respondeu, que não é sessão morta.
    pub async fn has_session(&self, name: &str) -> Option<bool> {
        self.output(&["has-session", "-t", &format!("={name}")]).await.ok().map(|out| out.status.success())
    }

    /// Captura que, recusada ou vazia, separa a sessão que sumiu da que falhou: o psmux aceita
    /// comando que não executa, e o vazio de uma sessão morta não pode virar pane parado.
    pub async fn capture_checked(&self, name: &str, target: &str) -> Result<String, &'static str> {
        match self.capture(target).await {
            Err("capture_refused") => Err(match self.has_session(name).await {
                Some(false) => "capture_session_missing",
                Some(true) => "capture_refused",
                None => "capture_mux_no_answer",
            }),
            Ok(text) if text.is_empty() && self.has_session(name).await == Some(false) => Err("capture_session_missing"),
            other => other,
        }
    }
}

/// Fonte de quadro do `Monitor`: cliente `-C` em processo; no Windows, um `capture-pane` por rodada.
pub enum PaneCapture {
    Pool(PoolCapture),
    Process { mux: MuxProcess, target: String },
}

impl PaneCapture {
    pub fn new(pool: TerminalPool, program: impl Into<OsString>, name: &str, binding: &str, target: String) -> Self {
        // `terminal_control` recusa o `-C` no Windows.
        if cfg!(windows) { Self::process(program, target) } else { Self::Pool(PoolCapture::new(pool, name, binding, target)) }
    }

    /// Como `new`, com consumidor próprio no pool (captura avulsa que não pode soltar o do `Monitor`).
    pub fn with_consumer(pool: TerminalPool, consumer: String, program: impl Into<OsString>, name: &str, binding: &str, target: String) -> Self {
        if cfg!(windows) { Self::process(program, target) } else { Self::Pool(PoolCapture::with_consumer(pool, consumer, name, binding, target)) }
    }

    pub fn process(program: impl Into<OsString>, target: String) -> Self {
        Self::Process { mux: MuxProcess::new(program, TIMEOUT), target }
    }

    /// Falha do processo não tem tentativa guardada: cada rodada é uma tentativa nova.
    pub async fn capture(&self) -> Result<Frame, CaptureFailed> {
        match self {
            Self::Pool(pool) => pool.capture().await,
            Self::Process { mux, target } => match mux.capture(target).await {
                Ok(text) => Ok(Frame { analysis: analyze(&text), text }),
                Err(code) => Err(CaptureFailed { code: code.to_owned(), attempt: None }),
            },
        }
    }

    pub async fn release(&self) {
        if let Self::Pool(pool) = self { pool.release().await }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use hangar_api::state::StateEvent;
    use tokio::sync::Notify;

    use crate::state::facts::Dead;
    use crate::state::monitor::{FileFacts, Monitor, RoundFacts, Sources};

    const IDLE: &str = "● pronto\n────────────\n❯\n────────────\n🤖 Opus 4.5\n";

    /// Multiplexador falso: `capture-pane` escreve `frame` e sai com `capture_rc`; `has-session`
    /// sai com `has_rc` (`HANG`: não responde) e deixa uma linha em `calls`.
    struct Fake { dir: tempfile::TempDir, program: PathBuf }

    const HANG: i32 = -1;

    impl Fake {
        fn new(frame: &[u8], capture_rc: i32, has_rc: i32) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("frame");
            #[cfg(windows)]
            let calls = dir.path().join("calls");
            std::fs::write(&out, frame).unwrap();
            #[cfg(unix)]
            let program = {
                let path = dir.path().join("fake-tmux");
                std::fs::write(dir.path().join("capture-rc"), capture_rc.to_string()).unwrap();
                std::fs::write(dir.path().join("has-rc"), has_rc.to_string()).unwrap();
                // Spawns paralelos não podem herdar um descritor escritor do executável.
                std::os::unix::fs::symlink(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/mux_capture.sh"), &path).unwrap();
                path
            };
            #[cfg(windows)]
            let program = {
                let has_exit = if has_rc == HANG { "ping -n 6 127.0.0.1 >nul\r\n  exit /b 0".to_owned() } else { format!("exit /b {has_rc}") };
                let path = dir.path().join("fake-psmux.cmd");
                std::fs::write(&path, format!("@echo off\r\nif \"%1\"==\"has-session\" (\r\n  echo has>>\"{}\"\r\n  {has_exit}\r\n)\r\nif \"%1\"==\"capture-pane\" (\r\n  type \"{}\"\r\n  exit /b {capture_rc}\r\n)\r\nexit /b 99\r\n",
                    calls.display(), out.display())).unwrap();
                path
            };
            Self { dir, program }
        }
        fn mux(&self) -> MuxProcess { MuxProcess::new(&self.program, TIMEOUT) }
        fn has_calls(&self) -> usize {
            std::fs::read_to_string(self.dir.path().join("calls")).map_or(0, |s| s.lines().count())
        }
    }

    #[tokio::test]
    async fn psmux_reads_output_not_exit_code() {
        // O psmux sai com código ≠ 0 tendo escrito o quadro: vale o quadro, sem conferir a sessão.
        let fake = Fake::new(IDLE.as_bytes(), 1, 0);
        assert_eq!(fake.mux().capture("=s:").await.as_deref(), Ok(IDLE));
        assert_eq!(fake.mux().capture_checked("s", "=s:").await.as_deref(), Ok(IDLE));
        let list = crate::list::classify::MuxCapture::new(&fake.program, TIMEOUT, Default::default());
        use crate::list::classify::CaptureSource;
        assert_eq!(list.capture("s").await.map_err(|e| e.code).as_deref(), Ok(IDLE), "a lista lê pela mesma fonte");
        assert_eq!(fake.has_calls(), 0, "quadro bom não abre outro processo");
        // Sem saída e com código ≠ 0 é recusa, não pane vazio.
        let refused = Fake::new(b"", 1, 0);
        assert_eq!(refused.mux().capture("=s:").await, Err("capture_refused"));
        // Sem saída e código 0 é o pane vazio; o `Monitor` confere a sessão no quadro vazio.
        let empty = Fake::new(b"", 0, 0);
        assert_eq!(empty.mux().capture("=s:").await.as_deref(), Ok(""));
    }

    #[tokio::test]
    async fn replacement_char_frame_not_good() {
        let mut bytes = IDLE.as_bytes().to_vec();
        bytes.extend_from_slice(b"\xff\xfe torto\n");
        let fake = Fake::new(&bytes, 0, 0);
        assert_eq!(fake.mux().capture("=s:").await, Err("capture_undecodable"));
        let list = crate::list::classify::MuxCapture::new(&fake.program, TIMEOUT, Default::default());
        use crate::list::classify::CaptureSource;
        assert_eq!(list.capture("s").await.map_err(|e| e.code), Err("capture_undecodable"));
        let failed = PaneCapture::process(&fake.program, "=s:".into()).capture().await.unwrap_err();
        assert_eq!((failed.code.as_str(), failed.attempt), ("capture_undecodable", None));
        // U+FFFD bem formado é texto do pane fora do Windows; lá é o byte que o psmux não leu.
        let shown = format!("{IDLE}� no log\n");
        let fake = Fake::new(shown.as_bytes(), 0, 0);
        let want = if cfg!(windows) { Err("capture_undecodable") } else { Ok(shown.clone()) };
        assert_eq!(fake.mux().capture("=s:").await, want);
    }

    #[tokio::test]
    async fn missing_vs_failed_by_has_session() {
        let gone = Fake::new(b"", 1, 1);
        assert_eq!(gone.mux().capture_checked("s", "=s:").await, Err("capture_session_missing"));
        assert_eq!(gone.has_calls(), 1);
        let failed = Fake::new(b"", 1, 0);
        assert_eq!(failed.mux().capture_checked("s", "=s:").await, Err("capture_refused"));
        let list = crate::list::classify::MuxCapture::new(&gone.program, TIMEOUT, Default::default());
        use crate::list::classify::CaptureSource;
        assert_eq!(list.capture("s").await.map_err(|e| e.code), Err("capture_session_missing"));
        assert_eq!(gone.mux().has_session("s").await, Some(false));
        assert_eq!(failed.mux().has_session("s").await, Some(true));
        assert_eq!(MuxProcess::new("/nonexistent/psmux", TIMEOUT).has_session("s").await, None, "sem resposta não é sessão morta");
        // Vazio com código 0 também confere: o psmux aceita comando que não executa.
        let vanished = Fake::new(b"", 0, 1);
        assert_eq!(vanished.mux().capture_checked("s", "=s:").await, Err("capture_session_missing"));
        let blank = Fake::new(b"", 0, 0);
        assert_eq!(blank.mux().capture_checked("s", "=s:").await.as_deref(), Ok(""));
        // Recusa com o multiplexador calado no `has-session` não é recusa conferida.
        let silent = Fake::new(b"", 1, HANG);
        let mux = MuxProcess::new(&silent.program, Duration::from_millis(500));
        assert_eq!(mux.capture_checked("s", "=s:").await, Err("capture_mux_no_answer"));
    }

    #[test]
    fn monitor_windows_uses_subprocess() {
        let capture = PaneCapture::new(TerminalPool::new(), "tmux", "s", "sid", "=s:".into());
        assert_eq!(matches!(capture, PaneCapture::Process { .. }), cfg!(windows), "-C só fora do Windows");
    }

    struct Src { capture: PaneCapture, events: Mutex<Vec<StateEvent>>, wake: Arc<Notify> }

    impl Sources for Arc<Src> {
        fn name(&self) -> &str { "s" }
        fn sid(&self) -> Option<String> { Some("sid".into()) }
        fn epoch(&self) -> u64 { 0 }
        fn wake(&self) -> Arc<Notify> { self.wake.clone() }
        async fn facts(&self) -> RoundFacts { RoundFacts::default() }
        async fn capture(&self) -> Result<Frame, CaptureFailed> { self.capture.capture().await }
        async fn has_session(&self) -> Option<bool> { Some(true) }
        async fn dead(&self) -> Result<Dead, String> { Ok(Dead::Ok) }
        async fn observe_permission(&self, _: &str, mode: &str) -> Result<(String, String), String> { Ok((mode.into(), "manual".into())) }
        async fn files(&self, _: Option<&str>) -> FileFacts { FileFacts::default() }
        async fn publish(&self, event: StateEvent) -> bool { self.events.lock().unwrap().push(event); true }
        fn hub_wake(&self) -> Arc<Notify> { Arc::default() }
        async fn emit(&self, _: &'static str, _: serde_json::Value) -> bool { true }
        fn runtime_wake(&self) -> Arc<Notify> { Arc::default() }
        fn runtime_problem(&self) -> Option<(String, String)> { None }
        async fn ask_payload(&self) -> Result<Option<hangar_api::ask::AskQuestion>, String> { Ok(None) }
        fn deliverable(&self) {}
        async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { None }
        async fn preview_files(&self, _: &str) -> Vec<crate::state::preview::HookFile> { Vec::new() }
        fn committed(&self) -> Option<Arc<str>> { None }
        async fn publish_preview(&self, _: hangar_api::preview::PreviewEvent) -> bool { true }
        fn wall(&self) -> f64 { crate::state::monitor::wall_now() }
    }

    #[tokio::test]
    async fn monitor_reduces_subprocess_frame() {
        // O psmux sai com código ≠ 0: o `Monitor` reduz o quadro, sem problema de observação.
        let fake = Fake::new(IDLE.as_bytes(), 1, 0);
        let src = Arc::new(Src { capture: PaneCapture::process(&fake.program, "=s:".into()), events: Mutex::default(), wake: Arc::default() });
        let task = tokio::spawn(Monitor::new(src.clone()).run());
        let event = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(e) = src.events.lock().unwrap().first().cloned() { return e; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("o Monitor publicou");
        task.abort();
        assert_eq!((event.state.as_str(), event.problema), ("idle", None));
    }
}
