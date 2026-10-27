//! Relatório da falha (spec "Falha e relatório"): o log da instalação, o doctor, etapa e código, sistema e versão do app,
//! limpo de senha, nome do usuário, nomes da tailnet e IPs. A pessoa vê exatamente o texto que sai.
use std::{sync::{Arc, Mutex, mpsc}, path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};
use serde::Serialize;
use crate::i18n::tr;
use super::flow::Screen;
use super::local::Mode;
use super::system::{hidden, refreshed_path};

/// O Worker aceita 256 KB de JSON. Os tetos contam bytes já escapados (`json_len`): `"`, `\\` e controle pesam mais no corpo.
pub(crate) const BASE_MAX: usize = 140 * 1024;
pub(crate) const AGENT_MAX: usize = 60 * 1024;
/// Corpo inteiro do `POST`, com folga abaixo dos 262144 do Worker.
pub(crate) const BODY_MAX: usize = 250 * 1024;
pub(crate) const URL: &str = "https://hangar.dev.br/api/relatorio";
const SECRET: &str = "<senha>";
const USER: &str = "<usuario>";
const IP: &str = "<ip>";
const TAILNET_HOST: &str = "<maquina>";
/// Senha mais curta que isto não é procurada no texto: trocaria letras soltas no relatório inteiro.
const MIN_SECRET: usize = 4;
/// Nome seguido de `=` ou `:` (também JSON, `"token": "x"`): o valor depois dele é senha.
const SECRET_NAMES: [&str; 6] = ["token", "password", "passwd", "secret", "api_key", "apikey"];
/// Teto de cada pedaço do começo do relatório: o log, que é o que mais ajuda, fica com o resto do orçamento.
const TEXT_MAX: usize = 8 * 1024;
const DOCTOR_MAX: usize = 24 * 1024;
const SYSTEM_DIRS: [&str; 18] = ["dev", "bin", "sbin", "etc", "usr", "var", "tmp", "proc", "sys", "run", "opt", "srv", "mnt", "lib", "boot", "root", "home", "media"];
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(40);

pub(crate) struct Secrets { pub values: Vec<String>, pub home: Option<String>, pub users: Vec<String> }

impl Secrets {
    /// Os segredos que o app tem agora, mais a pasta pessoal e o nome do usuário deste computador.
    pub(crate) fn here(values: Vec<String>) -> Self {
        let home_dir = std::env::home_dir();
        let mut users: Vec<String> = ["USER", "USERNAME"].iter().filter_map(|k| std::env::var(k).ok()).collect();
        users.extend(home_dir.as_ref().and_then(|h| h.file_name()).map(|n| n.to_string_lossy().into_owned()));
        users.retain(|u| u.chars().count() >= 2);
        users.sort();
        users.dedup();
        Self { values, home: home_dir.map(|h| h.to_string_lossy().into_owned()), users }
    }
}

pub(crate) fn scrub(text: &str, secrets: &Secrets) -> String {
    let mut out = strip_controls(text);
    let mut values: Vec<&str> = secrets.values.iter().map(String::as_str).filter(|v| v.chars().count() >= MIN_SECRET).collect();
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for value in values { out = out.replace(value, SECRET); }
    out = scrub_assignments(&out);
    // Pasta pessoal vazia ou "/" casaria com toda barra do texto.
    if let Some(home) = secrets.home.as_ref().filter(|h| !h.trim_matches(['/', '\\']).is_empty()) {
        out = replace_path(&out, home, "~");
        out = replace_path(&out, &home.replace('\\', "/"), "~");
    }
    // Nome igual a pasta do sistema ("dev" em "/dev/null") só é trocado sob a pasta pessoal; os outros, em qualquer segmento.
    for user in &secrets.users {
        let system = SYSTEM_DIRS.iter().any(|d| d.eq_ignore_ascii_case(user));
        let roots: &[&str] = if system { &["/home/", "/Users/", "\\Users\\"] } else { &["/", "\\"] };
        for dir in roots {
            out = replace_path(&out, &format!("{dir}{user}"), &format!("{dir}{USER}"));
        }
    }
    scrub_ips(&scrub_tailnet(&out))
}

/// Cor ANSI (o `install.sh` pinta mesmo sem terminal) e `\r` de progresso: lixo na prévia e 6 bytes cada no JSON. Da linha
/// com `\r` fica o que vem depois do último, como o terminal mostrou.
fn strip_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 { out.push('\n'); }
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut chars = line.rsplit('\r').next().unwrap_or(line).chars();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    // CSI: parâmetros até o byte final (`m` da cor, `K` de apagar a linha).
                    Some('[') => { for c in chars.by_ref() { if ('\x40'..='\x7e').contains(&c) { break; } } }
                    // OSC (título, link): até BEL ou ESC \.
                    Some(']') => { while let Some(c) = chars.next() { if c == '\x07' { break; } if c == '\x1b' { chars.next(); break; } } }
                    _ => {}
                },
                '\t' => out.push(c),
                c if c.is_control() => {}
                c => out.push(c),
            }
        }
    }
    out
}

