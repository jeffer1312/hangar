//! `install.json` `{tag,binary}`: a versão que o Hangar instalou e o programa que o serviço roda.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tag: String,
    pub binary: PathBuf,
}

/// A instalação que o registro aponta, só com o programa no disco.
pub fn read(record: &Path) -> Option<Installed> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(record).ok()?).ok()?;
    let tag = value["tag"].as_str().filter(|t| !t.is_empty())?.to_owned();
    let binary = PathBuf::from(value["binary"].as_str()?);
    binary.is_file().then_some(Installed { tag, binary })
}

/// Grava o registro, criando a pasta dele.
///
/// # Errors
///
/// Falha ao criar a pasta ou ao gravar o arquivo.
pub fn write(record: &Path, tag: &str, binary: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(record.parent().expect("registro tem pasta"))?;
    let body = serde_json::json!({"tag": tag, "binary": binary.to_string_lossy()});
    hangar_core::fs::atomic_write(record, body.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_counts_only_with_the_program_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("service/install.json");
        assert_eq!(read(&record), None);
        let binary = dir.path().join("service/bin/0.1.0/programa");
        write(&record, "v0.1.0", &binary).unwrap();
        assert_eq!(read(&record), None, "programa ausente");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        assert_eq!(read(&record), Some(Installed { tag: "v0.1.0".into(), binary: binary.clone() }));
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!({"tag": "v0.1.0", "binary": binary.to_string_lossy()}));
    }
}
