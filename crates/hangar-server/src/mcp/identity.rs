//! Quem é a sessão que chama o MCP, pelo que ela manda no cabeçalho (`quem_chama.py`). Ordem:
//!
//! - `X-Hangar-Voice`: a assistente de voz, que não é sessão. Só com a assinatura que este servidor
//!   faz a cada chamada (`<nonce>.<hmac>`); presente e inválida é erro, sem cair nos outros.
//! - `X-Hangar-Key`: sidecar da sessão sem terminal ou `CP_SESSION_KEY` no ambiente da sessão com
//!   terminal. Não muda no rename nem no `/clear`. Presente, vence o pane:
//!   o filho pode ter herdado o `TMUX_PANE` de outra sessão.
//! - `X-Hangar-Pane`: dono da sessão com terminal (o rename não troca o pane id). Id que aparece em
//!   mais de uma sessão (o psmux numera por sessão) não resolve: nome errado é pior que nenhum.
//! - `X-Hangar-Session`: o carimbo do nascimento, só se a sessão ainda se chama assim.
use std::collections::BTreeSet;
use std::path::Path;

use axum::http::HeaderMap;
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::list::mux::Mux;

pub const VOICE: &str = "voz";
pub const HINT: &str = "nenhum cabeçalho de identidade resolveu uma sessão viva — mande X-Hangar-Key \
(CP_SESSION_KEY), X-Hangar-Pane (TMUX_PANE) ou X-Hangar-Session (CP_SESSION_NAME)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin { Voice, Key, Pane, Name }

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self { Origin::Voice => VOICE, Origin::Key => "chave", Origin::Pane => "pane", Origin::Name => "nome" }
    }
}

/// Onde as sessões desta máquina estão: os sidecars em `home` e os panes do multiplexador.
pub struct Local<'a> { pub home: &'a Path, pub mux: &'a Mux }

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h [u8]> {
    headers.get(name).map(|v| v.as_bytes().trim_ascii()).filter(|v| !v.is_empty())
}

/// `(nome, origem)`; o erro é a frase de por que nada resolveu. Nunca um chamador padrão.
pub async fn resolve(headers: &HeaderMap, local: &Local<'_>, secret: &str) -> Result<(String, Origin), String> {
    if let Some(value) = header(headers, "x-hangar-voice") {
        if voice_signature_valid(value, secret) { return Ok((VOICE.to_owned(), Origin::Voice)); }
        return Err("assinatura da voz inválida (o hangar-server reiniciou ou não é a voz)".into());
    }
    let text = |name| header(headers, name).map(|v| String::from_utf8_lossy(v).into_owned());
    let mut terminal_key = None;
    if let Some(key) = text("x-hangar-key") {
        let found = { let (home, key) = (local.home.to_path_buf(), key.clone()); tokio::task::spawn_blocking(move || by_key(&home, &key)).await.ok().flatten() };
        if let Some(name) = found { return Ok((name, Origin::Key)); }
        // Sessão com terminal aberta pelo Rust: a chave mora no registro dela, sem sidecar, e quem
        // diz o nome atual é o ambiente da sessão ou o pane.
        if !record_exists(local.home, &key) {
            return Err("chave de sessão sem terminal não corresponde a nenhum sidecar".into());
        }
        terminal_key = Some(key);
    }
    let (pane, name) = (text("x-hangar-pane"), text("x-hangar-session"));
    // ponytail: multiplexador que não responde vale como "nenhum pane", como o `list_panes_all`.
    let panes = if pane.is_some() || name.is_some() || terminal_key.is_some() { local.mux.list_panes().await.unwrap_or_default() } else { Vec::new() };
    if let Some(key) = terminal_key {
        // O psmux repete o id do pane entre sessões e o rename envelhece CP_SESSION_NAME; a chave no
        // ambiente não muda. As sessões do pane são perguntadas primeiro; sem dona entre elas (pane
        // herdado de outra sessão), as demais. Chave em mais de uma sessão (ambiente herdado) não
        // resolve, como o pane repetido.
        let (near, far): (BTreeSet<&str>, BTreeSet<&str>) = {
            let all: BTreeSet<&str> = panes.iter().map(|p| p.session.as_str()).collect();
            let near: BTreeSet<&str> = panes.iter().filter(|p| pane.as_ref().is_none_or(|id| &p.pane_id == id)).map(|p| p.session.as_str()).collect();
            let far = all.difference(&near).copied().collect();
            (near, far)
        };
        let mut owners = owners_of(local.mux, near, &key).await;
        if owners.is_empty() { owners = owners_of(local.mux, far, &key).await; }
        if let [owner] = owners[..] { return Ok((owner.to_owned(), Origin::Key)); }
    }
    if let Some(pane) = pane {
        let owners: BTreeSet<&str> = panes.iter().filter(|p| p.pane_id == pane).map(|p| p.session.as_str()).collect();
        if let [owner] = owners.into_iter().collect::<Vec<_>>()[..] { return Ok((owner.to_owned(), Origin::Pane)); }
    }
    if let Some(name) = name && (panes.iter().any(|p| p.session == name) || sidecar_exists(local.home, &name)) {
        return Ok((name, Origin::Name));
    }
    Err(HINT.into())
}

