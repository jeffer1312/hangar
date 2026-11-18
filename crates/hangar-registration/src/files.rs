//! Leitura do `.env`, erros de disco com o texto do Python e gravação que mantém o link.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::Invocation;

/// O `.env` do servidor: `~/.hangar/config/.env` quando existe; senão o da `backend/` da cópia
/// (máquina não migrada).
pub(crate) fn env_file(invocation: &Invocation) -> PathBuf {
    let config = invocation.home.join(".hangar").join("config").join(".env");
    if config.exists() {
        return config;
    }
    invocation.root().map_or(config, |root| root.join("backend").join(".env"))
}

/// As chaves do `.env` (em minúsculas, a última linha vale), pelo leitor da subida do servidor.
pub(crate) fn read_env(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path)?;
    Ok(hangar_core::user_data::parse_env_file(&text).into_iter().collect())
}

/// O texto de um `OSError` do Python (`[Errno 13] Permission denied: '<caminho>'`): a frase que o
/// instalador já mostrava continua a mesma.
pub(crate) fn os_error(error: &io::Error, path: &Path) -> String {
    let Some(code) = error.raw_os_error() else {
        return format!("{error}: '{}'", path.display());
    };
    let text = error.to_string();
    let detail = text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text);
    format!("[Errno {code}] {detail}: '{}'", path.display())
}

/// O arquivo de verdade atrás de uma cadeia de links: gravar nele deixa o link como link.
pub(crate) fn through_links(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&current) {
                Ok(target) if target.is_absolute() => current = target,
                Ok(target) => current = current.parent().map_or(target.clone(), |dir| dir.join(&target)),
                Err(_) => break,
            },
            _ => break,
        }
    }
    current
}

/// Temporário ao lado, nascido 0600, no modo pedido e então no lugar do alvo. O erro sai com o
/// caminho que falhou, no texto do Python.
pub(crate) fn replace(target: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!("{name}.hangar-novo"));
    let written = (|| {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        set_mode(&tmp, mode)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(os_error(&error, &tmp));
    }
    std::fs::rename(&tmp, target).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        os_error(&error, target)
    })
}

#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(crate) fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn mode_of(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).ok().map(|meta| meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
pub(crate) fn mode_of(_path: &Path) -> Option<u32> {
    Some(0o600)
}

/// `open(f, encoding="utf-8").read()`: `\r\n` e `\r` viram `\n`.
pub(crate) fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Uma string básica do TOML: o `json.dumps(s, ensure_ascii=False)` de antes, mais o DEL (U+007F), que
/// o JSON deixa cru e o TOML recusa.
pub fn toml_string(text: &str) -> String {
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
            c if c < ' ' || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// O nome do tipo do Python para um valor do JSON, nas frases de `AttributeError` de antes.
pub(crate) fn py_type(value: &serde_json::Value) -> &'static str {
    use serde_json::Value;
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        Value::Number(number) if !number.to_string().contains(['.', 'e', 'E']) => "int",
        Value::Number(_) => "float",
        Value::String(_) if hangar_core::orq_codec::codec::infinite(value) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