/// Bytes do caractere dentro de uma string JSON do `serde_json` (que não escapa o que não é ASCII).
fn json_char(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\x08' | '\x0c' => 2,
        c if c < ' ' => 6,
        c => c.len_utf8(),
    }
}

pub(crate) fn json_len(text: &str) -> usize { text.chars().map(json_char).sum() }

/// Troca `needle` (sem diferenciar maiúsculas ASCII: caminho do Windows) só quando o nome acaba ali.
fn replace_path(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() { return text.to_owned(); }
    let (lower, pattern) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower.match_indices(&pattern) {
        let end = at + needle.len();
        if text[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-')) { continue; }
        out.push_str(&text[last..at]);
        out.push_str(with);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// O valor que começa em `from` (depois de espaços): entre aspas até a aspa que fecha, senão até um separador.
/// `to_eol`: o cabeçalho `Authorization: Basic xxx` vai até o fim da linha (esquema + credencial).
fn value_range(text: &str, from: usize, to_eol: bool) -> Option<(usize, usize)> {
    let start = from + text[from..].len() - text[from..].trim_start_matches([' ', '\t']).len();
    // JSON dentro de outra string (`{\"password\": \"x\"}`): o valor vai até a aspa escapada que fecha.
    if let Some(q) = ["\\\"", "\\'"].into_iter().find(|q| text[start..].starts_with(q)) {
        let s = start + q.len();
        let end = text[s..].find(q).into_iter().chain(text[s..].find('\n')).min().map_or(text.len(), |n| s + n);
        return (end > s).then_some((s, end));
    }
    let quote = text[start..].chars().next().filter(|c| matches!(c, '"' | '\''));
    let (start, end) = match quote {
        Some(q) => (start + 1, text[start + 1..].find([q, '\n']).map_or(text.len(), |n| start + 1 + n)),
        None if to_eol => (start, text[start..].find(['\n', '"', '\'']).map_or(text.len(), |n| start + n)),
        None => (start, text[start..].find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ',' | ';' | ')' | '<' | '}'))
            .map_or(text.len(), |n| start + n)),
    };
    (end > start).then_some((start, end))
}

/// `token=…`, `"password": "…"`, `Authorization: …` e `Bearer …`: senha que o app não conhecia (outra URL de pareamento, um cabeçalho).
fn scrub_assignments(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (names, to_eol) in [(&SECRET_NAMES[..], false), (&["authorization"][..], true)] {
        for name in names {
            for (at, _) in lower.match_indices(name) {
                // Nome, aspa opcional (JSON), espaços, `=` ou `:`.
                let mut at = at + name.len();
                if lower[at..].starts_with("\\\"") || lower[at..].starts_with("\\'") { at += 2; }
                else if matches!(lower.as_bytes().get(at), Some(b'"' | b'\'')) { at += 1; }
                at += lower[at..].len() - lower[at..].trim_start_matches([' ', '\t']).len();
                if matches!(lower.as_bytes().get(at), Some(b'=' | b':')) {
                    ranges.extend(value_range(text, at + 1, to_eol));
                }
            }
        }
    }
    ranges.extend(lower.match_indices("bearer ").filter_map(|(at, k)| value_range(text, at + k.len(), false)));
    ranges.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in ranges {
        if start < last { continue; }
        out.push_str(&text[last..start]);
        out.push_str(SECRET);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

fn scrub_tailnet(text: &str) -> String {
    const SUFFIX: &str = ".ts.net";
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower.match_indices(SUFFIX) {
        let end = at + SUFFIX.len();
        if text[end..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-') { continue; }
        let start = text[..at].char_indices().rev().take_while(|&(_, c)| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            .last().map_or(at, |(i, _)| i);
        if start >= at || start < last { continue; }
        out.push_str(&text[last..start]);
        out.push_str(TAILNET_HOST);
        last = at;
    }
    out.push_str(&text[last..]);
    out
}

fn scrub_ips(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut last) = (0, 0);
    while i < bytes.len() {
        // IPv4 pode vir depois de ":" ("addr:10.0.0.5"); IPv6 não, ou pegaria o meio de outro endereço.
        let boundary4 = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'.' | b'_'));
        let boundary6 = boundary4 && (i == 0 || bytes[i - 1] != b':');
        let found = if boundary4 { ipv4_at(&bytes[i..]) } else { None }.or_else(|| if boundary6 { ipv6_at(&bytes[i..]) } else { None });
        if let Some(len) = found {
            out.push_str(&text[last..i]);
            out.push_str(IP);
            i += len;
            last = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

fn ipv4_at(b: &[u8]) -> Option<usize> {
    let (at, octets) = ipv4_parse(b)?;
    // Loopback e "qualquer endereço" não identificam ninguém e ajudam a ler o log.
    if octets[0] == 127 || octets == [0, 0, 0, 0] { return None; }
    Some(at)
}

fn ipv4_parse(b: &[u8]) -> Option<(usize, [u32; 4])> {
    let (mut at, mut octets) = (0, [0u32; 4]);
    for (n, slot) in octets.iter_mut().enumerate() {
        if n > 0 {
            if b.get(at) != Some(&b'.') { return None; }
            at += 1;
        }
        let digits = b[at..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || digits > 3 { return None; }
        *slot = std::str::from_utf8(&b[at..at + digits]).ok()?.parse().ok()?;
        if *slot > 255 { return None; }
        at += digits;
    }
    // "1.2.3.4.5" e "1.2.3.4abc" não são IP (versão, número de build).
    if b.get(at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') || (b.get(at) == Some(&b'.') && b.get(at + 1).is_some_and(u8::is_ascii_digit)) {
        return None;
    }
    Some((at, octets))
}

fn ipv6_at(b: &[u8]) -> Option<usize> {
    let len = b.iter().take_while(|c| c.is_ascii_hexdigit() || **c == b':').count();
    let run = &b[..len];
    let colons = run.iter().filter(|c| **c == b':').count();
    // Hora ("12:34:56") tem dois-pontos mas não tem "::" nem cinco separadores.
    if len - colons < 2 || colons < 2 || !(run.windows(2).any(|w| w == b"::") || colons >= 5) { return None; }
    // "::ffff:192.168.0.5": o final é um IPv4, e o endereço inteiro sai junto (ou fica, se for loopback).
    // Ponto que não é de IPv4 ("fe80::1c2b:3a4d." no fim da frase) cai no comprimento comum.
    if b.get(len) == Some(&b'.') && let Some(tail) = run.iter().rposition(|c| *c == b':').map(|c| c + 1)
        && let Some((v4, octets)) = ipv4_parse(&b[tail..]) {
        return (octets[0] != 127 && octets != [0, 0, 0, 0]).then_some(tail + v4);
    }
    if b.get(len).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') { return None; }
    Some(len)
}

/// O fim do texto em até `max` bytes de JSON, começando numa linha inteira: no log de instalação o que importa é o fim.
pub(crate) fn tail(text: &str, max: usize) -> String {
    if json_len(text) <= max { return text.to_owned(); }
    let mut size = 0;
    let start = text.char_indices().rev().find(|&(_, c)| { size += json_char(c); size > max }).map_or(0, |(i, c)| i + c.len_utf8());
    let start = text[start..].find('\n').map_or(start, |n| start + n + 1);
    format!("\n{}\n{}", tr("setup_report_log_cut").replace("{bytes}", &start.to_string()), &text[start..])
}

pub(crate) struct Facts {
    pub step: Screen,
    pub code: Option<String>,
    pub text: String,
    pub system: String,
    pub app: String,
    pub doctor: Option<String>,
    /// (`check`/`install`, conteúdo do arquivo de saída), na ordem em que rodaram.
    pub logs: Vec<(String, String)>,
}

pub(crate) fn screen_slug(screen: Screen) -> &'static str {
    match screen {
        Screen::Welcome => "bem-vindo", Screen::Prepare => "preparar", Screen::Install => "instalar",
        Screen::Tailscale => "tailscale", Screen::Phone => "celular", Screen::Done => "final",
    }
}

/// Cada pedaço é limpo antes de montar; o log é cortado pelo começo para caber.
pub(crate) fn compose(f: &Facts, secrets: &Secrets) -> String {
    let code = match &f.code { Some(code) => tr("setup_failure_code").replace("{code}", code), None => tr("setup_failure_unexpected") };
    let mut head = format!("# {}\n\n{}\n{}\n{}\n{}\n{}\n\n## {}\n\n", tr("setup_report_heading"),
        tr("setup_report_step").replace("{step}", screen_slug(f.step)), code, tail(&scrub(&f.text, secrets), TEXT_MAX),
        tr("setup_report_system").replace("{system}", &tail(&scrub(&f.system, secrets), 1024)), tr("setup_report_app").replace("{app}", &f.app),
        tr("setup_report_doctor"));
    head.push_str(&match &f.doctor { Some(text) => tail(&scrub(text, secrets), DOCTOR_MAX), None => tr("setup_report_doctor_missing") });
    let logs: String = f.logs.iter().map(|(run, text)| format!("\n\n## {}\n\n{}", tr("setup_report_log").replace("{run}", run), scrub(text, secrets))).collect();
    let budget = BASE_MAX.saturating_sub(json_len(&head) + 256);
    format!("{head}{}\n", tail(&logs, budget))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Recheck { Passed, Failed(String), NotRun(String) }

pub(crate) struct AgentSection {
    pub agent: String,
    pub commands: Vec<String>,
    pub explanation: String,
    pub diff: String,
    pub notes: Vec<String>,
    pub recheck: Recheck,
}

/// A seção do conserto vem depois do relatório de base; o diff, que é o que mais cresce, fica por último e é o que corta.
pub(crate) fn with_agent(base: &str, s: &AgentSection, secrets: &Secrets) -> String {
    let recheck = match &s.recheck {
        Recheck::Passed => tr("setup_report_recheck_passed"),
        Recheck::Failed(detail) => tr("setup_report_recheck_failed").replace("{detail}", detail),
        Recheck::NotRun(detail) => tr("setup_report_recheck_not_run").replace("{detail}", detail),
    };
    let commands = s.commands.iter().map(|c| format!("$ {c}")).collect::<Vec<_>>().join("\n");
    let mut section = format!("\n\n# {}\n\n## {}\n\n{recheck}\n\n## {}\n\n{}\n\n## {}\n\n{commands}\n",
        tr("setup_report_agent").replace("{agente}", &s.agent), tr("setup_report_recheck"), tr("setup_report_explanation"),
        s.explanation, tr("setup_report_commands"));
    for note in &s.notes { section.push_str(&format!("\n{note}\n")); }
    let diff = if s.diff.is_empty() { tr("setup_report_no_diff") } else { s.diff.clone() };
    section.push_str(&format!("\n## {}\n\n{diff}\n", tr("setup_report_diff")));
    let section = scrub(&section, secrets);
    if json_len(&section) <= AGENT_MAX { return format!("{base}{section}"); }
    let mut size = 0;
    let end = section.char_indices().find(|&(_, c)| { size += json_char(c); size > AGENT_MAX }).map_or(section.len(), |(i, _)| i);
    format!("{base}{}\n{}\n", &section[..end], tr("setup_report_cut"))
}

pub(crate) fn restore_notes(head_moved: Option<&(String, String)>, errors: &[String], timed_out: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if timed_out { notes.push(tr("setup_report_timed_out").replace("{min}", &(super::agent::MAX_RUN.as_secs() / 60).to_string())); }
    if let Some((before, after)) = head_moved { notes.push(tr("setup_report_head_moved").replace("{before}", before).replace("{after}", after)); }
    if !errors.is_empty() { notes.push(tr("setup_report_not_restored").replace("{detail}", &errors.join("; "))); }
    notes
}

/// O Python e a pasta `backend` do doctor: no clone, o venv; no pacote, o Python embutido da versão em `current`.
pub(crate) fn doctor_paths(mode: Mode, dest: &Path, windows: bool) -> (PathBuf, PathBuf) {
    // Os caminhos do `install_root::python_exe` do `hangar-server`, que este crate não importa.
    let root = match mode { Mode::Git => dest.to_owned(), Mode::Package => dest.join("current") };
    let python = match (mode, windows) {
        (Mode::Git, true) => root.join(r"backend\.venv\Scripts\python.exe"),
        (Mode::Git, false) => root.join("backend/.venv/bin/python"),
        (Mode::Package, true) => root.join("python").join("python.exe"),
        (Mode::Package, false) => root.join("python").join("bin").join("python3"),
    };
    (python, root.join("backend"))
}

/// `python -m app.doctor` com o PATH refeito. Sem o Python, o backend ainda não existe: `None`.
pub(crate) fn doctor(mode: Mode, dest: &Path) -> Option<String> {
    let (python, backend) = doctor_paths(mode, dest, cfg!(windows));
    if !python.is_file() { return None; }
    // `--qr` imprimiria a URL com o token; sem ele o doctor só lista itens.
    let mut command = Command::new(python);
    hidden(&mut command).args(["-m", "app.doctor"]).current_dir(backend)
        .env("PATH", refreshed_path()).env("PYTHONIOENCODING", "utf-8");
    Some(run_with_timeout(command, DOCTOR_TIMEOUT))
}

/// Saída (stdout + stderr) do comando; passou de `limit`, ele é morto e o relatório diz isso, em vez de travar o envio.
fn run_with_timeout(mut command: Command, limit: Duration) -> String {
    use std::io::Read;
    let mut child = match command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(error) => return format!("doctor: {error}"),
    };
    // Um fio por cano: encher um deles sem ninguém ler travaria o filho antes do prazo. O texto vai para um buffer
    // compartilhado e o fim da leitura é avisado por canal: um neto que herdou o cano não prende o relatório.
    fn drain(pipe: Option<impl Read + Send + 'static>) -> (Arc<Mutex<Vec<u8>>>, mpsc::Receiver<()>) {
        let (buf, (tx, rx)) = (Arc::new(Mutex::new(Vec::new())), mpsc::channel());
        let shared = Arc::clone(&buf);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 { break; }
                    shared.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]);
                }
            }
            let _ = tx.send(());
        });
        (buf, rx)
    }
    let (out, err) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let deadline = std::time::Instant::now() + limit;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let mut cut = false;
    let mut collect = |(buf, done): (Arc<Mutex<Vec<u8>>>, mpsc::Receiver<()>)| {
        cut |= done.recv_timeout(Duration::from_secs(2)).is_err();
        let bytes = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let (out, err) = (collect(out), collect(err));
    let mut text = format!("{out}{err}");
    if cut { text.push_str("\ndoctor: output cut (a child process kept the pipe open)\n"); }
    if timed_out { text.push_str(&format!("\ndoctor: timeout after {}s, stopped\n", limit.as_secs_f32().ceil())); }
    text
}

