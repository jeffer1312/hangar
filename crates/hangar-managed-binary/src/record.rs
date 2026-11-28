//! `install.json` `{tag,binary}`: a versão que o Hangar instalou e o programa que o serviço roda.
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tag: String,
    pub binary: PathBuf,
}

/// A instalação que o registro aponta, só com o programa no disco.
pub fn read(record: &Path) -> Option<Installed> {
    read_checked(record).ok().flatten()
}

/// Ausência do registro ou do programa não é falha de leitura.
///
/// # Errors
/// Erro de E/S diferente de ausência, ou registro fora do formato (`InvalidData`).
pub fn read_checked(record: &Path) -> io::Result<Option<Installed>> {
    let raw = match std::fs::read(record) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "registro de instalação inválido");
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
    let tag = value["tag"].as_str().filter(|t| !t.is_empty()).ok_or_else(invalid)?.to_owned();
    let binary = PathBuf::from(value["binary"].as_str().filter(|b| !b.is_empty()).ok_or_else(invalid)?);
    match std::fs::metadata(&binary) {
        Ok(info) => Ok(info.is_file().then_some(Installed { tag, binary })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Grava o registro, criando a pasta dele.
///
/// # Errors
///
/// `InvalidInput` com caminho que não é UTF-8 (gravado trocado, a leitura procuraria outro programa);
/// falha ao criar a pasta ou ao gravar o arquivo.
pub fn write(record: &Path, tag: &str, binary: &Path) -> std::io::Result<()> {
    let binary = binary.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput,
        format!("caminho do programa não é UTF-8: {}", binary.display())))?;
    std::fs::create_dir_all(record.parent().expect("registro tem pasta"))?;
    let body = serde_json::json!({"tag": tag, "binary": binary});
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

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused_without_writing() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("service/install.json");
        let binary = dir.path().join(std::ffi::OsStr::from_bytes(b"bin/\xff/programa"));
        assert_eq!(write(&record, "v0.1.0", &binary).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        assert!(!record.exists(), "nada gravado");
    }
}
