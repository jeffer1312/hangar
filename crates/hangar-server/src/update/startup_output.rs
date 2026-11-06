//! O que explica um reinício que não voltou: a saída da versão nova desde antes do reinício
//! (reduzida, com `scrub`), cada mudança na resposta da saúde durante a espera e o retrato do fim do
//! prazo (lançador, processos, tarefa ou unit, porta). Vai ao relatório como `update: saída`; fora
//! desse caso o log do backend não sai da máquina.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use regex::Regex;

use super::{HealthProbe, Reading, ShellRunner, StepRunner};
use crate::diag_report::Scrubber;

const HEAD_LINES: usize = 25;
const TAIL_LINES: usize = 60;
const LINE_MAX: usize = 300;
/// Uma subida que morre escreve pouco; o teto só protege de um log que cresceu sem parar.
const READ_MAX: u64 = 256 * 1024;

/// Onde a saída do backend estava antes do reinício.
pub enum Mark {
    /// Windows: a tarefa grava em `hangar-backend*.log` (`app.main --log`); o mais novo e o tamanho dele.
    File { dir: PathBuf, newest: Option<(PathBuf, u64)>, at: SystemTime },
    /// systemd do usuário: a unit roda sem `--log`, a saída fica no journal.
    Journal { since: SystemTime },
    /// Reinício na mão: ninguém sobe a versão nova, não há o que ler.
    Nothing,
}

fn backend_logs(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut logs: Vec<_> = entries.flatten().filter(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with("hangar-backend") && name.ends_with(".log")
    }).filter_map(|e| {
        let meta = e.metadata().ok()?;
        Some((e.path(), meta.modified().ok()?, meta.len()))
    }).collect();
    logs.sort_by_key(|(_, modified, _)| *modified);
    logs
}

impl Mark {
    pub fn take(manual: bool) -> Mark {
        if manual { return Mark::Nothing; }
        if cfg!(windows) {
            let Some(base) = crate::runtime::local_policy::log_base() else { return Mark::Nothing };
            let dir = base.join("privado");
            let newest = backend_logs(&dir).pop().map(|(path, _, len)| (path, len));
            return Mark::File { dir, newest, at: SystemTime::now() };
        }
        Mark::Journal { since: SystemTime::now() }
    }

    fn at(&self) -> DateTime<Utc> {
        match self {
            Mark::File { at, .. } | Mark::Journal { since: at } => (*at).into(),
            Mark::Nothing => Utc::now(),
        }
    }

    /// O que entrou depois da marca. Bloqueante. `Err`: por que não deu para ler, que também vai ao
    /// relatório; sem isso ele sairia sem a causa e sem dizer por quê.
    pub fn read(&self) -> Result<String, String> {
        match self {
            Mark::Nothing => Ok(String::new()),
            Mark::File { dir, newest, at } => {
                let logs = backend_logs(dir);
                if logs.is_empty() { return Err(format!("nenhum hangar-backend*.log em {}", dir.display())); }
                let mut out = String::new();
                for (path, modified, _) in logs {
                    let offset = match newest { Some((marked, len)) if *marked == path => *len, _ if modified > *at => 0, _ => continue };
                    out += &tail_from(&path, offset);
                    out.push('\n');
                }
                Ok(out)
            }
            Mark::Journal { since } => {
                let epoch = since.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let output = std::process::Command::new("journalctl")
                    .args(["--user", "-u", "hangar-backend.service", "--since", &format!("@{epoch}"), "-n", "5000", "-o", "cat", "--no-pager"])
                    .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output()
                    .map_err(|e| format!("journalctl não rodou: {e}"))?;
                if !output.status.success() { return Err(format!("journalctl saiu com {}", output.status)); }
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        }
    }

    /// A saída reduzida, ou a linha que diz por que não foi lida.
    pub fn lines(&self, scrubber: &Scrubber) -> Vec<String> {
        match self.read() {
            Ok(text) => lines(&text, self.at(), scrubber),
            Err(why) => vec![format!("saída da versão nova não lida: {why}")],
        }
    }
}

/// Do `offset` ao fim, no máximo `READ_MAX` do fim. O backend segue escrevendo: o Windows abre com
/// compartilhamento de leitura e escrita por padrão.
fn tail_from(path: &Path, offset: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = if len < offset { 0 } else { offset.max(len.saturating_sub(READ_MAX)) };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.take(READ_MAX).read_to_end(&mut bytes).is_err() { return String::new(); }
    String::from_utf8_lossy(&bytes).into_owned()
}

static RUST_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)$").unwrap());
/// `LEVEL:     [logger] mensagem`, o formato do `app.main`.
static PYTHON_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(DEBUG|INFO|WARNING|ERROR|CRITICAL):\s+\[([^\]]{1,80})\]\s?(.*)$").unwrap());
/// As linhas do uvicorn (`Started server process`, `error while attempting to bind`…): sem logger.
static UVICORN_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(INFO|WARNING|ERROR|CRITICAL):\s+[^\[\s]").unwrap());
static FIELD_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s[a-z_][a-z0-9_.]*=").unwrap());
static ACCESS_LINE:LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[A-Z]+ \S+ HTTP/\d"#).unwrap());
static EXCEPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][\w.]*(Error|Exception|Exit|Interrupt)\b").unwrap());

