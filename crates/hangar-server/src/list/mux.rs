//! Panes do multiplexador (`backend/app/tmux.py:list_panes_all`).
use std::ffi::OsString;
use std::time::Duration;

use super::discover::DiscoveryProblem;

/// Os 8 campos de `list_panes_all` e o endereço do pane no psmux, numa chamada só.
pub const LIST_PANES_FORMAT: &str = "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}\t#{@cp_hidden}\t#{CP_PROVIDER}\t#{session_created}\t#{window_index}\t#{pane_index}\t#{pane_dead}";
const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "omp", "kimi"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    pub session: String,
    pub active: bool,
    pub pid: Option<u32>,
    pub cwd: String,
    pub pane_id: String,
    pub hidden: bool,
    pub provider: Option<String>,
    pub session_created: Option<u64>,
    pub window_index: Option<u32>,
    pub pane_index: Option<u32>,
    pub dead: bool,
}

impl Pane {
    /// Endereço que mira este pane. O psmux numera `%N` por SESSÃO, e `-t %N` lá cai na sessão de
    /// quem chama (`tmux.py:alvo_de_pane`); `None` = sem alvo preciso, quem chama usa `=<sessão>:`.
    pub fn target(&self) -> Option<String> {
        if cfg!(windows) { self.psmux_target() } else { Some(self.pane_id.clone()) }
    }

    fn psmux_target(&self) -> Option<String> {
        Some(format!("={}:{}.{}", self.session, self.window_index?, self.pane_index?))
    }
}

/// O multiplexador não respondeu: a lista é desconhecida, não vazia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MuxUnavailable { pub code: &'static str }

impl std::fmt::Display for MuxUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.code) }
}
impl std::error::Error for MuxUnavailable {}

/// Uma linha por pane, e quantas linhas não vazias ficaram de fora por terem menos de 5 campos
/// (cwd com `\n` parte o pane em dois). Os demais campos podem faltar (opção de usuário que o
/// multiplexador não interpola): faltando, a sessão aparece como sempre.
pub fn parse_list_panes(output: &str) -> (Vec<Pane>, usize) {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut dropped = 0;
    let panes = output.lines().filter_map(|line| {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 5 {
            dropped += usize::from(!line.trim().is_empty());
            return None;
        }
        let field = |i: usize| parts.get(i).copied().unwrap_or("");
        Some(Pane {
            session: parts[0].to_string(),
            active: parts[1] == "1",
            pid: if digits(parts[2]) { parts[2].parse().ok() } else { None },
            cwd: live_cwd(parts[3]),
            pane_id: parts[4].to_string(),
            hidden: field(5) == "1",
            provider: PROVIDERS.contains(&field(6)).then(|| field(6).to_string()),
            session_created: if digits(field(7)) { field(7).parse().ok().filter(|n| *n > 0) } else { None },
            window_index: if digits(field(8)) { field(8).parse().ok() } else { None },
            pane_index: if digits(field(9)) { field(9).parse().ok() } else { None },
            dead: field(10) == "1",
        })
    }).collect();
    (panes, dropped)
}

/// Pasta apagada com o processo dentro: o Linux devolve o caminho com " (deleted)" no fim, e com
/// ele a pasta da conversa (`projects/<cwd>`) não é achada.
fn live_cwd(cwd: &str) -> String {
    match cwd.strip_suffix(" (deleted)") {
        Some(base) if !std::path::Path::new(cwd).exists() => base.to_string(),
        _ => cwd.to_string(),
    }
}

/// Recusa que quer dizer "não há sessão", não "não sei" (`tmux.py:_run`, `absent`). Fica de fora o
/// `invalid option: @cp_hidden`: é o formato recusado, e a lista seria desconhecida.
pub(crate) fn is_absence(stderr: &str) -> bool {
    // O psmux põe o próprio nome na frente da mensagem.
    let stderr = stderr.strip_prefix("psmux: ").unwrap_or(stderr);
    ["no server", "no sessions", "can't find session", "no such session"]
        .iter().any(|p| stderr.starts_with(p))
        || (stderr.starts_with("error connecting to ") && stderr.trim_end().ends_with("(No such file or directory)"))
}

