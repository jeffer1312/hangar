//! Panes do multiplexador (`backend/app/tmux.py:list_panes_all`).
use std::ffi::OsString;
use std::time::Duration;

use super::discover::DiscoveryProblem;

/// Os 8 campos de `list_panes_all` e o endereço do pane no psmux, numa chamada só.
pub const LIST_PANES_FORMAT: &str = "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}\t#{@cp_hidden}\t#{CP_PROVIDER}\t#{session_created}\t#{window_index}\t#{pane_index}";
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
            cwd: parts[3].to_string(),
            pane_id: parts[4].to_string(),
            hidden: field(5) == "1",
            provider: PROVIDERS.contains(&field(6)).then(|| field(6).to_string()),
            session_created: if digits(field(7)) { field(7).parse().ok().filter(|n| *n > 0) } else { None },
            window_index: if digits(field(8)) { field(8).parse().ok() } else { None },
            pane_index: if digits(field(9)) { field(9).parse().ok() } else { None },
        })
    }).collect();
    (panes, dropped)
}

/// Recusa que quer dizer "não há sessão", não "não sei" (`tmux.py:_run`, `absent`). Fica de fora o
/// `invalid option: @cp_hidden`: é o formato recusado, e a lista seria desconhecida.
fn is_absence(stderr: &str) -> bool {
    ["no server", "no sessions", "can't find session", "no such session"]
        .iter().any(|p| stderr.starts_with(p))
        || (stderr.starts_with("error connecting to ") && stderr.trim_end().ends_with("(No such file or directory)"))
}

pub struct Mux { program: OsString, timeout: Duration }

impl Default for Mux {
    fn default() -> Self { Self::with_program("tmux", Duration::from_secs(5)) }
}

impl Mux {
    pub fn with_program(program: impl Into<OsString>, timeout: Duration) -> Self {
        Self { program: program.into(), timeout }
    }

    pub async fn list_panes(&self) -> Result<Vec<Pane>, MuxUnavailable> {
        self.list_panes_checked().await.map(|(panes, _)| panes)
    }

    /// Os panes e, se linhas do `list-panes` ficaram de fora, o problema para o diário: a sessão
    /// delas some da lista enquanto a lista em si responde.
    pub async fn list_panes_checked(&self) -> Result<(Vec<Pane>, Option<DiscoveryProblem>), MuxUnavailable> {
        let mut command = crate::terminal_input::child_command(&self.program);
        command.args(["list-panes", "-a", "-F", LIST_PANES_FORMAT])
            .stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = match tokio::time::timeout(self.timeout, command.output()).await {
            Err(_) => return Err(unavailable("mux_timeout", None, None)),
            Ok(Err(error)) => return Err(unavailable("mux_spawn_failed", Some(&error), None)),
            Ok(Ok(out)) => out,
        };
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let (panes, dropped) = parse_list_panes(&stdout);
            if panes.is_empty() && !stdout.trim().is_empty() {
                return Err(unavailable("mux_unparsed", None, None));
            }
            let mut problems = super::discover::Problems::default();
            if dropped > 0 {
                problems.note("mux_unparsed", "list-panes", "linha do list-panes com menos de 5 campos; sessão fora da lista");
            }
            return Ok((panes, problems.into_vec().pop()));
        }
        // O Python lia qualquer recusa como "zero sessões"; só a ausência conhecida vira vazio.
        if out.status.code() == Some(1) && is_absence(&String::from_utf8_lossy(&out.stderr)) {
            Ok((Vec::new(), None))
        } else {
            Err(unavailable("mux_refused", None, out.status.code()))
        }
    }
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
        assert_eq!(missing.list_panes().await, Err(MuxUnavailable { code: "mux_spawn_failed" }));
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
}
