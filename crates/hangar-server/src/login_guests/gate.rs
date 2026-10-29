//! Política do login próprio: o dono da sessão e a permissão vêm do cadastro local.
const GLOBAL: &[(&str, &str)] = &[
    ("GET", "/api/sessions"), ("GET", "/api/sessions/events"),
    ("GET", "/api/model-options"), ("GET", "/api/engines"),
    ("GET", "/api/harness/codex/opcoes"), ("POST", "/api/ditado/relimpar"),
    ("POST", "/api/pensamento/pt"), ("POST", "/api/tts"), ("POST", "/api/tts/narrar"),
    ("POST", "/api/sessions"), ("GET", "/api/sessions/creation-progress"),
    ("GET", "/api/fs/roots"), ("GET", "/api/fs/scan"), ("GET", "/api/fs/branches"),
    ("GET", "/api/providers"), ("GET", "/api/claude-configs"),
    ("GET", "/api/codex-contas"), ("GET", "/api/cotas"), ("GET", "/api/me"),
    ("POST", "/api/diag"),
];
const BLOCKED: &[&str] = &[
    "pair", "pair-remote", "unpair-remote", "group-message", "then", "orq", "bastao",
    "open-terminal", "open-editor", "nav", "share", "pair-invite", "pair-accept",
    "pair-access", "nav-remoto", "navegador",
];
pub fn allowed(method: &str, path: &str, visible: impl Fn(&str)->bool) -> bool {
    if GLOBAL.contains(&(method,path)) || (method == "GET" && path.starts_with("/api/tts/audio/")) {
        return true;
    }
    let Some(tail) = path.strip_prefix("/api/sessions/") else {return false};
    let (name, rest) = tail.split_once('/').unwrap_or((tail,""));
    if let Some(owner) = name.strip_prefix("term-").filter(|_|rest == "term") {
        return visible(owner);
    }
    if BLOCKED.contains(&rest.split('/').next().unwrap_or("")) {return false;}
    visible(name)
}
