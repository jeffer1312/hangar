//! O comando de conferência do loop: argv sem shell, na pasta da sessão, com prazo; a saída volta
//! como a cauda que o re-prompt mostra. Comando que nem chega a rodar é `UNRUNNABLE` (o loop falha).
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const TIMEOUT: Duration = Duration::from_secs(600);
pub const UNRUNNABLE: i64 = -404;
const TAIL_CHARS: usize = 8 * 1024;

/// O `str.strip()` do Python também tira os separadores `\x1c`–`\x1f`.
pub fn py_space(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }

/// `shlex.split` (POSIX, sem comentários). `escapes: false` é o Windows: a contrabarra é parte do
/// caminho, nunca escape.
pub fn split(cmd: &str, escapes: bool) -> Result<Vec<String>, &'static str> {
    #[derive(Clone, Copy, PartialEq)]
    enum S { Space, Word, Quote(char), Escape(Option<char>) }
    let ws = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let (mut out, mut token, mut quoted, mut state) = (Vec::new(), String::new(), false, S::Space);
    for c in cmd.chars() {
        state = match state {
            S::Space | S::Word if ws(c) => {
                if state == S::Word || quoted { out.push(std::mem::take(&mut token)); quoted = false; }
                S::Space
            }
            S::Space | S::Word if c == '\\' && escapes => S::Escape(None),
            S::Space | S::Word if c == '"' || c == '\'' => { quoted = true; S::Quote(c) }
            S::Space | S::Word => { token.push(c); S::Word }
            S::Quote(q) if c == q => S::Word,
            S::Quote('"') if c == '\\' && escapes => S::Escape(Some('"')),
            S::Quote(q) => { token.push(c); S::Quote(q) }
            S::Escape(inside) => {
                // Dentro de aspas duplas só `\"` e `\\` escapam; o resto guarda a contrabarra.
                if inside.is_some_and(|q| c != '\\' && c != q) { token.push('\\'); }
                token.push(c);
                inside.map_or(S::Word, S::Quote)
            }
        };
    }
    match state {
        S::Quote(_) => Err("No closing quotation"),
        S::Escape(_) => Err("No escaped character"),
        S::Word => { out.push(token); Ok(out) }
        S::Space => { if quoted { out.push(token); } Ok(out) }
    }
}

/// O texto do `OSError` do Python (`[Errno 2] No such file or directory: 'x'`).
fn os_error(e: &std::io::Error, what: &str) -> String {
    let text = e.to_string();
    let text = text.rsplit_once(" (os error ").map_or(text.as_str(), |(t, _)| t);
    match e.raw_os_error() {
        Some(code) => format!("[Errno {code}] {text}: '{what}'"),
        None => format!("{text}: '{what}'"),
    }
}

/// No Windows o `npm` do PATH é `npm.cmd`: sem a extensão, o processo não acha o programa.
#[cfg(windows)]
fn program(prog: &str, cwd: &Path) -> PathBuf {
    let path = Path::new(prog);
    if path.extension().is_some() { return path.to_owned(); }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs: Vec<PathBuf> = if prog.contains(['\\', '/']) { vec![cwd.to_owned()] }
        else { std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default() };
    dirs.iter().flat_map(|d| exts.split(';').filter(|e| !e.is_empty()).map(move |e| d.join(format!("{prog}{e}"))))
        .find(|c| c.is_file()).unwrap_or_else(|| path.to_owned())
}

#[cfg(not(windows))]
fn program(prog: &str, _cwd: &Path) -> PathBuf { PathBuf::from(prog) }

/// Saída do Python com `text=True`: UTF-8 com troca do inválido e quebras de linha universais.
fn decode(raw: &[u8]) -> String { String::from_utf8_lossy(raw).replace("\r\n", "\n").replace('\r', "\n") }

