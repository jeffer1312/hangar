//! Pacote por versão do Hangar: manifesto, índice da release e extração conferida.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub commit: String,
    pub channel: String,
    pub protocol: u32,
    pub api: u32,
    pub os: String,
    pub dist_key: Option<String>,
    /// Caminho relativo com `/` → sha256 em hexadecimal minúsculo.
    pub files: BTreeMap<String, String>,
}

impl Manifest {
    /// Lê e valida: versão no formato vigente, caminhos relativos seguros e sha256 bem formado.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, ExtractError> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| ExtractError::ManifestInvalid(e.to_string()))?;
        m.version.parse::<Version>()?;
        for (path, sha) in &m.files {
            if !is_safe_relative(path) {
                return Err(ExtractError::ManifestInvalid(format!("unsafe path {path:?}")));
            }
            if sha.len() != 64 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err(ExtractError::ManifestInvalid(format!("bad sha256 for {path:?}")));
            }
        }
        Ok(m)
    }
}

/// Caminho relativo só com componentes normais, separado por `/`.
fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIndex {
    pub platforms: BTreeMap<String, PlatformEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub version: String,
    pub asset: String,
    pub sha256: String,
    pub protocol: u32,
    pub api: u32,
    pub commit: String,
    pub dist_key: Option<String>,
}

/// `VERSION` + número de commits: quatro números separados por ponto (`0.1.0.8207`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub [u64; 4]);

impl FromStr for Version {
    type Err = ExtractError;
    fn from_str(s: &str) -> Result<Version, ExtractError> {
        let bad = || ExtractError::ManifestInvalid(format!("bad version {s:?}"));
        let mut parts = [0u64; 4];
        let mut it = s.split('.');
        for slot in &mut parts {
            let p = it.next().ok_or_else(bad)?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            *slot = p.parse().map_err(|_| bad())?;
        }
        if it.next().is_some() {
            return Err(bad());
        }
        Ok(Version(parts))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

#[derive(Debug)]
pub enum ExtractError {
    ManifestInvalid(String),
    ShaMismatch(String),
    UnsafePath(String),
    /// A pasta final já existe e não confere; o atualizador decide apagar e extrair de novo.
    ExistingDiffers(String),
    RenameFailed(io::Error),
    Io(io::Error),
}

impl ExtractError {
    /// Código curto para o diário.
    pub fn code(&self) -> &'static str {
        match self {
            ExtractError::ManifestInvalid(_) => "package_manifest_invalid",
            ExtractError::ShaMismatch(_) => "package_sha_mismatch",
            ExtractError::UnsafePath(_) => "package_unsafe_path",
            ExtractError::ExistingDiffers(_) => "package_existing_differs",
            ExtractError::RenameFailed(_) => "package_rename_failed",
            ExtractError::Io(_) => "package_io",
        }
    }
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::ManifestInvalid(d)
            | ExtractError::ShaMismatch(d)
            | ExtractError::UnsafePath(d)
            | ExtractError::ExistingDiffers(d) => {
                write!(f, "{}: {d}", self.code())
            }
            ExtractError::RenameFailed(e) | ExtractError::Io(e) => write!(f, "{}: {e}", self.code()),
        }
    }
}

impl std::error::Error for ExtractError {}

pub const RENAME_TRIES: u32 = 20;
pub const RENAME_WAIT: Duration = Duration::from_millis(500);

const MANIFEST_MAX: u64 = 4 << 20;

/// Renomeia tentando de novo enquanto o Windows disser que o arquivo está em uso: o antivírus
/// segura por um instante os arquivos recém-extraídos.
pub fn rename_with_retry(
    from: &Path,
    to: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
    wait: &mut dyn FnMut(Duration),
) -> Result<(), ExtractError> {
    retry_transient(&mut || rename(from, to), wait).map_err(ExtractError::RenameFailed)
}

