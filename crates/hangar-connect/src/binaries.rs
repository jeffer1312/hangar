//! `frpc` e Caddy de versão fixa: o pacote só entra em `bin/<versão>/` depois do hash conferido.
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use ring::digest;

use super::ConnectError;

const FRP_VERSION: &str = "0.71.0";
const CADDY_VERSION: &str = "2.11.6";
const MAX_DOWNLOAD: usize = 200 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub enum Digest {
    Sha256(String),
    Sha512(String),
}

impl Digest {
    fn matches(&self, data: &[u8]) -> bool {
        let (algorithm, expected) = match self {
            Self::Sha256(hex) => (&digest::SHA256, hex),
            Self::Sha512(hex) => (&digest::SHA512, hex),
        };
        let hash = digest::digest(algorithm, data);
        let bytes: &[u8] = hash.as_ref();
        bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>() == *expected
    }
}

#[derive(Clone)]
pub struct Package {
    pub version: &'static str,
    pub url: String,
    /// Caminho do programa dentro do pacote.
    pub member: String,
    /// Nome do programa em `bin/<versão>/`.
    pub file: String,
    pub digest: Digest,
}

fn frp_sha256(system: &str, arch: &str) -> Option<&'static str> {
    Some(match (system, arch) {
        ("linux", "amd64") => "84f27e39f11169f7adcef8e8b70c9329de17747b1f14dad9fb95eef5682ea716",
        ("linux", "arm64") => "f33c293c275d8fc68c654b6fba8f10b2551d6463d09a9fc9cffb7227eae82266",
        ("darwin", "amd64") => "1b1b4e2f1836e21e8733f1dddaacd4ed9ae67d7dbee39046b9d7b7eda6253637",
        ("darwin", "arm64") => "45be02b186860d375ed49a8941ae9569628a54bf14e67fc36b29c98c99dabcc6",
        ("windows", "amd64") => "9e5062e3e5cf07e67144a3a4acf175ef6a2486f3605dd6cf288bae34ab39819f",
        _ => return None,
    })
}

fn caddy_sha512(system: &str, arch: &str) -> Option<&'static str> {
    Some(match (system, arch) {
        ("linux", "amd64") => "422771007d505ea97efd1177a4905b2c1a471cd426668f2ace3bcda3d8e30b11f9b1610bfb02c6ad60f2a795f56124f2f5eec6409c17d5a0dd4c21a11375fb94",
        ("linux", "arm64") => "bd228ea44b6b95720a0c2d7b62886e99cb4a0b05356fe6e058c4a155618c913377b18e686839ae7fcc6c2c05aea2d549fa91f25aa3ef8d43cdead117259577ed",
        ("darwin", "amd64") => "7e53d356e09cbc7ac8d7959d4515a2e0adebd66548c31fefda7d6771665263c70a30e1b8d3f37633c75b05454b8f3a97be38b60ef6c4d84fb1b0e117c7972c5f",
        ("darwin", "arm64") => "8b08d5a25ab612946b18b82c78fc32b194e69e576dd120cdca44f467ded2d6cb6fcfe899f05a71774f04b454e5cc9adcf08f8545c8d560bd92079ce015c8bf2e",
        ("windows", "amd64") => "e2626405400f15131886a6dc03883962bdc960fdcb1cdd0882f28af8821c9b370c83fdc60cb8a4deaf27008166f0e080cc7208e699dc0ce15a534ae75cf53d93",
        _ => return None,
    })
}

/// `system` e `arch` nos nomes do frp. O Caddy chama o Mac de `mac`.
pub fn packages(system: &str, arch: &str) -> Result<[Package; 2], ConnectError> {
    let (Some(frp_hash), Some(caddy_hash)) = (frp_sha256(system, arch), caddy_sha512(system, arch)) else {
        return Err(ConnectError::new(StatusCode::BAD_REQUEST, "connect_platform_unsupported", format!("plataforma sem suporte: {system} {arch}")));
    };
    let (exe, ext) = if system == "windows" { (".exe", "zip") } else { ("", "tar.gz") };
    let frp = format!("frp_{FRP_VERSION}_{system}_{arch}");
    let caddy_system = if system == "darwin" { "mac" } else { system };
    let caddy = format!("caddy_{CADDY_VERSION}_{caddy_system}_{arch}.{ext}");
    Ok([
        Package {
            version: FRP_VERSION,
            url: format!("https://github.com/fatedier/frp/releases/download/v{FRP_VERSION}/{frp}.{ext}"),
            member: format!("{frp}/frpc{exe}"),
            file: format!("frpc{exe}"),
            digest: Digest::Sha256(frp_hash.to_owned()),
        },
        Package {
            version: CADDY_VERSION,
            url: format!("https://github.com/caddyserver/caddy/releases/download/v{CADDY_VERSION}/{caddy}"),
            member: format!("caddy{exe}"),
            file: format!("caddy{exe}"),
            digest: Digest::Sha512(caddy_hash.to_owned()),
        },
    ])
}