pub struct Mux { program: OsString, timeout: Duration, direct: Option<Direct> }

/// Leitura direta do psmux (Windows): vale só para os servidores que uma comparação com o CLI
/// conferiu; servidor novo, recriado ou falha voltam ao CLI.
struct Direct { dir: std::path::PathBuf, gate: std::sync::Mutex<Gate> }

#[derive(Clone)]
enum Gate {
    /// Versão do programa ainda não lida.
    Unchecked,
    /// Comparações que não bateram (ou leituras que falharam) desde a última conferência.
    Trying(u8),
    Verified(Vec<hangar_psmux::Server>),
    Off,
}

/// Parte do prazo do `list-panes` que a leitura direta pode gastar; o resto fica para o CLI.
const DIRECT_BUDGET: Duration = Duration::from_secs(1);
const DIRECT_TRIES: u8 = 3;

enum DirectRead { Served(String), Compare(hangar_psmux::Listing), Skip }

impl Direct {
    fn gate(&self) -> Gate { self.gate.lock().unwrap_or_else(|e| e.into_inner()).clone() }
    fn set(&self, gate: Gate) { *self.gate.lock().unwrap_or_else(|e| e.into_inner()) = gate; }
    /// Uma tentativa sem conferência; a terceira seguida desliga o direto nesta vida do servidor.
    fn missed(&self, code: &'static str) {
        let tries = match self.gate() { Gate::Trying(n) => n + 1, _ => 1 };
        if tries < DIRECT_TRIES { self.set(Gate::Trying(tries)); return; }
        self.set(Gate::Off);
        tracing::warn!(code, "psmux: leitura direta desligada; a lista segue pelo CLI");
    }
}

impl Default for Mux {
    fn default() -> Self { Self::with_program("tmux", Duration::from_secs(5)) }
}

impl Mux {
    pub fn with_program(program: impl Into<OsString>, timeout: Duration) -> Self {
        let dir = if cfg!(windows) { hangar_psmux::data_dir(|name| std::env::var_os(name)) } else { None };
        match dir {
            Some(dir) => Self::with_direct(program, timeout, dir),
            None => Self { program: program.into(), timeout, direct: None },
        }
    }

    /// Como `with_program`, lendo os servidores do psmux em `dir` direto quando o portão aceitar.
    pub fn with_direct(program: impl Into<OsString>, timeout: Duration, dir: std::path::PathBuf) -> Self {
        Self { program: program.into(), timeout, direct: Some(Direct { dir, gate: std::sync::Mutex::new(Gate::Unchecked) }) }
    }

    /// Valor de uma variável no ambiente da sessão (`show-environment`); ausente ou erro = `None`.
    pub async fn session_env(&self, session: &str, var: &str) -> Option<String> {
        // O psmux não aceita o `=` de casamento exato nesse comando.
        let target = if cfg!(windows) { session.to_owned() } else { format!("={session}") };
        let mut command = crate::terminal_input::child_command(&self.program);
        command.args(["show-environment", "-t", &target, var]).stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(self.timeout, command.output()).await.ok()?.ok()?;
        if !out.status.success() { return None; }
        let prefix = format!("{var}=");
        String::from_utf8_lossy(&out.stdout).lines().find_map(|line| line.trim().strip_prefix(prefix.as_str()).map(str::to_owned))
    }

    /// `tmux -V` (o psmux responde ao mesmo); `None` sem programa, no prazo estourado ou na recusa.
    pub async fn version(&self) -> Option<String> {
        self.version_until(tokio::time::Instant::now() + self.timeout).await
    }

