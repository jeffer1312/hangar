//! `hangar-server mcp-headers`: o `headersHelper` do MCP `hangar` no Claude Code. Roda a cada
//! conexão, herda o ambiente da sessão e imprime os cabeçalhos numa linha JSON. O token sai do `.env`,
//! nunca do ambiente da sessão.
use std::io::Write;
use std::process::ExitCode;

use crate::{Invocation, files};

/// A chave da sessão vence a do cano: a primeira variável não vazia de cada cabeçalho fica.
const HEADERS: [(&str, &str); 5] = [
    ("X-Hangar-Key", "CP_SESSION_KEY"),
    ("X-Hangar-Key", "HANGAR_CANO_KEY"),
    ("X-Hangar-Pane", "TMUX_PANE"),
    ("X-Hangar-Session", "CP_SESSION_NAME"),
    ("X-Hangar-Jev", "HANGAR_JEV"),
];

pub fn cli(args: &[String]) -> ExitCode {
    crate::cli(args, "mcp-headers", false, run)
}

pub fn run(invocation: &Invocation, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let path = files::env_file(invocation);
    let token = match files::read_env(&path) {
        Ok(env) => env.get("cp_auth_token").cloned().unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            let _ = writeln!(err, "hangar-mcp-headers: não consegui ler {}: {}", path.display(), files::os_error(&error, &path));
            return 1;
        }
    };
    // Sem cabeçalho o servidor responde 401 e o Claude cai no login OAuth: a falha tem de dizer o porquê.
    if token.is_empty() {
        let _ = writeln!(err, "hangar-mcp-headers: CP_AUTH_TOKEN ausente em {}", path.display());
        return 1;
    }
    let mut headers = vec![("Authorization", format!("Bearer {token}"))];
    headers.extend(identity(&invocation.vars));
    let pairs: Vec<String> = headers.iter().map(|(name, value)| format!("{}: {}", ascii_json(name), ascii_json(value))).collect();
    match writeln!(out, "{{{}}}", pairs.join(", ")) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Os cabeçalhos de identidade da sessão, pelo ambiente de quem chama (o `agy` passa o dele ao `mcp-stdio`).
pub fn identity(vars: &std::collections::BTreeMap<String, String>) -> Vec<(&'static str, String)> {
    let mut headers: Vec<(&'static str, String)> = Vec::new();
    for (header, var) in HEADERS {
        if let Some(value) = vars.get(var).filter(|value| !value.is_empty())
            && !headers.iter().any(|(name, _)| *name == header)
        {
            headers.push((header, value.clone()));
        }
    }
    headers
}

/// Uma string como o `json.dumps` do Python a escreve (`ensure_ascii`, fora do BMP em par de surrogates).
pub fn ascii_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' || c as u32 > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