fn retry_transient(op: &mut dyn FnMut() -> io::Result<()>, wait: &mut dyn FnMut(Duration)) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_TRIES && is_transient(&e) => {
                wait(RENAME_WAIT);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Só no Windows: 5/32/33 = acesso negado, arquivo em uso, trecho travado. No Unix permissão
/// negada é permanente.
fn is_transient(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

fn read_manifest(reader: impl Read) -> Result<Manifest, ExtractError> {
    let mut bytes = Vec::new();
    reader.take(MANIFEST_MAX + 1).read_to_end(&mut bytes).map_err(ExtractError::Io)?;
    if bytes.len() as u64 > MANIFEST_MAX {
        return Err(ExtractError::ManifestInvalid(format!("{MANIFEST_FILE} over {MANIFEST_MAX} bytes")));
    }
    Manifest::parse(&bytes)
}

fn read_manifest_file(path: &Path) -> Result<Manifest, ExtractError> {
    read_manifest(fs::File::open(path).map_err(ExtractError::Io)?)
}

/// A pasta final que já existe só vale se for diretório de verdade (link não é seguido), com o
/// mesmo manifesto e todo sha batendo.
fn check_existing(dir: &Path, meta: &fs::Metadata, manifest: &Manifest) -> Result<(), ExtractError> {
    let differs = |why: String| ExtractError::ExistingDiffers(format!("{}: {why}", dir.display()));
    if !meta.is_dir() {
        return Err(differs("not a plain directory".into()));
    }
    let existing = read_manifest_file(&dir.join(MANIFEST_FILE)).map_err(|e| match e {
        ExtractError::Io(e) if e.kind() != io::ErrorKind::NotFound => ExtractError::Io(e),
        other => differs(other.to_string()),
    })?;
    if existing != *manifest {
        return Err(differs("another manifest".into()));
    }
    verify_tree(dir, manifest).map_err(|e| match e {
        ExtractError::ShaMismatch(d) => differs(d),
        other => other,
    })
}

pub fn extract(archive: &Path, versions_dir: &Path) -> Result<PathBuf, ExtractError> {
    extract_with(archive, versions_dir, &mut |a, b| fs::rename(a, b), &mut std::thread::sleep)
}

/// Extrai em `versions/<versao>.tmp/`, confere cada arquivo contra o manifesto e só então
/// renomeia para `versions/<versao>/`: pasta incompleta nunca tem o nome final.
pub fn extract_with(
    archive: &Path,
    versions_dir: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
    wait: &mut dyn FnMut(Duration),
) -> Result<PathBuf, ExtractError> {
    let manifest = read_archive_manifest(archive)?;
    let final_dir = versions_dir.join(&manifest.version);
    if let Ok(meta) = fs::symlink_metadata(&final_dir) {
        check_existing(&final_dir, &meta, &manifest)?;
        return Ok(final_dir);
    }

    let tmp = versions_dir.join(format!("{}.tmp", manifest.version));
    // Sobra de uma extração interrompida.
    if fs::symlink_metadata(&tmp).is_ok() {
        retry_transient(&mut || fs::remove_dir_all(&tmp), wait).map_err(ExtractError::Io)?;
    }
    fs::create_dir_all(&tmp).map_err(ExtractError::Io)?;
    let result = unpack(archive, &tmp)
        .and_then(|()| {
            let extracted = read_manifest_file(&tmp.join(MANIFEST_FILE))?;
            if extracted != manifest {
                return Err(ExtractError::ManifestInvalid("archive has more than one manifest".into()));
            }
            verify_tree(&tmp, &manifest)
        })
        .and_then(|()| rename_with_retry(&tmp, &final_dir, rename, wait));
    if let Err(e) = result {
        // Melhor esforço: o erro original é o que importa, e a próxima extração apaga a sobra.
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    Ok(final_dir)
}

fn open_archive(archive: &Path) -> Result<tar::Archive<GzDecoder<fs::File>>, ExtractError> {
    Ok(tar::Archive::new(GzDecoder::new(fs::File::open(archive).map_err(ExtractError::Io)?)))
}

fn read_archive_manifest(archive: &Path) -> Result<Manifest, ExtractError> {
    let mut ar = open_archive(archive)?;
    for entry in ar.entries().map_err(ExtractError::Io)? {
        let mut entry = entry.map_err(ExtractError::Io)?;
        if entry.header().entry_type().is_file() && entry_rel(&entry.path_bytes())?.as_deref() == Some(MANIFEST_FILE) {
            return read_manifest(&mut entry);
        }
    }
    Err(ExtractError::ManifestInvalid(format!("{MANIFEST_FILE} not found in archive")))
}

fn unpack(archive: &Path, dest: &Path) -> Result<(), ExtractError> {
    let mut ar = open_archive(archive)?;
    for entry in ar.entries().map_err(ExtractError::Io)? {
        let mut entry = entry.map_err(ExtractError::Io)?;
        let kind = entry.header().entry_type();
        if matches!(kind, tar::EntryType::XGlobalHeader | tar::EntryType::XHeader) {
            continue;
        }
        let Some(rel) = entry_rel(&entry.path_bytes())? else { continue };
        let target = dest.join(&rel);
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(ExtractError::Io)?;
        } else if kind.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(ExtractError::Io)?;
            }
            entry.unpack(&target).map_err(ExtractError::Io)?;
        } else {
            return Err(ExtractError::UnsafePath(format!("{rel}: entry type {kind:?} not allowed")));
        }
    }
    Ok(())
}