    async fn version_until(&self, deadline: tokio::time::Instant) -> Option<String> {
        let mut command = crate::terminal_input::child_command(&self.program);
        command.arg("-V").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout_at(deadline, command.output()).await.ok()?.ok()?;
        if !out.status.success() { return None; }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|v| !v.is_empty())
    }

    pub async fn list_panes(&self) -> Result<Vec<Pane>, MuxUnavailable> {
        self.list_panes_checked().await.map(|(panes, _)| panes)
    }

    /// Os panes e, se linhas do `list-panes` ficaram de fora, o problema para o diário: a sessão
    /// delas some da lista enquanto a lista em si responde.
    pub async fn list_panes_checked(&self) -> Result<(Vec<Pane>, Option<DiscoveryProblem>), MuxUnavailable> {
        // Um prazo só para a leitura direta e o CLI que a confere ou substitui.
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut unverified = None;
        if let Some(direct) = &self.direct {
            match self.read_direct(direct, deadline).await {
                DirectRead::Served(text) => return panes_of(&text),
                DirectRead::Compare(listing) => unverified = Some(listing),
                DirectRead::Skip => {}
            }
        }
        let mut command = crate::terminal_input::child_command(&self.program);
        command.args(["list-panes", "-a", "-F", LIST_PANES_FORMAT])
            .stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = match tokio::time::timeout_at(deadline, command.output()).await {
            Err(_) => return Err(unavailable("mux_timeout", None, None)),
            // Sem o programa não há sessão de terminal nenhuma; quem só pergunta por nome lê assim.
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return Err(unavailable("mux_missing", Some(&error), None)),
            Ok(Err(error)) => return Err(unavailable("mux_spawn_failed", Some(&error), None)),
            Ok(Ok(out)) => out,
        };
        if let (Some(direct), Some(listing)) = (&self.direct, unverified) {
            // Conferido byte a byte, e só para estes servidores: outro `-L` ou outra pasta não bate.
            if out.status.success() && !listing.servers.is_empty() && listing.text.as_bytes() == out.stdout.as_slice() {
                direct.set(Gate::Verified(listing.servers));
            } else {
                direct.missed("psmux_direct_mismatch");
            }
        }
        if out.status.success() {
            return panes_of(&String::from_utf8_lossy(&out.stdout));
        }
        // O Python lia qualquer recusa como "zero sessões"; só a ausência conhecida vira vazio.
        if out.status.code() == Some(1) && is_absence(&String::from_utf8_lossy(&out.stderr)) {
            Ok((Vec::new(), None))
        } else {
            Err(unavailable("mux_refused", None, out.status.code()))
        }
    }

    async fn read_direct(&self, direct: &Direct, deadline: tokio::time::Instant) -> DirectRead {
        let mut gate = direct.gate();
        if matches!(gate, Gate::Unchecked) {
            // É a versão do programa chamado; a dos servidores vivos fica provada só pela comparação.
            let known = self.version_until(deadline).await.is_some_and(|v| hangar_psmux::known_version(&v));
            gate = if known { Gate::Trying(0) } else { Gate::Off };
            direct.set(gate.clone());
            if !known { tracing::warn!(code = "psmux_direct_version", "psmux: versão do programa não conferida; a lista segue pelo CLI"); }
        }
        if matches!(gate, Gate::Off) { return DirectRead::Skip; }
        let budget = DIRECT_BUDGET.min(deadline.saturating_duration_since(tokio::time::Instant::now()) / 2);
        let until = std::time::Instant::now() + budget;
        let dir = direct.dir.clone();
        let read = tokio::time::timeout(budget + Duration::from_millis(50),
            tokio::task::spawn_blocking(move || hangar_psmux::list_panes(&dir, LIST_PANES_FORMAT, until))).await;
        let code = match read {
            // Sem servidor no registro, a ausência (ou o registro sujo) é resposta do CLI.
            Ok(Ok(Ok(listing))) if listing.servers.is_empty() => return DirectRead::Skip,
            Ok(Ok(Ok(listing))) => return match gate {
                Gate::Verified(servers) if servers == listing.servers => DirectRead::Served(listing.text),
                _ => DirectRead::Compare(listing),
            },
            Ok(Ok(Err(failure))) => failure.code,
            Ok(Err(_)) => "psmux_direct_panicked",
            Err(_) => "psmux_direct_timeout",
        };
        if crate::warn_limit::allow(None, code) {
            tracing::warn!(code, "psmux: leitura direta falhou; o pedido segue pelo CLI");
        }
        direct.missed(code);
        DirectRead::Skip
    }
}