pub(crate) fn read_log(path: &Path) -> String {
    std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_else(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn system_line() -> String {
    let name = os_name().unwrap_or_else(|| std::env::consts::OS.to_owned());
    format!("{name} ({}-{})", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(target_os = "linux")]
fn os_name() -> Option<String> {
    std::fs::read_to_string("/etc/os-release").ok()?.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_owned())
}

#[cfg(windows)]
fn os_name() -> Option<String> {
    super::system::powershell("$o = Get-CimInstance Win32_OperatingSystem; \"$($o.Caption) $($o.Version)\"").ok()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn os_name() -> Option<String> { None }

pub(crate) fn app_line() -> String { format!("{} · {}", crate::update::CURRENT, super::run::COMMIT) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome { Consertado, Aberto }

/// O corpo do `POST /api/relatorio` (Task 5 confere cada campo).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Payload {
    pub v: u8,
    pub app: String,
    pub commit: String,
    pub os: String,
    pub step: String,
    pub code: Option<String>,
    pub outcome: Outcome,
    pub agent: Option<String>,
    pub report: String,
}

pub(crate) fn payload(step: Screen, code: Option<String>, outcome: Outcome, agent: Option<&str>, report: String) -> Payload {
    let code = code.filter(|c| (1..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
    let mut p = Payload { v: 1, app: crate::update::CURRENT.to_owned(), commit: super::run::COMMIT.to_owned(),
        os: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH), step: screen_slug(step).to_owned(), code, outcome,
        agent: agent.map(str::to_owned), report };
    // `compose`/`with_agent` já cabem; isto segura qualquer outro caminho: o Worker recusaria e "Enviar de novo" nunca
    // passaria. Corta o começo, o fim do log é o que importa.
    let body = serde_json::to_vec(&p).map_or(0, |b| b.len());
    if body > BODY_MAX { p.report = tail(&p.report, json_len(&p.report).saturating_sub(body - BODY_MAX + 1024)); }
    p
}

/// Carimbo do app: barra robô genérico, não alguém determinado (o código é aberto).
pub(crate) fn stamp() -> String { format!("hangar-native/{}", crate::update::CURRENT) }

/// `HANGAR_REPORT_URL` só para provar o envio com um receptor local, sem tocar no Worker de verdade.
pub(crate) async fn send(payload: Payload) -> Result<(), String> {
    let url = std::env::var("HANGAR_REPORT_URL").unwrap_or_else(|_| URL.to_owned());
    send_to(&url, payload).await
}

pub(crate) async fn send_to(url: &str, payload: Payload) -> Result<(), String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| describe(&e))?;
    let response = client.post(url).header("X-Hangar-Stamp", stamp()).json(&payload).send().await.map_err(|e| describe(&e))?;
    if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE { return Err(tr("setup_report_too_large")); }
    response.error_for_status().map(|_| ()).map_err(|e| describe(&e))
}

/// A mensagem de topo do reqwest ("error sending request") não diz a causa; ela está na cadeia de origens.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        Secrets { values: vec!["s3nh4-do-celular".into(), "abcdef0123456789token".into(), "adm1n!".into()],
            home: Some("/home/maria".into()), users: vec!["maria".into()] }
    }

    #[test]
    fn scrub_removes_known_secrets_and_token_assignments() {
        let text = "CP_AUTH_TOKEN=abcdef0123456789token\nabra http://192.168.0.5:5173/?token=outro-token-qualquer&api=x\n\
            sudo: adm1n! errado\nAuthorization: Bearer eyJabc.def\nHANGAR_TOKEN=s3nh4-do-celular\npassword=segredo123 ok\n";
        let out = scrub(text, &secrets());
        for leaked in ["abcdef0123456789token", "outro-token-qualquer", "adm1n!", "eyJabc.def", "s3nh4-do-celular", "segredo123", "192.168.0.5"] {
            assert!(!out.contains(leaked), "{leaked} vazou: {out}");
        }
        assert!(out.contains("CP_AUTH_TOKEN=<senha>"));
        assert!(out.contains("&api=x"));
        assert!(out.contains("HANGAR_TOKEN=<senha>"));
    }

    #[test]
    fn scrub_removes_the_askpass_one_time_code() {
        let code = "0123456789abcdef0123456789abcdef";
        let s = Secrets::here(vec![code.into()]);
        let out = scrub(&format!("HANGAR_ASKPASS_CODE vazou {code} no log"), &s);
        assert!(!out.contains(code), "{out}");
        assert!(out.contains("vazou <senha> no log"), "{out}");
    }

    #[test]
    fn scrub_swaps_user_paths_tailnet_names_and_ips_but_keeps_loopback() {
        let s = Secrets { values: vec![], home: Some("/home/maria".into()), users: vec!["maria".into()] };
        let out = scrub("venv em /home/maria/hangar/.venv\noutro /srv/maria/x e /home/mariana/y\n\
            https://pc-da-maria.tail1234.ts.net:8443\nip 100.64.0.2 e 127.0.0.1 e 0.0.0.0:8765 e fe80::1c2b:3a4d e ::1\n\
            versão 1.2.3 e 10.0.19045.1 e 12:34:56 e app::doctor\n", &s);
        assert!(out.contains("venv em ~/hangar/.venv"), "{out}");
        assert!(out.contains("/srv/<usuario>/x"), "{out}");
        // Nome inteiro, nunca pedaço: "/home/mariana" é outra pessoa.
        assert!(out.contains("/home/mariana/y"), "{out}");
        assert!(out.contains("https://<maquina>.ts.net:8443"), "{out}");
        assert!(out.contains("ip <ip> e 127.0.0.1 e 0.0.0.0:8765 e <ip> e ::1"), "{out}");
        assert!(out.contains("versão 1.2.3 e 10.0.19045.1 e 12:34:56 e app::doctor"), "{out}");
    }

    #[test]
    fn scrub_handles_windows_paths_in_any_case() {
        let s = Secrets { values: vec![], home: Some(r"C:\Users\Maria".into()), users: vec!["Maria".into()] };
        let out = scrub(r"C:\Users\Maria\hangar e c:/users/maria/x e D:\Dados\maria\y", &s);
        assert_eq!(out, r"~\hangar e ~/x e D:\Dados\<usuario>\y");
    }

    #[test]
    fn doctor_runs_the_python_of_each_mode() {
        let clone = Path::new("/home/dev/hangar");
        assert_eq!(doctor_paths(Mode::Git, clone, false), (clone.join("backend/.venv/bin/python"), clone.join("backend")));
        assert_eq!(doctor_paths(Mode::Git, clone, true).0, clone.join(r"backend\.venv\Scripts\python.exe"));
        let hangar = Path::new("/home/dev/.hangar");
        assert_eq!(doctor_paths(Mode::Package, hangar, false), (hangar.join("current/python/bin/python3"), hangar.join("current/backend")));
        assert_eq!(doctor_paths(Mode::Package, hangar, true), (hangar.join("current").join("python").join("python.exe"), hangar.join("current/backend")));
    }

    fn facts(log: String) -> Facts {
        Facts { step: Screen::Install, code: Some("sem-systemd".into()), text: "este Linux não tem como iniciar o Hangar sozinho".into(),
            system: "Ubuntu 26.04 (linux-x86_64)".into(), app: "0.20.1.3456 · abc123".into(), doctor: None,
            logs: vec![("install".into(), log)] }
    }

    #[test]
    fn compose_has_step_code_system_app_doctor_and_log() {
        let out = compose(&facts("linha 1\n##HANGAR-ERRO## sem-systemd\nabra ?token=abcdef0123456789token\n".into()), &secrets());
        for needle in ["instalar", "sem-systemd", "Ubuntu 26.04", "0.20.1.3456 · abc123", "linha 1", tr("setup_report_doctor_missing").as_str()] {
            assert!(out.contains(needle), "{needle} faltou: {out}");
        }
        assert!(!out.contains("abcdef0123456789token"));
    }

    #[test]
    fn compose_keeps_the_end_of_a_huge_log_under_the_limit() {
        let log: String = (0..200_000).map(|n| format!("linha {n}\n")).collect();
        let out = compose(&facts(log), &secrets());
        assert!(out.len() <= BASE_MAX, "{}", out.len());
        assert!(out.contains("linha 199999"));
        assert!(!out.contains("linha 0\n"));
        assert!(out.contains("sem-systemd"));
    }

    #[test]
    fn agent_section_is_scrubbed_capped_and_keeps_the_recheck_first() {
        let section = AgentSection { agent: "Claude Code".into(), commands: vec!["cat backend/.env".into()],
            explanation: "o token era abcdef0123456789token".into(), diff: "+x\n".repeat(40_000), notes: vec![],
            recheck: Recheck::Failed("sem-systemd".into()) };
        let out = with_agent("base", &section, &secrets());
        assert!(out.starts_with("base"));
        assert!(out.len() <= "base".len() + AGENT_MAX + 64);
        assert!(!out.contains("abcdef0123456789token"));
        assert!(out.contains("$ cat backend/.env"));
        assert!(out.find("sem-systemd") < out.find("+x"));
    }

    #[test]
    fn payload_carries_the_worker_contract() {
        let p = payload(Screen::Install, Some("sem-systemd".into()), Outcome::Aberto, Some("claude"), "texto".into());
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["step"], "instalar");
        assert_eq!(v["code"], "sem-systemd");
        assert_eq!(v["outcome"], "aberto");
        assert_eq!(v["agent"], "claude");
        assert!(v["os"].as_str().unwrap().starts_with(std::env::consts::OS));
        // Código fora do formato do Worker (`[a-z0-9-]`) vai como nulo: o texto do relatório já o leva.
        let odd = payload(Screen::Install, Some("Disco Cheio!".into()), Outcome::Consertado, None, "t".into());
        assert_eq!(serde_json::to_value(&odd).unwrap()["code"], serde_json::Value::Null);
        assert_eq!(serde_json::to_value(&odd).unwrap()["outcome"], "consertado");
        assert!(stamp().starts_with("hangar-native/"));
    }

    #[test]
    fn send_error_names_the_cause() {
        // Porta fechada (aberta e solta agora): o envio falha com o motivo, nunca em silêncio.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let p = payload(Screen::Install, None, Outcome::Aberto, None, "t".into());
        let error = runtime.block_on(send_to(&format!("http://127.0.0.1:{port}/api/relatorio"), p)).unwrap_err();
        // A mensagem de topo do reqwest só diz "error sending request"; a causa vem na cadeia (`Connect`/recusada).
        assert!(error.to_lowercase().contains("connect"), "{error}");
        assert!(error.contains(": "), "{error}");
    }

    #[test]
    fn scrub_handles_quoted_json_and_authorization_values() {
        let s = Secrets { values: vec![], home: None, users: vec![] };
        let out = scrub("a password=\"com espaco\" b token='outro seg' c token=cru&x=1\n\
            {\"token\": \"jsonsecret\", \"auth_token\":\"j2\", \"password\" : \"j3\"}\n\
            Authorization: Basic dXNlcjpwYXNz\nAuthorization: Bearer eyJabc.def\n", &s);
        for leaked in ["com espaco", "outro seg", "cru", "jsonsecret", "j2", "j3", "dXNlcjpwYXNz", "eyJabc.def"] {
            assert!(!out.contains(leaked), "{leaked} vazou: {out}");
        }
        assert!(out.contains("password=\"<senha>\""), "{out}");
        assert!(out.contains("token='<senha>'"), "{out}");
        assert!(out.contains("token=<senha>&x=1"), "{out}");
        assert!(out.contains("\"token\": \"<senha>\""), "{out}");
    }

    #[test]
    fn scrub_replaces_ipv4_after_a_colon_and_inside_mapped_ipv6() {
        let s = Secrets { values: vec![], home: None, users: vec![] };
        let out = scrub("inet addr:192.168.0.5 IP:100.64.0.2 ::ffff:192.168.0.5 e ::ffff:127.0.0.1 e addr:127.0.0.1 e 0.0.0.0:8765", &s);
        assert_eq!(out, "inet addr:<ip> IP:<ip> <ip> e ::ffff:127.0.0.1 e addr:127.0.0.1 e 0.0.0.0:8765");
    }

    #[test]
    fn scrub_system_named_user_only_under_home_roots() {
        let s = Secrets { values: vec![], home: Some("/home/maria".into()), users: vec!["maria".into(), "dev".into()] };
        let out = scrub("/home/maria.old/x /dev/null /mnt/c/Users/dev/y /home/dev/z /usr/lib", &s);
        assert_eq!(out, "~.old/x /dev/null /mnt/c/Users/<usuario>/y /home/<usuario>/z /usr/lib");
        // HOME vazio ou "/" não vira "troque toda barra".
        let root = Secrets { values: vec![], home: Some("/".into()), users: vec![] };
        assert_eq!(scrub("/usr/bin/x", &root), "/usr/bin/x");
        let empty = Secrets { values: vec![], home: Some(String::new()), users: vec![] };
        assert_eq!(scrub("/usr/bin/x", &empty), "/usr/bin/x");
    }

    #[test]
    fn compose_caps_a_huge_text_and_doctor_and_still_keeps_the_log_end() {
        let mut f = facts((0..50_000).map(|n| format!("linha {n}\n")).collect());
        f.text = "erro ".repeat(100_000);
        f.doctor = Some((0..100_000).map(|n| format!("item {n}\n")).collect());
        let out = compose(&f, &secrets());
        assert!(out.len() <= BASE_MAX, "{}", out.len());
        assert!(out.contains("linha 49999"));
        assert!(out.contains("item 99999"));
    }

    #[test]
    fn scrub_drops_ansi_colors_and_progress_carriage_returns() {
        let s = Secrets { values: vec![], home: None, users: vec![] };
        let out = scrub("\x1b[1;31merro\x1b[0m: x\n10%\r50%\r100% pronto\r\n\x1b]0;titulo\x07fim\tok\x08\n", &s);
        assert_eq!(out, "erro: x\n100% pronto\nfim\tok\n");
    }

    #[test]
    fn a_huge_colored_log_fits_the_worker_body_and_keeps_its_end() {
        // Cada linha do `install.sh` vem pintada; caminho do Windows e aspas pesam o dobro no JSON.
        let log: String = (0..200_000).map(|n| format!("\x1b[32mok\x1b[0m C:\\Hangar\\x \"q\" linha {n}\r\n")).collect();
        let base = compose(&facts(log), &secrets());
        assert!(!base.contains('\x1b') && !base.contains('\r'));
        let section = AgentSection { agent: "Claude Code".into(), commands: vec![], explanation: "x".into(),
            diff: "+\"C:\\\\a\\\\b\"\n".repeat(40_000), notes: vec![], recheck: Recheck::Passed };
        let report = with_agent(&base, &section, &secrets());
        let p = payload(Screen::Install, Some("sem-systemd".into()), Outcome::Aberto, Some("claude"), report);
        let body = serde_json::to_vec(&p).unwrap().len();
        assert!(body <= BODY_MAX, "{body}");
        assert!(p.report.contains("linha 199999") && p.report.contains("sem-systemd"));
    }

    #[test]
    fn payload_cuts_the_head_of_a_report_too_big_for_the_worker() {
        let report: String = (0..60_000).map(|n| format!("\"\\\" {n}\n")).collect();
        let p = payload(Screen::Install, None, Outcome::Aberto, None, report);
        assert!(serde_json::to_vec(&p).unwrap().len() <= BODY_MAX);
        assert!(p.report.ends_with("\"\\\" 59999\n"));
    }

    #[test]
    fn scrub_catches_api_keys_secrets_and_escaped_json() {
        let s = Secrets { values: vec![], home: None, users: vec![] };
        let out = scrub("OPENAI_API_KEY=sk-k1 apikey: k2 client_secret='k3' SECRET=\"k4\" access_token=k5\n\
            {\"api_key\": \"k6\"} log: {\\\"password\\\": \\\"k7 com espaco\\\", \\\"secret\\\":\\\"k8\\\"}\n", &s);
        for leaked in ["k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8"] {
            assert!(!out.contains(leaked), "{leaked} vazou: {out}");
        }
        assert!(out.contains("{\\\"password\\\": \\\"<senha>\\\""), "{out}");
    }

    #[test]
    fn scrub_keeps_ipv6_before_a_sentence_period() {
        let s = Secrets { values: vec![], home: None, users: vec![] };
        assert_eq!(scrub("rota fe80::1c2b:3a4d. fim", &s), "rota <ip>. fim");
    }

    #[cfg(unix)]
    #[test]
    fn doctor_output_is_cut_when_a_grandchild_holds_the_pipe() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 20 & echo antes"]);
        let started = std::time::Instant::now();
        let out = run_with_timeout(command, Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(out.contains("antes") && out.contains("output cut"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn doctor_run_is_killed_on_timeout() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo antes; exec sleep 30"]);
        let started = std::time::Instant::now();
        let out = run_with_timeout(command, Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(out.contains("antes") && out.contains("timeout"), "{out}");
    }
}