/// A saída inteira, reduzida ao que diz até onde a subida chegou sem levar conteúdo de conversa:
/// traceback, linhas do lançador e do uvicorn e `[hangar]` inteiras; do Python com logger, aviso e
/// erro inteiros e o resto só nível e logger; do `hangar-server`, a lista fixa de campos. Repetidas
/// em sequência viram uma com `×N`; ficam o começo e o fim, cada uma até `LINE_MAX`.
pub fn lines(text: &str, since: DateTime<Utc>, scrubber: &Scrubber) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let (mut access, mut other) = (0usize, 0usize);
    let mut in_traceback = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() { continue; }
        let reduced = if line.starts_with("Traceback (most recent call last)") {
            in_traceback = true;
            Some(line.to_owned())
        } else if in_traceback && (line.starts_with(char::is_whitespace) || EXCEPTION.is_match(line)) {
            if !line.starts_with(char::is_whitespace) { in_traceback = false; }
            Some(line.to_owned())
        } else {
            in_traceback = false;
            if let Some(c) = RUST_LINE.captures(line) {
                match &c[1] {
                    "WARN" | "ERROR" => crate::diag_report::reduce_server_line(line, since, scrubber),
                    // Só a mensagem fixa, antes do primeiro campo: os valores podem levar texto de conversa.
                    level => Some(format!("{level} {}", FIELD_START.split(&c[2]).next().unwrap_or("").chars().take(80).collect::<String>())),
                }
            } else if let Some(c) = PYTHON_LINE.captures(line) {
                Some(match &c[1] { "WARNING" | "ERROR" | "CRITICAL" => line.to_owned(), level => format!("{level}: [{}]", &c[2]) })
            } else if ACCESS_LINE.is_match(line) {
                access += 1;
                None
            } else if UVICORN_LINE.is_match(line) || line.starts_with("[hangar]") || EXCEPTION.is_match(line) {
                Some(line.to_owned())
            } else {
                other += 1;
                None
            }
        };
        let Some(reduced) = reduced else { continue };
        let reduced: String = scrubber.scrub(&reduced).chars().take(LINE_MAX).collect();
        match kept.last_mut() {
            Some(last) if strip_count(last) == reduced => *last = format!("{reduced} ×{}", count(last) + 1),
            _ => kept.push(reduced),
        }
    }
    if kept.len() > HEAD_LINES + TAIL_LINES {
        let cut = kept.len() - HEAD_LINES - TAIL_LINES;
        kept.splice(HEAD_LINES..HEAD_LINES + cut, [format!("({cut} linhas do meio fora)")]);
    }
    if kept.is_empty() { kept.push("a versão nova não escreveu nada no log desde o reinício".to_owned()); }
    if access > 0 { kept.push(format!("({access} linhas de acesso HTTP fora)")); }
    if other > 0 { kept.push(format!("({other} linhas sem formato conhecido fora)")); }
    kept
}

fn count(line: &str) -> usize { line.rsplit_once(" ×").and_then(|(_, n)| n.parse().ok()).unwrap_or(1) }
fn strip_count(line: &str) -> &str { line.rsplit_once(" ×").filter(|(_, n)| n.parse::<usize>().is_ok()).map_or(line, |(head, _)| head) }