/// Saída do `list-panes`, do CLI ou da leitura direta, que deram os mesmos bytes ao serem conferidas.
fn panes_of(stdout: &str) -> Result<(Vec<Pane>, Option<DiscoveryProblem>), MuxUnavailable> {
    let (panes, dropped) = parse_list_panes(stdout);
    if panes.is_empty() && !stdout.trim().is_empty() {
        return Err(unavailable("mux_unparsed", None, None));
    }
    let mut problems = super::discover::Problems::default();
    if dropped > 0 {
        problems.note("mux_unparsed", "list-panes", "linha do list-panes com menos de 5 campos; sessão fora da lista");
    }
    Ok((panes, problems.into_vec().pop()))
}

/// Só código, tipo de erro e retorno: o stderr pode repetir o argv.
fn unavailable(code: &'static str, error: Option<&std::io::Error>, exit: Option<i32>) -> MuxUnavailable {
    if crate::warn_limit::allow(None, code) {
        tracing::warn!(code, io_kind = ?error.map(|e| e.kind()), os_error = ?error.and_then(|e| e.raw_os_error()),
            exit = ?exit, "list-panes sem resposta");
    }
    MuxUnavailable { code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_panes_fields() {
        let out = "main\t1\t100\t/home/a b\t%0\t\tclaude\t1700000000\n\
                   main\t0\t101\t/home\t%1\t\t\t1700000000\n\
                   v1.2\t1\t200\t/srv\t%2\t1\tpi\t0\n\
                   old\t1\tx\t/o\t%3\n\
                   raw\t1\t300\t/r\t%4\t#{@cp_hidden}\tbash\t\n\
                   short\t1\t5\n\n";
        let (panes, dropped) = parse_list_panes(out);
        assert_eq!(panes.len(), 5);
        assert_eq!(dropped, 1, "a linha curta conta; a vazia não");
        assert_eq!(panes[0], Pane { session: "main".into(), active: true, pid: Some(100), cwd: "/home/a b".into(),
            pane_id: "%0".into(), hidden: false, provider: Some("claude".into()), session_created: Some(1700000000),
            ..Pane::default() });
        assert!(!panes[1].active);
        assert_eq!(panes[1].provider, None);
        assert_eq!(panes[2].session, "v1.2");
        assert!(panes[2].hidden);
        assert_eq!(panes[2].provider.as_deref(), Some("pi"));
        assert_eq!(panes[2].session_created, None, "zero não é nascimento");
        assert_eq!((panes[3].pid, panes[3].hidden, &panes[3].provider, panes[3].session_created), (None, false, &None, None));
        assert_eq!((panes[4].hidden, &panes[4].provider), (false, &None), "campo cru e provedor desconhecido");
    }

    #[test]
    fn deleted_cwd_keeps_the_original_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("x (deleted)");
        std::fs::create_dir(&real).unwrap();
        let out = format!("a\t1\t1\t/sumiu/wt-PM-1 (deleted)\t%0\nb\t1\t2\t{}\t%1\n", real.display());
        let (panes, _) = parse_list_panes(&out);
        assert_eq!(panes[0].cwd, "/sumiu/wt-PM-1");
        assert_eq!(panes[1].cwd, real.display().to_string(), "pasta que existe com esse nome fica");
    }

    #[test]
    fn dead_pane_is_marked() {
        let (panes, _) = parse_list_panes("morto\t1\t\t\t%9\t\t\t1\t0\t0\t1\nvivo\t1\t5\t/w\t%1\t\t\t1\t0\t0\t0\n");
        assert_eq!((panes[0].dead, panes[1].dead), (true, false));
    }

    #[test]
    fn parses_psmux_list_panes() {
        // psmux numera `%N` por sessão: as duas têm `%1`, e só `=<sessão>:<janela>.<pane>` mira certo.
        let out = b"zzX\t1\t40\tC:\\w\t%1\t\tclaude\t1700000000\t0\t0\n\
                    zzY\t1\t41\tC:\\S\xe3o\t%1\t\tpi\t1700000001\t2\t1\n\
                    zzZ\t1\t42\tC:\\z\t%1\n";
        let (panes, _) = parse_list_panes(&String::from_utf8_lossy(out));
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].psmux_target().as_deref(), Some("=zzX:0.0"));
        assert_eq!(panes[1].psmux_target().as_deref(), Some("=zzY:2.1"));
        assert_eq!(panes[1].cwd, "C:\\S\u{fffd}o", "byte inválido trocado, pane mantido");
        assert_eq!(panes[2].psmux_target(), None, "sem janela/pane não há alvo preciso");
        assert_eq!(panes[0].target().as_deref(), Some(if cfg!(windows) { "=zzX:0.0" } else { "%1" }));
    }

    #[cfg(unix)]
    fn script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake-tmux");
        crate::write_test_executable(&path, format!("#!/bin/sh\n{body}\n"));
        (dir, path)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_is_unavailable() {
        let (_dir, path) = script("sleep 5");
        let mux = Mux::with_program(&path, Duration::from_millis(200));
        assert_eq!(mux.list_panes().await, Err(MuxUnavailable { code: "mux_timeout" }));
        let missing = Mux::with_program("/nonexistent/tmux", Duration::from_secs(1));
        assert_eq!(missing.list_panes().await, Err(MuxUnavailable { code: "mux_missing" }));
    }

    /// Sem sessão, o psmux responde com o próprio nome na frente.
    #[test]
    fn the_psmux_absence_is_an_absence() {
        assert!(is_absence("psmux: no server running on session 'default'\n"));
        assert!(!is_absence("psmux: unknown command\n"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn absence_is_empty_and_other_refusal_is_unavailable() {
        let (_d1, none) = script("echo 'no server running on /tmp/tmux-1000/default' >&2; exit 1");
        assert_eq!(Mux::with_program(&none, Duration::from_secs(2)).list_panes().await, Ok(Vec::new()));
        let (_d2, odd) = script("echo 'unknown failure' >&2; exit 1");
        assert_eq!(Mux::with_program(&odd, Duration::from_secs(2)).list_panes().await,
            Err(MuxUnavailable { code: "mux_refused" }));
        let (_d4, hidden) = script("echo 'invalid option: @cp_hidden' >&2; exit 1");
        assert_eq!(Mux::with_program(&hidden, Duration::from_secs(2)).list_panes().await,
            Err(MuxUnavailable { code: "mux_refused" }), "formato recusado não é ausência");
        let (_d5, signal) = script("echo 'no server' >&2; exit 2");
        assert_eq!(Mux::with_program(&signal, Duration::from_secs(2)).list_panes().await,
            Err(MuxUnavailable { code: "mux_refused" }), "ausência só com retorno 1");
        let (_d6, garbage) = script("echo 'garbage'");
        assert_eq!(Mux::with_program(&garbage, Duration::from_secs(2)).list_panes().await,
            Err(MuxUnavailable { code: "mux_unparsed" }));
        let (_d3, ok) = script("printf 'a\\t1\\t7\\t/w\\t%%0\\t\\tcodex\\t5\\n'");
        let (panes, problem) = Mux::with_program(&ok, Duration::from_secs(2)).list_panes_checked().await.unwrap();
        assert_eq!(panes[0].provider.as_deref(), Some("codex"));
        assert_eq!(problem, None);
        // cwd com `\n`: o pane parte em dois, a cabeça cai e o resto não parece pane.
        let (_d7, split) = script("printf 'b\\t1\\t8\\t/w/a\\nb\\t%%1\\n' ; printf 'c\\t1\\t9\\t/c\\t%%2\\n'");
        let (panes, problem) = Mux::with_program(&split, Duration::from_secs(2)).list_panes_checked().await.unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(problem.map(|p| p.code), Some("mux_unparsed"), "descarte parcial vai ao diário");
    }

    /// Servidor de sessão do psmux falso: conta conexões e responde como o real, ou se cala.
    struct FakeServer { port: u16, conns: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        silent: std::sync::Arc<std::sync::atomic::AtomicBool>, stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>> }
    impl FakeServer {
        fn start(key: &'static str, reply: String) -> Self {
            use std::io::{BufRead, Write};
            use std::sync::atomic::Ordering::SeqCst;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (conns, silent, stop) = (std::sync::Arc::default(), std::sync::Arc::default(), std::sync::Arc::default());
            let (c, s, st): (std::sync::Arc<std::sync::atomic::AtomicUsize>, std::sync::Arc<std::sync::atomic::AtomicBool>, std::sync::Arc<std::sync::atomic::AtomicBool>) =
                (std::sync::Arc::clone(&conns), std::sync::Arc::clone(&silent), std::sync::Arc::clone(&stop));
            let handle = std::thread::spawn(move || while !st.load(SeqCst) {
                let Ok((stream, _)) = listener.accept() else { std::thread::sleep(Duration::from_millis(2)); continue };
                c.fetch_add(1, SeqCst);
                stream.set_nonblocking(false).unwrap();
                if s.load(SeqCst) { std::thread::sleep(Duration::from_millis(700)); continue; }
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let (mut auth, mut cmd) = (String::new(), String::new());
                reader.read_line(&mut auth).unwrap();
                let mut out = stream;
                if auth.trim() != format!("AUTH {key}") { out.write_all(b"ERROR: Invalid session key\n").unwrap(); continue; }
                out.write_all(b"OK\n").unwrap();
                reader.read_line(&mut cmd).unwrap();
                out.write_all(reply.as_bytes()).unwrap();
            });
            Self { port, conns, silent, stop, handle: Some(handle) }
        }
        fn conns(&self) -> usize { self.conns.load(std::sync::atomic::Ordering::SeqCst) }
        fn go_silent(&self) { self.silent.store(true, std::sync::atomic::Ordering::SeqCst); }
        fn register(&self, dir: &std::path::Path, base: &str, key: &str) {
            std::fs::write(dir.join(format!("{base}.port")), format!("{}\n", self.port)).unwrap();
            std::fs::write(dir.join(format!("{base}.key")), format!("{key}\n")).unwrap();
        }
    }
    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = self.handle.take() { handle.join().unwrap(); }
        }
    }

    const LINE_A: &str = "a\t1\t7\t/w\t%0\t\tcodex\t5\t0\t0\t0\n";
    const LINE_B: &str = "b\t1\t8\t/v\t%0\t\tclaude\t6\t0\t0\t0\n";

    /// `tmux` falso: `-V` responde `version`, `list-panes` conta a chamada e imprime `listing`.
    struct FakeCli { _dir: tempfile::TempDir, path: std::path::PathBuf, data: std::path::PathBuf, count: std::path::PathBuf, listing: std::path::PathBuf }
    impl FakeCli {
        fn new(version: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (data, count, listing) = (dir.path().join("data"), dir.path().join("count"), dir.path().join("listing"));
            std::fs::create_dir(&data).unwrap();
            std::fs::write(&listing, "").unwrap();
            let path = dir.path().join("fake-tmux");
            crate::write_test_executable(&path, format!("#!/bin/sh\ncase \"$1\" in -V) echo '{version}' ;; list-panes) echo x >> '{}' ; cat '{}' ;; esac\n",
                count.display(), listing.display()));
            Self { _dir: dir, path, data, count, listing }
        }
        fn lists(&self, text: &str) { std::fs::write(&self.listing, text).unwrap(); }
        fn calls(&self) -> usize { std::fs::read_to_string(&self.count).map_or(0, |t| t.lines().count()) }
        fn mux(&self, timeout: Duration) -> Mux { Mux::with_direct(&self.path, timeout, self.data.clone()) }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_reading_starts_after_matching_the_cli_and_then_skips_it() {
        let cli = FakeCli::new(hangar_psmux::VERSIONS[1]);
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_secs(2));
        let first = mux.list_panes().await.unwrap();
        assert_eq!((cli.calls(), a.conns()), (1, 1), "a primeira leitura confere o direto contra o CLI");
        let second = mux.list_panes().await.unwrap();
        assert_eq!((cli.calls(), a.conns()), (1, 2), "conferido, o CLI não é aberto");
        assert_eq!(first, second);
        assert_eq!(second[0].session, "a");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_two_line_version_the_real_psmux_prints_is_recognized() {
        let real = hangar_psmux::VERSIONS[0].replacen(" psmux", "\npsmux", 1);
        let cli = FakeCli::new(&real);
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_secs(2));
        for _ in 0..2 { mux.list_panes().await.unwrap(); }
        assert_eq!((cli.calls(), a.conns()), (1, 2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unknown_version_never_connects() {
        let cli = FakeCli::new("tmux 9.9 psmux 9.9.9 (0000000 2099-01-01)");
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_secs(2));
        for _ in 0..2 { mux.list_panes().await.unwrap(); }
        assert_eq!((cli.calls(), a.conns()), (2, 0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_or_recreated_server_needs_a_new_comparison() {
        let cli = FakeCli::new(hangar_psmux::VERSIONS[0]);
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_secs(2));
        mux.list_panes().await.unwrap();
        mux.list_panes().await.unwrap();
        assert_eq!(cli.calls(), 1);
        let b = FakeServer::start("kb", LINE_B.into());
        b.register(&cli.data, "b", "kb");
        cli.lists(&format!("{LINE_A}{LINE_B}"));
        let both = mux.list_panes().await.unwrap();
        assert_eq!(both.len(), 2);
        assert_eq!(cli.calls(), 2, "servidor novo não herda a conferência antiga");
        mux.list_panes().await.unwrap();
        assert_eq!(cli.calls(), 2);
        // Mesmo nome, outra porta e outra chave: outra vida.
        let b2 = FakeServer::start("kb2", LINE_B.into());
        b2.register(&cli.data, "b", "kb2");
        mux.list_panes().await.unwrap();
        assert_eq!(cli.calls(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_mismatch_keeps_the_cli_and_gives_up_after_three_tries() {
        let cli = FakeCli::new(hangar_psmux::VERSIONS[1]);
        let a = FakeServer::start("ka", LINE_B.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_secs(2));
        for _ in 0..4 { assert_eq!(mux.list_panes().await.unwrap()[0].session, "a"); }
        assert_eq!((cli.calls(), a.conns()), (4, 3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn no_sessions_is_the_cli_answer_and_does_not_disable_direct_reading() {
        let cli = FakeCli::new(hangar_psmux::VERSIONS[1]);
        let mux = cli.mux(Duration::from_secs(2));
        for _ in 0..4 { assert_eq!(mux.list_panes().await.unwrap(), Vec::new()); }
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        mux.list_panes().await.unwrap();
        mux.list_panes().await.unwrap();
        assert_eq!(a.conns(), 2, "sem sessão não é falha da leitura direta");
        assert_eq!(cli.calls(), 5);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_server_falls_back_to_the_cli_within_the_deadline() {
        let cli = FakeCli::new(hangar_psmux::VERSIONS[1]);
        let a = FakeServer::start("ka", LINE_A.into());
        a.register(&cli.data, "a", "ka");
        cli.lists(LINE_A);
        let mux = cli.mux(Duration::from_millis(400));
        mux.list_panes().await.unwrap();
        a.go_silent();
        let started = std::time::Instant::now();
        let panes = mux.list_panes().await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(450), "levou {:?}", started.elapsed());
        assert_eq!((panes[0].session.as_str(), cli.calls()), ("a", 2), "o pedido cai no CLI");
    }
}