async fn owners_of<'a>(mux: &Mux, sessions: BTreeSet<&'a str>, key: &str) -> Vec<&'a str> {
    let keys = futures_util::future::join_all(sessions.iter().map(|s| mux.session_env(s, "CP_SESSION_KEY"))).await;
    sessions.into_iter().zip(keys).filter(|(_, k)| k.as_deref() == Some(key)).map(|(s, _)| s).collect()
}

/// A sessão existe nesta máquina (`quem_chama._por_nome`): pane no multiplexador ou sidecar.
pub async fn exists(local: &Local<'_>, name: &str) -> bool {
    sidecar_exists(local.home, name) || local.mux.list_panes().await.unwrap_or_default().iter().any(|p| p.session == name)
}

/// `<nonce>.<hmac>` com HMAC-SHA256(segredo interno, "hangar-voice:" + nonce), a conta de
/// `voice::machines::voice_signature`. Sem segredo nada vale.
pub fn voice_signature_valid(value: &[u8], secret: &str) -> bool {
    let Some(dot) = value.iter().position(|b| *b == b'.') else { return false };
    let (nonce, signature) = (&value[..dot], &value[dot + 1..]);
    if secret.is_empty() || nonce.is_empty() || signature.is_empty() { return false; }
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let mut message = b"hangar-voice:".to_vec();
    message.extend_from_slice(nonce);
    let expected: String = ring::hmac::sign(&key, &message).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    bool::from(expected.as_bytes().ct_eq(signature))
}

/// A chave no sidecar do Claude sem terminal (com `name` e `session_id`, como o `list_all` de lá) ou
/// no do Codex.
// ponytail: o Codex em preparação de transferência também conta; ele não chama o MCP antes de nascer.
fn by_key(home: &Path, key: &str) -> Option<String> {
    use crate::list::discover_other::sidecars;
    let (hangar, mut problems) = (home.join(".hangar"), crate::list::discover::Problems::default());
    let text = |m: &serde_json::Map<String, Value>, field: &str| m.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let claude = sidecars(&hangar.join("claude-headless"), "claude", &mut problems);
    let codex = sidecars(&hangar.join("codex-sessions"), "codex", &mut problems);
    claude.iter().filter(|(_, m)| text(m, "session_id").is_some()).chain(codex.iter())
        .find(|(_, m)| m.get("key").and_then(Value::as_str) == Some(key) && text(m, "name").is_some())
        .and_then(|(_, m)| text(m, "name"))
}

/// `~/.hangar/session-specs/<chave>.json`, que o Rust grava ao abrir (32 hex).
fn record_exists(home: &Path, key: &str) -> bool {
    crate::session_open::record::path(home, key).is_ok_and(|path| path.is_file())
}

fn sidecar_exists(home: &Path, name: &str) -> bool {
    let file = format!("{}.json", crate::list::discover_other::sanitize_session_name(name));
    ["claude-headless", "codex-sessions"].iter().any(|dir| home.join(".hangar").join(dir).join(&file).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, nonce: &str) -> String {
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        let tag = ring::hmac::sign(&key, format!("hangar-voice:{nonce}").as_bytes());
        format!("{nonce}.{}", tag.as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    #[test]
    fn voice_signature_matches_the_python_check() {
        assert!(voice_signature_valid(sign("s3", "0a1b").as_bytes(), "s3"));
        for bad in [sign("outro", "0a1b"), "sem-ponto".into(), "0a1b.".into(), ".abc".into(), format!("0a1b.é{}", "0".repeat(63))] {
            assert!(!voice_signature_valid(bad.as_bytes(), "s3"), "{bad}");
        }
        assert!(!voice_signature_valid(sign("", "0a1b").as_bytes(), ""), "sem segredo nenhuma assinatura vale");
        assert!(!voice_signature_valid(b"\xf1.\xff", "s3"));
    }
}