// ---------- prova de vida ----------

/// A saúde lida com o registro de cada mudança no que respondeu: separa porta muda o tempo todo de
/// processo que subiu e caiu, ou de outra versão de pé.
pub struct Trail<'a, H> {
    inner: &'a H,
    start: Instant,
    seen: Mutex<Vec<(f64, f64, String, u32)>>,
}

impl<'a, H> Trail<'a, H> {
    pub fn new(inner: &'a H) -> Self { Trail { inner, start: Instant::now(), seen: Mutex::default() } }

    pub fn lines(&self, label: &str) -> Vec<String> {
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.is_empty() { return vec![format!("{label}: nenhuma leitura")]; }
        seen.iter().map(|(from, to, what, n)| format!("{label} {from:.1}–{to:.1} s: {what} (×{n})")).collect()
    }
}

fn describe(reading: &Reading) -> String {
    match reading {
        Reading::Down(d) => format!("não respondeu: {d}"),
        Reading::NotRust(d) => format!("só o Python respondeu: {d}"),
        Reading::Rust { protocol, pid, commit } => format!("hangar-server respondeu: pid {}, commit {}, contrato {protocol}",
            pid.map_or("?".into(), |p| p.to_string()), commit.as_deref().unwrap_or("?")),
    }
}

impl<H: HealthProbe> HealthProbe for Trail<'_, H> {
    // O futuro leva só o `read` de dentro (já `Send`) e o registro: sem `&H`, `H` não precisa ser `Sync`.
    fn read(&self) -> impl std::future::Future<Output = Reading> + Send {
        let (pending, start, seen) = (self.inner.read(), self.start, &self.seen);
        async move {
            let reading = pending.await;
            let (at, what) = (start.elapsed().as_secs_f64(), describe(&reading));
            let mut seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
            match seen.last_mut() {
                Some(last) if last.2 == what => { last.1 = at; last.3 += 1; }
                _ => seen.push((at, at, what, 1)),
            }
            reading
        }
    }
}

// ---------- retrato do fim do prazo ----------

/// Lançador, pacote, processos do Hangar, tarefa ou unit e quem escuta a porta. Bloqueante.
/// `new`/`old`: as versões da rodada, para dizer de qual cada processo é.
pub fn snapshot(home: &Path, port: u16, new: Option<&Path>, old: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    out.push(match crate::install_root::backend_command(home, &[], cfg!(windows)) {
        Ok(command) => format!("lançador: python {}", Path::new(command.get_program()).display()),
        Err((code, path)) => format!("lançador: {code}: {}", path.display()),
    });
    if let Some(new) = new {
        let bin = new.join("bin").join(if cfg!(windows) { "hangar-server.exe" } else { "hangar-server" });
        let has = |p: &Path| if p.exists() { "ok" } else { "FALTA" };
        out.push(format!("pacote novo: manifest {}, python {}, backend/app/main.py {}, bin/hangar-server {}",
            has(&new.join("manifest.json")), has(&crate::install_root::python_exe(new, cfg!(windows))),
            has(&new.join("backend").join("app").join("main.py")), has(&bin)));
    }
    out.extend(processes(new, old));
    out.extend(service(home, port));
    // A lista de portas do sistema já disse "ninguém escutando" com o servidor atendendo: a conexão desempata.
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    out.push(match std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
        Ok(_) => format!("porta {port} por conexão em 127.0.0.1: aceita"),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => format!("porta {port} por conexão em 127.0.0.1: recusa ({e})"),
        Err(e) => format!("porta {port} por conexão em 127.0.0.1: sem resposta ({e})"),
    });
    out
}

fn same_root(path: &str, root: &Path) -> bool {
    let root = root.to_string_lossy();
    let (path, root) = if cfg!(windows) { (path.to_lowercase(), root.to_lowercase()) } else { (path.to_owned(), root.into_owned()) };
    path.contains(root.trim_end_matches(['/', '\\']))
}

