//! O alvo tmux do painel: a sessão pedida, ou o terminal de atalho cujo dono gravado no tmux é
//! quem pede (`shortcut_terminals.find`/`find_hangar`). O id vem do cliente; o dono, nunca.
use std::process::{Output, Stdio};

use super::TermConfig;

/// O multiplexador não respondeu: não dá para dizer que o terminal não existe.
#[derive(Debug)]
pub(crate) struct MuxDown;

pub(crate) async fn tmux(cfg: &TermConfig, args: &[&str]) -> Result<Output, MuxDown> {
    let mut command = crate::terminal_input::child_command(&cfg.program);
    if let Some(socket) = &cfg.socket {
        command.arg("-S").arg(socket);
    }
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(cfg.mux_timeout, command.output()).await {
        Ok(Ok(out)) => Ok(out),
        _ => Err(MuxDown),
    }
}

/// `has-session` só resolve sessão: `=` sem `:` de propósito (`tmux.has_session`).
pub(crate) async fn has_session(cfg: &TermConfig, name: &str) -> Result<bool, MuxDown> {
    Ok(tmux(cfg, &["has-session", "-t", &format!("={name}")]).await?.status.success())
}

/// O shell separado de uma sessão (`term-X`) nasce com `@cp_hidden` = 1; sessão comum com esse nome, não.
pub(crate) async fn hidden_shell(cfg: &TermConfig, name: &str) -> Result<bool, MuxDown> {
    let out = tmux(cfg, &["show-options", "-v", "-t", &format!("={name}:"), "@cp_hidden"]).await?;
    Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "1")
}

fn valid_ident(ident: &str) -> bool {
    ident.len() == 6 && ident.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Sessão tmux do atalho `ident` de `owner` (vazio = No Hangar). Um `list-sessions` por abertura.
pub(crate) async fn shortcut(cfg: &TermConfig, owner: &str, ident: &str) -> Result<Option<String>, MuxDown> {
    if !valid_ident(ident) {
        return Ok(None);
    }
    let out = tmux(cfg, &["list-sessions", "-F", "#{session_name}\t#{@cp_shortcut_owner}\t#{@cp_shortcut_id}"]).await?;
    // Recusa (sem servidor) é "nenhum terminal", como o `_read_rows`.
    if !out.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        let mut f = line.splitn(3, '\t');
        let (name, o, id) = (f.next()?, f.next()?, f.next()?);
        (name.starts_with("shortcut-") && o == owner && id == ident).then(|| name.to_string())
    }))
}
