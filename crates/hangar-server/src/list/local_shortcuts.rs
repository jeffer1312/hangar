//! Terminais de atalho da lista (`shortcut_terminals.list_all` + `terminal_prompt.pending_question`):
//! o multiplexador é a fonte; dono, id e rótulo moram nas opções `@cp_shortcut_*` da sessão.
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::procs::ChildrenMap;

pub(crate) const PREFIX: &str = "shortcut-";
pub(crate) const FORMAT: &str = "#{session_name}\t#{@cp_shortcut_owner}\t#{@cp_shortcut_id}\t#{session_created}\t#{pane_dead}\t#{pane_dead_status}\t#{pane_pid}\t#{@cp_shortcut_seq}\t#{@cp_shortcut_key}\t#{@cp_shortcut_origin}\t#{@cp_shortcut_ask}\t#{@cp_shortcut_label}";
const MUX_TIMEOUT: Duration = Duration::from_secs(5);
/// Servidor calado não come o prazo do CLI que vem depois.
#[cfg(windows)]
const DIRECT_TIMEOUT: Duration = Duration::from_secs(2);
const TTY_WAITS: [&str; 2] = ["wait_woken", "n_tty_read"];
const SCREEN_LINES: usize = 6;
/// Windows: tela e CPU parados por pelo menos isto entre duas leituras contam como pergunta.
#[cfg(windows)]
const MIN_GAP: Duration = Duration::from_secs(1);
/// Windows: `.cmd`/`.exit` de terminal que sumiu sai depois disto (o `.cmd` guarda o comando).
#[cfg(windows)]
const ORPHAN_GRACE_S: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) tmux: String, pub(crate) owner: String, pub(crate) id: String, pub(crate) created: u64, pub(crate) alive: bool, pub(crate) exit_code: Option<i64>,
    pub(crate) pid: Option<u32>, pub(crate) label: String, pub(crate) seq: u64, pub(crate) key: String, pub(crate) origin: String, pub(crate) ask: bool,
}

fn is_id(s: &str) -> bool { s.len() == 6 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }

fn digits(s: &str) -> Option<u64> { (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten() }

/// `_read_rows` sobre a saída do `list-sessions`.
pub(crate) fn parse(out: &str, windows_status: impl Fn(&str) -> (bool, Option<i64>)) -> Vec<Row> {
    out.lines().filter_map(|line| {
        let p: Vec<&str> = line.splitn(12, '\t').collect();
        if p.len() != 12 || !p[0].starts_with(PREFIX) || !is_id(p[2]) { return None; }
        let dead = p[4] == "1";
        let code = if dead && !p[5].is_empty() && p[5].trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) && !p[5].trim_start_matches('-').is_empty() {
            p[5].parse().ok()
        } else { None };
        let (alive, exit_code) = if cfg!(windows) { windows_status(p[2]) } else { (!dead, code) };
        Some(Row { tmux: p[0].into(), owner: p[1].into(), id: p[2].into(), created: digits(p[3]).unwrap_or(0), alive, exit_code,
            pid: digits(p[6]).and_then(|v| u32::try_from(v).ok()), label: p[11].into(), seq: digits(p[7]).unwrap_or(0),
            key: p[8].into(), origin: p[9].into(), ask: p[10] != "0" })
    }).collect()
}

#[cfg(windows)]
fn windows_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::home_dir().unwrap_or_default());
    base.join("Hangar").join("shortcuts")
}

/// `_windows_status`: o `.cmd` grava o código de saída num arquivo; sem ele o terminal vive.
fn windows_status(ident: &str) -> (bool, Option<i64>) {
    #[cfg(windows)]
    {
        let Ok(raw) = std::fs::read(windows_dir().join(format!("{ident}.exit"))) else { return (true, None) };
        let text = String::from_utf8_lossy(&raw).trim().to_owned();
        let numeric = !text.trim_start_matches('-').is_empty() && text.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit());
        (false, if numeric { text.parse().ok() } else { None })
    }
    #[cfg(not(windows))]
    { let _ = ident; (true, None) }
}