fn tail(text: &str) -> String {
    let n = text.chars().count();
    text.chars().skip(n.saturating_sub(TAIL_CHARS)).collect()
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status.code().map(i64::from).or_else(|| status.signal().map(|s| -i64::from(s))).unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i64 { status.code().map_or(-1, i64::from) }

/// `(código, cauda)`. Estourado o prazo, o grupo inteiro do comando morre (netos incluídos) e volta
/// `(124, "timeout Ns")`.
pub async fn run(cmd: &str, cwd: &Path, timeout: Duration) -> (i64, String) {
    let argv = match split(cmd, cfg!(not(windows))) {
        Ok(argv) => argv,
        Err(e) => return (UNRUNNABLE, e.into()),
    };
    let Some(prog) = argv.first() else { return (UNRUNNABLE, "comando de verificação vazio".into()) };
    if !cwd.is_dir() {
        return (UNRUNNABLE, os_error(&std::io::Error::from(std::io::ErrorKind::NotFound), &cwd.to_string_lossy()));
    }
    let mut command = tokio::process::Command::new(program(prog, cwd));
    command.args(&argv[1..]).current_dir(cwd).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
    let mut tree = match crate::terminal_process::CommandTree::configure(&mut command) {
        Ok(tree) => tree,
        Err(e) => return (UNRUNNABLE, os_error(&e, prog)),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return (UNRUNNABLE, os_error(&e, prog)),
    };
    if let Err(e) = tree.attach(&child) {
        crate::terminal_process::finish(&mut tree).await;
        let _ = child.wait().await;
        return (UNRUNNABLE, os_error(&e, prog));
    }
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let done = tokio::time::timeout(timeout, async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let (ro, re) = tokio::join!(out.read_to_end(&mut o), err.read_to_end(&mut e));
        let status = child.wait().await;
        (ro.and(re).and(status), o, e)
    }).await;
    crate::terminal_process::finish(&mut tree).await;
    match done {
        Err(_) => {
            let _ = child.wait().await;
            (124, format!("timeout {}s", timeout.as_secs()))
        }
        Ok((Ok(status), o, e)) => {
            let text = format!("{}\n{}", decode(&o), decode(&e));
            (exit_code(status), tail(text.trim_matches(py_space)))
        }
        Ok((Err(e), _, _)) => (UNRUNNABLE, os_error(&e, prog)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn golden() -> Value { super::super::tests::golden() }

    #[test]
    fn split_matches_python_shlex() {
        for c in golden()["splits"].as_array().unwrap() {
            let got = split(c["in"].as_str().unwrap(), true);
            match c.get("argv") {
                Some(argv) => assert_eq!(serde_json::to_value(got.unwrap()).unwrap(), *argv, "{c}"),
                None => assert_eq!(got, Err(c["error"].as_str().unwrap()), "{c}"),
            }
        }
    }

    /// B5: no Windows a contrabarra fica no caminho.
    #[test]
    fn windows_split_keeps_backslashes() {
        assert_eq!(split(r"C:\Users\x\npm.cmd run", false).unwrap(), [r"C:\Users\x\npm.cmd", "run"]);
        assert_eq!(split(r#""C:\Program Files\node.exe" a\b"#, false).unwrap(), [r"C:\Program Files\node.exe", r"a\b"]);
        assert_eq!(split(r"fim\", false).unwrap(), [r"fim\"]);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn real_commands_match_the_python_check() {
        let dir = tempfile::tempdir().unwrap();
        let noexec = dir.path().join("noexec.sh");
        std::fs::write(&noexec, "#!/bin/sh\necho x\n").unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        for c in golden()["checks"].as_array().unwrap() {
            let cmd = c["cmd"].as_str().unwrap();
            let (code, tail) = run(cmd, dir.path(), Duration::from_secs(30)).await;
            match c.get("raises") {
                // O Python derruba a thread do tique com o comando só de espaços; aqui o loop falha.
                Some(_) => assert_eq!((code, tail.as_str()), (UNRUNNABLE, "comando de verificação vazio"), "{c}"),
                None => {
                    assert_eq!(code, c["exit"].as_i64().unwrap(), "{c}");
                    assert_eq!(tail.replace(&cwd, "<cwd>"), c["tail"].as_str().unwrap(), "{c}");
                }
            }
        }
    }

    /// B6: no prazo morre o grupo inteiro, não só o filho direto.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("neto.pid");
        let cmd = format!("sh -c 'sleep 300 & echo $! > {}; wait'", pidfile.display());
        // Neto vivo seguraria o `sh` no `wait` até o fim do `sleep`: a volta não pode demorar.
        let (code, tail) = tokio::time::timeout(Duration::from_secs(10), run(&cmd, dir.path(), Duration::from_millis(500))).await
            .expect("o prazo não derrubou o grupo");
        assert_eq!((code, tail.as_str()), (124, "timeout 0s"));
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        let alive = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "));
        assert!(!alive, "o neto {pid} ficou vivo");
    }

    #[tokio::test]
    async fn missing_cwd_is_unrunnable() {
        let (code, tail) = run("true", Path::new("/nao/existe/mesmo"), Duration::from_secs(5)).await;
        assert_eq!(code, UNRUNNABLE);
        assert!(tail.contains("/nao/existe/mesmo"), "{tail}");
    }

    proptest::proptest! {
        #[test]
        fn split_never_panics(s in ".{0,64}") { let _ = split(&s, true); let _ = split(&s, false); }
    }
}
