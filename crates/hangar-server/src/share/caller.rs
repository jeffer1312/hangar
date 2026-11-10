//! Quem chama e o que o convidado alcança: as listas do `share_gate.py`, copiadas.
use axum::http::Method;

pub use hangar_core::guest::{Caller, GuestGrant, caller};
use super::store::Kind;

const LIST_ROUTES: &[(&str, &str)] = &[("GET", "/api/sessions"), ("GET", "/api/sessions/events")];
const GLOBAL_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/model-options"),
    ("GET", "/api/engines"),
    ("GET", "/api/harness/codex/opcoes"),
    ("POST", "/api/ditado/relimpar"),
    ("POST", "/api/pensamento/pt"),
    ("POST", "/api/tts"),
    ("POST", "/api/tts/narrar"),
];
const GLOBAL_PREFIXES: &[(&str, &str)] = &[("GET", "/api/tts/audio/")];
// Alcançam outra sessão do dono ou abrem janela na tela dele; `share` deixaria o convidado
// criar e revogar convites.
const BLOCKED: &[&str] = &["pair", "group", "pair-remote", "unpair-remote", "group-message", "then", "orq", "bastao",
    "open-terminal", "open-editor", "nav", "share", "pair-invite", "pair-accept", "pair-access"];
// Leituras do chat do nativo: quem só vê a sessão pareada não alcança arquivo, terminal nem git.
const PAIR_READ: &[&str] = &["", "events", "history", "commands", "plan-preview", "subagents", "uploads",
    "transcript-image"];
const PAIR_ROUTES: &[(&str, &str)] = &[("POST", "/api/pair/message"), ("DELETE", "/api/pair")];
const ANY_GUEST_ROUTES: &[(&str, &str)] = &[("POST", "/api/guest/attach")];

fn has(list: &[(&str, &str)], method: &str, path: &str) -> bool {
    list.iter().any(|(m, p)| *m == method && *p == path)
}

/// O Python casa as rotas com o `scope["path"]`, já decodificado (`unquote`, inválido vira U+FFFD).
fn decoded(path: &str) -> std::borrow::Cow<'_, str> {
    percent_encoding::percent_decode_str(path).decode_utf8_lossy()
}

/// Rota de sessão partida uma vez, no porteiro, e lida por quem vem depois (extensão do pedido).
/// `name` é o segmento cru decodificado sozinho, como o `Path` dos handlers: um `%2F` fica dentro
/// do nome. `session` conta o terminal `term-X/term` como X. `rest` é o resto decodificado inteiro
/// e partido em '/', como o Starlette casaria.
#[derive(Clone, Debug)]
pub struct SessionPath {
    pub name: String,
    pub session: String,
    pub rest: Vec<String>,
}

impl SessionPath {
    /// `None` fora de `/api/sessions/{name}` e na lista (`/api/sessions/events`).
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(5, '/').skip(1);
        if (parts.next(), parts.next()) != (Some("api"), Some("sessions")) {
            return None;
        }
        let name = decoded(parts.next()?).into_owned();
        if name == "events" {
            return None;
        }
        let tail = parts.next();
        // Sessão cujo nome começa com "term-" não vira outra: só o terminal `term-X/term`.
        let session = match name.strip_prefix("term-") {
            Some(inner) if tail == Some("term") => inner.to_owned(),
            _ => name.clone(),
        };
        let rest = tail.map(|t| decoded(t).split('/').map(str::to_owned).collect()).unwrap_or_default();
        Some(Self { name, session, rest })
    }
}

/// Sessão que a rota nomeia (o terminal `term-X/term` conta como X); `None` fora de sessão.
pub fn path_session(path: &str) -> Option<String> {
    SessionPath::parse(path).map(|p| p.session)
}

pub fn guest_allowed(method: &Method, raw: &str, grant: &GuestGrant) -> bool {
    allowed(method, raw, SessionPath::parse(raw).as_ref(), grant)
}

/// `guest_allowed` com a rota já partida pelo porteiro.
pub(crate) fn allowed(method: &Method, raw: &str, parsed: Option<&SessionPath>, grant: &GuestGrant) -> bool {
    let path = &*decoded(raw);
    let method = method.as_str();
    if has(LIST_ROUTES, method, path) || has(ANY_GUEST_ROUTES, method, path) {
        return true;
    }
    if has(PAIR_ROUTES, method, path) {
        return !grant.pair_roots.is_empty();
    }
    if has(GLOBAL_ROUTES, method, path) || GLOBAL_PREFIXES.iter().any(|(m, p)| *m == method && path.starts_with(p)) {
        // Acesso vindo de par, mesmo completo, não gasta crédito do dono (TTS, ditado, pensamento).
        return grant.invite;
    }
    let Some(SessionPath { name, session, rest }) = parsed else { return false };
    let Some(kind) = grant.sessions.get(session).copied() else { return false };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    if kind == Kind::Pair {
        let head = rest.first().copied().unwrap_or("");
        // Anexo é um arquivo só; a listagem de uploads fica fechada.
        return method == "GET" && !name.starts_with("term-") && PAIR_READ.contains(&head)
            && (head != "uploads" || rest.len() == 2);
    }
    if name.starts_with("term-") {
        return rest == ["term"];
    }
    // Fechar mata a sessão do dono; o convidado só para de acompanhar do lado dele.
    match rest.first() {
        None => method != "DELETE",
        Some(head) => !BLOCKED.contains(head),
    }
}

#[cfg(test)]
mod tests {
    use super::{GLOBAL_PREFIXES, GLOBAL_ROUTES};
    use std::collections::BTreeSet;

    /// Strings do array `export const <name>` em `share.ts`: o cliente só pede o que a porta deixa.
    fn ts_array(src: &str, name: &str) -> BTreeSet<String> {
        let start = src.find(&format!("export const {name}")).unwrap_or_else(|| panic!("{name} sumiu de share.ts"));
        let body = &src[start..];
        let body = &body[body.find("= [").expect("array") + 3..];
        let body = &body[..body.find("];").expect("fim do array")];
        body.split('\'').skip(1).step_by(2).map(str::to_owned).collect()
    }

    #[test]
    fn client_global_routes_match_the_guest_port() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/core/src/share.ts");
        let src = std::fs::read_to_string(path).expect("packages/core/src/share.ts");
        let paths: BTreeSet<String> = GLOBAL_ROUTES.iter().map(|(_, p)| (*p).to_owned()).collect();
        let prefixes: BTreeSet<String> = GLOBAL_PREFIXES.iter().map(|(_, p)| (*p).to_owned()).collect();
        assert_eq!(ts_array(&src, "INVITE_GLOBAL_PATHS"), paths);
        assert_eq!(ts_array(&src, "INVITE_GLOBAL_PREFIXES"), prefixes);
    }
}