/// `_sweep_orphans`: arquivos de terminal que sumiu, passado o prazo (o `.cmd` pode ter credencial).
#[cfg(windows)]
fn sweep_orphans(live: &std::collections::HashSet<&str>) {
    let Ok(entries) = std::fs::read_dir(windows_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(ident) = ["-cmd.cmd", "-cmd.ps1", ".cmd", ".exit"].iter().find_map(|s| name.strip_suffix(s)).filter(|i| is_id(i)) else { continue };
        if live.contains(ident) { continue; }
        let old = entry.metadata().and_then(|m| m.modified()).ok()
            .and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > ORPHAN_GRACE_S);
        if old { let _ = std::fs::remove_file(entry.path()); }
    }
}

async fn mux(program: &OsStr, args: &[&str]) -> Option<std::process::Output> {
    let mut command = crate::terminal_input::child_command(program);
    command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(MUX_TIMEOUT, command.output()).await.ok()?.ok()
}

/// Leitura no servidor do psmux da sessão que a lista conferiu, sem abrir o `tmux.exe`. `None` = sem
/// conferência ou falhou: vale o CLI.
#[cfg(windows)]
async fn direct(program: &OsStr, session: &str, args: &[&str]) -> Option<String> {
    let dir = super::mux::verified_direct(program, session)?;
    let (base, args) = (session.to_owned(), args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
    let deadline = std::time::Instant::now() + DIRECT_TIMEOUT;
    let stdout = tokio::task::spawn_blocking(move || hangar_psmux::command(&dir, &base, &args, deadline)).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&stdout).into_owned();
    (!text.starts_with("ERROR: ")).then_some(text)
}

#[cfg(not(windows))]
async fn direct(_: &OsStr, _: &str, _: &[&str]) -> Option<String> { None }

/// Comando de leitura na sessão: direto quando dá, senão o CLI. `(sucesso, saída)`.
async fn read(program: &OsStr, session: &str, args: &[&str]) -> Option<(bool, String)> {
    if let Some(text) = direct(program, session, args).await { return Some((true, text)); }
    let out = mux(program, args).await?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// `terminal_prompt.parse_prompt`: a linha termina em `:` ou `?`; `[padrão]` no fim é o valor sugerido.
fn parse_prompt(line: &str) -> Option<Value> {
    let text = line.trim_end();
    let body = if let Some(b) = text.strip_suffix(':') { b.trim_end() } else if text.ends_with('?') { text } else { return None };
    let (question, default) = match body.strip_suffix(']').and_then(|b| b.rfind('[').map(|i| (&b[..i], &b[i + 1..]))) {
        Some((q, d)) if !d.contains(']') => (q.trim(), d),
        _ => (body.trim(), ""),
    };
    (!question.is_empty()).then(|| json!({"text": question, "default": default}))
}

/// `terminal_prompt.logical_line`: a linha que o programa escreveu, juntando a quebra da largura.
fn logical_line(screen: &[&str], y: usize, width: usize) -> String {
    let len = |s: &str| s.chars().count();
    let mut line = screen.get(y).copied().unwrap_or("").to_owned();
    let mut y = y;
    while y > 0 && y <= screen.len() && width > 1 && len(screen[y - 1]) >= width - 1 {
        y -= 1;
        line = format!("{}{}{}", screen[y], if len(screen[y]) == width - 1 { " " } else { "" }, line);
    }
    line
}

/// Campos do `/proc/<pid>/stat` depois do `(comm)`, que pode ter espaço e parêntese.
fn stat(pid: i64) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    Some(raw.rsplit_once(')')?.1.split_whitespace().map(str::to_owned).collect())
}

/// Linux: algum processo do primeiro plano do pane dormindo na leitura do tty.
fn reading_tty(pane_pid: u32, children: &ChildrenMap) -> bool {
    let Some(head) = stat(pane_pid.into()).filter(|h| h.len() >= 6) else { return false };
    let foreground = &head[5];
    super::discover::descendants(pane_pid.into(), children).into_iter().any(|pid| {
        stat(pid).filter(|s| s.len() >= 6 && &s[2] == foreground && s[0] == "S").is_some()
            && std::fs::read_to_string(format!("/proc/{pid}/wchan")).is_ok_and(|w| TTY_WAITS.contains(&w.trim()))
    })
}

/// Windows: (instante, tela, CPU) da última leitura de cada alvo.
static LAST: LazyLock<Mutex<HashMap<String, (Instant, String, f64)>>> = LazyLock::new(Mutex::default);

