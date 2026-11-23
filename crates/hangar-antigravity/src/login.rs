//! Login de uma conta pelo próprio `agy acp --login`: ele imprime o link do Google e espera o código (ou o
//! endereço da página onde o Google parou). O Hangar entrega o link à tela da pessoa e escreve o que ela colou na
//! entrada do processo. Quem troca o código pelo token e o guarda é o `agy`; o Hangar nunca vê o token.
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::env::ProcessEnv;

const LINK_PREFIX: &str = "https://accounts.google.com/o/oauth2/auth?";
const CALLBACK: &str = "https://antigravity.google/oauth-callback";
const CALLBACK_ENCODED: &str = "redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";
/// Guarda só o fim da saída para o diagnóstico: o resto é o link, que leva `state`.
const TAIL_LINES: usize = 8;

/// Argumentos e ambiente do processo de login: sem tela, nada abre navegador na máquina.
pub fn command(mut env: ProcessEnv) -> (Vec<String>, ProcessEnv) {
    for key in ["DISPLAY", "WAYLAND_DISPLAY"] {
        if !env.unset.iter().any(|k| k == key) { env.unset.push(key.to_owned()); }
    }
    env.unset.retain(|k| k != "BROWSER");
    env.set.retain(|(k, _)| k != "BROWSER");
    env.set.push(("BROWSER".to_owned(), "true".to_owned()));
    (vec!["acp".to_owned(), "--login".to_owned()], env)
}

/// O link de login numa linha da saída, se for o do Google com volta para a página do Antigravity.
pub fn link_in(line: &str) -> Option<String> {
    let url = line.trim();
    (url.starts_with(LINK_PREFIX) && url.contains(CALLBACK_ENCODED) && !url.chars().any(char::is_whitespace)).then(|| url.to_owned())
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodeError { Empty, Invalid }

/// O que a pessoa colou: o código, ou o endereço inteiro da página de volta (que o `agy` também aceita).
pub fn check_code(input: &str) -> Result<String, CodeError> {
    let text = input.trim();
    if text.is_empty() { return Err(CodeError::Empty); }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) { return Err(CodeError::Invalid); }
    if let Some(query) = text.strip_prefix(CALLBACK) {
        let has_code = query.trim_start_matches('/').strip_prefix('?').is_some_and(|q| q.split('&').any(|p| p.starts_with("code=") && p.len() > 5));
        return if has_code { Ok(text.to_owned()) } else { Err(CodeError::Invalid) };
    }
    let ok = (10..=2048).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-_.~%".contains(&b));
    if ok { Ok(text.to_owned()) } else { Err(CodeError::Invalid) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    /// O link está pronto para a tela.
    Waiting { link: String },
    Done { ok: bool, tail: Vec<String> },
}

#[derive(Default)]
struct Shared { link: Option<String>, tail: Vec<String> }

/// Uma tentativa de login em andamento. Derrubar a tentativa mata o processo.
pub struct Attempt {
    child: Child,
    stdin: Option<ChildStdin>,
    shared: Arc<Mutex<Shared>>,
    exit: Option<bool>,
}

impl Attempt {
    pub fn start(program: &Path, cwd: &Path, env: ProcessEnv) -> std::io::Result<Self> {
        let (args, env) = command(env);
        let mut cmd = Command::new(program);
        cmd.args(&args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for key in &env.unset { cmd.env_remove(key); }
        for (key, value) in &env.set { cmd.env(key, value); }
        let mut child = cmd.spawn()?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let shared = shared.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    match link_in(&line) {
                        Some(link) => state.link = Some(link),
                        None if !line.trim().is_empty() => {
                            state.tail.push(line);
                            let excess = state.tail.len().saturating_sub(TAIL_LINES);
                            state.tail.drain(..excess);
                        }
                        None => {}
                    }
                }
            });
        }
        let stdin = child.stdin.take();
        Ok(Self { child, stdin, shared, exit: None })
    }

    pub fn status(&mut self) -> Status {
        if self.exit.is_none() {
            if let Ok(Some(code)) = self.child.try_wait() { self.exit = Some(code.success()); }
        }
        let state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        match (self.exit, &state.link) {
            (Some(ok), _) => Status::Done { ok, tail: state.tail.clone() },
            (None, Some(link)) => Status::Waiting { link: link.clone() },
            (None, None) => Status::Starting,
        }
    }

    /// Escreve o código conferido; a resposta do `agy` aparece no `status` quando o processo termina.
    pub fn submit(&mut self, input: &str) -> Result<(), SubmitError> {
        let code = check_code(input).map_err(SubmitError::Code)?;
        let stdin = self.stdin.as_mut().ok_or(SubmitError::Closed)?;
        stdin.write_all(format!("{code}\n").as_bytes()).and_then(|_| stdin.flush()).map_err(SubmitError::Io)?;
        self.stdin = None;
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if self.exit.is_none() && matches!(self.child.try_wait(), Ok(None)) { self.cancel(); }
    }
}

#[derive(Debug)]
pub enum SubmitError { Code(CodeError), Closed, Io(std::io::Error) }

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x.apps.googleusercontent.com&code_challenge=abc&code_challenge_method=S256&prompt=consent&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback&response_type=code&scope=openid&state=s1";

    #[test]
    fn finds_the_google_link_with_the_antigravity_callback_only() {
        assert_eq!(link_in(&format!("  {LINK}")).as_deref(), Some(LINK));
        assert!(link_in("Open this link in a browser to log in with Google:").is_none());
        assert!(link_in(&LINK.replace("antigravity.google", "evil.example")).is_none());
        assert!(link_in(&LINK.replace("accounts.google.com", "accounts.google.com.evil.example")).is_none());
    }

    #[test]
    fn accepts_a_code_or_the_callback_page_and_refuses_the_rest() {
        assert_eq!(check_code(" 4/0AVGzR1A-abcdefghij_klmn "), Ok("4/0AVGzR1A-abcdefghij_klmn".into()));
        assert!(check_code("https://antigravity.google/oauth-callback?state=s1&code=4/0AVG").is_ok());
        assert_eq!(check_code("https://antigravity.google/oauth-callback?state=s1"), Err(CodeError::Invalid));
        assert_eq!(check_code("abc def ghi jkl"), Err(CodeError::Invalid));
        assert_eq!(check_code("abc\nrm -rf"), Err(CodeError::Invalid));
        assert_eq!(check_code("   "), Err(CodeError::Empty));
        assert_eq!(check_code("curto"), Err(CodeError::Invalid));
    }

    #[test]
    fn the_login_never_opens_a_browser_on_the_machine() {
        let (args, env) = command(ProcessEnv { set: vec![("BROWSER".into(), "firefox".into())], unset: vec![] });
        assert_eq!(args, ["acp", "--login"]);
        assert_eq!(env.set.iter().filter(|(k, _)| k == "BROWSER").map(|(_, v)| v.as_str()).collect::<Vec<_>>(), ["true"]);
        assert!(env.unset.contains(&"DISPLAY".to_owned()) && env.unset.contains(&"WAYLAND_DISPLAY".to_owned()));
    }
}
