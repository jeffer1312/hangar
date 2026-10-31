//! Pacote em memória: o arquivo tar nunca escolhe um destino no sistema de arquivos.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

pub const ITEMS: [&str; 13] = [
    "claude_instructions",
    "claude_skills",
    "claude_agents",
    "claude_hooks",
    "claude_plugins",
    "claude_mcp",
    "claude_env",
    "claude_settings",
    "codex",
    "engines",
    "hangar_prefs",
    "claude_accounts",
    "hangar_appearance",
];
pub const MAX_BUNDLE: usize = 90 * 1024 * 1024;
pub const MAX_UNPACKED: u64 = 300 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Blob {
    pub data: Vec<u8>,
    pub mode: u32,
}
#[derive(Clone, Default, Debug)]
pub struct Bundle {
    pub items: serde_json::Map<String, Value>,
    pub files: BTreeMap<String, Blob>,
    pub warnings: serde_json::Map<String, Value>,
}

#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub params: Value,
    pub os_error: Option<i32>,
}
impl Error {
    pub fn new(code: &'static str, message: impl Into<String>, params: Value) -> Self {
        Self {
            code,
            message: message.into(),
            params,
            os_error: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("config_sync_invalid_bundle", message, json!({}))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        let mut error = Self::new("config_sync_io", e.to_string(), json!({}));
        error.os_error = e.raw_os_error();
        error
    }
}

pub fn sha256(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// O hash publicado já usa os separadores do json.dumps; alterar espaços quebraria a comparação.
pub fn python_json(value: &Value, sorted: bool) -> String {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<_> = map.iter().collect();
            if sorted {
                pairs.sort_by(|a, b| a.0.cmp(b.0));
            }
            format!(
                "{{{}}}",
                pairs
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap(),
                        python_json(value, sorted)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|v| python_json(v, sorted))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}
pub fn hash(value: &Value) -> String {
    sha256(python_json(value, true).as_bytes())
}
pub fn hashes(values: &Value, prefix: &str) -> Value {
    Value::Object(
        values
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (format!("{prefix}{k}"), Value::String(hash(v))))
            .collect(),
    )
}
pub fn warning(code: &str, params: Value) -> Value {
    json!({"code":code,"params":params})
}

impl Bundle {
    pub fn warn(&mut self, item: &str, code: &str, params: Value) {
        self.warnings
            .entry(item)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(warning(code, params));
    }
    pub fn item_bytes(&self, item: &str) -> usize {
        let prefix = format!("files/{item}/");
        let refs = self
            .items
            .get(item)
            .and_then(|i| i.get("refs"))
            .and_then(Value::as_object);
        self.files
            .iter()
            .filter(|(m, _)| {
                m.starts_with(&prefix)
                    || refs.is_some_and(|r| {
                        r.values().any(|v| v["member"].as_str() == Some(m.as_str()))
                    })
            })
            .map(|(_, b)| b.data.len())
            .sum()
    }
    pub fn manifest(&self) -> Value {
        let items: serde_json::Map<_,_> = ITEMS.into_iter().map(|item| {
            let data = self.items.get(item).cloned().unwrap_or_else(|| json!({}));
            (item.to_owned(), json!({"ok":self.items.contains_key(item),"hashes":data.get("hashes").cloned().unwrap_or_else(||json!({})),
                "labels":data.get("labels").cloned().unwrap_or_else(||json!({})),"descriptions":data.get("descriptions").cloned().unwrap_or_else(||json!({})),
                "bytes":self.item_bytes(item),"warnings":self.warnings.get(item).cloned().unwrap_or_else(||json!([]))}))
        }).collect();
        json!({"version":1,"items":items})
    }
    pub fn pack(&self) -> Result<Vec<u8>, Error> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut add = |name: &str, data: &[u8], mode: u32| -> Result<(), Error> {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(mode & 0o777);
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(&mut header, name, data)?;
            Ok(())
        };
        add(
            "manifest.json",
            python_json(
                &json!({"version":1,"items":self.items,"warnings":self.warnings}),
                false,
            )
            .as_bytes(),
            0o600,
        )?;
        for (name, blob) in &self.files {
            add(name, &blob.data, blob.mode)?;
        }
        Ok(archive.into_inner()?.finish()?)
    }
    pub fn unpack(raw: &[u8]) -> Result<Self, Error> {
        Self::unpack_with_entry_limit(raw, 100_000)
    }
    fn unpack_with_entry_limit(raw: &[u8], entry_limit: usize) -> Result<Self, Error> {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(raw)));
        let mut result = Self::default();
        let mut meta = Value::Null;
        let mut total = 0u64;
        let mut count = 0usize;
        static MEMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\A(?:manifest\.json|files/[a-z_]+/.+|refs/[0-9a-f]{40})\z").unwrap()
        });
        for entry in archive
            .entries()
            .map_err(|_| Error::invalid("pacote ilegível"))?
        {
            count += 1;
            if count > entry_limit {
                return Err(Error::invalid("pacote com entradas demais"));
            }
            let mut entry = entry.map_err(|_| Error::invalid("pacote ilegível"))?;
            let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
            if !entry.header().entry_type().is_file()
                || !MEMBER.is_match(&name)
                || name.split('/').any(|p| p == "..")
            {
                return Err(Error::new(
                    "config_sync_invalid_bundle",
                    format!("entrada inválida no pacote: {name}"),
                    json!({"entry":name}),
                ));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| Error::invalid("pacote grande demais aberto"))?;
            if total > MAX_UNPACKED {
                return Err(Error::invalid("pacote grande demais aberto"));
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|_| Error::invalid("pacote ilegível"))?;
            if name == "manifest.json" {
                meta = serde_json::from_slice(&data)
                    .map_err(|_| Error::invalid("manifesto ilegível"))?;
            } else {
                let mode = entry
                    .header()
                    .mode()
                    .map_err(|_| Error::invalid("pacote ilegível"))?;
                result.files.insert(
                    name,
                    Blob {
                        data,
                        mode: if mode == 0 { 0o644 } else { mode },
                    },
                );
            }
        }
        if !meta.is_object() {
            return Err(Error::invalid("pacote sem manifesto"));
        }
        if meta["version"] != 1 && meta["version"] != true && meta["version"].as_f64() != Some(1.0)
        {
            let version = match &meta["version"] {
                Value::Null => "None".to_owned(),
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            return Err(Error::new(
                "config_sync_version",
                "o pacote veio de um Hangar com outro formato",
                json!({"version":version}),
            ));
        }
        result.items = meta["items"].as_object().cloned().unwrap_or_default();
        result.warnings = meta["warnings"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, value)| value.is_array())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(result)
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn empty_entries_also_count_toward_the_archive_limit() {
        let mut bundle = Bundle::default();
        bundle.files.insert(
            "files/engines/a".into(),
            Blob {
                data: vec![],
                mode: 0o600,
            },
        );
        let raw = bundle.pack().unwrap();
        assert!(Bundle::unpack_with_entry_limit(&raw, 2).is_ok());
        assert_eq!(
            Bundle::unpack_with_entry_limit(&raw, 1)
                .unwrap_err()
                .message,
            "pacote com entradas demais"
        );
    }
}