fn processes(new: Option<&Path>, old: Option<&Path>) -> Vec<String> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always).with_memory().with_cpu());
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut out = Vec::new();
    let mut others = 0usize;
    for (pid, p) in system.processes() {
        let name = p.name().to_string_lossy().to_lowercase();
        let ours = name.starts_with("hangar-server") || name.starts_with("hangar-cano");
        if !ours && !name.starts_with("python") { continue; }
        let exe = p.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let cmd = p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
        let here = format!("{exe} {cmd}");
        // Python de fora do Hangar pode levar segredo no argv: só conta.
        if !ours && !(here.contains(".hangar") || here.contains("app.main")) { others += 1; continue; }
        let version = match (new, old) {
            (Some(n), _) if same_root(&here, n) => "versão nova",
            (_, Some(o)) if same_root(&here, o) => "versão anterior",
            _ => "versão ?",
        };
        // Só o que identifica o processo: o resto do argv pode ser texto digitado (`hangar-server open claude "…"`).
        let args: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let what = if ours { args.get(1).cloned().unwrap_or_default() } else {
            args.iter().position(|a| a == "-m").and_then(|i| args.get(i + 1)).cloned()
                .or_else(|| args.iter().find(|a| a.ends_with(".py")).cloned()).unwrap_or_default()
        };
        out.push(format!("processo: pid {pid} (pai {}), {name}, {version}, há {} s, {:?}, {} MB, cpu {} s: {exe} {}",
            p.parent().map_or("?".into(), |pp| pp.to_string()), now.saturating_sub(p.start_time()), p.status(),
            p.memory() / 1_048_576, p.accumulated_cpu_time() / 1000, what.chars().take(200).collect::<String>()));
    }
    out.sort();
    if out.is_empty() { out.push("processo: nenhum python nem hangar-server do Hangar de pé".to_owned()); }
    if others > 0 { out.push(format!("processo: {others} python de fora do Hangar")); }
    out
}

