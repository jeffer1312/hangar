//! Release no GitHub: escolhe o pacote da plataforma, confere o sha256 do `checksums.txt` e só então grava o
//! programa em `bin/<versão>/`.
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ErrorCodes;

const MAX_DOWNLOAD: usize = 200 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const LATEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Github {
    pub api: String,
    pub download: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Network(String),
    UnsupportedPlatform,
    NoChecksum,
    Hash,
    BinaryMissing,
    Write(String),
}

/// Sistema e arquitetura com os nomes do Rust (`std::env::consts`): `linux`/`macos`/`windows`,
/// `x86_64`/`aarch64`. O nome publicado na release sai do [`Binary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

/// Como cada projeto escreve a arquitetura no nome do pacote: o CLIProxy publica `aarch64`, outros `arm64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchNames {
    pub x86_64: &'static str,
    pub aarch64: &'static str,
}

/// Um programa publicado em releases do GitHub como `<archive_prefix>_<versão>_<os>_<arch>.<tar.gz|zip>`
/// (zip só no Windows), com `checksums.txt` ao lado e o programa na raiz do pacote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binary {
    /// `dono/projeto`.
    pub repo: &'static str,
    pub archive_prefix: &'static str,
    /// Nome do programa sem `.exe`.
    pub program: &'static str,
    pub arch: ArchNames,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
}

impl Default for Github {
    fn default() -> Self {
        Self { api: "https://api.github.com".into(), download: "https://github.com".into() }
    }
}

impl Error {
    pub fn code(&self, codes: &ErrorCodes) -> &'static str {
        match self {
            Self::Network(_) => codes.download_failed,
            Self::UnsupportedPlatform => codes.no_asset,
            Self::NoChecksum => codes.no_checksum,
            Self::Hash => codes.hash,
            Self::BinaryMissing => codes.binary_missing,
            Self::Write(_) => codes.write_failed,
        }
    }
}

impl Platform {
    pub fn local() -> Self {
        Self { os: std::env::consts::OS, arch: std::env::consts::ARCH }
    }
}

impl Release {
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }
}

impl Binary {
    pub fn program_file(&self, platform: Platform) -> String {
        if platform.os == "windows" { format!("{}.exe", self.program) } else { self.program.to_owned() }
    }

    pub fn asset(&self, platform: Platform, version: &str) -> Result<String, Error> {
        let os = match platform.os {
            "linux" => "linux",
            "macos" => "darwin",
            "windows" => "windows",
            _ => return Err(Error::UnsupportedPlatform),
        };
        let arch = match platform.arch {
            "x86_64" => self.arch.x86_64,
            "aarch64" => self.arch.aarch64,
            _ => return Err(Error::UnsupportedPlatform),
        };
        let ext = if os == "windows" { "zip" } else { "tar.gz" };
        Ok(format!("{}_{version}_{os}_{arch}.{ext}", self.archive_prefix))
    }

    pub async fn latest(&self, github: &Github) -> Result<Release, Error> {
        let url = format!("{}/repos/{}/releases/latest", github.api, self.repo);
        let body = github_json(&url).await.map_err(Error::Network)?;
        let tag = body["tag_name"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Error::Network("release sem tag".into()))?;
        Ok(Release { tag: tag.to_owned() })
    }

    /// Baixa uma vez por versão; devolve o caminho do programa em `<dir>/bin/<versão>/`.
    pub async fn fetch(&self, github: &Github, release: &Release, platform: Platform, dir: &Path) -> Result<PathBuf, Error> {
        let member = self.program_file(platform);
        let dest = dir.join("bin").join(release.version()).join(&member);
        if ready(&dest) {
            return Ok(dest);
        }
        let asset = self.asset(platform, release.version())?;
        let base = format!("{}/{}/releases/download/{}", github.download, self.repo, release.tag);
        let checksums = String::from_utf8(download(&format!("{base}/checksums.txt")).await?).map_err(|_| Error::NoChecksum)?;
        let expected = checksum_for(&checksums, &asset).ok_or(Error::NoChecksum)?;
        let data = download(&format!("{base}/{asset}")).await?;
        let target = dest.clone();
        tokio::task::spawn_blocking(move || install(&data, &expected, &member, &target))
            .await
            .map_err(|e| Error::Write(e.to_string()))??;
        Ok(dest)
    }
}

/// `GET` de JSON na API do GitHub. A tela espera essa resposta (e as outras leituras esperam a trava dela):
/// rede caída não pode prender o prazo longo do download.
///
/// # Errors
///
/// O texto da falha de rede, de status ou de um corpo que não é JSON.
pub async fn github_json(url: &str) -> Result<serde_json::Value, String> {
    hangar_core::net::http_client()
        .timeout(LATEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hangar")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// A tag como semver, com ou sem o `v`.
pub fn tag_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim_start_matches('v')).ok()
}

