//! O que a instalação e os passos de atualização pediam ao Python: as chaves do push
//! (`vapid-setup`), o token de quem não tem `openssl` (`install-token`) e o nome do Tailscale lido do
//! `tailscale status --json` (`tailscale-dns-name`). Cada comando tem um `run` sobre escritores, para
//! os testes, e um `cli` que o `hangar-server` chama.
use std::io;
use std::path::Path;

pub mod tailscale;
pub mod token;
pub mod vapid;

/// O texto do `OSError` do Python (`[Errno 13] Permission denied: '/x'`), que os instaladores e as
/// pessoas já leem; sem número de erro, só a mensagem.
pub fn os_error(error: &io::Error, path: &Path) -> String {
    let (message, number) = hangar_core::fs::os_error_parts(error);
    match number {
        Some(number) => format!("[Errno {number}] {message}: {}", py_repr(&path.to_string_lossy())),
        None => message,
    }
}

/// O `repr` de texto do Python: barra invertida dobrada, aspas simples, duplas quando o texto tem
/// simples e não duplas.
fn py_repr(text: &str) -> String {
    let text = text.replace('\\', "\\\\");
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "\\'"))
    }
}

/// O texto do `UnicodeDecodeError` do Python para um arquivo que não é UTF-8.
pub fn utf8_error(bytes: &[u8], error: &std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let byte = bytes.get(start).copied().unwrap_or_default();
    let (last, reason) = match error.error_len() {
        None => (bytes.len().saturating_sub(1), "unexpected end of data"),
        Some(len) => {
            let reason = if matches!(byte, 0x80..=0xC1 | 0xF5..=0xFF) { "invalid start byte" } else { "invalid continuation byte" };
            (start + len - 1, reason)
        }
    };
    if last > start {
        format!("'utf-8' codec can't decode bytes in position {start}-{last}: {reason}")
    } else {
        format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {start}: {reason}")
    }
}
