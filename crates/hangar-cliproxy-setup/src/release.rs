//! Release do CLIProxyAPI no GitHub: escolhe o pacote da plataforma, confere o sha256 do
//! `checksums.txt` e só então grava o programa em `bin/<versão>/`.
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "router-for-me/CLIProxyAPI";
const MAX_DOWNLOAD: usize = 200 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const LATEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Github {
    pub api: String,
    pub download: String,
}

impl Default for Github {
    fn default() -> Self {
        Self { api: "https://api.github.com".into(), download: "https://github.com".into() }
    }
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

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Network(_) => "cliproxy_download_failed",
            Self::UnsupportedPlatform => "cliproxy_sem_asset",
            Self::NoChecksum => "cliproxy_sem_checksum",
            Self::Hash => "cliproxy_hash",
            Self::BinaryMissing => "cliproxy_binary_missing",
            Self::Write(_) => "cliproxy_write_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Platform {
    /// Nomes como o projeto publica: `darwin`, `amd64`, `aarch64`.
    pub fn local() -> Self {
        let os = match std::env::consts::OS { "macos" => "darwin", other => other };
        let arch = match std::env::consts::ARCH { "x86_64" => "amd64", other => other };
        Self { os, arch }
    }

    pub fn binary(&self) -> &'static str {
        if self.os == "windows" { "cli-proxy-api.exe" } else { "cli-proxy-api" }
    }

    /// O pacote com plugins; o `_no-plugin` só existe para algumas plataformas.
    pub fn asset(&self, version: &str) -> Result<String, Error> {
        if !matches!(self.os, "linux" | "darwin" | "windows") || !matches!(self.arch, "amd64" | "aarch64") {
            return Err(Error::UnsupportedPlatform);
        }
        let ext = if self.os == "windows" { "zip" } else { "tar.gz" };
        Ok(format!("CLIProxyAPI_{version}_{}_{}.{ext}", self.os, self.arch))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
}

impl Release {
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }
}

fn client() -> Result<reqwest::Client, Error> {
    hangar_core::net::http_client().timeout(DOWNLOAD_TIMEOUT).build().map_err(|e| Error::Network(e.to_string()))
}

pub async fn latest(github: &Github) -> Result<Release, Error> {
    let url = format!("{}/repos/{REPO}/releases/latest", github.api);
    let body: serde_json::Value = client()?
        .get(&url)
        // A tela espera essa resposta (e as outras leituras esperam a trava dela): rede caída não pode prender 300 s.
        .timeout(LATEST_TIMEOUT)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hangar")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| Error::Network(e.to_string()))?
        .json()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let tag = body["tag_name"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Error::Network("release sem tag".into()))?;
    Ok(Release { tag: tag.to_owned() })
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

fn sha256_hex(data: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, data).as_ref().iter().map(|b| format!("{b:02x}")).collect()
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
    if sha256_hex(data) != expected_sha256 {
        return Err(Error::Hash);
    }
    let content = extract(data, member)?;
    let write = |e: std::io::Error| Error::Write(format!("{}: {e}", dest.display()));
    let parent = dest.parent().expect("destino tem pasta");
    std::fs::create_dir_all(parent).map_err(write)?;
    // Permissão posta antes de publicar: o programa só aparece no lugar final já executável e inteiro.
    let part = parent.join(format!(".{}.part", dest.file_name().expect("destino tem nome").to_string_lossy()));
    std::fs::write(&part, &content).map_err(write)?;
    #[cfg(unix)]
    std::fs::set_permissions(&part, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(write)?;
    std::fs::rename(&part, dest).map_err(write)?;
    Ok(())
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

/// Baixa uma vez por versão; devolve o caminho do programa em `<dir>/bin/<versão>/`.
pub async fn fetch(github: &Github, release: &Release, platform: Platform, dir: &Path) -> Result<PathBuf, Error> {
    let dest = dir.join("bin").join(release.version()).join(platform.binary());
    if ready(&dest) {
        return Ok(dest);
    }
    let asset = platform.asset(release.version())?;
    let base = format!("{}/{REPO}/releases/download/{}", github.download, release.tag);
    let checksums = String::from_utf8(download(&format!("{base}/checksums.txt")).await?).map_err(|_| Error::NoChecksum)?;
    let expected = checksum_for(&checksums, &asset).ok_or(Error::NoChecksum)?;
    let data = download(&format!("{base}/{asset}")).await?;
    let (target, member) = (dest.clone(), platform.binary());
    tokio::task::spawn_blocking(move || install(&data, &expected, member, &target))
        .await
        .map_err(|e| Error::Write(e.to_string()))??;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
    fn asset_names_follow_the_published_release() {
        let linux = Platform { os: "linux", arch: "amd64" };
        assert_eq!(linux.asset("8.0.79").unwrap(), "CLIProxyAPI_8.0.79_linux_amd64.tar.gz");
        assert_eq!(Platform { os: "darwin", arch: "aarch64" }.asset("8.0.79").unwrap(), "CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz");
        let windows = Platform { os: "windows", arch: "amd64" };
        assert_eq!((windows.asset("8.0.79").unwrap().as_str(), windows.binary()), ("CLIProxyAPI_8.0.79_windows_amd64.zip", "cli-proxy-api.exe"));
        assert_eq!(Platform { os: "freebsd", arch: "amd64" }.asset("1"), Err(Error::UnsupportedPlatform));
        assert_eq!(Release { tag: "v8.0.79".into() }.version(), "8.0.79");
    }

    #[test]
    fn checksum_lines_match_the_exact_asset() {
        let text = "3f807ccc49ea0c3fce7240fb5890904e6953fbe42d20cb16d0e326b1387085c8  CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz\n\
                    89593f851a324fb3c5f1a9b8d58fd3d27269ae71a1e27ea8de7aba02c516180c  CLIProxyAPI_8.0.79_linux_amd64_no-plugin.tar.gz\n";
        assert_eq!(checksum_for(text, "CLIProxyAPI_8.0.79_darwin_aarch64.tar.gz").as_deref(),
            Some("3f807ccc49ea0c3fce7240fb5890904e6953fbe42d20cb16d0e326b1387085c8"));
        assert_eq!(checksum_for(text, "CLIProxyAPI_8.0.79_linux_amd64.tar.gz"), None, "o _no-plugin não serve");
    }

    #[test]
    fn install_checks_the_hash_before_writing_and_extracts_both_formats() {
        let dir = tempfile::tempdir().unwrap();
        for (data, member) in [(tar_gz("cli-proxy-api", b"programa"), "cli-proxy-api"), (zipped("cli-proxy-api.exe", b"programa"), "cli-proxy-api.exe")] {
            let dest = dir.path().join("bin/8.0.79").join(member);
            assert_eq!(install(&data, &"0".repeat(64), member, &dest), Err(Error::Hash));
            assert!(!dest.exists(), "hash errado não grava nada");
            install(&data, &sha256_hex(&data), member, &dest).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), b"programa");
            assert!(ready(&dest), "publicado já pronto para rodar");
        }
        let data = tar_gz("outro", b"x");
        assert_eq!(install(&data, &sha256_hex(&data), "cli-proxy-api", &dir.path().join("y")), Err(Error::BinaryMissing));
    }

    #[test]
    fn a_half_installed_program_is_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cli-proxy-api");
        assert!(!ready(&dest), "ausente");
        std::fs::write(&dest, b"").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(!ready(&dest), "vazio");
        std::fs::write(&dest, b"programa").unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
            assert!(!ready(&dest), "sem permissão de execução, como o `atomic_write` deixava");
            std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        assert!(ready(&dest));
    }
}