/// Caminho da entrada normalizado (sem `./`); `None` para a própria raiz.
fn entry_rel(raw: &[u8]) -> Result<Option<String>, ExtractError> {
    let unsafe_path = || ExtractError::UnsafePath(String::from_utf8_lossy(raw).into_owned());
    let s = std::str::from_utf8(raw).map_err(|_| unsafe_path())?;
    if s.starts_with('/') {
        return Err(unsafe_path());
    }
    let parts: Vec<&str> = s.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    if parts.is_empty() {
        return Ok(None);
    }
    let rel = parts.join("/");
    if !is_safe_relative(&rel) {
        return Err(unsafe_path());
    }
    Ok(Some(rel))
}

/// Os arquivos da pasta (fora o manifesto) têm que ser exatamente os do manifesto, com o mesmo sha.
fn verify_tree(root: &Path, manifest: &Manifest) -> Result<(), ExtractError> {
    let mut found = BTreeMap::new();
    collect_files(root, "", &mut found)?;
    found.remove(MANIFEST_FILE);
    for rel in found.keys() {
        if !manifest.files.contains_key(rel) {
            return Err(ExtractError::ShaMismatch(format!("{rel}: not in manifest")));
        }
    }
    for (rel, expected) in &manifest.files {
        let Some(path) = found.get(rel) else {
            return Err(ExtractError::ShaMismatch(format!("{rel}: missing")));
        };
        if sha256_file(path)? != *expected {
            return Err(ExtractError::ShaMismatch(rel.clone()));
        }
    }
    Ok(())
}

fn collect_files(dir: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) -> Result<(), ExtractError> {
    for entry in fs::read_dir(dir).map_err(ExtractError::Io)? {
        let entry = entry.map_err(ExtractError::Io)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| ExtractError::ShaMismatch(format!("{prefix}{name:?}: non-UTF-8 name")))?;
        let rel = format!("{prefix}{name}");
        let kind = entry.file_type().map_err(ExtractError::Io)?;
        if kind.is_dir() {
            collect_files(&entry.path(), &format!("{rel}/"), out)?;
        } else if kind.is_file() {
            out.insert(rel, entry.path());
        } else {
            return Err(ExtractError::ShaMismatch(format!("{rel}: not a regular file")));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, ExtractError> {
    let mut file = fs::File::open(path).map_err(ExtractError::Io)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(ExtractError::Io)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect())
}
