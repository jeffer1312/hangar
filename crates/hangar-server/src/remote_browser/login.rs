//! O login próprio é recusado antes de contar um palpite de senha do dono.
use axum::http::{HeaderMap, header};
use std::{collections::HashMap, sync::LazyLock};

fn latin1(bytes: &[u8]) -> String { bytes.iter().map(|byte| char::from(*byte)).collect() }

fn unquote(raw: &str) -> String {
    let Some(raw) = raw.strip_prefix('"').and_then(|raw| raw.strip_suffix('"')) else { return raw.into() };
    let mut chars = raw.chars().peekable();
    let mut value = String::new();
    while let Some(character) = chars.next() {
        if character != '\\' { value.push(character); continue; }
        let Some(next) = chars.next() else { break };
        let mut rest = chars.clone();
        let octal = [Some(next), rest.next(), rest.next()];
        if matches!(next, '0'..='3') && octal.iter().all(|c| c.is_some_and(|c| matches!(c, '0'..='7'))) {
            let code = octal.into_iter().flatten().fold(0, |value, digit| value * 8 + digit.to_digit(8).unwrap());
            value.push(char::from_u32(code).unwrap());
            chars.next(); chars.next();
        } else { value.push(next); }
    }
    value
}

fn cookies(raw: &str) -> HashMap<String, String> {
    // A gramática é a de http.cookies.SimpleCookie, distinta do parser HTTP do Starlette.
    static COOKIE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
        r#"^[ \t\n\r\x0b\x0c]*(?P<key>[A-Za-z0-9_!#%&'~`><@,:/\$\*\+\-\.\^\|\)\(\?\}\{\=]+?)(?:[ \t\n\r\x0b\x0c]*=[ \t\n\r\x0b\x0c]*(?P<val>"(?:[^\\"]|\\.)*"|(?:[A-Za-z0-9_]{3,6}day|[A-Za-z0-9_]{3}),[ \t\n\r\x0b\x0c][A-Za-z0-9_ \t\n\r\x0b\x0c-]{9,11}[ \t\n\r\x0b\x0c][0-9:]{8}[ \t\n\r\x0b\x0c]GMT|[A-Za-z0-9_!#%&'~`><@,:/\$\*\+\-\.\^\|\)\(\?\}\{\=\[\]]*))?[ \t\n\r\x0b\x0c]*(?:[ \t\n\r\x0b\x0c]+|;|$)"#,
    ).expect("gramática de cookie"));
    const RESERVED: &[&str] = &["expires", "path", "comment", "domain", "max-age", "secure", "httponly", "version", "samesite", "partitioned"];
    let mut jar = HashMap::new();
    let mut tail = raw;
    while let Some(capture) = COOKIE.captures(tail) {
        tail = &tail[capture.get(0).unwrap().end()..];
        let key = &capture["key"];
        let value = capture.name("val").map(|value| value.as_str());
        let lower = key.to_ascii_lowercase();
        if key.starts_with('$') {
            if jar.is_empty() { continue; }
            if !RESERVED.contains(&key[1..].to_ascii_lowercase().as_str()) { return HashMap::new(); }
        } else if RESERVED.contains(&lower.as_str()) {
            if jar.is_empty() || (value.is_none() && !["secure", "httponly", "partitioned"].contains(&lower.as_str())) { return HashMap::new(); }
        } else {
            let Some(value) = value else { return HashMap::new() };
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~:".contains(c)) { return HashMap::new(); }
            jar.insert(key.to_owned(), unquote(value));
        }
        if tail.is_empty() { break; }
    }
    jar
}

pub(super) fn token(headers: &HeaderMap, query: Option<&str>, https: bool) -> String {
    if let Some(raw) = headers.get_all(header::AUTHORIZATION).iter().last() {
        if let Some(value) = latin1(raw.as_bytes()).strip_prefix("Bearer ") { return value.to_owned(); }
    }
    if let Some(value) = query.and_then(|query| form_urlencoded::parse(query.as_bytes())
        .find(|(key, value)| key == "token" && !value.is_empty()).map(|(_, value)| value.into_owned())) { return value; }
    let raw = headers.get_all(header::COOKIE).iter().last().map(|value| latin1(value.as_bytes())).unwrap_or_default();
    let jar = cookies(&raw);
    jar.get("__Host-cp_token").or_else(|| if https { None } else { jar.get("cp_token") }).cloned().unwrap_or_default()
}

pub(super) async fn refused(st: &crate::routes::AppState, headers: &HeaderMap, query: Option<&str>, https: bool) -> bool {
    let token = token(headers, query, https);
    if token.is_empty() || st.auth.token_matches(Some(token.as_bytes())) { return false; }
    let base = st.accounts.env.claude_base.clone();
    match tokio::task::spawn_blocking(move || crate::session_open::server::login_guest(&base, token.as_bytes()).is_some()).await {
        Ok(guest) => guest,
        Err(_) => { tracing::warn!(code="browser_login_lookup_failed", "leitura do login de convidado não terminou"); false }
    }
}
