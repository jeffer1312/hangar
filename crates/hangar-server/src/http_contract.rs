//! Decisão de roteamento sem efeitos: preserva a precedência das rotas publicadas.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Matched,
    NotFound,
    MethodNotAllowed(String),
    Redirect(String),
}

pub fn resolve(table: &[(&str, &str)], method: &str, decoded_path: &str) -> Decision {
    let mut first_method = None;
    for &(pattern, allowed_method) in table {
        if matches_path(pattern, decoded_path) {
            if method == allowed_method { return Decision::Matched; }
            first_method.get_or_insert(allowed_method);
        }
    }
    if let Some(method) = first_method { return Decision::MethodNotAllowed(method.into()); }
    let alternate = if decoded_path.ends_with('/') {
        decoded_path.trim_end_matches('/').to_owned()
    } else { format!("{decoded_path}/") };
    if decoded_path != "/" && table.iter().any(|(pattern, _)| matches_path(pattern, &alternate)) {
        Decision::Redirect(alternate)
    } else { Decision::NotFound }
}

fn matches_path(pattern: &str, path: &str) -> bool {
    let mut expected = pattern.split('/');
    let mut actual = path.split('/');
    loop {
        match (expected.next(), actual.next()) {
            (None, None) => return true,
            (Some(pattern), Some(component)) => {
                if pattern.starts_with('{') && pattern.ends_with('}') && pattern.len() > 2 {
                    if pattern.ends_with(":path}") { return expected.next().is_none(); }
                    if component.is_empty() { return false; }
                } else if pattern != component { return false; }
            }
            _ => return false,
        }
    }
}
