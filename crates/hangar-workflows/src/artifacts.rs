use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use num_bigint::BigInt;
use serde_json::{Value, json};

use hangar_core::pycompat::{decode_line, py, truthy};

static EMPTY_OBJECT: LazyLock<Value> = LazyLock::new(|| json!({}));

pub(crate) fn session_dir(transcript: &Path) -> PathBuf {
    transcript.with_file_name(transcript.file_stem().unwrap_or_default())
}

/// Ids viram nome de arquivo: separador ou `..` sairia da pasta da sessão.
pub(crate) fn safe_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && id.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub(crate) fn entries(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    // Arquivo que some durante a leitura não elimina os outros workflows.
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect()
}

pub(crate) fn matches_name(name: &str, prefix: &str, suffix: &str) -> bool {
    // O glob do pathlib segue a sensibilidade a caixa da plataforma.
    if cfg!(windows) {
        let name = name.to_lowercase();
        name.starts_with(&prefix.to_lowercase()) && name.ends_with(&suffix.to_lowercase())
    } else {
        name.starts_with(prefix) && name.ends_with(suffix)
    }
}

pub(crate) fn read_json(path: &Path) -> Option<Value> {
    decode_line(&fs::read(path).ok()?)
}

pub(crate) fn read_lines(path: &Path) -> impl Iterator<Item = Value> {
    let text = fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    // Mantém só o JSON da linha atual, mesmo quando o transcript do agente é grande.
    let origin = text.as_ptr() as usize;
    let lines = py::splitlines(&text)
        .into_iter()
        .map(|line| {
            let start = line.as_ptr() as usize - origin;
            start..start + line.len()
        })
        .collect::<Vec<_>>();
    lines
        .into_iter()
        .filter_map(move |range| decode_line(text[range].as_bytes()))
        .filter(Value::is_object)
}

pub(crate) fn field<'a>(object: &'a Value, key: &str) -> io::Result<&'a Value> {
    let object = object.as_object().ok_or_else(invalid_shape)?;
    Ok(object.get(key).unwrap_or(&Value::Null))
}

pub(crate) fn value_or(object: &Value, key: &str, fallback: Value) -> io::Result<Value> {
    let value = field(object, key)?;
    Ok(if truthy(value) {
        value.clone()
    } else {
        fallback
    })
}

pub(crate) fn mapping_or_empty(value: &Value) -> io::Result<&Value> {
    if !truthy(value) {
        return Ok(&EMPTY_OBJECT);
    }
    if value.is_object() {
        Ok(value)
    } else {
        Err(invalid_shape())
    }
}

pub(crate) fn items(value: &Value) -> io::Result<&[Value]> {
    if !truthy(value) {
        return Ok(&[]);
    }
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(invalid_shape)
}

pub(crate) fn number(value: BigInt) -> io::Result<Value> {
    serde_json::from_str(&value.to_string()).map_err(|_| invalid_shape())
}

pub(crate) fn invalid_shape() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "o artefato de workflow tem formato inválido",
    )
}

pub(crate) fn integer(value: &Value) -> io::Result<BigInt> {
    let text = match value {
        Value::Number(number) if number.is_f64() => {
            let number = number
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(invalid_shape)?;
            format!("{:.0}", number.trunc())
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => py::strip(text).to_owned(),
        Value::Bool(value) => return Ok(BigInt::from(u8::from(*value))),
        _ => return Err(invalid_shape()),
    };
    text.parse().map_err(|_| invalid_shape())
}