fn last() -> std::sync::MutexGuard<'static, HashMap<String, (Instant, String, f64)>> { LAST.lock().unwrap_or_else(|e| e.into_inner()) }

#[cfg(windows)]
fn cpu_ms(pane_pid: u32, children: &ChildrenMap) -> f64 {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pids: Vec<Pid> = super::discover::descendants(pane_pid.into(), children).into_iter()
        .filter_map(|p| u32::try_from(p).ok()).map(Pid::from_u32).collect();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
    pids.iter().filter_map(|p| system.process(*p)).map(|p| p.accumulated_cpu_time() as f64).sum()
}

/// `_idle_windows`: a mesma tela e CPU parada entre duas leituras com `MIN_GAP` de distância.
#[cfg(windows)]
fn idle_windows(target: &str, pane_pid: u32, screen: &str, children: &ChildrenMap) -> bool {
    let (now, cpu) = (Instant::now(), cpu_ms(pane_pid, children));
    let mut map = last();
    let before = map.get(target).cloned();
    if before.as_ref().is_none_or(|(at, s, _)| now.duration_since(*at) >= MIN_GAP || s != screen) {
        map.insert(target.to_owned(), (now, screen.to_owned(), cpu));
    }
    match before {
        Some((at, s, c)) if s == screen => now.duration_since(at) >= MIN_GAP && (cpu - c).abs() < 1.0,
        _ => false,
    }
}

/// `terminal_prompt.pending_question`: a pergunta que o terminal espera responder.
pub(crate) async fn pending_question(program: &OsStr, target: &str, pane_pid: Option<u32>, children: &ChildrenMap) -> Option<Value> {
    let pane_pid = pane_pid.filter(|p| *p != 0)?;
    if !cfg!(windows) && !reading_tty(pane_pid, children) { return None; }
    let where_ = format!("={target}:");
    let (ok, text) = read(program, target, &["display", "-p", "-t", &where_, "#{cursor_y},#{pane_width}"]).await?;
    let (y, width) = text.trim().split_once(',')?;
    let (y, width) = (digits(y)? as usize, digits(width)? as usize);
    if !ok { return None; }
    let raw = read(program, target, &["capture-pane", "-p", "-t", &where_]).await.map(|(_, text)| text).unwrap_or_default();
    let screen: Vec<&str> = raw.split('\n').collect();
    let Some(mut question) = parse_prompt(&logical_line(&screen, y, width)) else {
        last().remove(target);
        return None;
    };
    #[cfg(windows)]
    if !idle_windows(target, pane_pid, &raw, children) { return None; }
    let tail: Vec<&str> = screen[..(y + 1).min(screen.len())].iter().filter(|l| !l.trim().is_empty()).map(|l| l.trim_end()).collect();
    question["screen"] = json!(tail[tail.len().saturating_sub(SCREEN_LINES)..]);
    Some(question)
}

pub(crate) fn forget_target(target: &str) { last().remove(target); }

pub(crate) fn maintain(rows: &[Row]) {
    #[cfg(windows)]
    sweep_orphans(&rows.iter().map(|r| r.id.as_str()).collect());
    let alive: std::collections::HashSet<&str> = rows.iter().filter(|r|r.alive).map(|r|r.tmux.as_str()).collect();
    last().retain(|target,_|alive.contains(target.as_str()));
}

/// O `list-sessions` dos atalhos. `sessions` são as sessões de atalho que os panes da rodada mostraram;
/// `None` = os panes não provam a ausência e só o multiplexador responde. Nenhuma = nada a ler.
async fn read_sessions(program: &OsStr, sessions: Option<Vec<String>>) -> Result<Option<String>, &'static str> {
    if sessions.as_ref().is_some_and(Vec::is_empty) { return Ok(None); }
    let args = ["list-sessions", "-F", FORMAT];
    // No psmux cada sessão tem servidor próprio, que responde só a linha dela.
    if let Some(sessions) = &sessions {
        let mut text = Some(String::new());
        for session in sessions {
            let Some(line) = direct(program, session, &args).await else { text = None; break };
            text.as_mut().map(|t| t.push_str(&line));
        }
        if text.is_some() { return Ok(text); }
    }
    let out = mux(program, &args).await.ok_or("shortcuts_mux_unavailable")?;
    Ok(Some(if out.status.success() { String::from_utf8_lossy(&out.stdout).into_owned() } else { String::new() }))
}