/// `candidate` é uma versão maior que `installed`; tag que não é semver nunca é mais nova.
pub fn newer(candidate: &str, installed: &str) -> bool {
    matches!((tag_version(candidate), tag_version(installed)), (Some(c), Some(i)) if c > i)
}

fn client() -> Result<reqwest::Client, Error> {
    hangar_core::net::http_client().timeout(DOWNLOAD_TIMEOUT).build().map_err(|e| Error::Network(e.to_string()))
}

async fn download(url: &str) -> Result<Vec<u8>, Error> {
    let failed = |cause: String| Error::Network(format!("{url}: {cause}"));
    let mut response = client()?
        .get(url)
        .header("user-agent", "hangar")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| failed(e.to_string()))?;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| failed(e.to_string()))? {
        if data.len() + chunk.len() > MAX_DOWNLOAD {
            return Err(failed("maior que o esperado".into()));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

/// Linha `<sha256>  <nome>` do `checksums.txt` da release.
pub fn checksum_for(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == asset && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn extract(data: &[u8], member: &str) -> Result<Vec<u8>, Error> {
    let mut content = Vec::new();
    if data.starts_with(b"PK") {
        let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| Error::BinaryMissing)?;
        let mut file = archive.by_name(member).map_err(|_| Error::BinaryMissing)?;
        file.read_to_end(&mut content).map_err(|_| Error::BinaryMissing)?;
        return Ok(content);
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    for entry in archive.entries().map_err(|_| Error::BinaryMissing)? {
        let mut entry = entry.map_err(|_| Error::BinaryMissing)?;
        if entry.path().map_err(|_| Error::BinaryMissing)?.to_str() == Some(member) {
            entry.read_to_end(&mut content).map_err(|_| Error::BinaryMissing)?;
            return Ok(content);
        }
    }
    Err(Error::BinaryMissing)
}

/// O pacote inteiro é conferido antes de extrair: o que não bate nem chega ao disco.
pub fn install(data: &[u8], expected_sha256: &str, member: &str, dest: &Path) -> Result<(), Error> {
    if crate::hex::sha256(data) != expected_sha256 {
        return Err(Error::Hash);
    }
    let content = extract(data, member)?;
    // Mesmo critério do `ready`: programa vazio não conta como baixado.
    if content.is_empty() {
        return Err(Error::BinaryMissing);
    }
    let write = |e: std::io::Error| Error::Write(format!("{}: {e}", dest.display()));
    let parent = dest.parent().expect("destino tem pasta");
    std::fs::create_dir_all(parent).map_err(write)?;
    // Permissão posta antes de publicar: o programa só aparece no lugar final já executável e inteiro.
    let part = parent.join(format!(".{}.part", dest.file_name().expect("destino tem nome").to_string_lossy()));
    let published = (|| {
        std::fs::write(&part, &content)?;
        #[cfg(unix)]
        std::fs::set_permissions(&part, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        std::fs::rename(&part, dest)
    })();
    // Falhou no meio: o `.part` não fica para trás ocupando disco.
    if published.is_err() { let _ = std::fs::remove_file(&part); }
    published.map_err(write)
}

/// O programa já baixado serve: existe, não está vazio e (no Unix) é executável. Senão, baixa de novo.
fn ready(dest: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(dest) else { return false };
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 == 0 {
        return false;
    }
    meta.is_file() && meta.len() > 0
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::hex::sha256 as sha256_hex;

    /// Os nomes que o WhatsApp vai publicar: `arm64`, prefixo próprio.
    const WHATSAPP: Binary = Binary {
        repo: "dono/hangar-whatsapp",
        archive_prefix: "hangar-whatsapp",
        program: "hangar-whatsapp",
        arch: ArchNames { x86_64: "amd64", aarch64: "arm64" },
    };

    fn tar_gz(member: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, bytes) in [("README.md", b"leia".as_slice()), (member, content)] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn zipped(member: &str, content: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file(member, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn asset_names_follow_the_names_each_binary_publishes() {
        let p = |os, arch| Platform { os, arch };
        assert_eq!(WHATSAPP.asset(p("linux", "x86_64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_linux_amd64.tar.gz");
        assert_eq!(WHATSAPP.asset(p("linux", "aarch64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_linux_arm64.tar.gz");
        assert_eq!(WHATSAPP.asset(p("macos", "aarch64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_darwin_arm64.tar.gz");
        assert_eq!(WHATSAPP.asset(p("windows", "x86_64"), "0.1.0").unwrap(), "hangar-whatsapp_0.1.0_windows_amd64.zip", "zip só no Windows");
        assert_eq!(WHATSAPP.asset(p("freebsd", "x86_64"), "0.1.0"), Err(Error::UnsupportedPlatform));
        assert_eq!(WHATSAPP.asset(p("linux", "riscv64"), "0.1.0"), Err(Error::UnsupportedPlatform));
        assert_eq!(WHATSAPP.program_file(p("windows", "x86_64")), "hangar-whatsapp.exe");
        assert_eq!(WHATSAPP.program_file(p("macos", "aarch64")), "hangar-whatsapp");
        assert_eq!(Release { tag: "v0.1.0".into() }.version(), "0.1.0");
        assert_eq!(Release { tag: "0.1.0".into() }.version(), "0.1.0");
    }

    #[test]
    fn newer_compares_semver_not_text() {
        assert!(newer("v0.10.0", "v0.9.1"));
        assert!(newer("8.0.80", "v8.0.79"), "com ou sem o v");
        assert!(!newer("v0.1.0", "v0.1.0"));
        assert!(!newer("nightly", "v0.1.0"), "tag que não é semver");
    }

    #[test]
    fn error_codes_come_from_the_binary_set() {
        let codes = crate::error_codes!("whatsapp");
        assert_eq!(Error::Network("x".into()).code(&codes), "whatsapp_download_failed");
        assert_eq!(Error::UnsupportedPlatform.code(&codes), "whatsapp_sem_asset");
        assert_eq!(Error::Hash.code(&codes), "whatsapp_hash");
        assert_eq!(Error::Write("x".into()).code(&codes), "whatsapp_write_failed");
    }

    #[test]
    fn checksum_lines_match_the_exact_asset() {
        let text = "3f807ccc49ea0c3fce7240fb5890904e6953fbe42d20cb16d0e326b1387085c8  CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz\n\
                    89593f851a324fb3c5f1a9b8d58fd3d27269ae71a1e27ea8de7aba02c516180c  CLIProxyAPI_8.0.79_linux_amd64_no-plugin.tar.gz\n\
                    ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789 *hangar-whatsapp_0.1.0_windows_amd64.zip\n\
                    curto  hangar-whatsapp_0.1.0_linux_amd64.tar.gz\n";
        assert_eq!(checksum_for(text, "CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz").as_deref(),
            Some("3f807ccc49ea0c3fce7240fb5890904e6953fbe42d20cb16d0e326b1387085c8"));
        assert_eq!(checksum_for(text, "CLIProxyAPI_8.0.79_linux_amd64.tar.gz"), None, "o _no-plugin não serve");
        assert_eq!(checksum_for(text, "hangar-whatsapp_0.1.0_windows_amd64.zip").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"), "modo binário e maiúsculas");
        assert_eq!(checksum_for(text, "hangar-whatsapp_0.1.0_linux_amd64.tar.gz"), None, "hash que não tem 64 dígitos");
    }

    #[test]
    fn install_checks_the_hash_before_writing_and_extracts_both_formats() {
        let dir = tempfile::tempdir().unwrap();
        for (data, member) in [(tar_gz("hangar-whatsapp", b"programa"), "hangar-whatsapp"), (zipped("hangar-whatsapp.exe", b"programa"), "hangar-whatsapp.exe")] {
            let dest = dir.path().join("bin/0.1.0").join(member);
            assert_eq!(install(&data, &"0".repeat(64), member, &dest), Err(Error::Hash));
            assert!(!dest.exists(), "hash errado não grava nada");
            install(&data, &sha256_hex(&data), member, &dest).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), b"programa");
            assert!(ready(&dest), "publicado já pronto para rodar");
        }
        let data = tar_gz("outro", b"x");
        assert_eq!(install(&data, &sha256_hex(&data), "hangar-whatsapp", &dir.path().join("y")), Err(Error::BinaryMissing));
        let empty = tar_gz("hangar-whatsapp", b"");
        assert_eq!(install(&empty, &sha256_hex(&empty), "hangar-whatsapp", &dir.path().join("z")), Err(Error::BinaryMissing));
        assert!(!dir.path().join("z").exists(), "programa vazio não é publicado");
    }

    #[test]
    fn a_half_installed_program_is_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("hangar-whatsapp");
        assert!(!ready(&dest), "ausente");
        std::fs::write(&dest, b"").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(!ready(&dest), "vazio");
        std::fs::write(&dest, b"programa").unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
            assert!(!ready(&dest), "sem permissão de execução");
            std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        assert!(ready(&dest));
    }
}