/// Os dois pacotes desta máquina, `frpc` primeiro.
pub fn local() -> Result<[Package; 2], ConnectError> {
    let system = match std::env::consts::OS { "macos" => "darwin", other => other };
    let arch = match std::env::consts::ARCH { "x86_64" => "amd64", "aarch64" => "arm64", other => other };
    packages(system, arch)
}

async fn download(url: &str) -> Result<Vec<u8>, ConnectError> {
    let failed = |cause: String| ConnectError::new(StatusCode::BAD_GATEWAY, "connect_download_failed", format!("não consegui baixar {url}: {cause}"));
    let client = hangar_core::net::http_client().timeout(DOWNLOAD_TIMEOUT).build().map_err(|error| failed(error.to_string()))?;
    let mut response = client.get(url).send().await.and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| failed(error.to_string()))? {
        if data.len() + chunk.len() > MAX_DOWNLOAD {
            return Err(failed("maior que o esperado".to_owned()));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

fn extract(data: &[u8], member: &str) -> Result<Vec<u8>, ConnectError> {
    let missing = || ConnectError::new(StatusCode::BAD_GATEWAY, "connect_binary_missing", format!("pacote sem {member}"));
    let mut content = Vec::new();
    if data.starts_with(b"PK") {
        let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| missing())?;
        let mut file = archive.by_name(member).map_err(|_| missing())?;
        file.read_to_end(&mut content).map_err(|_| missing())?;
        return Ok(content);
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    for entry in archive.entries().map_err(|_| missing())? {
        let mut entry = entry.map_err(|_| missing())?;
        if entry.path().map_err(|_| missing())?.to_str() == Some(member) {
            entry.read_to_end(&mut content).map_err(|_| missing())?;
            return Ok(content);
        }
    }
    Err(missing())
}

fn write_failed(dest: &Path, cause: &std::io::Error) -> ConnectError {
    ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_binary_write_failed", format!("não consegui gravar {}: {cause}", dest.display()))
}

fn make_executable(dest: &Path) -> Result<(), ConnectError> {
    #[cfg(unix)]
    std::fs::set_permissions(dest, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(|cause| write_failed(dest, &cause))?;
    #[cfg(not(unix))]
    let _ = dest;
    Ok(())
}

fn install(data: &[u8], package: &Package, dest: &Path) -> Result<(), ConnectError> {
    // No pacote inteiro, antes de extrair: o que não bate nem chega ao disco.
    if !package.digest.matches(data) {
        return Err(ConnectError::new(StatusCode::BAD_GATEWAY, "connect_binary_hash", format!("{} não bate com o hash esperado", package.url)));
    }
    let content = extract(data, &package.member)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|cause| write_failed(dest, &cause))?;
    }
    hangar_core::fs::atomic_write(dest, &content).map_err(|cause| write_failed(dest, &cause))?;
    make_executable(dest)
}

/// Baixa uma vez por versão. Devolve o caminho do programa.
pub async fn fetch(package: &Package, dir: &Path) -> Result<PathBuf, ConnectError> {
    let dest = dir.join("bin").join(package.version).join(&package.file);
    if dest.exists() {
        // Instalação interrompida entre gravar e dar permissão não pode travar a subida para sempre.
        make_executable(&dest)?;
        return Ok(dest);
    }
    let data = download(&package.url).await?;
    let (package, target) = (package.clone(), dest.clone());
    tokio::task::spawn_blocking(move || install(&data, &package, &target)).await
        .unwrap_or_else(|cause| Err(ConnectError::new(StatusCode::INTERNAL_SERVER_ERROR, "connect_binary_write_failed", cause.to_string())))?;
    Ok(dest)
}