type Reading = Result<String, &'static str>;

/// A leitura dos atalhos corre fora da rodada da lista, como o `_ListRefresher` do Python: cada
/// rodada colhe a que terminou e dispara a próxima, sem esperar o multiplexador.
#[derive(Default)]
pub struct Refresher { task: std::sync::Mutex<Option<tokio::task::JoinHandle<Reading>>> }

impl Refresher {
    /// A leitura que terminou desde a rodada anterior; `None` = nada novo (vale a última boa).
    pub fn round<F>(&self, read: impl FnOnce() -> F) -> Option<Reading>
    where F: std::future::Future<Output = Reading> + Send + 'static {
        use futures_util::FutureExt;
        let mut slot = self.task.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|t| !t.is_finished()) { return None; }
        let done = slot.take().and_then(|t| t.now_or_never()).map(|r| r.unwrap_or(Err("shortcuts_task_failed")));
        *slot = Some(tokio::spawn(read()));
        done
    }
}

/// `list_all` em JSON: todos os terminais de atalho, do mais antigo ao mais novo. `Err` = o
/// multiplexador não respondeu (lista vazia apagaria as abas e as perguntas).
pub async fn list_all(program: &OsString, children: &ChildrenMap, sessions: Option<Vec<String>>) -> Result<String, &'static str> {
    let mut rows = match read_sessions(program, sessions).await? {
        Some(text) => parse(&text, windows_status),
        None => Vec::new(),
    };
    maintain(&rows);
    rows.sort_by(|a, b| (a.created, a.seq, &a.tmux).cmp(&(b.created, b.seq, &b.tmux)));
    let mut list = Vec::with_capacity(rows.len());
    for r in &rows {
        let question = if r.alive && r.ask { pending_question(program, &r.tmux, r.pid, children).await } else { None };
        list.push(json!({"id": r.id, "label": r.label, "alive": r.alive, "exit_code": r.exit_code, "created": r.created,
            "owner": r.owner, "key": r.key, "origin": r.origin, "ask": r.ask, "question": question}));
    }
    Ok(Value::Array(list).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_parse_like_python() {
        let out = "shortcut-a\tsess\tab12cd\t100\t0\t\t4242\t2\tk\to\t1\tRodar testes\n\
                   shortcut-b\t\tzz\t1\t0\t\t1\t0\t\t\t0\tx\n\
                   main\tsess\tab12cd\t1\t0\t\t1\t0\t\t\t0\tx\n\
                   shortcut-c\t\tff00ff\tx\t1\t-2\tp\t\tk2\t\t0\tFim";
        let rows = parse(out, |_| (true, None));
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].alive, rows[0].pid, rows[0].seq, rows[0].ask), (true, Some(4242), 2, true));
        if !cfg!(windows) {
            assert_eq!((rows[1].alive, rows[1].exit_code, rows[1].created, rows[1].pid), (false, Some(-2), 0, None));
        }
    }

    #[test]
    fn prompt_and_logical_line_like_terminal_prompt() {
        assert_eq!(parse_prompt("Porta [8080]:"), Some(json!({"text": "Porta", "default": "8080"})));
        assert_eq!(parse_prompt("Continuar?"), Some(json!({"text": "Continuar?", "default": ""})));
        assert_eq!(parse_prompt("rodando..."), None);
        assert_eq!(parse_prompt(":"), None);
        let screen = ["abcd", "ef:"];
        assert_eq!(logical_line(&screen, 1, 5), "abcd ef:");
        assert_eq!(logical_line(&screen, 1, 10), "ef:");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn no_shortcut_session_skips_the_mux() {
        let (program, children) = (OsString::from("missing-test-mux"), ChildrenMap::default());
        assert_eq!(list_all(&program, &children, false).await, Ok("[]".to_owned()), "sem sessão de atalho o multiplexador foi chamado");
        assert_eq!(list_all(&program, &children, true).await, Err("shortcuts_mux_unavailable"));
    }

    #[tokio::test]
    async fn the_list_round_never_waits_for_the_mux() {
        let stuck = Refresher::default();
        assert!(stuck.round(std::future::pending).is_none(), "multiplexador parado não segura a rodada");
        assert!(stuck.round(|| async { Ok("b".to_owned()) }).is_none(), "leitura em voo: nada novo");
        let r = Refresher::default();
        assert!(r.round(|| async { Ok("a".to_owned()) }).is_none());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(r.round(|| async { Ok("b".to_owned()) }), Some(Ok("a".to_owned())), "a rodada colhe a leitura que terminou");
    }
}

