//! A identidade conserva os segmentos literais; URL de requisição normalizaria pontos e escapes.
use super::{Error, error};
use unicode_normalization::UnicodeNormalization;

pub(super) fn url_parts(raw: &str) -> Result<(String, String), Error> {
    let text: String = raw
        .trim_start_matches(|ch| ch <= ' ')
        .chars()
        .filter(|ch| !matches!(ch, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = text.as_str();
    if let Some((scheme, after)) = rest.split_once(':')
        && scheme
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte))
    {
        rest = after;
    }
    let Some(after) = rest.strip_prefix("//") else {
        return Ok((String::new(), String::new()));
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (netloc, path) = after.split_at(end);
    if netloc.contains('[') != netloc.contains(']') {
        return Err(error("ValueError", "Invalid IPv6 URL"));
    }
    let hostinfo = netloc.rsplit('@').next().unwrap_or("");
    let host = if netloc.contains('[') {
        let host = if let Some((before, bracketed)) = hostinfo.split_once('[') {
            let (host, port) = bracketed
                .split_once(']')
                .ok_or_else(|| error("ValueError", "Invalid IPv6 URL"))?;
            if !before.is_empty() || (!port.is_empty() && !port.starts_with(':')) {
                return Err(error("ValueError", "Invalid IPv6 URL"));
            }
            host
        } else {
            hostinfo.split(':').next().unwrap_or("")
        };
        if let Some(future) = host.strip_prefix('v') {
            if !future.split_once('.').is_some_and(|(version, body)| {
                !version.is_empty()
                    && version.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && !body.is_empty()
            }) {
                return Err(error("ValueError", "IPvFuture address is invalid"));
            }
        } else {
            let address = host.split_once('%').map_or(host, |(address, _)| address);
            match address.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(_)) => {
                    return Err(error("ValueError", "An IPv4 address cannot be in brackets"));
                }
                Ok(std::net::IpAddr::V6(_))
                    if !host.ends_with('%') && host.matches('%').count() <= 1 => {}
                _ => {
                    return Err(error(
                        "ValueError",
                        format!(
                            "{} does not appear to be an IPv4 or IPv6 address",
                            crate::model_picker::py_repr(host)
                        ),
                    ));
                }
            }
        }
        host
    } else {
        hostinfo.split(':').next().unwrap_or("")
    };
    if !netloc.is_ascii() {
        let stripped: String = netloc
            .chars()
            .filter(|ch| !matches!(ch, '@' | ':' | '#' | '?'))
            .collect();
        let normalized: String = stripped.nfkc().collect();
        if stripped != normalized && normalized.contains(['/', '?', '#', '@', ':']) {
            return Err(error(
                "ValueError",
                format!("netloc '{netloc}' contains invalid characters under NFKC normalization"),
            ));
        }
    }
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    Ok((host.to_lowercase(), path.into()))
}