fn service(home: &Path, port: u16) -> Vec<String> {
    let runner = ShellRunner { timeout: Duration::from_secs(20) };
    let command = if cfg!(windows) {
        // Sem acento nem aspas: o texto passa por cmd e powershell e volta na codepage do console.
        let script = format!(concat!(
            "$n='hangar-backend'; $t=Get-ScheduledTask -TaskName $n -EA SilentlyContinue; $i=Get-ScheduledTaskInfo -TaskName $n -EA SilentlyContinue; ",
            "if($t){{'tarefa: estado=' + $t.State + ' ultima_execucao=' + $i.LastRunTime.ToString('s') + ' ultimo_resultado=0x' + ('{{0:X8}}' -f $i.LastTaskResult) + ' acao=' + $t.Actions[0].Execute + ' ' + $t.Actions[0].Arguments}} else {{'tarefa: ' + $n + ' nao registrada'}}; ",
            "$l=@(Get-NetTCPConnection -State Listen -LocalPort {port} -EA SilentlyContinue); ",
            "if($l.Count){{'porta {port}: escutando, pid ' + (($l | ForEach-Object {{ $_.OwningProcess }} | Sort-Object -Unique) -join ',')}} else {{'porta {port}: ninguem escutando'}}"),
            port = port);
        let encoded: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!("powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, encoded))
    } else if cfg!(target_os = "linux") {
        format!(concat!("systemctl --user show hangar-backend.service --no-pager -p ActiveState -p SubState -p MainPID -p ExecMainStatus ",
            "-p ExecMainCode -p NRestarts -p ExecMainStartTimestamp | tr '\\n' ' ' | sed 's/^/unit: /'; echo; ",
            "l=$(ss -ltnpH 'sport = :{port}' 2>&1); echo \"porta {port}: ${{l:-ninguém escutando}}\""), port = port)
    } else {
        return Vec::new();
    };
    let run = runner.run(&command, home);
    let lines: Vec<String> = run.output.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    match run.code {
        Some(0) if !lines.is_empty() => lines,
        code => vec![format!("serviço: consulta saiu com {code:?}: {}", lines.join(" | "))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrubber() -> Scrubber { Scrubber::new(Some("/home/fulano".into()), Some(r"C:\Users\Fulano\AppData\Local".into())) }
    fn since() -> DateTime<Utc> { "2026-10-30T11:00:00Z".parse().unwrap() }

    #[test]
    fn keeps_how_far_startup_went_without_conversation_text() {
        let text = concat!(
            "[hangar] hangar-server launch: launch_start: versão 0.1.0.9091, lançador pid 10\n",
            "INFO:     [hangar.sse] sessão projeto-exemplo abriu com texto da conversa\n",
            "INFO:     [hangar.sse] outra linha\n",
            "WARNING:  [hangar.loop_monitor] laço lento 4 s\n",
            "INFO:     Started server process [4242]\n",
            "INFO:     127.0.0.1:5000 - \"GET /api/sessions/x HTTP/1.1\" 200 OK\n",
            "2026-10-30T11:31:00.300Z  WARN estado: falhou code=x text=\"conversa secreta\"\n",
            "2026-10-30T11:31:00.350Z  INFO hangar_server::list: tique da lista session=conversa-secreta n=3\n",
            "Traceback (most recent call last):\n",
            "  File \"C:\\Users\\Fulano\\.hangar\\current\\backend\\app\\main.py\", line 12, in <module>\n",
            "ModuleNotFoundError: No module named 'app.novo'\n",
            "lixo qualquer\n",
            "ERROR:    [hangar] subida falhou Authorization: Bearer s3cr3t em /home/fulano/x\n",
        );
        let out = lines(text, since(), &scrubber());
        assert_eq!(out[0], "[hangar] hangar-server launch: launch_start: versão 0.1.0.9091, lançador pid 10");
        assert_eq!(out[1], "INFO: [hangar.sse] ×2");
        assert_eq!(out[2], "WARNING:  [hangar.loop_monitor] laço lento 4 s");
        assert_eq!(out[3], "INFO:     Started server process [4242]");
        assert!(out[4].contains("code=x") && !out[4].contains("conversa secreta"), "{out:?}");
        assert_eq!(out[5], "INFO hangar_server::list: tique da lista");
        assert_eq!(out[6], "Traceback (most recent call last):");
        assert_eq!(out[8], "ModuleNotFoundError: No module named 'app.novo'");
        assert_eq!(out[9], "ERROR:    [hangar] subida falhou Authorization: Bearer *** em ~/x");
        assert_eq!(&out[10..], ["(1 linhas de acesso HTTP fora)", "(1 linhas sem formato conhecido fora)"]);
    }

    #[test]
    fn empty_output_says_so_and_long_output_keeps_head_and_tail() {
        assert_eq!(lines("", since(), &scrubber()), vec!["a versão nova não escreveu nada no log desde o reinício".to_owned()]);
        let text: String = (0..200).map(|i| format!("ERROR:    [hangar] linha {i}\n")).collect();
        let out = lines(&text, since(), &scrubber());
        assert_eq!(out.len(), HEAD_LINES + 1 + TAIL_LINES);
        assert_eq!(out[HEAD_LINES], "(115 linhas do meio fora)");
        assert!(out.last().unwrap().ends_with("linha 199"));
    }

    #[test]
    fn file_mark_reads_what_came_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("hangar-backend.log");
        std::fs::write(&log, "ERROR:    [x] velho\n").unwrap();
        // O arquivo marcado lê pelo deslocamento; a hora só decide arquivos novos, e aqui não há outro.
        let mark = Mark::File { dir: dir.path().into(), newest: Some((log.clone(), 20)), at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000) };
        std::fs::write(&log, "ERROR:    [x] velho\nERROR:    [x] novo\n").unwrap();
        assert_eq!(mark.lines(&scrubber()), vec!["ERROR:    [x] novo".to_owned()]);
    }

    #[tokio::test]
    async fn trail_folds_repeated_readings_and_keeps_each_change() {
        struct Seq(Mutex<Vec<Reading>>);
        impl HealthProbe for Seq {
            async fn read(&self) -> Reading { self.0.lock().unwrap().remove(0) }
        }
        let probe = Seq(Mutex::new(vec![Reading::Down("recusada".into()), Reading::Down("recusada".into()),
            Reading::NotRust("503".into()), Reading::Rust { protocol: 7, pid: Some(9), commit: Some("abc".into()) }]));
        let trail = Trail::new(&probe);
        for _ in 0..4 { trail.read().await; }
        let out = trail.lines("prova");
        assert_eq!(out.len(), 3);
        assert!(out[0].ends_with("não respondeu: recusada (×2)"), "{out:?}");
        assert!(out[1].ends_with("só o Python respondeu: 503 (×1)"));
        assert!(out[2].ends_with("hangar-server respondeu: pid 9, commit abc, contrato 7 (×1)"));
    }
}
