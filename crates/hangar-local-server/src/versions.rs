//! As versões do pacote em `~/.hangar/versions/` e para qual delas o `current` aponta.
use std::path::{Path, PathBuf};

use crate::link;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Versions {
    /// A versão para onde o `current` aponta; `None` com o link ausente ou quebrado e no modo
    /// desenvolvedor.
    pub current: Option<String>,
    /// Para onde a ação "apontar para" leva: a versão de antes da troca sem prova, senão a mais nova
    /// abaixo da atual com o servidor dentro. Nunca a que acabou de falhar. `None` no modo
    /// desenvolvedor.
    pub previous: Option<String>,
    /// Todas as pastas de versão, da mais nova para a mais velha.
    pub installed: Vec<String>,
    /// O `current` aponta para um checkout: sem ações de versão.
    pub dev_mode: bool,
}

pub fn versions_dir(hangar: &Path) -> PathBuf { hangar.join("versions") }

pub fn server_binary(version_dir: &Path) -> PathBuf {
    version_dir.join("bin").join(if cfg!(windows) { "hangar-server.exe" } else { "hangar-server" })
}

/// O número de quatro partes do pacote (`0.1.0.10824`), na regra do `hangar_package::Version`.
pub fn parse(name: &str) -> Option<[u64; 4]> {
    let mut parts = [0u64; 4];
    let mut it = name.split('.');
    for slot in &mut parts {
        let part = it.next().filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?;
        *slot = part.parse().ok()?;
    }
    it.next().is_none().then_some(parts)
}

pub const MANIFEST_FILE: &str = "manifest.json";

/// É pacote pelo mesmo critério do `diag.install_manifest()` do Python: `version`, `commit` e
/// `channel` presentes e não vazios. O resto não decide o modo: os dois lados têm que concordar,
/// senão um manifesto aceito por um e recusado pelo outro deixa a máquina sem quem a atualize.
pub fn package_manifest(dir: &Path) -> Option<serde_json::Value> {
    let value = std::fs::read(dir.join(MANIFEST_FILE)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())?;
    ["version", "commit", "channel"].iter()
        .all(|k| value.get(k).and_then(serde_json::Value::as_str).is_some_and(|s| !s.is_empty())).then_some(value)
}

pub fn is_package(dir: &Path) -> bool { package_manifest(dir).is_some() }

/// Pasta de versão inteira: nome de versão, pasta (não link) e o servidor dentro.
pub fn runnable(hangar: &Path, name: &str) -> bool {
    let dir = versions_dir(hangar).join(name);
    parse(name).is_some() && std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) && server_binary(&dir).is_file()
}

pub fn read(hangar: &Path) -> Versions {
    let versions = versions_dir(hangar);
    let mut installed: Vec<String> = std::fs::read_dir(&versions).map(|entries| entries.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().filter(|n| parse(n).is_some()).map(str::to_owned)).collect()).unwrap_or_default();
    installed.sort_by_key(|name| std::cmp::Reverse(parse(name)));

    let real_versions = link::real(&versions).ok();
    let (current, dev_mode) = match link::real(&link::current(hangar)) {
        Ok(real) => {
            let name = real.file_name().and_then(|n| n.to_str()).filter(|n| parse(n).is_some()).map(str::to_owned);
            let package = name.is_some() && real.parent() == real_versions.as_deref() && is_package(&real);
            if package { (name, false) } else { (None, true) }
        }
        // Ausente ou apontando para o nada: quebrado, não modo desenvolvedor.
        Err(_) => (None, false),
    };
    if dev_mode { return Versions { current, previous: None, installed, dev_mode }; }

    // A versão da troca sem prova é a que falhou; quem estava antes dela é a volta natural.
    let unproven = link::read_unproven(hangar).ok().flatten();
    let name_of = |path: &Path| path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let failed = unproven.as_ref().and_then(|u| name_of(&u.target));
    let usable = |name: &String| Some(name) != current.as_ref() && Some(name) != failed.as_ref() && runnable(hangar, name);
    let below = |name: &String| current.as_deref().and_then(parse).is_none_or(|current| parse(name).is_some_and(|v| v < current));
    let previous = unproven.as_ref().and_then(|u| u.previous.as_deref()).and_then(name_of).filter(|n| usable(n))
        .or_else(|| installed.iter().find(|n| below(n) && usable(n)).cloned());
    Versions { current, previous, installed, dev_mode }
}
