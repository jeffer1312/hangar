//! O que se sabe de uma conta sem ler o token: o próprio agy responde se há login (agy models), o arquivo de
//! primeiro uso diz se os Termos já foram aceitos, e o logout do ACP encerra o login.
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::env::ProcessEnv;
use crate::Account;

const SIGNED_OUT: &str = "sign in";

fn command(program: &Path, cwd: &Path, env: &ProcessEnv) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for key in &env.unset { cmd.env_remove(key); }
    for (key, value) in &env.set { cmd.env(key, value); }
    cmd
}

/// Roda o agy com os argumentos e devolve (saiu com sucesso, stdout+stderr); None se passou do prazo.
fn run(program: &Path, cwd: &Path, env: &ProcessEnv, args: &[&str], limit: Duration) -> std::io::Result<Option<(bool, String)>> {
    let mut child = command(program, cwd, env).args(args).spawn()?;
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    std::thread::spawn(move || {
        let mut text = String::new();
        for stream in [stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stream), &mut text);
        }
        let _ = tx.send(text);
    });
    let text = match rx.recv_timeout(limit) {
        Ok(text) => text,
        Err(_) => { let _ = child.kill(); let _ = child.wait(); return Ok(None) }
    };
    let status = child.wait()?;
    Ok(Some((status.success(), text)))
}

/// Some(true) com login, Some(false) sem; None quando o agy não respondeu no prazo ou falhou de outro jeito.
pub fn signed_in(program: &Path, cwd: &Path, env: &ProcessEnv, limit: Duration) -> std::io::Result<Option<bool>> {
    Ok(run(program, cwd, env, &["models"], limit)?.and_then(|(ok, text)| answer(ok, &text)))
}

/// Os modelos que a conta pode usar, como o agy models lista (id, nome). O esforço vem no id
/// (gemini-4-neon-high). None sem login, no prazo estourado ou com saída que não é a lista.
pub fn models(program: &Path, cwd: &Path, env: &ProcessEnv, limit: Duration) -> std::io::Result<Option<Vec<(String, String)>>> {
    Ok(run(program, cwd, env, &["models"], limit)?.and_then(|(ok, text)| ok.then(|| model_lines(&text)).filter(|m| !m.is_empty())))
}

fn model_lines(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|line| {
        let (id, name) = line.split_once('\t')?;
        let id = id.trim();
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))).then(|| (id.to_owned(), name.trim().to_owned()))
    }).collect()
}

fn answer(success: bool, text: &str) -> Option<bool> {
    if text.to_lowercase().contains(SIGNED_OUT) { return Some(false); }
    (success && text.lines().any(|l| l.contains('\t'))).then_some(true)
}

/// Os Termos do Google aceitos nesta pasta (cache/onboarding.json). Pasta nova abre a tela de aceite na
/// primeira sessão com terminal; quem aceita é a pessoa, nunca o Hangar.
pub fn onboarding_complete(account: &Account, home: &Path) -> bool {
    std::fs::read_to_string(account.data_dir(home).join("cache").join("onboarding.json")).ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|value| value["onboardingComplete"] == true)
}

/// Encerra o login da conta pelo método logout do ACP. Ok(false) quando o agy recusou ou não respondeu.
pub fn logout(program: &Path, cwd: &Path, env: &ProcessEnv, limit: Duration) -> std::io::Result<bool> {
    let mut cmd = command(program, cwd, env);
    cmd.arg("acp").stdin(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("stdout"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() { break; }
        }
    });
    let send = |stdin: &mut std::process::ChildStdin, value: serde_json::Value| writeln!(stdin, "{value}").and_then(|_| stdin.flush());
    send(&mut stdin, serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": crate::acp::PROTOCOL_VERSION,
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}, "clientInfo": {"name": "hangar"}}}))?;
    let deadline = std::time::Instant::now() + limit;
    let (mut asked, mut done) = (false, false);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(line) = rx.recv_timeout(left) else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        match value["id"].as_u64() {
            Some(1) if !asked => { send(&mut stdin, serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "logout", "params": {}}))?; asked = true; }
            Some(2) => { done = value.get("error").is_none(); break; }
            _ => {}
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_models_answer_says_if_there_is_a_login() {
        assert_eq!(answer(true, "Fetching available models...\ngemini-4-neon-high\tGemini 4 Neon (High)\n"), Some(true));
        assert_eq!(answer(false, "Fetching available models...\nError: Please sign in to view available models."), Some(false));
        assert_eq!(answer(false, "boom"), None);
    }

    #[test]
    fn the_model_list_keeps_only_tab_separated_ids() {
        let text = "Fetching available models...
gemini-4-neon-high	Gemini 4 Neon (High)
gemini-3.8-flash-low	Gemini 3.8 Flash (Low)
";
        assert_eq!(model_lines(text), [("gemini-4-neon-high".to_owned(), "Gemini 4 Neon (High)".to_owned()), ("gemini-3.8-flash-low".to_owned(), "Gemini 3.8 Flash (Low)".to_owned())]);
    }

    #[test]
    fn the_onboarding_file_says_if_the_terms_were_accepted() {
        let home = tempfile::tempdir().unwrap();
        let account = Account::default();
        assert!(!onboarding_complete(&account, home.path()));
        let cache = account.data_dir(home.path()).join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("onboarding.json"), r#"{"consumerOnboardingComplete":true,"onboardingComplete":true}"#).unwrap();
        assert!(onboarding_complete(&account, home.path()));
    }
}