#[cfg(test)]
mod direct_tests {
    use super::*;

    #[tokio::test]
    async fn no_shortcut_in_the_panes_reads_nothing() {
        // O programa não existe: abrir o CLI daria erro.
        assert_eq!(read_sessions(OsStr::new("missing-shortcut-mux"), Some(Vec::new())).await, Ok(None));
    }

    #[tokio::test]
    async fn an_unproved_round_still_asks_the_program() {
        assert_eq!(read_sessions(OsStr::new("missing-shortcut-mux"), None).await, Err("shortcuts_mux_unavailable"));
    }
}

#[cfg(all(test, windows))]
mod psmux_direct_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    /// Servidor de sessão falso: responde à conferência da lista e, depois, a cada comando com o que `reply` der.
    fn server(dir: &std::path::Path, base: &str, calls: usize, reply: fn(&str) -> &'static str) -> std::thread::JoinHandle<Vec<String>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(dir.join(format!("{base}.port")), listener.local_addr().unwrap().port().to_string()).unwrap();
        std::fs::write(dir.join(format!("{base}.key")), "k").unwrap();
        let name = base.to_owned();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        // Quem não vier dentro do teto não chegou: o teste falha na asserção, sem travar no `join`.
        let accept = move || loop {
            match listener.accept() {
                Ok((stream, _)) => { stream.set_nonblocking(false).unwrap(); return Some(stream); }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)), // estavel: espera de conexão com teto
                Err(_) => return None,
            }
        };
        std::thread::spawn(move || (0..calls + 1).map_while(|_| {
            let stream = accept()?;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut auth, mut cmd) = (String::new(), String::new());
            reader.read_line(&mut auth).unwrap(); reader.read_line(&mut cmd).unwrap();
            let body = if cmd.starts_with("\"list-panes\"") || cmd.starts_with("list-panes") { format!("{name}\n") } else { reply(&cmd).to_owned() };
            (&stream).write_all(format!("OK\n{body}").as_bytes()).unwrap();
            Some(cmd)
        }).collect())
    }

    fn verify(program: &str, dir: &std::path::Path) {
        let until = Instant::now() + Duration::from_secs(5);
        let listing = hangar_psmux::list_panes(dir, "#{session_name}", until).unwrap();
        crate::list::mux::verify_for_test(program, dir, listing.servers);
    }

    #[tokio::test]
    async fn verified_shortcut_sessions_are_listed_without_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let a = server(dir.path(), "shortcut-a", 1, |_| "shortcut-a\t\tab12cd\t100\t0\t\t4242\t2\tk\to\t0\tA\n");
        let b = server(dir.path(), "shortcut-b", 1, |_| "shortcut-b\t\tcd34ef\t101\t0\t\t4243\t3\tk\to\t0\tB\n");
        verify("missing-shortcut-mux-2", dir.path());
        let sessions = vec!["shortcut-a".to_owned(), "shortcut-b".to_owned()];
        let text = read_sessions(OsStr::new("missing-shortcut-mux-2"), Some(sessions)).await.unwrap().unwrap();
        assert_eq!(parse(&text, |_| (true, None)).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["ab12cd", "cd34ef"]);
        assert!(a.join().unwrap().get(1).is_some_and(|cmd| cmd.starts_with("\"list-sessions\"")));
        b.join().unwrap();
    }

    #[tokio::test]
    async fn the_pending_question_reads_the_screen_without_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let seen = server(dir.path(), "shortcut-q", 2, |cmd| if cmd.starts_with("\"display\"") { "0,80\n" } else { "Nome do projeto:\n" });
        verify("missing-shortcut-mux-3", dir.path());
        pending_question(OsStr::new("missing-shortcut-mux-3"), "shortcut-q", Some(4242), &ChildrenMap::new()).await;
        let cmds = seen.join().unwrap();
        assert!(cmds.len() == 3 && cmds[1].starts_with("\"display\"") && cmds[2].starts_with("\"capture-pane\""), "{cmds:?}");
    }
}
